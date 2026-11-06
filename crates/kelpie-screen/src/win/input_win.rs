//! The phone's input as `SendInput`: absolute mouse positions over the virtual desktop, buttons, the wheel, keys and Unicode text.
//!
//! For a window target, nothing goes anywhere but that window:
//! - a press that would land on another program's window over it is dropped and reported (`Covered`); its own menus and dropdowns take it;
//! - keys and text go only while the target, or a dialog it owns, is the foreground window. If not (another program, or another window of the
//!   same one), it is brought to the front once; if it still is not, they are refused (`NotFront`) and the phone is told, so a password never goes
//!   into whatever window took the focus meanwhile.
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
    BringWindowToTop, GetAncestor, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, WindowFromPoint, GA_ROOT, GA_ROOTOWNER, GUITHREADINFO, SW_RESTORE,
};

use super::targets_win::{minimized, process_of, window_rect};
use crate::geom::{self, Rect};
use crate::input::{key_events, text_events, KeyEvent};
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

/// Whether a window belongs to the target: the target itself, or a menu, a dropdown or a dialog of its process (which are top-level windows
/// of their own).
fn of_target(h: HWND, w: HWND) -> bool {
    !w.is_invalid() && (w == h || process_of(w) == process_of(h))
}

/// Whether a window in front is the target or one it owns (its dialogs): another top-level window of the same program (a second document, a
/// chat window) is not, so what the phone types never goes into a window it does not see.
fn owned_by_target(h: HWND, w: HWND) -> bool {
    !w.is_invalid() && (w == h || unsafe { GetAncestor(w, GA_ROOTOWNER) } == h)
}

/// Whether the target (or a dialog of its own) has the keyboard focus: the window in front is, and so is the one its thread gives the keys to,
/// which moves only once that thread has taken the activation (two windows of one program share a thread).
fn in_front(h: HWND) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    if !owned_by_target(h, fg) {
        return false;
    }
    let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let thread = unsafe { GetWindowThreadProcessId(fg, None) };
    if thread == 0 || unsafe { GetGUIThreadInfo(thread, &mut info) }.is_err() {
        return false;
    }
    info.hwndFocus.is_invalid() || owned_by_target(h, unsafe { GetAncestor(info.hwndFocus, GA_ROOT) })
}

/// The target in front, brought there once if it is not (and given a moment to take the keys): false when it still is not.
fn front(h: HWND) -> bool {
    if in_front(h) && !minimized(h) {
        return true;
    }
    bring_front(h);
    for _ in 0..FRONT_WAIT_STEPS {
        if in_front(h) && !minimized(h) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// How long `front` waits for the target to take the keys after bringing it there: 15 × 20 ms.
const FRONT_WAIT_STEPS: u32 = 15;

/// Whether the screen point is on the target (or a window of its own), not on another window over it.
fn lands_on(h: HWND, x: i32, y: i32) -> bool {
    of_target(h, unsafe { WindowFromPoint(POINT { x, y }) })
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
pub fn run(ops: &[Value], g: &Geometry, policy: InputPolicy, pressed: &mut Option<&'static str>) -> Outcome {
    if !policy.mouse && !policy.keys {
        return Outcome::Refused;
    }
    let mut outcome = Outcome::Done;
    for op in ops {
        let kind = op["k"].as_str().unwrap_or_default();
        if kind == "key" {
            if !policy.keys {
                outcome = Outcome::Refused;
                continue;
            }
            if let Some(h) = g.window {
                if !front(h) {
                    return Outcome::NotFront;
                }
            }
            let mods: Vec<&str> = op["mods"].as_array().map(|m| m.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let events: Vec<INPUT> = key_events(op["key"].as_str().unwrap_or_default(), &mods).into_iter().map(key).collect();
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
        if let Some(h) = g.window {
            if matches!(kind, "down" | "click" | "wheel") && !lands_on(h, x, y) {
                outcome = Outcome::Covered;
                continue;
            }
        }
        let (nx, ny) = geom::to_absolute(x, y, g.desktop);
        let abs = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
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
                *pressed = Some(b);
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

/// Lets go of a button a `down` left held (the stream ended, or the release was dropped as late).
pub fn release(pressed: &mut Option<&'static str>) {
    if let Some(b) = pressed.take() {
        let (_, up) = buttons(b);
        send(&[mouse(0, 0, 0, up)]);
    }
}

/// Types text into the target, which must be the foreground window (brought there once if it is not): `NotFront` otherwise, and nothing typed.
pub fn type_text(text: &str, g: &Geometry, policy: InputPolicy) -> Outcome {
    if !policy.keys {
        return Outcome::Refused;
    }
    if let Some(h) = g.window {
        if !front(h) {
            return Outcome::NotFront;
        }
    }
    let events: Vec<INPUT> = text_events(text).into_iter().map(key).collect();
    // In pieces: one call per few hundred events keeps a long text from stalling the input queue. The focus is checked again between them.
    for chunk in events.chunks(200) {
        if let Some(h) = g.window {
            if !in_front(h) {
                return Outcome::NotFront;
            }
        }
        if !send(chunk) {
            return Outcome::Blocked;
        }
    }
    Outcome::Done
}