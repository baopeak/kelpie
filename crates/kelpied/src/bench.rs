//! `kelpied bench` — load test against a running daemon.
//!
//! Spawns N sessions that each print `--mb` MiB of coloured text (via `kelpied bench-emit`),
//! subscribes to all of them, and checks every output frame continues exactly where the
//! previous one ended (no lost or duplicated bytes, resyncs handled). Then measures attach
//! (snapshot) latency over `--attach` cycles. ConPTY re-renders output, so we verify stream
//! continuity against the daemon's own sequence numbers rather than against emitted bytes.

use std::collections::HashMap;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use kelpie_proto::Frame;
use serde_json::{json, Value};

use crate::client::Client;

struct Opts {
    sessions: usize,
    mb: usize,
    attach: usize,
    /// Optional command (space separated) instead of the built-in emitter.
    cmd: Option<Vec<String>>,
}

fn parse(args: &[String]) -> Opts {
    let mut o = Opts { sessions: 20, mb: 2, attach: 200, cmd: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--cmd" {
            o.cmd = it.next().map(|c| c.split(' ').map(str::to_string).collect());
            continue;
        }
        let v = it.next().and_then(|v| v.parse().ok());
        match (a.as_str(), v) {
            ("--sessions", Some(v)) => o.sessions = v,
            ("--mb", Some(v)) => o.mb = v,
            ("--attach", Some(v)) => o.attach = v,
            _ => {}
        }
    }
    o
}

/// Child side: deterministic coloured output, then exit.
pub fn emit(args: &[String]) {
    let total: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(1 << 20);
    // 64 KiB writes: per-call console overhead dominates on Windows otherwise.
    let mut out = std::io::stdout().lock();
    let mut chunk = String::with_capacity(70 * 1024);
    let mut written = 0;
    let mut i = 0u64;
    while written < total {
        let before = chunk.len();
        let _ = std::fmt::Write::write_fmt(
            &mut chunk,
            format_args!("\x1b[3{}m{:>8} the quick brown fox jumps over the lazy dog 0123456789\x1b[0m\r\n", i % 7 + 1, i),
        );
        written += chunk.len() - before;
        i += 1;
        if chunk.len() >= 64 * 1024 {
            let _ = out.write_all(chunk.as_bytes());
            chunk.clear();
        }
    }
    let _ = out.write_all(chunk.as_bytes());
    let _ = out.flush();
}

pub fn pct(sorted: &[Duration], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

pub async fn run(args: &[String]) -> Result<()> {
    let o = parse(args);
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    let mut c = Client::connect_user().await?;
    let cwd = std::env::temp_dir().to_string_lossy().into_owned();
    let bytes = (o.mb << 20).to_string();

    let mut handles = Vec::new();
    // Stream offset per handle; each (re)attach reply resets the base.
    let mut last: HashMap<u32, u64> = HashMap::new();
    let started = Instant::now();
    for i in 0..o.sessions {
        let info = c
            .call("session.create", json!({
                "kind": "custom", "cwd": cwd, "name": format!("bench-{i}"),
                "args": o.cmd.clone().unwrap_or_else(|| vec![exe.clone(), "bench-emit".into(), bytes.clone()]), "cols": 120, "rows": 30,
            }))
            .await?;
        let h = info["handle"].as_u64().unwrap() as u32;
        let r = c.call("session.attach", json!({"handle": h, "interactive": false})).await?;
        last.insert(h, r["seq"].as_u64().unwrap());
        handles.push(h);
    }
    let spawn_ms = started.elapsed().as_millis();
    let mut received: u64 = 0;
    let mut frames: u64 = 0;
    let mut resyncs = 0;
    let mut gaps = 0;
    let mut exited = 0;
    let deadline = Instant::now() + Duration::from_secs(300);
    while exited < handles.len() && Instant::now() < deadline {
        let frame = tokio::time::timeout(Duration::from_secs(20), c.next_frame()).await??;
        match frame {
            Frame::Output { handle, seq, data } => {
                let Some(prev) = last.get_mut(&handle) else { continue };
                if seq != *prev + data.len() as u64 {
                    gaps += 1;
                }
                *prev = seq;
                received += data.len() as u64;
                frames += 1;
            }
            Frame::Json(body) => {
                let v: Value = serde_json::from_slice(&body)?;
                match v["event"].as_str() {
                    Some("session.resync") => {
                        resyncs += 1;
                        let h = v["params"]["handle"].as_u64().unwrap() as u32;
                        let r = c.call("session.attach", json!({"handle": h, "interactive": false})).await?;
                        last.insert(h, r["seq"].as_u64().unwrap());
                    }
                    Some("session.updated") => {
                        let p = &v["params"];
                        let h = p["handle"].as_u64().unwrap_or(0) as u32;
                        if handles.contains(&h) && !p["alive"].as_bool().unwrap_or(true) {
                            exited += 1;
                        }
                    }
                    _ => {}
                }
            }
            Frame::Input { .. } => {}
        }
    }
    // Drain the tail (exit marker etc.).
    while let Ok(Ok(frame)) = tokio::time::timeout(Duration::from_millis(500), c.next_frame()).await {
        if let Frame::Output { handle, seq, data } = frame {
            if let Some(prev) = last.get_mut(&handle) {
                if seq != *prev + data.len() as u64 {
                    gaps += 1;
                }
                *prev = seq;
                received += data.len() as u64;
            }
        }
    }
    let stream_secs = started.elapsed().as_secs_f64();

    let list = c.call("session.list", json!({})).await?;
    let mut mismatched = 0;
    for s in list.as_array().unwrap() {
        let h = s["handle"].as_u64().unwrap() as u32;
        if let Some(seen) = last.get(&h) {
            if *seen != s["seq"].as_u64().unwrap() {
                mismatched += 1;
            }
        }
    }

    let mut lat = Vec::with_capacity(o.attach);
    for i in 0..o.attach {
        let h = handles[i % handles.len()];
        let t = Instant::now();
        c.call("session.attach", json!({"handle": h, "interactive": false})).await?;
        lat.push(t.elapsed());
        c.call("session.detach", json!({"handle": h})).await?;
    }
    lat.sort();

    for h in &handles {
        let _ = c.call("session.close", json!({"handle": h})).await;
    }

    println!("sessions           {}", o.sessions);
    println!("spawn total        {spawn_ms} ms");
    println!("stream bytes       {:.1} MiB in {:.2} s ({:.1} MiB/s)", received as f64 / 1048576.0, stream_secs, received as f64 / 1048576.0 / stream_secs);
    println!("output frames      {frames} (avg {} B)", received / frames.max(1));
    println!("resyncs            {resyncs}");
    println!("seq gaps           {gaps}");
    println!("final seq mismatch {mismatched}");
    println!("sessions exited    {exited}/{}", handles.len());
    println!("attach p50/p95/max {:.2} / {:.2} / {:.2} ms", pct(&lat, 0.5), pct(&lat, 0.95), pct(&lat, 1.0));
    if gaps > 0 || mismatched > 0 {
        bail!("stream integrity check failed");
    }
    Ok(())
}
