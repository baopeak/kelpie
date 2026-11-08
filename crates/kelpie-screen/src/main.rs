//! `kelpie-screen`, spawned by the Kelpie app (`app/src-tauri/src/screen.rs`). It says `hello` first; the app checks `proto`. Then it
//! takes one JSON command a line on stdin and answers on stdout in frames (`wire`): replies `{id, ok | err}`, events `{event, sc, …}`, and
//! media for the phone. Closing stdin ends it: the app does that 30 s after the last viewer, or when it quits.
//!
//! Commands: `targets`, `start {sc, target, device, hotkey, level, tiles}`, `stop {sc, reason}`, `input {sc, ops, t}`, `text {id, sc, text, t}`,
//! `flow {sc, acked, rtt_ms, level, lost_tiles}`, `keyframe {sc}`, `pause {sc, paused}`, `resume_load {sc}`. What is typed is never written
//! anywhere but into `SendInput`.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use kelpie_screen::wire;

fn main() {
    #[cfg(windows)]
    kelpie_screen::win::system::init_process();
    let (out, out_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(256);
    let writer = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        while let Ok(bytes) = out_rx.recv() {
            if lock.write_all(&bytes).is_err() {
                return;
            }
            // Whatever else is ready goes in the same write.
            while let Ok(more) = out_rx.try_recv() {
                if lock.write_all(&more).is_err() {
                    return;
                }
            }
            if lock.flush().is_err() {
                return;
            }
        }
    });
    let _ = out.send(wire::out_json(&json!({"event": "hello", "proto": wire::PROTO, "version": env!("CARGO_PKG_VERSION"), "available": cfg!(windows)})));

    #[cfg(windows)]
    let mut host = kelpie_screen::win::stream::Host::new(out.clone());

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let id = req["id"].clone();
        let cmd = req["cmd"].as_str().unwrap_or_default().to_string();
        #[cfg(windows)]
        let reply: Option<Result<Value, String>> = match cmd.as_str() {
            "targets" => Some(Ok(host.targets())),
            "start" => Some(host.start(&req)),
            _ => host.command(&cmd, &req),
        };
        #[cfg(not(windows))]
        let reply: Option<Result<Value, String>> = Some(Err(format!("{cmd}: screen sharing is not available on this platform yet")));
        if let (Some(reply), false) = (reply, id.is_null()) {
            let msg = match reply {
                Ok(v) => json!({"id": id, "ok": v}),
                Err(e) => json!({"id": id, "err": e}),
            };
            let _ = out.send(wire::out_json(&msg));
        }
    }
    #[cfg(windows)]
    {
        host.stop("app_closed");
        drop(host);
    }
    drop(out);
    let _ = writer.join();
}
