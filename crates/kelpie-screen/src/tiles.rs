//! Crisp text over the video ("paint-over"). H.264 at 4:2:0 and a bitrate a phone link can carry smears one-pixel coloured edges, which is
//! most of what an IDE shows. So the picture is cut into 64×64 cells; a cell's pixels are hashed on every frame captured, and once a run of
//! cells has stood still for `STABLE_MS` it is sent once more as a lossless (PNG) or high-quality (JPEG) tile at the source's own size,
//! which the phone draws over the video. When a painted cell changes, the next video frame carries its rectangle as "unpaint": the phone
//! drops the tile as it draws that frame, so a stale tile never covers a live change.
//!
//! The cursor is not in the captured pixels (the phone draws it), so moving it dirties nothing. What is here is the bookkeeping, tested
//! without a GPU; the readback and the image encoding are the native side's (`win`) and `tile_image`'s.

use crate::geom::Rect;

pub const TILE: u32 = 64;
/// How long cells must stand still before they are painted.
pub const STABLE_MS: u64 = 300;
/// The largest tile: 8 cells across, 4 down (512×256).
pub const MAX_RUN: usize = 8;
pub const MAX_ROWS: usize = 4;

/// A hash of one cell of a BGRA picture with rows `stride` bytes apart. Different pixels give a different hash but for a chance of 2⁻⁶⁴.
pub fn cell_hash(px: &[u8], stride: usize, x0: usize, y0: usize, w: usize, h: usize) -> u64 {
    const K: u64 = 0x0000_0100_0000_01B3;
    let mut hash: u64 = 0x9E37_79B9_7F4A_7C15 ^ ((w as u64) << 32) ^ (h as u64);
    for y in y0..y0 + h {
        let start = y * stride + x0 * 4;
        let row = &px[start..start + w * 4];
        let mut chunks = row.chunks_exact(8);
        for c in &mut chunks {
            let v = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
            hash = (hash ^ v).wrapping_mul(K).rotate_left(29);
        }
        for &b in chunks.remainder() {
            hash = (hash ^ u64::from(b)).wrapping_mul(K);
        }
        hash = hash.rotate_left(7) ^ (y as u64);
    }
    hash
}

#[derive(Debug)]
pub struct Grid {
    w: u32,
    h: u32,
    cols: usize,
    rows: usize,
    hash: Vec<u64>,
    changed: Vec<u64>,
    painted: Vec<bool>,
    seen: bool,
}

impl Grid {
    pub fn new(w: u32, h: u32, now: u64) -> Grid {
        let cols = w.div_ceil(TILE) as usize;
        let rows = h.div_ceil(TILE) as usize;
        let n = cols * rows;
        Grid { w, h, cols, rows, hash: vec![0; n], changed: vec![now; n], painted: vec![false; n], seen: false }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn cell_rect(&self, col: usize, row: usize) -> Rect {
        let x = col as u32 * TILE;
        let y = row as u32 * TILE;
        Rect::new(x as i32, y as i32, TILE.min(self.w - x) as i32, TILE.min(self.h - y) as i32)
    }

    /// The cells a rectangle touches.
    fn cells_in(&self, r: Rect) -> impl Iterator<Item = (usize, usize)> + '_ {
        let clip = r.intersect(Rect::new(0, 0, self.w as i32, self.h as i32));
        let (c0, c1, r0, r1) = match clip {
            Some(c) => (c.x as usize / TILE as usize, (c.right() as usize - 1) / TILE as usize, c.y as usize / TILE as usize, (c.bottom() as usize - 1) / TILE as usize),
            None => (1, 0, 1, 0),
        };
        (r0..=r1).flat_map(move |row| (c0..=c1).map(move |col| (col, row)))
    }

    /// A captured picture (BGRA, `stride` bytes a row, this grid's size). `dirty`, when the capture says what changed (DXGI), limits the
    /// cells looked at. The painted regions that changed are returned, to be unpainted with the next video frame (merged along rows).
    pub fn update(&mut self, px: &[u8], stride: usize, now: u64, dirty: Option<&[Rect]>) -> Vec<Rect> {
        assert!(stride >= self.w as usize * 4 && px.len() >= stride * (self.h as usize).saturating_sub(1) + self.w as usize * 4, "a picture smaller than its grid");
        let first = !self.seen;
        self.seen = true;
        let cells: Vec<(usize, usize)> = match dirty {
            Some(rects) if !first => {
                let mut v: Vec<(usize, usize)> = rects.iter().flat_map(|r| self.cells_in(*r)).collect();
                v.sort_unstable_by_key(|(c, r)| (*r, *c));
                v.dedup();
                v
            }
            _ => (0..self.rows).flat_map(|row| (0..self.cols).map(move |col| (col, row))).collect(),
        };
        let mut unpaint: Vec<Rect> = Vec::new();
        for (col, row) in cells {
            let i = row * self.cols + col;
            let r = self.cell_rect(col, row);
            let h = cell_hash(px, stride, r.x as usize, r.y as usize, r.w as usize, r.h as usize);
            if first {
                self.hash[i] = h;
                continue;
            }
            if h != self.hash[i] {
                self.hash[i] = h;
                self.changed[i] = now;
                if std::mem::take(&mut self.painted[i]) {
                    match unpaint.last_mut() {
                        Some(last) if last.y == r.y && last.h == r.h && last.right() == r.x => last.w += r.w,
                        _ => unpaint.push(r),
                    }
                }
            }
        }
        unpaint
    }

    /// Whether every cell has stood still for `STABLE_MS`.
    pub fn all_still(&self, now: u64) -> bool {
        self.changed.iter().all(|c| now.saturating_sub(*c) >= STABLE_MS)
    }

    /// Whether any cell waits to be painted now.
    pub fn any_due(&self, now: u64) -> bool {
        self.seen && (0..self.hash.len()).any(|i| !self.painted[i] && now.saturating_sub(self.changed[i]) >= STABLE_MS)
    }

    /// Still regions to paint now, up to `max_cells` cells in all, as rectangles of up to `MAX_RUN`×`MAX_ROWS` cells. They count as painted
    /// from now on (a tile that could not be sent is undone with `forget_painted`).
    pub fn due(&mut self, now: u64, max_cells: usize) -> Vec<Rect> {
        if !self.seen {
            return Vec::new();
        }
        let ready = |g: &Grid, i: usize| !g.painted[i] && now.saturating_sub(g.changed[i]) >= STABLE_MS;
        let mut out = Vec::new();
        let mut budget = max_cells;
        for row in 0..self.rows {
            let mut col = 0;
            while col < self.cols && budget > 0 {
                if !ready(self, row * self.cols + col) {
                    col += 1;
                    continue;
                }
                let mut run = 1;
                while col + run < self.cols && run < MAX_RUN && run < budget && ready(self, row * self.cols + col + run) {
                    run += 1;
                }
                let mut rows = 1;
                while row + rows < self.rows && rows < MAX_ROWS && (rows + 1) * run <= budget && (0..run).all(|k| ready(self, (row + rows) * self.cols + col + k)) {
                    rows += 1;
                }
                for dr in 0..rows {
                    for k in 0..run {
                        self.painted[(row + dr) * self.cols + col + k] = true;
                    }
                }
                budget -= run * rows;
                let first = self.cell_rect(col, row);
                let last = self.cell_rect(col + run - 1, row + rows - 1);
                out.push(Rect::from_edges(first.x, first.y, last.right(), last.bottom()));
                col += run;
            }
        }
        out
    }

    /// The phone holds no tile any more (it was told to drop them all, or one could not be sent): every still cell is painted again.
    pub fn forget_painted(&mut self) {
        self.painted.fill(false);
    }

    /// One region was not sent after all.
    pub fn unmark(&mut self, r: Rect) {
        let cells: Vec<(usize, usize)> = self.cells_in(r).collect();
        for (col, row) in cells {
            self.painted[row * self.cols + col] = false;
        }
    }

    #[cfg(test)]
    fn painted_count(&self) -> usize {
        self.painted.iter().filter(|p| **p).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Picture {
        w: usize,
        h: usize,
        stride: usize,
        px: Vec<u8>,
    }

    impl Picture {
        fn new(w: usize, h: usize) -> Picture {
            // A stride with padding, as a mapped texture has.
            let stride = w * 4 + 64;
            Picture { w, h, stride, px: vec![0x20; stride * h] }
        }

        fn set(&mut self, x: usize, y: usize, v: u8) {
            let i = y * self.stride + x * 4;
            self.px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }

    #[test]
    fn a_change_dirties_its_own_cell_only_and_the_hash_sees_one_pixel() {
        let mut p = Picture::new(200, 130);
        let mut g = Grid::new(200, 130, 0);
        assert_eq!((g.cols, g.rows), (4, 3));
        assert!(g.update(&p.px, p.stride, 0, None).is_empty(), "the first picture is the baseline");
        p.set(70, 70, 0xff);
        g.update(&p.px, p.stride, 100, None);
        let changed: Vec<usize> = (0..12).filter(|i| g.changed[*i] == 100).collect();
        assert_eq!(changed, [5], "column 1, row 1");
        // The padding past each row is not the picture: changing it changes nothing.
        let pad = p.stride - 1;
        p.px[pad] ^= 0xff;
        g.update(&p.px, p.stride, 200, None);
        assert!((0..12).all(|i| g.changed[i] != 200));
        assert_ne!(cell_hash(&[1u8; 64 * 4], 64 * 4, 0, 0, 64, 1), cell_hash(&[2u8; 64 * 4], 64 * 4, 0, 0, 64, 1));
    }

    #[test]
    fn edge_cells_are_clipped_to_the_picture() {
        let g = Grid::new(100, 70, 0);
        assert_eq!(g.cell_rect(1, 1), Rect::new(64, 64, 36, 6));
        assert_eq!(g.cell_rect(0, 0), Rect::new(0, 0, 64, 64));
    }

    #[test]
    fn still_regions_are_painted_once_in_large_tiles_and_a_change_unpaints_them() {
        let mut p = Picture::new(640, 256);
        let mut g = Grid::new(640, 256, 0);
        g.update(&p.px, p.stride, 0, None);
        assert!(g.due(STABLE_MS - 1, 1000).is_empty(), "not still for long enough");
        let tiles = g.due(STABLE_MS, 1000);
        // 10 columns × 4 rows: one 8×4 tile and one 2×4 tile.
        assert_eq!(tiles, [Rect::new(0, 0, 512, 256), Rect::new(512, 0, 128, 256)]);
        assert_eq!(g.painted_count(), 40);
        assert!(g.due(STABLE_MS + 1000, 1000).is_empty(), "painted once");
        // Two neighbouring cells of the second row change: unpainted together, the rest stays painted.
        p.set(10, 70, 1);
        p.set(70, 70, 1);
        let unpaint = g.update(&p.px, p.stride, 2000, None);
        assert_eq!(unpaint, [Rect::new(0, 64, 128, 64)]);
        assert_eq!(g.painted_count(), 38);
        assert!(!g.all_still(2000) && g.all_still(2000 + STABLE_MS));
        assert_eq!(g.due(2000 + STABLE_MS, 1000), [Rect::new(0, 64, 128, 64)], "painted again once still");
        // A change in an unpainted cell unpaints nothing.
        let mut g2 = Grid::new(640, 256, 0);
        g2.update(&p.px, p.stride, 0, None);
        p.set(300, 10, 9);
        assert!(g2.update(&p.px, p.stride, 10, None).is_empty());
    }

    #[test]
    fn a_budget_limits_how_much_is_painted_at_once() {
        let p = Picture::new(640, 256);
        let mut g = Grid::new(640, 256, 0);
        g.update(&p.px, p.stride, 0, None);
        let first = g.due(STABLE_MS, 6);
        assert_eq!(first, [Rect::new(0, 0, 384, 64)], "six cells");
        let next = g.due(STABLE_MS, 6);
        let cells: i32 = next.iter().map(|r| ((r.w + 63) / 64) * ((r.h + 63) / 64)).sum();
        assert_eq!(cells, 6);
        assert!(g.any_due(STABLE_MS));
        g.forget_painted();
        assert_eq!(g.painted_count(), 0);
        let all = g.due(STABLE_MS, 1000);
        g.unmark(all[0]);
        assert_eq!(g.painted_count(), 40 - 32);
    }

    #[test]
    fn what_the_capture_says_changed_is_all_that_is_looked_at() {
        let mut p = Picture::new(256, 128);
        let mut g = Grid::new(256, 128, 0);
        g.update(&p.px, p.stride, 0, None);
        p.set(10, 10, 5);
        p.set(200, 100, 5);
        // DXGI said only the top-left changed: the other change is not seen now (it will be, with its own dirty rectangle).
        g.update(&p.px, p.stride, 50, Some(&[Rect::new(0, 0, 20, 20)]));
        assert_eq!(g.changed[0], 50);
        assert_eq!(g.changed[3 + 4], 0);
        g.update(&p.px, p.stride, 60, Some(&[Rect::new(190, 90, 400, 400)]));
        assert_eq!(g.changed[7], 60, "clipped to the picture");
    }
}
