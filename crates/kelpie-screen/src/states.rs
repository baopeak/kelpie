//! What the phone is told about the picture, so that it never shows a frozen screen as if it were live, and what input may do then.
//!
//! The native side looks four times a second (`win::states_win::observe`) and hands over what it saw; this decides. A program in the
//! user's session cannot see or drive the secure desktop (UAC, the lock and sign-in screens: that needs a SYSTEM service, a non-goal), and
//! Windows drops input into a window of a higher integrity level (UIPI) without a word: both are said plainly instead.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    /// The user's own desktop.
    Default,
    /// The secure desktop: a UAC prompt, the lock screen, sign-in, Ctrl+Alt+Del.
    Secure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetSeen {
    Shown,
    Minimized,
    Closed,
    /// A window target with another window over the point it was asked to click (window targets only).
    Covered,
}

/// One look at the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    pub desktop: Desktop,
    pub session_locked: bool,
    /// The window in front runs at a higher integrity level than the helper (an app run as administrator).
    pub foreground_elevated: bool,
    /// The target window itself does (window targets only).
    pub target_elevated: bool,
    pub target: TargetSeen,
}

impl Observed {
    pub fn normal() -> Observed {
        Observed { desktop: Desktop::Default, session_locked: false, foreground_elevated: false, target_elevated: false, target: TargetSeen::Shown }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Live,
    /// The computer is locked: nothing can be seen or done until someone signs in at it.
    Locked,
    /// A UAC prompt or another secure screen is up.
    SecureDesktop,
    Closed,
    Minimized,
    /// An app run as administrator is in front (a monitor) or is the target (a window).
    Elevated,
    /// Another window is over the target where it was clicked.
    Covered,
    /// A dialog another program made and gave the shared window as its owner (a credential prompt, another program's file picker) is in
    /// front: the phone does not see it, so nothing is typed or clicked into it; the person can share the whole monitor to use it.
    CoveredByDialog,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Live => "live",
            State::Locked => "locked",
            State::SecureDesktop => "secure_desktop",
            State::Closed => "closed",
            State::Minimized => "minimized",
            State::Elevated => "elevated",
            State::Covered => "covered",
            State::CoveredByDialog => "covered_dialog",
        }
    }

    /// Whether the picture can still change: frames are expected (and the phone should not count their absence as a freeze).
    pub fn picture_moves(self) -> bool {
        matches!(self, State::Live | State::Elevated | State::Covered | State::CoveredByDialog)
    }

    /// Whether the stream ends rather than waits.
    pub fn ends(self) -> bool {
        self == State::Closed
    }
}

/// The state one look says, worst first: a closed target ends everything; locked and secure screens hide everything else.
pub fn decide(o: &Observed, window_target: bool) -> State {
    if window_target && o.target == TargetSeen::Closed {
        return State::Closed;
    }
    if o.session_locked {
        return State::Locked;
    }
    if o.desktop == Desktop::Secure {
        return State::SecureDesktop;
    }
    if window_target && o.target == TargetSeen::Minimized {
        return State::Minimized;
    }
    if (window_target && o.target_elevated) || (!window_target && o.foreground_elevated) {
        return State::Elevated;
    }
    if window_target && o.target == TargetSeen::Covered {
        return State::Covered;
    }
    State::Live
}

/// What input may do in a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputPolicy {
    pub mouse: bool,
    pub keys: bool,
}

/// A monitor with an elevated window in front still takes clicks (on another window, to get away from it), but not keys, which would go to
/// that window and be dropped. An elevated window target takes nothing. A minimized window is restored by a click (the native side does it).
pub fn input_policy(state: State, window_target: bool) -> InputPolicy {
    match state {
        State::Live | State::Covered | State::CoveredByDialog => InputPolicy { mouse: true, keys: true },
        State::Elevated if !window_target => InputPolicy { mouse: true, keys: false },
        State::Minimized => InputPolicy { mouse: true, keys: false },
        _ => InputPolicy { mouse: false, keys: false },
    }
}

/// Turns looks into changes worth telling: a state is reported once it held for two looks in a row (a flicker of focus is not news), except
/// what makes the picture wrong now (closed, locked, a secure screen), which is reported at once. Covered is set by input, not by looking,
/// and is cleared by the next look that does not see it.
#[derive(Debug)]
pub struct Tracker {
    told: State,
    candidate: Option<State>,
}

impl Default for Tracker {
    fn default() -> Tracker {
        Tracker { told: State::Live, candidate: None }
    }
}

impl Tracker {
    pub fn state(&self) -> State {
        self.told
    }

    /// One look; the new state when it should be told.
    pub fn look(&mut self, o: &Observed, window_target: bool) -> Option<State> {
        let now = decide(o, window_target);
        if now == self.told {
            self.candidate = None;
            return None;
        }
        let urgent = matches!(now, State::Closed | State::Locked | State::SecureDesktop);
        if urgent || self.candidate == Some(now) {
            self.told = now;
            self.candidate = None;
            return Some(now);
        }
        self.candidate = Some(now);
        None
    }

    /// A click was dropped because another window covers the target there.
    pub fn covered(&mut self) -> Option<State> {
        (self.told == State::Live).then(|| {
            self.told = State::Covered;
            State::Covered
        })
    }

    /// Keys or a click were refused because another program's dialog, owned by the target, is in front. Cleared like `Covered`.
    pub fn covered_by_dialog(&mut self) -> Option<State> {
        matches!(self.told, State::Live | State::Covered).then(|| {
            self.told = State::CoveredByDialog;
            State::CoveredByDialog
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(f: impl FnOnce(&mut Observed)) -> Observed {
        let mut o = Observed::normal();
        f(&mut o);
        o
    }

    #[test]
    fn the_worst_thing_seen_is_the_state() {
        assert_eq!(decide(&Observed::normal(), true), State::Live);
        assert_eq!(decide(&with(|o| o.session_locked = true), false), State::Locked);
        assert_eq!(decide(&with(|o| o.desktop = Desktop::Secure), false), State::SecureDesktop);
        assert_eq!(decide(&with(|o| { o.desktop = Desktop::Secure; o.session_locked = true }), false), State::Locked, "locked says more than secure desktop");
        assert_eq!(decide(&with(|o| { o.target = TargetSeen::Closed; o.session_locked = true }), true), State::Closed, "a closed window ends the stream whatever else");
        assert_eq!(decide(&with(|o| o.target = TargetSeen::Minimized), true), State::Minimized);
        assert_eq!(decide(&with(|o| o.target = TargetSeen::Minimized), false), State::Live, "a monitor is never minimized");
        assert_eq!(decide(&with(|o| o.foreground_elevated = true), false), State::Elevated);
        assert_eq!(decide(&with(|o| o.foreground_elevated = true), true), State::Live, "a window target cares about itself, not what is in front");
        assert_eq!(decide(&with(|o| o.target_elevated = true), true), State::Elevated);
        assert_eq!(decide(&with(|o| { o.target_elevated = true; o.desktop = Desktop::Secure }), true), State::SecureDesktop);
    }

    #[test]
    fn input_goes_only_where_it_can_land() {
        let all = InputPolicy { mouse: true, keys: true };
        let none = InputPolicy { mouse: false, keys: false };
        assert_eq!(input_policy(State::Live, true), all);
        for s in [State::Locked, State::SecureDesktop, State::Closed] {
            assert_eq!(input_policy(s, true), none, "{s:?}");
            assert_eq!(input_policy(s, false), none, "{s:?}");
        }
        assert_eq!(input_policy(State::Elevated, false), InputPolicy { mouse: true, keys: false }, "a click can still get away from it");
        assert_eq!(input_policy(State::Elevated, true), none);
        assert_eq!(input_policy(State::Minimized, true), InputPolicy { mouse: true, keys: false });
        assert!(State::Live.picture_moves() && !State::Locked.picture_moves() && !State::Minimized.picture_moves());
        assert!(State::Closed.ends() && !State::Locked.ends());
    }

    #[test]
    fn a_flicker_is_not_news_but_a_lock_is_told_at_once() {
        let mut t = Tracker::default();
        let elevated = with(|o| o.foreground_elevated = true);
        assert_eq!(t.look(&elevated, false), None, "one look is not enough");
        assert_eq!(t.look(&Observed::normal(), false), None, "and it went away");
        assert_eq!(t.look(&elevated, false), None);
        assert_eq!(t.look(&elevated, false), Some(State::Elevated), "two looks in a row");
        assert_eq!(t.look(&elevated, false), None, "told once");
        assert_eq!(t.look(&with(|o| o.session_locked = true), false), Some(State::Locked), "at once");
        assert_eq!(t.look(&Observed::normal(), false), None);
        assert_eq!(t.look(&Observed::normal(), false), Some(State::Live), "back, after two looks");
        assert_eq!(t.state(), State::Live);
    }

    #[test]
    fn covered_is_set_by_a_dropped_click_and_cleared_by_looking() {
        let mut t = Tracker::default();
        assert_eq!(t.covered(), Some(State::Covered));
        assert_eq!(t.covered(), None, "told once");
        assert_eq!(t.look(&Observed::normal(), true), None);
        assert_eq!(t.look(&Observed::normal(), true), Some(State::Live));
        let mut locked = Tracker::default();
        locked.look(&with(|o| o.session_locked = true), true);
        assert_eq!(locked.covered(), None, "covered says nothing over a lock");
    }

    #[test]
    fn states_have_the_names_the_phone_knows() {
        assert_eq!(serde_json::to_value(State::SecureDesktop).unwrap(), "secure_desktop");
        for s in [State::Live, State::Locked, State::SecureDesktop, State::Closed, State::Minimized, State::Elevated, State::Covered] {
            assert_eq!(serde_json::to_value(s).unwrap(), s.name());
        }
    }
}
