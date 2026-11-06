//! D3D11: the device every stage of a stream shares (capture, conversion and the hardware encoder must be on the same adapter as the
//! monitor), the monitors as DXGI sees them, textures, and the CPU readback the paint-over tiles need.

use windows::core::{Interface, Result};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

use super::from_wide;
use crate::geom::{Monitor, Rect};

pub struct Device {
    pub device: ID3D11Device,
    pub ctx: ID3D11DeviceContext,
    pub adapter: IDXGIAdapter1,
}

impl Device {
    /// A device on `adapter`, safe to use from the capture callback's thread and the stream's (multithread protection on).
    pub fn on(adapter: &IDXGIAdapter1) -> Result<Device> {
        let mut device = None;
        let mut ctx = None;
        unsafe {
            D3D11CreateDevice(
                &adapter.cast::<IDXGIAdapter>()?,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
        }
        let device: ID3D11Device = device.ok_or_else(windows::core::Error::empty)?;
        let ctx: ID3D11DeviceContext = ctx.ok_or_else(windows::core::Error::empty)?;
        if let Ok(mt) = device.cast::<ID3D10Multithread>() {
            unsafe {
                let _ = mt.SetMultithreadProtected(true);
            }
        }
        Ok(Device { device, ctx, adapter: adapter.clone() })
    }

    pub fn texture(&self, w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex))? };
        tex.ok_or_else(windows::core::Error::empty)
    }

    /// A BGRA texture the capture copies into: a render target the video processor can read.
    pub fn frame_texture(&self, w: u32, h: u32) -> Result<ID3D11Texture2D> {
        self.texture(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE)
    }
}

/// A texture the CPU can read, for the tiles; kept and reused while the size holds.
pub struct Readback {
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    pub buf: Vec<u8>,
    pub stride: usize,
}

impl Readback {
    pub fn new() -> Readback {
        Readback { staging: None, buf: Vec::new(), stride: 0 }
    }

    /// Copies the top-left `w`×`h` of `tex` (BGRA, which may be larger: a monitor of an odd size) into `buf`, rows `stride` bytes apart.
    pub fn read(&mut self, d: &Device, tex: &ID3D11Texture2D, w: u32, h: u32) -> Result<()> {
        self.read_regions(d, tex, w, h, None)
    }

    /// `read`, of only `regions` when the capture says what changed (DXGI): the rest of `buf` still holds the picture before, which is the
    /// picture now there. The first read, a new size, or no regions read everything.
    pub fn read_regions(&mut self, d: &Device, tex: &ID3D11Texture2D, w: u32, h: u32, regions: Option<&[crate::geom::Rect]>) -> Result<()> {
        let same = matches!(self.staging, Some((_, sw, sh)) if sw == w && sh == h) && self.stride > 0;
        if let (true, Some(regions)) = (same, regions) {
            let staging = &self.staging.as_ref().expect("same size").0;
            let bounds = crate::geom::Rect::new(0, 0, w as i32, h as i32);
            let clipped: Vec<crate::geom::Rect> = regions.iter().filter_map(|r| r.intersect(bounds)).collect();
            if clipped.is_empty() {
                return Ok(());
            }
            unsafe {
                for r in &clipped {
                    let region = windows::Win32::Graphics::Direct3D11::D3D11_BOX { left: r.x as u32, top: r.y as u32, front: 0, right: r.right() as u32, bottom: r.bottom() as u32, back: 1 };
                    d.ctx.CopySubresourceRegion(staging, 0, r.x as u32, r.y as u32, 0, tex, 0, Some(&region));
                }
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                d.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                if mapped.RowPitch as usize == self.stride {
                    let src = mapped.pData as *const u8;
                    for r in &clipped {
                        for y in r.y as usize..r.bottom() as usize {
                            let at = y * self.stride + r.x as usize * 4;
                            std::ptr::copy_nonoverlapping(src.add(at), self.buf.as_mut_ptr().add(at), r.w as usize * 4);
                        }
                    }
                    d.ctx.Unmap(staging, 0);
                    return Ok(());
                }
                d.ctx.Unmap(staging, 0);
            }
        }
        if !matches!(self.staging, Some((_, sw, sh)) if sw == w && sh == h) {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut t = None;
            unsafe { d.device.CreateTexture2D(&desc, None, Some(&mut t))? };
            self.staging = Some((t.ok_or_else(windows::core::Error::empty)?, w, h));
        }
        let staging = &self.staging.as_ref().expect("made above").0;
        unsafe {
            let region = windows::Win32::Graphics::Direct3D11::D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
            d.ctx.CopySubresourceRegion(staging, 0, 0, 0, 0, tex, 0, Some(&region));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            d.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let pitch = mapped.RowPitch as usize;
            let len = pitch * h as usize;
            self.buf.resize(len, 0);
            std::ptr::copy_nonoverlapping(mapped.pData as *const u8, self.buf.as_mut_ptr(), len);
            d.ctx.Unmap(staging, 0);
            self.stride = pitch;
        }
        Ok(())
    }
}

impl Default for Readback {
    fn default() -> Self {
        Readback::new()
    }
}

/// Every monitor attached to the desktop, with the adapter and output that drive it.
pub fn outputs() -> Result<Vec<(Monitor, IDXGIAdapter1, IDXGIOutput1)>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut out = Vec::new();
    let mut a = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(a) } {
        let mut o = 0;
        while let Ok(output) = unsafe { adapter.EnumOutputs(o) } {
            o += 1;
            let output: IDXGIOutput = output;
            let Ok(desc) = (unsafe { output.GetDesc() }) else { continue };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let r: RECT = desc.DesktopCoordinates;
            let mut dpi = (96u32, 96u32);
            unsafe {
                let _ = GetDpiForMonitor(desc.Monitor, MDT_EFFECTIVE_DPI, &mut dpi.0, &mut dpi.1);
            }
            let primary = r.left == 0 && r.top == 0;
            let monitor = Monitor { name: from_wide(&desc.DeviceName), rect: Rect::from_edges(r.left, r.top, r.right, r.bottom), dpi: dpi.0, primary };
            if let Ok(o1) = output.cast::<IDXGIOutput1>() {
                out.push((monitor, adapter.clone(), o1));
            }
        }
        a += 1;
    }
    Ok(out)
}

pub fn monitors() -> Vec<Monitor> {
    outputs().map(|v| v.into_iter().map(|(m, ..)| m).collect()).unwrap_or_default()
}

/// The adapter driving the monitor a screen point is on (for a window: where it mostly is).
pub fn adapter_at(x: i32, y: i32) -> Result<IDXGIAdapter1> {
    let all = outputs()?;
    let monitors: Vec<Monitor> = all.iter().map(|(m, ..)| m.clone()).collect();
    let name = crate::geom::monitor_at(&monitors, x, y).map(|m| m.name.clone());
    all.into_iter().find(|(m, ..)| Some(&m.name) == name.as_ref()).map(|(_, a, _)| a).ok_or_else(windows::core::Error::empty)
}
