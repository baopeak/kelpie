//! `kelpied bench-echo` — keystroke-to-echo latency through a running daemon.
//!
//! Starts one session (`--cmd`, default the platform shell), attaches to it interactively the
//! way a visible terminal does, waits for it to go quiet, then types `--keys` printable
//! characters one at a time and times each until the daemon's screen shows it. `--load N` adds
//! N background sessions that each spin one core (`kelpied bench-burn`), the way agents running
//! builds and tests compete for the CPU; `--external N` spins N cores outside the daemon, like
//! other programs on the machine.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use kelpie_proto::Frame;
use serde_json::json;

use crate::bench::pct;
use crate::client::Client;

struct Opts {
    /// Session kind: `custom` runs `--cmd`; an agent kind (`claude`) starts the agent exactly as
    /// the app does (its preset, hooks, priority as an agent).
    kind: String,
    cmd: Vec<String>,
    cwd: String,
    keys: usize,
    load: usize,
    external: usize,
    /// Pause after each echo has gone quiet, roughly a fast typist's pace.
    gap_ms: u64,
    settle_ms: u64,
    /// Print the final screen.
    show: bool,
    /// Keys typed after start-up, before measuring, e.g. `down enter` to answer a folder-trust
    /// prompt (`down`, `up`, `enter`, `esc`; anything else is typed as text).
    pre: Option<String>,
    /// Text the screen shows once the program takes input (e.g. Claude's `❯`).
    ready: Option<String>,
    /// `KEY=VALUE` pairs for the measured program (`--env` may repeat).
    env: serde_json::Map<String, serde_json::Value>,
}

fn parse(args: &[String]) -> Opts {
    let shell = if cfg!(windows) { vec!["cmd.exe".to_string()] } else { vec!["sh".to_string()] };
    let mut o = Opts {
        kind: "custom".into(),
        cmd: shell,
        cwd: std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        keys: 40,
        load: 0,
        external: 0,
        gap_ms: 120,
        settle_ms: 1500,
        show: false,
        pre: None,
        ready: None,
        env: serde_json::Map::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--show" {
            o.show = true;
            continue;
        }
        let Some(v) = it.next() else { break };
        match a.as_str() {
            "--cmd" => o.cmd = v.split(' ').map(str::to_string).collect(),
            "--kind" => o.kind = v.clone(),
            "--cwd" => o.cwd = v.clone(),
            "--keys" => o.keys = v.parse().unwrap_or(o.keys),
            "--load" => o.load = v.parse().unwrap_or(0),
            "--external" => o.external = v.parse().unwrap_or(0),
            "--gap" => o.gap_ms = v.parse().unwrap_or(o.gap_ms),
            "--settle" => o.settle_ms = v.parse().unwrap_or(o.settle_ms),
            "--pre" => o.pre = Some(v.clone()),
            "--ready" => o.ready = Some(v.clone()),
            "--env" => {
                if let Some((k, val)) = v.split_once('=') {
                    o.env.insert(k.to_string(), val.into());
                }
            }
            _ => {}
        }
    }
    o
}

/// Child side: keep one core busy until killed.
pub fn burn() {
    let mut x: u64 = 1;
    loop {
        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
    }
}

fn key_bytes(key: &str) -> &[u8] {
    match key {
        "down" => b"\x1b[B",
        "up" => b"\x1b[A",
        "enter" => b"\r",
        "esc" => b"\x1b",
        text => text.as_bytes(),
    }
}

/// The measured session: RPCs and keys go over one connection; its output is read on a second
/// one by its own task, so waiting with a timeout never cuts a frame in half.
struct Echo {
    rpc: Client,
    handle: u32,
    /// One message per output frame of `handle`.
    output: tokio::sync::mpsc::UnboundedReceiver<()>,
}

impl Echo {
    async fn attach(handle: u32) -> Result<Echo> {
        let mut sub = Client::connect_user().await?;
        sub.call("session.attach", json!({"handle": handle, "interactive": true})).await?;
        let (tx, output) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok(frame) = sub.next_frame().await {
                if matches!(frame, Frame::Output { handle: h, .. } if h == handle) && tx.send(()).is_err() {
                    break;
                }
            }
        });
        Ok(Echo { rpc: Client::connect_user().await?, handle, output })
    }

    async fn type_keys(&mut self, bytes: &[u8]) -> Result<()> {
        self.rpc.send(&Frame::Input { handle: self.handle, data: bytes.to_vec() }).await
    }

    async fn screen(&mut self) -> Result<String> {
        let r = self.rpc.call("session.read", json!({"handle": self.handle})).await?;
        Ok(r["text"].as_str().unwrap_or_default().to_string())
    }

    /// Time until the screen shows `expected`, checked on every output frame — the moment the
    /// echo is on screen, not merely the first byte of a redraw.
    async fn wait_for_screen(&mut self, expected: &str, max: Duration) -> Result<Option<Duration>> {
        let sent = Instant::now();
        let end = sent + max;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.output.recv()).await {
                Err(_) => return Ok(None),
                Ok(None) => bail!("output stream closed"),
                Ok(Some(())) => {}
            }
            let at = sent.elapsed();
            if self.screen().await?.contains(expected) {
                return Ok(Some(at));
            }
        }
    }

    /// Waits until the session has printed nothing for `quiet`, or `max` passes.
    async fn drain(&mut self, quiet: Duration, max: Duration) {
        let end = Instant::now() + max;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() || !matches!(tokio::time::timeout(quiet.min(left), self.output.recv()).await, Ok(Some(()))) {
                return;
            }
        }
    }
}

pub async fn run(args: &[String]) -> Result<()> {
    let o = parse(args);
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let mut c = Client::connect_user().await?;
    let mut sessions = Vec::new();
    let mut external = Vec::new();
    // Whatever happens, no spinning load may outlive the run.
    let result = measure(&o, &exe, &mut c, &mut sessions, &mut external).await;
    for handle in &sessions {
        let _ = c.call("session.close", json!({"handle": handle})).await;
    }
    for mut p in external {
        let _ = p.kill();
        let _ = p.wait();
    }
    let (mut lat, missed) = result?;
    if lat.is_empty() {
        bail!("no keystroke was echoed — is `{}` reading input?", o.cmd.join(" "));
    }
    let mean = lat.iter().sum::<Duration>().as_secs_f64() * 1000.0 / lat.len() as f64;
    lat.sort();
    println!("command            {}", o.cmd.join(" "));
    println!("load               {} background sessions, {} external", o.load, o.external);
    println!("keys echoed        {}/{}", lat.len(), o.keys);
    println!("missed             {missed}");
    println!(
        "echo p50/p95/max   {:.1} / {:.1} / {:.1} ms (mean {mean:.1})",
        pct(&lat, 0.5),
        pct(&lat, 0.95),
        pct(&lat, 1.0)
    );
    Ok(())
}

/// Starts the load and the measured session (recording both for cleanup) and types into it.
async fn measure(o: &Opts, exe: &str, c: &mut Client, sessions: &mut Vec<u32>, external: &mut Vec<std::process::Child>) -> Result<(Vec<Duration>, usize)> {
    for i in 0..o.load {
        let info = c
            .call("session.create", json!({
                "kind": "custom", "cwd": o.cwd, "name": format!("echo-load-{i}"),
                "args": [exe, "bench-burn"], "cols": 80, "rows": 24,
            }))
            .await?;
        sessions.push(info["handle"].as_u64().context("handle")? as u32);
    }
    for _ in 0..o.external {
        external.push(std::process::Command::new(exe).arg("bench-burn").spawn()?);
    }

    let info = c
        .call("session.create", json!({
            "kind": o.kind, "cwd": o.cwd, "name": "echo-bench", "env": o.env, "cols": 120, "rows": 30,
            "args": (if o.kind == "custom" { o.cmd.clone() } else { Vec::new() }),
        }))
        .await?;
    let h = info["handle"].as_u64().context("handle")? as u32;
    sessions.push(h);
    let mut e = Echo::attach(h).await?;
    e.drain(Duration::from_millis(o.settle_ms), Duration::from_secs(30)).await;
    if let Some(pre) = &o.pre {
        for key in pre.split(' ') {
            e.type_keys(key_bytes(key)).await?;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        e.drain(Duration::from_millis(o.settle_ms), Duration::from_secs(30)).await;
    }
    if let Some(ready) = &o.ready {
        let end = Instant::now() + Duration::from_secs(90);
        loop {
            let screen = e.screen().await?;
            if screen.contains(ready.as_str()) {
                break;
            }
            if Instant::now() > end {
                bail!("`{ready}` never appeared; screen:\n{screen}");
            }
            e.drain(Duration::from_millis(250), Duration::from_millis(500)).await;
        }
        e.drain(Duration::from_millis(o.settle_ms), Duration::from_secs(30)).await;
    }

    // A marker first, so every later check looks for text only this run typed.
    let mut typed = String::from("zq");
    e.type_keys(typed.as_bytes()).await?;
    if e.wait_for_screen(&typed, Duration::from_secs(10)).await?.is_none() {
        bail!("`{}` never echoed the typed text; screen:\n{}", o.cmd.join(" "), e.screen().await?);
    }
    e.drain(Duration::from_millis(200), Duration::from_secs(3)).await;

    let text = "thequickbrownfoxjumpsoverthelazydog";
    let mut lat = Vec::with_capacity(o.keys);
    let mut missed = 0;
    for i in 0..o.keys {
        let key = text.as_bytes()[i % text.len()];
        typed.push(key as char);
        e.type_keys(&[key]).await?;
        match e.wait_for_screen(&typed, Duration::from_secs(5)).await? {
            Some(d) => lat.push(d),
            None => {
                missed += 1;
                if o.show && missed == 1 {
                    println!("first miss, expected `{typed}`:\n{}", e.screen().await?);
                }
            }
        }
        e.drain(Duration::from_millis(60), Duration::from_secs(2)).await;
        tokio::time::sleep(Duration::from_millis(o.gap_ms)).await;
    }
    if o.show {
        println!("{}", e.screen().await?);
    }
    Ok((lat, missed))
}
