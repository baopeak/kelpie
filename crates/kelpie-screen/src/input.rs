//! The input a phone may send to a screen: a closed set of operations with numbers in range, checked by the daemon before anything goes on
//! (`checked_ops`) and turned by the helper into what `SendInput` takes (`key_events`, `text_events`). Coordinates are the source picture's
//! pixels. None of it is logged anywhere.
//!
//! Keys are a fixed table (what an IDE, a dialog or a sign-in needs, with Ctrl, Shift, Alt and the Windows key, alone for Start or held as
//! `meta` for Win+E, Win+L and the like: owner decision 2026-11-06). Ctrl+Alt+Del cannot be sent by any program.

use serde_json::{json, Value};

/// Operations in one message, and text in one.
pub const MAX_OPS: usize = 32;
pub const MAX_TEXT: usize = 4096;
/// The largest coordinate taken (an 8K picture fits).
pub const MAX_COORD: f64 = 16_384.0;
/// The helper's queue of input not yet done, at most.
pub const QUEUE: usize = 256;
/// Input the daemon stamped longer ago than this is dropped by the helper too (a queue that stalled).
pub const TTL_MS: u64 = 1000;

pub const BUTTONS: &[&str] = &["left", "right", "middle"];
pub const MODS: &[&str] = &["ctrl", "shift", "alt", "meta"];

/// `(name, virtual key, extended)`.
const KEYS: &[(&str, u16, bool)] = &[
    ("Enter", 0x0D, false),
    ("Tab", 0x09, false),
    ("Backspace", 0x08, false),
    ("Delete", 0x2E, true),
    ("Insert", 0x2D, true),
    ("Escape", 0x1B, false),
    ("Space", 0x20, false),
    ("ArrowUp", 0x26, true),
    ("ArrowDown", 0x28, true),
    ("ArrowLeft", 0x25, true),
    ("ArrowRight", 0x27, true),
    ("Home", 0x24, true),
    ("End", 0x23, true),
    ("PageUp", 0x21, true),
    ("PageDown", 0x22, true),
    ("ContextMenu", 0x5D, true),
    // The Windows key on its own: Start.
    ("Meta", 0x5B, true),
];

/// A named key: its virtual key and whether it is an extended one. Letters and digits (for shortcuts such as Ctrl+S) and F1–F12 too.
pub fn key_code(name: &str) -> Option<(u16, bool)> {
    if let Some((_, vk, ext)) = KEYS.iter().find(|(n, ..)| *n == name) {
        return Some((*vk, *ext));
    }
    match name.as_bytes() {
        [c] if c.is_ascii_uppercase() || c.is_ascii_digit() => Some((u16::from(*c), false)),
        [b'F', rest @ ..] if !rest.is_empty() && rest.len() <= 2 => {
            let n: u16 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            (1..=12).contains(&n).then_some((0x6F + n, false))
        }
        _ => None,
    }
}

/// A modifier's virtual key and whether it is an extended one (the Windows key is).
fn mod_code(m: &str) -> Option<(u16, bool)> {
    Some(match m {
        "ctrl" => (0x11, false),
        "shift" => (0x10, false),
        "alt" => (0x12, false),
        "meta" => (0x5B, true),
        _ => return None,
    })
}

fn number(v: &Value, key: &str, lo: f64, hi: f64) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= lo && *n <= hi)
}

/// The phone's input as the helper takes it, or None: a closed set of operations with numbers in range, refused whole if one is not.
/// `move {x,y}`, `down`/`up {x,y,b}`, `click {x,y,b,n}`, `wheel {x,y,dx,dy}` (dx/dy in notches of 120), `key {key, mods}`, and `release`, which lets go
/// of a button held by a `down` where the pointer is, without moving it (what the daemon sends for an `up` it drops as late).
pub fn checked_ops(ops: &Value) -> Option<Vec<Value>> {
    let ops = ops.as_array().filter(|o| !o.is_empty() && o.len() <= MAX_OPS)?;
    ops.iter()
        .map(|op| {
            let kind = op.get("k").and_then(Value::as_str)?;
            let at = || Some((number(op, "x", 0.0, MAX_COORD)?, number(op, "y", 0.0, MAX_COORD)?));
            let button = || op.get("b").and_then(Value::as_str).filter(|b| BUTTONS.contains(b)).or(op.get("b").is_none().then_some("left"));
            Some(match kind {
                "move" => {
                    let (x, y) = at()?;
                    json!({"k": "move", "x": x, "y": y})
                }
                "down" | "up" => {
                    let (x, y) = at()?;
                    json!({"k": kind, "x": x, "y": y, "b": button()?})
                }
                "click" => {
                    let (x, y) = at()?;
                    let n = op.get("n").map_or(Some(1), |n| n.as_u64().filter(|n| (1..=2).contains(n)))?;
                    json!({"k": "click", "x": x, "y": y, "b": button()?, "n": n})
                }
                "wheel" => {
                    let (x, y) = at()?;
                    json!({"k": "wheel", "x": x, "y": y, "dx": number(op, "dx", -12_000.0, 12_000.0)?.round(), "dy": number(op, "dy", -12_000.0, 12_000.0)?.round()})
                }
                "key" => {
                    let key = op.get("key").and_then(Value::as_str).filter(|k| key_code(k).is_some())?;
                    let mods: Vec<&str> = match op.get("mods") {
                        None => vec![],
                        Some(m) => m.as_array().filter(|a| a.len() <= MODS.len())?.iter().map(|m| m.as_str().filter(|m| MODS.contains(m))).collect::<Option<Vec<_>>>()?,
                    };
                    let mut mods: Vec<&str> = MODS.iter().copied().filter(|m| mods.contains(m)).collect();
                    mods.dedup();
                    json!({"k": "key", "key": key, "mods": mods})
                }
                "release" => json!({"k": "release"}),
                _ => return None,
            })
        })
        .collect()
}

/// What a window is to a shared window target, from what Windows says about it (`win::input_win` reads it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowFacts {
    /// Its top-level window is the target.
    pub is_target: bool,
    /// Its owner chain ends at the target (a dialog, an owned popup), and it is not the target itself.
    pub owned_by_target: bool,
    /// Same process as the target.
    pub same_process: bool,
    /// Same thread as the target.
    pub same_thread: bool,
    /// A popup window (`WS_POPUP`: a menu, a dropdown list).
    pub popup: bool,
}

/// Whether input may go to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Yes,
    /// A dialog another process made and gave the target as owner: not the shared window's, and the phone does not see it (owner decision
    /// 2026-11-07: typing goes only into the shared window and dialogs of its own process).
    ForeignDialog,
    /// Another window: not in front (keys), or over the target (a press).
    Other,
}

/// Where keys may go: the target, or a dialog of its own process that it owns. Another top-level window of the same program (a second
/// document window) is `Other`, and so is anything else; a dialog of another process that the target owns is `ForeignDialog`.
pub fn keys_reach(f: WindowFacts) -> Reach {
    if f.is_target || (f.owned_by_target && f.same_process) {
        Reach::Yes
    } else if f.owned_by_target {
        Reach::ForeignDialog
    } else {
        Reach::Other
    }
}

/// Where a press may land: where keys may go, and a popup of the target's own thread (its menus and dropdown lists, separate top-level
/// windows). A second window of the same program over the shared one, which the phone cannot see (the capture shows the target alone), is
/// `Other`: the press is dropped as covered.
pub fn press_reach(f: WindowFacts) -> Reach {
    match keys_reach(f) {
        Reach::Other if f.same_thread && f.popup => Reach::Yes,
        r => r,
    }
}
/// Whether a button is held after `ops`, given whether one was before: a `down` holds one, an `up`, a `click` or a `release` lets it go.
pub fn held_after(held: bool, ops: &[Value]) -> bool {
    ops.iter().fold(held, |held, op| match op["k"].as_str() {
        Some("down") => true,
        Some("up" | "click" | "release") => false,
        _ => held,
    })
}

/// Text to type, if it may be typed: not empty, not too long, no control characters but new lines and tabs.
pub fn checked_text(text: &str) -> Option<&str> {
    (!text.is_empty() && text.chars().count() <= MAX_TEXT && text.chars().all(|c| !c.is_control() || c == '\n' || c == '\t' || c == '\r')).then_some(text)
}

/// One key going down or up, as `SendInput` takes it: a virtual key, or a UTF-16 unit typed as Unicode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Vk { vk: u16, extended: bool, up: bool },
    Unicode { unit: u16, up: bool },
}

/// A key with modifiers: the modifiers down, the key down and up, the modifiers up in reverse.
pub fn key_events(key: &str, mods: &[&str]) -> Vec<KeyEvent> {
    let Some((vk, extended)) = key_code(key) else { return vec![] };
    let mods: Vec<(u16, bool)> = mods.iter().filter_map(|m| mod_code(m)).collect();
    let mut out: Vec<KeyEvent> = mods.iter().map(|&(m, ext)| KeyEvent::Vk { vk: m, extended: ext, up: false }).collect();
    out.push(KeyEvent::Vk { vk, extended, up: false });
    out.push(KeyEvent::Vk { vk, extended, up: true });
    out.extend(mods.iter().rev().map(|&(m, ext)| KeyEvent::Vk { vk: m, extended: ext, up: true }));
    out
}

/// Text as key events: each UTF-16 unit as Unicode (a character outside the BMP is its two surrogates), a new line as Enter (programs do
/// not take a Unicode carriage return as one), a tab as Tab.
pub fn text_events(text: &str) -> Vec<KeyEvent> {
    let mut out = Vec::with_capacity(text.len() * 2);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let vk = match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                Some(0x0D)
            }
            '\n' => Some(0x0D),
            '\t' => Some(0x09),
            _ => None,
        };
        if let Some(vk) = vk {
            out.push(KeyEvent::Vk { vk, extended: false, up: false });
            out.push(KeyEvent::Vk { vk, extended: false, up: true });
            continue;
        }
        let mut buf = [0u16; 2];
        for &unit in c.encode_utf16(&mut buf).iter() {
            out.push(KeyEvent::Unicode { unit, up: false });
            out.push(KeyEvent::Unicode { unit, up: true });
        }
    }
    out
}

/// Whether input the daemon stamped at `t` (its clock, which is this machine's) is still worth doing at `now`.
pub fn fresh(t: Option<u64>, now: u64) -> bool {
    t.is_some_and(|t| t <= now + TTL_MS && now.saturating_sub(t) <= TTL_MS)
}

/// The stop hotkey: `RegisterHotKey`'s modifiers and virtual key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u16,
}

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Shift+K";

/// `Ctrl+Alt+Shift+K` and the like. At least two modifiers, one of them Ctrl or Alt, and a letter, a digit or F1–F12: a hotkey that a
/// person types by accident, or that takes a key from every program, is refused.
pub fn parse_hotkey(s: &str) -> Option<Hotkey> {
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let (key, mods) = parts.split_last()?;
    let mut bits = 0;
    for m in mods {
        let bit = match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" | "meta" => MOD_WIN,
            _ => return None,
        };
        if bits & bit != 0 {
            return None;
        }
        bits |= bit;
    }
    if bits.count_ones() < 2 || bits & (MOD_CONTROL | MOD_ALT) == 0 {
        return None;
    }
    let key = key.to_ascii_uppercase();
    let (vk, _) = key_code(&key).filter(|(vk, _)| (0x30..=0x39).contains(vk) || (0x41..=0x5A).contains(vk) || (0x70..=0x7B).contains(vk))?;
    Some(Hotkey { mods: bits, vk })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_go_only_to_the_target_and_its_own_dialogs_and_presses_also_to_its_own_menus() {
        let target = WindowFacts { is_target: true, same_process: true, same_thread: true, ..Default::default() };
        let own_dialog = WindowFacts { owned_by_target: true, same_process: true, ..Default::default() };
        let foreign_dialog = WindowFacts { owned_by_target: true, same_process: false, ..Default::default() };
        let second_window = WindowFacts { same_process: true, same_thread: true, ..Default::default() };
        let own_menu = WindowFacts { same_process: true, same_thread: true, popup: true, ..Default::default() };
        let other_program = WindowFacts::default();
        assert_eq!(keys_reach(target), Reach::Yes);
        assert_eq!(keys_reach(own_dialog), Reach::Yes, "a Save As of the shared program");
        assert_eq!(keys_reach(foreign_dialog), Reach::ForeignDialog, "another process's window the target owns: a credential prompt, or anything that named it as owner");
        assert_eq!(keys_reach(second_window), Reach::Other, "another top-level window of the same program");
        assert_eq!(keys_reach(own_menu), Reach::Other, "keys go to the window that has the focus, never to a menu as such");
        assert_eq!(keys_reach(other_program), Reach::Other);
        assert_eq!(press_reach(own_menu), Reach::Yes, "its own menus and dropdowns take clicks");
        assert_eq!(press_reach(second_window), Reach::Other, "a window the phone does not see takes no click");
        assert_eq!(press_reach(foreign_dialog), Reach::ForeignDialog);
        assert_eq!(press_reach(WindowFacts { popup: true, ..Default::default() }), Reach::Other, "another program's popup");
    }
    #[test]
    fn a_release_has_no_place_and_a_held_button_is_followed_through_the_ops() {
        assert_eq!(checked_ops(&json!([{"k": "release", "x": 5, "y": 6}])).unwrap(), [json!({"k": "release"})], "no coordinates go with it");
        let ops = |v: Value| checked_ops(&v).unwrap();
        assert!(held_after(false, &ops(json!([{"k": "down", "x": 1, "y": 1}, {"k": "move", "x": 2, "y": 2}]))));
        assert!(!held_after(true, &ops(json!([{"k": "up", "x": 2, "y": 2}]))));
        assert!(!held_after(false, &ops(json!([{"k": "click", "x": 1, "y": 1}]))), "a click lets go of what it pressed");
        assert!(!held_after(true, &ops(json!([{"k": "release"}]))));
        assert!(held_after(true, &ops(json!([{"k": "move", "x": 3, "y": 3}]))));
    }
    #[test]
    fn only_input_a_screen_takes_goes_and_a_bad_op_refuses_the_message() {
        let ok = json!([
            {"k": "move", "x": 10.5, "y": 20},
            {"k": "down", "x": 1, "y": 2},
            {"k": "up", "x": 1, "y": 2, "b": "left"},
            {"k": "click", "x": 3, "y": 4, "b": "right"},
            {"k": "click", "x": 3, "y": 4, "n": 2},
            {"k": "wheel", "x": 5, "y": 6, "dx": 0, "dy": -240.4},
            {"k": "key", "key": "S", "mods": ["shift", "ctrl", "ctrl"]},
            {"k": "key", "key": "F5"},
        ]);
        let got = checked_ops(&ok).unwrap();
        assert_eq!(got[1]["b"], "left", "a button defaults to left");
        assert_eq!(got[4]["n"], 2);
        assert_eq!(got[5]["dy"], -240.0);
        assert_eq!(got[6]["mods"], json!(["ctrl", "shift"]), "in a fixed order, once each");
        assert_eq!(got[7]["mods"], json!([]));
        for bad in [
            json!([]),
            json!({"k": "move", "x": 1, "y": 1}),
            json!([{"k": "eval", "x": 1, "y": 1}]),
            json!([{"k": "move", "x": -1, "y": 1}]),
            json!([{"k": "move", "x": 1e9, "y": 1}]),
            json!([{"k": "move", "x": "1", "y": 1}]),
            json!([{"k": "click", "x": 1, "y": 1, "b": "x1"}]),
            json!([{"k": "click", "x": 1, "y": 1, "n": 3}]),
            json!([{"k": "key", "key": "LWin"}]),
            json!([{"k": "key", "key": "a"}]),
            json!([{"k": "key", "key": "F13"}]),
            json!([{"k": "key", "key": "Delete", "mods": ["win"]}]),
            json!([{"k": "key", "key": "Delete", "mods": ["ctrl", "shift", "alt", "meta", "ctrl"]}]),
            json!([{"k": "key", "key": "Delete", "mods": "ctrl"}]),
            json!([{"k": "wheel", "x": 1, "y": 1, "dx": 0}]),
            json!(vec![json!({"k": "move", "x": 1, "y": 1}); MAX_OPS + 1]),
        ] {
            assert_eq!(checked_ops(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_key_with_modifiers_presses_them_around_it() {
        let ev = key_events("S", &["ctrl", "shift"]);
        let vk = |v: u16, up: bool| KeyEvent::Vk { vk: v, extended: false, up };
        assert_eq!(ev, [vk(0x11, false), vk(0x10, false), vk(0x53, false), vk(0x53, true), vk(0x10, true), vk(0x11, true)]);
        assert_eq!(key_events("ArrowLeft", &[]), [KeyEvent::Vk { vk: 0x25, extended: true, up: false }, KeyEvent::Vk { vk: 0x25, extended: true, up: true }]);
        assert_eq!(key_events("F12", &[]).len(), 2);
        assert!(key_events("Nope", &["ctrl"]).is_empty());
        // The Windows key: alone it opens Start, held it is an extended modifier (Win+E).
        let win = |up: bool| KeyEvent::Vk { vk: 0x5B, extended: true, up };
        assert_eq!(key_events("Meta", &[]), [win(false), win(true)]);
        assert_eq!(key_events("E", &["meta"]), [win(false), vk(0x45, false), vk(0x45, true), win(true)]);
        assert_eq!(checked_ops(&json!([{"k": "key", "key": "L", "mods": ["meta"]}])).unwrap()[0]["mods"], json!(["meta"]));
        assert_eq!(key_code("0"), Some((0x30, false)));
        assert_eq!(key_code("F1"), Some((0x70, false)));
        assert_eq!(key_code("F0"), None);
    }

    #[test]
    fn text_is_typed_as_unicode_and_line_ends_as_enter() {
        let ev = text_events("é\r\n😀\tx");
        let units: Vec<String> = ev
            .iter()
            .filter(|e| !matches!(e, KeyEvent::Vk { up: true, .. } | KeyEvent::Unicode { up: true, .. }))
            .map(|e| match e {
                KeyEvent::Vk { vk, .. } => format!("vk{vk:x}"),
                KeyEvent::Unicode { unit, .. } => format!("u{unit:x}"),
            })
            .collect();
        assert_eq!(units, ["ue9", "vkd", "ud83d", "ude00", "vk9", "u78"], "one Enter for CR LF; the emoji as its two surrogates");
        assert_eq!(ev.len(), units.len() * 2, "every key goes up again");
        assert_eq!(checked_text("Mật khẩu 123"), Some("Mật khẩu 123"));
        assert_eq!(checked_text("line\nline"), Some("line\nline"));
        for bad in ["", "\u{7}bell", "\u{1b}[2J"] {
            assert_eq!(checked_text(bad), None, "{bad:?}");
        }
        assert_eq!(checked_text(&"x".repeat(MAX_TEXT + 1)), None);
    }

    #[test]
    fn late_input_is_not_done() {
        assert!(fresh(Some(1000), 1500));
        assert!(fresh(Some(1000), 2000));
        assert!(!fresh(Some(1000), 2001));
        assert!(!fresh(Some(5000), 1000), "from the future");
        assert!(!fresh(None, 1000));
    }

    #[test]
    fn the_stop_hotkey_needs_two_modifiers_one_of_them_ctrl_or_alt() {
        assert_eq!(parse_hotkey(DEFAULT_HOTKEY), Some(Hotkey { mods: MOD_CONTROL | MOD_ALT | MOD_SHIFT, vk: 0x4B }));
        assert_eq!(parse_hotkey("ctrl + shift + f9"), Some(Hotkey { mods: MOD_CONTROL | MOD_SHIFT, vk: 0x78 }));
        assert_eq!(parse_hotkey("Win+Alt+3"), Some(Hotkey { mods: MOD_WIN | MOD_ALT, vk: 0x33 }));
        for bad in ["K", "Ctrl+K", "Shift+Win+K", "Ctrl+Ctrl+K", "Ctrl+Alt+Enter", "Ctrl+Alt+", "Ctrl+Hyper+K", ""] {
            assert_eq!(parse_hotkey(bad), None, "{bad:?}");
        }
    }
}
