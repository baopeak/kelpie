//! When to take a picture and what to do with a still one, as Chrome Remote Desktop's host does it (`remoting/protocol/
//! webrtc_frame_scheduler_constant_rate.cc` and `webrtc_video_encoder_wrapper.cc`, read in the raw source on 2026-11-07), on our H.264:
//!
//! - **`Pacer`**: a capture on a constant-rate timer anchored to the last capture's start (`last_capture_started + interval`, at least 1 ms
//!   away), **one frame in the encoder** unless the encoder asks for another (an encoder that holds a frame until the next arrives is fed at
//!   the rate, never left waiting). Nothing queues: a picture that arrives before its time waits in the capture's one-slot mailbox, newest
//!   wins. Where CRD captures exactly at the timer, a still screen gives us nothing to capture, so once due the stream waits for the next change
//!   and takes it then. There is no boost after input (CRD's `BoostCaptureRate`): CRD boosts from a low idle rate, and ours has none, since a
//!   still screen sends nothing; a boost could only override the link's and the encoder's own rate, with the encoder set for another.
//! - **`TopOff`**: once the picture stops changing, the same picture is encoded again on CRD's top-off timer, `max(1.1 × interval, interval
//!   + 2 ms)`, so that each pass refines it in place: CRD lowers the quantizer itself; Media Foundation takes a fixed QP only in fixed-QP
//!   mode, so on our CBR stream the rate control spends the bits of a still picture on it. It ends when the encoder says its quantizer is at
//!   CRD's `kMinQuantizer` (10) or below, when a pass comes out tiny (nothing left to refine), or after `TOP_OFF_MAX` passes.
//! - **`BigFrame`**: CRD's large-frame guard: an update more than `kBigFrameThresholdPixels` (300 000) larger than the largest of the last
//!   `kStatsWindow` (5) frames, estimated at `kEstimatedBytesPerMegapixel` (100 000 B/MP), that would take longer than a frame interval to
//!   send at the bitrate, is encoded at lower quality rather than late. CRD raises the minimum quantizer for that frame; we lower the bitrate
//!   for that one frame (`BIG_FRAME_PERCENT`), which a CBR encoder answers with a coarser quantizer.
//! - **`EncoderRate`**: the encoder is reconfigured only for a real change: up by more than 33 % (CRD's `kEncoderBitrateChangePercentage`,
//!   since a reconfiguration costs quality), down as soon as the link controller cuts (a cut is congestion, which must not wait).
//!
//! Every clock is milliseconds passed in by the caller.

use std::collections::VecDeque;

/// The least time between two captures (CRD's floor).
pub const MIN_GAP_MS: f64 = 1.0;

/// The constant-rate capture schedule, with what the encoder holds.
#[derive(Debug, Default)]
pub struct Pacer {
    last_start: Option<u64>,
    in_encoder: u32,
}

impl Pacer {
    /// Milliseconds until the next capture may start (0: now), or None while a frame is in the encoder and the encoder has not asked for
    /// another (`encoder_asks`: the hardware encoder's `METransformNeedInput`, or the software one's "more input" with a frame inside). An
    /// encoder that keeps one frame until the next comes (one that does not honour low latency) asks, and is fed at the rung's rate; one that
    /// gives each frame back at once never asks, and has one at a time.
    pub fn due_in(&self, interval_ms: f64, now: u64, encoder_asks: bool) -> Option<u64> {
        if self.in_encoder > 0 && !encoder_asks {
            return None;
        }
        let Some(last) = self.last_start else { return Some(0) };
        let next = last as f64 + interval_ms.max(MIN_GAP_MS);
        Some((next - now as f64).max(0.0).ceil() as u64)
    }

    /// A picture was taken at `now` and handed to the encoder.
    pub fn started(&mut self, now: u64) {
        self.last_start = Some(now);
        self.in_encoder += 1;
    }

    /// The encoder gave a frame back (or it was given up on).
    pub fn done(&mut self) {
        self.in_encoder = self.in_encoder.saturating_sub(1);
    }

    /// The encoder was made again, or emptied: nothing is in it.
    pub fn clear(&mut self) {
        self.in_encoder = 0;
    }

    pub fn pending(&self) -> bool {
        self.in_encoder > 0
    }
}

/// A big frame's bitrate dip: the frame it is for (its time in the encoder) and the bitrate to go back to. However that frame leaves the
/// encoder (out, dropped, or given up on after a second), the bitrate goes back: a dip never stays.
#[derive(Debug, Default)]
pub struct Dip {
    held: Option<(i64, u32)>,
}

impl Dip {
    pub fn start(&mut self, time: i64, back: u32) {
        self.held = Some((time, back));
    }

    pub fn active(&self) -> bool {
        self.held.is_some()
    }

    /// The frame at `time` left the encoder: the bitrate to set again, when it was the dipped one.
    pub fn left(&mut self, time: i64) -> Option<u32> {
        match self.held {
            Some((t, back)) if t == time => {
                self.held = None;
                Some(back)
            }
            _ => None,
        }
    }

    /// The encoder was made again: whatever it was told is gone with it.
    pub fn clear(&mut self) {
        self.held = None;
    }
}
/// The least time between two encodes of the same still picture for a keyframe (a still window gives nothing new to encode).
pub const REENCODE_GAP_MS: u64 = 1000;

/// A keyframe owed on a still window encodes the last picture again: at most once a second, whatever owes it (a phone's request is gated
/// already; a failure on the helper's own side, a parameter set the rewriter refuses or an encoder with no room, would otherwise ask again on
/// every turn of the loop, an IDR and a readback each time).
#[derive(Debug, Default)]
pub struct ReencodeGate {
    last: Option<u64>,
}

impl ReencodeGate {
    pub fn ready(&self, now: u64) -> bool {
        self.last.is_none_or(|l| now.saturating_sub(l) >= REENCODE_GAP_MS)
    }

    pub fn used(&mut self, now: u64) {
        self.last = Some(now);
    }
}

/// CRD's `kMinQuantizer`: a quantizer at or below this is as good as it gets.
pub const TOP_OFF_QP: u32 = 10;
/// Passes at most for one still picture (ours: without a quantizer from the encoder, this bounds what refining costs).
pub const TOP_OFF_MAX: u32 = 10;
/// A pass this small (bytes) refined nothing worth more passes (ours).
pub const TOP_OFF_DONE_BYTES: usize = 200;
/// A change smaller than four 64×64 cells does not start a top-off (ours: a blinking caret would otherwise restart up to ten full-frame
/// passes every half second).
pub const TOP_OFF_MIN_AREA: u64 = 4 * 64 * 64;

/// The top-off of a still picture.
#[derive(Debug, Default)]
pub struct TopOff {
    active: bool,
    passes: u32,
    next_at: u64,
}

impl TopOff {
    /// CRD's top-off interval: `max(interval × 1.1, interval + 2 ms)`.
    pub fn gap_ms(interval_ms: f64) -> u64 {
        (interval_ms * 1.1).max(interval_ms + 2.0).ceil() as u64
    }

    /// A new picture changing `area` pixels was encoded at `now` (with `qp` when the encoder says): a still picture after it is topped off,
    /// unless it is already at the best quantizer. A change under `TOP_OFF_MIN_AREA` (a caret blinking, a clock's digit) starts nothing and
    /// stops nothing: a top-off under way goes on, and a converged picture is left alone.
    pub fn changed(&mut self, now: u64, interval_ms: f64, qp: Option<u32>, area: u64) {
        if area < TOP_OFF_MIN_AREA {
            return;
        }
        self.active = qp.is_none_or(|q| q > TOP_OFF_QP);
        self.passes = 0;
        self.next_at = now + Self::gap_ms(interval_ms);
    }

    /// Whether a pass is due now.
    pub fn due(&self, now: u64) -> bool {
        self.active && now >= self.next_at
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// A pass came out: `bytes` long, at `qp` when the encoder says.
    pub fn passed(&mut self, now: u64, interval_ms: f64, bytes: usize, qp: Option<u32>) {
        self.passes += 1;
        let converged = qp.is_some_and(|q| q <= TOP_OFF_QP) || bytes <= TOP_OFF_DONE_BYTES;
        if converged || self.passes >= TOP_OFF_MAX {
            self.active = false;
        }
        self.next_at = now + Self::gap_ms(interval_ms);
    }

    /// Nothing to refine any longer (a keyframe owed, the stream paused, a new size).
    pub fn stop(&mut self) {
        self.active = false;
    }
}

/// CRD's `kBigFrameThresholdPixels`.
pub const BIG_FRAME_PIXELS: u64 = 300_000;
/// CRD's `kEstimatedBytesPerMegapixel`.
pub const BIG_FRAME_BYTES_PER_MP: u64 = 100_000;
/// CRD's `kStatsWindow`: the frames whose largest update a new one is compared with.
pub const BIG_FRAME_WINDOW: usize = 5;
/// The bitrate a big frame is encoded at, in percent of the link's (ours: CRD raises the quantizer instead).
pub const BIG_FRAME_PERCENT: u32 = 40;

/// The large-frame guard.
#[derive(Debug, Default)]
pub struct BigFrame {
    areas: VecDeque<u64>,
}

impl BigFrame {
    /// A frame updating `area` pixels is about to be encoded at `kbps` with frames every `interval_ms`: true when its quality should drop so
    /// that it is not late. As CRD's `ShouldDropQualityForLargeFrame`, the area is remembered either way.
    pub fn check(&mut self, area: u64, kbps: u32, interval_ms: f64) -> bool {
        let largest = self.areas.iter().copied().max().unwrap_or(0);
        let big = area.saturating_sub(largest) > BIG_FRAME_PIXELS && {
            let bytes = area * BIG_FRAME_BYTES_PER_MP / 1_000_000;
            let send_ms = bytes as f64 * 8.0 / (f64::from(kbps.max(1)) * 1000.0) * 1000.0;
            send_ms > interval_ms
        };
        self.areas.push_back(area);
        while self.areas.len() > BIG_FRAME_WINDOW {
            self.areas.pop_front();
        }
        big
    }
}

/// CRD's `kEncoderBitrateChangePercentage`, for increases.
pub const RATE_UP_PERCENT: u32 = 33;

/// What the encoder was last told, and whether a new target is worth telling it.
#[derive(Debug)]
pub struct EncoderRate {
    pub applied: u32,
}

impl EncoderRate {
    pub fn new(kbps: u32) -> EncoderRate {
        EncoderRate { applied: kbps }
    }

    /// The bitrate to give the encoder now, if any: any cut at once, an increase only beyond 33 %.
    pub fn next(&mut self, target: u32) -> Option<u32> {
        let up = u64::from(self.applied) * u64::from(100 + RATE_UP_PERCENT) / 100;
        let change = target < self.applied || u64::from(target) > up;
        change.then(|| {
            self.applied = target;
            target
        })
    }
}

/// The pixels a list of rectangles covers (they may overlap: the sum, which only errs high).
pub fn area(rects: &[crate::geom::Rect]) -> u64 {
    rects.iter().map(|r| u64::from(r.w.max(0) as u32) * u64::from(r.h.max(0) as u32)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_follow_a_timer_from_the_last_start_with_one_frame_in_the_encoder_unless_it_asks() {
        let mut p = Pacer::default();
        assert_eq!(p.due_in(100.0, 0, false), Some(0), "the first capture at once");
        p.started(0);
        assert_eq!(p.due_in(100.0, 10, false), None, "a frame in the encoder: nothing scheduled");
        assert_eq!(p.due_in(100.0, 10, true), Some(90), "the encoder asks for more: the next at the rung's time");
        p.done();
        assert_eq!(p.due_in(100.0, 40, false), Some(60), "anchored to the last start, not to when it finished");
        assert_eq!(p.due_in(100.0, 150, false), Some(0), "late is now, never queued up");
        assert_eq!(p.due_in(0.2, 1, false), Some(1), "never under CRD's 1 ms floor");
        p.started(10);
        p.clear();
        assert!(!p.pending(), "a new encoder holds nothing");
    }

    /// An encoder that gives frame N back only once N+1 is in (one that does not honour low latency), asking for input as it should.
    #[test]
    fn an_encoder_that_holds_one_frame_is_fed_at_the_rate_not_once_a_second() {
        let mut p = Pacer::default();
        let (mut held, mut out, mut t) = (Vec::<u64>::new(), Vec::<(u64, u64)>::new(), 0u64);
        while t < 2000 {
            // It asks while it holds fewer than two (it needs the next to give one back).
            let asks = held.len() < 2;
            if p.due_in(100.0, t, asks) == Some(0) {
                p.started(t);
                held.push(t);
                if held.len() == 2 {
                    out.push((held.remove(0), t));
                    p.done();
                }
            }
            t += 5;
        }
        assert!(out.len() >= 18, "about ten frames a second at 100 ms, not one: {}", out.len());
        assert!(out.iter().all(|(into, back)| back - into <= 105), "each comes back one frame later, not after a second's give-up: {out:?}");
    }

    #[test]
    fn a_dip_is_undone_however_its_frame_leaves_the_encoder() {
        let mut d = Dip::default();
        d.start(77, 1500);
        assert!(d.active());
        assert_eq!(d.left(76), None, "another frame");
        assert_eq!(d.left(77), Some(1500), "out, dropped or given up on: the bitrate goes back");
        assert!(!d.active());
        assert_eq!(d.left(77), None, "once");
        d.start(80, 900);
        d.clear();
        assert!(!d.active());
    }

    const BIG: u64 = 100_000;

    #[test]
    fn a_still_picture_is_topped_off_on_crd_s_timer_until_the_encoder_says_it_is_as_good_as_it_gets() {
        assert_eq!(TopOff::gap_ms(33.3), 37, "1.1 × 33.3 is more than 33.3 + 2");
        assert_eq!(TopOff::gap_ms(10.0), 12, "10 + 2 is more than 1.1 × 10");
        let mut t = TopOff::default();
        assert!(!t.due(0));
        t.changed(0, 33.3, Some(30), BIG);
        assert!(!t.due(36) && t.due(37));
        t.passed(37, 33.3, 4000, Some(22));
        assert!(t.active() && !t.due(70) && t.due(74));
        t.passed(74, 33.3, 900, Some(TOP_OFF_QP));
        assert!(!t.active(), "at the best quantizer: done");
        // Without a quantizer from the encoder: a tiny pass ends it, or the passes run out.
        t.changed(100, 33.3, None, BIG);
        t.passed(140, 33.3, TOP_OFF_DONE_BYTES, None);
        assert!(!t.active());
        t.changed(200, 33.3, None, BIG);
        for i in 0..TOP_OFF_MAX {
            assert!(t.active());
            t.passed(240 + u64::from(i) * 40, 33.3, 5000, None);
        }
        assert!(!t.active(), "bounded");
        t.changed(1000, 33.3, Some(8), BIG);
        assert!(!t.active(), "already at its best: nothing to refine");
        t.changed(2000, 33.3, Some(30), BIG);
        t.stop();
        assert!(!t.due(5000));
        // A caret that blinks: no new top-off for it, and one under way is not started over.
        let mut c = TopOff::default();
        c.changed(0, 33.3, None, 1200);
        assert!(!c.active(), "a caret's few pixels start nothing");
        c.changed(0, 33.3, None, BIG);
        c.passed(40, 33.3, 5000, None);
        c.changed(50, 33.3, None, 1200);
        c.passed(90, 33.3, 5000, None);
        for i in 0..TOP_OFF_MAX - 2 {
            c.passed(130 + u64::from(i) * 40, 33.3, 5000, None);
        }
        assert!(!c.active(), "the caret did not start the passes over: still bounded by the first count");
    }

    #[test]
    fn a_large_update_that_would_be_late_is_sent_at_lower_quality_as_crd_s_guard_says() {
        let mut g = BigFrame::default();
        // A cursor blink, a line of text: small, never big.
        for _ in 0..5 {
            assert!(!g.check(20_000, 1500, 33.3));
        }
        // A window switch on 1080p: 2 MP, about 200 KB, a second at 1.5 Mbps: big.
        assert!(g.check(2_073_600, 1500, 33.3));
        // The same again right after: not larger than the largest of the last five by 300 000 px, so not "big" (it is the new normal).
        assert!(!g.check(2_073_600, 1500, 33.3));
        // At 100 Mbps it goes in time: not big.
        let mut fast = BigFrame::default();
        assert!(!fast.check(2_073_600, 100_000, 33.3));
        // Only the last five count.
        let mut h = BigFrame::default();
        h.check(2_000_000, 1500, 33.3);
        for _ in 0..BIG_FRAME_WINDOW {
            h.check(1000, 1500, 33.3);
        }
        assert!(h.check(2_000_000, 1500, 33.3), "the large one aged out");
    }

    #[test]
    fn a_still_window_s_last_picture_is_encoded_again_at_most_once_a_second() {
        let mut g = ReencodeGate::default();
        assert!(g.ready(0), "the first keyframe owed on a still window goes at once");
        g.used(0);
        // Every turn of the loop wants it again (a failure that persists): no more than once a second.
        let encodes = (4..=3000u64).step_by(4).filter(|t| {
            let go = g.ready(*t);
            if go {
                g.used(*t);
            }
            go
        });
        assert_eq!(encodes.count(), 3, "at 1000, 2000 and 3000 ms only");
    }

    #[test]
    fn the_encoder_hears_every_cut_at_once_and_an_increase_only_beyond_a_third() {
        let mut r = EncoderRate::new(1500);
        assert_eq!(r.next(1500), None);
        assert_eq!(r.next(1650), None, "10 % up: noise");
        assert_eq!(r.next(1995), None, "33 % up: not yet");
        assert_eq!(r.next(2000), Some(2000));
        assert_eq!(r.next(1700), Some(1700), "a cut goes at once");
        assert_eq!(r.applied, 1700);
    }
}
