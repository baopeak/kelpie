//! The phone's input as Core Graphics events posted at the HID tap: the mouse at global points, the wheel, keys with their modifiers, and
//! text as Unicode strings.
//!
//! For a window target, nothing goes anywhere but that window, with the Windows side's semantics (`input::keys_reach`, `input::press_reach`):
//! - a press that would land on another window over it (a second window of the same app included: the phone does not see it) is dropped and
//!   reported (`Covered`); the target app's own menus and pop-ups (windows above the normal layer) take it;
//! - keys and text go only while the target's app has the keyboard and its focused window is the target, or a dialog of its own. If not, the
//!   target is raised once (un-minimized, its app frontmost, `AXRaise`); if it still is not, they are refused (`NotFront`). The focus is looked
//!   at again before every character.
//!
//! Posting events needs the Accessibility permission: without it macOS drops them without a word, so input is refused (`Blocked`) instead.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use super::ffi::*;
use super::targets_mac;
use crate::geom::{self, Rect};
use crate::input::{drag_step, keys_reach, mac_key_events, mac_text, press_reach, DragStep, MacTyped, Reach, WindowFacts};
use crate::states::InputPolicy;

/// Where input goes now.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    /// The window target's number and process.
    pub window: Option<(u32, i32)>,
    /// The display's place (a display target), global points; a window's is read at each event.
    pub rect: Rect,
    /// The source picture's size (pixels).
    pub src: (u32, u32),
    /// Pixels a point.
    pub scale: f64,
    /// Where the window's content is in the source picture (pixels: x, y, width, height), when the capture says.
    pub content: Option<[f64; 4]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// macOS will not take posted input (no Accessibility permission).
    Blocked,
    Covered,
    NotFront,
    ForeignDialog,
    Refused,
    /// The stream is stopping: nothing more was done.
    Stopped,
}

/// A button a `down` holds, and the last point the pointer was at on the shared window while it did.
#[derive(Clone, Copy, Debug)]
pub struct Held {
    pub button: &'static str,
    pub last: CGPoint,
}

const MOVED: u32 = 5;
/// `(down, up, dragged, CGMouseButton)` of each button.
fn button(b: &str) -> (u32, u32, u32, u32) {
    match b {
        "right" => (3, 4, 7, 1),
        "middle" => (25, 26, 27, 2),
        _ => (1, 2, 6, 0),
    }
}

/// Events from a private source, so that the keys held on the Mac's own keyboard do not mix into them.
struct Source(*mut std::ffi::c_void);

impl Source {
    fn new() -> Source {
        // kCGEventSourceStatePrivate.
        Source(unsafe { CGEventSourceCreate(-1) })
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0 as CFTypeRef) }
        }
    }
}

fn post(ev: CGEventRef) -> bool {
    if ev.is_null() {
        return false;
    }
    unsafe {
        CGEventPost(kCGHIDEventTap, ev);
        CFRelease(ev as CFTypeRef);
    }
    true
}

fn mouse(s: &Source, kind: u32, at: CGPoint, button: u32, clicks: i64) -> bool {
    unsafe {
        let ev = CGEventCreateMouseEvent(s.0, kind, at, button);
        if !ev.is_null() && clicks > 0 {
            CGEventSetIntegerValueField(ev, kCGMouseEventClickState, clicks);
        }
        post(ev)
    }
}

fn key(s: &Source, code: u16, down: bool, flags: u64) -> bool {
    unsafe {
        let ev = CGEventCreateKeyboardEvent(s.0, code, down);
        if !ev.is_null() {
            CGEventSetFlags(ev, flags);
        }
        post(ev)
    }
}

fn unicode(s: &Source, units: &[u16]) -> bool {
    [true, false].into_iter().all(|down| unsafe {
        let ev = CGEventCreateKeyboardEvent(s.0, 0, down);
        if !ev.is_null() {
            CGEventSetFlags(ev, 0);
            CGEventKeyboardSetUnicodeString(ev, units.len(), units.as_ptr());
        }
        post(ev)
    })
}

fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// What a window is to the target.
fn facts(target: (u32, i32), w: &targets_mac::CgWindow) -> WindowFacts {
    let same = w.pid == target.1;
    WindowFacts { is_target: w.number == target.0, owned_by_target: false, same_process: same, same_thread: same, menu: same && w.layer > 0 }
}

/// Where keys go now, for the target: its app must have the keyboard, and its focused window must be the target or a dialog of its own.
fn keys_now(target: (u32, i32)) -> Reach {
    if targets_mac::focused_pid() != Some(target.1) {
        return Reach::Other;
    }
    match targets_mac::focused_window(target.1) {
        Some((n, _)) if n == target.0 => Reach::Yes,
        Some((_, dialog)) => keys_reach(WindowFacts { owned_by_target: dialog, same_process: true, same_thread: true, ..Default::default() }),
        None => Reach::Other,
    }
}

fn on_screen(target: (u32, i32)) -> bool {
    targets_mac::window(target.0).is_some_and(|w| w.onscreen)
}

const FRONT_WAIT_STEPS: u32 = 15;

/// The target with the keys, raised once if it has not them. Not waited for once the stream is stopping.
fn front(target: (u32, i32), stopping: &AtomicBool) -> Reach {
    if keys_now(target) == Reach::Yes && on_screen(target) {
        return Reach::Yes;
    }
    targets_mac::raise(target.1, target.0);
    for _ in 0..FRONT_WAIT_STEPS {
        if stopping.load(Ordering::SeqCst) {
            return Reach::Other;
        }
        if keys_now(target) == Reach::Yes && on_screen(target) {
            return Reach::Yes;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Reach::Other
}

fn refusal(r: Reach) -> Outcome {
    if r == Reach::ForeignDialog {
        Outcome::ForeignDialog
    } else {
        Outcome::NotFront
    }
}

/// Where a press at `at` lands. A window of another app above the normal layer (an overlay, which often lets clicks through) covers the target
/// only if Accessibility's hit test, which knows what really takes the click, does not find the target's app there; when it does, the target's
/// own windows at that point decide, as for any press.
fn lands_on(target: (u32, i32), at: CGPoint) -> Reach {
    let Some(w) = targets_mac::window_at(at.x, at.y, None) else { return Reach::Other };
    let reach = press_reach(facts(target, &w));
    if reach != Reach::Other || w.layer <= 0 || w.pid == target.1 || targets_mac::pid_at(at.x, at.y) != Some(target.1) {
        return reach;
    }
    match targets_mac::window_at(at.x, at.y, Some(target.1)) {
        Some(own) if own.layer >= 0 => press_reach(facts(target, &own)),
        _ => Reach::Other,
    }
}

/// A point of the source picture (pixels) as global points: through the window's content when the capture says where it is in the picture.
fn to_points(sx: f64, sy: f64, g: &Geometry, rect: Rect) -> CGPoint {
    let (x, y) = match g.content.filter(|_| g.window.is_some()) {
        Some([cx, cy, cw, ch]) => geom::source_to_points(sx - cx, sy - cy, cw.round().max(1.0) as u32, ch.round().max(1.0) as u32, rect, g.scale),
        None => geom::source_to_points(sx, sy, g.src.0, g.src.1, rect, g.scale),
    };
    CGPoint { x, y }
}

/// Does the checked ops (`input::checked_ops`). `pressed` keeps the button held by a `down` not yet released, so that the stream's end can let go.
/// Each op first looks at `stopping`: once the stream stops, nothing more is posted.
pub fn run(ops: &[Value], g: &Geometry, policy: InputPolicy, pressed: &mut Option<Held>, stopping: &AtomicBool) -> Outcome {
    if !policy.mouse && !policy.keys {
        if ops.iter().any(|op| op["k"] == "release" || op["k"] == "up") {
            release(pressed);
        }
        return Outcome::Refused;
    }
    if !trusted() {
        return Outcome::Blocked;
    }
    let s = Source::new();
    let mut outcome = Outcome::Done;
    for op in ops {
        if stopping.load(Ordering::SeqCst) {
            return Outcome::Stopped;
        }
        let kind = op["k"].as_str().unwrap_or_default();
        if kind == "release" {
            release(pressed);
            continue;
        }
        if kind == "key" {
            if !policy.keys {
                outcome = Outcome::Refused;
                continue;
            }
            if let Some(t) = g.window {
                let r = front(t, stopping);
                if stopping.load(Ordering::SeqCst) {
                    return Outcome::Stopped;
                }
                if r != Reach::Yes {
                    return refusal(r);
                }
            }
            let mods: Vec<&str> = op["mods"].as_array().map(|m| m.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            let events = mac_key_events(op["key"].as_str().unwrap_or_default(), &mods);
            if let Some(t) = g.window {
                let r = keys_now(t);
                if r != Reach::Yes {
                    return refusal(r);
                }
            }
            if !events.iter().all(|e| key(&s, e.code, e.down, e.flags)) {
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
            Some(t) => {
                // A press makes sure the target is in front (a minimized one is restored), every time: the focus may have moved since.
                if pressing && !(keys_now(t) == Reach::Yes && on_screen(t)) {
                    targets_mac::raise(t.1, t.0);
                    std::thread::sleep(std::time::Duration::from_millis(60));
                    if stopping.load(Ordering::SeqCst) {
                        return Outcome::Stopped;
                    }
                }
                match targets_mac::window(t.0) {
                    Some(w) if w.pid == t.1 => w.bounds.rect(),
                    _ => return Outcome::Refused,
                }
            }
            None => g.rect,
        };
        let at = to_points(op["x"].as_f64().unwrap_or(0.0), op["y"].as_f64().unwrap_or(0.0), g, rect);
        if let Some(t) = g.window {
            if matches!(kind, "move" | "up") && pressed.is_some() {
                match drag_step(kind, true, lands_on(t, at)) {
                    DragStep::Go => {}
                    DragStep::Hold => {
                        outcome = Outcome::Covered;
                        continue;
                    }
                    DragStep::ReleaseAtLastSafe => {
                        if let Some(held) = pressed.take() {
                            let (_, up, dragged, b) = button(held.button);
                            if !(mouse(&s, dragged, held.last, b, 0) && mouse(&s, up, held.last, b, 1)) {
                                return Outcome::Blocked;
                            }
                        }
                        outcome = Outcome::Covered;
                        continue;
                    }
                }
            }
            if matches!(kind, "down" | "click" | "wheel") {
                match lands_on(t, at) {
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
        let b = match op["b"].as_str() {
            Some("right") => "right",
            Some("middle") => "middle",
            _ => "left",
        };
        let (down, up, dragged, cg_button) = button(b);
        let ok = match kind {
            "down" => {
                *pressed = Some(Held { button: b, last: at });
                mouse(&s, MOVED, at, 0, 0) && mouse(&s, down, at, cg_button, 1)
            }
            "move" => match pressed.as_mut() {
                Some(held) => {
                    held.last = at;
                    let (_, _, dragged, hb) = button(held.button);
                    mouse(&s, dragged, at, hb, 0)
                }
                None => mouse(&s, MOVED, at, 0, 0),
            },
            "up" => {
                *pressed = None;
                mouse(&s, dragged, at, cg_button, 0) && mouse(&s, up, at, cg_button, 1)
            }
            "click" => {
                let n = op["n"].as_u64().unwrap_or(1).clamp(1, 2) as i64;
                mouse(&s, MOVED, at, 0, 0) && (1..=n).all(|i| mouse(&s, down, at, cg_button, i) && mouse(&s, up, at, cg_button, i))
            }
            "wheel" => {
                // The phone's dy is how far the page should move down, in Windows' notches (120 = three lines): about 40 points a notch, the
                // wheel turned towards the person, which a Mac counts as negative.
                let (dx, dy) = (op["dx"].as_f64().unwrap_or(0.0), op["dy"].as_f64().unwrap_or(0.0));
                mouse(&s, MOVED, at, 0, 0)
                    && unsafe {
                        let ev = CGEventCreateScrollWheelEvent2(s.0, kCGScrollEventUnitPixel, 2, (-dy / 3.0).round() as i32, (-dx / 3.0).round() as i32, 0);
                        if !ev.is_null() {
                            CGEventSetLocation(ev, at);
                        }
                        post(ev)
                    }
            }
            _ => true,
        };
        if !ok {
            return Outcome::Blocked;
        }
    }
    outcome
}

/// Lets go of a button a `down` left held where it was (the stream ended, or the release was dropped as late).
pub fn release(pressed: &mut Option<Held>) {
    if let Some(held) = pressed.take() {
        let (_, up, _, b) = button(held.button);
        mouse(&Source::new(), up, held.last, b, 1);
    }
}

/// Types text into the target, which must have the keys (raised once if it has not): `NotFront` otherwise, and nothing typed. With the
/// characters typed so far: the focus is looked at again before each one, and typing stops the moment it moved, or the stream is stopping
/// (`Stopped`).
pub fn type_text(text: &str, g: &Geometry, policy: InputPolicy, stopping: &AtomicBool) -> (Outcome, usize) {
    if !policy.keys {
        return (Outcome::Refused, 0);
    }
    if !trusted() {
        return (Outcome::Blocked, 0);
    }
    if let Some(t) = g.window {
        let r = front(t, stopping);
        if stopping.load(Ordering::SeqCst) {
            return (Outcome::Stopped, 0);
        }
        if r != Reach::Yes {
            return (refusal(r), 0);
        }
    }
    let s = Source::new();
    let typed = mac_text(text);
    for (n, unit) in typed.iter().enumerate() {
        if stopping.load(Ordering::SeqCst) {
            return (Outcome::Stopped, n);
        }
        if let Some(t) = g.window {
            let r = keys_now(t);
            if r != Reach::Yes {
                return (refusal(r), n);
            }
        }
        let ok = match *unit {
            MacTyped::Key(code) => key(&s, code, true, 0) && key(&s, code, false, 0),
            MacTyped::Char(units, len) => unicode(&s, &units[..len]),
        };
        if !ok {
            return (Outcome::Blocked, n);
        }
    }
    (Outcome::Done, typed.len())
}
