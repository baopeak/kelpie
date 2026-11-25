//! The C side of the Mac's frameworks this helper calls, declared by hand (the few dozen functions and constants it uses), and the small owned
//! wrappers around Core Foundation objects. Objective-C classes (ScreenCaptureKit, AppKit) are reached by name through `objc2`'s messaging;
//! the structures they take are declared here with their type encodings, which `objc2` checks in a debug build.

#![allow(non_upper_case_globals, non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_void, CStr};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, Encode, Encoding, RefEncode};

pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDictionaryRef = *const c_void;
pub type CFArrayRef = *const c_void;
pub type CFNumberRef = *const c_void;
pub type CFBooleanRef = *const c_void;
pub type CFIndex = isize;
pub type OSStatus = i32;
pub type Boolean = u8;
pub type CGEventRef = *mut c_void;
pub type CVPixelBufferRef = *mut c_void;
pub type CMSampleBufferRef = *mut c_void;
pub type VTSessionRef = *mut c_void;
pub type AXUIElementRef = *const c_void;
pub type DispatchQueue = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

unsafe impl Encode for CGPoint {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl RefEncode for CGPoint {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl RefEncode for CGSize {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}
unsafe impl Encode for CGRect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
}
unsafe impl RefEncode for CGRect {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

impl CGRect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
    }

    /// In whole points (the rectangle around it).
    pub fn rect(&self) -> crate::geom::Rect {
        let (x, y) = (self.origin.x.floor(), self.origin.y.floor());
        let (r, b) = ((self.origin.x + self.size.width).ceil(), (self.origin.y + self.size.height).ceil());
        crate::geom::Rect::from_edges(x as i32, y as i32, r as i32, b as i32)
    }
}

/// `CMTime`, an anonymous struct in its header (`{?=qiIq}`).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct("?", &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING]);
}

impl CMTime {
    pub const INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };

    pub fn new(value: i64, timescale: i32) -> CMTime {
        CMTime { value, timescale, flags: 1, epoch: 0 }
    }
}

/// `CGColorRef`, as AppKit's layer takes it (`^{CGColor=}`).
#[repr(C)]
pub struct CGColor {
    _p: [u8; 0],
}

unsafe impl RefEncode for CGColor {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColor", &[]));
}

#[repr(C)]
pub struct EventTypeSpec {
    pub class: u32,
    pub kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventHotKeyID {
    pub signature: u32,
    pub id: u32,
}

pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
pub const kCFNumberSInt32Type: CFIndex = 3;
pub const kCFNumberSInt64Type: CFIndex = 4;
pub const kCFNumberFloat64Type: CFIndex = 6;
pub const kCVPixelFormatType_32BGRA: u32 = 0x4247_5241;
pub const kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: u32 = 0x3432_3076;
pub const kCVPixelBufferLock_ReadOnly: u64 = 1;
pub const kCMVideoCodecType_H264: u32 = 0x6176_6331;
pub const kVTEncodeInfo_FrameDropped: u32 = 2;
pub const kCGHIDEventTap: u32 = 0;
pub const kCGWindowListOptionAll: u32 = 0;
pub const kCGWindowListOptionOnScreenOnly: u32 = 1;
pub const kCGWindowListExcludeDesktopElements: u32 = 16;
pub const kCGNullWindowID: u32 = 0;
pub const kCGEventSourceStateHIDSystemState: i32 = 1;
pub const kCGMouseEventClickState: u32 = 1;
pub const kCGScrollEventUnitPixel: u32 = 0;
pub const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub static kCFBooleanTrue: CFBooleanRef;
    pub static kCFBooleanFalse: CFBooleanRef;
    pub static kCFTypeDictionaryKeyCallBacks: u8;
    pub static kCFTypeDictionaryValueCallBacks: u8;
    pub static kCFTypeArrayCallBacks: u8;
    pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFNumberGetTypeID() -> usize;
    pub fn CFBooleanGetTypeID() -> usize;
    pub fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> Boolean;
    pub fn CFStringCreateWithBytes(alloc: CFTypeRef, bytes: *const u8, len: CFIndex, encoding: u32, external: Boolean) -> CFStringRef;
    pub fn CFStringGetLength(s: CFStringRef) -> CFIndex;
    pub fn CFStringGetMaximumSizeForEncoding(len: CFIndex, encoding: u32) -> CFIndex;
    pub fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> Boolean;
    pub fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFNumberRef;
    pub fn CFNumberGetValue(n: CFNumberRef, kind: CFIndex, value: *mut c_void) -> Boolean;
    pub fn CFBooleanGetValue(b: CFBooleanRef) -> Boolean;
    pub fn CFDictionaryCreate(alloc: CFTypeRef, keys: *const CFTypeRef, values: *const CFTypeRef, n: CFIndex, kcb: *const c_void, vcb: *const c_void) -> CFDictionaryRef;
    pub fn CFDictionaryGetValue(d: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    pub fn CFArrayCreate(alloc: CFTypeRef, values: *const CFTypeRef, n: CFIndex, cb: *const c_void) -> CFArrayRef;
    pub fn CFArrayGetCount(a: CFArrayRef) -> CFIndex;
    pub fn CFArrayGetValueAtIndex(a: CFArrayRef, i: CFIndex) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    pub static kCGWindowNumber: CFStringRef;
    pub static kCGWindowOwnerPID: CFStringRef;
    pub static kCGWindowOwnerName: CFStringRef;
    pub static kCGWindowName: CFStringRef;
    pub static kCGWindowLayer: CFStringRef;
    pub static kCGWindowBounds: CFStringRef;
    pub static kCGWindowIsOnscreen: CFStringRef;
    pub static kCGWindowAlpha: CFStringRef;
    pub fn CGMainDisplayID() -> u32;
    pub fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    pub fn CGDisplayBounds(display: u32) -> CGRect;
    pub fn CGDisplayCopyDisplayMode(display: u32) -> *mut c_void;
    pub fn CGDisplayModeGetPixelWidth(mode: *mut c_void) -> usize;
    pub fn CGDisplayModeGetPixelHeight(mode: *mut c_void) -> usize;
    pub fn CGDisplayModeRelease(mode: *mut c_void);
    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    pub fn CGRectMakeWithDictionaryRepresentation(dict: CFDictionaryRef, rect: *mut CGRect) -> bool;
    pub fn CGEventSourceCreate(state: i32) -> *mut c_void;
    pub fn CGEventCreate(source: *mut c_void) -> CGEventRef;
    pub fn CGEventCreateMouseEvent(source: *mut c_void, kind: u32, at: CGPoint, button: u32) -> CGEventRef;
    pub fn CGEventCreateKeyboardEvent(source: *mut c_void, key: u16, down: bool) -> CGEventRef;
    pub fn CGEventCreateScrollWheelEvent2(source: *mut c_void, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> CGEventRef;
    pub fn CGEventKeyboardSetUnicodeString(ev: CGEventRef, len: usize, chars: *const u16);
    pub fn CGEventSetFlags(ev: CGEventRef, flags: u64);
    pub fn CGEventSetIntegerValueField(ev: CGEventRef, field: u32, value: i64);
    pub fn CGEventSetLocation(ev: CGEventRef, at: CGPoint);
    pub fn CGEventGetLocation(ev: CGEventRef) -> CGPoint;
    pub fn CGEventPost(tap: u32, ev: CGEventRef);
    pub fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> *mut CGColor;
    pub fn CGColorRelease(c: *mut CGColor);
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    pub static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    pub static kCVPixelBufferWidthKey: CFStringRef;
    pub static kCVPixelBufferHeightKey: CFStringRef;
    pub static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    pub static kCVImageBufferYCbCrMatrixKey: CFStringRef;
    pub static kCVImageBufferYCbCrMatrix_ITU_R_709_2: CFStringRef;
    pub static kCVImageBufferColorPrimariesKey: CFStringRef;
    pub static kCVImageBufferColorPrimaries_ITU_R_709_2: CFStringRef;
    pub static kCVImageBufferTransferFunctionKey: CFStringRef;
    pub static kCVImageBufferTransferFunction_ITU_R_709_2: CFStringRef;
    pub fn CVPixelBufferGetWidth(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetHeight(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferGetPixelFormatType(pb: CVPixelBufferRef) -> u32;
    pub fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    pub fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    pub fn CVPixelBufferGetBaseAddress(pb: CVPixelBufferRef) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(pb: CVPixelBufferRef) -> usize;
    pub fn CVPixelBufferPoolCreatePixelBuffer(alloc: CFTypeRef, pool: *mut c_void, out: *mut CVPixelBufferRef) -> i32;
    pub fn CVBufferSetAttachment(buf: CVPixelBufferRef, key: CFStringRef, value: CFTypeRef, mode: u32);
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub static kCMSampleAttachmentKey_NotSync: CFStringRef;
    pub fn CMSampleBufferGetImageBuffer(sb: CMSampleBufferRef) -> CVPixelBufferRef;
    pub fn CMSampleBufferGetSampleAttachmentsArray(sb: CMSampleBufferRef, create: Boolean) -> CFArrayRef;
    pub fn CMSampleBufferGetDataBuffer(sb: CMSampleBufferRef) -> *mut c_void;
    pub fn CMSampleBufferGetFormatDescription(sb: CMSampleBufferRef) -> *mut c_void;
    pub fn CMBlockBufferGetDataLength(bb: *mut c_void) -> usize;
    pub fn CMBlockBufferCopyDataBytes(bb: *mut c_void, offset: usize, len: usize, dst: *mut c_void) -> OSStatus;
    pub fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fd: *mut c_void, index: usize, ptr: *mut *const u8, size: *mut usize, count: *mut usize, nal_header_len: *mut i32) -> OSStatus;
}

pub type VTOutputCallback = unsafe extern "C" fn(refcon: *mut c_void, frame: *mut c_void, status: OSStatus, flags: u32, sb: CMSampleBufferRef);

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    pub static kVTCompressionPropertyKey_RealTime: CFStringRef;
    pub static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    pub static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    pub static kVTCompressionPropertyKey_DataRateLimits: CFStringRef;
    pub static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    pub static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    pub static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    pub static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: CFStringRef;
    pub static kVTCompressionPropertyKey_ColorPrimaries: CFStringRef;
    pub static kVTCompressionPropertyKey_TransferFunction: CFStringRef;
    pub static kVTCompressionPropertyKey_YCbCrMatrix: CFStringRef;
    pub static kVTProfileLevel_H264_High_AutoLevel: CFStringRef;
    pub static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    pub static kVTVideoEncoderSpecification_EnableLowLatencyRateControl: CFStringRef;
    pub static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;
    pub fn VTCompressionSessionCreate(
        alloc: CFTypeRef,
        width: i32,
        height: i32,
        codec: u32,
        spec: CFDictionaryRef,
        source_attrs: CFDictionaryRef,
        data_alloc: CFTypeRef,
        callback: VTOutputCallback,
        refcon: *mut c_void,
        out: *mut VTSessionRef,
    ) -> OSStatus;
    pub fn VTCompressionSessionPrepareToEncodeFrames(s: VTSessionRef) -> OSStatus;
    pub fn VTCompressionSessionGetPixelBufferPool(s: VTSessionRef) -> *mut c_void;
    pub fn VTCompressionSessionEncodeFrame(s: VTSessionRef, image: CVPixelBufferRef, pts: CMTime, duration: CMTime, props: CFDictionaryRef, frame: *mut c_void, flags: *mut u32) -> OSStatus;
    pub fn VTCompressionSessionCompleteFrames(s: VTSessionRef, until: CMTime) -> OSStatus;
    pub fn VTCompressionSessionInvalidate(s: VTSessionRef);
    pub fn VTSessionSetProperty(s: VTSessionRef, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    pub fn VTSessionCopyProperty(s: VTSessionRef, key: CFStringRef, alloc: CFTypeRef, out: *mut CFTypeRef) -> OSStatus;
    pub fn VTPixelTransferSessionCreate(alloc: CFTypeRef, out: *mut VTSessionRef) -> OSStatus;
    pub fn VTPixelTransferSessionTransferImage(s: VTSessionRef, src: CVPixelBufferRef, dst: CVPixelBufferRef) -> OSStatus;
    pub fn VTPixelTransferSessionInvalidate(s: VTSessionRef);
}

#[link(name = "ScreenCaptureKit", kind = "framework")]
extern "C" {
    pub static SCStreamFrameInfoStatus: CFStringRef;
    pub static SCStreamFrameInfoDirtyRects: CFStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    pub fn AXIsProcessTrusted() -> Boolean;
    pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
    pub fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, out: *mut CFTypeRef) -> i32;
    pub fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: CFTypeRef) -> i32;
    pub fn AXUIElementPerformAction(el: AXUIElementRef, action: CFStringRef) -> i32;
    pub fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> i32;
    /// Private but long stable (used by every window manager on the Mac): the window number of an AX window.
    pub fn _AXUIElementGetWindow(el: AXUIElementRef, out: *mut u32) -> i32;
}

pub type EventHandler = unsafe extern "C" fn(call: *mut c_void, event: *mut c_void, data: *mut c_void) -> OSStatus;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    pub fn GetApplicationEventTarget() -> *mut c_void;
    pub fn InstallEventHandler(target: *mut c_void, handler: EventHandler, n: usize, types: *const EventTypeSpec, data: *mut c_void, out: *mut *mut c_void) -> OSStatus;
    pub fn RegisterEventHotKey(code: u32, mods: u32, id: EventHotKeyID, target: *mut c_void, options: u32, out: *mut *mut c_void) -> OSStatus;
    pub fn UnregisterEventHotKey(hotkey: *mut c_void) -> OSStatus;
    pub fn IsSecureEventInputEnabled() -> Boolean;
}

extern "C" {
    static _dispatch_main_q: u8;
    pub fn dispatch_sync_f(queue: DispatchQueue, ctx: *mut c_void, work: unsafe extern "C" fn(*mut c_void));
    pub fn dispatch_async_f(queue: DispatchQueue, ctx: *mut c_void, work: unsafe extern "C" fn(*mut c_void));
    pub fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> DispatchQueue;
    pub fn dispatch_release(obj: *mut c_void);
    pub fn pthread_main_np() -> i32;
    pub fn pthread_set_qos_class_self_np(qos: u32, relative: i32) -> i32;
}

pub fn main_queue() -> DispatchQueue {
    unsafe { &_dispatch_main_q as *const u8 as DispatchQueue }
}

/// An owned Core Foundation object: released when it goes, retained when cloned.
pub struct Cf(pub CFTypeRef);

unsafe impl Send for Cf {}
unsafe impl Sync for Cf {}

impl Cf {
    /// Takes a +1 reference (a `Create` or `Copy` function's); None for null.
    pub fn take(p: CFTypeRef) -> Option<Cf> {
        (!p.is_null()).then_some(Cf(p))
    }

    /// Retains a +0 reference (a `Get` function's).
    pub fn retain(p: CFTypeRef) -> Option<Cf> {
        (!p.is_null()).then(|| Cf(unsafe { CFRetain(p) }))
    }

    pub fn ptr(&self) -> CFTypeRef {
        self.0
    }
}

impl Clone for Cf {
    fn clone(&self) -> Cf {
        Cf(unsafe { CFRetain(self.0) })
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}

pub fn cfstr(s: &str) -> Cf {
    Cf(unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0) })
}

pub fn cfnum_i64(v: i64) -> Cf {
    Cf(unsafe { CFNumberCreate(std::ptr::null(), kCFNumberSInt64Type, &v as *const i64 as *const c_void) })
}

pub fn cfnum_f64(v: f64) -> Cf {
    Cf(unsafe { CFNumberCreate(std::ptr::null(), kCFNumberFloat64Type, &v as *const f64 as *const c_void) })
}

pub fn cfbool(v: bool) -> CFTypeRef {
    unsafe { if v { kCFBooleanTrue } else { kCFBooleanFalse } }
}

pub fn cfdict(pairs: &[(CFTypeRef, CFTypeRef)]) -> Cf {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let values: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    unsafe {
        Cf(CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            pairs.len() as CFIndex,
            &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
        ))
    }
}

pub fn cfarray(values: &[CFTypeRef]) -> Cf {
    unsafe { Cf(CFArrayCreate(std::ptr::null(), values.as_ptr(), values.len() as CFIndex, &kCFTypeArrayCallBacks as *const u8 as *const c_void)) }
}

/// The items of a `CFArray` (+0, valid while the array is).
pub fn array_items(a: CFArrayRef) -> Vec<CFTypeRef> {
    if a.is_null() {
        return vec![];
    }
    unsafe { (0..CFArrayGetCount(a)).map(|i| CFArrayGetValueAtIndex(a, i)).collect() }
}

pub fn dict_get(d: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef {
    if d.is_null() {
        return std::ptr::null();
    }
    unsafe { CFDictionaryGetValue(d, key) }
}

pub fn string_of(s: CFTypeRef) -> Option<String> {
    unsafe {
        if s.is_null() || CFGetTypeID(s) != CFStringGetTypeID() {
            return None;
        }
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), kCFStringEncodingUTF8) + 1;
        let mut buf = vec![0u8; size.max(1) as usize];
        if CFStringGetCString(s, buf.as_mut_ptr() as *mut c_char, size, kCFStringEncodingUTF8) == 0 {
            return None;
        }
        Some(CStr::from_ptr(buf.as_ptr() as *const c_char).to_string_lossy().into_owned())
    }
}

pub fn i64_of(n: CFTypeRef) -> Option<i64> {
    unsafe {
        if n.is_null() || CFGetTypeID(n) != CFNumberGetTypeID() {
            return None;
        }
        let mut v = 0i64;
        (CFNumberGetValue(n, kCFNumberSInt64Type, &mut v as *mut i64 as *mut c_void) != 0).then_some(v)
    }
}

pub fn f64_of(n: CFTypeRef) -> Option<f64> {
    unsafe {
        if n.is_null() || CFGetTypeID(n) != CFNumberGetTypeID() {
            return None;
        }
        let mut v = 0f64;
        (CFNumberGetValue(n, kCFNumberFloat64Type, &mut v as *mut f64 as *mut c_void) != 0).then_some(v)
    }
}

pub fn bool_of(b: CFTypeRef) -> Option<bool> {
    unsafe {
        if b.is_null() {
            return None;
        }
        if CFGetTypeID(b) == CFBooleanGetTypeID() {
            return Some(CFBooleanGetValue(b) != 0);
        }
        i64_of(b).map(|v| v != 0)
    }
}

pub fn rect_of(d: CFTypeRef) -> Option<CGRect> {
    let mut r = CGRect::default();
    (!d.is_null() && unsafe { CGRectMakeWithDictionaryRepresentation(d, &mut r) }).then_some(r)
}

/// An Objective-C class of a system framework, by name.
pub fn class(name: &CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

/// An `NSString` (toll-free bridged with `CFString`).
pub fn nsstring(s: &str) -> Retained<AnyObject> {
    let cf = cfstr(s);
    let p = cf.0 as *mut AnyObject;
    std::mem::forget(cf);
    unsafe { Retained::from_raw(p).expect("CFStringCreateWithBytes") }
}

/// An `NSString`'s text.
pub fn text_of(obj: Option<&AnyObject>) -> String {
    obj.and_then(|o| string_of(o as *const AnyObject as CFTypeRef)).unwrap_or_default()
}

/// `[NSArray count]` and `objectAtIndex:`.
pub fn ns_items(array: Option<&AnyObject>) -> Vec<Retained<AnyObject>> {
    let Some(a) = array else { return vec![] };
    unsafe {
        let n: usize = msg_send![a, count];
        (0..n).filter_map(|i| -> Option<Retained<AnyObject>> { msg_send![a, objectAtIndex: i] }).collect()
    }
}

/// A Core Foundation object as the Objective-C object it is.
pub fn as_object(cf: &Cf) -> &AnyObject {
    unsafe { &*(cf.0 as *const AnyObject) }
}

/// Runs `f` on the main thread (where AppKit and Carbon's events live) and waits for it. On the main thread already, it runs at once.
pub fn on_main<R: Send, F: FnOnce() -> R + Send>(f: F) -> R {
    if unsafe { pthread_main_np() } != 0 {
        return f();
    }
    struct Job<F, R> {
        f: Option<F>,
        out: Option<R>,
    }
    unsafe extern "C" fn trampoline<F: FnOnce() -> R, R>(ctx: *mut c_void) {
        let job = &mut *(ctx as *mut Job<F, R>);
        if let Some(f) = job.f.take() {
            // A panic must not cross into the main queue's C frames.
            job.out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok();
        }
    }
    let mut job = Job { f: Some(f), out: None };
    unsafe { dispatch_sync_f(main_queue(), &mut job as *mut Job<F, R> as *mut c_void, trampoline::<F, R>) };
    job.out.expect("the main thread's work panicked")
}

/// Runs `f` on the main thread later, without waiting.
pub fn on_main_later(f: impl FnOnce() + Send + 'static) {
    unsafe extern "C" fn trampoline(ctx: *mut c_void) {
        let f = Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    }
    let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    unsafe { dispatch_async_f(main_queue(), Box::into_raw(boxed) as *mut c_void, trampoline) };
}
