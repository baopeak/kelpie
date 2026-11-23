//! What a phone asks about a project's pull request (`pr_get`): the daemon's own read of it (`pulls`), for the project's folder as the window
//! told the daemon (the launch catalogue). The folder never goes to the phone, and neither does anything that names it.

use std::collections::HashMap;

use parking_lot::Mutex;
use serde_json::{json, Value};

use super::{launch, refuse, take_device_token, Conn, Mobile, Refusal, SendBucket};

/// Times one device may read a project's pull request: this many at once, then one every five seconds (each is one or two calls to GitHub).
const READ_BURST: f64 = 4.0;
const READ_RATE: f64 = 0.2;

#[derive(Default)]
pub struct PrMemory {
    /// How fast each device has been reading pull requests (`allow_read`).
    reads: Mutex<HashMap<String, SendBucket>>,
}

impl PrMemory {
    pub fn allow_read(&self, device: &str) -> bool {
        take_device_token(&self.reads, device, READ_RATE, READ_BURST)
    }
}

impl Mobile {
    /// The folder of project `id`, from the launch catalogue the window pushed with `mobile.set_launch`: (code, message) when there is none.
    /// The catalogue lives in memory only, so a daemon the window has not talked to since it started knows no project.
    pub fn project_folder(&self, id: &str) -> Result<String, (&'static str, String)> {
        match &*self.launch.lock() {
            Err(launch::Missing::NotSent) => Err(("not_ready", "Kelpie's window has not told the daemon about your projects since the daemon started: open Kelpie on the computer.".to_string())),
            Err(refused) => Err(refused.refusal()),
            Ok(catalogue) => catalogue
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.path.clone())
                .ok_or(("no_project", "Kelpie's window has not told the daemon about this project: it is not on this computer, or it runs over SSH.".to_string())),
        }
    }
}

impl Conn {
    /// The folder of the project a request names.
    fn project_folder_of(&self, req: &Value) -> Result<String, Refusal> {
        let id = req["project"].as_str().filter(|p| !p.is_empty() && p.len() <= launch::MAX_PROJECT_ID).ok_or_else(|| refuse("bad_request", "missing project"))?;
        self.daemon.mobile.project_folder(id).map_err(|(code, message)| refuse(code, message))
    }

    /// `pr_get {project}`: the pull request of the project's branch and GitHub's word on merging it (`pulls::status`), or `{none: true}`.
    pub(super) async fn pr_get(&self, req: &Value) -> Result<Value, Refusal> {
        if !self.daemon.mobile.pr.allow_read(&self.device) {
            return Err(refuse("rate_limited", "Asking too often: wait a moment."));
        }
        let folder = self.project_folder_of(req)?;
        let neutral = self.daemon.data_dir.clone();
        let read = tokio::task::spawn_blocking(move || crate::pulls::status(&folder, &neutral)).await.map_err(|_| refuse("not_kept", "That did not finish."))?;
        read.map(|pr| json!({"type": "ack", "result": "ok", "pr": pr})).map_err(|why| refuse("pr_unavailable", why))
    }
}
