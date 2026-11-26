//! BGRA → NV12 at the encoder's size on the CPU, for a computer whose graphics have no video processor (a virtual machine, a cloud desktop):
//! a bilinear scale (a box average on an axis reduced more than twice, so that a thin line is never skipped), and BT.709 limited range, the
//! colours the GPU path gives (`win/convert.rs`). The scaler makes its own buffers once; the NV12 it writes into is the caller's.
//!
//! NV12 here is the Y plane (`w` bytes a row, `h` rows) followed by the interleaved CbCr plane (a pair for every 2×2 block, `chroma_w(w) * 2`
//! bytes a row, `(h + 1) / 2` rows): for the even sizes an encoder takes, that is `w * h * 3 / 2` bytes with both planes `w` bytes a row.
//! Chroma is sited as H.264 assumes by default (MPEG-2's): on the left column of its pair, between its two rows.

/// Weights are in 1/256.
const ONE: u32 = 256;

/// Full-range RGB to BT.709 limited range, in 1/65536: Y = 16 + 219/255 (0.2126 R + 0.7152 G + 0.0722 B), Cb and Cr scaled by 224/255
/// (each row of Cb and Cr adds up to zero, so a grey has no colour).
const Y_RGB: [i32; 3] = [11966, 40254, 4064];
const CB_RGB: [i32; 3] = [-6596, -22188, 28784];
const CR_RGB: [i32; 3] = [28784, -26145, -2639];

/// The chroma pairs in a row of `w` pixels.
pub fn chroma_w(w: u32) -> usize {
    (w as usize).div_ceil(2)
}

/// The bytes of an NV12 picture of `w`×`h`.
pub fn nv12_len(w: u32, h: u32) -> usize {
    w as usize * h as usize + chroma_w(w) * 2 * (h as usize).div_ceil(2)
}

/// For each output pixel of one axis, the source pixels it is made of and their weights (adding up to `ONE`).
#[derive(Debug)]
struct Axis {
    at: Vec<usize>,
    weight: Vec<u32>,
    /// Output `i`'s taps are `start[i]..start[i + 1]`.
    start: Vec<usize>,
}

impl Axis {
    fn new(src: u32, out: u32) -> Axis {
        let mut axis = Axis { at: Vec::new(), weight: Vec::new(), start: vec![0] };
        let (s, o) = (u64::from(src), u64::from(out));
        let last = src as usize - 1;
        for i in 0..o {
            if s > 2 * o {
                // A box: each source pixel by how much of it the output pixel covers (positions in 1/256 of a pixel).
                let (a, b) = (i * s * 256 / o, (i + 1) * s * 256 / o);
                let first = axis.at.len();
                for p in (a >> 8)..b.div_ceil(256) {
                    let covered = b.min((p + 1) << 8) - a.max(p << 8);
                    axis.at.push((p as usize).min(last));
                    axis.weight.push((covered * u64::from(ONE) / (b - a)) as u32);
                }
                // Rounding leaves the sum a little short: the rest goes to the largest weight.
                let short = ONE - axis.weight[first..].iter().sum::<u32>();
                if let Some(w) = axis.weight[first..].iter_mut().max() {
                    *w += short;
                }
            } else {
                // Bilinear: pixel centres mapped onto each other (as a GPU sampler does), clamped at the edges.
                let pos = ((2 * i + 1) * s * u64::from(ONE) / (2 * o)).saturating_sub(u64::from(ONE / 2));
                let at = (pos >> 8) as usize;
                let frac = (pos as u32) & (ONE - 1);
                if at >= last || frac == 0 {
                    // On a pixel (the same size, an edge): that pixel alone.
                    axis.at.push(at.min(last));
                    axis.weight.push(ONE);
                } else {
                    axis.at.extend([at, at + 1]);
                    axis.weight.extend([ONE - frac, frac]);
                }
            }
            axis.start.push(axis.at.len());
        }
        axis
    }

    #[cfg(test)]
    fn taps(&self, i: usize) -> impl Iterator<Item = (usize, u32)> + '_ {
        let r = self.start[i]..self.start[i + 1];
        self.at[r.clone()].iter().copied().zip(self.weight[r].iter().copied())
    }
}

pub struct Scaler {
    pub src: (u32, u32),
    pub out: (u32, u32),
    cols: Axis,
    rows: Axis,
    /// One source-wide row of R, G, B mixed from the source rows of an output row (×256).
    mixed: Vec<u32>,
    /// Two output rows, scaled, as R, G, B: the pair a row of chroma is made from.
    scratch: Vec<u8>,
}

impl Scaler {
    /// A scaler from `src` to `out` (each side at least 1).
    pub fn new(src: (u32, u32), out: (u32, u32)) -> Scaler {
        let (src, out) = ((src.0.max(1), src.1.max(1)), (out.0.max(1), out.1.max(1)));
        Scaler { src, out, cols: Axis::new(src.0, out.0), rows: Axis::new(src.1, out.1), mixed: vec![0; src.0 as usize * 3], scratch: vec![0; out.0 as usize * 3 * 2] }
    }

    /// The bytes `convert` writes.
    pub fn out_len(&self) -> usize {
        nv12_len(self.out.0, self.out.1)
    }

    /// `bgra` (`src`, rows `stride` bytes apart) scaled and converted into `nv12` (`out_len` bytes at least).
    pub fn convert(&mut self, bgra: &[u8], stride: usize, nv12: &mut [u8]) -> Result<(), &'static str> {
        let (sw, sh) = (self.src.0 as usize, self.src.1 as usize);
        let (ow, oh) = (self.out.0 as usize, self.out.1 as usize);
        if stride < sw * 4 || bgra.len() < stride * (sh - 1) + sw * 4 {
            return Err("the picture is smaller than its size says");
        }
        if nv12.len() < self.out_len() {
            return Err("the NV12 buffer is too small");
        }
        let (y_plane, uv_plane) = nv12.split_at_mut(ow * oh);
        let uv_stride = chroma_w(self.out.0) * 2;
        let (first, second) = self.scratch.split_at_mut(ow * 3);
        for pair in 0..oh.div_ceil(2) {
            let top = pair * 2;
            // A last row of its own (an odd height) is its own pair.
            let bottom = (top + 1).min(oh - 1);
            scale_row(bgra, stride, sw, &self.cols, &self.rows, top, &mut self.mixed, first);
            if bottom != top {
                scale_row(bgra, stride, sw, &self.cols, &self.rows, bottom, &mut self.mixed, second);
            } else {
                second.copy_from_slice(first);
            }
            luma_row(first, &mut y_plane[top * ow..(top + 1) * ow]);
            if bottom != top {
                luma_row(second, &mut y_plane[bottom * ow..(bottom + 1) * ow]);
            }
            chroma_row(first, second, ow, &mut uv_plane[pair * uv_stride..(pair + 1) * uv_stride]);
        }
        Ok(())
    }
}

/// Output row `y` as R, G, B: its source rows mixed (into `mixed`), then each output pixel's columns. Plain index loops, which stay fast in a
/// debug build too.
#[allow(clippy::too_many_arguments)]
fn scale_row(bgra: &[u8], stride: usize, sw: usize, cols: &Axis, rows: &Axis, y: usize, mixed: &mut [u32], out: &mut [u8]) {
    mixed.fill(0);
    for k in rows.start[y]..rows.start[y + 1] {
        let wy = rows.weight[k];
        if wy == 0 {
            continue;
        }
        let line = &bgra[rows.at[k] * stride..rows.at[k] * stride + sw * 4];
        let mixed = &mut mixed[..sw * 3];
        for x in 0..sw {
            // BGRA in, RGB out.
            mixed[x * 3] += u32::from(line[x * 4 + 2]) * wy;
            mixed[x * 3 + 1] += u32::from(line[x * 4 + 1]) * wy;
            mixed[x * 3 + 2] += u32::from(line[x * 4]) * wy;
        }
    }
    let ow = out.len() / 3;
    for x in 0..ow {
        let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
        for k in cols.start[x]..cols.start[x + 1] {
            let (at, w) = (cols.at[k] * 3, cols.weight[k]);
            r += mixed[at] * w;
            g += mixed[at + 1] * w;
            b += mixed[at + 2] * w;
        }
        out[x * 3] = ((r + (1 << 15)) >> 16) as u8;
        out[x * 3 + 1] = ((g + (1 << 15)) >> 16) as u8;
        out[x * 3 + 2] = ((b + (1 << 15)) >> 16) as u8;
    }
}
fn luma_row(rgb: &[u8], y: &mut [u8]) {
    for (px, out) in rgb.chunks_exact(3).zip(y.iter_mut()) {
        let v = Y_RGB[0] * i32::from(px[0]) + Y_RGB[1] * i32::from(px[1]) + Y_RGB[2] * i32::from(px[2]) + (16 << 16) + (1 << 15);
        *out = (v >> 16).clamp(0, 255) as u8;
    }
}

/// Cb and Cr of each pair of columns, sited on its left column: that column, half of each neighbour ([1, 2, 1], clamped at the edges), the
/// two rows averaged.
fn chroma_row(first: &[u8], second: &[u8], w: usize, uv: &mut [u8]) {
    for (i, pair) in uv.chunks_exact_mut(2).enumerate() {
        let (l, c, r) = ((2 * i).saturating_sub(1) * 3, 2 * i * 3, (2 * i + 1).min(w - 1) * 3);
        let mut sum = [0i32; 3];
        for ch in 0..3 {
            sum[ch] = i32::from(first[l + ch]) + 2 * i32::from(first[c + ch]) + i32::from(first[r + ch]) + i32::from(second[l + ch]) + 2 * i32::from(second[c + ch]) + i32::from(second[r + ch]);
        }
        // Eight weights summed: 1/65536 · 1/8 is 1/2^19.
        let cb = CB_RGB[0] * sum[0] + CB_RGB[1] * sum[1] + CB_RGB[2] * sum[2] + (128 << 19) + (1 << 18);
        let cr = CR_RGB[0] * sum[0] + CR_RGB[1] * sum[1] + CR_RGB[2] * sum[2] + (128 << 19) + (1 << 18);
        pair[0] = (cb >> 19).clamp(0, 255) as u8;
        pair[1] = (cr >> 19).clamp(0, 255) as u8;
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`×`h` BGRA picture, rows `stride` bytes apart (the padding filled with a colour that must never show), from `at(x, y)` → RGB.
    fn picture(w: usize, h: usize, stride: usize, at: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut buf = vec![0xAB; stride * h];
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = at(x, y);
                buf[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        buf
    }

    fn run(src: (u32, u32), out: (u32, u32), stride: usize, px: &[u8]) -> Vec<u8> {
        let mut s = Scaler::new(src, out);
        let mut nv12 = vec![0u8; s.out_len()];
        s.convert(px, stride, &mut nv12).expect("converts");
        nv12
    }

    /// BT.709 limited range, in floating point: what the fixed point must match.
    fn reference(rgb: [u8; 3]) -> (f64, f64, f64) {
        let [r, g, b] = rgb.map(|c| f64::from(c) / 255.0);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        (16.0 + 219.0 * y, 128.0 + 224.0 * (b - y) / 1.8556, 128.0 + 224.0 * (r - y) / 1.5748)
    }

    #[test]
    fn the_sizes_of_nv12_are_those_an_encoder_takes() {
        assert_eq!(nv12_len(1920, 1080), 1920 * 1080 * 3 / 2);
        assert_eq!(nv12_len(2, 2), 6);
        // Odd sides round the chroma up.
        assert_eq!(nv12_len(3, 3), 9 + 2 * 2 * 2);
        assert_eq!(nv12_len(1, 1), 1 + 2);
        assert_eq!(Scaler::new((1023, 767), (640, 480)).out_len(), 640 * 480 * 3 / 2);
    }

    #[test]
    fn flat_colours_come_out_as_bt709_limited_range() {
        let colours = [[0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [128, 128, 128], [255, 255, 0], [0, 255, 255], [255, 0, 255], [12, 200, 77], [250, 128, 3]];
        for rgb in colours {
            let px = picture(4, 4, 16, |_, _| rgb);
            let nv12 = run((4, 4), (4, 4), 16, &px);
            let (y, cb, cr) = reference(rgb);
            for v in &nv12[..16] {
                assert!((f64::from(*v) - y).abs() <= 0.6, "Y of {rgb:?}: {v} against {y:.2}");
            }
            for pair in nv12[16..].chunks_exact(2) {
                assert!((f64::from(pair[0]) - cb).abs() <= 0.6, "Cb of {rgb:?}: {} against {cb:.2}", pair[0]);
                assert!((f64::from(pair[1]) - cr).abs() <= 0.6, "Cr of {rgb:?}: {} against {cr:.2}", pair[1]);
            }
        }
        // The ends of the range exactly: black 16, white 235, no colour 128; red's Cr at the top of its range.
        let black = run((2, 2), (2, 2), 8, &picture(2, 2, 8, |_, _| [0, 0, 0]));
        assert_eq!(black, vec![16, 16, 16, 16, 128, 128]);
        let white = run((2, 2), (2, 2), 8, &picture(2, 2, 8, |_, _| [255, 255, 255]));
        assert_eq!(white, vec![235, 235, 235, 235, 128, 128]);
        assert_eq!(run((2, 2), (2, 2), 8, &picture(2, 2, 8, |_, _| [255, 0, 0]))[5], 240);
        assert_eq!(run((2, 2), (2, 2), 8, &picture(2, 2, 8, |_, _| [0, 0, 255]))[4], 240);
    }

    #[test]
    fn the_same_size_copies_every_pixel_and_chroma_is_sited_on_the_left_column() {
        // A different grey in every pixel: each Y is that pixel's.
        let px = picture(4, 2, 16, |x, y| {
            let v = (x * 40 + y * 100) as u8;
            [v, v, v]
        });
        let nv12 = run((4, 2), (4, 2), 16, &px);
        for y in 0..2 {
            for x in 0..4 {
                let v = (x * 40 + y * 100) as u8;
                assert_eq!(f64::from(nv12[y * 4 + x]), reference([v, v, v]).0.round(), "pixel {x},{y}");
            }
        }
        // Red on the left half, blue on the right: the first pair is red's, the last blue's, and the pair across the edge is a quarter of its
        // left neighbour (red) and three quarters of its own two columns (blue): [1, 2, 1] on its left column.
        let px = picture(8, 2, 32, |x, _| if x < 4 { [255, 0, 0] } else { [0, 0, 255] });
        let uv = &run((8, 2), (8, 2), 32, &px)[16..];
        assert_eq!((uv[1], uv[6]), (240, 240), "red's Cr, then blue's Cb");
        let across = 0.25 * reference([255, 0, 0]).1 + 0.75 * reference([0, 0, 255]).1;
        assert!((f64::from(uv[4]) - across).abs() <= 1.0, "Cb across the edge: {} against {across:.2}", uv[4]);
    }

    #[test]
    fn a_large_reduction_averages_a_box_and_keeps_a_thin_line() {
        // A one-pixel white line in black, reduced four times each way: two taps would miss it, a box keeps a quarter of it.
        let px = picture(400, 8, 1600, |x, _| if x == 203 { [255, 255, 255] } else { [0, 0, 0] });
        let nv12 = run((400, 8), (100, 2), 1600, &px);
        let quarter = reference([64, 64, 64]).0;
        assert!((f64::from(nv12[50]) - quarter).abs() <= 1.0, "the line's column: {} against {quarter:.1}", nv12[50]);
        assert!(nv12[..100].iter().enumerate().all(|(x, v)| x == 50 || *v == 16), "only that column");
        // A one-pixel checkerboard averages to a flat mid grey.
        let px = picture(16, 16, 64, |x, y| if (x + y) % 2 == 0 { [0, 0, 0] } else { [255, 255, 255] });
        let mid = reference([128, 128, 128]).0;
        assert!(run((16, 16), (4, 4), 64, &px)[..16].iter().all(|v| (f64::from(*v) - mid).abs() <= 1.0));
    }

    #[test]
    fn halving_averages_each_pair_of_pixels() {
        // Columns alternating black and white: half the width is a flat mid grey.
        let px = picture(8, 4, 32, |x, _| if x % 2 == 0 { [0, 0, 0] } else { [255, 255, 255] });
        let nv12 = run((8, 4), (4, 2), 32, &px);
        let mid = reference([128, 128, 128]).0;
        for v in &nv12[..8] {
            assert!((f64::from(*v) - mid).abs() <= 1.0, "{v} against {mid:.1}");
        }
    }

    #[test]
    fn scaling_keeps_a_gradient_in_order_and_the_corners_in_place() {
        let px = picture(100, 50, 400, |x, y| [(x * 255 / 99) as u8, (y * 255 / 49) as u8, 0]);
        let (ow, oh) = (64usize, 36usize);
        let nv12 = run((100, 50), (ow as u32, oh as u32), 400, &px);
        // Red grows to the right and green down: Y never falls along a row or down a column.
        for y in 0..oh {
            for x in 1..ow {
                assert!(nv12[y * ow + x] >= nv12[y * ow + x - 1], "row {y} at {x}");
            }
        }
        for x in 0..ow {
            for y in 1..oh {
                assert!(nv12[y * ow + x] >= nv12[(y - 1) * ow + x], "column {x} at {y}");
            }
        }
        // The corners stay near the source's corners.
        assert!((f64::from(nv12[0]) - reference([0, 0, 0]).0).abs() <= 3.0);
        assert!((f64::from(nv12[oh * ow - 1]) - reference([255, 255, 0]).0).abs() <= 3.0);
        // Up as well as down.
        let up = run((100, 50), (200, 100), 400, &px);
        assert_eq!(up.len(), nv12_len(200, 100));
    }

    #[test]
    fn odd_sizes_in_and_out_are_converted_whole() {
        for (src, out) in [((1023u32, 767u32), (640u32, 360u32)), ((5, 3), (5, 3)), ((7, 5), (3, 3)), ((1, 1), (2, 2)), ((3, 1), (1, 1))] {
            let stride = src.0 as usize * 4;
            let px = picture(src.0 as usize, src.1 as usize, stride, |_, _| [0, 255, 0]);
            let nv12 = run(src, out, stride, &px);
            assert_eq!(nv12.len(), nv12_len(out.0, out.1));
            let (y, cb, cr) = reference([0, 255, 0]);
            let luma = out.0 as usize * out.1 as usize;
            assert!(nv12[..luma].iter().all(|v| (f64::from(*v) - y).abs() <= 0.6), "{src:?} → {out:?}: Y");
            assert!(nv12[luma..].chunks_exact(2).all(|p| (f64::from(p[0]) - cb).abs() <= 0.6 && (f64::from(p[1]) - cr).abs() <= 0.6), "{src:?} → {out:?}: chroma");
        }
    }

    #[test]
    fn row_padding_is_never_read_as_picture() {
        // A wide stride whose padding (0xAB) would tint the picture if it were read.
        let (w, h) = (6usize, 4usize);
        for stride in [w * 4, w * 4 + 8, 64] {
            let px = picture(w, h, stride, |_, _| [255, 255, 255]);
            assert_eq!(&run((6, 4), (6, 4), stride, &px)[..24], &[235u8; 24][..], "stride {stride}");
            // Scaling reads the last column's neighbours too.
            assert!(run((6, 4), (4, 2), stride, &px)[..8].iter().all(|v| *v == 235), "stride {stride} scaled");
        }
        // The last row needs no padding after it.
        let whole = picture(w, h, 64, |_, _| [0, 0, 0]);
        let tight = &whole[..64 * (h - 1) + w * 4];
        let mut nv12 = vec![0; nv12_len(6, 4)];
        assert!(Scaler::new((6, 4), (6, 4)).convert(tight, 64, &mut nv12).is_ok());
        assert_eq!(&nv12[..24], &[16u8; 24][..]);
    }

    #[test]
    fn short_buffers_are_refused_not_read_past() {
        let mut s = Scaler::new((4, 4), (4, 4));
        let px = picture(4, 4, 16, |_, _| [1, 2, 3]);
        let need = s.out_len();
        let mut nv12 = vec![0u8; need];
        assert!(s.convert(&px[..px.len() - 1], 16, &mut nv12).is_err(), "a picture short of its last byte");
        assert!(s.convert(&px, 12, &mut nv12).is_err(), "a stride shorter than a row");
        assert!(s.convert(&px, 16, &mut nv12[..need - 1]).is_err(), "an output too small");
        assert!(s.convert(&px, 16, &mut nv12).is_ok());
    }

    #[test]
    fn taps_map_centres_weigh_to_one_and_stay_inside() {
        let taps = |src: u32, out: u32| {
            let a = Axis::new(src, out);
            (0..out as usize).map(|i| a.taps(i).filter(|(_, w)| *w > 0).collect::<Vec<_>>()).collect::<Vec<_>>()
        };
        // The same size: every pixel is its own, no blend.
        assert!(taps(10, 10).iter().enumerate().all(|(i, t)| *t == vec![(i, ONE)]));
        // Halving: each output pixel is halfway between two.
        assert!(taps(10, 5).iter().enumerate().all(|(i, t)| *t == vec![(2 * i, ONE / 2), (2 * i + 1, ONE / 2)]));
        // Four times smaller: a box of four, a quarter each.
        assert!(taps(12, 3).iter().enumerate().all(|(i, t)| *t == (4 * i..4 * i + 4).map(|p| (p, ONE / 4)).collect::<Vec<_>>()));
        // Any reduction or enlargement: weights add up to one, never past the last pixel.
        for (src, out) in [(3, 100), (1920, 640), (1023, 360), (7, 3), (2560, 960), (5, 1)] {
            for t in taps(src, out) {
                assert_eq!(t.iter().map(|(_, w)| w).sum::<u32>(), ONE, "{src} → {out}");
                assert!(t.iter().all(|(p, _)| *p < src as usize), "{src} → {out}");
            }
        }
    }
}
