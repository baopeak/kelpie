//! What a phone may ask for: a closed table of verbs, each with the least permission it needs.
//! Anything else is refused. A phone is never a client of the daemon's own methods, so
//! `session.update`, `session.attach`, closing, phone settings and the other admin methods cannot
//! be reached by naming them.

/// What a paired device may do, least first. `Control` (taking over a browser pane, seeing and driving the screen) is the native app's, or a browser device's that paired on a secure origin with its app lock set up and on (`mobile::record_ceiling`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Read,
    Write,
    Control,
}

impl Permission {
    pub fn name(self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Control => "control",
        }
    }

    pub fn parse(s: &str) -> Option<Permission> {
        Some(match s {
            "read" => Permission::Read,
            "write" => Permission::Write,
            "control" => Permission::Control,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// The session list again.
    List,
    /// A heartbeat, either way: any message counts as proof of life, this one is only for that.
    Hb,
    /// The phone has applied screen frame `rev` of a session: frames follow it from now on.
    Ack,
    /// Follow a terminal session's screen.
    Sub,
    Unsub,
    /// Watch a chat session's log.
    ChatSub,
    /// Type a message on the prompt line.
    Input,
    Key,
    /// Answer the prompt a terminal session waits on.
    Answer,
    /// Answer a chat agent's request (permission, question).
    ChatAnswer,
    /// Queue a message for a chat's agent; the daemon writes it when the agent is idle.
    ChatSend,
    /// Take a message that still waits out of a chat's queue.
    ChatCancel,
    /// Stop the turn a chat's agent is running.
    ChatStop,
    /// End the wait a stop asked for.
    ChatResume,
    /// What this phone needs to subscribe to alerts: the key the computer signs with.
    PushInfo,
    /// This phone's subscription: the computer sends alerts to it from now on.
    PushSub,
    PushUnsub,
    /// A test alert to this phone.
    PushTest,
    /// A fresh challenge for a passkey ceremony (the last one may have expired on the screen).
    LockChallenge,
    /// A new passkey for this phone.
    LockEnrol,
    /// A passkey's assertion: this phone may go on.
    LockUnlock,
    /// The person left the app: it locks now.
    LockNow,
    /// The person did something, at most every 30 s: the idle time starts over.
    LockTouch,
    /// The app's browser panes this phone may take over (`takeover`).
    Panes,
    /// Take over a pane: its screen comes as `tk_frame`.
    TkStart,
    /// Taps, drags, scrolls and keys into the pane held.
    TkInput,
    /// Text into the pane held, with an answer of its own.
    TkText,
    /// The phone has shown a frame: it may have the next.
    TkAck,
    TkStop,
    /// The same device's new connection keeps the hold its dropped one had.
    TkRebind,
    /// The computer's monitors and windows a phone may see (`screen`).
    Screens,
    /// See one: its picture comes as binary media messages on the link.
    ScStart,
    ScStop,
    /// The phone has decoded a video frame.
    ScAck,
    /// Clicks, moves, the wheel and keys.
    ScInput,
    /// Text, with an answer of its own.
    ScText,
    /// The decoder lost its place: a keyframe, please.
    ScKey,
    ScRebind,
    /// Go on after a pause to protect a busy computer.
    ScResume,
}

impl Verb {
    pub fn parse(name: &str) -> Option<Verb> {
        Some(match name {
            "list" => Verb::List,
            "hb" => Verb::Hb,
            "ack" => Verb::Ack,
            "sub" => Verb::Sub,
            "unsub" => Verb::Unsub,
            "chat_sub" => Verb::ChatSub,
            "input" => Verb::Input,
            "key" => Verb::Key,
            "answer" => Verb::Answer,
            "chat_answer" => Verb::ChatAnswer,
            "chat_send" => Verb::ChatSend,
            "chat_cancel" => Verb::ChatCancel,
            "chat_stop" => Verb::ChatStop,
            "chat_resume" => Verb::ChatResume,
            "push_info" => Verb::PushInfo,
            "push_sub" => Verb::PushSub,
            "push_unsub" => Verb::PushUnsub,
            "push_test" => Verb::PushTest,
            "lock_challenge" => Verb::LockChallenge,
            "lock_enrol" => Verb::LockEnrol,
            "lock_unlock" => Verb::LockUnlock,
            "lock_now" => Verb::LockNow,
            "lock_touch" => Verb::LockTouch,
            "panes" => Verb::Panes,
            "tk_start" => Verb::TkStart,
            "tk_input" => Verb::TkInput,
            "tk_text" => Verb::TkText,
            "tk_ack" => Verb::TkAck,
            "tk_stop" => Verb::TkStop,
            "tk_rebind" => Verb::TkRebind,
            "screens" => Verb::Screens,
            "sc_start" => Verb::ScStart,
            "sc_stop" => Verb::ScStop,
            "sc_ack" => Verb::ScAck,
            "sc_input" => Verb::ScInput,
            "sc_text" => Verb::ScText,
            "sc_key" => Verb::ScKey,
            "sc_rebind" => Verb::ScRebind,
            "sc_resume" => Verb::ScResume,
            _ => return None,
        })
    }

    pub fn needs(self) -> Permission {
        match self {
            Verb::List | Verb::Hb | Verb::Ack | Verb::Sub | Verb::Unsub | Verb::ChatSub | Verb::PushInfo | Verb::PushSub | Verb::PushUnsub | Verb::PushTest | Verb::LockChallenge | Verb::LockEnrol | Verb::LockUnlock | Verb::LockNow | Verb::LockTouch => Permission::Read,
            Verb::Input | Verb::Key | Verb::Answer | Verb::ChatAnswer | Verb::ChatSend | Verb::ChatCancel | Verb::ChatStop | Verb::ChatResume => Permission::Write,
            Verb::Panes | Verb::TkStart | Verb::TkInput | Verb::TkText | Verb::TkAck | Verb::TkStop | Verb::TkRebind => Permission::Control,
            Verb::Screens | Verb::ScStart | Verb::ScStop | Verb::ScAck | Verb::ScInput | Verb::ScText | Verb::ScKey | Verb::ScRebind | Verb::ScResume => Permission::Control,
        }
    }

    /// Whether this is one of the control verbs (a browser pane's takeover or the screen), which a broker carries out (`takeover::Broker::phone_verb`,
    /// `screen::Broker::phone_verb`) and the audit log never sees the content of.
    pub fn takeover(self) -> bool {
        self.needs() == Permission::Control
    }

    /// Whether this is one of the screen's verbs (`screen`).
    pub fn screen(self) -> bool {
        matches!(self, Verb::Screens | Verb::ScStart | Verb::ScStop | Verb::ScAck | Verb::ScInput | Verb::ScText | Verb::ScKey | Verb::ScRebind | Verb::ScResume)
    }

    /// What may pass while the phone is locked: keeping the connection up, and getting out of the lock. Nothing here says anything about a session.
    pub fn passes_lock(self) -> bool {
        matches!(self, Verb::Hb | Verb::Ack | Verb::LockChallenge | Verb::LockEnrol | Verb::LockUnlock | Verb::LockNow | Verb::LockTouch)
    }
}

/// Bytes for a named key (the phone's quick keys): the keys a terminal program reads, and
/// `ctrl-<letter>` / `ctrl+<letter>` for any letter.
pub fn key_bytes(key: &str) -> Option<Vec<u8>> {
    if let Some(letter) = key.strip_prefix("ctrl-").or_else(|| key.strip_prefix("ctrl+")) {
        return match letter.as_bytes() {
            [c] if c.is_ascii_alphabetic() => Some(vec![c.to_ascii_lowercase() & 0x1f]),
            _ => None,
        };
    }
    Some(
        match key {
            "enter" => &b"\r"[..],
            "esc" => b"\x1b",
            "up" => b"\x1b[A",
            "down" => b"\x1b[B",
            "right" => b"\x1b[C",
            "left" => b"\x1b[D",
            "home" => b"\x1b[H",
            "end" => b"\x1b[F",
            "pgup" => b"\x1b[5~",
            "pgdn" => b"\x1b[6~",
            "tab" => b"\t",
            "shift-tab" => b"\x1b[Z",
            "backspace" => b"\x7f",
            "space" => b" ",
            "y" => b"y",
            "n" => b"n",
            "1" => b"1",
            "2" => b"2",
            "3" => b"3",
            _ => return None,
        }
        .to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Verb; 39] = [Verb::List, Verb::Hb, Verb::Ack, Verb::Sub, Verb::Unsub, Verb::ChatSub, Verb::Input, Verb::Key, Verb::Answer, Verb::ChatAnswer, Verb::ChatSend, Verb::ChatCancel, Verb::ChatStop, Verb::ChatResume, Verb::PushInfo, Verb::PushSub, Verb::PushUnsub, Verb::PushTest, Verb::LockChallenge, Verb::LockEnrol, Verb::LockUnlock, Verb::LockNow, Verb::LockTouch, Verb::Panes, Verb::TkStart, Verb::TkInput, Verb::TkText, Verb::TkAck, Verb::TkStop, Verb::TkRebind, Verb::Screens, Verb::ScStart, Verb::ScStop, Verb::ScAck, Verb::ScInput, Verb::ScText, Verb::ScKey, Verb::ScRebind, Verb::ScResume];

    #[test]
    fn the_table_is_closed_and_every_verb_has_its_permission() {
        let names = ["list", "hb", "ack", "sub", "unsub", "chat_sub", "input", "key", "answer", "chat_answer", "chat_send", "chat_cancel", "chat_stop", "chat_resume", "push_info", "push_sub", "push_unsub", "push_test", "lock_challenge", "lock_enrol", "lock_unlock", "lock_now", "lock_touch", "panes", "tk_start", "tk_input", "tk_text", "tk_ack", "tk_stop", "tk_rebind", "screens", "sc_start", "sc_stop", "sc_ack", "sc_input", "sc_text", "sc_key", "sc_rebind", "sc_resume"];
        assert_eq!(names.len(), ALL.len());
        for (name, verb) in names.iter().zip(ALL) {
            assert_eq!(Verb::parse(name), Some(verb), "{name}");
        }
        let reads = ["list", "hb", "ack", "sub", "unsub", "chat_sub", "push_info", "push_sub", "push_unsub", "push_test", "lock_challenge", "lock_enrol", "lock_unlock", "lock_now", "lock_touch"];
        for (name, verb) in names.iter().zip(ALL) {
            let screen = name.starts_with("sc_") || *name == "screens";
            let expected = if reads.contains(name) { Permission::Read } else if name.starts_with("tk_") || *name == "panes" || screen { Permission::Control } else { Permission::Write };
            assert_eq!(verb.needs(), expected, "{name}");
            assert_eq!(verb.takeover(), expected == Permission::Control, "{name}");
            assert_eq!(verb.screen(), screen, "{name}");
        }
        // The daemon's own methods, admin ones above all, are not verbs, whatever they are named.
        for other in ["tk_eval", "tk_cdp", "sc_eval", "sc_send_input", "screen.input", "screen.start", "screen.text", "browser.takeover_input", "browser.takeover_start", "provider.reply", "provider.register", "chat.send", "chat.queue_op", "chat.write_turn", "chat_queue", "chat_write", "session.update", "session.attach", "session.answer", "session.input", "session.close", "mobile.approve", "mobile.revoke", "daemon.shutdown", "notify.test", "notify.set", "push.sub", "push", "read", "auth", "", "LIST", "list "] {
            assert_eq!(Verb::parse(other), None, "{other:?}");
        }
    }

    #[test]
    fn write_verbs_need_more_than_a_read_only_device_has() {
        let read_only = Permission::Read;
        let reachable: Vec<Verb> = ALL.into_iter().filter(|v| v.needs() <= read_only).collect();
        assert_eq!(reachable, [Verb::List, Verb::Hb, Verb::Ack, Verb::Sub, Verb::Unsub, Verb::ChatSub, Verb::PushInfo, Verb::PushSub, Verb::PushUnsub, Verb::PushTest, Verb::LockChallenge, Verb::LockEnrol, Verb::LockUnlock, Verb::LockNow, Verb::LockTouch]);
        let writes: Vec<Verb> = ALL.into_iter().filter(|v| v.needs() <= Permission::Write).collect();
        assert_eq!(writes.len(), 23, "everything but taking over a pane or seeing the screen");
        let control: Vec<Verb> = ALL.into_iter().filter(|v| v.needs() == Permission::Control).collect();
        assert_eq!(control, [Verb::Panes, Verb::TkStart, Verb::TkInput, Verb::TkText, Verb::TkAck, Verb::TkStop, Verb::TkRebind, Verb::Screens, Verb::ScStart, Verb::ScStop, Verb::ScAck, Verb::ScInput, Verb::ScText, Verb::ScKey, Verb::ScRebind, Verb::ScResume]);
    }

    #[test]
    fn only_keeping_the_connection_up_and_getting_out_of_the_lock_pass_it() {
        let passing: Vec<Verb> = ALL.into_iter().filter(|v| v.passes_lock()).collect();
        assert_eq!(passing, [Verb::Hb, Verb::Ack, Verb::LockChallenge, Verb::LockEnrol, Verb::LockUnlock, Verb::LockNow, Verb::LockTouch]);
        for alerts in [Verb::PushInfo, Verb::PushSub, Verb::PushUnsub, Verb::PushTest] {
            assert!(!alerts.passes_lock(), "{alerts:?}");
        }
    }

    #[test]
    fn quick_keys() {
        assert_eq!(key_bytes("ctrl-c"), Some(vec![3]));
        assert_eq!(key_bytes("ctrl+D"), Some(vec![4]));
        assert_eq!(key_bytes("ctrl-z"), Some(vec![26]));
        assert_eq!(key_bytes("ctrl-l"), Some(vec![12]));
        assert_eq!(key_bytes("ctrl-r"), Some(vec![18]));
        assert_eq!(key_bytes("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_bytes("pgup"), Some(b"\x1b[5~".to_vec()));
        assert_eq!(key_bytes("backspace"), Some(vec![0x7f]));
        for name in ["pgdn", "home", "end", "left", "right", "space", "up", "down", "tab", "esc", "enter", "y", "n", "1", "2", "3"] {
            assert!(key_bytes(name).is_some(), "{name}");
        }
        for bad in ["rm -rf", "ctrl-", "ctrl-1", "ctrl-cc", "ctrl-é", "f13", ""] {
            assert_eq!(key_bytes(bad), None, "{bad:?}");
        }
    }
}
