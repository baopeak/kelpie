//! A window, through Windows Graphics Capture. Frames arrive on a pool thread when the window's content changes; the handler copies the
//! newest into the stream's texture and signals, so only the latest is ever kept (the one-slot mailbox). The cursor is left out
//! (`IsCursorCaptureEnabled(false)`, Windows 10 2004+). On Windows 10 the yellow border stays: it marks what is shared.
//!
//! WGC on Windows 10 gives no dirty regions, so the tiles find changes by hashing (`tiles`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::core::{factory, Interface, Result};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_TEXTURE2D_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use super::d3d::Device;

struct Shared {
    /// The newest picture, copied out of the pool, and its size.
    latest: Option<(ID3D11Texture2D, u32, u32)>,
    fresh: bool,
}

pub struct Wgc {
    _item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    pub closed: Arc<AtomicBool>,
}

// The WinRT objects are agile (free-threaded pool); the stream owns this from one thread.
unsafe impl Send for Wgc {}

pub enum Grab {
    /// The newest picture, `w`×`h`, copied into `into` (made again by the caller when the size changed).
    Frame { w: u32, h: u32 },
    Timeout,
}

impl Wgc {
    pub fn start(d: &Device, hwnd: HWND) -> Result<Wgc> {
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let dxgi: IDXGIDevice = d.device.cast()?;
        let rt_device: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&rt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let shared = Arc::new((Mutex::new(Shared { latest: None, fresh: false }), Condvar::new()));
        let closed = Arc::new(AtomicBool::new(false));
        {
            let (shared, device, ctx, last_size) = (shared.clone(), d.device.clone(), d.ctx.clone(), Arc::new(Mutex::new(size)));
            let rt_device = rt_device.clone();
            pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(move |pool, _| {
                let Some(pool) = pool.as_ref() else { return Ok(()) };
                // Only the newest of what waits.
                let mut frame = None;
                while let Ok(f) = pool.TryGetNextFrame() {
                    frame = Some(f);
                }
                let Some(frame) = frame else { return Ok(()) };
                let content = frame.ContentSize()?;
                let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                let tex: ID3D11Texture2D = unsafe { access.GetInterface()? };
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe { tex.GetDesc(&mut desc) };
                let (w, h) = (content.Width.max(2) as u32 & !1, content.Height.max(2) as u32 & !1);
                let (lock, cv) = &*shared;
                let mut s = lock.lock().unwrap();
                let need_new = !matches!(s.latest, Some((_, lw, lh)) if lw == w && lh == h);
                if need_new {
                    let copy = D3D11_TEXTURE2D_DESC { Width: w, Height: h, BindFlags: (windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0 | windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE.0) as u32, MiscFlags: 0, CPUAccessFlags: 0, Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT, MipLevels: 1, ArraySize: 1, ..desc };
                    let mut t = None;
                    unsafe { device.CreateTexture2D(&copy, None, Some(&mut t))? };
                    s.latest = t.map(|t| (t, w, h));
                }
                if let Some((dst, ..)) = &s.latest {
                    // The window's content may be smaller than the pool's buffer: copy the content's part.
                    let src_box = windows::Win32::Graphics::Direct3D11::D3D11_BOX { left: 0, top: 0, front: 0, right: w.min(desc.Width), bottom: h.min(desc.Height), back: 1 };
                    unsafe { ctx.CopySubresourceRegion(dst, 0, 0, 0, 0, &tex, 0, Some(&src_box)) };
                }
                s.fresh = true;
                cv.notify_one();
                drop(s);
                // The window was resized: the pool follows, the next frame has the new size.
                let mut last = last_size.lock().unwrap();
                if content.Width != last.Width || content.Height != last.Height {
                    *last = content;
                    let _ = pool.Recreate(&rt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, SizeInt32 { Width: content.Width, Height: content.Height });
                }
                Ok(())
            }))?;
        }
        {
            let closed = closed.clone();
            let shared = shared.clone();
            item.Closed(&TypedEventHandler::<GraphicsCaptureItem, windows::core::IInspectable>::new(move |_, _| {
                closed.store(true, Ordering::SeqCst);
                shared.1.notify_one();
                Ok(())
            }))?;
        }
        session.StartCapture()?;
        Ok(Wgc { _item: item, pool, session, shared, closed })
    }

    /// Waits up to `timeout` for a picture newer than the last one taken, and hands its texture over.
    pub fn grab(&self, timeout: Duration) -> (Grab, Option<ID3D11Texture2D>) {
        let (lock, cv) = &*self.shared;
        let s = lock.lock().unwrap();
        let (mut s, _) = cv.wait_timeout_while(s, timeout, |s| !s.fresh && !self.closed.load(Ordering::SeqCst)).unwrap();
        if !s.fresh {
            return (Grab::Timeout, None);
        }
        s.fresh = false;
        match &s.latest {
            Some((t, w, h)) => (Grab::Frame { w: *w, h: *h }, Some(t.clone())),
            None => (Grab::Timeout, None),
        }
    }
}

impl Drop for Wgc {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
