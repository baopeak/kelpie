//! H.264 as the phone needs it. Two things the encoder may not do on its own:
//!
//! - **No reordering, said in the stream.** Without the VUI's `bitstream_restriction`, a decoder may assume frames are reordered and hold
//!   up to its whole picture buffer before it shows the first (seen as hundreds of milliseconds in WebCodecs). The encoder makes no B-frames,
//!   so every SPS it emits is rewritten with `max_num_reorder_frames = 0` and `max_dec_frame_buffering = max_num_ref_frames`; everything
//!   else is copied bit for bit (`rewrite_sps`).
//! - **Parameter sets with every keyframe**, so that a phone that joins or resets can start from it (`Params::fix`).
//!
//! Annex B in, Annex B out (4-byte start codes). The phone turns it into the `avc` format WebCodecs takes, with an `avcC` description from
//! these SPS/PPS.

use std::fmt;

pub const NAL_SLICE: u8 = 1;
pub const NAL_IDR: u8 = 5;
pub const NAL_SEI: u8 = 6;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpsError {
    Short,
    NotSps,
    /// A value no encoder writes (a count past its limit, an Exp-Golomb code too long).
    Bad(&'static str),
}

impl fmt::Display for SpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpsError::Short => write!(f, "the SPS ends early"),
            SpsError::NotSps => write!(f, "not an SPS"),
            SpsError::Bad(what) => write!(f, "an SPS with a bad {what}"),
        }
    }
}

impl std::error::Error for SpsError {}

type R<T> = Result<T, SpsError>;

pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1f)
}

/// The NAL units of an Annex B stream (each without its start code, trailing zero bytes trimmed).
pub fn nal_units(au: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(au.len(), |n| n - 3);
        while end > s && au[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&au[s..end]);
        }
    }
    out
}

/// Whether an access unit holds an IDR picture.
pub fn is_keyframe(au: &[u8]) -> bool {
    nal_units(au).iter().any(|n| nal_type(n) == NAL_IDR)
}

/// The payload with its emulation-prevention bytes removed.
pub fn unescape(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut zeros = 0;
    for &b in ebsp {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// The payload with emulation-prevention bytes put in: no `00 00 0x` (x ≤ 3) may appear in a NAL unit.
pub fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 1);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bit(&mut self) -> R<u32> {
        let byte = *self.data.get(self.pos / 8).ok_or(SpsError::Short)?;
        let v = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(u32::from(v))
    }

    fn bits(&mut self, n: u32) -> R<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    fn ue(&mut self) -> R<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(SpsError::Bad("Exp-Golomb code"));
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let rest = self.bits(zeros)?;
        Ok(((1u64 << zeros) - 1 + u64::from(rest)).min(u64::from(u32::MAX)) as u32)
    }

    fn se(&mut self) -> R<i32> {
        let k = self.ue()?;
        Ok(if k % 2 == 1 { (k / 2 + 1) as i32 } else { -((k / 2) as i32) })
    }
}

/// Writes bits, most significant first.
#[derive(Default)]
pub struct Writer {
    out: Vec<u8>,
    cur: u8,
    n: u8,
}

impl Writer {
    pub fn bit(&mut self, b: u32) {
        self.cur = (self.cur << 1) | (b & 1) as u8;
        self.n += 1;
        if self.n == 8 {
            self.out.push(self.cur);
            self.cur = 0;
            self.n = 0;
        }
    }

    pub fn bits(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1);
        }
    }

    pub fn ue(&mut self, v: u32) {
        let x = u64::from(v) + 1;
        let len = 64 - x.leading_zeros();
        for _ in 0..len - 1 {
            self.bit(0);
        }
        for i in (0..len).rev() {
            self.bit(((x >> i) & 1) as u32);
        }
    }

    pub fn se(&mut self, v: i32) {
        self.ue(if v > 0 { (v as u32) * 2 - 1 } else { v.unsigned_abs() * 2 });
    }

    /// `rbsp_trailing_bits`: a one, then zeros to the byte.
    pub fn trailing(mut self) -> Vec<u8> {
        self.bit(1);
        while self.n != 0 {
            self.bit(0);
        }
        self.out
    }
}

/// Reads and writes the same thing, so that what is not changed is copied exactly.
struct Copier<'a> {
    r: Reader<'a>,
    w: Writer,
}

impl Copier<'_> {
    fn u(&mut self, n: u32) -> R<u32> {
        let v = self.r.bits(n)?;
        self.w.bits(v, n);
        Ok(v)
    }

    fn ue(&mut self) -> R<u32> {
        let v = self.r.ue()?;
        self.w.ue(v);
        Ok(v)
    }

    fn se(&mut self) -> R<i32> {
        let v = self.r.se()?;
        self.w.se(v);
        Ok(v)
    }

    fn scaling_list(&mut self, size: u32) -> R<()> {
        let (mut last, mut next) = (8i32, 8i32);
        for _ in 0..size {
            if next != 0 {
                let delta = self.se()?;
                next = (last + delta + 256).rem_euclid(256);
            }
            if next != 0 {
                last = next;
            }
        }
        Ok(())
    }

    fn hrd(&mut self) -> R<()> {
        let count = self.ue()?;
        if count > 31 {
            return Err(SpsError::Bad("CPB count"));
        }
        self.u(4)?;
        self.u(4)?;
        for _ in 0..=count {
            self.ue()?;
            self.ue()?;
            self.u(1)?;
        }
        for _ in 0..4 {
            self.u(5)?;
        }
        Ok(())
    }
}

/// What the phone and the helper need to know of an SPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sps {
    pub profile_idc: u8,
    pub constraints: u8,
    pub level_idc: u8,
    pub id: u32,
    pub chroma_format_idc: u32,
    pub max_num_ref_frames: u32,
    pub width: u32,
    pub height: u32,
    pub vui: bool,
    /// From `bitstream_restriction`, when there is one.
    pub max_num_reorder_frames: Option<u32>,
    pub max_dec_frame_buffering: Option<u32>,
}

const HIGH_PROFILES: &[u32] = &[100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// Reads an SPS NAL unit and writes it again with no reordering. Both, since one walk through the bits does both.
fn walk(nal: &[u8]) -> R<(Sps, Vec<u8>)> {
    if nal.len() < 4 {
        return Err(SpsError::Short);
    }
    if nal_type(nal) != NAL_SPS {
        return Err(SpsError::NotSps);
    }
    let rbsp = unescape(&nal[1..]);
    let mut c = Copier { r: Reader { data: &rbsp, pos: 0 }, w: Writer::default() };
    let profile_idc = c.u(8)?;
    let constraints = c.u(8)?;
    let level_idc = c.u(8)?;
    let id = c.ue()?;
    if id > 31 {
        return Err(SpsError::Bad("SPS id"));
    }
    let mut chroma_format_idc = 1;
    if HIGH_PROFILES.contains(&profile_idc) {
        chroma_format_idc = c.ue()?;
        if chroma_format_idc > 3 {
            return Err(SpsError::Bad("chroma format"));
        }
        if chroma_format_idc == 3 {
            c.u(1)?;
        }
        c.ue()?;
        c.ue()?;
        c.u(1)?;
        if c.u(1)? == 1 {
            for i in 0..(if chroma_format_idc != 3 { 8 } else { 12 }) {
                if c.u(1)? == 1 {
                    c.scaling_list(if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    c.ue()?;
    match c.ue()? {
        0 => {
            c.ue()?;
        }
        1 => {
            c.u(1)?;
            c.se()?;
            c.se()?;
            let n = c.ue()?;
            if n > 255 {
                return Err(SpsError::Bad("POC cycle"));
            }
            for _ in 0..n {
                c.se()?;
            }
        }
        2 => {}
        _ => return Err(SpsError::Bad("POC type")),
    }
    let max_num_ref_frames = c.ue()?;
    if max_num_ref_frames > 16 {
        return Err(SpsError::Bad("reference count"));
    }
    c.u(1)?;
    let width_mbs = c.ue()? + 1;
    let height_units = c.ue()? + 1;
    let frame_mbs_only = c.u(1)?;
    if frame_mbs_only == 0 {
        c.u(1)?;
    }
    c.u(1)?;
    let mut crop = [0u32; 4];
    if c.u(1)? == 1 {
        for v in &mut crop {
            *v = c.ue()?;
        }
    }
    let (unit_x, unit_y) = match chroma_format_idc {
        0 | 3 => (1, 2 - frame_mbs_only),
        1 => (2, 2 * (2 - frame_mbs_only)),
        _ => (2, 2 - frame_mbs_only),
    };
    let width = (width_mbs * 16).saturating_sub(unit_x * (crop[0] + crop[1]));
    let height = ((2 - frame_mbs_only) * height_units * 16).saturating_sub(unit_y * (crop[2] + crop[3]));

    // The VUI: copied up to `bitstream_restriction`, which is then written as it should be.
    let vui = c.r.bit()? == 1;
    c.w.bit(1);
    let mut restriction = (1u32, 2u32, 1u32, 16u32, 16u32);
    let (mut reorder, mut dpb) = (None, None);
    if vui {
        if c.u(1)? == 1 && c.u(8)? == 255 {
            c.u(16)?;
            c.u(16)?;
        }
        if c.u(1)? == 1 {
            c.u(1)?;
        }
        if c.u(1)? == 1 {
            c.u(3)?;
            c.u(1)?;
            if c.u(1)? == 1 {
                c.u(8)?;
                c.u(8)?;
                c.u(8)?;
            }
        }
        if c.u(1)? == 1 {
            c.ue()?;
            c.ue()?;
        }
        if c.u(1)? == 1 {
            c.u(16)?;
            c.u(16)?;
            c.u(16)?;
            c.u(16)?;
            c.u(1)?;
        }
        let nal_hrd = c.u(1)?;
        if nal_hrd == 1 {
            c.hrd()?;
        }
        let vcl_hrd = c.u(1)?;
        if vcl_hrd == 1 {
            c.hrd()?;
        }
        if nal_hrd == 1 || vcl_hrd == 1 {
            c.u(1)?;
        }
        c.u(1)?;
        if c.r.bit()? == 1 {
            let r = &mut c.r;
            restriction = (r.bit()?, r.ue()?, r.ue()?, r.ue()?, r.ue()?);
            reorder = Some(r.ue()?);
            dpb = Some(r.ue()?);
        }
    } else {
        // aspect ratio, overscan, video signal, chroma location, timing, NAL HRD, VCL HRD, pic_struct: none.
        for _ in 0..8 {
            c.w.bit(0);
        }
    }
    let mut w = c.w;
    w.bit(1);
    w.bit(restriction.0);
    w.ue(restriction.1);
    w.ue(restriction.2);
    w.ue(restriction.3);
    w.ue(restriction.4);
    w.ue(0);
    w.ue(max_num_ref_frames);
    let mut out = vec![nal[0]];
    out.extend(escape(&w.trailing()));
    let sps = Sps {
        profile_idc: profile_idc as u8,
        constraints: constraints as u8,
        level_idc: level_idc as u8,
        id,
        chroma_format_idc,
        max_num_ref_frames,
        width,
        height,
        vui,
        max_num_reorder_frames: reorder,
        max_dec_frame_buffering: dpb,
    };
    Ok((sps, out))
}

pub fn parse_sps(nal: &[u8]) -> R<Sps> {
    walk(nal).map(|(s, _)| s)
}

/// The SPS again, saying no frame is reordered; everything else as it was.
pub fn rewrite_sps(nal: &[u8]) -> R<Vec<u8>> {
    walk(nal).map(|(_, out)| out)
}

/// The WebCodecs codec string of an SPS: `avc1.PPCCLL`.
pub fn codec_string(sps: &Sps) -> String {
    format!("avc1.{:02x}{:02x}{:02x}", sps.profile_idc, sps.constraints, sps.level_idc)
}

const START: [u8; 4] = [0, 0, 0, 1];

/// The parameter sets seen so far, rewritten, so that each keyframe can carry them.
#[derive(Debug, Default)]
pub struct Params {
    sps_in: Vec<u8>,
    sps_out: Vec<u8>,
    pps: Vec<u8>,
    pub sps: Option<Sps>,
}

impl Params {
    /// The encoder's sequence header (`MF_MT_MPEG_SEQUENCE_HEADER`), when it has one: SPS and PPS in Annex B.
    pub fn seed(&mut self, header: &[u8]) {
        for nal in nal_units(header) {
            self.learn(nal);
        }
    }

    fn learn(&mut self, nal: &[u8]) -> bool {
        match nal_type(nal) {
            NAL_SPS if nal != self.sps_in.as_slice() => match walk(nal) {
                Ok((sps, out)) => {
                    self.sps_in = nal.to_vec();
                    self.sps_out = out;
                    self.sps = Some(sps);
                    true
                }
                Err(_) => false,
            },
            NAL_SPS => true,
            NAL_PPS => {
                self.pps = nal.to_vec();
                true
            }
            _ => false,
        }
    }

    /// An access unit as the phone gets it: SPS rewritten, a keyframe with the SPS and PPS in front if it came without, delimiters dropped.
    /// None for an SPS that cannot be read (it would leave the phone unable to decode): the caller asks for another keyframe.
    pub fn fix(&mut self, au: &[u8]) -> Option<Vec<u8>> {
        let nals = nal_units(au);
        let key = nals.iter().any(|n| nal_type(n) == NAL_IDR);
        let has_sps = nals.iter().any(|n| nal_type(n) == NAL_SPS);
        let has_pps = nals.iter().any(|n| nal_type(n) == NAL_PPS);
        let mut out = Vec::with_capacity(au.len() + self.sps_out.len() + self.pps.len() + 16);
        for nal in &nals {
            match nal_type(nal) {
                NAL_SPS => {
                    if !self.learn(nal) {
                        return None;
                    }
                }
                NAL_PPS => {
                    self.learn(nal);
                }
                _ => {}
            }
        }
        if key && (self.sps_out.is_empty() || self.pps.is_empty()) {
            return None;
        }
        if key && !has_sps {
            out.extend_from_slice(&START);
            out.extend_from_slice(&self.sps_out);
        }
        if key && !has_pps {
            out.extend_from_slice(&START);
            out.extend_from_slice(&self.pps);
        }
        for nal in nals {
            match nal_type(nal) {
                NAL_AUD => continue,
                NAL_SPS => {
                    out.extend_from_slice(&START);
                    out.extend_from_slice(&self.sps_out);
                }
                _ => {
                    out.extend_from_slice(&START);
                    out.extend_from_slice(nal);
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An SPS as an encoder would write one.
    struct Make {
        profile: u32,
        refs: u32,
        width_mbs: u32,
        height_mbs: u32,
        crop_bottom: u32,
        vui: Option<Vui>,
        scaling: bool,
    }

    struct Vui {
        timing: bool,
        hrd: bool,
        restriction: Option<(u32, u32)>,
    }

    fn make(m: &Make) -> Vec<u8> {
        let mut w = Writer::default();
        w.bits(m.profile, 8);
        w.bits(0, 8);
        w.bits(40, 8);
        w.ue(0);
        if HIGH_PROFILES.contains(&m.profile) {
            w.ue(1);
            w.ue(0);
            w.ue(0);
            w.bit(0);
            w.bit(u32::from(m.scaling));
            if m.scaling {
                for i in 0..8 {
                    w.bit(u32::from(i == 0 || i == 6));
                    if i == 0 {
                        for d in [2i32, -1, 0, 3, 0, 0, 1, -2, 0, 0, 0, 0, 5, 0, 0, 0] {
                            w.se(d);
                        }
                    } else if i == 6 {
                        // A list that ends early with a delta that makes the next scale zero.
                        w.se(-8);
                    }
                }
            }
        }
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(0); // pic_order_cnt_type
        w.ue(2); // log2_max_pic_order_cnt_lsb_minus4
        w.ue(m.refs);
        w.bit(0);
        w.ue(m.width_mbs - 1);
        w.ue(m.height_mbs - 1);
        w.bit(1); // frame_mbs_only
        w.bit(1); // direct_8x8
        w.bit(u32::from(m.crop_bottom > 0));
        if m.crop_bottom > 0 {
            w.ue(0);
            w.ue(0);
            w.ue(0);
            w.ue(m.crop_bottom);
        }
        match &m.vui {
            None => w.bit(0),
            Some(v) => {
                w.bit(1);
                w.bit(1); // aspect ratio: idc 1
                w.bits(1, 8);
                w.bit(0); // overscan
                w.bit(1); // video signal
                w.bits(5, 3);
                w.bit(0);
                w.bit(1);
                w.bits(1, 8);
                w.bits(1, 8);
                w.bits(1, 8);
                w.bit(0); // chroma location
                w.bit(u32::from(v.timing));
                if v.timing {
                    w.bits(1, 16);
                    w.bits(0, 16);
                    w.bits(0, 16);
                    w.bits(60, 16);
                    w.bit(0);
                }
                w.bit(u32::from(v.hrd));
                if v.hrd {
                    w.ue(0);
                    w.bits(4, 4);
                    w.bits(6, 4);
                    w.ue(1499);
                    w.ue(2999);
                    w.bit(1);
                    for n in [23, 23, 23, 24] {
                        w.bits(n, 5);
                    }
                }
                w.bit(0); // VCL HRD
                if v.hrd {
                    w.bit(0); // low_delay_hrd_flag
                }
                w.bit(0); // pic_struct
                match v.restriction {
                    None => w.bit(0),
                    Some((reorder, dpb)) => {
                        w.bit(1);
                        w.bit(1);
                        w.ue(0);
                        w.ue(0);
                        w.ue(15);
                        w.ue(15);
                        w.ue(reorder);
                        w.ue(dpb);
                    }
                }
            }
        }
        let mut nal = vec![0x67];
        nal.extend(escape(&w.trailing()));
        nal
    }

    fn base() -> Make {
        Make { profile: 66, refs: 1, width_mbs: 120, height_mbs: 68, crop_bottom: 4, vui: None, scaling: false }
    }

    #[test]
    fn an_sps_without_vui_gets_one_that_says_no_reordering_and_nothing_else_changes() {
        let nal = make(&base());
        let before = parse_sps(&nal).unwrap();
        assert_eq!((before.width, before.height, before.vui, before.max_num_reorder_frames), (1920, 1080, false, None));
        let out = rewrite_sps(&nal).unwrap();
        let after = parse_sps(&out).unwrap();
        assert_eq!((after.max_num_reorder_frames, after.max_dec_frame_buffering, after.vui), (Some(0), Some(1), true));
        assert_eq!((after.profile_idc, after.level_idc, after.width, after.height, after.max_num_ref_frames), (66, 40, 1920, 1080, 1));
        assert_eq!(rewrite_sps(&out).unwrap(), out, "rewriting again changes nothing");
        assert_eq!(codec_string(&after), "avc1.420028");
    }

    #[test]
    fn an_sps_with_timing_hrd_and_reordering_keeps_them_and_loses_the_reordering() {
        for restriction in [None, Some((2, 4))] {
            let nal = make(&Make { profile: 100, refs: 2, vui: Some(Vui { timing: true, hrd: true, restriction }), scaling: true, ..base() });
            let before = parse_sps(&nal).unwrap();
            assert_eq!(before.max_num_reorder_frames, restriction.map(|r| r.0));
            let out = rewrite_sps(&nal).unwrap();
            let after = parse_sps(&out).unwrap();
            assert_eq!((after.max_num_reorder_frames, after.max_dec_frame_buffering), (Some(0), Some(2)));
            assert_eq!((after.profile_idc, after.width, after.height, after.chroma_format_idc), (100, 1920, 1080, 1));
            // Everything before the restriction is the same bits: the rewritten SPS starts with what the original said up to it.
            let rbsp_in = unescape(&nal[1..]);
            let rbsp_out = unescape(&out[1..]);
            let same = rbsp_in.iter().zip(&rbsp_out).take_while(|(a, b)| a == b).count();
            assert!(same >= rbsp_in.len() - 6, "{same} of {}", rbsp_in.len());
        }
    }

    #[test]
    fn emulation_prevention_round_trips() {
        let cases: [&[u8]; 6] = [&[0, 0, 0], &[0, 0, 1, 0, 0, 2], &[0, 0, 3, 0, 0, 3], &[1, 0, 0, 4, 0, 0], &[0, 0, 0, 0, 0, 0, 0], &[9, 8, 7]];
        for raw in cases {
            let e = escape(raw);
            assert_eq!(unescape(&e), raw, "{raw:?}");
            assert!(!e.windows(3).any(|w| w[0] == 0 && w[1] == 0 && w[2] <= 2), "{e:?}");
        }
        assert_eq!(escape(&[0, 0, 1]), [0, 0, 3, 1]);
    }

    #[test]
    fn exp_golomb_codes_write_and_read_back() {
        let mut w = Writer::default();
        let values = [0u32, 1, 2, 3, 7, 8, 255, 65_535, 1 << 20];
        for v in values {
            w.ue(v);
        }
        for v in [0i32, 1, -1, 5, -77] {
            w.se(v);
        }
        let bytes = w.trailing();
        let mut r = Reader { data: &bytes, pos: 0 };
        for v in values {
            assert_eq!(r.ue().unwrap(), v);
        }
        for v in [0i32, 1, -1, 5, -77] {
            assert_eq!(r.se().unwrap(), v);
        }
    }

    #[test]
    fn garbage_is_refused_not_rewritten() {
        assert_eq!(rewrite_sps(&[0x68, 1, 2, 3]), Err(SpsError::NotSps));
        assert_eq!(rewrite_sps(&[0x67, 66]), Err(SpsError::Short));
        assert!(rewrite_sps(&[0x67, 66, 0, 40, 0, 0, 0, 0, 0]).is_err(), "an Exp-Golomb code that never ends");
        let mut nal = make(&base());
        nal.truncate(8);
        assert!(rewrite_sps(&nal).is_err());
    }

    fn annex_b(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, n) in nals.iter().enumerate() {
            out.extend_from_slice(if i % 2 == 0 { &[0, 0, 0, 1][..] } else { &[0, 0, 1][..] });
            out.extend_from_slice(n);
        }
        out
    }

    #[test]
    fn nal_units_split_on_both_start_codes() {
        let au = annex_b(&[&[0x09, 0x10], &[0x67, 1, 2], &[0x68, 3], &[0x65, 4, 5, 0]]);
        let nals = nal_units(&au);
        assert_eq!(nals.iter().map(|n| nal_type(n)).collect::<Vec<_>>(), [NAL_AUD, NAL_SPS, NAL_PPS, NAL_IDR]);
        assert_eq!(nals[3], &[0x65, 4, 5], "trailing zeros are not the NAL's");
        assert!(is_keyframe(&au));
        assert!(!is_keyframe(&annex_b(&[&[0x41, 1]])));
    }

    #[test]
    fn every_keyframe_carries_the_rewritten_parameter_sets() {
        let sps = make(&base());
        let pps: &[u8] = &[0x68, 0xce, 0x3c, 0x80];
        let idr: &[u8] = &[0x65, 0x88, 0x84];
        let p_frame: &[u8] = &[0x41, 0x9a, 0x02];
        let mut params = Params::default();
        let first = params.fix(&annex_b(&[&[0x09, 0xf0], &sps, pps, idr])).unwrap();
        let nals = nal_units(&first);
        assert_eq!(nals.iter().map(|n| nal_type(n)).collect::<Vec<_>>(), [NAL_SPS, NAL_PPS, NAL_IDR], "the delimiter is dropped");
        assert_eq!(parse_sps(nals[0]).unwrap().max_num_reorder_frames, Some(0));
        assert_eq!(params.fix(&annex_b(&[p_frame])).unwrap(), annex_b(&[p_frame]), "a P-frame as it was");
        // A keyframe without parameter sets gets them in front.
        let later = params.fix(&annex_b(&[idr])).unwrap();
        assert_eq!(nal_units(&later).iter().map(|n| nal_type(n)).collect::<Vec<_>>(), [NAL_SPS, NAL_PPS, NAL_IDR]);
        // With no parameter set ever seen, a keyframe cannot be sent.
        assert!(Params::default().fix(&annex_b(&[idr])).is_none());
        let mut seeded = Params::default();
        seeded.seed(&annex_b(&[&sps, pps]));
        assert!(seeded.fix(&annex_b(&[idr])).is_some(), "the encoder's sequence header is enough");
        assert_eq!(codec_string(seeded.sps.as_ref().unwrap()), "avc1.420028");
    }
}
