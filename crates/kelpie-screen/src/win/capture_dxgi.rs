//! A monitor, through DXGI Desktop Duplication. DXGI keeps only the newest picture and adds up what changed until it is asked, so not
//! asking while the phone has no room is the one-slot mailbox. The cursor is not composited (the phone draws it).
//!
//! `ACCESS_LOST` comes with a desktop switch (a UAC prompt, the lock screen) and a mode change: the duplication is made again, and fails
//! with `E_ACCESSDENIED` for as long as the secure desktop is up, which the states report. A mode change that resized or rotated the monitor
//! ends the stream (`Grab::Changed`): the textures were made for the old size. A rotated monitor is refused at the start for now (its
//! duplicated image is not in the desktop's orientation).

use windows::core::{Interface, Result};
use windows::Win32::Foundation::{E_ACCESSDENIED, RECT};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED};
use windows::Win32::Graphics::Dxgi::{IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO};

use super::d3d::Device;
use crate::geom::Rect;

pub enum Grab {
    /// A new picture is in the texture; what changed, when DXGI says.
    Frame { dirty: Option<Vec<Rect>> },
    /// Nothing changed.
    Timeout,
    /// The secure desktop is up, or the duplication cannot be made now.
    Unavailable,
    /// The monitor was resized or rotated.
    Changed,
}

pub struct Dxgi {
    output: IDXGIOutput1,
    dup: Option<IDXGIOutputDuplication>,
    pub rect: Rect,
}

impl Dxgi {
    pub fn new(d: &Device, output: IDXGIOutput1, rect: Rect) -> std::result::Result<Dxgi, String> {
        let desc = unsafe { output.GetDesc() }.map_err(|e| e.to_string())?;
        if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
            return Err("a rotated screen cannot be shown yet: pick one of its windows instead".into());
        }
        let mut me = Dxgi { output, dup: None, rect };
        let _ = me.reopen(d);
        Ok(me)
    }

    /// Whether the monitor still has the size and orientation the stream was made for.
    fn unchanged(&self) -> bool {
        unsafe { self.output.GetDesc() }.is_ok_and(|d| {
            let r = d.DesktopCoordinates;
            (d.Rotation == DXGI_MODE_ROTATION_IDENTITY || d.Rotation == DXGI_MODE_ROTATION_UNSPECIFIED) && Rect::from_edges(r.left, r.top, r.right, r.bottom) == self.rect
        })
    }

    fn reopen(&mut self, d: &Device) -> Result<()> {
        self.dup = None;
        self.dup = Some(unsafe { self.output.DuplicateOutput(&d.device)? });
        Ok(())
    }

    /// Waits up to `timeout_ms` for a new picture and copies it into `into` (the monitor's size).
    pub fn grab(&mut self, d: &Device, timeout_ms: u32, into: &ID3D11Texture2D) -> Grab {
        if self.dup.is_none() {
            if let Err(e) = self.reopen(d) {
                if e.code() == E_ACCESSDENIED {
                    std::thread::sleep(std::time::Duration::from_millis(u64::from(timeout_ms)));
                }
                return Grab::Unavailable;
            }
        }
        let dup = self.dup.as_ref().expect("opened above");
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Grab::Timeout,
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                self.dup = None;
                return if self.unchanged() { Grab::Unavailable } else { Grab::Changed };
            }
            Err(_) => {
                self.dup = None;
                return Grab::Unavailable;
            }
        }
        // Only the pointer moved: no new picture.
        let out = if info.LastPresentTime == 0 || info.AccumulatedFrames == 0 {
            Grab::Timeout
        } else {
            let dirty = self.dirty_rects(dup, info.TotalMetadataBufferSize);
            match resource.and_then(|r| r.cast::<ID3D11Texture2D>().ok()) {
                Some(tex) => {
                    // `into` may be a little smaller (a monitor of an odd size, made even for the encoder): its own size is copied.
                    let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
                    unsafe { into.GetDesc(&mut desc) };
                    let region = windows::Win32::Graphics::Direct3D11::D3D11_BOX { left: 0, top: 0, front: 0, right: desc.Width, bottom: desc.Height, back: 1 };
                    unsafe { d.ctx.CopySubresourceRegion(into, 0, 0, 0, 0, &tex, 0, Some(&region)) };
                    Grab::Frame { dirty }
                }
                None => Grab::Timeout,
            }
        };
        unsafe {
            let _ = dup.ReleaseFrame();
        }
        out
    }

    /// The changed rectangles (move rectangles count as changes at their destination), monitor-relative.
    fn dirty_rects(&self, dup: &IDXGIOutputDuplication, size: u32) -> Option<Vec<Rect>> {
        if size == 0 {
            return None;
        }
        let mut buf = vec![RECT::default(); (size as usize) / std::mem::size_of::<RECT>() + 1];
        let mut used = 0u32;
        let bytes = (buf.len() * std::mem::size_of::<RECT>()) as u32;
        unsafe { dup.GetFrameDirtyRects(bytes, buf.as_mut_ptr(), &mut used).ok()? };
        let n = used as usize / std::mem::size_of::<RECT>();
        let mut out: Vec<Rect> = buf[..n].iter().map(|r| Rect::from_edges(r.left, r.top, r.right, r.bottom)).collect();
        let mut moves = vec![windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT::default(); 64];
        let mut used = 0u32;
        let bytes = (moves.len() * std::mem::size_of::<windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT>()) as u32;
        if unsafe { dup.GetFrameMoveRects(bytes, moves.as_mut_ptr(), &mut used) }.is_ok() {
            let n = used as usize / std::mem::size_of::<windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT>();
            out.extend(moves[..n].iter().map(|m| Rect::from_edges(m.DestinationRect.left, m.DestinationRect.top, m.DestinationRect.right, m.DestinationRect.bottom)));
        } else {
            // More moves than room: say "everything".
            return None;
        }
        Some(out)
    }
}
