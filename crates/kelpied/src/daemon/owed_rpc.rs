//! `owed.*`: the record of real-world checks a change still owes (rules and state in `owed.rs`), and the optional
//! `owes` list of `sub.report` and `run.report` that opens entries from an agent's final report.
//!
//! The user and the app may call every method. A program in a session may list and open checks of its own project
//! only (`auth::authorize` puts its id into `session_id`, and a project named in the request must be the session's);
//! it can never close one: an agent must not close what it owes.

use super::*;
use crate::owed::{NewOwed, MAX_OWES_PER_REPORT};

impl Daemon {
    /// `owed.list {project?}`, `owed.open {project, pr?, commit?, kind, what, closes_by}`, `owed.close {id, evidence}`.
    pub(super) fn owed_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        let caller = params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty());
        let own_project = match caller {
            Some(sid) => Some(self.project_of(sid).ok_or_else(|| anyhow!("{method}: this session belongs to no project, so it has no checks of its own"))?),
            None => None,
        };
        let asked = params.get("project").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty());
        let project = match (own_project, asked) {
            (Some(own), Some(asked)) => {
                anyhow::ensure!(own == asked, "{method}: only this session's own project");
                Some(own)
            }
            (Some(own), None) => Some(own),
            (None, asked) => asked.map(str::to_string),
        };
        match method {
            "owed.list" => Ok(json!(self.owed.lock().list(project.as_deref()))),
            "owed.open" => {
                let project = project.ok_or_else(|| anyhow!("missing param `project`"))?;
                let new = NewOwed::from_json(&project, params)?;
                let by = caller.map_or_else(|| "user".to_string(), |sid| format!("session:{sid}"));
                let entry = self.owed.lock().open(new, &by, now_millis())?;
                self.owed_changed(&entry.project);
                Ok(json!(entry))
            }
            "owed.close" => {
                anyhow::ensure!(caller.is_none(), "owed.close is the user's: a session cannot close what it owes");
                let id = params.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("missing param `id`"))?;
                let evidence = params.get("evidence").and_then(Value::as_str).unwrap_or_default();
                let entry = self.owed.lock().close(id, evidence, "user", now_millis())?;
                self.owed_changed(&entry.project);
                Ok(json!(entry))
            }
            other => bail!("unknown method {other}"),
        }
    }

    /// The project session `sid` belongs to, when it has one.
    fn project_of(&self, sid: &str) -> Option<String> {
        self.by_id(sid).and_then(|e| e.spec().project_id).filter(|p| !p.is_empty())
    }

    /// Tells the user's connections (and the app) of a project's checks. Sessions are told nothing: the event is not one
    /// `auth::event_visibility` shows them.
    fn owed_changed(&self, project: &str) {
        let owed = self.owed.lock().list(Some(project));
        self.broadcast("owed.changed", json!({"project": project, "owed": owed}));
    }

    /// The `owes` of a report from session `sid`: each valid `{kind, what, closes_by}` (at most `MAX_OWES_PER_REPORT`) opens a
    /// check for the session's project (else `fallback_project`, the project of its run), opened by that session. A bad one
    /// is refused with what is wrong, and never makes the report fail: by then the report has been recorded, and losing it
    /// because of a side note would cost more than the note. `None` when the report names no `owes`; otherwise
    /// `{opened: [entry], refused: [message]}` for the agent to read.
    pub(super) fn report_owes(&self, sid: &str, params: &Value, fallback_project: Option<String>) -> Option<Value> {
        let owes = params.get("owes").filter(|v| !v.is_null())?;
        let mut opened = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        let project = self.project_of(sid).or_else(|| fallback_project.filter(|p| !p.is_empty()));
        match (owes.as_array(), project) {
            (None, _) => refused.push("`owes` must be a list of {kind, what, closes_by}".to_string()),
            (_, None) => refused.push("this session belongs to no project, so a check cannot be recorded for it".to_string()),
            (Some(list), Some(project)) => {
                for (i, item) in list.iter().take(MAX_OWES_PER_REPORT).enumerate() {
                    let opened_by = format!("session:{sid}");
                    match NewOwed::from_json(&project, item).and_then(|new| self.owed.lock().open(new, &opened_by, now_millis())) {
                        Ok(entry) => opened.push(entry),
                        Err(e) => refused.push(format!("owes[{i}]: {e:#}")),
                    }
                }
                if list.len() > MAX_OWES_PER_REPORT {
                    refused.push(format!("at most {MAX_OWES_PER_REPORT} checks per report: the rest ({}) were not recorded", list.len() - MAX_OWES_PER_REPORT));
                }
                if !opened.is_empty() {
                    self.owed_changed(&project);
                }
            }
        }
        Some(json!({"opened": opened, "refused": refused}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fake;

    fn daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
    }

    /// A running agent session of `project` (none: a session outside any project).
    fn add(d: &Daemon, handle: u32, id: &str, project: Option<&str>) {
        let (session, _typed) = fake::running(handle, id, "claude");
        session.spec.lock().project_id = project.map(str::to_string);
        session.tick(Instant::now());
        d.insert_for_test(Entry::Term(session));
    }

    fn check(kind: &str, what: &str) -> Value {
        json!({"kind": kind, "what": what, "closes_by": "a picture from the window helper"})
    }

    #[test]
    fn the_user_opens_lists_and_closes_checks_and_clients_are_told() {
        let (d, dir) = daemon("owed-user");
        let (tx, mut rx) = crate::session::client_channel();
        d.register_client(1, tx.clone());
        d.set_client_role(1, &crate::auth::Role::User);
        let call = |method: &str, params: Value| d.dispatch(1, &tx, &json!(1), method, params).map(|v| v.expect("answered at once"));
        let opened = call("owed.open", json!({"project": "p1", "pr": 12, "kind": "phone", "what": "open the link on the phone", "closes_by": "the owner's word"})).unwrap();
        assert_eq!((opened["opened_by"].as_str(), opened["pr"].as_u64(), opened["kind"].as_str()), (Some("user"), Some(12), Some("phone")));
        assert!(call("owed.open", json!({"project": "p1", "kind": "phone", "what": "no way to close it"})).unwrap_err().to_string().contains("closes_by"));
        assert!(call("owed.open", json!({"kind": "phone", "what": "w", "closes_by": "c"})).unwrap_err().to_string().contains("project"));
        assert!(call("owed.open", json!({"project": "p1", "kind": "later", "what": "w", "closes_by": "c"})).is_err());
        call("owed.open", json!({"project": "p2", "kind": "soak", "what": "a day of use", "closes_by": "a day without a crash"})).unwrap();
        assert_eq!(call("owed.list", json!({})).unwrap().as_array().unwrap().len(), 2);
        assert_eq!(call("owed.list", json!({"project": "p1"})).unwrap().as_array().unwrap().len(), 1);
        let id = opened["id"].as_str().unwrap();
        assert!(call("owed.close", json!({"id": id})).unwrap_err().to_string().contains("evidence"));
        let closed = call("owed.close", json!({"id": id, "evidence": "the owner looked: fine"})).unwrap();
        assert_eq!((closed["closed_by"].as_str(), closed["evidence"].as_str()), (Some("user"), Some("the owner looked: fine")));
        assert_eq!(call("owed.list", json!({"project": "p1"})).unwrap()[0]["evidence"], "the owner looked: fine");
        let told: Vec<Value> = {
            let mut out = Vec::new();
            while let Ok(o) = rx.try_recv() {
                out.push(serde_json::from_slice::<Value>(&o.data[5..]).unwrap());
            }
            out
        };
        let changed: Vec<&Value> = told.iter().filter(|e| e["event"] == "owed.changed").collect();
        assert_eq!(changed.len(), 3, "two opened, one closed");
        assert_eq!(changed[2]["params"]["project"], "p1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_opens_and_lists_only_its_own_projects_checks_and_never_closes() {
        let (d, dir) = daemon("owed-session");
        add(&d, 1, "s1", Some("mine"));
        add(&d, 2, "s2", None);
        let by_user = d.owed_rpc("owed.open", &json!({"project": "other", "kind": "owner", "what": "someone else's", "closes_by": "word"})).unwrap();
        // No project named: its own. The opener is the session.
        let own = d.owed_rpc("owed.open", &json!({"session_id": "s1", "kind": "real_app", "what": "try the tray", "closes_by": "a picture"})).unwrap();
        assert_eq!((own["project"].as_str(), own["opened_by"].as_str()), (Some("mine"), Some("session:s1")));
        d.owed_rpc("owed.open", &json!({"session_id": "s1", "project": "mine", "kind": "owner", "what": "look at it", "closes_by": "word"})).unwrap();
        let err = d.owed_rpc("owed.open", &json!({"session_id": "s1", "project": "other", "kind": "owner", "what": "x", "closes_by": "y"})).unwrap_err();
        assert!(err.to_string().contains("own project"), "{err}");
        assert_eq!(d.owed_rpc("owed.list", &json!({"session_id": "s1"})).unwrap().as_array().unwrap().len(), 2, "its own project's, not the other's");
        assert!(d.owed_rpc("owed.list", &json!({"session_id": "s1", "project": "other"})).is_err());
        assert!(d.owed_rpc("owed.open", &json!({"session_id": "s2", "kind": "owner", "what": "x", "closes_by": "y"})).unwrap_err().to_string().contains("no project"));
        let err = d.owed_rpc("owed.close", &json!({"session_id": "s1", "id": by_user["id"], "evidence": "i say so"})).unwrap_err();
        assert!(err.to_string().contains("cannot close"), "{err}");
        assert!(d.owed_rpc("owed.close", &json!({"session_id": "s1", "id": own["id"], "evidence": "i say so"})).is_err(), "not even its own");
        assert_eq!(d.owed_rpc("owed.list", &json!({})).unwrap().as_array().unwrap().iter().filter(|e| e["closed_ms"].is_null()).count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_report_opens_the_checks_it_names_and_is_not_lost_for_a_bad_one() {
        let (d, dir) = daemon("owed-report");
        add(&d, 1, "c", Some("proj"));
        let out = d
            .sub_rpc(
                "sub.report",
                &json!({"session_id": "c", "status": "done", "summary": "fixed", "owes": [
                    check("real_app", "try the split picker"),
                    {"kind": "later", "what": "w", "closes_by": "c"},
                    {"kind": "phone", "what": "no way to close"},
                    "not an object",
                    check("soak", "five checks is one too many"),
                ]}),
            )
            .unwrap();
        // The report went through whatever the list held.
        assert!(out.get("delivered_to").is_some());
        assert_eq!(out["owes"]["opened"].as_array().unwrap().len(), 1, "{out}");
        let refused: Vec<&str> = out["owes"]["refused"].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect();
        assert_eq!(refused.len(), 4, "{refused:?}");
        assert!(refused[0].starts_with("owes[1]") && refused[0].contains("kind must be one of"));
        assert!(refused[1].starts_with("owes[2]") && refused[1].contains("closes_by"));
        assert!(refused[2].starts_with("owes[3]") && refused[2].contains("must be an object"));
        assert!(refused[3].contains("at most 4"), "the fifth is not looked at: {refused:?}");
        let list = d.owed_rpc("owed.list", &json!({"project": "proj"})).unwrap();
        assert_eq!((list[0]["what"].as_str(), list[0]["opened_by"].as_str()), (Some("try the split picker"), Some("session:c")));
        // No `owes`: the reply is what it always was. `owes` that is not a list is refused, the report still stands.
        assert!(d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "progress", "summary": "x"})).unwrap().get("owes").is_none());
        let bad = d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "progress", "summary": "x", "owes": "later"})).unwrap();
        assert_eq!(bad["owes"]["refused"].as_array().unwrap().len(), 1);
        // A session outside any project cannot have checks recorded for it, and its report stands.
        add(&d, 2, "loose", None);
        let loose = d.sub_rpc("sub.report", &json!({"session_id": "loose", "status": "done", "owes": [check("owner", "x")]})).unwrap();
        assert!(loose["owes"]["refused"][0].as_str().unwrap().contains("no project"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
