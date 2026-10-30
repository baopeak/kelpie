//! What a phone may ask for: a closed table of verbs, each with the least permission it needs.
//! Anything else is refused. A phone is never a client of the daemon's own methods, so
//! `session.update`, `session.attach`, closing, phone settings and the other admin methods cannot
//! be reached by naming them.

/// What a paired device may do, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Permission {
    Read,
    Write,
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
            _ => return None,
        })
    }

    pub fn needs(self) -> Permission {
        match self {
            Verb::List | Verb::Hb | Verb::Ack | Verb::Sub | Verb::Unsub | Verb::ChatSub => Permission::Read,
            Verb::Input | Verb::Key | Verb::Answer | Verb::ChatAnswer | Verb::ChatSend | Verb::ChatCancel | Verb::ChatStop | Verb::ChatResume => Permission::Write,
        }
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

    const ALL: [Verb; 14] = [Verb::List, Verb::Hb, Verb::Ack, Verb::Sub, Verb::Unsub, Verb::ChatSub, Verb::Input, Verb::Key, Verb::Answer, Verb::ChatAnswer, Verb::ChatSend, Verb::ChatCancel, Verb::ChatStop, Verb::ChatResume];

    #[test]
    fn the_table_is_closed_and_every_verb_has_its_permission() {
        let names = ["list", "hb", "ack", "sub", "unsub", "chat_sub", "input", "key", "answer", "chat_answer", "chat_send", "chat_cancel", "chat_stop", "chat_resume"];
        for (name, verb) in names.iter().zip(ALL) {
            assert_eq!(Verb::parse(name), Some(verb), "{name}");
        }
        let reads = ["list", "hb", "ack", "sub", "unsub", "chat_sub"];
        for (name, verb) in names.iter().zip(ALL) {
            let expected = if reads.contains(name) { Permission::Read } else { Permission::Write };
            assert_eq!(verb.needs(), expected, "{name}");
        }
        // The daemon's own methods, admin ones above all, are not verbs, whatever they are named.
        for other in ["chat.send", "chat.queue_op", "chat.write_turn", "chat_queue", "chat_write", "session.update", "session.attach", "session.answer", "session.input", "session.close", "mobile.approve", "mobile.revoke", "daemon.shutdown", "read", "auth", "", "LIST", "list "] {
            assert_eq!(Verb::parse(other), None, "{other:?}");
        }
    }

    #[test]
    fn write_verbs_need_more_than_a_read_only_device_has() {
        let read_only = Permission::Read;
        let reachable: Vec<Verb> = ALL.into_iter().filter(|v| v.needs() <= read_only).collect();
        assert_eq!(reachable, [Verb::List, Verb::Hb, Verb::Ack, Verb::Sub, Verb::Unsub, Verb::ChatSub]);
        assert!(ALL.into_iter().all(|v| v.needs() <= Permission::Write));
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
