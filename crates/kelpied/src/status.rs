//! Agent/session activity state derived from hooks, bells and output activity.
//!
//! Hooks are authoritative when an agent reported one recently; otherwise output activity is
//! used, ignoring echo that directly follows user input.

use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Process alive, no recent output.
    Idle,
    /// Producing output / agent turn in progress.
    Working,
    /// Agent needs the user (permission prompt, question, bell).
    Waiting,
    /// Process exited (session may be restarted).
    Exited,
}

/// A working stretch shorter than this (e.g. a shell drawing its prompt) does not mark the
/// session unread when it goes idle.
pub const MIN_WORK_FOR_UNREAD: Duration = Duration::from_secs(3);
/// Output within this window counts as "working".
const ACTIVE_WINDOW: Duration = Duration::from_millis(1500);
/// Output this soon after input is treated as echo, not work.
const ECHO_WINDOW: Duration = Duration::from_millis(300);
/// A bell this soon after input is the program reacting to that key (no completion, start of
/// line), not asking for the user. Wider than the echo window: over SSH it takes a round trip.
const BELL_REACTION_WINDOW: Duration = Duration::from_secs(1);
/// A hook state older than this, while output keeps flowing, stops overriding activity.
const HOOK_STALE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct Activity {
    pub alive: bool,
    pub last_output: Option<Instant>,
    /// Start of the current continuous output burst.
    pub burst_start: Option<Instant>,
    pub last_input: Option<Instant>,
    /// The last keystroke of the user: what `last_input` is without the terminal's own reports
    /// (mouse, query replies). A usage-limit timer only cares about this.
    pub last_typed: Option<Instant>,
    /// The last keystroke that went to the prompt line (not to a permission dialog or question): what
    /// a typed "continue" would be appended to.
    pub last_draft: Option<Instant>,
    /// When the failure was reported.
    pub failed_at: Option<Instant>,
    /// A report of this agent came with the session's token: reports without one are then ignored
    /// (a program in the session cannot speak for the agent once the agent has proved itself).
    pub verified_hooks: bool,
    pub bell: Option<Instant>,
    pub hook: Option<(Status, Instant)>,
    /// The last agent turn ended in an error (API failure, rate limit); cleared once it works again.
    pub failed: bool,
    /// The failed turn ended on the subscription's usage limit (Claude Code's `StopFailure` with
    /// error `rate_limit`); only meaningful while `failed`.
    pub limited: bool,
    /// When the turn that ended in the failure began (its first working report): anything typed
    /// after that may be a draft still sitting in the agent's prompt, which a typed "continue" would send.
    pub turn_started: Option<Instant>,
    /// Counts the turns this run of the agent has begun; a timer set for one turn is void in another.
    pub turn_seq: u64,
    /// A timer for this limit was cancelled because the user typed: reports of a limit no longer
    /// count (a program in the session could otherwise re-arm it by forging one).
    pub limit_dead: bool,
    /// The agent's hooks report when a turn starts too (Claude Code), not only when it ends
    /// (Codex): once it says it is idle, it says so again when it works.
    pub hooks_report_start: bool,
    /// When a prompt waiting on the user (menu, yes/no) was found on the quiet screen
    /// (`choices::detect`): agents without hooks say they need the user only this way.
    pub screen_prompt: Option<Instant>,
}

impl Default for Activity {
    fn default() -> Self {
        Activity { alive: true, last_output: None, burst_start: None, last_input: None, last_typed: None, last_draft: None, failed_at: None, verified_hooks: false, bell: None, hook: None, failed: false, limited: false, turn_started: None, turn_seq: 0, limit_dead: false, hooks_report_start: false, screen_prompt: None }
    }
}

impl Activity {
    pub fn on_output(&mut self, now: Instant) {
        let continuing = self.last_output.is_some_and(|t| now.duration_since(t) < ACTIVE_WINDOW);
        if !continuing {
            self.burst_start = Some(now);
        }
        self.last_output = Some(now);
    }

    pub fn on_bell(&mut self, now: Instant) {
        if self.last_input.is_none_or(|i| now.duration_since(i) >= BELL_REACTION_WINDOW) {
            self.bell = Some(now);
        }
    }

    /// Records an agent-reported state. A failed turn stays failed through later idle reports
    /// and clears as soon as the agent works or asks for something again.
    pub fn on_hook(&mut self, status: Status, failed: bool, now: Instant) {
        // Work after idle (or the very first report) begins a turn; work after a question goes on with it.
        if status == Status::Working && !matches!(self.hook, Some((Status::Working | Status::Waiting, _))) {
            self.turn_started = Some(now);
            self.turn_seq += 1;
            // A genuine new turn: a later limit is a new limit.
            self.limit_dead = false;
        }
        self.hook = Some((status, now));
        self.hooks_report_start |= status == Status::Working;
        if failed {
            self.failed = true;
            self.failed_at = Some(now);
            self.limited = false;
        } else if status != Status::Idle {
            self.failed = false;
            self.limited = false;
        }
    }

    /// The failure just reported was the usage limit.
    pub fn on_limit(&mut self) {
        self.limited = self.failed && !self.limit_dead;
    }

    /// Still stopped by the usage limit, and nothing was typed since the failed turn began (the
    /// Enter that sent its prompt is the last keystroke allowed): no draft waits in the prompt.
    pub fn limit_untouched(&self) -> bool {
        self.failed && self.limited && self.turn_started.is_some_and(|t| self.last_draft.is_none_or(|i| i <= t)) && self.failed_at.is_some_and(|f| self.last_typed.is_none_or(|i| i <= f))
    }

    pub fn status(&self, now: Instant) -> Status {
        if !self.alive {
            return Status::Exited;
        }
        if let Some((hook_status, at)) = self.hook {
            if hook_status == Status::Idle {
                // An idle agent repaints its screen now and then (a fullscreen TUI's status line):
                // that is not work. One whose hooks announce its turns stays idle until the next
                // hook; one that reports only their end (Codex) works again once the user types.
                if self.hooks_report_start || self.last_input.is_none_or(|i| i <= at) {
                    return Status::Idle;
                }
            } else {
                // A turn whose end was never reported (interrupted): after a while, output decides.
                let output_after_hook = self.last_output.is_some_and(|o| o > at);
                if !(output_after_hook && now.duration_since(at) > HOOK_STALE) {
                    return hook_status;
                }
            }
        }
        if let Some(bell) = self.bell {
            if self.last_input.is_none_or(|i| i < bell) {
                return Status::Waiting;
            }
        }
        // Until the user answers or a later look finds the prompt gone (the tick looks again once
        // the screen is quiet). Output alone does not end it: TUIs repaint while they wait
        // (spinners, clocks, a tmux status line), and the wait must not flicker.
        if let Some(found) = self.screen_prompt {
            if self.last_input.is_none_or(|i| i < found) {
                return Status::Waiting;
            }
        }
        let recent = self.last_output.is_some_and(|o| now.duration_since(o) < ACTIVE_WINDOW);
        let echo_only = match (self.burst_start, self.last_input) {
            (Some(start), Some(input)) => {
                start >= input && start.duration_since(input) < ECHO_WINDOW
                    && self.last_output.is_some_and(|o| o.duration_since(start) < ECHO_WINDOW)
            }
            _ => false,
        };
        if recent && !echo_only { Status::Working } else { Status::Idle }
    }
}

/// Maps a hook event name (Claude Code / Codex) to the state it implies.
pub fn hook_event_status(agent: &str, event: &str, payload: &serde_json::Value) -> Option<Status> {
    match (agent, event) {
        (_, "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "SubagentStart") => {
            let asks_user = payload.get("tool_name").and_then(|v| v.as_str()) == Some("AskUserQuestion");
            Some(if asks_user && event == "PreToolUse" { Status::Waiting } else { Status::Working })
        }
        (_, "PermissionRequest") => Some(Status::Waiting),
        (_, "Notification") => {
            let kind = payload.get("notification_type").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "idle_prompt" => Some(Status::Idle),
                _ => Some(Status::Waiting),
            }
        }
        (_, "Stop" | "StopFailure" | "SessionStart" | "PostCompact") => Some(Status::Idle),
        // Codex `notify` only fires on turn completion.
        ("codex", "agent-turn-complete") => Some(Status::Idle),
        _ => None,
    }
}

/// The `session.turn` event the app reads a transcript on: a hook fired, or a turn ended. Built
/// here from the hook payload (daemon → app only; no RPC lets a program publish one). The
/// transcript path is the agent's claim about its own session: the app accepts it only for a file
/// named after that session's conversation id.
pub fn turn_event(session: &str, event: &str, payload: &serde_json::Value, agent_session_id: Option<&str>) -> serde_json::Value {
    let path = payload.get("transcript_path").and_then(|v| v.as_str()).filter(|p| !p.is_empty() && p.len() <= 4096);
    // The strings come from the program in the session: bounded, not interpreted here.
    let event: String = event.chars().take(64).collect();
    let conv = agent_session_id.filter(|c| c.len() <= 200);
    serde_json::json!({"session": session, "event": event, "transcript_path": path, "agent_session_id": conv})
}

/// Whether a hook event means the agent's turn ended in an error.
pub fn hook_event_failed(event: &str) -> bool {
    event == "StopFailure"
}

/// Whether a hook event means the agent's turn ended on the subscription's usage limit
/// (`StopFailure` carries the error type; only Claude Code reports one).
pub fn hook_event_limited(event: &str, payload: &serde_json::Value) -> bool {
    event == "StopFailure" && payload.get("error").and_then(|v| v.as_str()) == Some("rate_limit")
}

/// Whether a process exit code is a failure rather than a normal or Ctrl+C exit.
pub fn exit_failed(code: i32) -> bool {
    // 130 / 143: shell-reported SIGINT / SIGTERM; 0xC000013A: Windows STATUS_CONTROL_C_EXIT.
    !matches!(code, 0 | 130 | 143) && code as u32 != 0xC000_013A
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_is_not_work() {
        let t0 = Instant::now();
        let mut a = Activity { last_input: Some(t0), ..Default::default() };
        a.on_output(t0 + Duration::from_millis(20));
        assert_eq!(a.status(t0 + Duration::from_millis(100)), Status::Idle);
    }

    #[test]
    fn sustained_output_is_work_then_idle() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        for i in 0..10 {
            a.on_output(t0 + Duration::from_millis(i * 100));
        }
        assert_eq!(a.status(t0 + Duration::from_millis(1000)), Status::Working);
        assert_eq!(a.status(t0 + Duration::from_millis(4000)), Status::Idle);
    }

    #[test]
    fn hook_overrides_and_bell_waits() {
        let t0 = Instant::now();
        let mut a = Activity { hook: Some((Status::Waiting, t0)), ..Default::default() };
        assert_eq!(a.status(t0), Status::Waiting);
        a.hook = None;
        a.bell = Some(t0);
        assert_eq!(a.status(t0), Status::Waiting);
        a.last_input = Some(t0 + Duration::from_millis(1));
        assert_eq!(a.status(t0 + Duration::from_secs(5)), Status::Idle);
    }

    #[test]
    fn a_prompt_on_screen_waits_until_answered_or_replaced() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.on_output(t0);
        a.screen_prompt = Some(t0 + Duration::from_millis(800));
        assert_eq!(a.status(t0 + Duration::from_secs(5)), Status::Waiting);
        // The user answered.
        a.last_input = Some(t0 + Duration::from_secs(6));
        assert_eq!(a.status(t0 + Duration::from_secs(10)), Status::Idle);
        // A prompt found again later: repaints while it waits keep it waiting.
        a.screen_prompt = Some(t0 + Duration::from_secs(11));
        assert_eq!(a.status(t0 + Duration::from_secs(12)), Status::Waiting);
        a.on_output(t0 + Duration::from_secs(13));
        assert_eq!(a.status(t0 + Duration::from_secs(13)), Status::Waiting);
        // A later look finds it gone.
        a.screen_prompt = None;
        assert_ne!(a.status(t0 + Duration::from_secs(20)), Status::Waiting);
    }

    #[test]
    fn an_idle_agent_repainting_is_not_working() {
        let t0 = Instant::now();
        let burst = |a: &mut Activity, from: Duration| {
            for i in 0..10 {
                a.on_output(t0 + from + Duration::from_millis(i * 100));
            }
        };
        // Claude Code: hooks announce turns; minutes later its fullscreen view repaints.
        let mut claude = Activity::default();
        claude.on_hook(Status::Working, false, t0);
        claude.on_hook(Status::Idle, false, t0 + Duration::from_secs(1));
        burst(&mut claude, Duration::from_secs(300));
        assert_eq!(claude.status(t0 + Duration::from_millis(300_900)), Status::Idle);
        // Codex reports only turn ends: a repaint is not work, a typed prompt starts a turn.
        let mut codex = Activity::default();
        codex.on_hook(Status::Idle, false, t0);
        burst(&mut codex, Duration::from_secs(10));
        assert_eq!(codex.status(t0 + Duration::from_millis(10_900)), Status::Idle);
        codex.last_input = Some(t0 + Duration::from_secs(20));
        burst(&mut codex, Duration::from_secs(21));
        assert_eq!(codex.status(t0 + Duration::from_millis(21_900)), Status::Working);
        // A turn never reported finished still falls back to output after a while.
        let mut cut = Activity::default();
        cut.on_hook(Status::Working, false, t0);
        cut.on_output(t0 + Duration::from_secs(1));
        assert_eq!(cut.status(t0 + Duration::from_secs(200)), Status::Idle);
    }

    #[test]
    fn bell_answering_a_key_is_not_waiting() {
        let t0 = Instant::now();
        let mut a = Activity { last_input: Some(t0), ..Default::default() };
        // Tab with no completion over a slow link.
        a.on_bell(t0 + Duration::from_millis(400));
        assert_eq!(a.status(t0 + Duration::from_secs(5)), Status::Idle);
        a.on_bell(t0 + Duration::from_secs(30));
        assert_eq!(a.status(t0 + Duration::from_secs(31)), Status::Waiting);
    }

    #[test]
    fn maps_claude_events() {
        let none = serde_json::Value::Null;
        assert_eq!(hook_event_status("claude", "UserPromptSubmit", &none), Some(Status::Working));
        assert_eq!(hook_event_status("claude", "Stop", &none), Some(Status::Idle));
        let ask = serde_json::json!({"tool_name": "AskUserQuestion"});
        assert_eq!(hook_event_status("claude", "PreToolUse", &ask), Some(Status::Waiting));
        assert!(hook_event_failed("StopFailure"));
        assert!(!hook_event_failed("Stop"));
    }

    #[test]
    fn failure_survives_idle_and_clears_on_work() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.on_hook(Status::Idle, true, t0);
        assert!(a.failed);
        a.on_hook(Status::Idle, false, t0);
        assert!(a.failed);
        a.on_hook(Status::Working, false, t0);
        assert!(!a.failed);
    }

    #[test]
    fn only_the_rate_limit_error_is_the_usage_limit() {
        let rate = serde_json::json!({"error": "rate_limit"});
        assert!(hook_event_limited("StopFailure", &rate));
        assert!(!hook_event_limited("StopFailure", &serde_json::json!({"error": "server_error"})));
        assert!(!hook_event_limited("StopFailure", &serde_json::Value::Null));
        assert!(!hook_event_limited("Stop", &rate));
    }

    #[test]
    fn a_limit_stays_until_the_agent_works_and_typing_ends_it_for_the_timer() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut a = Activity::default();
        a.on_hook(Status::Working, false, at(0));
        a.on_hook(Status::Idle, true, at(1));
        a.on_limit();
        assert!(a.limit_untouched());
        // The user types something after the failure.
        a.last_typed = Some(at(5));
        assert!(!a.limit_untouched());
        assert!(a.limited, "the session is still limited, only the timer's guard is gone");
        // A later idle report keeps the failure; working clears the limit.
        a.on_hook(Status::Idle, false, at(6));
        assert!(a.limited);
        a.on_hook(Status::Working, false, at(7));
        assert!(!a.limited && !a.failed);
        // A failure that is not the limit never counts.
        a.on_hook(Status::Idle, true, at(8));
        assert!(!a.limit_untouched());
    }

    #[test]
    fn a_draft_typed_while_the_turn_ran_is_not_a_clean_stop() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut a = Activity { last_typed: Some(at(0)), ..Default::default() };
        // The prompt was sent at 0 (its Enter is the last keystroke before the turn began).
        a.on_hook(Status::Working, false, at(1));
        a.on_hook(Status::Working, false, at(2));
        a.on_hook(Status::Waiting, false, at(3));
        a.on_hook(Status::Working, false, at(4));
        assert_eq!(a.turn_seq, 1, "a question and its answer stay in one turn");
        a.on_hook(Status::Idle, true, at(10));
        a.on_limit();
        assert!(a.limit_untouched());
        // A draft typed during the turn would be submitted together with "continue".
        a.last_typed = Some(at(6));
        a.last_draft = Some(at(6));
        assert!(!a.limit_untouched());
    }

    #[test]
    fn a_draft_blocks_the_timer_wherever_it_was_typed_but_an_answer_to_a_dialog_does_not() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        // A turn that asks a question: working 1-3, dialog 3-6, working 6-10, fails at 10. `typed` says
        // when the user pressed a key, `draft` whether it went to the prompt line (not to the dialog).
        let run = |typed: Option<(u64, bool)>| {
            let mut a = Activity { last_typed: Some(at(0)), last_draft: Some(at(0)), ..Default::default() };
            a.on_hook(Status::Working, false, at(1));
            a.on_hook(Status::Waiting, false, at(3));
            a.on_hook(Status::Working, false, at(6));
            a.on_hook(Status::Idle, true, at(10));
            a.on_limit();
            if let Some((when, draft)) = typed {
                a.last_typed = Some(at(when));
                if draft {
                    a.last_draft = Some(at(when));
                }
            }
            assert_eq!(a.turn_seq, 1, "a question and its answer stay in one turn");
            a.limit_untouched()
        };
        assert!(run(None), "nothing typed during the turn: clean");
        assert!(!run(Some((2, true))), "a draft typed before the dialog came up is still in the prompt");
        assert!(run(Some((4, false))), "the answer went to the dialog");
        assert!(!run(Some((7, true))), "a draft typed after work resumed");
        assert!(!run(Some((11, false))), "anything typed after the failure");
    }

    #[test]
    fn a_later_genuine_limit_gets_its_banner_again_but_a_bare_report_does_not() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut a = Activity::default();
        a.on_hook(Status::Working, false, at(1));
        a.on_hook(Status::Idle, true, at(2));
        a.limit_dead = true; // a timer for it was refused: the user typed
        a.on_limit();
        assert!(!a.limited);
        // A bare report of the limit again (no new turn): still refused.
        a.on_hook(Status::Idle, true, at(3));
        a.on_limit();
        assert!(!a.limited && a.limit_dead);
        // The user continued by hand: a new turn begins, and it ends on the limit again.
        a.on_hook(Status::Working, false, at(100));
        assert!(!a.limit_dead);
        a.on_hook(Status::Idle, true, at(200));
        a.on_limit();
        assert!(a.limited && a.limit_untouched(), "the new limit has its banner and its timer");
    }

    #[test]
    fn a_cancelled_limit_cannot_be_reported_again_and_no_report_of_a_turn_means_no_timer() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.on_hook(Status::Idle, true, t0);
        a.on_limit();
        assert!(a.limited && !a.limit_untouched(), "no turn was seen to begin: nothing proves the prompt is empty");
        a.on_hook(Status::Working, false, t0 + Duration::from_secs(1));
        a.on_hook(Status::Idle, true, t0 + Duration::from_secs(2));
        a.limit_dead = true;
        a.on_limit();
        assert!(!a.limited, "a forged report does not bring the banner or the timer back");
    }

    #[test]
    fn ctrl_c_exits_are_not_failures() {
        assert!(!exit_failed(0));
        assert!(!exit_failed(130));
        assert!(!exit_failed(0xC000_013Au32 as i32));
        assert!(exit_failed(1));
        assert!(exit_failed(-1));
    }

    #[test]
    fn turn_event_carries_identity_only() {
        let payload = serde_json::json!({"hook_event_name": "Stop", "transcript_path": "/c/projects/p/abc.jsonl", "prompt": "secret"});
        let ev = turn_event("s1", "Stop", &payload, Some("abc"));
        assert_eq!(ev["session"], "s1");
        assert_eq!(ev["event"], "Stop");
        assert_eq!(ev["agent_session_id"], "abc");
        assert_eq!(ev["transcript_path"], "/c/projects/p/abc.jsonl");
        assert!(ev.get("prompt").is_none(), "nothing else of the payload is forwarded");
        let none = turn_event("s1", "turn_end", &serde_json::json!({"transcript_path": ""}), None);
        assert!(none["transcript_path"].is_null() && none["agent_session_id"].is_null());
        let long = turn_event("s1", "Stop", &serde_json::json!({"transcript_path": "x".repeat(5000)}), None);
        assert!(long["transcript_path"].is_null());
        let wordy = turn_event("s1", &"e".repeat(500), &serde_json::Value::Null, Some(&"i".repeat(500)));
        assert_eq!(wordy["event"].as_str().map(str::len), Some(64));
        assert!(wordy["agent_session_id"].is_null());
    }
}
