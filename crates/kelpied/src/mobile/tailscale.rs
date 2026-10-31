//! Tailscale mode: phones reach this computer at `https://<machine>.<tailnet>.ts.net:8443`, an origin browsers trust, with no
//! server of ours. `tailscale serve` terminates HTTPS and forwards to one loopback listener of this daemon, which exists only
//! while the mode is on and stops with the same `Generation` as the others.
//!
//! - **Off by default.** The CLI runs only while the mode is on or when asked (`mobile.tailscale_status`, Settings → Mobile open):
//!   no polling, no child and no listener otherwise. Each call runs off the runtime, is limited to 5 s and is killed past it.
//! - **The CLI's output is not trusted to stay as it is** (Tailscale documents its format as subject to change): what cannot be
//!   read is a state of its own, and nothing is changed on the strength of it.
//! - **Never clobbers.** `serve status --json` is read first: port 8443 used by anything but our own entry is refused. Off removes
//!   only our entry, with `serve --https=8443 off`. `serve reset` is never used. "Ours" is the one handler `/` proxying to the
//!   loopback port this store remembers, on a port not open to the internet.
//! - **Publishing needs the person.** Turning it on is app-only and verified; `mobile.enable` with the plain user token cannot bring it
//!   back, since turning phone access off (or the kill switch, or a reset) ends the mode and removes the entry.
//! - **Who is calling** behind the entry is in `X-Forwarded-For`, believed on this listener alone (`link::Peer`). Without it all
//!   serve traffic shares one limiter bucket: nothing that comes through here counts as this machine.
//!
//! Test seam: `KELPIE_TAILSCALE_BIN` names the program, only with `KELPIE_E2E` set; with `KELPIE_E2E` set and no such variable there
//! is no Tailscale at all. Unit tests get a fake (`Mobile::tailscale_cli`). Neither can reach the person's real `tailscale serve`.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use axum::Extension;
use serde_json::{json, Value};

use super::{listen, serve_listener, Generation, LoopbackListener, Mobile};
use crate::daemon::Daemon;

/// The port the phone's origin is on. Fixed: the origin holds the phone's keys, passkey and push subscription, so it must not change.
/// 443 is left to whatever the person serves there.
pub const HTTPS_PORT: u16 = 8443;
#[cfg_attr(test, allow(dead_code))]
const CLI_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: u64 = 16 << 20;

pub const NOT_READY: &str = "Tailscale is not ready on this computer: follow the instruction in Settings → Mobile.";
pub const PORT_TAKEN: &str = "Port 8443 of Tailscale Serve is used by something else, and Kelpie left it alone. Free that port, then turn this on again.";
pub const NOT_READABLE: &str = "Kelpie could not read what Tailscale Serve is doing, so it changed nothing. Update Tailscale and try again.";
pub const DENIED: &str = "Tailscale did not let Kelpie publish this computer. On Linux, allow your user once with `sudo tailscale set --operator=$USER`; then try again.";
pub const NOT_PUBLISHED: &str = "Tailscale did not publish this computer. Check that HTTPS certificates and Serve are allowed for your tailnet, then try again.";
pub const NOT_REMOVED: &str = "Kelpie stopped answering through Tailscale, but its entry in Tailscale Serve is still there. If Tailscale is stopped, start it: Kelpie tries again when you press Check again in Settings → Mobile, and when Kelpie next starts.";
const LISTEN_FAILED: &str = "Kelpie could not open the local port Tailscale Serve forwards to. Try again in a moment.";
pub const NOT_ON: &str = "turn mobile access on first";
pub const ENDED_MEANWHILE: &str = "Phone access was turned off or changed while this was being done, so Tailscale publishes nothing. Turn this on again if you still want it.";

// ---------- what Tailscale says ----------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detected {
    NotInstalled,
    Stopped,
    NeedsLogin,
    HttpsOff,
    /// It answered, in a way that cannot be understood.
    Unreadable,
    Ready { fqdn: String },
}

impl Detected {
    pub fn name(&self) -> &'static str {
        match self {
            Detected::NotInstalled => "not_installed",
            Detected::Stopped => "stopped",
            Detected::NeedsLogin => "needs_login",
            Detected::HttpsOff => "https_off",
            Detected::Unreadable => "unreadable",
            Detected::Ready { .. } => "ready",
        }
    }
}

/// A DNS name fit to go in a URL.
fn dns_name(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('.').to_ascii_lowercase();
    let fine = !s.is_empty() && s.len() <= 253 && s.contains('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    fine.then_some(s)
}

/// `tailscale status --json`, read as loosely as it can be: only `BackendState`, `CertDomains` (null while HTTPS is off for the
/// tailnet) and `Self.DNSName` matter.
pub fn parse_status(text: &str) -> Detected {
    let Ok(doc) = serde_json::from_str::<Value>(text) else { return Detected::Unreadable };
    let Some(state) = doc.get("BackendState").and_then(Value::as_str) else { return Detected::Unreadable };
    match state {
        "Running" => {}
        "NeedsLogin" | "NeedsMachineAuth" => return Detected::NeedsLogin,
        _ => return Detected::Stopped,
    }
    let domains: Vec<String> = doc.get("CertDomains").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter_map(dns_name).collect()).unwrap_or_default();
    // This machine's own name when it can have a certificate, or else the first name that can.
    let own = doc.get("Self").and_then(|s| s.get("DNSName")).and_then(Value::as_str).and_then(dns_name);
    match own.filter(|o| domains.contains(o)).or_else(|| domains.first().cloned()) {
        Some(fqdn) => Detected::Ready { fqdn },
        None => Detected::HttpsOff,
    }
}

/// What holds port 8443 of Tailscale Serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Free,
    /// Our one handler, forwarding to this loopback port.
    Ours(u16),
    /// Ours, under a name that is no longer this machine's: serve keeps its entries by name, so the name it has now gets no answer from it.
    Renamed(u16),
    /// Anything else: not ours to change.
    Taken,
    Unreadable,
}

/// The loopback port of a handler that is only a proxy to `http://127.0.0.1:<port>`.
fn proxied_port(handler: &Value) -> Option<u16> {
    let fields = handler.as_object()?;
    if fields.len() != 1 {
        return None;
    }
    let target = fields.get("Proxy")?.as_str()?;
    target.strip_prefix("http://127.0.0.1:")?.trim_end_matches('/').parse().ok()
}

/// One serve configuration's use of the port: none; or the loopback port of an entry that could be ours, and whether one is under `name` (any name
/// when none is given); or `Err` for anything else.
fn use_of_port(config: &Value, name: Option<&str>) -> Result<Option<(u16, bool)>, ()> {
    let port = HTTPS_PORT.to_string();
    let suffix = format!(":{port}");
    let open_to_internet = config.get("AllowFunnel").and_then(Value::as_object).is_some_and(|f| f.iter().any(|(k, on)| k.ends_with(&suffix) && on.as_bool() != Some(false)));
    if open_to_internet {
        return Err(());
    }
    let tcp = config.get("TCP").and_then(|t| t.get(&port));
    if let Some(t) = tcp {
        let fields = t.as_object().ok_or(())?;
        if t.get("HTTPS").and_then(Value::as_bool) != Some(true) || fields.iter().any(|(k, _)| k != "HTTPS") {
            return Err(());
        }
    }
    let wanted = name.map(|n| format!("{n}{suffix}"));
    let mut found = None;
    let mut under_name = wanted.is_none();
    for (host, site) in config.get("Web").and_then(Value::as_object).into_iter().flatten() {
        if !host.ends_with(&suffix) {
            continue;
        }
        let handlers = site.get("Handlers").and_then(Value::as_object).ok_or(())?;
        let [(mount, handler)] = handlers.iter().collect::<Vec<_>>()[..] else { return Err(()) };
        let at = (mount == "/").then(|| proxied_port(handler)).flatten().ok_or(())?;
        if found.is_some_and(|f| f != at) {
            return Err(());
        }
        found = Some(at);
        under_name |= wanted.as_deref().is_some_and(|w| host.eq_ignore_ascii_case(w));
    }
    if tcp.is_some() && found.is_none() {
        return Err(());
    }
    Ok(found.map(|at| (at, under_name)))
}

/// `tailscale serve status --json`: `{}` (or `null`) when nothing is served. `known` is the loopback port our entry forwards to, `name` this machine's name
/// now, when that matters.
pub fn parse_serve(text: &str, known: Option<u16>, name: Option<&str>) -> Entry {
    let Ok(doc) = serde_json::from_str::<Value>(text) else { return Entry::Unreadable };
    if doc.is_null() {
        return Entry::Free;
    }
    if !doc.is_object() {
        return Entry::Unreadable;
    }
    // A `tailscale serve` running in some terminal keeps its own configuration: on this port it is not ours whatever it says.
    let foreground = doc.get("Foreground").and_then(Value::as_object).into_iter().flatten().any(|(_, c)| !matches!(use_of_port(c, None), Ok(None)));
    if foreground {
        return Entry::Taken;
    }
    match use_of_port(&doc, name) {
        Ok(None) => Entry::Free,
        Ok(Some((port, true))) if Some(port) == known => Entry::Ours(port),
        Ok(Some((port, false))) if Some(port) == known => Entry::Renamed(port),
        _ => Entry::Taken,
    }
}

// ---------- the CLI ----------

#[derive(Debug)]
pub enum Ran {
    /// There is no such program here.
    Missing,
    /// Past its time, and killed.
    TimedOut,
    /// It could not be started.
    Failed,
    Done { ok: bool, stdout: String, stderr: String },
}

pub trait Cli: Send + Sync {
    fn run(&self, args: &[&str]) -> Ran;
}

/// The real program, looked for at each use: it may be installed while the daemon runs.
#[cfg_attr(test, allow(dead_code))]
pub struct Program;

impl Cli for Program {
    fn run(&self, args: &[&str]) -> Ran {
        match program_for(std::env::var_os("KELPIE_E2E").is_some(), std::env::var_os("KELPIE_TAILSCALE_BIN")) {
            Some(path) => run_bounded(&path, args, CLI_TIMEOUT),
            None => Ran::Missing,
        }
    }
}

/// No Tailscale: what a unit test gets unless it brings its own.
#[cfg(test)]
pub struct Absent;

#[cfg(test)]
impl Cli for Absent {
    fn run(&self, _args: &[&str]) -> Ran {
        Ran::Missing
    }
}

/// A test run names the program or has none, so it never reaches the person's real Tailscale. Otherwise the fixed install places, then `PATH`.
fn program_for(test_run: bool, over: Option<OsString>) -> Option<PathBuf> {
    if test_run {
        return over.filter(|o| !o.is_empty()).map(PathBuf::from);
    }
    let fixed: &[&str] = if cfg!(windows) { &[r"C:\Program Files\Tailscale\tailscale.exe"] } else if cfg!(target_os = "macos") { &["/Applications/Tailscale.app/Contents/MacOS/Tailscale"] } else { &[] };
    let name = if cfg!(windows) { "tailscale.exe" } else { "tailscale" };
    fixed.iter().map(PathBuf::from).find(|p| p.is_file()).or_else(|| std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn read_off(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    if let Some(pipe) = pipe {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(OUTPUT_LIMIT).read_to_end(&mut bytes);
            let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
        });
    }
    rx
}

/// Runs a program for at most `limit`; past that it is killed. Whatever it left running cannot hold this up: its output is read off
/// to the side and waited for only briefly.
fn run_bounded(program: &Path, args: &[&str], limit: Duration) -> Ran {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else { return Ran::Failed };
    let (stdout, stderr) = (read_off(child.stdout.take()), read_off(child.stderr.take()));
    let end = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let drained = |rx: mpsc::Receiver<String>| rx.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
                return Ran::Done { ok: status.success(), stdout: drained(stdout), stderr: drained(stderr) };
            }
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::TimedOut;
            }
        }
    }
}

pub fn detect(cli: &dyn Cli) -> Detected {
    match cli.run(&["status", "--json"]) {
        Ran::Missing => Detected::NotInstalled,
        // No answer in time, or a refusal: the service is not running, which is what the CLI says when it cannot reach it.
        Ran::TimedOut | Ran::Failed | Ran::Done { ok: false, .. } => Detected::Stopped,
        Ran::Done { ok: true, stdout, .. } => parse_status(&stdout),
    }
}

pub fn read_entry(cli: &dyn Cli, known: Option<u16>, name: Option<&str>) -> Entry {
    match cli.run(&["serve", "status", "--json"]) {
        Ran::Done { ok: true, stdout, .. } => parse_serve(&stdout, known, name),
        // Uninstalled: there is nothing to serve anything, and nothing to wait for.
        Ran::Missing => Entry::Free,
        _ => Entry::Unreadable,
    }
}

/// What a failed serve command means for the person. Its own words (which can hold a login address) are never passed on.
fn serve_refusal(ran: &Ran) -> &'static str {
    match ran {
        Ran::Done { stdout, stderr, .. } => {
            let said = format!("{stdout}\n{stderr}").to_ascii_lowercase();
            if said.contains("access denied") || said.contains("permission") || said.contains("operator") {
                DENIED
            } else {
                NOT_PUBLISHED
            }
        }
        _ => NOT_PUBLISHED,
    }
}

fn publish(cli: &dyn Cli, port: u16) -> Result<(), &'static str> {
    let https = format!("--https={HTTPS_PORT}");
    let target = format!("http://127.0.0.1:{port}");
    match cli.run(&["serve", "--bg", &https, &target]) {
        Ran::Done { ok: true, .. } => Ok(()),
        other => Err(serve_refusal(&other)),
    }
}

fn unpublish(cli: &dyn Cli) -> Result<(), &'static str> {
    let https = format!("--https={HTTPS_PORT}");
    match cli.run(&["serve", &https, "off"]) {
        Ran::Done { ok: true, .. } => Ok(()),
        _ => Err(NOT_REMOVED),
    }
}

/// Removes our entry if it is there; anything else on the port stays as it is. Err when ours could not be removed or cannot be told apart. The command saying yes
/// is not enough: it acts on this machine's name now, and an entry under its old name can stay. Until the port is free again the entry is still ours to account for.
pub fn remove_ours(cli: &dyn Cli, known: Option<u16>) -> Result<(), &'static str> {
    match read_entry(cli, known, None) {
        Entry::Free | Entry::Taken => Ok(()),
        Entry::Unreadable => Err(NOT_REMOVED),
        Entry::Ours(_) | Entry::Renamed(_) => {
            unpublish(cli)?;
            match read_entry(cli, known, None) {
                Entry::Free => Ok(()),
                _ => Err(NOT_REMOVED),
            }
        }
    }
}

// ---------- the mode ----------

async fn off_runtime<T: Send + 'static>(m: &Mobile, work: impl FnOnce(&dyn Cli) -> T + Send + 'static) -> Result<T> {
    let cli = m.tailscale_cli.lock().clone();
    tokio::task::spawn_blocking(move || work(cli.as_ref())).await.map_err(|_| anyhow!("that could not be finished: an internal error"))
}

/// A page with no secret, for the install step.
fn install_qr(origin: &str) -> Option<String> {
    let qr = qrcode::QrCode::new(format!("https://{origin}/").as_bytes()).ok()?;
    Some(super::qr_svg(&qr))
}

/// What Settings says about it: `state` is `Detected::name`, `serve` one of `off`, `ours`, `missing` (the mode is on and the entry is gone),
/// `left_behind` (the mode is off and our entry could not be removed), `port_busy` (the mode is on, another program holds our loopback port, and the entry
/// could not be removed, so phones may be reaching that program), `not_listening` (the mode is on and our listener is not up, with no such program),
/// `renamed` (the mode is on and our entry is under the machine's old name), `taken`, `unknown`.
fn described(m: &Mobile, detected: &Detected, entry: Option<Entry>) -> Value {
    let on = m.config.lock().tailscale;
    let origin = m.tailscale_origin();
    let lost = m.running.lock().as_ref().is_some_and(|r| r.loopback_lost);
    let serve = match (entry, on, m.loopback_port().is_some()) {
        (Some(Entry::Free), false, _) => "off",
        (Some(Entry::Free), true, _) => "missing",
        (Some(Entry::Ours(_)), false, _) => "left_behind",
        (Some(Entry::Ours(_)), true, false) if lost => "port_busy",
        (Some(Entry::Ours(_)), true, false) => "not_listening",
        (Some(Entry::Ours(_)), true, true) => "ours",
        (Some(Entry::Renamed(_)), false, _) => "left_behind",
        (Some(Entry::Renamed(_)), true, _) => "renamed",
        (Some(Entry::Taken), ..) => "taken",
        (Some(Entry::Unreadable) | None, ..) => "unknown",
    };
    let fqdn = match detected {
        Detected::Ready { fqdn } => Some(fqdn.as_str()),
        _ => None,
    };
    json!({"on": on, "state": detected.name(), "fqdn": fqdn, "serve": serve, "origin": origin, "install_qr": origin.as_deref().and_then(install_qr)})
}

/// Looks at Tailscale now and follows it: the address phones are told is there only while the name, the entry and the listener all hold.
pub async fn status(daemon: Arc<Daemon>) -> Result<Value> {
    let m = &daemon.mobile;
    // One look at a time: a window that asks again does not start another child.
    let _one = m.tailscale_check.lock().await;
    // With the mode off, or an entry still to go, this may remove an entry or forget its port, which must not cross a change under way.
    let quiet = {
        let c = m.config.lock();
        c.tailscale && !c.tailscale_remove
    };
    let _change = if quiet { None } else { Some(m.tailscale_change.lock().await) };
    let (known, to_remove) = {
        let c = m.config.lock();
        (c.tailscale_port, c.tailscale_remove)
    };
    let (detected, entry, removed) = off_runtime(m, move |cli| {
        let detected = detect(cli);
        let ready = matches!(detected, Detected::Ready { .. });
        let removed = (ready && to_remove).then(|| remove_ours(cli, known).is_ok());
        let name = if let Detected::Ready { fqdn } = &detected { Some(fqdn.as_str()) } else { None };
        let entry = ready.then(|| read_entry(cli, known, name));
        (detected, entry, removed)
    })
    .await?;
    if let Some(removed) = removed {
        m.removal_done(removed);
        if removed {
            if let Some(r) = m.running.lock().as_mut() {
                r.loopback_lost = false;
            }
        }
    } else if known.is_some() && !m.config.lock().tailscale && matches!(entry, Some(Entry::Free | Entry::Taken)) {
        m.removal_done(true);
    }
    let origin = match (&detected, entry) {
        (Detected::Ready { fqdn }, Some(Entry::Ours(_))) if m.config.lock().tailscale && m.loopback_port().is_some() => Some(format!("{fqdn}:{HTTPS_PORT}")),
        _ => None,
    };
    // A removal done just now changes what the window shows, also with phone access off, when no origin is there to change.
    if m.set_tailscale_origin(origin) || removed == Some(true) {
        daemon.broadcast("mobile.updated", json!({}));
    }
    Ok(described(m, &detected, entry))
}

/// Everything about turning it on that can be told with no prompt. What it returns goes to `turn_on`: what the person is asked holds only
/// while nothing took access away.
pub async fn check_can_turn_on(daemon: &Arc<Daemon>) -> Result<u64> {
    let m = &daemon.mobile;
    if m.running.lock().is_none() {
        bail!(NOT_ON);
    }
    let asked_at = m.tailscale_ended.load(Ordering::SeqCst);
    let known = m.config.lock().tailscale_port;
    let refusal = off_runtime(m, move |cli| match detect(cli) {
        Detected::Ready { fqdn } => match read_entry(cli, known, Some(&fqdn)) {
            Entry::Free | Entry::Ours(_) | Entry::Renamed(_) => None,
            Entry::Taken => Some(PORT_TAKEN),
            Entry::Unreadable => Some(NOT_READABLE),
        },
        _ => Some(NOT_READY),
    })
    .await?;
    match refusal {
        Some(why) => bail!(why),
        None => Ok(asked_at),
    }
}

/// Why the loopback listener could not be opened.
enum Listen {
    /// The server stopped or started again meanwhile: nothing is wrong with the port.
    NoServer,
    PortTaken,
}

impl Listen {
    fn why(&self) -> anyhow::Error {
        anyhow!(match self {
            Listen::NoServer => NOT_ON,
            Listen::PortTaken => LISTEN_FAILED,
        })
    }
}

/// Opens the loopback listener if it is not open. Returns its port. `any_port`: when the port this store remembers is taken, another will
/// do (only when turning the mode on: the entry is written right after).
async fn listen_loopback(daemon: &Arc<Daemon>, any_port: bool) -> Result<u16, Listen> {
    let m = &daemon.mobile;
    let (app, all) = {
        let running = m.running.lock();
        let r = running.as_ref().ok_or(Listen::NoServer)?;
        if let Some(l) = &r.loopback {
            return Ok(l.port);
        }
        (r.app.clone(), r.stop.clone())
    };
    let saved = m.config.lock().tailscale_port;
    let listener = bind(saved, any_port).await.map_err(|_| Listen::PortTaken)?;
    let port = listener.local_addr().map_err(|_| Listen::PortTaken)?.port();
    let own = Arc::new(Generation::new());
    {
        let mut running = m.running.lock();
        let Some(r) = running.as_mut().filter(|r| Arc::ptr_eq(&r.stop, &all) && r.loopback.is_none()) else { return Err(Listen::NoServer) };
        r.loopback = Some(LoopbackListener { port, stop: own.clone() });
    }
    // Its connections have a stop of their own: turning the mode off cuts the phones that came through it, not the others.
    tokio::spawn(serve_listener(listener, app.layer(Extension(own.clone())), None, all, own.clone(), true));
    tokio::spawn(follow_addresses(Arc::downgrade(daemon), own));
    tracing::info!("phone access: Tailscale Serve forwards to 127.0.0.1:{port}");
    Ok(port)
}

/// The port a phone's origin is forwarded to, or (`any_port`) any other. Tried a few times: a listener that was just stopped may not have let go of it yet.
async fn bind(saved: Option<u16>, any_port: bool) -> Result<tokio::net::TcpListener> {
    let Some(port) = saved else { return tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| anyhow!(LISTEN_FAILED)) };
    for _ in 0..10 {
        if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            return Ok(l);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if any_port {
        return tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.map_err(|_| anyhow!(LISTEN_FAILED));
    }
    bail!(LISTEN_FAILED)
}

/// While the mode is on: Tailscale coming up or going away shows as a change of the machine's addresses, and then Tailscale is looked at again.
async fn follow_addresses(daemon: std::sync::Weak<Daemon>, stop: Arc<Generation>) {
    let mut seen = listen::addresses();
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => return,
            _ = listen::changed() => {}
        }
        let now = listen::addresses();
        if now == seen {
            continue;
        }
        seen = now;
        listen::settle().await;
        if stop.is_cancelled() {
            return;
        }
        let Some(d) = daemon.upgrade() else { return };
        if let Err(e) = status(d).await {
            tracing::debug!("phone access: tailscale: {e:#}");
        }
    }
}

/// Publishes this computer, once the person verified. Everything is checked again: the prompt can take a long time. What takes access away
/// never waits for this: it only leaves word, and the last step below finds it, under the lock it was left under, and undoes what was done.
pub async fn turn_on(daemon: Arc<Daemon>, asked_at: u64) -> Result<Value> {
    let m = &daemon.mobile;
    let change = m.tailscale_change.lock().await;
    if m.tailscale_ended.load(Ordering::SeqCst) != asked_at {
        bail!(ENDED_MEANWHILE);
    }
    let known = m.config.lock().tailscale_port;
    let was_listening = m.loopback_port().is_some();
    let run = m.running.lock().as_ref().map(|r| r.stop.clone()).ok_or_else(|| anyhow!(NOT_ON))?;
    let port = listen_loopback(&daemon, true).await.map_err(|e| e.why())?;
    let me = daemon.clone();
    let done = off_runtime(m, move |cli| -> Result<String, &'static str> {
        let Detected::Ready { fqdn } = detect(cli) else { return Err(NOT_READY) };
        match read_entry(cli, known, Some(&fqdn)) {
            Entry::Ours(at) if at == port => return Ok(fqdn),
            Entry::Ours(_) => unpublish(cli).map_err(|_| NOT_PUBLISHED)?,
            // Under the old name the CLI may not find it to take it off. It leads nowhere, and publishing under the new name does not wait for it, but it
            // must be on this port: two entries on two ports could not be told for ours, by this or by the next look.
            Entry::Renamed(at) => {
                let _ = unpublish(cli);
                if at != port && !matches!(read_entry(cli, known, Some(&fqdn)), Entry::Free) {
                    return Err(NOT_PUBLISHED);
                }
            }
            Entry::Free => {}
            Entry::Taken => return Err(PORT_TAKEN),
            Entry::Unreadable => return Err(NOT_READABLE),
        }
        // No entry of ours is there now. From here on one may be, whatever the command says.
        me.mobile.remember_port(port);
        // What Tailscale holds now, not what it said it would do.
        let published = publish(cli, port).and_then(|()| match read_entry(cli, Some(port), Some(&fqdn)) {
            Entry::Ours(at) if at == port => Ok(()),
            _ => Err(NOT_PUBLISHED),
        });
        if let Err(why) = published {
            me.mobile.removal_done(remove_ours(cli, Some(port)).is_ok());
            return Err(why);
        }
        Ok(fqdn)
    })
    .await?;
    let fqdn = match done {
        Ok(fqdn) => fqdn,
        Err(why) => {
            if !was_listening {
                m.stop_loopback();
            }
            bail!(why);
        }
    };
    // The last step, under the lock what takes access away holds: this server, still on, with nothing ended since the person was asked.
    // The origin goes with it: turning off, which takes this lock first, then always finds it to clear.
    let committed = {
        let mut c = m.config.lock();
        let mut running = m.running.lock();
        let this_run = running.as_mut().filter(|r| Arc::ptr_eq(&r.stop, &run));
        match this_run {
            Some(r) if c.enabled && m.tailscale_ended.load(Ordering::SeqCst) == asked_at => {
                c.tailscale = true;
                c.tailscale_remove = false;
                r.ts_origin = Some(format!("{fqdn}:{HTTPS_PORT}"));
                Some(r.listeners.clone())
            }
            _ => None,
        }
    };
    let Some(listeners) = committed else {
        m.stop_loopback();
        let removed = off_runtime(m, move |cli| remove_ours(cli, Some(port))).await?;
        m.removal_done(removed.is_ok());
        if removed.is_err() {
            m.audit("-", "tailscale_off", "ended while turning on, and the entry in Tailscale Serve could not be removed");
        }
        bail!(ENDED_MEANWHILE);
    };
    m.keep();
    m.audit("-", "tailscale_on", &format!("published as {fqdn}:{HTTPS_PORT}"));
    super::advertise(m, &listeners);
    daemon.broadcast("mobile.updated", json!({}));
    drop(change);
    status(daemon.clone()).await
}

/// Turns the mode off, or removes an entry left behind by an earlier turn. Access through Tailscale ends first, whatever Tailscale then says.
pub async fn turn_off(daemon: Arc<Daemon>) -> Result<Value> {
    let m = &daemon.mobile;
    let was_on = {
        let mut c = m.config.lock();
        m.tailscale_ended.fetch_add(1, Ordering::SeqCst);
        std::mem::replace(&mut c.tailscale, false)
    };
    m.stop_loopback();
    m.keep();
    if was_on {
        m.audit("-", "tailscale_off", "no longer published");
    }
    // Access has ended by now. The entry waits for a change under way: it may be writing one.
    let change = m.tailscale_change.lock().await;
    let known = m.config.lock().tailscale_port;
    let removed = off_runtime(m, move |cli| remove_ours(cli, known)).await?;
    m.removal_done(removed.is_ok());
    drop(change);
    daemon.broadcast("mobile.updated", json!({}));
    m.unsaved(super::NOT_SAVED_OFF)?;
    removed.map_err(|why| anyhow!(why))?;
    status(daemon.clone()).await
}

/// At the start of the server when the mode is on: the listener comes back on its port and Tailscale is looked at. Nothing is published here:
/// an entry the person removed by hand stays removed until they say otherwise. If that port is someone else's now, our entry would lead
/// to them, so it goes.
pub async fn resume(daemon: Arc<Daemon>) {
    let m = &daemon.mobile;
    match listen_loopback(&daemon, false).await {
        Ok(_) | Err(Listen::NoServer) => {}
        Err(Listen::PortTaken) => after_failed_bind(&daemon).await,
    }
    if let Err(e) = status(daemon.clone()).await {
        tracing::debug!("phone access: tailscale: {e:#}");
    }
}

/// The port our entry forwards to could not be had. If someone else holds it the entry would lead to them, so it goes. If our own listener does, a start at the same
/// time got there first, and all is well.
pub(super) async fn after_failed_bind(daemon: &Arc<Daemon>) {
    let m = &daemon.mobile;
    let change = m.tailscale_change.lock().await;
    if m.loopback_port().is_some() {
        return;
    }
    tracing::warn!("phone access: tailscale: {LISTEN_FAILED}");
    let known = m.config.lock().tailscale_port;
    if let Ok(removed) = off_runtime(m, move |cli| remove_ours(cli, known)).await {
        if let Some(r) = m.running.lock().as_mut() {
            r.loopback_lost = removed.is_err();
        }
        m.config.lock().tailscale_remove = removed.is_err();
        m.keep();
    }
    drop(change);
}

/// Removes an entry that is still to go: after phone access was turned off, by whatever means (the mode ended with it, `Mobile::disable`), and each
/// time phone access or the daemon starts. If Tailscale will not do it, that stays in the store and is said in the status. Runs on a thread of its own: a
/// CLI that hangs holds up nothing that takes access away. No timer retries it: with phone access off nothing runs.
pub fn remove_after_off(daemon: &Arc<Daemon>) {
    if !daemon.mobile.config.lock().tailscale_remove {
        return;
    }
    let me = daemon.clone();
    let spawned = std::thread::Builder::new().name("kelpie-tailscale".into()).spawn(move || {
        let _change = me.mobile.tailscale_change.blocking_lock();
        let (known, still) = {
            let c = me.mobile.config.lock();
            (c.tailscale_port, c.tailscale_remove)
        };
        if !still {
            return;
        }
        let cli = me.mobile.tailscale_cli.lock().clone();
        let removed = remove_ours(cli.as_ref(), known);
        me.mobile.removal_done(removed.is_ok());
        me.mobile.audit("-", "tailscale_off", if removed.is_ok() { "no longer published" } else { "no longer answering, and the entry in Tailscale Serve could not be removed" });
        me.broadcast("mobile.updated", json!({}));
    });
    if let Err(e) = spawned {
        tracing::warn!("phone access: tailscale: {e}");
    }
}

#[cfg(test)]
pub mod fake {
    use parking_lot::Mutex;

    use super::{Cli, Ran};

    /// Tailscale as tests want it: answers from its fields, and keeps its serve entry as the real one would.
    pub struct Fake {
        pub status: Mutex<String>,
        pub serve: Mutex<String>,
        pub calls: Mutex<Vec<String>>,
        /// What `serve --bg` answers when it should fail.
        pub publish_fails: Mutex<Option<String>>,
        /// `serve --https=8443 off` fails.
        pub off_fails: Mutex<bool>,
        /// `serve status` fails, as with Tailscale stopped.
        pub serve_down: Mutex<bool>,
        /// `serve --bg` runs out of its time, having written the entry.
        pub publish_times_out: Mutex<bool>,
        /// Holds `serve --bg` up: says it got there on the first, and goes on when the second says so.
        pub publish_holds: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    }

    pub const READY: &str = r#"{"Version":"1.98.10","BackendState":"Running","Self":{"DNSName":"desk.tail1234.ts.net."},"CertDomains":["desk.tail1234.ts.net"],"Peer":{},"User":{}}"#;

    impl Fake {
        pub fn ready() -> Fake {
            Fake { status: Mutex::new(READY.into()), serve: Mutex::new("{}".into()), calls: Mutex::new(Vec::new()), publish_fails: Mutex::new(None), off_fails: Mutex::new(false), serve_down: Mutex::new(false), publish_times_out: Mutex::new(false), publish_holds: Mutex::new(None) }
        }

        pub fn entry(port: u16) -> String {
            Fake::entry_named("desk.tail1234.ts.net", port)
        }

        pub fn entry_named(name: &str, port: u16) -> String {
            format!(r#"{{"TCP":{{"8443":{{"HTTPS":true}}}},"Web":{{"{name}:8443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:{port}"}}}}}}}}}}"#)
        }

        /// Adds an entry under this machine's name now, beside what is there, as serve keeps its entries by name.
        fn add_entry(&self, port: u16) {
            let mut doc: serde_json::Value = serde_json::from_str(&self.serve.lock()).unwrap_or_else(|_| serde_json::json!({}));
            doc["TCP"]["8443"]["HTTPS"] = true.into();
            doc["Web"][format!("{}:8443", self.name_now())] = serde_json::json!({"Handlers": {"/": {"Proxy": format!("http://127.0.0.1:{port}")}}});
            *self.serve.lock() = doc.to_string();
        }

        /// Takes off the entry under this machine's name now, and no other. False when there is none.
        fn take_off_entry(&self) -> bool {
            let mut doc: serde_json::Value = serde_json::from_str(&self.serve.lock()).unwrap_or_else(|_| serde_json::json!({}));
            let key = format!("{}:8443", self.name_now());
            let Some(web) = doc.get_mut("Web").and_then(serde_json::Value::as_object_mut) else { return false };
            if web.remove(&key).is_none() {
                return false;
            }
            let last = web.is_empty();
            *self.serve.lock() = if last { "{}".to_string() } else { doc.to_string() };
            true
        }

        fn name_now(&self) -> String {
            serde_json::from_str::<serde_json::Value>(&self.status.lock()).ok().and_then(|v| v["CertDomains"][0].as_str().map(str::to_string)).unwrap_or_default()
        }

        /// Calls that change anything.
        pub fn changes(&self) -> Vec<String> {
            self.calls.lock().iter().filter(|c| !c.ends_with("--json")).cloned().collect()
        }
    }

    impl Cli for Fake {
        fn run(&self, args: &[&str]) -> Ran {
            let call = args.join(" ");
            self.calls.lock().push(call.clone());
            let answer = |s: String| Ran::Done { ok: true, stdout: s, stderr: String::new() };
            match call.as_str() {
                "status --json" => answer(self.status.lock().clone()),
                "serve status --json" => {
                    if *self.serve_down.lock() {
                        return Ran::Done { ok: false, stdout: String::new(), stderr: "failed to connect to local Tailscale service".into() };
                    }
                    answer(self.serve.lock().clone())
                }
                "serve --https=8443 off" => {
                    if *self.off_fails.lock() || !self.take_off_entry() {
                        return Ran::Done { ok: false, stdout: String::new(), stderr: "no".into() };
                    }
                    answer(String::new())
                }
                _ => {
                    if let Some((reached, go)) = self.publish_holds.lock().take() {
                        let _ = reached.send(());
                        let _ = go.recv_timeout(std::time::Duration::from_secs(10));
                    }
                    if let Some(said) = self.publish_fails.lock().clone() {
                        return Ran::Done { ok: false, stdout: String::new(), stderr: said };
                    }
                    let port = call.rsplit(':').next().and_then(|p| p.parse().ok()).expect("a serve command with a port");
                    self.add_entry(port);
                    if *self.publish_times_out.lock() {
                        return Ran::TimedOut;
                    }
                    answer(String::new())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_status_says_comes_down_to_one_state() {
        let ready = Detected::Ready { fqdn: "desk.tail1234.ts.net".into() };
        assert_eq!(parse_status(fake::READY), ready);
        assert_eq!(parse_status(r#"{"BackendState":"Running","Self":{"DNSName":"DESK.tail1234.ts.net."},"CertDomains":["other.tail1234.ts.net","desk.tail1234.ts.net"]}"#), ready, "its own name, in any case, with the root dot gone");
        assert_eq!(parse_status(r#"{"BackendState":"Running","CertDomains":["desk.tail1234.ts.net"]}"#), ready, "no Self: the name it can have a certificate for");
        for stopped in ["Stopped", "NoState", "Starting", "SomethingNew"] {
            assert_eq!(parse_status(&format!(r#"{{"BackendState":"{stopped}","CertDomains":["desk.tail1234.ts.net"]}}"#)), Detected::Stopped, "{stopped}");
        }
        for login in ["NeedsLogin", "NeedsMachineAuth"] {
            assert_eq!(parse_status(&format!(r#"{{"BackendState":"{login}"}}"#)), Detected::NeedsLogin, "{login}");
        }
        // As the real CLI (1.98) says it on a tailnet with HTTPS off: the key is there, and null.
        assert_eq!(parse_status(r#"{"BackendState":"Running","Self":{"DNSName":"desk.tail1234.ts.net."},"CertDomains":null}"#), Detected::HttpsOff);
        assert_eq!(parse_status(r#"{"BackendState":"Running"}"#), Detected::HttpsOff, "no such field");
        assert_eq!(parse_status(r#"{"BackendState":"Running","CertDomains":[]}"#), Detected::HttpsOff);
        assert_eq!(parse_status(r#"{"BackendState":"Running","CertDomains":["no dots"]}"#), Detected::HttpsOff, "nothing fit for a URL");
        assert_eq!(parse_status(r#"{"BackendState":"Running","CertDomains":["evil.example/#","x@y.ts.net"]}"#), Detected::HttpsOff);
        for garbage in ["", "not json", "[]", "null", "42", r#"{"BackendState":7}"#, r#"{"NoSuchField":true}"#] {
            assert_eq!(parse_status(garbage), Detected::Unreadable, "{garbage}");
        }
    }

    #[test]
    fn a_cli_that_fails_or_hangs_is_tailscale_not_running() {
        struct Says(fn() -> Ran);
        impl Cli for Says {
            fn run(&self, _: &[&str]) -> Ran {
                (self.0)()
            }
        }
        assert_eq!(detect(&Says(|| Ran::Missing)), Detected::NotInstalled);
        assert_eq!(detect(&Says(|| Ran::TimedOut)), Detected::Stopped);
        assert_eq!(detect(&Says(|| Ran::Failed)), Detected::Stopped);
        assert_eq!(detect(&Says(|| Ran::Done { ok: false, stdout: String::new(), stderr: "failed to connect to local Tailscale service".into() })), Detected::Stopped);
        assert_eq!(detect(&Says(|| Ran::Done { ok: true, stdout: "gibberish".into(), stderr: String::new() })), Detected::Unreadable);
        assert_eq!(read_entry(&Says(|| Ran::TimedOut), Some(1), None), Entry::Unreadable, "not knowing is not free");
    }

    #[test]
    fn only_the_one_proxy_to_our_port_is_ours() {
        let ours = fake::Fake::entry(5000);
        assert_eq!(parse_serve(&ours, Some(5000), None), Entry::Ours(5000));
        assert_eq!(parse_serve(&ours, Some(5000), Some("desk.tail1234.ts.net")), Entry::Ours(5000));
        assert_eq!(parse_serve(&ours, Some(5000), Some("DESK.tail1234.ts.net")), Entry::Ours(5000), "names are not case sensitive");
        assert_eq!(parse_serve(&ours, Some(5000), Some("new.tail1234.ts.net")), Entry::Renamed(5000), "under the name it had");
        assert_eq!(parse_serve(&ours, Some(5001), Some("new.tail1234.ts.net")), Entry::Taken, "not ours under any name");
        assert_eq!(parse_serve(&ours, Some(5001), None), Entry::Taken, "another port: someone else's");
        assert_eq!(parse_serve(&ours, None, None), Entry::Taken, "we never had one");
        for free in ["{}", "null", r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"desk.tail1234.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:3000"}}}}}"#] {
            assert_eq!(parse_serve(free, Some(5000), None), Entry::Free, "{free}");
        }
        let site = |handlers: &str| format!(r#"{{"TCP":{{"8443":{{"HTTPS":true}}}},"Web":{{"desk.tail1234.ts.net:8443":{{"Handlers":{handlers}}}}}}}"#);
        for (what, taken) in [
            ("a path of its own beside ours", site(r#"{"/":{"Proxy":"http://127.0.0.1:5000"},"/app":{"Proxy":"http://127.0.0.1:3000"}}"#)),
            ("a path that is not the root", site(r#"{"/kelpie":{"Proxy":"http://127.0.0.1:5000"}}"#)),
            ("a file server", site(r#"{"/":{"Path":"C:\\www"}}"#)),
            ("another machine", site(r#"{"/":{"Proxy":"http://192.168.1.2:5000"}}"#)),
            ("https to itself", site(r#"{"/":{"Proxy":"https://127.0.0.1:5000"}}"#)),
            ("a raw TCP forward", r#"{"TCP":{"8443":{"TCPForward":"127.0.0.1:5000"}}}"#.to_string()),
            ("open to the internet", r#"{"TCP":{"8443":{"HTTPS":true}},"Web":{"desk.tail1234.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:5000"}}}},"AllowFunnel":{"desk.tail1234.ts.net:8443":true}}"#.to_string()),
            ("a terminal's own serve", format!(r#"{{"Foreground":{{"abc":{ours}}}}}"#)),
            ("a port with no site", r#"{"TCP":{"8443":{"HTTPS":true}}}"#.to_string()),
        ] {
            assert_eq!(parse_serve(&taken, Some(5000), None), Entry::Taken, "{what}");
        }
        for garbage in ["", "not json", "[]", "7"] {
            assert_eq!(parse_serve(garbage, Some(5000), None), Entry::Unreadable, "{garbage:?}");
        }
    }

    #[test]
    fn off_removes_our_entry_and_nothing_else_and_never_resets() {
        let cli = fake::Fake::ready();
        *cli.serve.lock() = fake::Fake::entry(5000);
        assert!(remove_ours(&cli, Some(5000)).is_ok());
        assert_eq!(cli.changes(), ["serve --https=8443 off"]);
        assert_eq!(*cli.serve.lock(), "{}");

        // The command says yes and an entry of ours is still there, under a name it no longer acts on: that is not gone.
        let cli = fake::Fake::ready();
        *cli.serve.lock() = r#"{"TCP":{"8443":{"HTTPS":true}},"Web":{"desk.tail1234.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:5000"}}},"old.tail1234.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:5000"}}}}}"#.into();
        assert_eq!(remove_ours(&cli, Some(5000)), Err(NOT_REMOVED));
        assert_eq!(cli.changes(), ["serve --https=8443 off"]);
        assert!(cli.serve.lock().contains("old.tail1234.ts.net:8443") && !cli.serve.lock().contains("desk.tail1234.ts.net:8443"));

        let cli = fake::Fake::ready();
        *cli.serve.lock() = fake::Fake::entry(3000);
        assert!(remove_ours(&cli, Some(5000)).is_ok(), "someone else's");
        assert!(cli.changes().is_empty());
        assert_eq!(*cli.serve.lock(), fake::Fake::entry(3000), "as it was");

        let cli = fake::Fake::ready();
        *cli.serve.lock() = "not json".into();
        assert_eq!(remove_ours(&cli, Some(5000)), Err(NOT_REMOVED), "not knowing changes nothing, and says so");
        assert!(cli.changes().is_empty());
    }

    #[test]
    fn what_a_failed_serve_said_is_never_passed_on() {
        let denied = Ran::Done { ok: false, stdout: String::new(), stderr: "Access denied: serve config denied. Use 'sudo tailscale set --operator=me'".into() };
        assert_eq!(serve_refusal(&denied), DENIED);
        let enable = Ran::Done { ok: false, stdout: "Serve is not enabled on your tailnet. To enable, visit: https://login.tailscale.com/f/serve?node=SECRET".into(), stderr: String::new() };
        assert_eq!(serve_refusal(&enable), NOT_PUBLISHED);
        assert!(!serve_refusal(&enable).contains("SECRET"));
        assert_eq!(serve_refusal(&Ran::TimedOut), NOT_PUBLISHED);
    }

    #[test]
    fn a_test_run_names_the_program_or_has_none() {
        let fake = || Some(OsString::from(r"D:\fake\tailscale.cmd"));
        assert_eq!(program_for(true, fake()), Some(PathBuf::from(r"D:\fake\tailscale.cmd")));
        assert_eq!(program_for(true, None), None, "never the real one");
        assert_eq!(program_for(true, Some(OsString::new())), None);
        assert_ne!(program_for(false, fake()), Some(PathBuf::from(r"D:\fake\tailscale.cmd")), "the seam means nothing outside a test run");
    }

    #[test]
    fn a_program_past_its_time_is_killed_and_holds_nothing_up() {
        #[cfg(windows)]
        let (program, args) = (r"C:\Windows\System32\cmd.exe", ["/C", "ping -n 30 127.0.0.1 > NUL"]);
        #[cfg(not(windows))]
        let (program, args) = ("/bin/sh", ["-c", "sleep 30"]);
        let began = Instant::now();
        assert!(matches!(run_bounded(Path::new(program), &args, Duration::from_millis(300)), Ran::TimedOut));
        assert!(began.elapsed() < Duration::from_secs(10), "{:?}", began.elapsed());
        #[cfg(windows)]
        let (program, args) = (r"C:\Windows\System32\cmd.exe", ["/C", "echo said"]);
        #[cfg(not(windows))]
        let (program, args) = ("/bin/sh", ["-c", "echo said"]);
        match run_bounded(Path::new(program), &args, Duration::from_secs(20)) {
            Ran::Done { ok, stdout, .. } => assert!(ok && stdout.contains("said"), "{stdout}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(run_bounded(Path::new("no-such-program-here"), &[], Duration::from_secs(1)), Ran::Failed));
    }
}
