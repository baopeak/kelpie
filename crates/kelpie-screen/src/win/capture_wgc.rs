//! A window, through Windows Graphics Capture. Frames arrive on a pool thread when the window's content changes; the handler keeps the newest
//! frame and signals, so only the latest is ever kept (the one-slot mailbox), and the stream's thread copies it. The cursor is left out
//! (`IsCursorCaptureEnabled(false)`, Windows 10 2004+). On Windows 10 the yellow border stays: it marks what is shared.
//!
//! WGC on Windows 10 gives no dirty regions, so the tiles find changes by hashing (`tiles`).
//!
//! The handler runs on a pool thread and does no Direct3D: it keeps the frame (which holds one of the pool's two buffers until it is closed) and
//! recreates the pool after a resize, with the WinRT device behind an `AgileReference`. `grab` takes the frame out of the mailbox, lets the
//! mailbox go, and copies the picture into a texture of the stream's own with the immediate context, which the pool's thread never touches.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows::core::{factory, AgileReference, Interface, Result};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

use super::d3d::Device;

struct Shared {
    /// The newest frame, still the pool's: taken and copied by the stream's thread.
    latest: Option<Direct3D11CaptureFrame>,
}

pub struct Wgc {
    item: GraphicsCaptureItem,
    /// The handlers' registrations, removed on drop: the pool's handler holds the mailbox, which may hold a frame of the pool.
    arrived: i64,
    closed_token: i64,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    pub closed: Arc<AtomicBool>,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    /// The stream's own copy of the newest picture.
    own: Option<(ID3D11Texture2D, u32, u32)>,
}

// The WinRT objects are agile (free-threaded pool); the stream owns this from one thread.
unsafe impl Send for Wgc {}

pub enum Grab {
    /// The newest picture, `w`×`h`, copied into `into` (made again by the caller when the size changed).
    Frame { w: u32, h: u32 },
    Timeout,
    /// A frame came and could not be copied (a lost device): counted by the stream, which asks the device why after a run of them.
    Failed,
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
        let shared = Arc::new((Mutex::new(Shared { latest: None }), Condvar::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let arrived = {
            let (shared, last_size) = (shared.clone(), Arc::new(Mutex::new(size)));
            let rt_device = AgileReference::new(&rt_device)?;
            // No Direct3D here: the immediate context is the stream's (and the encoder's), and a copy made on this thread under the mailbox's
            // lock waited on the context's lock while the stream, holding that, waited on the mailbox: both stopped for good.
            pool.FrameArrived(&TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(move |pool, _| {
                let Some(pool) = pool.as_ref() else { return Ok(()) };
                // Only the newest of what waits; the older ones go back to the pool at once.
                let mut frame: Option<Direct3D11CaptureFrame> = None;
                while let Ok(f) = pool.TryGetNextFrame() {
                    if let Some(old) = frame.replace(f) {
                        let _ = old.Close();
                    }
                }
                let Some(frame) = frame else { return Ok(()) };
                let content = frame.ContentSize()?;
                let replaced = {
                    let (lock, cv) = &*shared;
                    let mut s = lock.lock().unwrap();
                    let old = s.latest.replace(frame);
                    cv.notify_one();
                    old
                };
                // Closed with the mailbox let go.
                if let Some(old) = replaced {
                    let _ = old.Close();
                }
                // The window was resized: the pool follows, the next frame has the new size.
                let mut last = last_size.lock().unwrap();
                if content.Width != last.Width || content.Height != last.Height {
                    *last = content;
                    if let Ok(rt) = rt_device.resolve() {
                        let _ = pool.Recreate(&rt, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, SizeInt32 { Width: content.Width, Height: content.Height });
                    }
                }
                Ok(())
            }))?
        };
        let closed_token = {
            let closed = closed.clone();
            let shared = shared.clone();
            item.Closed(&TypedEventHandler::<GraphicsCaptureItem, windows::core::IInspectable>::new(move |_, _| {
                closed.store(true, Ordering::SeqCst);
                shared.1.notify_one();
                Ok(())
            }))?
        };
        session.StartCapture()?;
        Ok(Wgc { item, arrived, closed_token, pool, session, shared, closed, device: d.device.clone(), ctx: d.ctx.clone(), own: None })
    }

    /// Waits up to `timeout` for a picture newer than the last one taken, and copies it into the stream's own texture, which it hands over.
    pub fn grab(&mut self, timeout: Duration) -> (Grab, Option<ID3D11Texture2D>) {
        let frame = {
            let (lock, cv) = &*self.shared;
            let s = lock.lock().unwrap();
            let (mut s, _) = cv.wait_timeout_while(s, timeout, |s| s.latest.is_none() && !self.closed.load(Ordering::SeqCst)).unwrap();
            s.latest.take()
        };
        // The copy is made with the mailbox let go: the pool's thread can always leave the next frame meanwhile.
        let Some(frame) = frame else { return (Grab::Timeout, None) };
        let copied = self.copy(&frame);
        let _ = frame.Close();
        match copied {
            Some((own, w, h)) => (Grab::Frame { w, h }, Some(own)),
            None => (Grab::Failed, None),
        }
    }

    fn copy(&mut self, frame: &Direct3D11CaptureFrame) -> Option<(ID3D11Texture2D, u32, u32)> {
        let content = frame.ContentSize().ok()?;
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface().ok()?.cast().ok()?;
        let tex: ID3D11Texture2D = unsafe { access.GetInterface() }.ok()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        // The window's content may be smaller than the pool's buffer (it was just resized): its part, an even size.
        let (w, h) = ((content.Width.max(2) as u32).min(desc.Width) & !1, (content.Height.max(2) as u32).min(desc.Height) & !1);
        if !matches!(self.own, Some((_, ow, oh)) if ow == w && oh == h) {
            let copy = D3D11_TEXTURE2D_DESC { Width: w, Height: h, BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, MiscFlags: 0, CPUAccessFlags: 0, Usage: D3D11_USAGE_DEFAULT, MipLevels: 1, ArraySize: 1, ..desc };
            let mut t = None;
            unsafe { self.device.CreateTexture2D(&copy, None, Some(&mut t)) }.ok()?;
            self.own = t.map(|t| (t, w, h));
        }
        let (own, ..) = self.own.as_ref()?;
        let src_box = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
        unsafe { self.ctx.CopySubresourceRegion(own, 0, 0, 0, 0, &tex, 0, Some(&src_box)) };
        Some((own.clone(), w, h))
    }
}

impl Drop for Wgc {
    /// The session first (no frame comes after it), then the handlers, then a frame still in the mailbox, then the pool: nothing is left
    /// holding the pool, its buffers or the device behind it.
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.RemoveFrameArrived(self.arrived);
        let _ = self.item.RemoveClosed(self.closed_token);
        let waiting = self.shared.0.lock().ok().and_then(|mut s| s.latest.take());
        if let Some(frame) = waiting {
            let _ = frame.Close();
        }
        let _ = self.pool.Close();
    }
}
