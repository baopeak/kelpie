//! BGRA → NV12 at the encoder's size, on the GPU (D3D11 video processor), into a small ring of output textures so that a hardware encoder
//! that still holds one is never written over. BT.709 limited range out, as H.264 decoders expect by default.

use windows::core::{Interface, Result, BOOL};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_RATIONAL};

use super::d3d::Device;

/// Outputs in the ring: the hardware encoder holds at most two frames (`encode_mf`).
const RING: usize = 3;

pub struct Converter {
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    inputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
    next: usize,
    pub src: (u32, u32),
    pub out: (u32, u32),
}

impl Converter {
    pub fn new(d: &Device, src: (u32, u32), out: (u32, u32), fps: u32) -> Result<Converter> {
        let vdev: ID3D11VideoDevice = d.device.cast()?;
        let vctx: ID3D11VideoContext = d.ctx.cast()?;
        let rate = DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 };
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: src.0,
            InputHeight: src.1,
            OutputFrameRate: rate,
            OutputWidth: out.0,
            OutputHeight: out.1,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { vdev.CreateVideoProcessorEnumerator(&desc)? };
        let processor = unsafe { vdev.CreateVideoProcessor(&enumerator, 0)? };
        unsafe {
            // Input: full-range RGB (all bits 0). Output: YCbCr (bit 0 is playback usage, 0), BT.709 matrix (bit 2), nominal range 16-235 (bits 4-5 = 1).
            let input_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            let output_space = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0b01_0100 };
            vctx.VideoProcessorSetStreamColorSpace(&processor, 0, &input_space);
            vctx.VideoProcessorSetOutputColorSpace(&processor, &output_space);
            vctx.VideoProcessorSetStreamFrameFormat(&processor, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vctx.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&RECT { left: 0, top: 0, right: src.0 as i32, bottom: src.1 as i32 }));
            vctx.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&RECT { left: 0, top: 0, right: out.0 as i32, bottom: out.1 as i32 }));
            vctx.VideoProcessorSetOutputTargetRect(&processor, true, Some(&RECT { left: 0, top: 0, right: out.0 as i32, bottom: out.1 as i32 }));
        }
        let mut outputs = Vec::with_capacity(RING);
        for _ in 0..RING {
            let tex = d.texture(out.0, out.1, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET)?;
            let view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC { ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D, Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } } };
            let mut view = None;
            unsafe { vdev.CreateVideoProcessorOutputView(&tex, &enumerator, &view_desc, Some(&mut view))? };
            outputs.push((tex, view.ok_or_else(windows::core::Error::empty)?));
        }
        Ok(Converter { vdev, vctx, enumerator, processor, outputs, inputs: Vec::new(), next: 0, src, out })
    }

    fn input_view(&mut self, tex: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
        if let Some((_, v)) = self.inputs.iter().find(|(t, _)| t == tex) {
            return Ok(v.clone());
        }
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC { FourCC: 0, ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D, Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } } };
        let mut view = None;
        unsafe { self.vdev.CreateVideoProcessorInputView(tex, &self.enumerator, &desc, Some(&mut view))? };
        let view = view.ok_or_else(windows::core::Error::empty)?;
        // A few textures feed this (the capture's): keep their views, the oldest out first.
        if self.inputs.len() >= 4 {
            self.inputs.remove(0);
        }
        self.inputs.push((tex.clone(), view.clone()));
        Ok(view)
    }

    /// `input` (BGRA, `src`) converted into the next NV12 output of the ring.
    pub fn convert(&mut self, input: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        let view = self.input_view(input)?;
        let (tex, out_view) = self.outputs[self.next].clone();
        self.next = (self.next + 1) % self.outputs.len();
        let stream = D3D11_VIDEO_PROCESSOR_STREAM { Enable: BOOL::from(true), pInputSurface: std::mem::ManuallyDrop::new(Some(view)), ..Default::default() };
        let streams = [stream];
        let done = unsafe { self.vctx.VideoProcessorBlt(&self.processor, &out_view, 0, &streams) };
        // The stream's view reference is ours to release.
        for s in streams {
            drop(std::mem::ManuallyDrop::into_inner(s.pInputSurface));
        }
        done.map(|()| tex)
    }
}
