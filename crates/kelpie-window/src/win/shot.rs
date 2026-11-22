//! A picture of one window, taken from the window itself (`PrintWindow` with full content), so it is the window's own pixels when
//! another window covers it. The invisible resize border around a window is cut off: the picture is the frame a person sees.

use kelpie_screen::geom::Rect;
use kelpie_screen::win::targets_win::{minimized, window_rect};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetWindowDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

/// `PW_RENDERFULLCONTENT`: what the compositor shows, which is what a web view or a GPU-drawn window needs.
const FULL_CONTENT: u32 = 2;
const MAX_SIDE: i32 = 8192;

/// The frame a person sees, in screen pixels: where a node's rectangle and the picture's pixels are measured from.
pub fn frame(h: HWND) -> Option<Rect> {
    window_rect(h)
}

/// The window as a PNG, with its width and height.
pub fn png(h: HWND) -> Result<(u32, u32, Vec<u8>), String> {
    if minimized(h) {
        return Err("the window is minimized: it has no picture until it is restored".into());
    }
    let mut outer = RECT::default();
    unsafe {
        GetWindowRect(h, &mut outer)
            .map_err(|e| format!("the window's place cannot be read: {e}"))?
    };
    let (ow, oh) = (outer.right - outer.left, outer.bottom - outer.top);
    if ow <= 0 || oh <= 0 || ow > MAX_SIDE || oh > MAX_SIDE {
        return Err(format!("the window is {ow}x{oh}: nothing to picture"));
    }
    let mut bgra = vec![0u8; ow as usize * oh as usize * 4];
    let lines = unsafe {
        let wdc = GetWindowDC(Some(h));
        let mdc = CreateCompatibleDC(Some(wdc));
        let bmp = CreateCompatibleBitmap(wdc, ow, oh);
        let old = SelectObject(mdc, HGDIOBJ(bmp.0));
        let drawn = PrintWindow(h, mdc, PRINT_WINDOW_FLAGS(FULL_CONTENT)).as_bool();
        // `GetDIBits` wants the bitmap out of any device context.
        SelectObject(mdc, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: ow,
                biHeight: -oh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = if drawn {
            GetDIBits(
                mdc,
                bmp,
                0,
                oh as u32,
                Some(bgra.as_mut_ptr() as *mut _),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mdc);
        ReleaseDC(Some(h), wdc);
        lines
    };
    if lines != oh {
        return Err("the window did not draw itself".into());
    }
    // Cut to the visible frame.
    let f = frame(h).unwrap_or(Rect::new(outer.left, outer.top, ow, oh));
    let (x0, y0) = (
        (f.x - outer.left).clamp(0, ow - 1),
        (f.y - outer.top).clamp(0, oh - 1),
    );
    let (w, hgt) = (f.w.clamp(1, ow - x0), f.h.clamp(1, oh - y0));
    let mut rgb = Vec::with_capacity(w as usize * hgt as usize * 3);
    for y in y0..y0 + hgt {
        let row = &bgra[(y * ow + x0) as usize * 4..(y * ow + x0 + w) as usize * 4];
        for px in row.chunks_exact(4) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w as u32, hgt as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
    drop(writer);
    Ok((w as u32, hgt as u32, out))
}
