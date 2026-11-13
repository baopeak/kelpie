//! Agents in a pane through the DevTools protocol: input the page cannot tell from the person's (trusted mouse and key events, only in a pane
//! the computer shows), pictures of a shown page, and the requests a page made since an agent first used the pane. The protocol calls exist on
//! Windows only (`AVAILABLE`); what decides the events to send, how a picture is clipped and how requests are kept and listed builds everywhere,
//! so its tests run on every platform.
//!
//! Input goes as steps (`send`). A press is followed at once by its release, which always goes, even to a pane hidden meanwhile: no button or
//! key is left down. Every other step goes only while the window thread finds the pane shown (`devtools_when`); one refused ends the steps
//! there, and what went before says whether the synthetic path may do the action instead (`fallback`).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Url;

use super::agent_snapshot::scrub_url;
use super::NOT_SHOWN;

/// Trusted input, screenshots and the network list exist on this platform.
pub const AVAILABLE: bool = cfg!(windows);

pub const SHOT_HIDDEN: &str = "This tab is in the background; screenshots need it shown. Ask the user to show it.";
pub const SHOT_MINIMISED: &str = "The Kelpie window is minimised; screenshots need the pane shown. Ask the user to restore it.";

/// A step that was not sent because the page could not be told that the input is Kelpie's (`busy`).
pub const NOT_SENT: &str = "not sent: the page did not answer";
/// A step that was not sent because the page, the element, the focus or what is at the press's point is not what it was.
pub const MISSED: &str = "not sent: something else is at the element's place";

const MOUSE: &str = "Input.dispatchMouseEvent";
const KEY: &str = "Input.dispatchKeyEvent";
const TEXT: &str = "Input.insertText";

// ---------- keys ----------

const ALT: u32 = 1;
const CONTROL: u32 = 2;
const META: u32 = 4;
const SHIFT: u32 = 8;

/// The protocol's modifier bits of `names` (Alt, Control, Meta, Shift); other names count for nothing.
pub fn modifiers(names: &[&str]) -> u32 {
    names.iter().fold(0, |m, n| {
        m | match *n {
            "Alt" => ALT,
            "Control" => CONTROL,
            "Meta" => META,
            "Shift" => SHIFT,
            _ => 0,
        }
    })
}

/// The punctuation keys of a US keyboard, unshifted: `code` and Windows virtual key.
const PUNCT: [(char, &str, u32); 11] = [
    (';', "Semicolon", 186),
    ('=', "Equal", 187),
    (',', "Comma", 188),
    ('-', "Minus", 189),
    ('.', "Period", 190),
    ('/', "Slash", 191),
    ('`', "Backquote", 192),
    ('[', "BracketLeft", 219),
    ('\\', "Backslash", 220),
    (']', "BracketRight", 221),
    ('\'', "Quote", 222),
];

/// `(key, code, Windows virtual key, text)` of a key an agent may press: a named key (as a phone's), F1–F12, or one character.
fn key_of(key: &str) -> Option<(String, String, u32, Option<String>)> {
    if let Some((k, code, vk, text)) = super::takeover::key_info(key) {
        return Some((k.into(), code.into(), vk, text.map(str::to_string)));
    }
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=12).contains(n)) {
        return Some((key.into(), key.into(), 111 + n, None));
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    if c.is_control() {
        return None;
    }
    let (code, vk) = match c {
        'a'..='z' | 'A'..='Z' => (format!("Key{}", c.to_ascii_uppercase()), u32::from(c.to_ascii_uppercase())),
        '0'..='9' => (format!("Digit{c}"), u32::from(c)),
        ' ' => ("Space".into(), 32),
        _ => PUNCT.iter().find(|(p, _, _)| *p == c).map_or((String::new(), 0), |(_, code, vk)| (code.to_string(), *vk)),
    };
    Some((c.to_string(), code, vk, Some(c.to_string())))
}

/// Why an agent's key goes the synthetic way, if it does: with Control, Alt or Meta held, or Shift with Insert or Delete, a trusted key
/// could paste from or copy to the person's clipboard or reach the browser's own shortcuts; F5, F7, F11 and F12 are the browser's. Only
/// the page's own handlers see a synthetic key. (Typing's own Control+A and Control+End are not an agent's keys.)
pub fn synthetic_key(key: &str, mods: u32) -> Option<&'static str> {
    if mods & (ALT | CONTROL | META) != 0 || (mods & SHIFT != 0 && matches!(key, "Insert" | "Delete")) {
        return Some("a key with Control, Alt or Meta (or Shift with Insert or Delete) goes only to the page");
    }
    matches!(key, "F5" | "F7" | "F11" | "F12").then_some("a browser key goes only to the page")
}
/// A key's down and up events with modifier bits `mods`. With Control, Alt or Meta held it types nothing (a shortcut). None for a key not
/// in the list.
pub fn key_events(key: &str, mods: u32) -> Option<[Value; 2]> {
    let (key, code, vk, text) = key_of(key)?;
    let text = text.filter(|_| mods & (ALT | CONTROL | META) == 0);
    let mut down = json!({"type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": mods});
    if let Some(t) = &text {
        down["text"] = json!(t);
        down["unmodifiedText"] = json!(t);
    }
    let up = json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": mods});
    Some([down, up])
}

// ---------- input steps ----------

/// What a step does to the page: what may follow when the pane hides midway depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing the page acts on: the pointer moving there.
    None,
    /// Getting a field ready for text (the click that focuses it, the keys that empty it): typing the synthetic way does it again.
    Prep,
    /// The action itself: a click, a key.
    Input,
    /// The text going in.
    Text,
    /// The Enter that submits once the text is in.
    Submit,
    /// The release of what the step before pressed: always sent.
    Release,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub method: &'static str,
    pub params: Value,
    pub effect: Effect,
}

fn step(method: &'static str, params: Value, effect: Effect) -> Step {
    Step { method, params, effect }
}

/// A click at `(x, y)`, CSS px of the page's viewport: the pointer moves there, then the button goes down and up (twice for a double click).
pub fn click_steps(x: f64, y: f64, right: bool, double: bool, mods: u32, effect: Effect) -> Vec<Step> {
    let (button, buttons) = if right { ("right", 2) } else { ("left", 1) };
    let mut out = vec![step(MOUSE, json!({"type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0, "modifiers": mods}), Effect::None)];
    for n in 1..=if double { 2 } else { 1 } {
        out.push(step(MOUSE, json!({"type": "mousePressed", "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": n, "modifiers": mods}), effect));
        out.push(step(MOUSE, json!({"type": "mouseReleased", "x": x, "y": y, "button": button, "buttons": 0, "clickCount": n, "modifiers": mods}), Effect::Release));
    }
    out
}

/// A key pressed and released; None for a key not in the list.
pub fn key_steps(key: &str, mods: u32, effect: Effect) -> Option<Vec<Step>> {
    let [down, up] = key_events(key, mods)?;
    Some(vec![step(KEY, down, effect), step(KEY, up, Effect::Release)])
}

/// Typing into the field at `(x, y)`: a click focuses it, Control+A and Delete empty it (`clear`) or Control+End goes to its end, the text
/// goes in as typed text, and Enter submits (`submit`).
pub fn type_steps(x: f64, y: f64, text: &str, clear: bool, submit: bool) -> Vec<Step> {
    let mut out = click_steps(x, y, false, false, 0, Effect::Prep);
    let keys: &[(&str, u32)] = if clear { &[("a", CONTROL), ("Delete", 0)] } else { &[("End", CONTROL)] };
    for (k, m) in keys {
        out.extend(key_steps(k, *m, Effect::Prep).unwrap_or_default());
    }
    if !text.is_empty() {
        out.push(step(TEXT, json!({"text": text}), Effect::Text));
    }
    if submit {
        out.extend(key_steps("Enter", 0, Effect::Submit).unwrap_or_default());
    }
    out
}

/// How a run of steps went.
#[derive(Debug, PartialEq)]
pub enum Sent {
    All,
    /// Step `at` was not sent (`why`: `NOT_SHOWN`, `NOT_SENT` or `MISSED`), nor what follows.
    Refused { at: usize, why: String },
    /// Step `at` failed; it may have gone.
    Failed { at: usize, error: String },
}

/// Whether a guarded call's error says the step was not sent.
fn refused(e: &str) -> bool {
    e == NOT_SHOWN || e == NOT_SENT || e == MISSED
}

/// Sends `steps` in order through `call(method, params, guarded)`: releases unguarded, everything else only while the pane is shown and
/// the agent may act. A press that failed otherwise is released all the same (it may have gone down).
pub async fn send<C, F>(steps: &[Step], mut call: C) -> Sent
where
    C: FnMut(&'static str, Value, bool) -> F,
    F: Future<Output = Result<Value, String>>,
{
    for (i, s) in steps.iter().enumerate() {
        let guarded = s.effect != Effect::Release;
        match call(s.method, s.params.clone(), guarded).await {
            Ok(_) => {}
            Err(e) if guarded && refused(&e) => return Sent::Refused { at: i, why: e },
            Err(e) => {
                if let Some(release) = steps.get(i + 1).filter(|n| guarded && n.effect == Effect::Release) {
                    let _ = call(release.method, release.params.clone(), false).await;
                }
                return Sent::Failed { at: i, error: e };
            }
        }
    }
    Sent::All
}

/// What follows a step refused at `at`, given what changed (`changed`: `page`, `gone`, `focus`, or `point` when something else was at the
/// press's point, `hidden`, `silent`): the synthetic events do the whole action while no more than the preparation for typing went and the
/// page is the same one; only the Enter that submits once the text is in, when nothing but the pane's showing changed; otherwise nothing.
pub fn after_refusal(steps: &[Step], at: usize, changed: &str) -> Fallback {
    match fallback(steps, at) {
        Fallback::Action if changed != "page" => Fallback::Action,
        Fallback::Submit if matches!(changed, "hidden" | "silent" | "point") => Fallback::Submit,
        _ => Fallback::Nothing,
    }
}

/// What a trusted action that stopped at step `at` (for `why`) had done, for the agent.
pub fn stopped_text(tool: &str, steps: &[Step], at: usize, why: &str) -> String {
    let done = &steps[..at.min(steps.len())];
    let did = |e: Effect| done.iter().any(|s| s.effect == e);
    let cleared = done.iter().any(|s| s.effect == Effect::Prep && s.method == KEY);
    if did(Effect::Text) {
        format!("The text went in; then {why}, so Enter was not pressed. Take a snapshot to see the page now.")
    } else if tool == "type" && did(Effect::Prep) {
        let what = if cleared { "clicked, and its text selected or removed" } else { "clicked" };
        format!("The field was {what}; then {why}, so nothing was typed. Take a snapshot to see the page now.")
    } else if did(Effect::Input) {
        format!("Part of the action went; then {why}. Take a snapshot to see the page now.")
    } else {
        format!("Nothing was done: {why}. Take a snapshot and try again.")
    }
}

/// The characters typing `text` leaves in a field: a single-line field drops line breaks, a multi-line one (`lines`) keeps one per break.
pub fn typed_len(text: &str, lines: bool) -> u64 {
    let n = if lines { text.replace("\r\n", "\n").chars().count() } else { text.chars().filter(|c| !matches!(c, '\r' | '\n')).count() };
    n as u64
}

/// Whether steps up to and with `at` may have acted on the page (a step that failed may have gone).
pub fn acted(steps: &[Step], at: usize) -> bool {
    steps[..(at + 1).min(steps.len())].iter().any(|s| matches!(s.effect, Effect::Prep | Effect::Input | Effect::Text | Effect::Submit))
}

/// What may still be done the synthetic way after the pane hid at step `at`.
#[derive(Debug, PartialEq, Eq)]
pub enum Fallback {
    /// The whole action: nothing that acts on the page went yet, or only what typing the synthetic way does again.
    Action,
    /// Only the Enter that submits: the text is in.
    Submit,
    /// Nothing: part of the action happened.
    Nothing,
}

pub fn fallback(steps: &[Step], at: usize) -> Fallback {
    let done = &steps[..at.min(steps.len())];
    if done.iter().any(|s| s.effect == Effect::Input) {
        Fallback::Nothing
    } else if done.iter().any(|s| s.effect == Effect::Text) {
        Fallback::Submit
    } else {
        Fallback::Action
    }
}

// ---------- screenshots ----------

/// The widest picture an agent gets, and the tallest.
pub const MAX_WIDTH: f64 = 1280.0;
pub const MAX_HEIGHT: f64 = 8000.0;
/// A PNG whose base64 is longer than this goes as a JPEG instead, and a JPEG is made smaller until it is no longer than this.
const JPEG_PAST: usize = 1536 * 1024;
/// The JPEGs tried in turn: quality, and the scale relative to the PNG's.
pub const JPEG_TRIES: [(u8, f64); 5] = [(80, 1.0), (60, 1.0), (40, 1.0), (40, 0.75), (40, 0.5)];

#[derive(Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What the engine says of the page (`geometry`), CSS px: the viewport, its scroll, the document's size, the device pixel ratio, and an
/// element's box in document coordinates.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
pub struct Geometry {
    pub w: f64,
    pub h: f64,
    pub sx: f64,
    pub sy: f64,
    pub dw: f64,
    pub dh: f64,
    pub dpr: f64,
    pub rect: Option<Rect>,
}

/// A picture to take: a region of the document (CSS px), the scale it is drawn at, whether it reaches beyond the viewport, and whether its
/// height was cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    pub beyond: bool,
    pub cut: bool,
}

/// The picture of the element (`g.rect`), else the whole page (`full`), else the viewport: drawn at most 1280 px wide and 8000 px high
/// (device pixels: CSS px × scale × device pixel ratio).
pub fn plan_shot(g: &Geometry, full: bool) -> Shot {
    let dpr = if g.dpr > 0.0 { g.dpr } else { 1.0 };
    let (dw, dh) = (g.dw.max(g.w), g.dh.max(g.h));
    let (x, y, w, h) = match (&g.rect, full) {
        (Some(r), _) => {
            let (x, y) = (r.x.max(0.0), r.y.max(0.0));
            (x, y, (r.x + r.w).min(dw) - x, (r.y + r.h).min(dh) - y)
        }
        (None, true) => (0.0, 0.0, dw, dh),
        (None, false) => (g.sx, g.sy, g.w, g.h),
    };
    let (width, height) = (w.max(1.0), h.max(1.0));
    let scale = (MAX_WIDTH / (width * dpr)).min(1.0);
    let tallest = MAX_HEIGHT / (scale * dpr);
    let cut = height > tallest;
    let height = height.min(tallest);
    // Half a pixel either way is rounding, not outside.
    let beyond = full || x < g.sx - 0.5 || y < g.sy - 0.5 || x + width > g.sx + g.w + 0.5 || y + height > g.sy + g.h + 0.5;
    Shot { x, y, width, height, scale, beyond, cut }
}

impl Shot {
    /// `Page.captureScreenshot`'s parameters: a PNG, or a JPEG of `quality`.
    pub fn params(&self, jpeg: Option<u8>) -> Value {
        let mut p = json!({"format": if jpeg.is_some() { "jpeg" } else { "png" }, "clip": super::clip(self.x, self.y, self.width, self.height, self.scale), "captureBeyondViewport": self.beyond});
        if let Some(q) = jpeg {
            p["quality"] = json!(q);
        }
        p
    }
}

/// A PNG's width and height, from its header.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)))
}

/// A JPEG's width and height, from its first frame header.
pub fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        // Start of frame (baseline, progressive…), not a Huffman, arithmetic-coding or JPEG-extension table.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let f = b.get(i + 5..i + 9)?;
            return Some((u32::from(u16::from_be_bytes([f[2], f[3]])), u32::from(u16::from_be_bytes([f[0], f[1]]))));
        }
        i += 2 + len;
    }
    None
}

/// Whether a PNG whose base64 is `base64_len` long goes as a JPEG instead; for a JPEG, whether it is still too large.
pub fn jpeg_instead(base64_len: usize) -> bool {
    base64_len > JPEG_PAST
}

// ---------- the network list ----------

/// The requests a pane keeps.
pub const NET_RING: usize = 300;
#[cfg(windows)]
pub const NET_EVENTS: [&str; 4] = ["Network.requestWillBeSent", "Network.responseReceived", "Network.loadingFinished", "Network.loadingFailed"];

/// One request: no headers, no bodies. `doc` is the origin of the document that made it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub seq: u64,
    pub doc: String,
    pub method: String,
    pub url: String,
    pub kind: String,
    pub status: Option<u16>,
    pub ms: Option<u64>,
    pub bytes: Option<u64>,
    pub failed: Option<String>,
    start: Option<f64>,
}

/// The last `NET_RING` requests of a pane, numbered on from where the list last started (`base`), and those still under way by the
/// protocol's request id.
#[derive(Debug, Default)]
pub struct NetLog {
    list: VecDeque<Request>,
    open: HashMap<String, u64>,
    last: u64,
    base: u64,
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// An address as the list shows it: the values of secret-looking query parameters taken out, no fragment, a `data:` address by its type
/// only, at most 300 characters.
pub fn net_url(u: &str) -> String {
    if let Some(rest) = u.strip_prefix("data:") {
        return format!("data:{}…", cut(rest.split([';', ',']).next().unwrap_or_default(), 60));
    }
    let Ok(mut url) = Url::parse(&scrub_url(u)) else { return String::new() };
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    cut(url.as_str(), 300)
}

/// The origin (`scheme://host[:port]`) of an address, or nothing.
fn origin_of(u: &str) -> String {
    Url::parse(u).ok().map(|u| u.origin()).filter(|o| o.is_tuple()).map(|o| o.ascii_serialization()).unwrap_or_default()
}

/// `1.2 KB`.
pub fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

/// What the list shows: addresses after `since` (a cursor), containing `filter`, failed ones only (no answer, or 400 and up), at most `max`.
pub struct NetQuery<'a> {
    pub filter: Option<&'a str>,
    pub failed_only: bool,
    pub since: u64,
    pub max: usize,
}

impl NetLog {
    /// An empty list that numbers on after `last`: a cursor of the list before stays a cursor.
    pub fn after(last: u64) -> NetLog {
        NetLog { last, base: last, ..NetLog::default() }
    }

    pub fn last(&self) -> u64 {
        self.last
    }

    /// One protocol event (`NET_EVENTS`) with its parameters.
    pub fn event(&mut self, method: &str, p: &Value) {
        let Some(id) = p["requestId"].as_str() else { return };
        let ts = p["timestamp"].as_f64();
        let status = |v: &Value| v.as_u64().and_then(|s| u16::try_from(s).ok());
        match method {
            "Network.requestWillBeSent" => {
                // A redirect: the request so far ends with the redirect's status, and the next one goes on under the same id. Its document
                // is the one the event names (none: never listed).
                let doc = origin_of(p["documentURL"].as_str().unwrap_or_default());
                if p["redirectResponse"].is_object() {
                    self.end(id, ts, |r| r.status = status(&p["redirectResponse"]["status"]));
                }
                self.last += 1;
                self.list.push_back(Request {
                    seq: self.last,
                    doc,
                    method: cut(p["request"]["method"].as_str().unwrap_or_default(), 16),
                    url: net_url(p["request"]["url"].as_str().unwrap_or_default()),
                    kind: p["type"].as_str().unwrap_or_default().to_ascii_lowercase(),
                    start: ts,
                    ..Request::default()
                });
                self.open.insert(id.to_string(), self.last);
                if self.list.len() > NET_RING {
                    if let Some(gone) = self.list.pop_front() {
                        self.open.retain(|_, seq| *seq != gone.seq);
                    }
                }
            }
            "Network.responseReceived" => {
                if let Some(r) = self.open_mut(id) {
                    r.status = status(&p["response"]["status"]);
                    if r.kind.is_empty() {
                        r.kind = p["type"].as_str().unwrap_or_default().to_ascii_lowercase();
                    }
                }
            }
            "Network.loadingFinished" => self.end(id, ts, |r| r.bytes = p["encodedDataLength"].as_f64().map(|b| b.max(0.0) as u64)),
            "Network.loadingFailed" => {
                let why = if p["canceled"].as_bool() == Some(true) { "canceled".to_string() } else { cut(p["errorText"].as_str().unwrap_or("failed"), 100) };
                self.end(id, ts, |r| r.failed = Some(why));
            }
            _ => {}
        }
    }

    fn open_mut(&mut self, id: &str) -> Option<&mut Request> {
        let seq = *self.open.get(id)?;
        let first = self.list.front()?.seq;
        self.list.get_mut(usize::try_from(seq.checked_sub(first)?).ok()?)
    }

    /// The request under way as `id` ends at `ts` (seconds), with what `f` adds.
    fn end(&mut self, id: &str, ts: Option<f64>, f: impl FnOnce(&mut Request)) {
        if let Some(r) = self.open_mut(id) {
            if let (Some(t), Some(s)) = (ts, r.start) {
                r.ms = Some(((t - s).max(0.0) * 1000.0).round() as u64);
            }
            f(r);
        }
        self.open.remove(id);
    }

    /// `GET 200 /api/login 34 ms 1.2 KB xhr`, one line per request `q` picks among those made by a document on a site the call may see
    /// (`allowed`, given its origin), addresses on the page's `origin` by their path; then how to go on and the cursor for the next call.
    pub fn render(&self, q: &NetQuery, origin: &str, allowed: &dyn Fn(&str) -> bool) -> String {
        let filter = q.filter.map(str::to_lowercase).filter(|f| !f.is_empty());
        let picked: Vec<&Request> = self
            .list
            .iter()
            .filter(|r| r.seq > q.since && !r.doc.is_empty() && allowed(&r.doc))
            .filter(|r| !q.failed_only || r.failed.is_some() || r.status.is_some_and(|s| s >= 400))
            .filter(|r| filter.as_ref().is_none_or(|f| r.url.to_lowercase().contains(f.as_str())))
            .collect();
        let shown = &picked[..picked.len().min(q.max)];
        let more = picked.len() - shown.len();
        let mut lines = Vec::new();
        let first = self.list.front().map_or(self.last + 1, |r| r.seq);
        if q.since < self.base {
            lines.push("(The list started again since that cursor: the requests before are gone.)".into());
        }
        let lost = first.saturating_sub(q.since.max(self.base) + 1);
        if lost > 0 {
            lines.push(format!("({lost} earlier requests were dropped: Kelpie keeps the last {NET_RING}.)"));
        }
        lines.extend(shown.iter().map(|r| line(r, origin)));
        if shown.is_empty() {
            lines.push(if self.last == self.base { "No requests since an agent first used this pane." } else { "No matching requests." }.into());
        }
        let next = if more > 0 { shown.last().map_or(q.since, |r| r.seq) } else { q.since.max(self.last) };
        if more > 0 {
            lines.push(format!("… {more} more: call again with since={next}"));
        }
        lines.push(format!("next cursor: {next}"));
        lines.join("\n")
    }
}

fn line(r: &Request, origin: &str) -> String {
    let state = match (&r.failed, r.status) {
        (Some(_), _) => "failed".to_string(),
        (None, Some(s)) => s.to_string(),
        (None, None) => "pending".to_string(),
    };
    let url = r.url.strip_prefix(origin).filter(|rest| !origin.is_empty() && rest.starts_with('/')).unwrap_or(&r.url);
    let mut s = format!("{} {state} {url}", r.method);
    if let Some(ms) = r.ms {
        s.push_str(&format!(" {ms} ms"));
    }
    if let Some(b) = r.bytes {
        s.push_str(&format!(" {}", size(b)));
    }
    if !r.kind.is_empty() {
        s.push_str(&format!(" {}", r.kind));
    }
    if let Some(why) = &r.failed {
        s.push_str(&format!(" ({why})"));
    }
    s
}

// ---------- protocol calls ----------

/// Looked at on the window thread right before a guarded call: whether the window hides the pane.
pub type Hidden = Box<dyn FnOnce() -> bool + Send>;
/// A pane's network receivers: event and registration token.
pub type Tokens = Vec<(&'static str, i64)>;

/// One protocol call within `limit`; with `hidden`, only while the computer shows the page (else `NOT_SHOWN`).
#[cfg(windows)]
pub async fn call(wv: &tauri::Webview, method: &'static str, params: Value, hidden: Option<Hidden>, limit: Duration) -> Result<Value, String> {
    tokio::time::timeout(limit, super::devtools_when(wv, method, params, hidden)).await.unwrap_or_else(|_| Err(format!("{method} timed out")))
}

#[cfg(not(windows))]
pub async fn call(_wv: &tauri::Webview, _method: &'static str, _params: Value, _hidden: Option<Hidden>, _limit: Duration) -> Result<Value, String> {
    Err("not available on this platform".into())
}

/// Network receivers attached now, in every pane; a debug log follows the count, so that none can be seen to stay behind.
#[cfg(windows)]
static RECEIVERS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(windows)]
fn receivers(delta: isize) {
    let now = RECEIVERS.fetch_add(delta, std::sync::atomic::Ordering::Relaxed) + delta;
    tracing::debug!("agent network receivers attached: {now}");
}

/// Starts keeping the page's requests in `log`: the receivers, then `Network.enable` (no request or response bodies kept). None when the
/// page did not allow it.
#[cfg(windows)]
pub async fn network_on(wv: &tauri::Webview, log: Arc<Mutex<NetLog>>) -> Option<Tokens> {
    let mut tokens = Tokens::new();
    for event in NET_EVENTS {
        let log = log.clone();
        let on = move |json: String| {
            if let Ok(v) = serde_json::from_str::<Value>(&json) {
                log.lock().event(event, &v);
            }
        };
        match super::devtools_subscribe(wv, event, on).await {
            Ok(token) => {
                tokens.push((event, token));
                receivers(1);
            }
            Err(_) => {
                network_off(Some(wv), tokens);
                return None;
            }
        }
    }
    let limit = Duration::from_secs(5);
    let lean = json!({"maxTotalBufferSize": 0, "maxResourceBufferSize": 0, "maxPostDataSize": 0});
    let enabled = call(wv, "Network.enable", lean, None, limit).await.is_ok() || call(wv, "Network.enable", json!({}), None, limit).await.is_ok();
    if !enabled {
        detach(wv, tokens);
        return None;
    }
    Some(tokens)
}

#[cfg(windows)]
fn detach(wv: &tauri::Webview, tokens: Tokens) {
    let n = tokens.len() as isize;
    for (event, token) in tokens {
        super::devtools_unsubscribe(wv, event, token);
    }
    receivers(-n);
}

/// Stops handing the page's requests on: its receivers go (with the page, when it is gone already). `network_disable` follows.
#[cfg(windows)]
pub fn network_off(wv: Option<&tauri::Webview>, tokens: Tokens) {
    if tokens.is_empty() {
        return;
    }
    match wv {
        Some(wv) => detach(wv, tokens),
        None => receivers(-(tokens.len() as isize)),
    }
}

/// `Network.disable`: the page stops reporting requests.
#[cfg(windows)]
pub async fn network_disable(wv: &tauri::Webview) {
    let _ = call(wv, "Network.disable", json!({}), None, Duration::from_secs(5)).await;
}

/// The pane's zoom factor (1 at 100 %), as the person set it; none when the page did not say.
#[cfg(windows)]
pub async fn zoom_factor(wv: &tauri::Webview) -> Option<f64> {
    let (tx, rx) = tokio::sync::oneshot::channel::<f64>();
    wv.with_webview(move |pv| unsafe {
        let mut z = 0.0;
        if pv.controller().ZoomFactor(&mut z).is_ok() {
            let _ = tx.send(z);
        }
    })
    .ok()?;
    tokio::time::timeout(Duration::from_secs(1), rx).await.ok()?.ok()
}

#[cfg(not(windows))]
pub async fn network_on(_wv: &tauri::Webview, _log: Arc<Mutex<NetLog>>) -> Option<Tokens> {
    None
}

#[cfg(not(windows))]
pub fn network_off(_wv: Option<&tauri::Webview>, _tokens: Tokens) {}

#[cfg(not(windows))]
pub async fn network_disable(_wv: &tauri::Webview) {}

#[cfg(not(windows))]
pub async fn zoom_factor(_wv: &tauri::Webview) -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_are_the_protocol_s_bits() {
        assert_eq!(modifiers(&[]), 0);
        assert_eq!(modifiers(&["Control", "Shift"]), 10);
        assert_eq!(modifiers(&["Alt", "Control", "Meta", "Shift"]), 15);
        assert_eq!(modifiers(&["Hyper", "Alt"]), 1);
    }

    #[test]
    fn keys_carry_their_codes_and_text() {
        let [down, up] = key_events("Enter", 0).unwrap();
        assert_eq!((down["type"].as_str(), down["text"].as_str(), down["windowsVirtualKeyCode"].as_u64()), (Some("keyDown"), Some("\r"), Some(13)));
        assert_eq!(up["type"], "keyUp");
        let [f5, _] = key_events("F5", 0).unwrap();
        assert_eq!((f5["type"].as_str(), f5["code"].as_str(), f5["windowsVirtualKeyCode"].as_u64()), (Some("rawKeyDown"), Some("F5"), Some(116)));
        assert_eq!(key_events("F12", 0).unwrap()[0]["windowsVirtualKeyCode"], 123);
        let [a, _] = key_events("a", 0).unwrap();
        assert_eq!((a["key"].as_str(), a["code"].as_str(), a["windowsVirtualKeyCode"].as_u64(), a["text"].as_str()), (Some("a"), Some("KeyA"), Some(65), Some("a")));
        assert_eq!(key_events("7", 0).unwrap()[0]["code"], "Digit7");
        assert_eq!(key_events(";", 0).unwrap()[0]["windowsVirtualKeyCode"], 186);
        assert_eq!(key_events("Space", 0).unwrap()[0]["text"], " ");
        // A character with no key of its own still types.
        let [e, _] = key_events("é", 0).unwrap();
        assert_eq!((e["code"].as_str(), e["text"].as_str()), (Some(""), Some("é")));
    }

    #[test]
    fn a_shortcut_types_nothing_but_shift_still_types() {
        let [ctrl_a, up] = key_events("a", CONTROL).unwrap();
        assert_eq!((ctrl_a["type"].as_str(), ctrl_a.get("text"), ctrl_a["modifiers"].as_u64()), (Some("rawKeyDown"), None, Some(2)));
        assert_eq!(up["modifiers"], 2);
        assert_eq!(key_events("A", SHIFT).unwrap()[0]["text"], "A");
    }

    #[test]
    fn only_listed_keys_go() {
        for k in ["F13", "F0", "Foo", "", "ab", "\n"] {
            assert!(key_events(k, 0).is_none(), "{k:?}");
        }
    }

    fn kinds(steps: &[Step]) -> Vec<String> {
        steps.iter().map(|s| if s.method == TEXT { "text".to_string() } else { s.params["type"].as_str().unwrap_or_default().to_string() }).collect()
    }

    #[test]
    fn a_click_moves_presses_and_releases() {
        let s = click_steps(10.0, 20.0, false, false, 0, Effect::Input);
        assert_eq!(kinds(&s), ["mouseMoved", "mousePressed", "mouseReleased"]);
        assert_eq!((s[1].params["button"].as_str(), s[1].params["buttons"].as_u64(), s[1].params["x"].as_f64()), (Some("left"), Some(1), Some(10.0)));
        assert_eq!(s.iter().map(|s| s.effect).collect::<Vec<_>>(), [Effect::None, Effect::Input, Effect::Release]);
        let double = click_steps(1.0, 1.0, false, true, 0, Effect::Input);
        assert_eq!(double.len(), 5);
        assert_eq!(double[3].params["clickCount"], 2);
        let right = click_steps(1.0, 1.0, true, false, CONTROL, Effect::Input);
        assert_eq!((right[1].params["button"].as_str(), right[1].params["buttons"].as_u64(), right[1].params["modifiers"].as_u64()), (Some("right"), Some(2), Some(2)));
    }

    #[test]
    fn typing_focuses_empties_inserts_and_submits() {
        let s = type_steps(5.0, 5.0, "hello", true, true);
        assert_eq!(kinds(&s), ["mouseMoved", "mousePressed", "mouseReleased", "rawKeyDown", "keyUp", "rawKeyDown", "keyUp", "text", "keyDown", "keyUp"]);
        assert_eq!((s[3].params["key"].as_str(), s[3].params["modifiers"].as_u64()), (Some("a"), Some(2)));
        assert_eq!(s[5].params["key"], "Delete");
        assert_eq!(s[7].params["text"], "hello");
        assert_eq!(s[8].effect, Effect::Submit);
        // Kept text: the caret goes to the end; no text, nothing inserted.
        let keep = type_steps(5.0, 5.0, "", false, false);
        assert_eq!(kinds(&keep), ["mouseMoved", "mousePressed", "mouseReleased", "rawKeyDown", "keyUp"]);
        assert_eq!((keep[3].params["key"].as_str(), keep[3].params["modifiers"].as_u64()), (Some("End"), Some(2)));
        assert!(keep.iter().all(|s| matches!(s.effect, Effect::None | Effect::Prep | Effect::Release)));
    }

    /// The calls `send` makes (" unguarded" marked); the window thread finds the pane hidden at call `hidden_at`, a call fails at `fails_at`.
    fn drive(steps: &[Step], hidden_at: Option<usize>, fails_at: Option<usize>) -> (Sent, Vec<String>) {
        let made = Mutex::new(Vec::new());
        let sent = tauri::async_runtime::block_on(send(steps, |method, params, guarded| {
            let n = made.lock().len();
            let kind = if method == TEXT { "text".to_string() } else { params["type"].as_str().unwrap_or_default().to_string() };
            let refused = guarded && hidden_at == Some(n);
            if !refused {
                made.lock().push(format!("{kind}{}", if guarded { "" } else { " unguarded" }));
            }
            async move {
                if refused {
                    Err(NOT_SHOWN.to_string())
                } else if fails_at == Some(n) {
                    Err("timed out".to_string())
                } else {
                    Ok(json!({}))
                }
            }
        }));
        (sent, made.into_inner())
    }

    #[test]
    fn steps_go_in_order_and_releases_always_go() {
        let (sent, made) = drive(&click_steps(1.0, 1.0, false, false, 0, Effect::Input), None, None);
        assert_eq!((sent, made), (Sent::All, vec!["mouseMoved".into(), "mousePressed".into(), "mouseReleased unguarded".into()]));
    }

    #[test]
    fn a_pane_hidden_midway_gets_nothing_more() {
        let steps = click_steps(1.0, 1.0, false, false, 0, Effect::Input);
        let (sent, made) = drive(&steps, Some(1), None);
        assert_eq!(sent, Sent::Refused { at: 1, why: NOT_SHOWN.into() });
        assert_eq!(made, ["mouseMoved"], "the press was refused: nothing is down, nothing to release");
    }

    #[test]
    fn a_press_that_failed_is_released() {
        let (sent, made) = drive(&click_steps(1.0, 1.0, false, false, 0, Effect::Input), None, Some(1));
        assert_eq!(sent, Sent::Failed { at: 1, error: "timed out".into() });
        assert_eq!(made, ["mouseMoved", "mousePressed", "mouseReleased unguarded"]);
    }

    #[test]
    fn the_synthetic_path_takes_over_only_while_nothing_acted() {
        let click = click_steps(1.0, 1.0, false, true, 0, Effect::Input);
        assert_eq!(fallback(&click, 0), Fallback::Action, "only the pointer moved");
        assert_eq!(fallback(&click, 1), Fallback::Action, "the press was refused");
        assert_eq!(fallback(&click, 3), Fallback::Nothing, "a double click hidden after its first click");
        let typing = type_steps(1.0, 1.0, "x", true, true);
        assert_eq!(fallback(&typing, 5), Fallback::Action, "focusing and emptying are done again");
        let submit = typing.iter().position(|s| s.effect == Effect::Submit).unwrap();
        assert_eq!(fallback(&typing, submit), Fallback::Submit, "the text is in: only Enter is left");
        let key = key_steps("Enter", 0, Effect::Input).unwrap();
        assert_eq!(fallback(&key, 0), Fallback::Action);
    }

    fn viewport() -> Geometry {
        Geometry { w: 1280.0, h: 800.0, sx: 0.0, sy: 300.0, dw: 1280.0, dh: 5000.0, dpr: 1.0, rect: None }
    }

    #[test]
    fn the_visible_page_is_clipped_where_it_is_scrolled() {
        let s = plan_shot(&viewport(), false);
        assert_eq!(s, Shot { x: 0.0, y: 300.0, width: 1280.0, height: 800.0, scale: 1.0, beyond: false, cut: false });
        let p = s.params(None);
        assert_eq!((p["format"].as_str(), p["clip"]["y"].as_f64(), p["captureBeyondViewport"].as_bool(), p.get("quality")), (Some("png"), Some(300.0), Some(false), None));
        assert_eq!(s.params(Some(60))["quality"], 60);
    }

    #[test]
    fn a_picture_is_at_most_1280_px_wide() {
        let wide = plan_shot(&Geometry { w: 1920.0, dw: 1920.0, ..viewport() }, false);
        assert!((wide.scale - 2.0 / 3.0).abs() < 1e-9, "{}", wide.scale);
        let sharp = plan_shot(&Geometry { dpr: 2.0, ..viewport() }, false);
        assert_eq!(sharp.scale, 0.5, "device pixels count");
        let small = plan_shot(&Geometry { w: 600.0, dw: 600.0, dpr: 1.5, ..viewport() }, false);
        assert_eq!(small.scale, 1.0, "never drawn larger");
        assert_eq!(plan_shot(&Geometry { dpr: 0.0, ..viewport() }, false).scale, 1.0);
    }

    #[test]
    fn the_whole_page_is_cut_at_8000_px() {
        let s = plan_shot(&Geometry { dh: 20_000.0, ..viewport() }, true);
        assert_eq!((s.x, s.y, s.height, s.beyond, s.cut), (0.0, 0.0, 8000.0, true, true));
        let sharp = plan_shot(&Geometry { dh: 20_000.0, dpr: 2.0, ..viewport() }, true);
        assert_eq!((sharp.scale, sharp.height), (0.5, 8000.0), "8000 device pixels at half scale");
        assert!(!plan_shot(&viewport(), true).cut);
    }

    #[test]
    fn an_element_is_clipped_to_its_box_within_the_document() {
        let inside = plan_shot(&Geometry { rect: Some(Rect { x: 100.0, y: 400.0, w: 200.0, h: 50.0 }), ..viewport() }, false);
        assert_eq!((inside.x, inside.y, inside.width, inside.height, inside.beyond), (100.0, 400.0, 200.0, 50.0, false));
        let tall = plan_shot(&Geometry { rect: Some(Rect { x: 0.0, y: 200.0, w: 400.0, h: 2000.0 }), ..viewport() }, false);
        assert!(tall.beyond, "it reaches past the viewport");
        let off = plan_shot(&Geometry { rect: Some(Rect { x: -10.0, y: 4990.0, w: 50.0, h: 50.0 }), ..viewport() }, false);
        assert_eq!((off.x, off.y, off.width, off.height), (0.0, 4990.0, 40.0, 10.0));
    }

    #[test]
    fn png_headers_give_the_size_and_large_ones_go_as_jpeg() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(1280u32.to_be_bytes());
        png.extend(720u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((1280, 720)));
        assert_eq!(png_size(b"\xff\xd8\xff\xe0 not a png at all"), None);
        assert!(!jpeg_instead(1536 * 1024));
        // SOI, an APP0 segment, then a baseline frame header: precision 8, height 720, width 1280.
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x02, 0xD0, 0x05, 0x00, 0x03];
        assert_eq!(jpeg_size(&jpeg), Some((1280, 720)));
        assert_eq!(jpeg_size(&png), None);
        assert!(jpeg_instead(1536 * 1024 + 1));
    }

    fn sent(log: &mut NetLog, id: &str, method: &str, url: &str, kind: &str, ts: f64) {
        sent_by(log, id, method, url, kind, ts, url);
    }

    /// A request the document at `doc` made.
    fn sent_by(log: &mut NetLog, id: &str, method: &str, url: &str, kind: &str, ts: f64, doc: &str) {
        log.event("Network.requestWillBeSent", &json!({"requestId": id, "request": {"method": method, "url": url}, "type": kind, "timestamp": ts, "documentURL": doc}));
    }

    fn any(_: &str) -> bool {
        true
    }

    fn all() -> NetQuery<'static> {
        NetQuery { filter: None, failed_only: false, since: 0, max: 50 }
    }

    #[test]
    fn a_request_is_one_line() {
        let mut log = NetLog::default();
        sent(&mut log, "1", "POST", "http://127.0.0.1:8080/api/login", "XHR", 10.0);
        log.event("Network.responseReceived", &json!({"requestId": "1", "response": {"status": 200}, "type": "XHR"}));
        log.event("Network.loadingFinished", &json!({"requestId": "1", "timestamp": 10.034, "encodedDataLength": 1229}));
        sent(&mut log, "2", "GET", "https://cdn.example.com/app.js", "Script", 10.1);
        assert_eq!(log.render(&all(), "http://127.0.0.1:8080", &any), "POST 200 /api/login 34 ms 1.2 KB xhr\nGET pending https://cdn.example.com/app.js script\nnext cursor: 2");
    }

    #[test]
    fn failures_and_redirects_are_kept_apart() {
        let mut log = NetLog::default();
        sent(&mut log, "1", "GET", "http://a.test/old", "Document", 1.0);
        log.event("Network.requestWillBeSent", &json!({"requestId": "1", "request": {"method": "GET", "url": "http://a.test/new"}, "type": "Document", "timestamp": 1.01, "documentURL": "http://a.test/new", "redirectResponse": {"status": 301}}));
        log.event("Network.responseReceived", &json!({"requestId": "1", "response": {"status": 404}}));
        log.event("Network.loadingFinished", &json!({"requestId": "1", "timestamp": 1.02, "encodedDataLength": 10}));
        sent(&mut log, "2", "GET", "http://a.test/down", "Fetch", 2.0);
        log.event("Network.loadingFailed", &json!({"requestId": "2", "timestamp": 2.5, "errorText": "net::ERR_CONNECTION_REFUSED"}));
        let text = log.render(&all(), "http://a.test", &any);
        assert_eq!(text.lines().take(3).collect::<Vec<_>>(), ["GET 301 /old 10 ms document", "GET 404 /new 10 ms 10 B document", "GET failed /down 500 ms fetch (net::ERR_CONNECTION_REFUSED)"]);
        let failed = log.render(&NetQuery { failed_only: true, ..all() }, "http://a.test", &any);
        assert!(!failed.contains("/old") && failed.contains("/new") && failed.contains("/down"), "{failed}");
        let filtered = log.render(&NetQuery { filter: Some("DOWN"), ..all() }, "http://a.test", &any);
        assert_eq!(filtered, "GET failed /down 500 ms fetch (net::ERR_CONNECTION_REFUSED)\nnext cursor: 3");
    }

    #[test]
    fn the_cursor_goes_on_where_the_last_call_stopped() {
        let mut log = NetLog::default();
        for i in 1..=5 {
            sent(&mut log, &i.to_string(), "GET", &format!("http://a.test/{i}"), "Image", 1.0);
        }
        let first = log.render(&NetQuery { max: 2, ..all() }, "http://a.test", &any);
        assert_eq!(first, "GET pending /1 image\nGET pending /2 image\n… 3 more: call again with since=2\nnext cursor: 2");
        let rest = log.render(&NetQuery { since: 2, ..all() }, "http://a.test", &any);
        assert!(rest.starts_with("GET pending /3 image") && rest.ends_with("next cursor: 5"), "{rest}");
        assert_eq!(log.render(&NetQuery { since: 5, ..all() }, "http://a.test", &any), "No matching requests.\nnext cursor: 5");
        assert_eq!(NetLog::default().render(&all(), "", &any), "No requests since an agent first used this pane.\nnext cursor: 0");
    }

    #[test]
    fn the_log_keeps_the_last_300() {
        let mut log = NetLog::default();
        for i in 1..=(NET_RING + 20) {
            sent(&mut log, &i.to_string(), "GET", &format!("http://a.test/{i}"), "Image", 1.0);
        }
        assert_eq!(log.list.len(), NET_RING);
        assert_eq!(log.open.len(), NET_RING, "requests dropped from the list are forgotten");
        // An event for a dropped request changes nothing.
        log.event("Network.loadingFinished", &json!({"requestId": "1", "timestamp": 2.0, "encodedDataLength": 5}));
        let text = log.render(&NetQuery { max: 1, ..all() }, "http://a.test", &any);
        assert!(text.starts_with("(20 earlier requests were dropped: Kelpie keeps the last 300.)\nGET pending /21 image"), "{text}");
        assert!(!log.render(&NetQuery { since: 20, max: 1, ..all() }, "http://a.test", &any).contains("dropped"));
    }

    #[test]
    fn addresses_lose_secrets_and_fragments() {
        let u = net_url("https://api.example.com/cb?code=abc&page=2&access_token=xyz#state=1");
        assert!(!u.contains("abc") && !u.contains("xyz") && !u.contains('#') && u.contains("page=2"), "{u}");
        assert_eq!(net_url("data:image/png;base64,iVBORw0KGgo="), "data:image/png…");
        assert_eq!(net_url("not a url"), "");
        assert_eq!(net_url(&format!("https://a.test/{}", "x".repeat(400))).chars().count(), 301);
        let mut log = NetLog::default();
        sent(&mut log, "1", "GET", "https://a.test/x?session=s3cr3t", "XHR", 1.0);
        assert!(!log.render(&all(), "https://a.test", &any).contains("s3cr3t"));
    }

    #[test]
    fn sizes_read_as_people_write_them() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1229), "1.2 KB");
        assert_eq!(size(3 * 1024 * 1024 + 400 * 1024), "3.4 MB");
    }

    #[test]
    fn only_requests_of_documents_on_allowed_sites_are_listed() {
        use super::super::agent_snapshot::{site_allowed, Kind};
        let mut log = NetLog::default();
        sent_by(&mut log, "1", "POST", "https://app.example.com/api/login", "XHR", 1.0, "https://app.example.com/login");
        // The person took the shared pane to their bank meanwhile, and its page fetched from a third site.
        sent_by(&mut log, "2", "GET", "https://bank.example.org/accounts?id=42", "Document", 2.0, "https://bank.example.org/accounts?id=42");
        sent_by(&mut log, "3", "GET", "https://cdn.example.net/app.js", "Script", 2.1, "https://bank.example.org/accounts?id=42");
        // Back on the shared site: a request to another site made by the shared page is the page's own.
        sent_by(&mut log, "4", "GET", "https://cdn.example.net/ui.js", "Script", 3.0, "https://app.example.com/home");
        let shared = vec!["https://app.example.com".to_string()];
        let allowed = |doc: &str| site_allowed(doc, Kind::Shared, false, &shared, &[]).is_ok();
        let text = log.render(&all(), "https://app.example.com", &allowed);
        assert_eq!(text, "POST pending /api/login xhr\nGET pending https://cdn.example.net/ui.js script\nnext cursor: 4");
        assert!(!text.contains("bank") && !text.contains("app.js"));
        // A request whose document is unknown is not listed.
        log.event("Network.requestWillBeSent", &json!({"requestId": "5", "request": {"method": "GET", "url": "https://app.example.com/x"}, "timestamp": 4.0}));
        assert!(!log.render(&all(), "https://app.example.com", &allowed).contains("/x"));
    }

    #[test]
    fn a_list_started_again_keeps_its_cursors() {
        let mut log = NetLog::default();
        for i in 1..=3 {
            sent(&mut log, &i.to_string(), "GET", &format!("http://a.test/{i}"), "Image", 1.0);
        }
        let mut log = NetLog::after(log.last());
        assert_eq!(log.render(&NetQuery { since: 3, ..all() }, "http://a.test", &any), "No requests since an agent first used this pane.\nnext cursor: 3");
        sent(&mut log, "9", "GET", "http://a.test/new", "Fetch", 2.0);
        assert_eq!(log.render(&NetQuery { since: 3, ..all() }, "http://a.test", &any), "GET pending /new fetch\nnext cursor: 4");
        let old = log.render(&NetQuery { since: 1, ..all() }, "http://a.test", &any);
        assert_eq!(old, "(The list started again since that cursor: the requests before are gone.)\nGET pending /new fetch\nnext cursor: 4");
    }

    #[test]
    fn addresses_lose_their_user_and_password() {
        let u = net_url("https://ann:hunter2@a.test/x");
        assert_eq!(u, "https://a.test/x");
    }

    #[test]
    fn keys_that_could_reach_the_clipboard_or_the_browser_go_the_synthetic_way() {
        for (k, m) in [("v", CONTROL), ("v", META), ("V", CONTROL | SHIFT), ("c", CONTROL), ("x", CONTROL), ("Insert", SHIFT), ("Delete", SHIFT), ("Insert", CONTROL), ("a", CONTROL), ("p", CONTROL), ("ArrowLeft", ALT), ("Enter", CONTROL), ("F5", 0), ("F12", SHIFT)] {
            assert!(synthetic_key(k, m).is_some(), "{k} {m}");
        }
        for (k, m) in [("v", 0), ("V", SHIFT), ("Enter", 0), ("Tab", SHIFT), ("ArrowLeft", SHIFT), ("Delete", 0), ("Insert", 0), ("F1", 0), ("Escape", 0)] {
            assert!(synthetic_key(k, m).is_none(), "{k} {m}");
        }
    }
    #[test]
    fn a_step_that_failed_may_have_acted() {
        let click = click_steps(1.0, 1.0, false, false, 0, Effect::Input);
        assert!(!acted(&click, 0), "only the pointer moved");
        assert!(acted(&click, 1), "the press may have gone down");
    }

    #[test]
    fn a_stopped_action_says_what_went() {
        let typing = type_steps(1.0, 1.0, "x", true, true);
        let at = |e: Effect| typing.iter().position(|s| s.effect == e).unwrap();
        assert!(stopped_text("type", &typing, 1, "the page changed").starts_with("Nothing was done: the page changed."));
        assert!(stopped_text("type", &typing, 3, "the focus moved to textbox \"Search\"").starts_with("The field was clicked; then the focus moved to textbox \"Search\", so nothing was typed."));
        assert!(stopped_text("type", &typing, at(Effect::Text), "the page changed").starts_with("The field was clicked, and its text selected or removed; then"));
        assert!(stopped_text("type", &typing, at(Effect::Submit), "the page changed").starts_with("The text went in; then the page changed, so Enter was not pressed."));
        let click = click_steps(1.0, 1.0, false, true, 0, Effect::Input);
        assert!(stopped_text("click", &click, 3, "the pane was hidden").starts_with("Part of the action went"));
    }

    #[test]
    fn the_typed_length_follows_what_a_field_keeps() {
        assert_eq!(typed_len("héllo", false), 5);
        assert_eq!(typed_len("a\r\nb", true), 3);
        assert_eq!(typed_len("a\r\nb\nc", false), 3);
    }

    #[test]
    fn a_refused_step_falls_back_while_only_preparation_went() {
        let typing = type_steps(1.0, 1.0, "x", true, true);
        let text = typing.iter().position(|s| s.effect == Effect::Text).unwrap();
        let submit = typing.iter().position(|s| s.effect == Effect::Submit).unwrap();
        // The focusing click went but the focus is elsewhere: typing the synthetic way fills the field itself.
        assert_eq!(after_refusal(&typing, 3, "focus"), Fallback::Action);
        assert_eq!(after_refusal(&typing, text, "focus"), Fallback::Action);
        assert_eq!(after_refusal(&typing, 1, "point"), Fallback::Action);
        // Another page: nothing more.
        assert_eq!(after_refusal(&typing, 0, "page"), Fallback::Nothing);
        assert_eq!(after_refusal(&typing, 3, "page"), Fallback::Nothing);
        // The text is in: Enter goes the synthetic way only when nothing but the pane's showing changed.
        assert_eq!(after_refusal(&typing, submit, "hidden"), Fallback::Submit);
        assert_eq!(after_refusal(&typing, submit, "focus"), Fallback::Nothing);
        assert_eq!(after_refusal(&typing, submit, "page"), Fallback::Nothing);
        // A key pressed on an element that does not take focus: the synthetic key goes to the element itself.
        let key = key_steps("ArrowDown", 0, Effect::Input).unwrap();
        assert_eq!(after_refusal(&key, 0, "focus"), Fallback::Action);
        // A click that already went is not repeated.
        let double = click_steps(1.0, 1.0, false, true, 0, Effect::Input);
        assert_eq!(after_refusal(&double, 3, "point"), Fallback::Nothing);
    }
}