//! BGRA → NV12 at the encoder's size on the CPU, for a computer whose graphics have no video processor (a virtual machine, a cloud desktop):
//! a bilinear scale and BT.709 limited range, the colours the GPU path gives (`win/convert.rs`). Every buffer is made with the scaler, none
//! per frame.
//!
//! NV12 here is the Y plane (`w` bytes a row, `h` rows) followed by the interleaved CbCr plane (a pair for every 2×2 block, `chroma_w(w) * 2`
//! bytes a row, `(h + 1) / 2` rows): for the even sizes an encoder takes, that is `w * h * 3 / 2` bytes with both planes `w` bytes a row.

/// Fractional bits of a sample position.
const FRAC: u32 = 8;
const ONE: u32 = 1 << FRAC;

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

/// Where an output sample is taken in the source: the left (or top) source pixel and the weight of the next one, in 1/256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tap {
    at: usize,
    next: usize,
    frac: u32,
}

/// Pixel centres mapped onto each other (as a GPU sampler does), clamped at the edges.
fn taps(src: u32, out: u32) -> Vec<Tap> {
    let last = src as usize - 1;
    (0..u64::from(out))
        .map(|i| {
            // ((i + ½) · src / out − ½) in 1/256.
            let pos = ((2 * i + 1) * u64::from(src) * u64::from(ONE) / (2 * u64::from(out))).saturating_sub(u64::from(ONE / 2));
            let at = (pos >> FRAC) as usize;
            if at >= last {
                Tap { at: last, next: last, frac: 0 }
            } else {
                Tap { at, next: at + 1, frac: (pos as u32) & (ONE - 1) }
            }
        })
        .collect()
}

pub struct Scaler {
    pub src: (u32, u32),
    pub out: (u32, u32),
    cols: Vec<Tap>,
    rows: Vec<Tap>,
    /// Two output rows, scaled, as R, G, B: the pair a row of chroma is made from.
    scratch: Vec<u8>,
}

impl Scaler {
    /// A scaler from `src` to `out` (each side at least 1).
    pub fn new(src: (u32, u32), out: (u32, u32)) -> Scaler {
        let (src, out) = ((src.0.max(1), src.1.max(1)), (out.0.max(1), out.1.max(1)));
        Scaler { src, out, cols: taps(src.0, out.0), rows: taps(src.1, out.1), scratch: vec![0; out.0 as usize * 3 * 2] }
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
            scale_row(bgra, stride, &self.cols, self.rows[top], first);
            if bottom != top {
                scale_row(bgra, stride, &self.cols, self.rows[bottom], second);
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

/// One output row, bilinear, as R, G, B.
fn scale_row(bgra: &[u8], stride: usize, cols: &[Tap], row: Tap, out: &mut [u8]) {
    let (a, b) = (&bgra[row.at * stride..], &bgra[row.next * stride..]);
    let fy = row.frac;
    for (x, c) in cols.iter().enumerate() {
        let (l, r) = (c.at * 4, c.next * 4);
        for ch in 0..3 {
            // BGRA in, RGB out.
            let i = 2 - ch;
            let top = u32::from(a[l + i]) * (ONE - c.frac) + u32::from(a[r + i]) * c.frac;
            let bot = u32::from(b[l + i]) * (ONE - c.frac) + u32::from(b[r + i]) * c.frac;
            out[x * 3 + ch] = ((top * (ONE - fy) + bot * fy + (1 << (2 * FRAC - 1))) >> (2 * FRAC)) as u8;
        }
    }
}

fn luma_row(rgb: &[u8], y: &mut [u8]) {
    for (px, out) in rgb.chunks_exact(3).zip(y.iter_mut()) {
        let v = Y_RGB[0] * i32::from(px[0]) + Y_RGB[1] * i32::from(px[1]) + Y_RGB[2] * i32::from(px[2]) + (16 << 16) + (1 << 15);
        *out = (v >> 16).clamp(0, 255) as u8;
    }
}

/// Cb and Cr of each 2×2 block of the two rows (a last column of its own, an odd width, counts twice).
fn chroma_row(first: &[u8], second: &[u8], w: usize, uv: &mut [u8]) {
    for (i, pair) in uv.chunks_exact_mut(2).enumerate() {
        let (l, r) = (i * 2 * 3, (i * 2 + 1).min(w - 1) * 3);
        let mut sum = [0i32; 3];
        for (ch, s) in sum.iter_mut().enumerate() {
            *s = i32::from(first[l + ch]) + i32::from(first[r + ch]) + i32::from(second[l + ch]) + i32::from(second[r + ch]);
        }
        // Four pixels summed: 1/65536 · 1/4 is 1/2^18.
        let cb = CB_RGB[0] * sum[0] + CB_RGB[1] * sum[1] + CB_RGB[2] * sum[2] + (128 << 18) + (1 << 17);
        let cr = CR_RGB[0] * sum[0] + CR_RGB[1] * sum[1] + CR_RGB[2] * sum[2] + (128 << 18) + (1 << 17);
        pair[0] = (cb >> 18).clamp(0, 255) as u8;
        pair[1] = (cr >> 18).clamp(0, 255) as u8;
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
    fn the_same_size_copies_every_pixel_and_chroma_averages_each_block() {
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
        // Left block red, right block blue: each pair of chroma is its own block's.
        let px = picture(4, 2, 16, |x, _| if x < 2 { [255, 0, 0] } else { [0, 0, 255] });
        let uv = &run((4, 2), (4, 2), 16, &px)[8..];
        assert_eq!((uv[1], uv[2]), (240, 240), "red's Cr, then blue's Cb");
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
        let tight = &picture(w, h, 64, |_, _| [0, 0, 0])[..64 * (h - 1) + w * 4];
        assert!(Scaler::new((6, 4), (6, 4)).convert(tight, 64, &mut vec![0; nv12_len(6, 4)]).is_ok());
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
    fn taps_map_centres_and_stay_inside() {
        // The same size: every pixel is its own, no blend.
        assert!(taps(10, 10).iter().enumerate().all(|(i, t)| t.at == i && t.frac == 0));
        // Halving: each output pixel is halfway between two.
        assert!(taps(10, 5).iter().enumerate().all(|(i, t)| t.at == 2 * i && t.next == 2 * i + 1 && t.frac == ONE / 2));
        // Enlarging never reads past the last pixel.
        assert!(taps(3, 100).iter().all(|t| t.at < 3 && t.next < 3));
    }
}
