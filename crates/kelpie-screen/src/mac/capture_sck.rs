//! ScreenCaptureKit: a display (leaving out this helper's own windows, the sharing border) or one window, as BGRA at its pixel size without
//! the cursor. The newest frame waits in a one-slot mailbox (as with Windows Graphics Capture): one the loop did not take is replaced, and the
//! rectangles it changed (`SCStreamFrameInfo.dirtyRects`) go with the next. Frames that say nothing changed (idle) carry no picture and are
//! not handed over.

use std::ffi::c_void;
use std::sync::mpsc::channel;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, ClassBuilder, Sel};
use objc2::{msg_send, sel};

use super::ffi::*;
use crate::geom::Rect;

/// One picture: a retained `CVPixelBuffer` (IOSurface-backed BGRA) and what it changed, when known.
pub struct Frame {
    pub picture: Cf,
    pub dirty: Option<Vec<Rect>>,
}

#[derive(Default)]
struct Slot {
    frame: Option<Frame>,
    /// The stream stopped by itself (the window closed, the display went, the permission was taken back).
    stopped: bool,
}

#[derive(Default)]
pub struct Mailbox {
    slot: Mutex<Slot>,
    ready: Condvar,
}

impl Mailbox {
    fn put(&self, mut frame: Frame) {
        let Ok(mut slot) = self.slot.lock() else { return };
        // A frame not taken yet is replaced; what it changed still counts.
        if let Some(old) = slot.frame.take() {
            frame.dirty = match (old.dirty, frame.dirty) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                _ => None,
            };
        }
        slot.frame = Some(frame);
        self.ready.notify_one();
    }

    fn stop(&self) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.stopped = true;
            self.ready.notify_one();
        }
    }
}

/// An Objective-C object that may go to another thread (the completion handlers' results).
struct Sendable(Retained<AnyObject>);
unsafe impl Send for Sendable {}

const IVAR: &std::ffi::CStr = c"kelpieMailbox";

fn mailbox_of(this: &AnyObject) -> Option<&Mailbox> {
    let ivar = this.class().instance_variable(IVAR)?;
    let p = unsafe { *ivar.load::<usize>(this) };
    (p != 0).then(|| unsafe { &*(p as *const Mailbox) })
}

extern "C-unwind" fn on_sample(this: &AnyObject, _cmd: Sel, _stream: *mut AnyObject, sb: *mut c_void, kind: isize) {
    // SCStreamOutputTypeScreen.
    if kind != 0 || sb.is_null() {
        return;
    }
    let Some(mailbox) = mailbox_of(this) else { return };
    unsafe {
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sb, 0);
        let info = array_items(attachments).first().copied().unwrap_or(std::ptr::null());
        // SCFrameStatusComplete: a new picture. Idle, blank, suspended, started and stopped frames carry none.
        if i64_of(dict_get(info, SCStreamFrameInfoStatus)) != Some(0) {
            return;
        }
        let Some(picture) = Cf::retain(CMSampleBufferGetImageBuffer(sb) as CFTypeRef) else { return };
        let rects = dict_get(info, SCStreamFrameInfoDirtyRects);
        let dirty = (!rects.is_null()).then(|| array_items(rects).into_iter().filter_map(rect_of).map(|r| r.rect()).collect());
        mailbox.put(Frame { picture, dirty });
    }
}

extern "C-unwind" fn on_stop(this: &AnyObject, _cmd: Sel, _stream: *mut AnyObject, _error: *mut AnyObject) {
    if let Some(m) = mailbox_of(this) {
        m.stop();
    }
}

/// The class that takes the stream's frames and its end (`SCStreamOutput`, `SCStreamDelegate`).
fn output_class() -> Option<&'static AnyClass> {
    static CLASS: OnceLock<Option<&'static AnyClass>> = OnceLock::new();
    *CLASS.get_or_init(|| {
        let mut b = ClassBuilder::new(c"KelpieScreenOutput", class(c"NSObject")?)?;
        b.add_ivar::<usize>(IVAR);
        unsafe {
            b.add_method(sel!(stream:didOutputSampleBuffer:ofType:), on_sample as extern "C-unwind" fn(_, _, _, _, _));
            b.add_method(sel!(stream:didStopWithError:), on_stop as extern "C-unwind" fn(_, _, _, _));
        }
        for p in [c"SCStreamOutput", c"SCStreamDelegate"] {
            if let Some(p) = AnyProtocol::get(p) {
                b.add_protocol(p);
            }
        }
        Some(b.register())
    })
}

fn description(error: *mut AnyObject) -> String {
    if error.is_null() {
        return "unknown error".into();
    }
    unsafe {
        let d: Option<Retained<AnyObject>> = msg_send![&*error, localizedDescription];
        text_of(d.as_deref())
    }
}

/// What may be captured now (`SCShareableContent`).
fn shareable() -> Result<Retained<AnyObject>, String> {
    let (tx, rx) = channel::<Result<Sendable, String>>();
    let block = RcBlock::new(move |content: *mut AnyObject, error: *mut AnyObject| {
        let got = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(Sendable(c)),
            None => Err(description(error)),
        };
        let _ = tx.send(got);
    });
    unsafe {
        let cls = class(c"SCShareableContent").ok_or("ScreenCaptureKit is not available on this Mac")?;
        let _: () = msg_send![cls, getShareableContentExcludingDesktopWindows: false, onScreenWindowsOnly: false, completionHandler: &*block];
    }
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(c)) => Ok(c.0),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the Mac did not say what can be captured".into()),
    }
}

/// Waits for a completion handler that takes only an error.
fn wait_done(start: impl FnOnce(&block2::Block<dyn Fn(*mut AnyObject)>)) -> Result<(), String> {
    let (tx, rx) = channel::<Result<(), String>>();
    let block = RcBlock::new(move |error: *mut AnyObject| {
        let _ = tx.send(if error.is_null() { Ok(()) } else { Err(description(error)) });
    });
    start(&block);
    rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err("the capture did not answer".into()))
}

pub enum Grabbed {
    Frame(Frame),
    /// Nothing new yet.
    Idle,
    /// The stream stopped by itself (the window closed, the display went, the permission was taken back).
    Stopped,
}

pub enum Source {
    Display(u32),
    Window(u32),
}

pub struct Capture {
    stream: Retained<AnyObject>,
    config: Retained<AnyObject>,
    _output: Retained<AnyObject>,
    queue: DispatchQueue,
    mailbox: Arc<Mailbox>,
}

/// The plain reason a capture cannot start without the Screen Recording permission (the phone shows it).
pub const NO_PERMISSION: &str = "no_screen_permission: The Mac has not allowed Kelpie to record the screen. Allow it in System Settings → Privacy & Security → Screen Recording, on the Mac";

impl Capture {
    /// Starts capturing `source` at `size` pixels, `fps` at most.
    pub fn start(source: Source, size: (u32, u32), fps: u32) -> Result<Capture, String> {
        if !unsafe { CGPreflightScreenCaptureAccess() } {
            return Err(NO_PERMISSION.into());
        }
        objc2::rc::autoreleasepool(|_| unsafe { Capture::open(source, size, fps) })
    }

    unsafe fn open(source: Source, size: (u32, u32), fps: u32) -> Result<Capture, String> {
        let content = shareable()?;
        let array = class(c"NSArray").ok_or("no Foundation")?;
        let alloc: Allocated<AnyObject> = msg_send![class(c"SCContentFilter").ok_or("no ScreenCaptureKit")?, alloc];
        let filter: Option<Retained<AnyObject>> = match source {
            Source::Display(id) => {
                let displays: Option<Retained<AnyObject>> = msg_send![&*content, displays];
                let display = ns_items(displays.as_deref()).into_iter().find(|d| {
                    let n: u32 = msg_send![&**d, displayID];
                    n == id
                });
                let display = display.ok_or("that screen is no longer connected")?;
                // This helper's own windows (the border) are never captured.
                let apps: Option<Retained<AnyObject>> = msg_send![&*content, applications];
                let me = std::process::id() as i32;
                let own: Vec<Retained<AnyObject>> = ns_items(apps.as_deref())
                    .into_iter()
                    .filter(|a| {
                        let pid: i32 = msg_send![&**a, processID];
                        pid == me
                    })
                    .collect();
                let excluded: Retained<AnyObject> = match own.first() {
                    Some(a) => msg_send![array, arrayWithObject: &**a],
                    None => msg_send![array, array],
                };
                let none: Retained<AnyObject> = msg_send![array, array];
                msg_send![alloc, initWithDisplay: &*display, excludingApplications: &*excluded, exceptingWindows: &*none]
            }
            Source::Window(number) => {
                let windows: Option<Retained<AnyObject>> = msg_send![&*content, windows];
                let window = ns_items(windows.as_deref()).into_iter().find(|w| {
                    let n: u32 = msg_send![&**w, windowID];
                    n == number
                });
                let window = window.ok_or("that window cannot be captured")?;
                msg_send![alloc, initWithDesktopIndependentWindow: &*window]
            }
        };
        let filter = filter.ok_or("the capture could not be set up")?;
        let config: Retained<AnyObject> = msg_send![class(c"SCStreamConfiguration").ok_or("no ScreenCaptureKit")?, new];
        let _: () = msg_send![&*config, setWidth: size.0 as usize];
        let _: () = msg_send![&*config, setHeight: size.1 as usize];
        let _: () = msg_send![&*config, setPixelFormat: kCVPixelFormatType_32BGRA];
        let _: () = msg_send![&*config, setShowsCursor: false];
        let _: () = msg_send![&*config, setMinimumFrameInterval: CMTime::new(1, fps.max(1) as i32)];
        let _: () = msg_send![&*config, setQueueDepth: 4isize];

        let mailbox = Arc::new(Mailbox::default());
        let cls = output_class().ok_or("the capture could not be set up")?;
        let output: Retained<AnyObject> = msg_send![cls, new];
        if let Some(ivar) = cls.instance_variable(IVAR) {
            // The output keeps a reference for as long as it may be called: it is never given back (a few bytes a stream).
            *ivar.load_ptr::<usize>(&output) = Arc::into_raw(mailbox.clone()) as usize;
        }
        let alloc: Allocated<AnyObject> = msg_send![class(c"SCStream").ok_or("no ScreenCaptureKit")?, alloc];
        let stream: Option<Retained<AnyObject>> = msg_send![alloc, initWithFilter: &*filter, configuration: &*config, delegate: &*output];
        let stream = stream.ok_or("the capture could not be set up")?;
        let queue = dispatch_queue_create(c"dev.kelpie.screen.capture".as_ptr(), std::ptr::null());
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let added: bool = msg_send![&*stream, addStreamOutput: &*output, type: 0isize, sampleHandlerQueue: queue as *mut AnyObject, error: &mut error];
        let capture = Capture { stream, config, _output: output, queue, mailbox };
        if !added {
            return Err(format!("the capture could not be set up: {}", description(error)));
        }
        let stream = &capture.stream;
        wait_done(|b| {
            let _: () = msg_send![&**stream, startCaptureWithCompletionHandler: b];
        })
        .map_err(|e| format!("the screen cannot be captured: {e}"))?;
        Ok(capture)
    }

    /// The newest frame, waiting at most `wait` for one.
    pub fn grab(&self, wait: Duration) -> Grabbed {
        let Ok(mut slot) = self.mailbox.slot.lock() else { return Grabbed::Stopped };
        if slot.frame.is_none() && !slot.stopped && !wait.is_zero() {
            slot = match self.mailbox.ready.wait_timeout(slot, wait) {
                Ok((s, _)) => s,
                Err(_) => return Grabbed::Stopped,
            };
        }
        if slot.stopped {
            return Grabbed::Stopped;
        }
        slot.frame.take().map_or(Grabbed::Idle, Grabbed::Frame)
    }

    /// The target is now `size` pixels (a window was resized).
    pub fn resize(&self, size: (u32, u32)) {
        unsafe {
            let _: () = msg_send![&*self.config, setWidth: size.0 as usize];
            let _: () = msg_send![&*self.config, setHeight: size.1 as usize];
            let block = RcBlock::new(|_error: *mut AnyObject| {});
            let _: () = msg_send![&*self.stream, updateConfiguration: &*self.config, completionHandler: &*block];
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let stream = &self.stream;
        let _ = wait_done(|b| unsafe {
            let _: () = msg_send![&**stream, stopCaptureWithCompletionHandler: b];
        });
        if let Ok(mut slot) = self.mailbox.slot.lock() {
            slot.frame = None;
        }
        unsafe { dispatch_release(self.queue) };
    }
}
