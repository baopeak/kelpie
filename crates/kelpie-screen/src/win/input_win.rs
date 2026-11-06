//! The phone's input as `SendInput`: absolute mouse positions over the virtual desktop, buttons, the wheel, keys and Unicode text. A window
//! target is brought to the front before its first click, and a click that would land on another window (one over it) is dropped and
//! reported, never sent to whatever is there. `SendInput` reports fewer events than given when UIPI dropped them: that is said too.

use serde_json::Value;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, WindowFromPoint, GA_ROOT, SW_RESTORE};

use super::targets_win::{minimized, window_rect};
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

/// Whether the screen point is on the window itself (or a child of it), not on one over it.
fn lands_on(h: HWND, x: i32, y: i32) -> bool {
    unsafe {
        let at = WindowFromPoint(POINT { x, y });
        !at.is_invalid() && GetAncestor(at, GA_ROOT) == h
    }
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

/// Does the checked ops (`input::checked_ops`). `fronted` remembers that the window was brought to the front for this stream.
pub fn run(ops: &[Value], g: &Geometry, policy: InputPolicy, fronted: &mut bool) -> Outcome {
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
        let rect = match g.window {
            Some(h) => {
                let pressing = matches!(kind, "down" | "click");
                if pressing && !*fronted {
                    bring_front(h);
                    *fronted = true;
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
        let b = op["b"].as_str().unwrap_or("left");
        let (down, up) = buttons(b);
        match kind {
            "down" => events.push(mouse(0, 0, 0, down)),
            "up" => events.push(mouse(0, 0, 0, up)),
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

/// Types text into whatever has the keyboard focus (the target window, brought to the front first).
pub fn type_text(text: &str, g: &Geometry, policy: InputPolicy, fronted: &mut bool) -> Outcome {
    if !policy.keys {
        return Outcome::Refused;
    }
    if let Some(h) = g.window {
        if !*fronted {
            bring_front(h);
            *fronted = true;
        }
    }
    let events: Vec<INPUT> = text_events(text).into_iter().map(key).collect();
    // In pieces: one call per few hundred events keeps a long text from stalling the input queue.
    for chunk in events.chunks(200) {
        if !send(chunk) {
            return Outcome::Blocked;
        }
    }
    Outcome::Done
}
