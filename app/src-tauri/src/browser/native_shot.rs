//! Pictures of a pane's page where there is no DevTools protocol: WKWebView's snapshots on macOS, WebKitGTK's on Linux. The contract is
//! Windows' (`Page.captureScreenshot`): a PNG, or a JPEG of a quality, of the visible page or of a region of the document in CSS px, drawn
//! at a scale of the page's own pixels. The page says where it is scrolled to and how large it is (`VIEW_JS`); WebKit pictures the visible
//! page (on Linux also the whole document), and what is kept of that picture is decided here. That part builds everywhere, so its tests run
//! on every platform.
//!
//! WebKit takes snapshot calls on the main thread only (`with_webview` runs there). A pane that is not shown is refused before the
//! snapshot (`NOT_SHOWN`): WebKitGTK may never answer for a view that is not on screen.

// Elsewhere only the tests use the pure part.
#![cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]

use std::time::Duration;

use serde::Deserialize;

/// Whether this platform pictures panes this way.
pub const AVAILABLE: bool = cfg!(any(target_os = "macos", target_os = "linux"));
/// Whether a region outside the visible page can be pictured (WebKitGTK's whole-document snapshot); macOS pictures only what is on screen.
pub const DOCUMENT: bool = cfg!(target_os = "linux");
/// The longest a snapshot may take.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// The tallest and widest whole-document snapshot taken (device px): past it only the visible page is pictured.
pub const MAX_DOCUMENT_PX: f64 = 16384.0;
/// The largest whole-document snapshot taken (device px²): a long and wide page at a high pixel ratio is a picture of hundreds of MB.
pub const MAX_DOCUMENT_AREA: f64 = 64.0e6;

pub const OUTSIDE: &str = "the region to picture is outside the visible page";
pub const TOO_LARGE: &str = "the page is too large to picture beyond its visible part";
pub const NOT_IN_TIME: &str = "the page was not pictured in time";
pub const NO_VIEW: &str = "the page did not say where it is scrolled to";
pub const NOT_PICTURED: &str = "WebKit could not picture the page";

/// Looked at on the main thread right before the snapshot: whether the window hides the pane.
pub type Hidden = Box<dyn FnOnce() -> bool + Send>;

/// Where the page is scrolled to and how large it is, CSS px: the viewport with its scrollbars (`iw`×`ih`) and without them (`cw`×`ch`),
/// the document, and the device pixel ratio.
#[derive(Deserialize, Default, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct View {
    pub sx: f64,
    pub sy: f64,
    pub iw: f64,
    pub ih: f64,
    pub cw: f64,
    pub ch: f64,
    pub dw: f64,
    pub dh: f64,
    pub dpr: f64,
}

/// What the page is asked for its `View`: a JSON string, which every platform hands back the same way.
pub const VIEW_JS: &str = "(() => { const d = document.documentElement, b = document.body; \
    return JSON.stringify({ sx: scrollX, sy: scrollY, iw: innerWidth, ih: innerHeight, cw: d.clientWidth || innerWidth, ch: d.clientHeight || innerHeight, \
    dw: Math.max(d.scrollWidth, b ? b.scrollWidth : 0), dh: Math.max(d.scrollHeight, b ? b.scrollHeight : 0), dpr: devicePixelRatio || 1 }); })()";

/// Whether a whole document `dw`×`dh` CSS px large, drawn at the device pixel ratio `dpr`, is small enough to snapshot (`MAX_DOCUMENT_PX`
/// each way, `MAX_DOCUMENT_AREA` in all).
pub fn document_fits(dw: f64, dh: f64, dpr: f64) -> bool {
    let dpr = if dpr > 0.0 { dpr } else { 1.0 };
    dw * dpr <= MAX_DOCUMENT_PX && dh * dpr <= MAX_DOCUMENT_PX && dw * dh * dpr * dpr <= MAX_DOCUMENT_AREA
}

/// The page's answer to `VIEW_JS` as `eval_with_callback` hands it on (the JSON of a string of JSON); none for anything else, or a page
/// without a size.
pub fn parse_view(s: &str) -> Option<View> {
    let inner: String = serde_json::from_str(s).ok()?;
    let v: View = serde_json::from_str(&inner).ok()?;
    (v.iw > 0.0 && v.ih > 0.0 && v.cw > 0.0 && v.ch > 0.0).then_some(v)
}

/// What WebKit pictures: the visible page, or the whole document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Region {
    Visible,
    Document,
}

/// The part of a snapshot that is kept: of which region, and where in it (CSS px from its top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub region: Region,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// `clip` (`[x, y, width, height]`, document CSS px; none: the visible page) as a part of what the platform pictures, `vis` being the size of
/// the visible page it pictures (WebKitGTK leaves the scrollbars out): a clip within the viewport (`iw`×`ih`, scrollbars included) is cut to
/// the visible page; one reaching past the viewport comes from the whole document where the platform pictures it (`document`) and the
/// document is not too large for it (`document_fits`), else is cut to the visible page. Refused when less than a pixel of it is left.
pub fn part(clip: Option<[f64; 4]>, v: &View, vis: (f64, f64), document: bool) -> Result<Part, &'static str> {
    let Some([x, y, w, h]) = clip else {
        return Ok(Part { region: Region::Visible, x: 0.0, y: 0.0, w: vis.0, h: vis.1 });
    };
    let (vx, vy) = (x - v.sx, y - v.sy);
    // Half a pixel either way is rounding, not outside.
    let inside = vx >= -0.5 && vy >= -0.5 && vx + w <= v.iw + 0.5 && vy + h <= v.ih + 0.5;
    let (dw, dh) = region_size(v, Region::Document, vis);
    let whole = document && document_fits(dw, dh, v.dpr);
    if !inside && whole {
        let [x, y, w, h] = cut([x, y, w, h], dw, dh).ok_or(OUTSIDE)?;
        return Ok(Part { region: Region::Document, x, y, w, h });
    }
    let refused = if !inside && document { TOO_LARGE } else { OUTSIDE };
    let [x, y, w, h] = cut([vx, vy, w, h], vis.0, vis.1).ok_or(refused)?;
    Ok(Part { region: Region::Visible, x, y, w, h })
}

/// The size of `region` in CSS px, `vis` being the visible page's: the whole document is at least as large as the visible page.
pub fn region_size(v: &View, region: Region, vis: (f64, f64)) -> (f64, f64) {
    match region {
        Region::Visible => vis,
        Region::Document => (v.dw.max(vis.0), v.dh.max(vis.1)),
    }
}

/// The rectangle `[x, y, w, h]` within `0..bw × 0..bh`; none when less than a pixel of it is left.
fn cut([x, y, w, h]: [f64; 4], bw: f64, bh: f64) -> Option<[f64; 4]> {
    let (l, t) = (x.max(0.0), y.max(0.0));
    let (r, b) = ((x + w).min(bw), (y + h).min(bh));
    (r - l >= 1.0 && b - t >= 1.0).then_some([l, t, r - l, b - t])
}

/// A scale of the page's own pixels: above 0, at most 1 (a picture is never drawn larger than the page shows it).
pub fn unit_scale(scale: f64) -> f64 {
    if scale > 0.0 && scale < 1.0 {
        scale
    } else {
        1.0
    }
}

/// The pixels of `p` in a snapshot `px` pixels large of a region `css` CSS px large (WebKitGTK draws at the screen's scale and the page's
/// zoom, so the two sizes say how many pixels a CSS px is): `[x, y, w, h]` within the snapshot; none when less than a pixel is left.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn pixels(p: &Part, css: (f64, f64), px: (i32, i32)) -> Option<[i32; 4]> {
    if css.0 <= 0.0 || css.1 <= 0.0 || px.0 < 1 || px.1 < 1 {
        return None;
    }
    let (pw, ph) = (f64::from(px.0), f64::from(px.1));
    let (rx, ry) = (pw / css.0, ph / css.1);
    let l = (p.x * rx).round().clamp(0.0, pw);
    let t = (p.y * ry).round().clamp(0.0, ph);
    let r = ((p.x + p.w) * rx).round().clamp(l, pw);
    let b = ((p.y + p.h) * ry).round().clamp(t, ph);
    (r - l >= 1.0 && b - t >= 1.0).then(|| [l as i32, t as i32, (r - l) as i32, (b - t) as i32])
}

/// `w`×`h` pixels drawn at `scale` (`unit_scale`), at least one pixel each way.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn scaled(w: i32, h: i32, scale: f64) -> (i32, i32) {
    let s = unit_scale(scale);
    ((f64::from(w) * s).round().max(1.0) as i32, (f64::from(h) * s).round().max(1.0) as i32)
}

/// The rectangle `[x, y, w, h]` of an image of 4-byte pixels whose rows are `stride` bytes apart, packed (`w` × 4 bytes a row); none when
/// it is not within `data`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn copy_rect(data: &[u8], stride: usize, [x, y, w, h]: [i32; 4]) -> Option<Vec<u8>> {
    let (x, y, w, h) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?, usize::try_from(w).ok()?, usize::try_from(h).ok()?);
    let row = w.checked_mul(4)?;
    let mut out = Vec::with_capacity(row.checked_mul(h)?);
    for r in y..y + h {
        let start = r.checked_mul(stride)?.checked_add(x * 4)?;
        out.extend_from_slice(data.get(start..start.checked_add(row)?)?);
    }
    Some(out)
}

/// Packed cairo pixels (`w`×`h`, native-endian 32-bit `0xAARRGGBB`, colour premultiplied by alpha; `alpha` false: the top byte means
/// nothing) as packed RGB `out` pixels large: each output pixel the average of the source pixels it covers, on a white background.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn rgb_scaled(px: &[u8], w: usize, h: usize, alpha: bool, (ow, oh): (usize, usize)) -> Vec<u8> {
    let mut out = Vec::with_capacity(ow * oh * 3);
    if w == 0 || h == 0 || px.len() < w * h * 4 {
        out.resize(ow * oh * 3, 255);
        return out;
    }
    // The source pixels `[a, b)` an output pixel `i` of `n` covers out of `len`: at least one.
    let span = |i: usize, n: usize, len: usize| {
        let a = i * len / n;
        (a.min(len - 1), ((i + 1) * len).div_ceil(n).clamp(a + 1, len))
    };
    for oy in 0..oh {
        let (y0, y1) = span(oy, oh, h);
        for ox in 0..ow {
            let (x0, x1) = span(ox, ow, w);
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    let p = u32::from_ne_bytes([px[i], px[i + 1], px[i + 2], px[i + 3]]);
                    // Premultiplied colour over white: c + (255 - a).
                    let under = if alpha { 255 - (p >> 24) } else { 0 };
                    sum[0] += u64::from(((p >> 16) & 0xFF) + under);
                    sum[1] += u64::from(((p >> 8) & 0xFF) + under);
                    sum[2] += u64::from((p & 0xFF) + under);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.iter().map(|s| ((s + n / 2) / n).min(255) as u8));
        }
    }
    out
}

/// A visible part `p` in the coordinates of a WKWebView `bounds` points large, which shows `iw`×`ih` CSS px (the page's zoom is in the
/// ratio): `[x, y, w, h]`, from the top when the view is `flipped`, else from the bottom as AppKit counts; and the snapshot's width in points
/// at `scale` (WebKit draws it at the screen's scale, as the page shows it).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn view_rect(p: &Part, v: &View, bounds: (f64, f64), flipped: bool, scale: f64) -> ([f64; 4], f64) {
    let (kx, ky) = (bounds.0 / v.iw, bounds.1 / v.ih);
    let (x, w) = (p.x * kx, p.w * kx);
    let (top, h) = (p.y * ky, p.h * ky);
    let y = if flipped { top } else { bounds.1 - top - h };
    ([x, y, w, h], w * unit_scale(scale))
}

// ---------- the platforms ----------

/// The answer of a snapshot, sent once from the main thread.
#[cfg(any(target_os = "macos", target_os = "linux"))]
type Reply = std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>>>;

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn send(reply: &Reply, r: Result<Vec<u8>, String>) {
    if let Some(tx) = reply.lock().ok().and_then(|mut t| t.take()) {
        let _ = tx.send(r);
    }
}

/// Where the page is scrolled to and how large it is, within `limit`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn view(wv: &tauri::Webview, limit: Duration) -> Result<View, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));
    wv.eval_with_callback(VIEW_JS, move |s| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(s);
        }
    })
    .map_err(|e| e.to_string())?;
    let s = tokio::time::timeout(limit, rx).await.map_err(|_| NOT_IN_TIME.to_string())?.map_err(|_| NO_VIEW.to_string())?;
    parse_view(&s).ok_or_else(|| NO_VIEW.to_string())
}

/// A picture of `clip` (`[x, y, width, height]`, document CSS px; none: the visible page) at `scale` of the page's own pixels: a PNG, or a
/// JPEG of quality `jpeg`; within `limit` (at most `TIMEOUT`). Refused (`NOT_SHOWN`) when the pane is not shown, or `hidden` says the
/// window hides it, as found on the main thread right before the snapshot.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub async fn take(wv: &tauri::Webview, clip: Option<[f64; 4]>, scale: f64, jpeg: Option<u8>, hidden: Option<Hidden>, limit: Duration) -> Result<Vec<u8>, String> {
    let deadline = std::time::Instant::now() + limit.min(TIMEOUT);
    let view = view(wv, limit.min(TIMEOUT)).await?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, String>>();
    let reply: Reply = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    wv.with_webview(move |pv| platform::snapshot(pv, view, clip, unit_scale(scale), jpeg, hidden, reply)).map_err(|e| e.to_string())?;
    let left = deadline.saturating_duration_since(std::time::Instant::now());
    tokio::time::timeout(left, rx).await.map_err(|_| NOT_IN_TIME.to_string())?.map_err(|_| NOT_PICTURED.to_string())?
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn take(_wv: &tauri::Webview, _clip: Option<[f64; 4]>, _scale: f64, _jpeg: Option<u8>, _hidden: Option<Hidden>, _limit: Duration) -> Result<Vec<u8>, String> {
    Err("browser screenshots are not available on this platform yet".into())
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSImageCompressionFactor};
    use objc2_foundation::{NSDictionary, NSError, NSNumber, NSPoint, NSRect, NSSize, NSString};
    use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

    use super::{part, send, view_rect, Hidden, Reply, View, NOT_PICTURED};
    use crate::browser::NOT_SHOWN;

    /// On the main thread: refuses a pane that is not shown (hidden, in a window that is hidden or minimised), else asks WKWebView for the
    /// part of its visible page and answers `reply` from its completion handler.
    pub fn snapshot(pv: tauri::webview::PlatformWebview, view: View, clip: Option<[f64; 4]>, scale: f64, jpeg: Option<u8>, hidden: Option<Hidden>, reply: Reply) {
        let Some(mtm) = MainThreadMarker::new() else {
            return send(&reply, Err(NOT_PICTURED.into()));
        };
        // SAFETY: `inner` is the pane's WKWebView, alive while this runs on the main thread.
        let wk: &WKWebView = unsafe { &*pv.inner().cast::<WKWebView>() };
        let shown = !wk.isHiddenOrHasHiddenAncestor() && wk.window().is_some_and(|w| w.isVisible() && !w.isMiniaturized());
        if !shown || hidden.is_some_and(|h| h()) {
            return send(&reply, Err(NOT_SHOWN.into()));
        }
        let p = match part(clip, &view, (view.iw, view.ih), false) {
            Ok(p) => p,
            Err(e) => return send(&reply, Err(e.into())),
        };
        let bounds = wk.bounds();
        let ([x, y, w, h], width) = view_rect(&p, &view, (bounds.size.width, bounds.size.height), wk.isFlipped(), scale);
        // SAFETY: a new configuration on the main thread, set before WebKit sees it.
        let config = unsafe { WKSnapshotConfiguration::new(mtm) };
        unsafe {
            config.setRect(NSRect::new(NSPoint::new(x, y), NSSize::new(w, h)));
            config.setSnapshotWidth(Some(&NSNumber::new_f64(width)));
        }
        let handler = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            // SAFETY: WebKit hands a valid image, or null with an error.
            let (image, error) = unsafe { (image.as_ref(), error.as_ref()) };
            let r = match (image, error) {
                (Some(i), _) => encode(i, jpeg).ok_or_else(|| NOT_PICTURED.to_string()),
                (None, Some(e)) => Err(format!("{NOT_PICTURED}: {}", e.localizedDescription())),
                (None, None) => Err(NOT_PICTURED.to_string()),
            };
            send(&reply, r);
        });
        // SAFETY: on the main thread, with a configuration and a handler WebKit keeps until it answers.
        unsafe { wk.takeSnapshotWithConfiguration_completionHandler(Some(&config), &handler) };
    }

    /// The image as a PNG, or a JPEG of quality `jpeg`.
    pub(super) fn encode(image: &NSImage, jpeg: Option<u8>) -> Option<Vec<u8>> {
        let tiff = image.TIFFRepresentation()?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
        let data = match jpeg {
            Some(q) => {
                let factor: Retained<NSNumber> = NSNumber::new_f64(f64::from(q.min(100)) / 100.0);
                let value: &AnyObject = &factor;
                // SAFETY: an AppKit constant; the compression factor is an NSNumber, as the key wants.
                let props = NSDictionary::<NSString, AnyObject>::from_slices(&[unsafe { NSImageCompressionFactor }], &[value]);
                unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &props) }
            }
            // SAFETY: no properties.
            None => unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::<NSString, AnyObject>::new()) },
        }?;
        Some(data.to_vec())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use gtk::cairo;
    use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
    use gtk::prelude::*;
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};

    use super::{copy_rect, part, pixels, region_size, rgb_scaled, scaled, send, Hidden, Part, Region, Reply, View, NOT_PICTURED};
    use crate::browser::NOT_SHOWN;

    /// On the main thread: refuses a pane that is not shown (hidden, not on screen, in a minimised window), else asks WebKitGTK for the
    /// visible page, or the whole document when the part reaches outside it, and answers `reply`: the part's pixels are copied out on the
    /// main thread, scaled and encoded off it.
    pub fn snapshot(pv: tauri::webview::PlatformWebview, view: View, clip: Option<[f64; 4]>, scale: f64, jpeg: Option<u8>, hidden: Option<Hidden>, reply: Reply) {
        let wk = pv.inner();
        let minimised = wk.window().is_some_and(|w| w.toplevel().state().contains(gtk::gdk::WindowState::ICONIFIED));
        if !wk.is_visible() || !wk.is_mapped() || minimised || hidden.is_some_and(|h| h()) {
            return send(&reply, Err(NOT_SHOWN.into()));
        }
        // WebKitGTK's visible page leaves out the scrollbars.
        let vis = (view.cw, view.ch);
        let p = match part(clip, &view, vis, true) {
            Ok(p) => p,
            Err(e) => return send(&reply, Err(e.into())),
        };
        let css = region_size(&view, p.region, vis);
        let region = match p.region {
            Region::Visible => SnapshotRegion::Visible,
            Region::Document => SnapshotRegion::FullDocument,
        };
        wk.snapshot(region, SnapshotOptions::NONE, None::<&webkit2gtk::gio::Cancellable>, move |res| {
            let copied = res.map_err(|e| format!("{NOT_PICTURED}: {e}")).and_then(|surface| copy_part(surface, &p, css));
            match copied {
                Ok((px, w, h, alpha)) => {
                    let _ = tauri::async_runtime::spawn_blocking(move || send(&reply, encode(&px, w, h, alpha, scale, jpeg)));
                }
                Err(e) => send(&reply, Err(e)),
            }
        });
    }

    /// The pixels of the part `p` of a snapshot of a region `css` CSS px large, packed: the pixels, their width and height, and whether
    /// they carry alpha.
    fn copy_part(surface: cairo::Surface, p: &Part, css: (f64, f64)) -> Result<(Vec<u8>, usize, usize, bool), String> {
        let shot = cairo::ImageSurface::try_from(surface).map_err(|_| NOT_PICTURED.to_string())?;
        let alpha = match shot.format() {
            cairo::Format::ARgb32 => true,
            cairo::Format::Rgb24 => false,
            _ => return Err(NOT_PICTURED.into()),
        };
        let rect = pixels(p, css, (shot.width(), shot.height())).ok_or_else(|| NOT_PICTURED.to_string())?;
        let stride = usize::try_from(shot.stride()).map_err(|_| NOT_PICTURED.to_string())?;
        let mut copied = None;
        shot.with_data(|data| copied = copy_rect(data, stride, rect)).map_err(|_| NOT_PICTURED.to_string())?;
        let px = copied.ok_or_else(|| NOT_PICTURED.to_string())?;
        Ok((px, rect[2] as usize, rect[3] as usize, alpha))
    }

    /// Packed cairo pixels `w`×`h` drawn at `scale`: a PNG, or a JPEG of quality `jpeg`.
    fn encode(px: &[u8], w: usize, h: usize, alpha: bool, scale: f64, jpeg: Option<u8>) -> Result<Vec<u8>, String> {
        let (ow, oh) = scaled(i32::try_from(w).map_err(|e| e.to_string())?, i32::try_from(h).map_err(|e| e.to_string())?, scale);
        let rgb = rgb_scaled(px, w, h, alpha, (ow as usize, oh as usize));
        let pixbuf = Pixbuf::from_mut_slice(rgb, Colorspace::Rgb, false, 8, ow, oh, ow * 3);
        let quality = jpeg.map(|q| q.min(100).to_string());
        match &quality {
            None => pixbuf.save_to_bufferv("png", &[]),
            Some(q) => pixbuf.save_to_bufferv("jpeg", &[("quality", q.as_str())]),
        }
        .map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View { sx: 0.0, sy: 500.0, iw: 1000.0, ih: 800.0, cw: 985.0, ch: 785.0, dw: 1000.0, dh: 3000.0, dpr: 1.0 }
    }

    /// The visible page as WebKitGTK pictures it: without the scrollbars.
    const VIS: (f64, f64) = (985.0, 785.0);

    #[test]
    fn the_page_says_where_it_is_as_a_string_of_json() {
        let inner = serde_json::to_string(&serde_json::json!({"sx": 0, "sy": 500, "iw": 1000, "ih": 800, "cw": 985, "ch": 785, "dw": 1000, "dh": 3000, "dpr": 1})).unwrap();
        let s = serde_json::to_string(&inner).unwrap();
        assert_eq!(parse_view(&s), Some(view()));
        // An answer that is not the string, or a page without a size, is no answer.
        assert_eq!(parse_view(&inner), None);
        assert_eq!(parse_view("null"), None);
        assert_eq!(parse_view(&serde_json::to_string(r#"{"iw":0,"ih":0,"cw":0,"ch":0}"#).unwrap()), None);
    }

    #[test]
    fn no_clip_is_the_visible_page() {
        assert_eq!(part(None, &view(), VIS, true), Ok(Part { region: Region::Visible, x: 0.0, y: 0.0, w: 985.0, h: 785.0 }));
    }

    #[test]
    fn a_clip_within_the_visible_page_is_taken_from_it() {
        // Document coordinates: the page is scrolled 500 px down.
        let p = part(Some([10.0, 600.0, 200.0, 100.0]), &view(), VIS, true).unwrap();
        assert_eq!(p, Part { region: Region::Visible, x: 10.0, y: 100.0, w: 200.0, h: 100.0 });
        // Half a pixel out is rounding.
        assert_eq!(part(Some([-0.4, 499.6, 100.0, 100.0]), &view(), VIS, true).unwrap().region, Region::Visible);
    }

    #[test]
    fn the_viewport_with_its_scrollbars_is_the_visible_page_not_the_document() {
        // An agent's picture of the viewport: innerWidth × innerHeight at the scroll, wider than the page without its scrollbars.
        let p = part(Some([0.0, 500.0, 1000.0, 800.0]), &view(), VIS, true).unwrap();
        assert_eq!(p, Part { region: Region::Visible, x: 0.0, y: 0.0, w: 985.0, h: 785.0 });
    }

    #[test]
    fn a_clip_beyond_the_visible_page_comes_from_the_whole_document_where_there_is_one() {
        let p = part(Some([0.0, 100.0, 500.0, 300.0]), &view(), VIS, true).unwrap();
        assert_eq!(p, Part { region: Region::Document, x: 0.0, y: 100.0, w: 500.0, h: 300.0 });
        // Cut to the document.
        let p = part(Some([900.0, 2900.0, 300.0, 300.0]), &view(), VIS, true).unwrap();
        assert_eq!(p, Part { region: Region::Document, x: 900.0, y: 2900.0, w: 100.0, h: 100.0 });
        assert_eq!(region_size(&view(), Region::Document, VIS), (1000.0, 3000.0));
    }

    #[test]
    fn a_document_too_large_in_area_is_not_rendered_whole() {
        assert!(document_fits(1280.0, 16000.0, 1.0));
        assert!(!document_fits(2560.0, 16000.0, 2.0));
        assert!(!document_fits(8000.0, 8100.0, 1.0));
    }

    #[test]
    fn a_document_too_large_to_render_whole_gives_only_its_visible_part() {
        assert!(document_fits(1000.0, 16384.0, 1.0));
        assert!(!document_fits(1000.0, 9000.0, 2.0));
        assert!(!document_fits(20000.0, 800.0, 0.0));
        let long = View { dh: 40000.0, ..view() };
        // From 400 to 700 in the document; the visible page starts at 500.
        let p = part(Some([0.0, 400.0, 500.0, 300.0]), &long, VIS, true).unwrap();
        assert_eq!(p, Part { region: Region::Visible, x: 0.0, y: 0.0, w: 500.0, h: 200.0 });
        assert_eq!(part(Some([0.0, 20000.0, 500.0, 300.0]), &long, VIS, true), Err(TOO_LARGE));
    }

    #[test]
    fn without_the_whole_document_a_clip_is_cut_to_the_visible_page() {
        let p = part(Some([0.0, 400.0, 500.0, 300.0]), &view(), (1000.0, 800.0), false).unwrap();
        assert_eq!(p, Part { region: Region::Visible, x: 0.0, y: 0.0, w: 500.0, h: 200.0 });
        // Nothing of it on screen.
        assert_eq!(part(Some([0.0, 0.0, 500.0, 300.0]), &view(), (1000.0, 800.0), false), Err(OUTSIDE));
    }

    /// Packed native-endian cairo pixels from `0xAARRGGBB` values.
    fn cairo_px(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn a_rectangle_of_pixels_is_copied_out_packed() {
        // 3 × 2 pixels, rows 16 bytes apart (4 bytes of padding).
        let mut data = Vec::new();
        for row in [[1u32, 2, 3], [4, 5, 6]] {
            data.extend(cairo_px(&row));
            data.extend([0u8; 4]);
        }
        assert_eq!(copy_rect(&data, 16, [1, 0, 2, 2]), Some(cairo_px(&[2, 3, 5, 6])));
        assert_eq!(copy_rect(&data, 16, [0, 1, 3, 1]), Some(cairo_px(&[4, 5, 6])));
        // Not within the image.
        assert_eq!(copy_rect(&data, 16, [2, 1, 3, 1]), None);
        assert_eq!(copy_rect(&data, 16, [-1, 0, 1, 1]), None);
    }

    #[test]
    fn pixels_become_rgb_on_white() {
        // Opaque red and blue, a transparent pixel, and half-transparent green (premultiplied).
        let px = cairo_px(&[0xFFFF0000, 0xFF0000FF, 0x00000000, 0x80008000]);
        assert_eq!(rgb_scaled(&px, 4, 1, true, (4, 1)), vec![255, 0, 0, 0, 0, 255, 255, 255, 255, 127, 255, 127]);
        // Without alpha the top byte means nothing.
        assert_eq!(rgb_scaled(&cairo_px(&[0x00102030]), 1, 1, false, (1, 1)), vec![0x10, 0x20, 0x30]);
    }

    #[test]
    fn pixels_are_averaged_when_drawn_smaller() {
        // 2 × 2 of black and white: one grey pixel.
        let px = cairo_px(&[0xFF000000, 0xFFFFFFFF, 0xFFFFFFFF, 0xFF000000]);
        assert_eq!(rgb_scaled(&px, 2, 2, true, (1, 1)), vec![128, 128, 128]);
        // 3 → 2 across: each output pixel covers the source pixels it overlaps.
        let px = cairo_px(&[0xFF000000, 0xFF000000, 0xFFFFFFFF]);
        assert_eq!(rgb_scaled(&px, 3, 1, true, (2, 1)), vec![0, 0, 0, 128, 128, 128]);
        // Too few pixels for the size given: white.
        assert_eq!(rgb_scaled(&[], 2, 2, true, (1, 1)), vec![255, 255, 255]);
    }
    #[test]
    fn a_part_is_found_in_the_snapshots_own_pixels() {
        let p = Part { region: Region::Visible, x: 10.0, y: 100.0, w: 200.0, h: 100.0 };
        // A screen at twice the scale: the snapshot is twice the CSS size.
        assert_eq!(pixels(&p, (985.0, 800.0), (1970, 1600)), Some([20, 200, 400, 200]));
        assert_eq!(pixels(&p, (985.0, 800.0), (985, 800)), Some([10, 100, 200, 100]));
        // Cut to the snapshot.
        let edge = Part { region: Region::Visible, x: 900.0, y: 700.0, w: 200.0, h: 200.0 };
        assert_eq!(pixels(&edge, (985.0, 800.0), (985, 800)), Some([900, 700, 85, 100]));
        assert_eq!(pixels(&p, (0.0, 800.0), (985, 800)), None);
        assert_eq!(pixels(&p, (985.0, 800.0), (0, 0)), None);
    }

    #[test]
    fn a_picture_is_drawn_smaller_never_larger() {
        assert_eq!(scaled(2560, 1600, 0.5), (1280, 800));
        assert_eq!(scaled(800, 600, 1.0), (800, 600));
        assert_eq!(scaled(800, 600, 2.0), (800, 600));
        assert_eq!(scaled(800, 600, 0.0), (800, 600));
        assert_eq!(scaled(3, 1, 0.1), (1, 1));
    }

    #[test]
    fn a_part_is_found_in_the_views_points() {
        let v = view();
        let p = Part { region: Region::Visible, x: 10.0, y: 100.0, w: 200.0, h: 100.0 };
        // A flipped view the size of the page: points are CSS px; the width at half scale.
        assert_eq!(view_rect(&p, &v, (1000.0, 800.0), true, 0.5), ([10.0, 100.0, 200.0, 100.0], 100.0));
        // AppKit counts from the bottom.
        assert_eq!(view_rect(&p, &v, (1000.0, 800.0), false, 1.0), ([10.0, 600.0, 200.0, 100.0], 200.0));
        // A page zoomed to 200 %: half as many CSS px across the same view.
        let zoomed = View { iw: 500.0, ih: 400.0, ..v };
        assert_eq!(view_rect(&p, &zoomed, (1000.0, 800.0), true, 1.0), ([20.0, 200.0, 400.0, 200.0], 400.0));
    }

    /// AppKit encodes a snapshot's image as a PNG, or as a JPEG.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_image_is_encoded_as_png_or_jpeg() {
        use objc2::AllocAnyThread;
        // A 2 × 2 PNG: red and blue columns.
        const TINY: [u8; 75] = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
            0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd, 0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf,
            0xc0, 0xc0, 0xc0, 0xf0, 0x9f, 0x01, 0x42, 0x01, 0x00, 0x1b, 0xf2, 0x03, 0xfd, 0xcc, 0x41, 0xf7, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
            0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let data = objc2_foundation::NSData::with_bytes(&TINY);
        let image = objc2_app_kit::NSImage::initWithData(objc2_app_kit::NSImage::alloc(), &data).expect("AppKit reads the PNG");
        let png = platform::encode(&image, None).expect("a PNG");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let jpeg = platform::encode(&image, Some(80)).expect("a JPEG");
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
    }}
