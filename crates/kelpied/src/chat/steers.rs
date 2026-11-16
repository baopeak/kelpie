//! Claude: what the person said during a running turn, and whether a `result` ends that turn.
//!
//! A user line written to `claude -p` while it works is read at its next step and echoed then (`--replay-user-messages`,
//! `isReplay`). When the run ends first, by itself or by a stop, the CLI prints its `result` and at once starts another run for
//! the lines it has not read (`system/init`, then their echo): the person's turn goes on (verified with Claude Code 2.1.325). So a
//! `result` that leaves such a line unread is not the end of the turn. The rule goes by what was written and echoed, not by time;
//! time only lets go of a held turn that showed no sign of going on (`GRACE`).

use std::time::{Duration, Instant};

use serde_json::Value;

/// A held `result` that no new run or work followed for this long did not start another run after all: the turn is over.
pub const GRACE: Duration = Duration::from_secs(15);

#[derive(Debug, Default)]
pub struct Steers {
    /// The user lines written this turn and not echoed yet, oldest first, by text; `true` for one written while the turn ran (the
    /// line that started the turn is kept too, so that its echo cannot be taken for a later line's of the same text).
    unread: Vec<(String, bool)>,
    /// How many lines written during the turn were unread when a `result` was last held for them (none held this turn: `None`).
    /// A later `result` that finds no fewer ends the turn, so a line whose echo never comes cannot keep the session working.
    held_for: Option<usize>,
    /// When the held `result` came, until the CLI shows that it went on (a new run, work).
    held_at: Option<Instant>,
    /// The held `result` reported an error: what the turn ends with if the CLI does not go on after all.
    held_failed: bool,
}

impl Steers {
    /// A user line started a turn: what was owed for an earlier one is over.
    pub fn started(&mut self, v: &Value) {
        self.clear();
        self.unread.push((text_of(v), false));
    }

    /// A user line was written during a running turn.
    pub fn written(&mut self, v: &Value) {
        self.unread.push((text_of(v), true));
    }

    /// The CLI echoed a user line (`echo_text`): the oldest unread line of that text is read. The CLI reads lines in the order
    /// they were written, so two of the same text are taken oldest first.
    pub fn echoed(&mut self, text: &str) {
        if let Some(i) = self.unread.iter().position(|(t, _)| t == text) {
            self.unread.remove(i);
        }
    }

    /// The CLI started a run or works: a held `result` was followed by more, as it should be.
    pub fn went_on(&mut self) {
        self.held_at = None;
    }

    /// A `result` came (`failed`: it reports an error): true when the CLI goes on with an unread line (the turn is held), false
    /// when the turn ends.
    pub fn result(&mut self, now: Instant, failed: bool) -> bool {
        let n = self.unread.iter().filter(|(_, during)| *during).count();
        if n > 0 && self.held_for.is_none_or(|before| n < before) {
            self.unread.retain(|(_, during)| *during);
            self.held_for = Some(n);
            self.held_at = Some(now);
            self.held_failed = failed;
            return true;
        }
        self.clear();
        false
    }

    /// The turn is over: nothing is owed.
    pub fn clear(&mut self) {
        *self = Steers::default();
    }

    /// A held `result` that nothing followed for `GRACE`: `Some(failed)` with what it reported.
    pub fn expired(&self, now: Instant) -> Option<bool> {
        self.held_at.filter(|at| now.duration_since(*at) > GRACE).map(|_| self.held_failed)
    }
}

/// The text of the user line a Claude echo (`type: user`, `isReplay: true`) repeats, or `None` for any other line.
pub fn echo_text(line: &str) -> Option<String> {
    if !line.contains("isReplay") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    (v.get("type").and_then(Value::as_str) == Some("user") && v.get("isReplay").and_then(Value::as_bool) == Some(true)).then(|| text_of(&v))
}

/// Whether a line is Claude's `system/init`, which starts each run.
pub fn starts_run(line: &str) -> bool {
    line.contains("\"init\"")
        && line.contains("\"system\"")
        && serde_json::from_str::<Value>(line).is_ok_and(|v| v.get("type").and_then(Value::as_str) == Some("system") && v.get("subtype").and_then(Value::as_str) == Some("init"))
}

/// The text of a Claude user line (a string, or its text blocks), which its echo repeats.
fn text_of(v: &Value) -> String {
    let text = match v.pointer("/message/content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text")).filter_map(|b| b.get("text").and_then(Value::as_str)).collect(),
        _ => String::new(),
    };
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(text: &str) -> Value {
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}})
    }

    /// An echo with a picture the daemon took the bytes out of, and its text, if any.
    fn echo(text: &str) -> String {
        let mut content = vec![json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "kelpie_stripped": 100}})];
        if !text.is_empty() {
            content.push(json!({"type": "text", "text": text}));
        }
        json!({"type": "user", "isReplay": true, "message": {"role": "user", "content": content}}).to_string()
    }

    fn read(s: &mut Steers, line: &str) {
        if let Some(t) = echo_text(line) {
            s.echoed(&t);
        }
    }

    #[test]
    fn a_line_read_before_the_result_ends_nothing_early() {
        let mut s = Steers::default();
        s.started(&user("go"));
        read(&mut s, &echo("go"));
        s.written(&user("also BANANA"));
        read(&mut s, &echo("also BANANA"));
        assert!(!s.result(Instant::now(), false));
    }

    #[test]
    fn a_line_left_unread_holds_the_result_until_the_run_that_reads_it_ends() {
        let mut s = Steers::default();
        s.started(&user("go"));
        s.written(&user("also BANANA"));
        let now = Instant::now();
        assert!(s.result(now, false));
        assert_eq!(s.held_at, Some(now));
        s.went_on();
        assert_eq!(s.expired(now + GRACE * 2), None, "the CLI went on");
        read(&mut s, &echo("also BANANA"));
        assert!(!s.result(Instant::now(), false));
    }

    #[test]
    fn an_echo_that_never_comes_holds_one_result_only() {
        let mut s = Steers::default();
        s.started(&user("go"));
        s.written(&user("lost"));
        assert!(s.result(Instant::now(), false));
        assert!(!s.result(Instant::now(), false));
        assert!(!s.result(Instant::now(), false), "and nothing is owed afterwards");
    }

    #[test]
    fn two_unread_lines_hold_as_long_as_each_run_reads_one() {
        let mut s = Steers::default();
        s.started(&user("one"));
        s.written(&user("two"));
        s.written(&user("three"));
        assert!(s.result(Instant::now(), false));
        read(&mut s, &echo("two"));
        assert!(s.result(Instant::now(), false));
        read(&mut s, &echo("three"));
        assert!(!s.result(Instant::now(), false));
    }

    #[test]
    fn the_starting_line_s_echo_is_not_taken_for_a_later_line_of_the_same_text() {
        // Pictures alone have no text: the turn's own echo, late, must not read the picture sent during it.
        let mut s = Steers::default();
        s.started(&user(""));
        s.written(&user(""));
        read(&mut s, &echo(""));
        assert!(s.result(Instant::now(), false), "the second picture is still unread");
        read(&mut s, &echo(""));
        assert!(!s.result(Instant::now(), false));
    }

    #[test]
    fn a_new_turn_forgets_what_an_earlier_one_owed() {
        let mut s = Steers::default();
        s.started(&user("one"));
        s.written(&user("never echoed"));
        // That turn ended without a result (a stop the agent never answered, a watchdog): the next one starts clean.
        s.started(&user("two"));
        read(&mut s, &echo("two"));
        assert!(!s.result(Instant::now(), false));
    }

    #[test]
    fn only_an_echo_reads_a_line() {
        let mut s = Steers::default();
        s.started(&user("go"));
        s.written(&user("ok"));
        // A tool result, the CLI's own interrupt marker and an assistant line are not the echo.
        read(&mut s, r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#);
        read(&mut s, r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#);
        read(&mut s, r#"{"type":"assistant","isReplay":true,"message":{"content":[{"type":"text","text":"ok"}]}}"#);
        assert!(s.result(Instant::now(), false));
    }

    #[test]
    fn a_held_result_expires_after_the_grace_with_its_own_failure() {
        let mut s = Steers::default();
        s.started(&user("go"));
        s.written(&user("x"));
        let now = Instant::now();
        assert!(s.result(now, true));
        assert_eq!(s.expired(now + GRACE), None);
        assert_eq!(s.expired(now + GRACE + Duration::from_secs(1)), Some(true));
    }

    #[test]
    fn a_run_starts_with_system_init() {
        assert!(starts_run(r#"{"type":"system","subtype":"init","session_id":"c1"}"#));
        assert!(!starts_run(r#"{"type":"system","subtype":"status"}"#));
        assert!(!starts_run(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"{\"type\":\"system\",\"subtype\":\"init\"}"}]}}"#));
    }
}
