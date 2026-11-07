//! The phone's input as `SendInput`: absolute mouse positions over the virtual desktop, buttons, the wheel, keys and Unicode text.
//!
//! For a window target, nothing goes anywhere but that window (`input::keys_reach`, `input::press_reach` decide from what this reads):
//! - a press that would land on another window over it, a second window of the same program included (the phone does not see it), is dropped
//!   and reported (`Covered`); the target's own menus and dropdowns take it;
//! - keys and text go only while the target, or a dialog **of its own process** that it owns, is the foreground window and holds the keys. If
//!   not, it is brought to the front once; if it still is not, they are refused (`NotFront`) and the phone is told, so a password never goes into
//!   whatever window took the focus meanwhile;
//! - a dialog of **another** process that the target owns (a credential prompt, another program's file picker: Windows lets any program name
//!   a window as owner) takes neither keys nor clicks (`ForeignDialog`): the phone is told to share the whole monitor to use it.
//!
//! `SendInput` does not reduce its count when UIPI drops input into an elevated window (Microsoft documents this), so `Blocked` is rare: the
//! `elevated` state (`states_win`) is the real signal.

use serde_json::Value;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetAncestor, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, WindowFromPoint, GA_ROOT, GA_ROOTOWNER, GUITHREADINFO, SW_RESTORE,
};

use super::targets_win::{minimized, window_rect};
use crate::geom::{self, Rect};
use crate::input::{drag_step, key_events, keys_reach, press_reach, text_events, DragStep, KeyEvent, Reach, WindowFacts};
use crate::states::InputPolicy;

/// Where input goes now.
#[derive(Clone, Copy)]
pub struct Geometry {
    pub window: Option<HWND>,
    /// The monitor's rectangle (a monitor target); a window's is read at each event.
    pub rect: Rect,
    pub src: (u32, u32),
    pub desktop: Rect,
}

unsafe impl Send for Geometry {}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Windows took fewer events than sent: UIPI (an elevated window) or the secure desktop.
    Blocked,
    /// A click on a window target that another window covers there.
    Covered,
    /// Keys or text for a window target that is not in front, and could not be brought there.
    NotFront,
    /// Another process's dialog, owned by the target, is in front of it or under the press.
    ForeignDialog,
    /// The state allows none of it (locked, secure desktop, closed).
    Refused,
}

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn key(e: KeyEvent) -> INPUT {
    let (vk, scan, mut flags) = match e {
        KeyEvent::Vk { vk, extended, .. } => (vk, 0, if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) }),
        KeyEvent::Unicode { unit, .. } => (0, unit, KEYEVENTF_UNICODE),
    };
    if matches!(e, KeyEvent::Vk { up: true, .. } | KeyEvent::Unicode { up: true, .. }) {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn send(inputs: &[INPUT]) -> bool {
    if inputs.is_empty() {
        return true;
    }
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

/// Brings a window target to the front: restores it if minimized, and works around the foreground lock by attaching to the thread in front
/// for the moment it takes.
fn bring_front(h: HWND) {
    unsafe {
        if minimized(h) {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let fg = GetForegroundWindow();
        if fg == h {
            return;
        }
        if SetForegroundWindow(h).as_bool() {
            return;
        }
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        if fg_thread != 0 && AttachThreadInput(me, fg_thread, true).as_bool() {
            let _ = BringWindowToTop(h);
            let _ = SetForegroundWindow(h);
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

/// What `w` (any window: a child is taken at its top-level window) is to the target `h`.
fn facts(h: HWND, w: HWND) -> WindowFacts {
    if w.is_invalid() {
        return WindowFacts::default();
    }
    unsafe {
        let root = GetAncestor(w, GA_ROOT);
        let root = if root.is_invalid() { w } else { root };
        let (mut pid_w, mut pid_h) = (0u32, 0u32);
        let thread_w = GetWindowThreadProcessId(root, Some(&mut pid_w));
        let thread_h = GetWindowThreadProcessId(h, Some(&mut pid_h));
        let mut class = [0u16; 32];
        let n = GetClassNameW(root, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n.min(class.len())]);
        WindowFacts {
            is_target: root == h,
            owned_by_target: root != h && GetAncestor(root, GA_ROOTOWNER) == h,
            same_process: pid_w != 0 && pid_w == pid_h,
            same_thread: thread_w != 0 && thread_w == thread_h,
            menu: class == "#32768" || class == "ComboLBox",
        }
    }
}

/// Where keys go now, for the target: the window in front, and the one its thread gives the keys to, which moves only once that thread has
/// taken the activation (two windows of one program share a thread). The worse of the two.
fn keys_now(h: HWND) -> Reach {
    let fg = unsafe { GetForegroundWindow() };
    let in_front = keys_reach(facts(h, fg));
    if in_front != Reach::Yes {
        return in_front;
    }
    let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let thread = unsafe { GetWindowThreadProcessId(fg, None) };
    if thread == 0 || unsafe { GetGUIThreadInfo(thread, &mut info) }.is_err() {
        return Reach::Other;
    }
    if info.hwndFocus.is_invalid() {
        return Reach::Yes;
    }
    keys_reach(facts(h, info.hwndFocus))
}

/// Whether the target (or a dialog of its own) has the keyboard focus.
fn in_front(h: HWND) -> bool {
    keys_now(h) == Reach::Yes
}

/// The target in front, brought there once if it is not (and given a moment to take the keys). A dialog of another process in front stays
/// there (the target is under it): said at once.
fn front(h: HWND) -> Reach {
    match keys_now(h) {
        Reach::Yes if !minimized(h) => return Reach::Yes,
        Reach::ForeignDialog => return Reach::ForeignDialog,
        _ => {}
    }
    bring_front(h);
    for _ in 0..FRONT_WAIT_STEPS {
        match keys_now(h) {
            Reach::Yes if !minimized(h) => return Reach::Yes,
            Reach::ForeignDialog => return Reach::ForeignDialog,
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Reach::Other
}

/// How long `front` waits for the target to take the keys after bringing it there: 15 × 20 ms.
const FRONT_WAIT_STEPS: u32 = 15;

/// What a refusal to type says: a dialog of another process, or the target not in front.
fn refusal(r: Reach) -> Outcome {
    if r == Reach::ForeignDialog {
        Outcome::ForeignDialog
    } else {
        Outcome::NotFront
    }
}

/// Whether a press at the screen point lands on the target (or its own dialog, menu or dropdown), not on another window over it.
fn lands_on(h: HWND, x: i32, y: i32) -> Reach {
    press_reach(facts(h, unsafe { WindowFromPoint(POINT { x, y }) }))
}

fn at(op: &Value) -> (f64, f64) {
    (op["x"].as_f64().unwrap_or(0.0), op["y"].as_f64().unwrap_or(0.0))
}

fn buttons(b: &str) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    match b {
        "right" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
    }
}

/// Does the checked ops (`input::checked_ops`). `pressed` keeps the button held by a `down` not yet released, so that the stream's end can let go.
pub fn run(ops: &[Value], g: &Geometry, policy: InputPolicy, pressed: &mut Option<Held>) -> Outcome {
    if !policy.mouse && !policy.keys {
        // Nothing allowed now: a held button is still let go.
        if ops.iter().any(|op| op["k"] == "release" || op["k"] == "up") {
            release(pressed);
        }
        return Outcome::Refused;
    }
    let mut outcome = Outcome::Done;
    for op in ops {
        let kind = op["k"].as_str().unwrap_or_default();
        // Letting go of a held button, where the pointer is: always allowed, never a move.
        if kind == "release" {
            release(pressed);
            continue;
        }
        if kind == "key" {
            if !policy.keys {
                outcome = Outcome::Refused;
                continue;
            }
            if let Some(h) = g.window {
                let r = front(h);
                if r != Reach::Yes {
                    return refusal(r);
                }
            }
            let mods: Vec<&str> = op["mods"].as_array().map(|m| m.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let events: Vec<INPUT> = key_events(op["key"].as_str().unwrap_or_default(), &mods).into_iter().map(key).collect();
            // Looked at again right before the call: the focus may have moved while the events were made.
            if let Some(h) = g.window {
                let r = keys_now(h);
                if r != Reach::Yes {
                    return refusal(r);
                }
            }
            if !send(&events) {
                return Outcome::Blocked;
            }
            continue;
        }
        if !policy.mouse {
            outcome = Outcome::Refused;
            continue;
        }
        let pressing = matches!(kind, "down" | "click");
        let rect = match g.window {
            Some(h) => {
                // A press makes sure the target is in front (a minimized one is restored), every time: the focus may have moved since.
                if pressing && !in_front(h) {
                    bring_front(h);
                }
                match window_rect(h) {
                    Some(r) => r,
                    None => return Outcome::Refused,
                }
            }
            None => g.rect,
        };
        let (sx, sy) = at(op);
        let (x, y) = geom::source_to_screen(sx, sy, g.src.0, g.src.1, rect);
        let abs = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        if let Some(h) = g.window {
            // A drag goes only over what the phone sees: a move onto another window holds the pointer at the last point on the shared one, and
            // an `up` there lets go at that point, so nothing is dropped into a window the phone cannot see.
            if matches!(kind, "move" | "up") && pressed.is_some() {
                match drag_step(kind, true, lands_on(h, x, y)) {
                    DragStep::Go => {}
                    DragStep::Hold => {
                        outcome = Outcome::Covered;
                        continue;
                    }
                    DragStep::ReleaseAtLastSafe => {
                        if let Some(held) = pressed.take() {
                            let (_, up) = buttons(held.button);
                            if !send(&[mouse(held.last.0, held.last.1, 0, abs), mouse(0, 0, 0, up)]) {
                                return Outcome::Blocked;
                            }
                        }
                        outcome = Outcome::Covered;
                        continue;
                    }
                }
            }
            if matches!(kind, "down" | "click" | "wheel") {
                match lands_on(h, x, y) {
                    Reach::Yes => {}
                    Reach::ForeignDialog => {
                        outcome = Outcome::ForeignDialog;
                        continue;
                    }
                    Reach::Other => {
                        outcome = Outcome::Covered;
                        continue;
                    }
                }
            }
        }
        let (nx, ny) = geom::to_absolute(x, y, g.desktop);
        let mut events = vec![mouse(nx, ny, 0, abs)];
        let b = match op["b"].as_str() {
            Some("right") => "right",
            Some("middle") => "middle",
            _ => "left",
        };
        let (down, up) = buttons(b);
        match kind {
            "down" => {
                events.push(mouse(0, 0, 0, down));
                *pressed = Some(Held { button: b, last: (nx, ny) });
            }
            "move" => {
                if let Some(held) = pressed.as_mut() {
                    held.last = (nx, ny);
                }
            }
            "up" => {
                events.push(mouse(0, 0, 0, up));
                *pressed = None;
            }
            "click" => {
                for _ in 0..op["n"].as_u64().unwrap_or(1).clamp(1, 2) {
                    events.push(mouse(0, 0, 0, down));
                    events.push(mouse(0, 0, 0, up));
                }
            }
            "wheel" => {
                // The phone's dy is how far the page should move down: the wheel turned towards the person, which Windows counts as negative.
                let (dx, dy) = (op["dx"].as_f64().unwrap_or(0.0) as i32, op["dy"].as_f64().unwrap_or(0.0) as i32);
                if dy != 0 {
                    events.push(mouse(0, 0, -dy, MOUSEEVENTF_WHEEL));
                }
                if dx != 0 {
                    events.push(mouse(0, 0, dx, MOUSEEVENTF_HWHEEL));
                }
            }
            _ => {}
        }
        if !send(&events) {
            return Outcome::Blocked;
        }
    }
    outcome
}

/// A button a `down` holds, and the last point (`SendInput`'s absolute units) the pointer was at on the shared window while it did.
#[derive(Clone, Copy, Debug)]
pub struct Held {
    pub button: &'static str,
    pub last: (i32, i32),
}

/// Lets go of a button a `down` left held where the pointer is (the stream ended, or the release was dropped as late).
pub fn release(pressed: &mut Option<Held>) {
    if let Some(held) = pressed.take() {
        let (_, up) = buttons(held.button);
        send(&[mouse(0, 0, 0, up)]);
    }
}

/// The events of one typed unit (its key down and up): what goes in one `SendInput` call while typing.
const TEXT_BATCH: usize = 2;

/// Types text into the target, which must be the foreground window (brought there once if it is not): `NotFront` otherwise, and nothing typed.
/// With the units typed so far.
///
/// `SendInput` cannot name a window: keys go to whichever has the focus when Windows delivers them. So the foreground and its focus are looked
/// at again right before every call, one unit (a character, Enter, a Tab) at a time, and typing stops the moment they moved: what remains of
/// the race is the time between that look and the call, and a program that moves its own focus from one of its windows to another in it
/// (see `docs/architecture.md`).
pub fn type_text(text: &str, g: &Geometry, policy: InputPolicy) -> (Outcome, usize) {
    if !policy.keys {
        return (Outcome::Refused, 0);
    }
    if let Some(h) = g.window {
        let r = front(h);
        if r != Reach::Yes {
            return (refusal(r), 0);
        }
    }
    let events: Vec<INPUT> = text_events(text).into_iter().map(key).collect();
    for (typed, unit) in events.chunks(TEXT_BATCH).enumerate() {
        if let Some(h) = g.window {
            let r = keys_now(h);
            if r != Reach::Yes {
                return (refusal(r), typed);
            }
        }
        if !send(unit) {
            return (Outcome::Blocked, typed);
        }
    }
    (Outcome::Done, events.len() / TEXT_BATCH)
}