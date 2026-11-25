//! What the Mac shows while a phone sees it: an orange border around what is shared, with a badge naming the phone and the stop hotkey. The
//! window sits at the screen saver's level on every Space, lets every click through and is never captured: the capture's content filter leaves
//! out this helper's own windows (`capture_sck`), which recent macOS does for ScreenCaptureKit where `sharingType` alone would not.
//!
//! The stop hotkey is Carbon's `RegisterEventHotKey` (it needs no Accessibility permission), delivered on the main thread's loop to the handler
//! installed there once (`install_hotkey_handler`). AppKit and Carbon are main-thread APIs: everything here runs there (`ffi::on_main`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::msg_send;

use super::ffi::{
    class, nsstring, on_main, on_main_later, CGColorCreateSRGB, CGColorRelease, CGDisplayBounds, CGMainDisplayID, CGRect, EventHotKeyID, EventTypeSpec, GetApplicationEventTarget, InstallEventHandler,
    OSStatus, RegisterEventHotKey, UnregisterEventHotKey,
};
use crate::geom::Rect;
use crate::input::{mac_hotkey, Hotkey};

const THICK: f64 = 4.0;
const BADGE_W: f64 = 420.0;
const BADGE_H: f64 = 22.0;
/// `kCGScreenSaverWindowLevel`.
const LEVEL: isize = 1000;
/// `canJoinAllSpaces | stationary | ignoresCycle | fullScreenAuxiliary`.
const BEHAVIOR: usize = 1 | (1 << 4) | (1 << 6) | (1 << 8);
const ORANGE: (f64, f64, f64) = (1.0, 140.0 / 255.0, 0.0);
const DARK: (f64, f64, f64) = (0x14 as f64 / 255.0, 0x18 as f64 / 255.0, 0x23 as f64 / 255.0);

/// Where the hotkey goes while a stream holds it (one stream at a time).
static HOTKEY: Mutex<Option<Sender<()>>> = Mutex::new(None);
/// Carbon took the hotkey handler: without it a registered hotkey would do nothing, so no stream starts.
static HANDLER: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_hotkey(_call: *mut c_void, _event: *mut c_void, _data: *mut c_void) -> OSStatus {
    if let Ok(h) = HOTKEY.lock() {
        if let Some(tx) = h.as_ref() {
            let _ = tx.send(());
        }
    }
    0
}

/// Once, on the main thread before its loop: Carbon calls `on_hotkey` for every hotkey this app registers (`kEventClassKeyboard`,
/// `kEventHotKeyPressed`).
pub fn install_hotkey_handler() {
    let spec = EventTypeSpec { class: u32::from_be_bytes(*b"keyb"), kind: 5 };
    let installed = unsafe { InstallEventHandler(GetApplicationEventTarget(), on_hotkey, 1, &spec, std::ptr::null_mut(), std::ptr::null_mut()) } == 0;
    HANDLER.store(installed, Ordering::SeqCst);
}

/// The border window's frame around a rectangle of global points (top-left origin, as Core Graphics has it), as AppKit's (bottom-left origin,
/// from the primary display): around a window, outside it; on a whole display, inside its edges, where the stroke can be seen.
fn cocoa(r: Rect, outside: bool) -> CGRect {
    let primary = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    let grow = if outside { THICK } else { 0.0 };
    let (x, y, w, h) = (f64::from(r.x) - grow, f64::from(r.y) - grow, f64::from(r.w) + 2.0 * grow, f64::from(r.h) + 2.0 * grow);
    CGRect::new(x, primary.size.height - (y + h), w, h)
}

fn color(c: (f64, f64, f64)) -> Option<Retained<AnyObject>> {
    unsafe { msg_send![class(c"NSColor")?, colorWithSRGBRed: c.0, green: c.1, blue: c.2, alpha: 1.0f64] }
}

/// The border window, shown. Returns it retained (as an address: it is only ever touched on the main thread).
unsafe fn make_window(rect: Rect, outside: bool, label: &str) -> Option<usize> {
    let frame = cocoa(rect, outside);
    let alloc: Allocated<AnyObject> = msg_send![class(c"NSWindow")?, alloc];
    // Borderless, buffered.
    let w: Retained<AnyObject> = msg_send![alloc, initWithContentRect: frame, styleMask: 0usize, backing: 2usize, defer: false];
    let _: () = msg_send![&*w, setReleasedWhenClosed: false];
    let _: () = msg_send![&*w, setLevel: LEVEL];
    let _: () = msg_send![&*w, setOpaque: false];
    let clear: Option<Retained<AnyObject>> = msg_send![class(c"NSColor")?, clearColor];
    let _: () = msg_send![&*w, setBackgroundColor: clear.as_deref()];
    let _: () = msg_send![&*w, setHasShadow: false];
    let _: () = msg_send![&*w, setIgnoresMouseEvents: true];
    let _: () = msg_send![&*w, setCollectionBehavior: BEHAVIOR];
    // NSWindowSharingNone: kept from other capture APIs too.
    let _: () = msg_send![&*w, setSharingType: 0usize];
    let view: Option<Retained<AnyObject>> = msg_send![&*w, contentView];
    let view = view?;
    let _: () = msg_send![&*view, setWantsLayer: true];
    let layer: Option<Retained<AnyObject>> = msg_send![&*view, layer];
    if let Some(layer) = layer {
        let orange = CGColorCreateSRGB(ORANGE.0, ORANGE.1, ORANGE.2, 1.0);
        let _: () = msg_send![&*layer, setBorderWidth: THICK];
        let _: () = msg_send![&*layer, setBorderColor: orange as *const super::ffi::CGColor];
        CGColorRelease(orange);
    }
    let text = nsstring(label);
    let badge: Option<Retained<AnyObject>> = msg_send![class(c"NSTextField")?, labelWithString: &*text];
    if let Some(badge) = badge {
        let bw = BADGE_W.min(frame.size.width - 2.0 * THICK).max(0.0);
        let _: () = msg_send![&*badge, setFrame: CGRect::new((frame.size.width - bw) / 2.0, frame.size.height - THICK - BADGE_H, bw, BADGE_H)];
        // NSTextAlignmentCenter on macOS.
        let _: () = msg_send![&*badge, setAlignment: 2isize];
        let _: () = msg_send![&*badge, setTextColor: color(ORANGE).as_deref()];
        let _: () = msg_send![&*badge, setDrawsBackground: true];
        let _: () = msg_send![&*badge, setBackgroundColor: color(DARK).as_deref()];
        // Centred at the top whatever the window's size: NSViewMinXMargin | NSViewMaxXMargin | NSViewMinYMargin.
        let _: () = msg_send![&*badge, setAutoresizingMask: 1usize | 4 | 8];
        let _: () = msg_send![&*view, addSubview: &*badge];
    }
    let _: () = msg_send![&*w, orderFrontRegardless];
    Some(Retained::into_raw(w) as usize)
}

pub struct Border {
    window: usize,
    hotkey: usize,
    rect: Mutex<Rect>,
    outside: bool,
}

impl Border {
    /// Shows the border around `rect` (global points; `outside` it for a window, inside a display's edges) with `label` in its badge, and
    /// registers the stop hotkey, which sends on `on_hotkey`. An error (no window, the hotkey held already or its handler missing) means the
    /// screen must not be shared: the Mac would not show it or could not stop it.
    pub fn show(rect: Rect, outside: bool, label: &str, hotkey: Hotkey, on_hotkey: Sender<()>) -> Result<Border, String> {
        let (code, mods) = mac_hotkey(hotkey).ok_or("the stop hotkey in Settings → Mobile is not one a Mac has")?;
        *HOTKEY.lock().map_err(|_| "the stop hotkey cannot be set")? = Some(on_hotkey);
        let label = label.to_string();
        let made = on_main(move || unsafe {
            // Read here: the main thread installs the handler before it runs anything sent to it.
            if !HANDLER.load(Ordering::SeqCst) {
                return Err("the stop hotkey cannot be set on this Mac, so the screen is not shared".to_string());
            }
            let mut hk: *mut c_void = std::ptr::null_mut();
            let id = EventHotKeyID { signature: u32::from_be_bytes(*b"Klpe"), id: 1 };
            if RegisterEventHotKey(code, mods, id, GetApplicationEventTarget(), 0, &mut hk) != 0 || hk.is_null() {
                return Err("another program holds the stop hotkey: pick another in Settings → Mobile".to_string());
            }
            match objc2::rc::autoreleasepool(|_| make_window(rect, outside, &label)) {
                Some(w) => Ok((w, hk as usize)),
                None => {
                    let _ = UnregisterEventHotKey(hk);
                    Err("the sharing border could not be shown".to_string())
                }
            }
        });
        match made {
            Ok((window, hotkey)) => Ok(Border { window, hotkey, rect: Mutex::new(rect), outside }),
            Err(e) => {
                if let Ok(mut h) = HOTKEY.lock() {
                    *h = None;
                }
                Err(e)
            }
        }
    }

    /// The target moved or was resized.
    pub fn place(&self, rect: Rect) {
        let changed = self.rect.lock().map(|mut r| std::mem::replace(&mut *r, rect) != rect).unwrap_or(false);
        if changed {
            let (window, outside) = (self.window, self.outside);
            // Queued on the main thread before the border's own end (`Drop` waits behind it), so the window is still there.
            on_main_later(move || unsafe {
                let w = window as *mut AnyObject;
                let _: () = msg_send![&*w, setFrame: cocoa(rect, outside), display: true];
            });
        }
    }
}

impl Drop for Border {
    fn drop(&mut self) {
        let (window, hotkey) = (self.window, self.hotkey);
        on_main(move || unsafe {
            let _ = UnregisterEventHotKey(hotkey as *mut c_void);
            let w = window as *mut AnyObject;
            let _: () = msg_send![&*w, orderOut: std::ptr::null::<AnyObject>()];
            let _: () = msg_send![&*w, close];
            drop(Retained::from_raw(w));
        });
        if let Ok(mut h) = HOTKEY.lock() {
            *h = None;
        }
    }
}
