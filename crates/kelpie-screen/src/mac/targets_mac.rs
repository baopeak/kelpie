//! Displays and windows as `targets` wants them (front to back), where a window is now, which window is at a point, the AX side of a window
//! (focus, minimized, raise) and the cursor. Everything is in global points, top-left origin, as Core Graphics has it.

use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::msg_send;

use super::ffi::*;
use crate::geom::{Monitor, Rect};
use crate::targets::{WindowInfo, MAC_DISPLAY};
use crate::wire::Shape;

/// A display: where it is (points), its pixels and how many a point is.
#[derive(Clone, Copy, Debug)]
pub struct Display {
    pub id: u32,
    pub bounds: Rect,
    pub pixels: (u32, u32),
    pub scale: f64,
    pub primary: bool,
}

/// A display's backing scale, as AppKit has it (`NSScreen.backingScaleFactor`): the display mode alone may say points (a virtual machine's
/// does), while ScreenCaptureKit and the screen draw at the backing scale.
fn backing_scale(id: u32) -> Option<f64> {
    on_main(move || {
        objc2::rc::autoreleasepool(|_| unsafe {
            let screens: Option<Retained<AnyObject>> = msg_send![class(c"NSScreen")?, screens];
            let key = nsstring("NSScreenNumber");
            ns_items(screens.as_deref()).into_iter().find_map(|s| {
                let desc: Option<Retained<AnyObject>> = msg_send![&*s, deviceDescription];
                let number: Option<Retained<AnyObject>> = msg_send![&*desc?, objectForKey: &*key];
                let n: u32 = msg_send![&*number?, unsignedIntValue];
                (n == id).then(|| -> f64 { msg_send![&*s, backingScaleFactor] })
            })
        })
    })
}

pub fn display(id: u32) -> Option<Display> {
    unsafe {
        let b = CGDisplayBounds(id);
        if b.size.width <= 0.0 {
            return None;
        }
        let mode = CGDisplayCopyDisplayMode(id);
        let mode_px = if mode.is_null() {
            0.0
        } else {
            let w = CGDisplayModeGetPixelWidth(mode) as f64;
            CGDisplayModeRelease(mode);
            w
        };
        let scale = backing_scale(id).unwrap_or(mode_px / b.size.width).max(1.0);
        let pixels = ((b.size.width * scale).round() as u32, (b.size.height * scale).round() as u32);
        Some(Display { id, bounds: b.rect(), pixels, scale, primary: id == CGMainDisplayID() })
    }
}

/// The active displays, the primary first.
pub fn displays() -> Vec<Display> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } != 0 {
        return vec![];
    }
    let mut out: Vec<Display> = ids[..n as usize].iter().filter_map(|id| display(*id)).collect();
    out.sort_by_key(|d| !d.primary);
    out
}

pub fn monitors() -> Vec<Monitor> {
    displays().iter().map(|d| Monitor { name: format!("{MAC_DISPLAY}{}", d.id), rect: d.bounds, dpi: (96.0 * d.scale).round() as u32, primary: d.primary }).collect()
}

/// The backing scale where a rectangle's middle is (a window on a Retina display is captured at 2 pixels a point).
pub fn scale_at(r: Rect) -> f64 {
    let (x, y) = (r.x + r.w / 2, r.y + r.h / 2);
    let ds = displays();
    ds.iter().find(|d| d.bounds.contains(x, y)).or(ds.first()).map_or(1.0, |d| d.scale)
}

/// One window as the window server lists it.
#[derive(Clone, Debug, Default)]
pub struct CgWindow {
    pub number: u32,
    pub pid: i32,
    pub layer: i64,
    pub bounds: CGRect,
    pub onscreen: bool,
    pub alpha: f64,
    pub title: String,
    pub owner: String,
}

fn parse(d: CFTypeRef) -> Option<CgWindow> {
    unsafe {
        Some(CgWindow {
            number: i64_of(dict_get(d, kCGWindowNumber))? as u32,
            pid: i64_of(dict_get(d, kCGWindowOwnerPID))? as i32,
            layer: i64_of(dict_get(d, kCGWindowLayer)).unwrap_or(0),
            bounds: rect_of(dict_get(d, kCGWindowBounds)).unwrap_or_default(),
            onscreen: bool_of(dict_get(d, kCGWindowIsOnscreen)).unwrap_or(false),
            alpha: f64_of(dict_get(d, kCGWindowAlpha)).unwrap_or(1.0),
            title: string_of(dict_get(d, kCGWindowName)).unwrap_or_default(),
            owner: string_of(dict_get(d, kCGWindowOwnerName)).unwrap_or_default(),
        })
    }
}

fn list(option: u32, relative: u32) -> Vec<CgWindow> {
    let Some(a) = Cf::take(unsafe { CGWindowListCopyWindowInfo(option, relative) }) else { return vec![] };
    array_items(a.ptr()).into_iter().filter_map(parse).collect()
}

/// The windows on screen, front to back.
pub fn on_screen() -> Vec<CgWindow> {
    list(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, kCGNullWindowID)
}

/// One window, if it is still there (`kCGWindowListOptionIncludingWindow`).
pub fn window(number: u32) -> Option<CgWindow> {
    list(8, number).into_iter().find(|w| w.number == number)
}

/// The topmost window at a point that could take a click (of `pid` only, if given), leaving out this helper's own (the border) and invisible
/// ones.
pub fn window_at(x: f64, y: f64, pid: Option<i32>) -> Option<CgWindow> {
    let me = std::process::id() as i32;
    on_screen().into_iter().find(|w| {
        let b = w.bounds;
        w.pid != me && pid.is_none_or(|p| w.pid == p) && w.alpha > 0.0 && x >= b.origin.x && y >= b.origin.y && x < b.origin.x + b.size.width && y < b.origin.y + b.size.height
    })
}

/// The app a click at a point would go to, by Accessibility's hit test (which leaves out windows that let clicks through).
pub fn pid_at(x: f64, y: f64) -> Option<i32> {
    let sys = Cf::take(unsafe { AXUIElementCreateSystemWide() })?;
    unsafe { AXUIElementSetMessagingTimeout(sys.ptr(), 0.5) };
    let mut el: CFTypeRef = std::ptr::null();
    if unsafe { AXUIElementCopyElementAtPosition(sys.ptr(), x as f32, y as f32, &mut el) } != 0 {
        return None;
    }
    let el = Cf::take(el)?;
    let mut pid = 0;
    (unsafe { AXUIElementGetPid(el.ptr(), &mut pid) } == 0).then_some(pid)
}

/// An app's name and bundle identifier.
pub fn app_of(pid: i32) -> (String, String) {
    objc2::rc::autoreleasepool(|_| unsafe {
        let Some(cls) = class(c"NSRunningApplication") else { return Default::default() };
        let app: Option<Retained<AnyObject>> = msg_send![cls, runningApplicationWithProcessIdentifier: pid];
        let Some(app) = app else { return Default::default() };
        let name: Option<Retained<AnyObject>> = msg_send![&*app, localizedName];
        let bundle: Option<Retained<AnyObject>> = msg_send![&*app, bundleIdentifier];
        (text_of(name.as_deref()), text_of(bundle.as_deref()))
    })
}

fn ax_attr(el: CFTypeRef, name: &str) -> Option<Cf> {
    let mut v: CFTypeRef = std::ptr::null();
    let attr = cfstr(name);
    (unsafe { AXUIElementCopyAttributeValue(el, attr.ptr(), &mut v) } == 0).then(|| Cf::take(v)).flatten()
}

fn ax_app(pid: i32) -> Option<Cf> {
    let app = Cf::take(unsafe { AXUIElementCreateApplication(pid) })?;
    // A program that hangs does not hang the input with it.
    unsafe { AXUIElementSetMessagingTimeout(app.ptr(), 0.5) };
    Some(app)
}

fn ax_number(el: CFTypeRef) -> Option<u32> {
    let mut n = 0u32;
    (unsafe { _AXUIElementGetWindow(el, &mut n) } == 0 && n != 0).then_some(n)
}

/// An app's windows as Accessibility has them: each one's number and whether it is minimized.
fn ax_windows(pid: i32) -> Vec<(u32, bool, Cf)> {
    let Some(app) = ax_app(pid) else { return vec![] };
    let Some(ws) = ax_attr(app.ptr(), "AXWindows") else { return vec![] };
    array_items(ws.ptr())
        .into_iter()
        .filter_map(|w| {
            let n = ax_number(w)?;
            let minimized = ax_attr(w, "AXMinimized").and_then(|m| bool_of(m.ptr())).unwrap_or(false);
            Some((n, minimized, Cf::retain(w)?))
        })
        .collect()
}

/// A window's AX element.
pub fn ax_window(pid: i32, number: u32) -> Option<Cf> {
    ax_windows(pid).into_iter().find(|w| w.0 == number).map(|w| w.2)
}

pub fn minimized(pid: i32, number: u32) -> bool {
    ax_windows(pid).iter().any(|w| w.0 == number && w.1)
}

/// The app that has the keyboard, by Accessibility (its own answer, not one a notification has to bring).
pub fn focused_pid() -> Option<i32> {
    let sys = Cf::take(unsafe { AXUIElementCreateSystemWide() })?;
    unsafe { AXUIElementSetMessagingTimeout(sys.ptr(), 0.5) };
    let app = ax_attr(sys.ptr(), "AXFocusedApplication")?;
    let mut pid = 0;
    (unsafe { AXUIElementGetPid(app.ptr(), &mut pid) } == 0).then_some(pid)
}

/// The window of an app that has its keyboard focus: its number, and whether it is a dialog (an alert or a panel of its own).
pub fn focused_window(pid: i32) -> Option<(u32, bool)> {
    let app = ax_app(pid)?;
    let w = ax_attr(app.ptr(), "AXFocusedWindow")?;
    let n = ax_number(w.ptr())?;
    let sub = ax_attr(w.ptr(), "AXSubrole").and_then(|s| string_of(s.ptr())).unwrap_or_default();
    Some((n, matches!(sub.as_str(), "AXDialog" | "AXSystemDialog" | "AXFloatingWindow")))
}

/// Brings a window to the front: un-minimized, its app frontmost, the window raised and made main.
pub fn raise(pid: i32, number: u32) {
    let Some(app) = ax_app(pid) else { return };
    let yes = cfbool(true);
    unsafe {
        if let Some(w) = ax_window(pid, number) {
            let _ = AXUIElementSetAttributeValue(w.ptr(), cfstr("AXMinimized").ptr(), cfbool(false));
            let _ = AXUIElementSetAttributeValue(w.ptr(), cfstr("AXMain").ptr(), yes);
            let _ = AXUIElementPerformAction(w.ptr(), cfstr("AXRaise").ptr());
        }
        let _ = AXUIElementSetAttributeValue(app.ptr(), cfstr("AXFrontmost").ptr(), yes);
    }
    objc2::rc::autoreleasepool(|_| unsafe {
        if let Some(cls) = class(c"NSRunningApplication") {
            let running: Option<Retained<AnyObject>> = msg_send![cls, runningApplicationWithProcessIdentifier: pid];
            if let Some(running) = running {
                let _: bool = msg_send![&*running, unhide];
                // NSApplicationActivateAllWindows is not wanted: only the target is raised.
                let _: bool = msg_send![&*running, activateWithOptions: 0usize];
            }
        }
    });
}

/// Every window `targets` may offer, front to back: those on screen, and the minimized ones (which Accessibility tells apart from a window on
/// another Space or of a hidden app).
pub fn windows() -> Vec<WindowInfo> {
    let all = list(kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements, kCGNullWindowID);
    let mut minimized: HashMap<i32, Vec<u32>> = HashMap::new();
    for w in all.iter().filter(|w| !w.onscreen && w.layer == 0 && !w.title.trim().is_empty()) {
        minimized.entry(w.pid).or_insert_with(|| ax_windows(w.pid).into_iter().filter(|a| a.1).map(|a| a.0).collect());
    }
    let mut apps: HashMap<i32, (String, String)> = HashMap::new();
    all.into_iter()
        .map(|w| {
            let is_min = minimized.get(&w.pid).is_some_and(|m| m.contains(&w.number));
            let (name, bundle) = apps.entry(w.pid).or_insert_with(|| app_of(w.pid)).clone();
            WindowInfo {
                hwnd: u64::from(w.number),
                pid: w.pid as u32,
                title: w.title,
                class: String::new(),
                exe: if name.is_empty() { w.owner } else { name },
                bundle,
                rect: w.bounds.rect(),
                visible: w.onscreen || is_min,
                cloaked: false,
                // Menus, the Dock, panels above windows: not windows a person picks.
                tool: w.layer != 0 || w.alpha <= 0.0,
                owned: false,
                minimized: is_min,
            }
        })
        .collect()
}

/// What `targets` needs of one window (and `start` checks again).
pub fn info(number: u32) -> Option<WindowInfo> {
    let w = window(number)?;
    let (name, bundle) = app_of(w.pid);
    let is_min = !w.onscreen && minimized(w.pid, number);
    Some(WindowInfo {
        hwnd: u64::from(w.number),
        pid: w.pid as u32,
        title: w.title,
        class: String::new(),
        exe: if name.is_empty() { w.owner } else { name },
        bundle,
        rect: w.bounds.rect(),
        visible: w.onscreen || is_min,
        cloaked: false,
        tool: w.layer != 0 || w.alpha <= 0.0,
        owned: false,
        minimized: is_min,
    })
}

/// The system cursor's shapes, told apart by their hot spot and size (`currentSystemCursor` is a new object each time).
fn shapes() -> &'static [(CursorKey, Shape)] {
    static SHAPES: std::sync::OnceLock<Vec<(CursorKey, Shape)>> = std::sync::OnceLock::new();
    SHAPES.get_or_init(|| {
        let Some(cls) = class(c"NSCursor") else { return vec![] };
        let pick = |c: Option<Retained<AnyObject>>| c.and_then(|c| unsafe { key(&c) });
        objc2::rc::autoreleasepool(|_| unsafe {
            let mut out = Vec::new();
            let known: [(Option<Retained<AnyObject>>, Shape); 7] = [
                (msg_send![cls, IBeamCursor], Shape::IBeam),
                (msg_send![cls, pointingHandCursor], Shape::Hand),
                (msg_send![cls, crosshairCursor], Shape::Cross),
                (msg_send![cls, resizeLeftRightCursor], Shape::SizeWe),
                (msg_send![cls, resizeUpDownCursor], Shape::SizeNs),
                (msg_send![cls, operationNotAllowedCursor], Shape::No),
                (msg_send![cls, arrowCursor], Shape::Arrow),
            ];
            for (c, shape) in known {
                if let Some(k) = pick(c) {
                    out.push((k, shape));
                }
            }
            out
        })
    })
}

/// A cursor's hot spot and image size.
type CursorKey = (f64, f64, f64, f64);

unsafe fn key(c: &AnyObject) -> Option<CursorKey> {
    let hot: CGPoint = msg_send![c, hotSpot];
    let image: Option<Retained<AnyObject>> = msg_send![c, image];
    let size: CGSize = msg_send![&*image?, size];
    Some((hot.x, hot.y, size.width, size.height))
}

/// The cursor: shape, whether it shows, where (global points). The shape is AppKit's, read on the main thread (NSCursor and NSImage are not
/// for other threads).
pub fn cursor() -> (Shape, bool, i32, i32) {
    let at = unsafe {
        let ev = CGEventCreate(std::ptr::null_mut());
        if ev.is_null() {
            return (Shape::Arrow, false, 0, 0);
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev as CFTypeRef);
        p
    };
    let shape = on_main(|| {
        objc2::rc::autoreleasepool(|_| unsafe {
            let Some(cls) = class(c"NSCursor") else { return Shape::Arrow };
            let now: Option<Retained<AnyObject>> = msg_send![cls, currentSystemCursor];
            let k = now.and_then(|c| key(&c));
            k.and_then(|k| shapes().iter().find(|(s, _)| *s == k).map(|(_, shape)| *shape)).unwrap_or(Shape::Arrow)
        })
    });
    (shape, true, at.x.floor() as i32, at.y.floor() as i32)
}
