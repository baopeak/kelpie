//! A paint-over tile's image: PNG (lossless, small for text and flat colour), or JPEG at quality 90 when the region is a photo and PNG would
//! be larger than a byte a pixel. Pure Rust (`png`, `jpeg-encoder`), from the BGRA readback of the capture.

use crate::geom::Rect;
use crate::wire::{FORMAT_JPEG, FORMAT_PNG, MAX_TILE};

/// The region's pixels as RGB, rows packed.
fn rgb(px: &[u8], stride: usize, r: Rect) -> Vec<u8> {
    let (x, y, w, h) = (r.x as usize, r.y as usize, r.w as usize, r.h as usize);
    let mut out = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        let start = row * stride + x * 4;
        for p in px[start..start + w * 4].chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0]]);
        }
    }
    out
}

fn png(rgb: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.set_filter(png::FilterType::Sub);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgb).ok()?;
    }
    Some(out)
}

fn jpeg(rgb: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut out, 90);
    enc.encode(rgb, u16::try_from(w).ok()?, u16::try_from(h).ok()?, jpeg_encoder::ColorType::Rgb).ok()?;
    Some(out)
}

/// The tile for region `r` of a BGRA picture: its format and bytes, or None when it cannot be made small enough for one message (the
/// caller cuts the region smaller, or leaves it to the video).
pub fn encode(px: &[u8], stride: usize, r: Rect) -> Option<(u8, Vec<u8>)> {
    if r.is_empty() || r.x < 0 || r.y < 0 {
        return None;
    }
    let pixels = rgb(px, stride, r);
    let (w, h) = (r.w as u32, r.h as u32);
    let lossless = png(&pixels, w, h)?;
    if lossless.len() <= (w * h) as usize && lossless.len() <= MAX_TILE {
        return Some((FORMAT_PNG, lossless));
    }
    jpeg(&pixels, w, h).filter(|j| j.len() <= MAX_TILE).map(|j| (FORMAT_JPEG, j))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_like_regions_are_lossless_and_noise_falls_back_to_jpeg() {
        let (w, h) = (128usize, 64usize);
        let stride = w * 4 + 16;
        // Black text-like strokes on white.
        let mut text = vec![255u8; stride * h];
        for y in (8..h).step_by(12) {
            for x in 4..w - 4 {
                if (x / 3) % 2 == 0 {
                    let i = y * stride + x * 4;
                    text[i..i + 3].copy_from_slice(&[20, 20, 20]);
                }
            }
        }
        let (format, bytes) = encode(&text, stride, Rect::new(0, 0, w as i32, h as i32)).unwrap();
        assert_eq!(format, FORMAT_PNG);
        assert_eq!(&bytes[1..4], b"PNG");
        let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..3], &[255, 255, 255]);
        let stroke = (8 * w + 6) * 3;
        assert_eq!(&buf[stroke..stroke + 3], &[20, 20, 20], "exactly what was captured");
        // Noise: PNG would be more than a byte a pixel.
        let mut seed = 0x1234_5678u32;
        let noise: Vec<u8> = (0..stride * h)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let (format, bytes) = encode(&noise, stride, Rect::new(0, 0, w as i32, h as i32)).unwrap();
        assert_eq!((format, &bytes[..2]), (FORMAT_JPEG, &[0xff, 0xd8][..]));
    }

    #[test]
    fn a_region_is_cut_from_its_place_and_bgr_becomes_rgb() {
        let (w, h) = (8usize, 4usize);
        let stride = w * 4;
        let mut px = vec![0u8; stride * h];
        let i = 2 * stride + 5 * 4;
        px[i..i + 4].copy_from_slice(&[1, 2, 3, 255]);
        assert_eq!(&rgb(&px, stride, Rect::new(5, 2, 1, 1)), &[3, 2, 1]);
        assert!(encode(&px, stride, Rect::new(0, 0, 0, 4)).is_none());
    }
}
