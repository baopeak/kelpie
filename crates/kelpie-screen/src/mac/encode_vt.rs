//! VideoToolbox H.264 for a stream: real time, no frame reordering, High profile at the level its size needs, the average bitrate with a data
//! rate limit of about two frames, a keyframe only when asked for, and the low-latency rate control where the Mac has it. Each setting is read
//! back and the ones the encoder refused are reported (`refused`, as on Windows). The capture's BGRA picture is scaled into the encoder's own
//! NV12 buffers by a pixel transfer session (the BGRA picture stays for the tiles). Frames come back on VideoToolbox's thread in AVCC and go out
//! in Annex B, the SPS and PPS in front of each keyframe (`sps::avcc_to_annex_b`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use super::ffi::*;
use crate::sps::avcc_to_annex_b;
use crate::stream_core::Encoded;

/// What the output callback shares with the encoder.
struct Shared {
    done: Mutex<Vec<Encoded>>,
    delivered: AtomicU64,
    first_output_ms: Mutex<Option<f64>>,
    opened: Instant,
}

unsafe extern "C" fn on_output(refcon: *mut c_void, frame: *mut c_void, status: OSStatus, flags: u32, sb: CMSampleBufferRef) {
    let shared = &*(refcon as *const Shared);
    shared.delivered.fetch_add(1, Ordering::SeqCst);
    if status != 0 || sb.is_null() || flags & kVTEncodeInfo_FrameDropped != 0 {
        return;
    }
    let info = array_items(CMSampleBufferGetSampleAttachmentsArray(sb, 0)).first().copied().unwrap_or(std::ptr::null());
    let key = !bool_of(dict_get(info, kCMSampleAttachmentKey_NotSync)).unwrap_or(false);
    let block = CMSampleBufferGetDataBuffer(sb);
    if block.is_null() {
        return;
    }
    let len = CMBlockBufferGetDataLength(block);
    let mut data = vec![0u8; len];
    if CMBlockBufferCopyDataBytes(block, 0, len, data.as_mut_ptr() as *mut c_void) != 0 {
        return;
    }
    let format = CMSampleBufferGetFormatDescription(sb);
    let mut params: Vec<&[u8]> = Vec::new();
    let mut nal_len = 4i32;
    if !format.is_null() {
        let (mut count, mut ptr, mut size) = (0usize, std::ptr::null::<u8>(), 0usize);
        if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, 0, &mut ptr, &mut size, &mut count, &mut nal_len) == 0 && key {
            for i in 0..count {
                if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, i, &mut ptr, &mut size, std::ptr::null_mut(), std::ptr::null_mut()) == 0 && !ptr.is_null() {
                    params.push(std::slice::from_raw_parts(ptr, size));
                }
            }
        }
    }
    let Some(au) = avcc_to_annex_b(&data, nal_len.max(1) as usize, &params) else { return };
    if let Ok(mut first) = shared.first_output_ms.lock() {
        first.get_or_insert_with(|| shared.opened.elapsed().as_secs_f64() * 1000.0);
    }
    if let Ok(mut done) = shared.done.lock() {
        done.push(Encoded { data: au, key, time: frame as i64, qp: None });
    }
}

/// How many times the low-latency encoder is asked for before the ordinary one is taken.
const LOW_LATENCY_TRIES: u32 = 5;

pub struct Encoder {
    session: VTSessionRef,
    transfer: VTSessionRef,
    shared: *mut Shared,
    pub hardware: bool,
    pub name: String,
    pub refused: Vec<String>,
    pub w: u32,
    pub h: u32,
    fps: u32,
    submitted: u64,
    /// The low-latency rate control took it. The ordinary encoder may hold a frame until the next comes (`asks_for_input`).
    low_latency: bool,
}

unsafe impl Send for Encoder {}

/// The data rate limit: about two frames of the average bitrate, over two frames' time.
fn rate_limits(kbps: u32, fps: u32) -> Cf {
    let fps = f64::from(fps.max(1));
    let bytes = (f64::from(kbps) * 1000.0 / 8.0 * 2.0 / fps).round() as i64;
    let (b, s) = (cfnum_i64(bytes.max(1)), cfnum_f64(2.0 / fps));
    cfarray(&[b.ptr(), s.ptr()])
}

impl Encoder {
    /// An encoder for `w`×`h` NV12 at `fps`, `kbps`; `hardware`: the hardware encoder if the Mac has one (VideoToolbox falls back to its
    /// software one by itself: `hardware` says which it is).
    pub fn open(w: u32, h: u32, fps: u32, kbps: u32, hardware: bool) -> Result<Encoder, String> {
        let shared = Box::into_raw(Box::new(Shared { done: Mutex::new(Vec::new()), delivered: AtomicU64::new(0), first_output_ms: Mutex::new(None), opened: Instant::now() }));
        let mut e = Encoder { session: std::ptr::null_mut(), transfer: std::ptr::null_mut(), shared, hardware: false, name: String::new(), refused: Vec::new(), w, h, fps: fps.max(1), submitted: 0, low_latency: false };
        unsafe {
            let empty = cfdict(&[]);
            let (pf, wn, hn) = (cfnum_i64(i64::from(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange)), cfnum_i64(i64::from(w)), cfnum_i64(i64::from(h)));
            let attrs = cfdict(&[(kCVPixelBufferPixelFormatTypeKey, pf.ptr()), (kCVPixelBufferWidthKey, wn.ptr()), (kCVPixelBufferHeightKey, hn.ptr()), (kCVPixelBufferIOSurfacePropertiesKey, empty.ptr())]);
            let hw = cfbool(hardware);
            let low = kVTVideoEncoderSpecification_EnableLowLatencyRateControl;
            let use_hw = kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder;
            // Low latency first: with the hardware asked for, then as VideoToolbox picks (on a Mac without a hardware encoder the low-latency
            // one is its software encoder, which a hardware request makes it refuse), and only then without it.
            let mut specs = vec![(cfdict(&[(low, cfbool(true)), (use_hw, hw)]), true)];
            if hardware {
                specs.push((cfdict(&[(low, cfbool(true))]), true));
            }
            specs.push((cfdict(&[(use_hw, hw)]), false));
            let (mut last, mut low_err) = (0, 0);
            let last_low = specs.iter().rposition(|s| s.1);
            'specs: for (i, (spec, low_latency)) in specs.iter().enumerate() {
                // The low-latency encoder of a session just closed is let go a moment later: the last low-latency try waits for it.
                let tries = if Some(i) == last_low { LOW_LATENCY_TRIES } else { 1 };
                for t in 0..tries {
                    if t > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    last = VTCompressionSessionCreate(std::ptr::null(), w as i32, h as i32, kCMVideoCodecType_H264, spec.ptr(), attrs.ptr(), std::ptr::null(), on_output, shared as *mut c_void, &mut e.session);
                    if last == 0 && !e.session.is_null() {
                        e.low_latency = *low_latency;
                        if !low_latency {
                            e.refused.push(format!("EnableLowLatencyRateControl ({low_err} at {w}x{h})"));
                        }
                        break 'specs;
                    }
                    if *low_latency {
                        low_err = last;
                    }
                    e.session = std::ptr::null_mut();
                }
            }
            if e.session.is_null() {
                return Err(format!("no H.264 encoder works on this Mac ({last})"));
            }
            let fps_n = cfnum_i64(i64::from(fps.max(1)));
            let rate = cfnum_i64(i64::from(kbps) * 1000);
            let keys = cfnum_i64(100_000);
            e.set("RealTime", kVTCompressionPropertyKey_RealTime, cfbool(true));
            e.set("AllowFrameReordering", kVTCompressionPropertyKey_AllowFrameReordering, cfbool(false));
            // Every frame comes back before the next goes in (the loop keeps one in the encoder): an encoder that waits for more input
            // before it gives one back would never give it.
            let no_delay = cfnum_i64(0);
            e.set("MaxFrameDelayCount", kVTCompressionPropertyKey_MaxFrameDelayCount, no_delay.ptr());
            e.set("ProfileLevel", kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel);
            e.set("ExpectedFrameRate", kVTCompressionPropertyKey_ExpectedFrameRate, fps_n.ptr());
            e.set("MaxKeyFrameInterval", kVTCompressionPropertyKey_MaxKeyFrameInterval, keys.ptr());
            e.set("AverageBitRate", kVTCompressionPropertyKey_AverageBitRate, rate.ptr());
            e.set("DataRateLimits", kVTCompressionPropertyKey_DataRateLimits, rate_limits(kbps, fps).ptr());
            e.set("ColorPrimaries", kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
            e.set("TransferFunction", kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
            e.set("YCbCrMatrix", kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
            // What it kept, read back.
            for (name, key, want) in [
                ("RealTime", kVTCompressionPropertyKey_RealTime, cfbool(true)),
                ("AllowFrameReordering", kVTCompressionPropertyKey_AllowFrameReordering, cfbool(false)),
                ("ProfileLevel", kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel),
                ("MaxKeyFrameInterval", kVTCompressionPropertyKey_MaxKeyFrameInterval, keys.ptr()),
                ("AverageBitRate", kVTCompressionPropertyKey_AverageBitRate, rate.ptr()),
            ] {
                if e.refused.iter().any(|r| r == name) {
                    continue;
                }
                match e.get(key) {
                    Some(got) if CFEqual(got.ptr(), want) != 0 => {}
                    Some(got) => e.refused.push(format!("{name} (reads {})", shown(got.ptr()))),
                    None => e.refused.push(format!("{name} (unread)")),
                }
            }
            let _ = VTCompressionSessionPrepareToEncodeFrames(e.session);
            e.hardware = e.get(kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder).and_then(|v| bool_of(v.ptr())).unwrap_or(false);
            e.name = e.get(cfstr("EncoderID").ptr()).and_then(|v| string_of(v.ptr())).unwrap_or_else(|| "VideoToolbox H.264".into());
            if VTPixelTransferSessionCreate(std::ptr::null(), &mut e.transfer) != 0 {
                e.transfer = std::ptr::null_mut();
                return Err("the picture cannot be converted for the encoder".into());
            }
        }
        Ok(e)
    }

    unsafe fn set(&mut self, name: &str, key: CFStringRef, value: CFTypeRef) {
        if VTSessionSetProperty(self.session, key, value) != 0 {
            self.refused.push(name.to_string());
        }
    }

    unsafe fn get(&self, key: CFStringRef) -> Option<Cf> {
        let mut v: CFTypeRef = std::ptr::null();
        (VTSessionCopyProperty(self.session, key, std::ptr::null(), &mut v as *mut CFTypeRef) == 0).then(|| Cf::take(v)).flatten()
    }

    /// Whether the encoder asks for another frame while it holds one: the ordinary encoder (without the low-latency rate control) gives a frame
    /// back only once the next has come, so the loop feeds it at its rate rather than waiting.
    pub fn asks_for_input(&self) -> bool {
        !self.low_latency
    }

    pub fn held(&self) -> u64 {
        self.submitted.saturating_sub(unsafe { (*self.shared).delivered.load(Ordering::SeqCst) })
    }

    pub fn first_output_ms(&self) -> Option<f64> {
        unsafe { (*self.shared).first_output_ms.lock().ok().and_then(|f| *f) }
    }

    pub fn set_bitrate(&mut self, kbps: u32) {
        unsafe {
            let rate = cfnum_i64(i64::from(kbps) * 1000);
            let _ = VTSessionSetProperty(self.session, kVTCompressionPropertyKey_AverageBitRate, rate.ptr());
            let _ = VTSessionSetProperty(self.session, kVTCompressionPropertyKey_DataRateLimits, rate_limits(kbps, self.fps).ptr());
        }
    }

    /// The capture's BGRA picture, scaled into one of the encoder's NV12 buffers.
    pub fn convert(&mut self, picture: &Cf) -> Result<Cf, String> {
        unsafe {
            let pool = VTCompressionSessionGetPixelBufferPool(self.session);
            if pool.is_null() {
                return Err("the encoder has no buffers".into());
            }
            let mut out: CVPixelBufferRef = std::ptr::null_mut();
            if CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), pool, &mut out) != 0 || out.is_null() {
                return Err("the encoder has no buffer free".into());
            }
            let out = Cf(out as CFTypeRef);
            CVBufferSetAttachment(out.ptr() as CVPixelBufferRef, kCVImageBufferYCbCrMatrixKey, kCVImageBufferYCbCrMatrix_ITU_R_709_2, 1);
            CVBufferSetAttachment(out.ptr() as CVPixelBufferRef, kCVImageBufferColorPrimariesKey, kCVImageBufferColorPrimaries_ITU_R_709_2, 1);
            CVBufferSetAttachment(out.ptr() as CVPixelBufferRef, kCVImageBufferTransferFunctionKey, kCVImageBufferTransferFunction_ITU_R_709_2, 1);
            let st = VTPixelTransferSessionTransferImage(self.transfer, picture.ptr() as CVPixelBufferRef, out.ptr() as CVPixelBufferRef);
            if st != 0 {
                return Err(format!("the picture could not be converted ({st})"));
            }
            Ok(out)
        }
    }

    /// Gives the encoder a frame; its output comes from `poll`.
    pub fn encode(&mut self, input: &Cf, time: i64, key: bool) -> Result<bool, String> {
        unsafe {
            let force = key.then(|| cfdict(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, cfbool(true))]));
            let st = VTCompressionSessionEncodeFrame(
                self.session,
                input.ptr() as CVPixelBufferRef,
                CMTime::new(time, 10_000_000),
                CMTime::INVALID,
                force.as_ref().map_or(std::ptr::null(), |f| f.ptr()),
                time as *mut c_void,
                std::ptr::null_mut(),
            );
            if st != 0 {
                return Err(format!("the encoder failed ({st})"));
            }
            self.submitted += 1;
            Ok(true)
        }
    }

    pub fn poll(&mut self) -> Vec<Encoded> {
        unsafe { (*self.shared).done.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default() }
    }
}

fn shown(v: CFTypeRef) -> String {
    string_of(v).or_else(|| i64_of(v).map(|n| n.to_string())).or_else(|| bool_of(v).map(|b| b.to_string())).unwrap_or_else(|| "?".into())
}

impl Encoder {
    /// Lets the session go now (before its successor opens: the Mac may have one low-latency encoder at a time). What is called on a closed
    /// encoder fails, which ends the stream.
    pub fn close(&mut self) {
        unsafe {
            if !self.session.is_null() {
                let _ = VTCompressionSessionCompleteFrames(self.session, CMTime::INVALID);
                VTCompressionSessionInvalidate(self.session);
                CFRelease(self.session as CFTypeRef);
                self.session = std::ptr::null_mut();
            }
            if !self.transfer.is_null() {
                VTPixelTransferSessionInvalidate(self.transfer);
                CFRelease(self.transfer as CFTypeRef);
                self.transfer = std::ptr::null_mut();
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.close();
        // No callback comes after the session is invalidated.
        unsafe { drop(Box::from_raw(self.shared)) };
    }
}
