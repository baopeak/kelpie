//! `pr.status`: the pull request of a project's branch and whether GitHub would merge it (rules and parsing in `pulls.rs`).
//!
//! Only the user and the app may call it (`auth`): a program in a session is not in any list of methods that is its own. The project's folder comes
//! from the launch catalogue the window pushed (`mobile.set_launch`), so a project the window has not told the daemon about since it started has
//! none. The answer comes from a thread of its own: `gh` may take a minute, and this client's other requests must not wait for it.

use super::*;

impl Daemon {
    /// `pr.status {project}` → `{repo, number, title, url, base, head, line}`, or `{none: true}` when the project's branch has no pull request
    /// (`{none: true, github: false}` when the project has no `origin` on github.com).
    pub(super) fn pr_rpc(self: &Arc<Self>, tx: &ClientTx, id: &Value, method: &str, params: Value) -> Result<Option<Value>> {
        if method != "pr.status" {
            bail!("unknown method {method}");
        }
        let project = param(&params, "project")?.as_str().map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| anyhow!("`project` must be a project id"))?;
        let folder = self.mobile.project_folder(project).map_err(|(_, message)| anyhow!(message))?;
        let (neutral, tx, id) = (self.data_dir.clone(), tx.clone(), id.clone());
        std::thread::Builder::new().name("kelpie-pr-status".into()).spawn(move || {
            let reply = rpc_answer(&id, crate::pulls::status(&folder, &neutral).map_err(|message| anyhow!(message)));
            let _ = tx.send(json_out(&reply));
        })?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
    }

    #[test]
    fn a_project_the_window_has_not_told_the_daemon_about_is_refused_in_plain_words() {
        let (d, dir) = daemon("pr-status-unknown");
        let (tx, _rx) = crate::session::client_channel();
        let ask = |params: Value| d.dispatch(1, &tx, &json!(1), "pr.status", params);
        assert!(ask(json!({})).unwrap_err().to_string().contains("project"));
        assert!(ask(json!({"project": " "})).unwrap_err().to_string().contains("project"));
        let none_sent = ask(json!({"project": "p1"})).unwrap_err().to_string();
        assert!(none_sent.contains("Kelpie") && none_sent.contains("window"), "{none_sent}");
        let folder = dir.to_string_lossy().into_owned();
        d.mobile.set_launch(&json!({"projects": [{"id": "other", "name": "Other", "path": folder}], "agents": []})).unwrap();
        let unknown = ask(json!({"project": "p1"})).unwrap_err().to_string();
        assert!(unknown.contains("not told"), "{unknown}");
        assert!(!unknown.contains(&folder), "{unknown}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_whose_folder_is_gone_answers_later_with_a_message_and_no_folder() {
        let (d, dir) = daemon("pr-status-gone");
        let gone = dir.join("missing-project").to_string_lossy().into_owned();
        d.mobile.set_launch(&json!({"projects": [{"id": "p1", "name": "Shop", "path": gone}], "agents": []})).unwrap();
        let (tx, mut rx) = crate::session::client_channel();
        assert!(d.dispatch(1, &tx, &json!(7), "pr.status", json!({"project": "p1"})).unwrap().is_none(), "answered from its own thread");
        let mut waited = 0;
        let reply = loop {
            if let Ok(out) = rx.try_recv() {
                break serde_json::from_slice::<Value>(&out.data[5..]).unwrap();
            }
            waited += 1;
            assert!(waited < 500, "no answer within ten seconds");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(reply["id"], 7);
        let message = reply["error"]["message"].as_str().unwrap();
        assert!(message.contains("not on this computer") && !message.contains(&gone), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
