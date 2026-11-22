//! The record of real-world checks a change still owes: a check on the real app, the phone, a clean
//! machine, a soak, or the owner's own look. An entry is opened by the user or by an agent's report that
//! names a check it could not run, and is closed only with evidence (a path, a link, the owner's word).
//! An entry that says nothing about how it closes is refused when it is opened, so the list does not
//! fill with wishes. One store per daemon, in `owed.json`; entries are keyed by project and by pull
//! request number (a squash merge makes a new commit), with the merged commit added when it is known.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::persist::{load_state, save_state};

/// Open entries one project may hold: past this the list is not being read, and a new entry says so.
pub const MAX_OPEN_PER_PROJECT: usize = 50;
/// Open entries in all projects together.
pub const MAX_OPEN: usize = 200;
/// Closed entries kept as history; closing one more drops the one closed longest ago.
pub const MAX_CLOSED: usize = 500;
/// Characters of `what`, the one line saying what is owed.
pub const MAX_WHAT: usize = 300;
/// Characters of `closes_by`, the one line saying how the entry closes.
pub const MAX_CLOSES_BY: usize = 300;
/// Characters of `evidence`, the one line a close needs.
pub const MAX_EVIDENCE: usize = 500;
/// Characters of a project id.
const MAX_PROJECT: usize = 200;
/// Longest `opened_by` / `closed_by` (`session:<id>`).
const MAX_BY: usize = 100;
/// A commit is a hex name of 7 to 64 digits (an abbreviation up to a SHA-256).
const COMMIT_DIGITS: std::ops::RangeInclusive<usize> = 7..=64;
/// Checks one report may name; more are refused with a message.
pub const MAX_OWES_PER_REPORT: usize = 4;

/// What kind of check is owed (a closed set).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    RealApp,
    Phone,
    CleanMachine,
    Soak,
    Owner,
}

impl Kind {
    pub const ALL: &'static str = "real_app, phone, clean_machine, soak, owner";

    pub fn parse(s: &str) -> Result<Kind> {
        Ok(match s.trim() {
            "real_app" => Kind::RealApp,
            "phone" => Kind::Phone,
            "clean_machine" => Kind::CleanMachine,
            "soak" => Kind::Soak,
            "owner" => Kind::Owner,
            other => bail!("kind must be one of {} (got `{}`)", Kind::ALL, clean_line(other, 40)),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Owed {
    pub id: String,
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    /// The merged commit, when it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub kind: Kind,
    pub what: String,
    pub closes_by: String,
    /// `session:<id>` or `user`.
    pub opened_by: String,
    pub opened_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

impl Owed {
    pub fn is_open(&self) -> bool {
        self.closed_ms.is_none()
    }
}

/// A check as asked for, before it is cleaned and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct NewOwed {
    pub project: String,
    pub pr: Option<u64>,
    pub commit: Option<String>,
    pub kind: Kind,
    pub what: String,
    pub closes_by: String,
}

impl NewOwed {
    /// Reads `{pr?, commit?, kind, what, closes_by}` from a request or one item of a report's `owes`, for `project`.
    /// Missing text is left empty (the store refuses it with a message that says which); a wrong type is refused here.
    pub fn from_json(project: &str, v: &Value) -> Result<NewOwed> {
        let obj = v.as_object().ok_or_else(|| anyhow!("a check must be an object {{kind, what, closes_by}}"))?;
        let text = |k: &str| -> Result<String> {
            match obj.get(k) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) => Ok(s.clone()),
                Some(_) => bail!("`{k}` must be text"),
            }
        };
        let pr = match obj.get("pr") {
            None | Some(Value::Null) => None,
            Some(n) => Some(n.as_u64().ok_or_else(|| anyhow!("`pr` must be a pull request number"))?),
        };
        let commit = Some(text("commit")?).filter(|c| !c.trim().is_empty());
        let kind = Kind::parse(&text("kind")?)?;
        Ok(NewOwed { project: project.to_string(), pr, commit, kind, what: text("what")?, closes_by: text("closes_by")? })
    }
}

/// One line: whitespace becomes a space, control characters (an entry ends up in a window, a phone and an agent's
/// input) and bidi overrides are dropped, then cut to `max` characters.
fn clean_line(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !(c.is_control() || ('\u{202a}'..='\u{202e}').contains(c) || ('\u{2066}'..='\u{2069}').contains(c)))
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

pub struct OwedStore {
    path: PathBuf,
    list: Vec<Owed>,
    /// Why `owed.json` could not be used. While set, nothing is written: the file is there, so an empty list
    /// must not replace it.
    unreadable: Option<String>,
}

impl OwedStore {
    pub fn load(dir: &Path) -> OwedStore {
        let path = dir.join("owed.json");
        match load_state::<Vec<Owed>>(&path) {
            Ok(list) => OwedStore { path, list: list.unwrap_or_default(), unreadable: None },
            Err(e) => OwedStore { path, list: vec![], unreadable: Some(e.to_string()) },
        }
    }

    /// Writes the list, or refuses when the file on disk could not be read at start.
    fn save(&self) -> Result<()> {
        if let Some(why) = &self.unreadable {
            bail!("owed.json could not be read ({why}), so the record is not changed: fix or move that file and restart Kelpie's daemon");
        }
        save_state(&self.path, &serde_json::to_vec_pretty(&self.list)?)
    }

    /// Entries (of `project`, else all): the open ones first, oldest first, then the closed ones, last closed first.
    pub fn list(&self, project: Option<&str>) -> Vec<Owed> {
        let mut out: Vec<Owed> = self.list.iter().filter(|e| project.map_or(true, |p| e.project == p)).cloned().collect();
        out.sort_by(|a, b| match (a.closed_ms, b.closed_ms) {
            (None, None) => a.opened_ms.cmp(&b.opened_ms),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => y.cmp(&x),
        });
        out
    }

    /// Records a check as opened by `opened_by` (`user` or `session:<id>`) at `now`. The same open check (project,
    /// pull request, kind and text) asked for again is the same entry, returned as it is.
    pub fn open(&mut self, new: NewOwed, opened_by: &str, now: u64) -> Result<Owed> {
        let project = clean_line(&new.project, MAX_PROJECT);
        let what = clean_line(&new.what, MAX_WHAT);
        let closes_by = clean_line(&new.closes_by, MAX_CLOSES_BY);
        ensure!(!project.is_empty(), "a check belongs to a project");
        ensure!(!what.is_empty(), "say in one line what is owed (`what`)");
        ensure!(!closes_by.is_empty(), "say how it closes (`closes_by`: a screenshot path, a run's link, the owner's word); a check with no way to close is not recorded");
        ensure!(new.pr.map_or(true, |n| n >= 1), "a pull request number starts at 1");
        let commit = match new.commit.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => {
                ensure!(COMMIT_DIGITS.contains(&c.len()) && c.bytes().all(|b| b.is_ascii_hexdigit()), "`commit` must be a hex commit name of 7 to 64 digits");
                Some(c.to_ascii_lowercase())
            }
            None => None,
        };
        let opened_by = clean_line(opened_by, MAX_BY);
        ensure!(!opened_by.is_empty(), "a check says who opened it");
        if let Some(same) = self.list.iter().find(|e| e.is_open() && e.project == project && e.pr == new.pr && e.kind == new.kind && e.what == what) {
            return Ok(same.clone());
        }
        let open_here = self.list.iter().filter(|e| e.is_open() && e.project == project).count();
        ensure!(open_here < MAX_OPEN_PER_PROJECT, "this project has {MAX_OPEN_PER_PROJECT} checks owed already: close or drop some first");
        ensure!(self.list.iter().filter(|e| e.is_open()).count() < MAX_OPEN, "{MAX_OPEN} checks are owed in all: close some first");
        let entry = Owed { id: uuid::Uuid::new_v4().to_string(), project, pr: new.pr, commit, kind: new.kind, what, closes_by, opened_by, opened_ms: now, closed_ms: None, closed_by: None, evidence: None };
        self.change(|list| list.push(entry.clone()))?;
        Ok(entry)
    }

    /// Closes check `id` with `evidence`, as `closed_by`, at `now`.
    pub fn close(&mut self, id: &str, evidence: &str, closed_by: &str, now: u64) -> Result<Owed> {
        let evidence = clean_line(evidence, MAX_EVIDENCE);
        ensure!(!evidence.is_empty(), "closing a check needs evidence: a path, a link or the owner's word");
        let closed_by = clean_line(closed_by, MAX_BY);
        ensure!(!closed_by.is_empty(), "a close says who closed it");
        let at = self.list.iter().position(|e| e.id == id).ok_or_else(|| anyhow!("no check owed with id `{}`", clean_line(id, 80)))?;
        ensure!(self.list[at].is_open(), "that check is closed already");
        let mut closed = self.list[at].clone();
        closed.closed_ms = Some(now);
        closed.closed_by = Some(closed_by);
        closed.evidence = Some(evidence);
        self.change(|list| {
            list[at] = closed.clone();
            // The history is bounded: the entry closed longest ago goes first.
            while list.iter().filter(|e| !e.is_open()).count() > MAX_CLOSED {
                let oldest = list.iter().enumerate().filter(|(_, e)| !e.is_open()).min_by_key(|(_, e)| e.closed_ms).map(|(i, _)| i);
                match oldest {
                    Some(i) => {
                        list.remove(i);
                    }
                    None => break,
                }
            }
        })?;
        Ok(closed)
    }

    /// Applies `edit` and saves; when the save fails the list is what it was.
    fn change(&mut self, edit: impl FnOnce(&mut Vec<Owed>)) -> Result<()> {
        let before = self.list.clone();
        edit(&mut self.list);
        if let Err(e) = self.save() {
            self.list = before;
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-owed-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn new(project: &str, what: &str) -> NewOwed {
        NewOwed { project: project.into(), pr: Some(7), commit: None, kind: Kind::RealApp, what: what.into(), closes_by: "a screenshot from the window helper".into() }
    }

    #[test]
    fn a_check_with_no_way_to_close_is_refused_and_an_open_one_needs_its_words() {
        let dir = scratch("refuse");
        let mut s = OwedStore::load(&dir);
        let mut no_way = new("p", "try the split picker");
        no_way.closes_by = " \u{1b}\n ".into();
        assert!(s.open(no_way, "user", 1).unwrap_err().to_string().contains("closes_by"));
        assert!(s.open(new("p", "  "), "user", 1).unwrap_err().to_string().contains("what"));
        assert!(s.open(new("", "x"), "user", 1).is_err(), "a check belongs to a project");
        let mut zero = new("p", "x");
        zero.pr = Some(0);
        assert!(s.open(zero, "user", 1).is_err());
        let mut bad_commit = new("p", "x");
        bad_commit.commit = Some("not-hex".into());
        assert!(s.open(bad_commit, "user", 1).is_err());
        assert!(s.list(None).is_empty(), "nothing refused is kept");
        assert!(!dir.join("owed.json").exists(), "and nothing was written");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn text_is_one_clean_bounded_line() {
        let dir = scratch("clean");
        let mut s = OwedStore::load(&dir);
        let mut n = new("p", "first\nsecond\u{1b}[2J\u{202e} third");
        n.closes_by = "x".repeat(MAX_CLOSES_BY + 50);
        let e = s.open(n, "user", 1).unwrap();
        assert_eq!(e.what, "first second[2J third");
        assert_eq!(e.closes_by.chars().count(), MAX_CLOSES_BY);
        let long = s.open(new("p", &"w".repeat(MAX_WHAT * 2)), "user", 2).unwrap();
        assert_eq!(long.what.chars().count(), MAX_WHAT);
        let mut with_commit = new("p", "commit case");
        with_commit.commit = Some(" ABCDEF1234 ".into());
        assert_eq!(s.open(with_commit, "user", 3).unwrap().commit.as_deref(), Some("abcdef1234"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_check_closes_once_and_only_with_evidence() {
        let dir = scratch("close");
        let mut s = OwedStore::load(&dir);
        let e = s.open(new("p", "try it on the phone"), "session:s1", 10).unwrap();
        assert_eq!((e.opened_by.as_str(), e.opened_ms, e.is_open()), ("session:s1", 10, true));
        assert!(s.close(&e.id, "  \u{7} ", "user", 20).unwrap_err().to_string().contains("evidence"));
        assert!(s.list(None)[0].is_open(), "a refused close changes nothing");
        assert!(s.close("nope", "ok", "user", 20).is_err());
        let closed = s.close(&e.id, "the owner looked: fine", "user", 20).unwrap();
        assert_eq!((closed.closed_ms, closed.closed_by.as_deref(), closed.evidence.as_deref()), (Some(20), Some("user"), Some("the owner looked: fine")));
        assert!(s.close(&e.id, "again", "user", 30).unwrap_err().to_string().contains("closed already"));
        assert_eq!(s.list(None)[0].evidence.as_deref(), Some("the owner looked: fine"), "the first close stands");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_same_open_check_asked_for_twice_is_one_entry() {
        let dir = scratch("same");
        let mut s = OwedStore::load(&dir);
        let a = s.open(new("p", "check the tray"), "session:s1", 1).unwrap();
        let b = s.open(new("p", "check the tray"), "session:s2", 2).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(s.list(None).len(), 1);
        let mut other_pr = new("p", "check the tray");
        other_pr.pr = Some(8);
        assert_ne!(s.open(other_pr, "user", 3).unwrap().id, a.id);
        // Once closed, the same words open a new entry: the check is owed again.
        s.close(&a.id, "done", "user", 4).unwrap();
        assert_ne!(s.open(new("p", "check the tray"), "user", 5).unwrap().id, a.id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn open_ones_come_first_oldest_first_then_the_closed_last_closed_first() {
        let dir = scratch("order");
        let mut s = OwedStore::load(&dir);
        let late = s.open(new("p", "late"), "user", 300).unwrap();
        let early = s.open(new("p", "early"), "user", 100).unwrap();
        let mid = s.open(new("p", "mid"), "user", 200).unwrap();
        let other = s.open(new("q", "other project"), "user", 150).unwrap();
        s.close(&mid.id, "e", "user", 400).unwrap();
        s.close(&other.id, "e", "user", 500).unwrap();
        let whats: Vec<String> = s.list(None).into_iter().map(|e| e.what).collect();
        assert_eq!(whats, ["early", "late", "other project", "mid"]);
        let of_p: Vec<String> = s.list(Some("p")).into_iter().map(|e| e.id).collect();
        assert_eq!(of_p, [early.id, late.id, mid.id]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_numbers_of_open_and_closed_entries_are_bounded() {
        let dir = scratch("limits");
        let mut s = OwedStore::load(&dir);
        for i in 0..MAX_OPEN_PER_PROJECT {
            s.open(new("p", &format!("check {i}")), "user", i as u64).unwrap();
        }
        assert!(s.open(new("p", "one too many"), "user", 99).unwrap_err().to_string().contains("50 checks"));
        // Another project still has room, up to the total.
        for project in 0..(MAX_OPEN / MAX_OPEN_PER_PROJECT - 1) {
            for i in 0..MAX_OPEN_PER_PROJECT {
                s.open(new(&format!("q{project}"), &format!("check {i}")), "user", 1).unwrap();
            }
        }
        assert!(s.open(new("full", "no room"), "user", 1).unwrap_err().to_string().contains("200 checks"));
        // Closed entries are kept up to a bound, the longest closed going first.
        let mut s3 = OwedStore::load(&scratch("closed"));
        let mut first = None;
        for i in 0..MAX_CLOSED + 3 {
            let e = s3.open(new("p", &format!("n{i}")), "user", i as u64).unwrap();
            s3.close(&e.id, "ok", "user", 1000 + i as u64).unwrap();
            first.get_or_insert(e.id);
        }
        let kept = s3.list(None);
        assert_eq!(kept.len(), MAX_CLOSED);
        assert!(kept.iter().all(|e| e.id != first.clone().unwrap()), "the one closed longest ago is dropped");
        assert_eq!(kept[0].what, format!("n{}", MAX_CLOSED + 2), "the last closed is first");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn entries_survive_a_restart() {
        let dir = scratch("persist");
        let mut s = OwedStore::load(&dir);
        let a = s.open(new("p", "one"), "session:s1", 1).unwrap();
        let b = s.open(new("p", "two"), "user", 2).unwrap();
        s.close(&a.id, "link", "user", 3).unwrap();
        let back = OwedStore::load(&dir);
        assert_eq!(back.list(None), s.list(None));
        assert_eq!(back.list(None).iter().map(|e| e.id.clone()).collect::<Vec<_>>(), [b.id, a.id]);
        assert!(back.unreadable.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_never_replaced_with_an_empty_list() {
        let dir = scratch("unreadable");
        let file = dir.join("owed.json");
        // Neither the file nor a previous version is usable: it holds something that is not a list.
        std::fs::write(&file, br#"{"not": "a list of checks"}"#).unwrap();
        let mut s = OwedStore::load(&dir);
        assert!(s.unreadable.is_some());
        assert!(s.list(None).is_empty());
        assert!(s.open(new("p", "x"), "user", 1).unwrap_err().to_string().contains("could not be read"));
        assert!(s.list(None).is_empty(), "a refused write leaves no entry in memory either");
        assert_eq!(std::fs::read(&file).unwrap(), br#"{"not": "a list of checks"}"#, "the file is as it was");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_report_item_is_read_from_json_with_its_mistakes_named() {
        let ok = NewOwed::from_json("p", &serde_json::json!({"kind": "phone", "what": "w", "closes_by": "c", "pr": 12})).unwrap();
        assert_eq!((ok.kind, ok.pr, ok.project.as_str()), (Kind::Phone, Some(12), "p"));
        let err = |v: Value| NewOwed::from_json("p", &v).unwrap_err().to_string();
        assert!(err(serde_json::json!({"kind": "later", "what": "w", "closes_by": "c"})).contains("real_app, phone, clean_machine, soak, owner"));
        assert!(err(serde_json::json!({"what": "w", "closes_by": "c"})).contains("kind must be one of"));
        assert!(err(serde_json::json!({"kind": "soak", "what": 5, "closes_by": "c"})).contains("`what` must be text"));
        assert!(err(serde_json::json!({"kind": "soak", "what": "w", "closes_by": "c", "pr": "7"})).contains("`pr`"));
        assert!(err(serde_json::json!("a check")).contains("must be an object"));
        // A missing text is for the store to refuse, saying which one.
        let missing = NewOwed::from_json("p", &serde_json::json!({"kind": "owner", "what": "w"})).unwrap();
        let dir = scratch("json");
        assert!(OwedStore::load(&dir).open(missing, "user", 1).unwrap_err().to_string().contains("closes_by"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
