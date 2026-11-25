//! Where things are. Everything here is in physical pixels: the helper runs per-monitor DPI aware (v2), so window rectangles
//! (`DWMWA_EXTENDED_FRAME_BOUNDS`), monitor rectangles (`DXGI_OUTPUT_DESC`) and the virtual desktop are all in one pixel space, whatever
//! each monitor's scale. A monitor may sit left of or above the primary one: its origin is negative.
//!
//! The phone speaks in the picture's own pixels (the source: what was captured, before any scaling for the encoder). A point of the
//! source becomes a point of the screen through the target's rectangle, and that becomes `SendInput`'s absolute units (0..=65535 across
//! the whole virtual desktop, `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    /// From a Win32 `RECT`'s edges.
    pub fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect { x: left, y: top, w: (right - left).max(0), h: (bottom - top).max(0) }
    }

    pub fn right(self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(self) -> i32 {
        self.y + self.h
    }

    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn intersect(self, o: Rect) -> Option<Rect> {
        let r = Rect::from_edges(self.x.max(o.x), self.y.max(o.y), self.right().min(o.right()), self.bottom().min(o.bottom()));
        (!r.is_empty()).then_some(r)
    }

    pub fn union(self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        Rect::from_edges(self.x.min(o.x), self.y.min(o.y), self.right().max(o.right()), self.bottom().max(o.bottom()))
    }

    /// The nearest point inside.
    pub fn clamp(self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(self.x, (self.right() - 1).max(self.x)), y.clamp(self.y, (self.bottom() - 1).max(self.y)))
    }
}

/// A monitor as DXGI and the DPI API see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Monitor {
    /// `\\.\DISPLAY1`: what the target id names.
    pub name: String,
    pub rect: Rect,
    /// Effective DPI: 96 is 100 %, 144 is 150 %, 192 is 200 %.
    pub dpi: u32,
    pub primary: bool,
}

impl Monitor {
    pub fn scale_percent(&self) -> u32 {
        scale_percent(self.dpi)
    }
}

pub fn scale_percent(dpi: u32) -> u32 {
    (dpi.max(1) * 100 + 48) / 96
}

/// The rectangle around every monitor (`SM_XVIRTUALSCREEN` … in a per-monitor aware process).
pub fn virtual_desktop(monitors: &[Monitor]) -> Rect {
    monitors.iter().fold(Rect::default(), |acc, m| acc.union(m.rect))
}

/// The monitor a screen point is on, else the nearest one (a window partly off every monitor).
pub fn monitor_at(monitors: &[Monitor], x: i32, y: i32) -> Option<&Monitor> {
    monitors.iter().find(|m| m.rect.contains(x, y)).or_else(|| {
        monitors.iter().min_by_key(|m| {
            let (cx, cy) = m.rect.clamp(x, y);
            i64::from(cx - x).pow(2) + i64::from(cy - y).pow(2)
        })
    })
}

/// A point of the source picture (`src_w`×`src_h`, possibly fractional, as a finger lands) → the screen pixel under it, given where the
/// target is now. The target may have been resized since the picture was taken: the point keeps its place in proportion. Always inside.
pub fn source_to_screen(sx: f64, sy: f64, src_w: u32, src_h: u32, target: Rect) -> (i32, i32) {
    if target.is_empty() || src_w == 0 || src_h == 0 {
        return (target.x, target.y);
    }
    let fx = (sx / f64::from(src_w)).clamp(0.0, 1.0);
    let fy = (sy / f64::from(src_h)).clamp(0.0, 1.0);
    let x = target.x + (fx * f64::from(target.w)).floor() as i32;
    let y = target.y + (fy * f64::from(target.h)).floor() as i32;
    target.clamp(x, y)
}

/// A screen pixel → the source picture's pixel, for the cursor: None when it is outside the target.
pub fn screen_to_source(x: i32, y: i32, target: Rect, src_w: u32, src_h: u32) -> Option<(i32, i32)> {
    if !target.contains(x, y) || src_w == 0 || src_h == 0 {
        return None;
    }
    let sx = (i64::from(x - target.x) * i64::from(src_w) / i64::from(target.w)) as i32;
    let sy = (i64::from(y - target.y) * i64::from(src_h) / i64::from(target.h)) as i32;
    Some((sx, sy))
}

/// A screen pixel → `SendInput`'s absolute units over the virtual desktop. Windows maps a unit `n` to pixel `floor(n · width / 65536)`, so
/// the smallest `n` that lands on pixel `x` is `ceil(x · 65536 / width)`: that is the one sent, and it never lands on the next pixel.
pub fn to_absolute(x: i32, y: i32, desktop: Rect) -> (i32, i32) {
    let unit = |v: i32, origin: i32, size: i32| -> i32 {
        if size <= 0 {
            return 0;
        }
        let off = i64::from((v - origin).clamp(0, size - 1));
        ((off * 65536 + i64::from(size) - 1) / i64::from(size)).min(65535) as i32
    };
    (unit(x, desktop.x, desktop.w), unit(y, desktop.y, desktop.h))
}

/// The pixel a unit lands on, as Windows works it out (for tests, and to check `to_absolute`).
pub fn from_absolute(nx: i32, ny: i32, desktop: Rect) -> (i32, i32) {
    let px = |n: i32, origin: i32, size: i32| origin + (i64::from(n) * i64::from(size) / 65536) as i32;
    (px(nx, desktop.x, desktop.w), px(ny, desktop.y, desktop.h))
}

/// The encoder's picture for a source at `scale` percent: even sides (NV12 and H.264 need them), never larger than the source, and no taller
/// than `max_lines`; `min_lines` (a window's floor) wins over the scale but not over the source or `max_lines`.
pub fn scaled(src_w: u32, src_h: u32, scale: u8, min_lines: u32, max_lines: u32) -> (u32, u32) {
    fitted(src_w, src_h, (u64::from(src_h) * u64::from(scale.clamp(1, 100)) / 100) as u32, min_lines, max_lines)
}

/// The same for a picture `lines` tall before the bounds.
pub fn fitted(src_w: u32, src_h: u32, lines: u32, min_lines: u32, max_lines: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (0, 0);
    }
    let h = u64::from(lines).max(u64::from(min_lines)).min(u64::from(max_lines)).min(u64::from(src_h));
    let w = (u64::from(src_w) * h / u64::from(src_h)).max(2);
    let even = |v: u64| ((v & !1).max(2)) as u32;
    (even(w), even(h.max(2)))
}

/// What changed in a desktop duplication frame, as WebRTC's `DxgiOutputDuplicator::DoDetectUpdatedRegion` has it: every dirty rectangle, and
/// for each move both where the pixels came from and where they went (`(source x, source y, destination)`); a "move" that moved nothing is
/// skipped (DXGI reports some). Empty rectangles are left out.
pub fn updated_region(dirty: &[Rect], moves: &[(i32, i32, Rect)]) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::with_capacity(dirty.len() + moves.len() * 2);
    for &(sx, sy, dest) in moves {
        if sx == dest.x && sy == dest.y {
            continue;
        }
        out.push(Rect::new(sx, sy, dest.w, dest.h));
        out.push(dest);
    }
    out.extend(dirty.iter().copied());
    out.retain(|r| !r.is_empty());
    out
}

/// What changed in a frame of a desktop duplication that may just have been made (again): the first frame after that counts as all changed
/// (None), whatever its metadata says, since the desktop may have changed while there was no duplication (UAC, the lock screen).
pub fn after_reopen(fresh: &mut bool, dirty: Option<Vec<Rect>>) -> Option<Vec<Rect>> {
    if std::mem::take(fresh) {
        None
    } else {
        dirty
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_frame_after_the_duplication_is_made_again_counts_as_all_changed() {
        let mut fresh = true;
        let small = Some(vec![Rect::new(0, 0, 10, 10)]);
        assert_eq!(after_reopen(&mut fresh, small.clone()), None, "whatever its metadata says");
        assert_eq!(after_reopen(&mut fresh, small.clone()), small, "then what DXGI says");
    }
    #[test]
    fn a_scroll_updates_where_the_pixels_came_from_and_went_and_an_unmoved_move_nothing() {
        let dirty = [Rect::new(0, 900, 1920, 180), Rect::new(5, 5, 0, 10)];
        let moves = [(0, 100, Rect::new(0, 60, 1920, 840)), (10, 10, Rect::new(10, 10, 50, 50))];
        assert_eq!(updated_region(&dirty, &moves), [Rect::new(0, 100, 1920, 840), Rect::new(0, 60, 1920, 840), Rect::new(0, 900, 1920, 180)]);
        assert!(updated_region(&[], &[]).is_empty());
    }

    fn monitors() -> Vec<Monitor> {
        vec![
            // A 4K monitor at 200 % on the right, the primary.
            Monitor { name: r"\\.\DISPLAY1".into(), rect: Rect::new(0, 0, 3840, 2160), dpi: 192, primary: true },
            // A 1080p monitor at 100 % to its left, its top 300 px higher: negative origin on both axes.
            Monitor { name: r"\\.\DISPLAY2".into(), rect: Rect::new(-1920, -300, 1920, 1080), dpi: 96, primary: false },
            // A laptop panel at 150 % below the primary.
            Monitor { name: r"\\.\DISPLAY3".into(), rect: Rect::new(0, 2160, 1920, 1200), dpi: 144, primary: false },
        ]
    }

    #[test]
    fn the_virtual_desktop_spans_every_monitor_with_negative_origins() {
        let vd = virtual_desktop(&monitors());
        assert_eq!(vd, Rect::new(-1920, -300, 5760, 3660));
        assert_eq!(monitors().iter().map(Monitor::scale_percent).collect::<Vec<_>>(), [200, 100, 150]);
        assert_eq!(scale_percent(120), 125);
        assert_eq!(scale_percent(168), 175);
    }

    #[test]
    fn every_pixel_of_every_monitor_round_trips_through_absolute_units() {
        let ms = monitors();
        let vd = virtual_desktop(&ms);
        for m in &ms {
            for (x, y) in [(m.rect.x, m.rect.y), (m.rect.right() - 1, m.rect.bottom() - 1), (m.rect.x + m.rect.w / 2, m.rect.y + 7), (m.rect.x + 1, m.rect.bottom() - 2)] {
                let (nx, ny) = to_absolute(x, y, vd);
                assert!((0..=65535).contains(&nx) && (0..=65535).contains(&ny), "{x},{y} -> {nx},{ny}");
                assert_eq!(from_absolute(nx, ny, vd), (x, y), "{} at {x},{y}", m.name);
            }
        }
        // Outside the desktop: the nearest edge, never a wrap.
        assert_eq!(to_absolute(-5000, -5000, vd), (0, 0));
        assert_eq!(to_absolute(99_999, 99_999, vd), to_absolute(vd.right() - 1, vd.bottom() - 1, vd));
    }

    #[test]
    fn a_point_of_the_picture_lands_on_the_target_where_it_is_now() {
        // A window on the left monitor, captured at 1200×800; the phone taps the middle and the corners.
        let win = Rect::new(-1800, -200, 1200, 800);
        assert_eq!(source_to_screen(600.0, 400.0, 1200, 800, win), (-1200, 200));
        assert_eq!(source_to_screen(0.0, 0.0, 1200, 800, win), (-1800, -200));
        assert_eq!(source_to_screen(1200.0, 800.0, 1200, 800, win), (-601, 599), "the far edge stays inside");
        assert_eq!(source_to_screen(-50.0, 5000.0, 1200, 800, win), (-1800, 599));
        // The window grew to 2400×1600 since the frame: the same point keeps its place in proportion.
        let bigger = Rect::new(-1800, -200, 2400, 1600);
        assert_eq!(source_to_screen(600.0, 400.0, 1200, 800, bigger), (-600, 600));
        // A 4K monitor at 200 %: physical pixels throughout, the scale changes nothing.
        let m = monitors()[0].rect;
        assert_eq!(source_to_screen(1920.5, 1080.0, 3840, 2160, m), (1920, 1080));
    }

    #[test]
    fn the_cursor_is_found_in_the_picture_and_lost_outside_it() {
        let win = Rect::new(100, -300, 800, 600);
        assert_eq!(screen_to_source(500, 0, win, 800, 600), Some((400, 300)));
        assert_eq!(screen_to_source(500, 0, win, 400, 300), Some((200, 150)), "a picture scaled down by half");
        assert_eq!(screen_to_source(99, 0, win, 800, 600), None);
        assert_eq!(screen_to_source(900, 0, win, 800, 600), None);
    }

    #[test]
    fn a_point_off_every_monitor_belongs_to_the_nearest() {
        let ms = monitors();
        assert_eq!(monitor_at(&ms, -10, 500).map(|m| m.name.as_str()), Some(r"\\.\DISPLAY2"));
        assert_eq!(monitor_at(&ms, 100, 3000).map(|m| m.name.as_str()), Some(r"\\.\DISPLAY3"));
        assert_eq!(monitor_at(&ms, 3000, 3000).map(|m| m.name.as_str()), Some(r"\\.\DISPLAY1"), "below the 4K one, right of the laptop: nearest edge");
    }

    #[test]
    fn the_encoder_picture_is_even_never_upscaled_and_held_to_its_floor_and_cap() {
        assert_eq!(scaled(1920, 1080, 100, 0, 1080), (1920, 1080));
        assert_eq!(scaled(1920, 1080, 75, 0, 1080), (1440, 810));
        assert_eq!(scaled(1920, 1080, 50, 540, 1080), (960, 540));
        assert_eq!(scaled(1920, 1080, 50, 0, 1080), (960, 540));
        assert_eq!(scaled(1920, 1080, 25, 540, 1080), (960, 540), "a window never under 540 lines");
        assert_eq!(scaled(3840, 2160, 100, 0, 1080), (1920, 1080), "a load cap");
        assert_eq!(scaled(1001, 501, 100, 540, 1080), (1000, 500), "a small window: its own size, made even, never raised to the floor");
        assert_eq!(scaled(0, 0, 100, 0, 1080), (0, 0));
    }
}
