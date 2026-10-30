//! What the daemon knows of each agent's wire dialect so that it can write a plain-text turn itself
//! (a phone's message, or a desktop message whose builder is gone): the conversation ids the
//! agent printed, the three user-turn builders, the interrupt builders, and the detection of turn
//! and interrupt lines written by someone else. Rich turns (pictures, skills, picks, client-side
//! commands) stay with the desktop adapters, which build them at dequeue.
//!
//! `turn-fixtures.json` holds one line per builder; a cargo test checks it against the builders
//! here and a vitest test checks the TypeScript adapters against it, so the two cannot drift.

use serde_json::{json, Value};

/// The agent's protocol, from the session's kind as `mode-switch.ts` picks the adapter: Claude and
/// Codex have their own, every other chat agent speaks ACP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Claude,
    Codex,
    Acp,
}

impl Dialect {
    pub fn of(kind: &str) -> Dialect {
        match kind {
            "claude" => Dialect::Claude,
            "codex" => Dialect::Codex,
            _ => Dialect::Acp,
        }
    }
}

/// The ids a turn or an interrupt is addressed with, as the agent last printed them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conversation {
    /// Codex thread.
    pub thread: Option<String>,
    /// ACP session.
    pub session: Option<String>,
    /// Codex turn in progress.
    pub turn: Option<String>,
    /// The ACP session a `session/load` asked for: its answer carries no id of its own.
    loading: Option<String>,
}

impl Conversation {
    /// Learns from a line the agent printed.
    pub fn learn_agent_line(&mut self, dialect: Dialect, v: &Value) {
        match dialect {
            Dialect::Claude => {}
            Dialect::Codex => {
                if v.get("id").and_then(Value::as_str) == Some("kelpie-thread") {
                    if let Some(id) = v.pointer("/result/thread/id").and_then(Value::as_str) {
                        self.thread = Some(id.to_string());
                        self.turn = None;
                    }
                }
                match v.get("method").and_then(Value::as_str) {
                    Some("turn/started") => {
                        let id = v.pointer("/params/turn/id").or_else(|| v.pointer("/params/turnId")).and_then(Value::as_str);
                        self.turn = id.map(str::to_string);
                    }
                    Some("turn/completed") => self.turn = None,
                    _ => {}
                }
            }
            Dialect::Acp => {
                if v.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("kelpie-session")) && v.get("result").is_some() {
                    let id = v.pointer("/result/sessionId").and_then(Value::as_str).map(str::to_string).or_else(|| self.loading.take());
                    if id.is_some() {
                        self.session = id;
                    }
                }
            }
        }
    }

    /// Learns from a line written to the agent (by the desktop, or by this daemon): a request that
    /// opens another conversation makes the old id wrong until the answer comes.
    pub fn learn_written(&mut self, dialect: Dialect, v: &Value) {
        match (dialect, v.get("method").and_then(Value::as_str)) {
            (Dialect::Codex, Some("thread/start" | "thread/resume")) => {
                self.thread = None;
                self.turn = None;
            }
            (Dialect::Acp, Some("session/new")) => {
                self.session = None;
                self.loading = None;
            }
            (Dialect::Acp, Some("session/load")) => {
                self.session = None;
                self.loading = v.pointer("/params/sessionId").and_then(Value::as_str).map(str::to_string);
            }
            _ => {}
        }
    }

    /// Whether a turn can be addressed to the agent: Codex and ACP need the conversation.
    pub fn ready(&self, dialect: Dialect) -> bool {
        match dialect {
            Dialect::Claude => true,
            Dialect::Codex => self.thread.is_some(),
            Dialect::Acp => self.session.is_some(),
        }
    }
}

/// The line that starts a turn with `text` (plain text, no pictures, no overrides). `n` and `ms`
/// make the request id unique; `None` when the conversation is not known yet.
pub fn user_turn(dialect: Dialect, conv: &Conversation, text: &str, n: u64, ms: u64) -> Option<String> {
    let line = match dialect {
        Dialect::Claude => json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}}),
        Dialect::Codex => json!({
            "id": format!("kelpie-turn-d{n}-{ms}"), "method": "turn/start",
            "params": {"threadId": conv.thread.as_deref()?, "input": [{"type": "text", "text": text, "text_elements": []}]},
        }),
        Dialect::Acp => json!({
            "jsonrpc": "2.0", "id": format!("kelpie-prompt-d{n}-{ms}"), "method": "session/prompt",
            "params": {"sessionId": conv.session.as_deref()?, "prompt": [{"type": "text", "text": text}]},
        }),
    };
    Some(line.to_string())
}

/// The line that stops the turn in progress; `None` when there is nothing to address it to (Codex
/// needs the turn's id: before it started there is nothing to stop).
pub fn interrupt(dialect: Dialect, conv: &Conversation, n: u64) -> Option<String> {
    let line = match dialect {
        Dialect::Claude => json!({"type": "control_request", "request_id": format!("kelpie-interrupt-d{n}"), "request": {"subtype": "interrupt"}}),
        Dialect::Codex => json!({
            "id": format!("kelpie-int-d{n}"), "method": "turn/interrupt",
            "params": {"threadId": conv.thread.as_deref()?, "turnId": conv.turn.as_deref()?},
        }),
        Dialect::Acp => json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": conv.session.as_deref()?}}),
    };
    Some(line.to_string())
}

/// What a line written to the agent is, as far as the queue cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// Starts a turn; `req` is the id of its request, whose error answer ends the turn.
    Turn { req: Option<String> },
    Interrupt,
    Other,
}

pub fn written(dialect: Dialect, v: &Value) -> Written {
    let method = v.get("method").and_then(Value::as_str);
    let req = || v.get("id").and_then(super::id_key);
    match dialect {
        Dialect::Claude => match v.get("type").and_then(Value::as_str) {
            Some("user") => Written::Turn { req: None },
            Some("control_request") if v.pointer("/request/subtype").and_then(Value::as_str) == Some("interrupt") => Written::Interrupt,
            _ => Written::Other,
        },
        Dialect::Codex => match method {
            Some("turn/start" | "thread/compact/start" | "review/start") => Written::Turn { req: req() },
            Some("turn/interrupt") => Written::Interrupt,
            _ => Written::Other,
        },
        Dialect::Acp => match method {
            Some("session/prompt") => Written::Turn { req: req() },
            Some("session/cancel") => Written::Interrupt,
            _ => Written::Other,
        },
    }
}

/// Whether `text` is something the desktop's client runs itself instead of sending to the agent.
/// Codex: every `/command` and `!shell`. ACP: the commands that start another conversation.
pub fn needs_desktop(dialect: Dialect, text: &str) -> bool {
    // A byte-order mark is not whitespace to Rust, but the desktop's JS trims it: both sides agree.
    let text = text.trim().trim_start_matches('\u{feff}').trim_start();
    match dialect {
        Dialect::Claude => false,
        Dialect::Codex => text.starts_with('/') || text.starts_with('!'),
        Dialect::Acp => text.strip_prefix('/').is_some_and(|rest| {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or("");
            matches!(name, "new" | "clear" | "reset") && parts.next().is_none_or(|args| args.trim().is_empty())
        }),
    }
}

/// Whether the desktop may leave `text` to the daemon when its own builder is gone: plain text.
pub fn plain(text: &str) -> bool {
    let text = text.trim_start();
    !text.starts_with('/') && !text.starts_with('!')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> Value {
        serde_json::from_str(include_str!("turn-fixtures.json")).unwrap()
    }

    /// The request ids carry a counter and a time: they are not part of the format.
    fn without_ids(mut v: Value) -> Value {
        if let Some(o) = v.as_object_mut() {
            for key in ["id", "request_id"] {
                if o.contains_key(key) {
                    o.insert(key.into(), json!("*"));
                }
            }
            if let Some(r) = o.get_mut("request").and_then(Value::as_object_mut) {
                r.remove("request_id");
            }
        }
        v
    }

    fn conv() -> Conversation {
        Conversation { thread: Some("T1".into()), session: Some("S1".into()), turn: Some("TURN1".into()), loading: None }
    }

    #[test]
    fn the_builders_match_the_shared_fixture() {
        let f = fixtures();
        let conv = conv();
        for (name, dialect) in [("claude", Dialect::Claude), ("codex", Dialect::Codex), ("acp", Dialect::Acp)] {
            let turn: Value = serde_json::from_str(&user_turn(dialect, &conv, "hello", 1, 2).unwrap()).unwrap();
            assert_eq!(without_ids(turn), without_ids(f[name]["turn"].clone()), "{name} turn");
            let stop: Value = serde_json::from_str(&interrupt(dialect, &conv, 1).unwrap()).unwrap();
            assert_eq!(without_ids(stop), without_ids(f[name]["interrupt"].clone()), "{name} interrupt");
        }
    }

    #[test]
    fn what_the_builders_write_is_recognised_as_a_turn_or_an_interrupt() {
        let conv = conv();
        for dialect in [Dialect::Claude, Dialect::Codex, Dialect::Acp] {
            let turn: Value = serde_json::from_str(&user_turn(dialect, &conv, "hi", 3, 4).unwrap()).unwrap();
            assert!(matches!(written(dialect, &turn), Written::Turn { .. }), "{dialect:?}");
            let stop: Value = serde_json::from_str(&interrupt(dialect, &conv, 3).unwrap()).unwrap();
            assert_eq!(written(dialect, &stop), Written::Interrupt, "{dialect:?}");
        }
        let codex: Value = serde_json::from_str(&user_turn(Dialect::Codex, &conv, "hi", 3, 4).unwrap()).unwrap();
        assert_eq!(written(Dialect::Codex, &codex), Written::Turn { req: Some("s:kelpie-turn-d3-4".into()) });
        for line in [
            (Dialect::Claude, json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r"}})),
            (Dialect::Claude, json!({"type": "control_request", "request_id": "x", "request": {"subtype": "set_permission_mode"}})),
            (Dialect::Codex, json!({"id": "s", "method": "turn/steer", "params": {}})),
            (Dialect::Codex, json!({"id": "s", "method": "thread/shellCommand", "params": {}})),
            (Dialect::Acp, json!({"id": "s", "method": "session/set_config_option", "params": {}})),
        ] {
            assert_eq!(written(line.0, &line.1), Written::Other, "{}", line.1);
        }
        assert!(matches!(written(Dialect::Codex, &json!({"id": 4, "method": "review/start"})), Written::Turn { .. }));
        assert!(matches!(written(Dialect::Codex, &json!({"id": 4, "method": "thread/compact/start"})), Written::Turn { .. }));
    }

    #[test]
    fn nothing_is_built_without_the_conversation_it_is_addressed_to() {
        let none = Conversation::default();
        assert!(user_turn(Dialect::Claude, &none, "hi", 1, 1).is_some(), "Claude needs no id");
        assert!(user_turn(Dialect::Codex, &none, "hi", 1, 1).is_none());
        assert!(user_turn(Dialect::Acp, &none, "hi", 1, 1).is_none());
        assert!(!none.ready(Dialect::Codex) && !none.ready(Dialect::Acp) && none.ready(Dialect::Claude));
        // Codex stops a turn by its id: before one started there is nothing to stop.
        let thread_only = Conversation { thread: Some("T".into()), ..Default::default() };
        assert!(interrupt(Dialect::Codex, &thread_only, 1).is_none());
        assert!(interrupt(Dialect::Acp, &none, 1).is_none());
        assert!(interrupt(Dialect::Claude, &none, 1).is_some());
    }

    #[test]
    fn conversation_ids_come_from_the_handshake_answers() {
        let mut c = Conversation::default();
        c.learn_agent_line(Dialect::Codex, &json!({"id": "kelpie-thread", "result": {"thread": {"id": "T9"}}}));
        c.learn_agent_line(Dialect::Codex, &json!({"method": "turn/started", "params": {"turn": {"id": "U1"}}}));
        assert_eq!((c.thread.as_deref(), c.turn.as_deref()), (Some("T9"), Some("U1")));
        c.learn_agent_line(Dialect::Codex, &json!({"method": "turn/completed", "params": {"turn": {"id": "U1"}}}));
        assert_eq!(c.turn, None);
        // /new: the old thread is wrong from the request on, until the answer names the new one.
        c.learn_written(Dialect::Codex, &json!({"id": "kelpie-thread", "method": "thread/start", "params": {}}));
        assert!(!c.ready(Dialect::Codex));
        c.learn_agent_line(Dialect::Codex, &json!({"id": "kelpie-thread", "result": {"thread": {"id": "T10"}}}));
        assert_eq!(c.thread.as_deref(), Some("T10"));

        let mut a = Conversation::default();
        a.learn_written(Dialect::Acp, &json!({"id": "kelpie-session", "method": "session/load", "params": {"sessionId": "OLD"}}));
        a.learn_agent_line(Dialect::Acp, &json!({"id": "kelpie-session", "result": {}}));
        assert_eq!(a.session.as_deref(), Some("OLD"), "a load answers without an id");
        a.learn_written(Dialect::Acp, &json!({"id": "kelpie-session-new", "method": "session/new", "params": {}}));
        assert!(!a.ready(Dialect::Acp));
        a.learn_agent_line(Dialect::Acp, &json!({"id": "kelpie-session-new", "result": {"sessionId": "NEW"}}));
        assert_eq!(a.session.as_deref(), Some("NEW"));
        // Other answers name no conversation.
        a.learn_agent_line(Dialect::Acp, &json!({"id": "kelpie-config-3", "result": {"sessionId": "X"}}));
        assert_eq!(a.session.as_deref(), Some("NEW"));
    }

    #[test]
    fn commands_the_client_runs_stay_with_the_desktop() {
        for text in ["/compact", "/review now", "/new", "!ls -la", "  /model x"] {
            assert!(needs_desktop(Dialect::Codex, text), "{text}");
        }
        assert!(!needs_desktop(Dialect::Codex, "fix the bug in a/b"));
        for text in ["/new", "/clear", "/reset", " /new  "] {
            assert!(needs_desktop(Dialect::Acp, text), "{text}");
        }
        for text in ["/new project", "/help", "new", "/newer"] {
            assert!(!needs_desktop(Dialect::Acp, text), "{text}");
        }
        assert!(!needs_desktop(Dialect::Claude, "/compact"), "the Claude CLI runs its own commands");
        assert!(plain("hello") && !plain("/x") && !plain("  !x"));
    }
}
