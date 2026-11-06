//! The two formats the helper speaks.
//!
//! **Media messages** go to the phone as they are: the app passes them to the daemon (base64 in `provider.event`, on the local pipe), and
//! the daemon sends them as binary messages on the phone's encrypted link. They start with `MEDIA` (`0x01`), which no JSON message does
//! (they start with `{`), and are little-endian:
//!
//! ```text
//! video  (kind 1): 01 01 flags n_unpaint | seq u32 | capture_ms u32 | src_w u16 src_h u16 | coded_w u16 coded_h u16 | n_unpaint × (x y w h u16) | Annex B access unit
//!                  flags: 1 key, 2 unpaint every tile
//! tile   (kind 2): 01 02 format 00 | seq u32 | id u32 | x y w h u16 | PNG (format 1) or JPEG (format 2)
//! cursor (kind 3): 01 03 shape visible | x i32 | y i32
//! ```
//!
//! Rectangles and the cursor are in the source's pixels (the captured picture before any scaling), so a tile stays crisp whatever the
//! video's size. A tile matches video frame `seq` and later; a video frame's unpaint rectangles drop the tiles they touch as it is drawn.
//!
//! **The helper's stdout**: frames of `u32 LE length | u8 kind | payload`, kind 1 a JSON message, kind 2 a media message for stream `sc`
//! (`u8 length | sc | media`). Its stdin is JSON lines (`main.rs`).

use crate::geom::Rect;

/// What the helper says first; the app refuses a helper that speaks another.
pub const PROTO: u32 = 1;

pub const MEDIA: u8 = 0x01;
pub const KIND_VIDEO: u8 = 1;
pub const KIND_TILE: u8 = 2;
pub const KIND_CURSOR: u8 = 3;
pub const FLAG_KEY: u8 = 1;
pub const FLAG_UNPAINT_ALL: u8 = 2;
pub const FORMAT_PNG: u8 = 1;
pub const FORMAT_JPEG: u8 = 2;
pub const VIDEO_HEADER: usize = 20;
pub const TILE_HEADER: usize = 20;
pub const CURSOR_LEN: usize = 12;
/// The largest media message: one link message holds 1 MiB (`kelpie_link::MAX_APP_MESSAGE`).
pub const MAX_MEDIA: usize = 1000 * 1024;
/// The largest tile image; a larger region is sent as smaller tiles.
pub const MAX_TILE: usize = 256 * 1024;
pub const MAX_UNPAINT: usize = 255;

pub const OUT_JSON: u8 = 1;
pub const OUT_MEDIA: u8 = 2;
/// The largest stdout frame the app takes.
pub const MAX_OUT: usize = 4 * 1024 * 1024;

/// Cursor shapes the phone draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Shape {
    Arrow = 0,
    IBeam = 1,
    Hand = 2,
    Wait = 3,
    Cross = 4,
    SizeNs = 5,
    SizeWe = 6,
    SizeNwse = 7,
    SizeNesw = 8,
    SizeAll = 9,
    No = 10,
    AppStarting = 11,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    pub key: bool,
    pub seq: u32,
    pub capture_ms: u32,
    pub src: (u16, u16),
    pub coded: (u16, u16),
    /// Tiles to drop as this frame is drawn; more than `MAX_UNPAINT` become "all".
    pub unpaint: Vec<Rect>,
    pub unpaint_all: bool,
}

fn u16_of(v: i32) -> [u8; 2] {
    (v.clamp(0, i32::from(u16::MAX)) as u16).to_le_bytes()
}

fn rect_bytes(out: &mut Vec<u8>, r: Rect) {
    for v in [r.x, r.y, r.w, r.h] {
        out.extend_from_slice(&u16_of(v));
    }
}

pub fn video(v: &Video, au: &[u8]) -> Vec<u8> {
    let all = v.unpaint_all || v.unpaint.len() > MAX_UNPAINT;
    let rects: &[Rect] = if all { &[] } else { &v.unpaint };
    let mut out = Vec::with_capacity(VIDEO_HEADER + rects.len() * 8 + au.len());
    let flags = u8::from(v.key) * FLAG_KEY | u8::from(all) * FLAG_UNPAINT_ALL;
    out.extend_from_slice(&[MEDIA, KIND_VIDEO, flags, rects.len() as u8]);
    out.extend_from_slice(&v.seq.to_le_bytes());
    out.extend_from_slice(&v.capture_ms.to_le_bytes());
    for d in [v.src.0, v.src.1, v.coded.0, v.coded.1] {
        out.extend_from_slice(&d.to_le_bytes());
    }
    for r in rects {
        rect_bytes(&mut out, *r);
    }
    out.extend_from_slice(au);
    out
}

pub fn tile(seq: u32, id: u32, r: Rect, format: u8, image: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(TILE_HEADER + image.len());
    out.extend_from_slice(&[MEDIA, KIND_TILE, format, 0]);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    rect_bytes(&mut out, r);
    out.extend_from_slice(image);
    out
}

pub fn cursor(shape: Shape, visible: bool, x: i32, y: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(CURSOR_LEN);
    out.extend_from_slice(&[MEDIA, KIND_CURSOR, shape as u8, u8::from(visible)]);
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    out
}

/// What the daemon needs of a media message to pace it: its kind, and for video its number and whether it is a keyframe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peek {
    pub kind: u8,
    pub key: bool,
    pub seq: u32,
}

/// A media message's kind and number, if it is a well-formed one.
pub fn peek(m: &[u8]) -> Option<Peek> {
    if m.len() > MAX_MEDIA || m.first() != Some(&MEDIA) {
        return None;
    }
    let seq = |m: &[u8]| m.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    match *m.get(1)? {
        KIND_VIDEO => {
            let n = usize::from(*m.get(3)?);
            (m.len() > VIDEO_HEADER + n * 8).then_some(())?;
            Some(Peek { kind: KIND_VIDEO, key: m[2] & FLAG_KEY != 0, seq: seq(m)? })
        }
        KIND_TILE if m.len() > TILE_HEADER => Some(Peek { kind: KIND_TILE, key: false, seq: seq(m)? }),
        KIND_CURSOR if m.len() == CURSOR_LEN => Some(Peek { kind: KIND_CURSOR, key: false, seq: 0 }),
        _ => None,
    }
}

/// One stdout frame.
pub fn out_json(v: &serde_json::Value) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let mut out = Vec::with_capacity(5 + body.len());
    out.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    out.push(OUT_JSON);
    out.extend_from_slice(&body);
    out
}

pub fn out_media(sc: &str, media: &[u8]) -> Vec<u8> {
    let sc = &sc.as_bytes()[..sc.len().min(64)];
    let len = 1 + 1 + sc.len() + media.len();
    let mut out = Vec::with_capacity(4 + len);
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.push(OUT_MEDIA);
    out.push(sc.len() as u8);
    out.extend_from_slice(sc);
    out.extend_from_slice(media);
    out
}

#[derive(Debug, PartialEq)]
pub enum Out {
    Json(serde_json::Value),
    Media { sc: String, media: Vec<u8> },
}

/// Reads stdout frames as bytes arrive. A frame too long, of an unknown kind or with bad JSON ends the reading: the helper is not speaking
/// this protocol, and the app stops it.
#[derive(Default)]
pub struct Deframer {
    buf: Vec<u8>,
    broken: bool,
}

impl Deframer {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn broken(&self) -> bool {
        self.broken
    }

    /// The next whole frame, if one is in.
    pub fn next(&mut self) -> Option<Out> {
        if self.broken || self.buf.len() < 5 {
            return None;
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len == 0 || len > MAX_OUT {
            self.broken = true;
            return None;
        }
        if self.buf.len() < 4 + len {
            return None;
        }
        let frame: Vec<u8> = self.buf.drain(..4 + len).skip(4).collect();
        let out = match frame[0] {
            OUT_JSON => serde_json::from_slice(&frame[1..]).ok().map(Out::Json),
            OUT_MEDIA => {
                let n = usize::from(*frame.get(1).unwrap_or(&255));
                frame.get(2..2 + n).and_then(|sc| std::str::from_utf8(sc).ok()).map(|sc| Out::Media { sc: sc.to_string(), media: frame[2 + n..].to_vec() })
            }
            _ => None,
        };
        if out.is_none() {
            self.broken = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_video_message_has_its_header_where_the_phone_reads_it() {
        let v = Video { key: true, seq: 0x0102_0304, capture_ms: 7, src: (2560, 1440), coded: (1280, 720), unpaint: vec![Rect::new(64, 128, 192, 64)], unpaint_all: false };
        let m = video(&v, &[0, 0, 0, 1, 0x65, 0xaa]);
        assert_eq!(&m[..4], &[MEDIA, KIND_VIDEO, FLAG_KEY, 1]);
        assert_eq!(&m[4..8], &[4, 3, 2, 1]);
        assert_eq!(&m[8..12], &[7, 0, 0, 0]);
        assert_eq!(&m[12..20], &[0x00, 0x0a, 0xa0, 0x05, 0x00, 0x05, 0xd0, 0x02]);
        assert_eq!(&m[20..28], &[64, 0, 128, 0, 192, 0, 64, 0]);
        assert_eq!(&m[28..], &[0, 0, 0, 1, 0x65, 0xaa]);
        assert_eq!(peek(&m), Some(Peek { kind: KIND_VIDEO, key: true, seq: 0x0102_0304 }));
        // The phone's fixture (app/phone/src/screen.test.ts) is this message.
        let fixture = video(&Video { key: false, seq: 9, capture_ms: 1000, src: (100, 50), coded: (100, 50), unpaint: vec![], unpaint_all: true }, &[0, 0, 0, 1, 0x41, 0x9a]);
        assert_eq!(fixture, [1, 1, 2, 0, 9, 0, 0, 0, 0xe8, 3, 0, 0, 100, 0, 50, 0, 100, 0, 50, 0, 0, 0, 0, 1, 0x41, 0x9a]);
    }

    #[test]
    fn too_many_unpaint_rectangles_become_unpaint_all() {
        let v = Video { key: false, seq: 1, capture_ms: 0, src: (10, 10), coded: (10, 10), unpaint: vec![Rect::new(0, 0, 1, 1); MAX_UNPAINT + 1], unpaint_all: false };
        let m = video(&v, &[1]);
        assert_eq!((m[2], m[3], m.len()), (FLAG_UNPAINT_ALL, 0, VIDEO_HEADER + 1));
    }

    #[test]
    fn tiles_and_the_cursor() {
        let t = tile(5, 77, Rect::new(512, 0, 512, 256), FORMAT_PNG, b"\x89PNG");
        assert_eq!(&t[..TILE_HEADER], &[1, 2, 1, 0, 5, 0, 0, 0, 77, 0, 0, 0, 0, 2, 0, 0, 0, 2, 0, 1]);
        assert_eq!(&t[TILE_HEADER..], b"\x89PNG");
        assert_eq!(peek(&t).map(|p| (p.kind, p.seq)), Some((KIND_TILE, 5)));
        let c = cursor(Shape::IBeam, true, -3, 400);
        assert_eq!(c, [1, 3, 1, 1, 0xfd, 0xff, 0xff, 0xff, 0x90, 1, 0, 0]);
        assert_eq!(peek(&c).map(|p| p.kind), Some(KIND_CURSOR));
    }

    #[test]
    fn what_is_not_a_media_message_is_not_taken_for_one() {
        for bad in [&b"{\"type\":\"hb\"}"[..], &[], &[1], &[1, 1, 0, 0, 1, 0, 0, 0], &[1, 9, 0, 0, 0, 0, 0, 0, 0], &[1, 3, 0, 0], &[1, 2, 1, 0, 1, 0, 0, 0]] {
            assert_eq!(peek(bad), None, "{bad:?}");
        }
        let v = video(&Video { key: false, seq: 1, capture_ms: 0, src: (1, 1), coded: (1, 1), unpaint: vec![Rect::default(); 3], unpaint_all: false }, &[]);
        assert_eq!(peek(&v), None, "a header that promises rectangles and no picture");
        assert_eq!(peek(&vec![1u8; MAX_MEDIA + 1]), None);
    }

    #[test]
    fn stdout_frames_come_apart_however_they_arrive() {
        let mut bytes = out_json(&json!({"event": "hello", "proto": PROTO}));
        bytes.extend(out_media("sc1", &[1, 3, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]));
        bytes.extend(out_json(&json!({"id": 4, "ok": true})));
        let mut d = Deframer::default();
        let mut got = Vec::new();
        for chunk in bytes.chunks(3) {
            d.push(chunk);
            while let Some(o) = d.next() {
                got.push(o);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], Out::Json(json!({"event": "hello", "proto": 1})));
        assert_eq!(got[1], Out::Media { sc: "sc1".into(), media: vec![1, 3, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0] });
        assert!(!d.broken());
        let mut bad = Deframer::default();
        bad.push(&[0xff, 0xff, 0xff, 0x7f, 1]);
        assert_eq!(bad.next(), None);
        assert!(bad.broken());
        let mut odd = Deframer::default();
        odd.push(&[2, 0, 0, 0, 9, 0]);
        assert_eq!(odd.next(), None);
        assert!(odd.broken(), "an unknown kind");
    }
}
