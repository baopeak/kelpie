//! `resources.*` and the thread that samples the machine (rules and state in `resources.rs`).
//!
//! Only the user's own connections may call these (a program in a session is not given them, see `auth`): what runs on
//! the computer, and in other projects, is not a session's to read.

use std::collections::BTreeSet;
use std::path::Path;

use super::*;
use crate::procs::{Sampler, Target};
use crate::resources::{Probe, SessionSample};

/// Time between samples while a session works, and while none does.
const BUSY_EVERY: Duration = Duration::from_secs(2);
const IDLE_EVERY: Duration = Duration::from_secs(10);
/// Least time between two samples: a command starting wakes the sampler, and a burst of them must not make rates
/// over a few milliseconds (noise) or let two samples a moment apart pass for a lasting rise.
const MIN_GAP: Duration = Duration::from_secs(1);
/// Volumes looked at: the data folder's and those of the folders sessions run in.
const MAX_VOLUMES: usize = 8;
/// The ledger is written at most this often, and when the daemon saves its state on the way out.
const SAVE_EVERY: Duration = Duration::from_secs(30);

/// The volume a folder is on, as its free space is read. Windows: the drive's root (`C:\`); a UNC path (a network share,
/// `\\wsl$`) is left out, as asking one can stall. Unix: the folder itself (`statvfs` answers for its volume).
fn volume_of(path: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        match path.components().next()? {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => Some(PathBuf::from(format!("{}:\\", char::from(d).to_ascii_uppercase()))),
                _ => None,
            },
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        Some(path.to_path_buf())
    }
}

/// `KELPIE_RESOURCE_MONITOR=0` turns the sampling off (the gauges then stay unknown).
fn monitor_enabled() -> bool {
    std::env::var("KELPIE_RESOURCE_MONITOR").map_or(true, |v| v != "0")
}

impl Daemon {
    /// The load the sub-session caps read: the phone's level (lag, CPU, memory) while phone access measures it, and the
    /// resource monitor's, which says `L3` whenever memory is saturated, phone or not.
    pub(super) fn machine_load(&self) -> crate::load::Level {
        self.mobile.load.level().max(self.resources.caps_level())
    }

    /// Every session that runs a process here, as the sampler needs it, with whether any works and the volumes to look at.
    fn resource_targets(&self) -> (Vec<(Target, Option<String>)>, bool, Vec<PathBuf>) {
        let mut targets = Vec::new();
        let mut working = false;
        let mut volumes: BTreeSet<PathBuf> = volume_of(&self.data_dir).into_iter().collect();
        let ids: Vec<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
        for id in ids {
            let Some(entry) = self.by_id(&id) else { continue };
            let info = entry.info();
            if !info.alive || info.spec.remote.is_some() {
                continue;
            }
            working |= matches!(info.status, Status::Working);
            if volumes.len() < MAX_VOLUMES && !info.spec.cwd.is_empty() {
                volumes.extend(volume_of(Path::new(&info.spec.cwd)));
            }
            let project = info.spec.project_id.clone().or_else(|| Some(info.spec.cwd.clone()));
            if let Ok(t) = self.process_target(&id) {
                targets.push((t, project));
            }
        }
        (targets, working, volumes.into_iter().collect())
    }

    /// Starts sampling on a thread of its own: a reading of performance counters can take a moment and must never hold
    /// the event loop. The thread holds the daemon only while it samples, so it ends with it.
    pub fn start_resource_monitor(self: &Arc<Self>) {
        if !monitor_enabled() {
            tracing::info!("resource monitor off (KELPIE_RESOURCE_MONITOR=0)");
            return;
        }
        let weak = Arc::downgrade(self);
        let spawned = std::thread::Builder::new().name("kelpie-resources".into()).spawn(move || {
            let (mut probe, mut sampler) = (Probe::default(), Sampler::default());
            let (mut last, mut saved) = (Instant::now(), Instant::now());
            loop {
                let Some(d) = weak.upgrade() else { return };
                let (targets, working, volumes) = d.resource_targets();
                let reading = probe.read(&volumes);
                let plain: Vec<Target> = targets.iter().map(|(t, _)| t.clone()).collect();
                let (all, trees) = sampler.read(&plain);
                let sessions: Vec<SessionSample> = trees
                    .into_iter()
                    .zip(&targets)
                    .map(|(tree, (t, project))| SessionSample { tree, project: project.clone(), peak_commit: t.tree.as_ref().and_then(|tr| tr.peak_commit()) })
                    .collect();
                let secs = last.elapsed().as_secs_f64();
                last = Instant::now();
                let changed = d.resources.record(now_millis(), reading, &all, sessions, secs);
                if changed {
                    tracing::info!(gauges = %d.resources.snapshot(false, Some(""))["gauges"], "resources");
                }
                d.broadcast("resources.changed", d.resources.snapshot(false, None));
                if saved.elapsed() >= SAVE_EVERY {
                    saved = Instant::now();
                    d.resources.save();
                }
                let pause = if working || d.resources.commands_running() { BUSY_EVERY } else { IDLE_EVERY };
                let resources = d.resources.clone();
                drop(d);
                resources.wait(pause);
                if let Some(rest) = MIN_GAP.checked_sub(last.elapsed()) {
                    std::thread::sleep(rest);
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("resource monitor not started: {e}");
        }
    }

    /// `resources.get {history?, session?}` and `resources.ledger {project?}`.
    pub(super) fn resource_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "resources.get" => {
                let history = params.get("history").and_then(Value::as_bool).unwrap_or(false);
                Ok(self.resources.snapshot(history, params.get("session").and_then(Value::as_str)))
            }
            "resources.ledger" => Ok(json!({"commands": self.resources.ledger(params.get("project").and_then(Value::as_str))})),
            other => bail!("unknown method {other}"),
        }
    }

    /// What a hook says about a shell command the agent runs: its start and end are what the ledger learns from.
    pub(super) fn resource_hook(&self, session_id: &str, agent: &str, event: &str, payload: &Value, project: &str) {
        if agent != "claude" {
            return;
        }
        let call = payload.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
        let now = now_millis();
        match event {
            // A command sent to the background ends its tool call at once: what it costs cannot be told apart, so it is not measured.
            "PreToolUse"
                if payload.get("tool_name").and_then(Value::as_str) == Some("Bash")
                    && !call.is_empty()
                    && payload.pointer("/tool_input/run_in_background").and_then(Value::as_bool) != Some(true) =>
            {
                if let Some(command) = payload.pointer("/tool_input/command").and_then(Value::as_str) {
                    self.resources.command_started(session_id, call, command, project, now);
                }
            }
            "PostToolUse" | "PostToolUseFailure" if !call.is_empty() => {
                self.resources.command_ended(session_id, call, event == "PostToolUseFailure", now);
                self.gate_release(session_id, Some(call));
            }
            "UserPromptSubmit" | "Stop" | "StopFailure" | "SessionEnd" => {
                self.resources.turn_boundary(session_id);
                self.gate_release(session_id, None);
            }
            _ => {}
        }
    }
}
