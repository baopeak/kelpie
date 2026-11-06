//! H.264 with Media Foundation. The hardware encoder of the adapter the picture is on is tried first (an asynchronous MFT fed D3D11 NV12
//! surfaces through a DXGI device manager); the inbox software encoder (synchronous, CPU NV12) is the fallback, never openh264.
//!
//! Settings, from the H.264 encoder's documentation: output type before input type; CBR with a buffer of about two frames; no B-frames;
//! a long GOP with a keyframe only when asked (`CODECAPI_AVEncVideoForceKeyFrame`, which certified hardware encoders must support);
//! `CODECAPI_AVLowLatencyMode` (optional for hardware: it may be ignored, which the spike measures); one reference frame where offered.
//! The bitrate changes live (`CODECAPI_AVEncCommonMeanBitRate`); a new size makes a new encoder. Every setting that did not take is
//! recorded in `refused`, and the stream says which encoder it uses and what it refused.

use std::mem::ManuallyDrop;

use windows::core::{Interface, Result, GUID, PWSTR};
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VARIANT;

use super::d3d::Device;

const MF_VERSION_LITE: u32 = 0x0002_0070;

pub fn startup() -> Result<()> {
    unsafe { MFStartup(MF_VERSION_LITE, MFSTARTUP_LITE) }
}

pub struct Encoded {
    pub data: Vec<u8>,
    pub key: bool,
    /// The input's time, in 100 ns units, as it was given.
    pub time: i64,
}

pub struct Encoder {
    mft: IMFTransform,
    activate: IMFActivate,
    codec: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    _manager: Option<IMFDXGIDeviceManager>,
    in_id: u32,
    out_id: u32,
    need_input: u32,
    provides_samples: bool,
    out_size: u32,
    staging: Option<ID3D11Texture2D>,
    pub hardware: bool,
    pub name: String,
    pub w: u32,
    pub h: u32,
    fps: u32,
    pub sequence_header: Option<Vec<u8>>,
    /// Settings this encoder would not take.
    pub refused: Vec<&'static str>,
}

// The MFT is used from the stream's thread only.
unsafe impl Send for Encoder {}

fn video_type(subtype: &GUID, w: u32, h: u32, fps: u32) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(w) << 32) | u64::from(h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, (u64::from(fps.max(1)) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        Ok(t)
    }
}

/// The encoders on offer, best first (hardware: those of `luid`'s adapter first).
fn candidates(hardware: bool, luid: Option<(u32, i32)>) -> Result<Vec<IMFActivate>> {
    let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
    let output = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_H264 };
    let flags = if hardware { MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER } else { MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe { MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, Some(&input), Some(&output), &mut list, &mut count)? };
    let mut all = Vec::new();
    for i in 0..count as usize {
        if let Some(a) = unsafe { (*list.add(i)).take() } {
            all.push(a);
        }
    }
    if !list.is_null() {
        unsafe { CoTaskMemFree(Some(list as *const _)) };
    }
    if let Some((low, high)) = luid {
        // MFT_ENUM_ADAPTER_LUID is the adapter's LUID as a UINT64.
        let want = (u64::from(high as u32) << 32) | u64::from(low);
        all.sort_by_key(|a| unsafe { a.GetUINT64(&MFT_ENUM_ADAPTER_LUID) }.ok() != Some(want));
    }
    Ok(all)
}

fn friendly_name(a: &IMFActivate) -> String {
    unsafe {
        let mut p = PWSTR::null();
        let mut len = 0u32;
        if a.GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut p, &mut len).is_err() || p.is_null() {
            return "H.264 encoder".into();
        }
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

impl Encoder {
    /// The hardware encoder if `hardware` and one works, else the software one.
    pub fn open(d: &Device, w: u32, h: u32, fps: u32, kbps: u32, hardware: bool) -> Result<Encoder> {
        let luid = unsafe { d.adapter.GetDesc1() }.ok().map(|desc| (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart));
        let mut last = windows::core::Error::empty();
        for a in candidates(hardware, luid)? {
            match Encoder::with(d, a, w, h, fps, kbps, hardware) {
                Ok(e) => return Ok(e),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn with(d: &Device, activate: IMFActivate, w: u32, h: u32, fps: u32, kbps: u32, hardware: bool) -> Result<Encoder> {
        let name = friendly_name(&activate);
        let mft: IMFTransform = unsafe { activate.ActivateObject()? };
        let mut manager = None;
        if hardware {
            unsafe {
                let attrs = mft.GetAttributes()?;
                attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
                let mut token = 0u32;
                let mut m = None;
                MFCreateDXGIDeviceManager(&mut token, &mut m)?;
                let m: IMFDXGIDeviceManager = m.ok_or_else(windows::core::Error::empty)?;
                m.ResetDevice(&d.device, token)?;
                mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize)?;
                manager = Some(m);
            }
        }
        let (mut ins, mut outs) = ([0u32], [0u32]);
        let (in_id, out_id) = match unsafe { mft.GetStreamIDs(&mut ins, &mut outs) } {
            Ok(()) => (ins[0], outs[0]),
            Err(_) => (0, 0),
        };
        let codec = mft.cast::<ICodecAPI>().ok();
        let mut me = Encoder {
            mft,
            activate,
            codec,
            events: None,
            _manager: manager,
            in_id,
            out_id,
            need_input: 0,
            provides_samples: false,
            out_size: 0,
            staging: None,
            hardware,
            name,
            w,
            h,
            fps,
            sequence_header: None,
            refused: Vec::new(),
        };
        // What must be set before the output type.
        me.set("low latency", &CODECAPI_AVLowLatencyMode, VARIANT::from(true));
        me.set("B-frames", &CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
        me.set("CBR", &CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32));
        me.set("bitrate", &CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(kbps * 1000));
        me.set("buffer", &CODECAPI_AVEncCommonBufferSize, VARIANT::from(kbps * 1000 / 8 * 2 / fps.max(1)));
        me.set("GOP", &CODECAPI_AVEncMPVGOPSize, VARIANT::from(fps.max(1) * 3600));
        me.set("one reference", &CODECAPI_AVEncVideoMaxNumRefFrame, VARIANT::from(1u32));
        if !hardware {
            me.set("speed", &CODECAPI_AVEncCommonQualityVsSpeed, VARIANT::from(20u32));
        }
        unsafe {
            let out = video_type(&MFVideoFormat_H264, w, h, fps)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, kbps * 1000)?;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)?;
            me.mft.SetOutputType(me.out_id, &out, 0)?;
            let input = video_type(&MFVideoFormat_NV12, w, h, fps)?;
            me.mft.SetInputType(me.in_id, &input, 0)?;
            if let Ok(t) = me.mft.GetOutputCurrentType(me.out_id) {
                me.sequence_header = blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER);
            }
            let info = me.mft.GetOutputStreamInfo(me.out_id)?;
            me.provides_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
            me.out_size = info.cbSize.max(w * h);
            me.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            me.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        }
        if hardware {
            me.events = Some(me.mft.cast::<IMFMediaEventGenerator>()?);
        }
        Ok(me)
    }

    fn set(&mut self, what: &'static str, key: &GUID, value: VARIANT) {
        let ok = self.codec.as_ref().is_some_and(|c| unsafe { c.SetValue(key, &value) }.is_ok());
        if !ok {
            self.refused.push(what);
        }
    }

    pub fn set_bitrate(&mut self, kbps: u32) {
        let fps = self.fps.max(1);
        if let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(kbps * 1000));
                let _ = c.SetValue(&CODECAPI_AVEncCommonBufferSize, &VARIANT::from(kbps * 1000 / 8 * 2 / fps));
            }
        }
    }

    /// Feeds one NV12 picture. False when the encoder has no room for it now (the frame is dropped and counted).
    pub fn encode(&mut self, d: &Device, nv12: &ID3D11Texture2D, time: i64, key: bool) -> Result<bool> {
        if self.hardware {
            self.pump(&mut Vec::new())?;
            if self.need_input == 0 {
                return Ok(false);
            }
        }
        if key {
            if let Some(c) = &self.codec {
                unsafe {
                    let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32));
                }
            }
        }
        let buffer = if self.hardware { unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, nv12, 0, false)? } } else { self.cpu_buffer(d, nv12)? };
        unsafe {
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(10_000_000 / i64::from(self.fps.max(1)))?;
            match self.mft.ProcessInput(self.in_id, &sample, 0) {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_NOTACCEPTING => return Ok(false),
                Err(e) => return Err(e),
            }
        }
        if self.hardware {
            self.need_input -= 1;
        }
        Ok(true)
    }

    /// The software encoder takes CPU memory: the NV12 texture read back, its two planes packed.
    fn cpu_buffer(&mut self, d: &Device, nv12: &ID3D11Texture2D) -> Result<IMFMediaBuffer> {
        if self.staging.is_none() {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.w,
                Height: self.h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut t = None;
            unsafe { d.device.CreateTexture2D(&desc, None, Some(&mut t))? };
            self.staging = t;
        }
        let staging = self.staging.as_ref().ok_or_else(windows::core::Error::empty)?;
        let (w, h) = (self.w as usize, self.h as usize);
        let len = w * h * 3 / 2;
        unsafe {
            d.ctx.CopyResource(staging, nv12);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            d.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let buffer = MFCreateMemoryBuffer(len as u32)?;
            let mut dst: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut dst, None, None)?;
            let pitch = mapped.RowPitch as usize;
            let src = mapped.pData as *const u8;
            // Y: h rows; UV: h/2 rows, each w bytes, right below the Y plane in the mapping.
            for row in 0..h + h / 2 {
                std::ptr::copy_nonoverlapping(src.add(row * pitch), dst.add(row * w), w);
            }
            buffer.Unlock()?;
            buffer.SetCurrentLength(len as u32)?;
            d.ctx.Unmap(staging, 0);
            Ok(buffer)
        }
    }

    /// What the encoder has finished.
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        let mut out = Vec::new();
        if self.hardware {
            self.pump(&mut out)?;
        } else {
            while let Some(e) = self.output()? {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// The hardware encoder's events: room for input, output ready.
    fn pump(&mut self, out: &mut Vec<Encoded>) -> Result<()> {
        let Some(events) = self.events.clone() else { return Ok(()) };
        loop {
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e),
            };
            match unsafe { event.GetType()? } {
                t if t == METransformNeedInput.0 as u32 => self.need_input += 1,
                t if t == METransformHaveOutput.0 as u32 => {
                    if let Some(e) = self.output()? {
                        out.push(e);
                    }
                }
                _ => {}
            }
        }
    }

    fn output(&mut self) -> Result<Option<Encoded>> {
        let sample = if self.provides_samples {
            None
        } else {
            unsafe {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
                Some(s)
            }
        };
        let mut buffers = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: self.out_id, pSample: ManuallyDrop::new(sample), dwStatus: 0, pEvents: ManuallyDrop::new(None) }];
        let mut status = 0u32;
        let result = unsafe { self.mft.ProcessOutput(0, &mut buffers, &mut status) };
        let sample = unsafe { ManuallyDrop::take(&mut buffers[0].pSample) };
        drop(unsafe { ManuallyDrop::take(&mut buffers[0].pEvents) });
        match result {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                unsafe {
                    let t = self.mft.GetOutputAvailableType(self.out_id, 0)?;
                    self.mft.SetOutputType(self.out_id, &t, 0)?;
                    self.sequence_header = blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER).or(self.sequence_header.take());
                }
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
        let Some(sample) = sample else { return Ok(None) };
        unsafe {
            let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) == 1;
            let time = sample.GetSampleTime().unwrap_or(0);
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            buffer.Lock(&mut p, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(p, len as usize).to_vec();
            buffer.Unlock()?;
            Ok(Some(Encoded { data, key, time }))
        }
    }
}

fn blob(t: &IMFMediaType, key: &GUID) -> Option<Vec<u8>> {
    unsafe {
        let size = t.GetBlobSize(key).ok()?;
        let mut buf = vec![0u8; size as usize];
        t.GetBlob(key, &mut buf, None).ok()?;
        Some(buf)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self.activate.ShutdownObject();
        }
    }
}
