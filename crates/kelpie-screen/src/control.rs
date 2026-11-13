//! How much picture to send: frames a second, size and bitrate, from three things that each may ask for less and never for more than
//! another allows: the encoder's own load (`Overuse`, libwebrtc's model), the phone's link (`Bandwidth`, from acknowledgements), and the
//! machine's load level as the daemon reports it (caps). Text is what people look at, so frames a second go first and the size last
//! (`RUNGS`); a window is never made shorter than 540 lines.
//!
//! The link is TCP under the Noise session: nothing is lost, only delayed. So frames are dropped before they are encoded, never after (a
//! dropped P-frame would break every frame after it): the stream encodes only while fewer frames and bytes than its window wait for the
//! phone's acknowledgement (`Controller::window`: enough for the round trip at the rung's frame rate, 3 frames and 192 KiB until a round trip
//! is known or when the daemon does not take another), and the newest capture waits in a one-slot mailbox meanwhile. Keyframes are for a
//! decoder that starts or lost its place, at most one a second (`KeyframeGate`).
//!
//! Every clock here is milliseconds passed in by the caller, so that all of it is tested without waiting.

use std::collections::VecDeque;

use serde::Serialize;

use crate::geom;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub fps: u8,
    /// Percent of the top rung's height: the source's, or `TOP_LINES` for a taller one (a 4K monitor at half is 540 lines, not 1080).
    pub scale: u8,
}

const fn rung(fps: u8, scale: u8) -> Rung {
    Rung { fps, scale }
}

/// Frames a second first, then size, down to about 360p at 5 fps for a slow link (200-300 kbps).
pub const RUNGS: [Rung; 9] = [rung(30, 100), rung(20, 100), rung(15, 100), rung(10, 100), rung(8, 100), rung(8, 75), rung(6, 67), rung(6, 50), rung(5, 33)];
pub const LOWEST: usize = RUNGS.len() - 1;
/// From this rung on, a keyframe always takes the big frame's bitrate dip (`pace::BigFrame`), so that it stays small on a slow link.
pub const KEY_DIP_RUNG: usize = 7;
/// A window is never shown shorter than this (unless it is shorter itself).
pub const MIN_WINDOW_LINES: u32 = 540;
/// Frames sent and not yet acknowledged, at most, before the encoder waits for the phone: this many until the round trip is known (and with a
/// daemon that does not take another window), and never fewer.
pub const MIN_WINDOW: u32 = 3;
/// …and never more, however long the round trip.
pub const MAX_WINDOW: u32 = 12;
/// The same for the bytes of those frames.
pub const MIN_WINDOW_BYTES: usize = 192 * 1024;
pub const MAX_WINDOW_BYTES: usize = 2 * 1024 * 1024;

/// What the machine's load level allows at most: (frames a second, lines).
pub fn load_cap(level: u8) -> Option<(u8, u32)> {
    match level {
        0 => None,
        1 => Some((20, 1080)),
        2 => Some((15, 720)),
        _ => Some((8, 540)),
    }
}

/// The software encoder's cap.
pub const SOFTWARE_CAP: (u8, u32) = (15, 720);
pub const SOFTWARE_MAX_KBPS: u32 = 2500;

// ---------- the encoder's own load ----------

const OVER: f64 = 0.85;
const UNDER: f64 = 0.42;
const OVER_FOR_MS: u64 = 2000;
const UNDER_FOR_MS: u64 = 10_000;
const UP_MIN_FRAMES: u32 = 60;
const EMA_ALPHA: f64 = 0.1;

/// `encode_usage = EMA(encode ms) / the frame interval of the rung`: over 0.85 for 2 s is one rung down, under 0.42 for 10 s (with 60 frames
/// seen since the last change) one up.
#[derive(Debug, Default)]
pub struct Overuse {
    ema: Option<f64>,
    over_since: Option<u64>,
    under_since: Option<u64>,
    frames: u32,
    pub rung: usize,
}

impl Overuse {
    pub fn usage(&self, interval_ms: f64) -> Option<f64> {
        self.ema.map(|e| e / interval_ms.max(1.0))
    }

    /// One frame took `encode_ms` at a rung whose frames come every `interval_ms`. True when the rung changed.
    pub fn sample(&mut self, encode_ms: f64, interval_ms: f64, now: u64) -> bool {
        let ema = self.ema.map_or(encode_ms, |e| e + EMA_ALPHA * (encode_ms - e));
        self.ema = Some(ema);
        self.frames = self.frames.saturating_add(1);
        let usage = ema / interval_ms.max(1.0);
        if usage > OVER {
            self.under_since = None;
            let since = *self.over_since.get_or_insert(now);
            if now.saturating_sub(since) >= OVER_FOR_MS && self.rung < LOWEST {
                self.rung += 1;
                self.changed();
                return true;
            }
        } else if usage < UNDER {
            self.over_since = None;
            let since = *self.under_since.get_or_insert(now);
            if now.saturating_sub(since) >= UNDER_FOR_MS && self.frames >= UP_MIN_FRAMES && self.rung > 0 {
                self.rung -= 1;
                self.changed();
                return true;
            }
        } else {
            self.over_since = None;
            self.under_since = None;
        }
        false
    }

    fn changed(&mut self) {
        self.over_since = None;
        self.under_since = None;
        self.frames = 0;
    }
}

// ---------- the link ----------

pub const START_KBPS: u32 = 1500;
pub const MIN_KBPS: u32 = 150;
pub const MAX_KBPS: u32 = 4000;
/// Queueing delay that counts as overuse: the round trip's floor of the last trend window (`trend_window_ms`) over its floor of
/// `MIN_RTT_WINDOW_MS` (the research's "act at +40-50 ms", GCC-like, instead of waiting for 150 ms for two seconds).
const QUEUE_LAG_MS: f64 = 40.0;
/// The least window the recent floor is taken over, and the least time between two cuts for queueing; at a low frame rate it is three frames.
const TREND_WINDOW_MS: u64 = 500;
/// Samples the recent floor needs: one late acknowledgement is not a trend (two at a low frame rate, so that a window of three frames can tell).
const TREND_MIN_SAMPLES: usize = 3;
/// The link's window of frames full this long, as the daemon saw it, is a cut too (twice the round trip, between half a second and a second:
/// `full_for_ms`); and this much longer again for each cut after.
const FULL_FOR_MS: u64 = 1000;
const FULL_FOR_MIN_MS: u64 = 500;
/// A cut keeps this share (percent): at once, as GCC's multiplicative decrease does.
const CUT_PERCENT: u32 = 85;
const MIN_RTT_WINDOW_MS: u64 = 10_000;
const UP_WAIT_MS: u64 = 10_000;
const UP_WAIT_MAX_MS: u64 = 60_000;
/// An up-step that held this long without a down-step resets the wait.
const UP_HELD_MS: u64 = 30_000;
/// No frame sent for this long: the stream is application-limited (a still screen), which says nothing about what the link carries.
const IDLE_AFTER_MS: u64 = 1000;
/// After a cut, the round trip's floor is looked at again this long after: one that held within `SHIFT_TOLERANCE_MS` is a new base (another
/// route, a slower decoder), not a queue, and becomes the floor; one that fell is a queue draining (no cut); only one that rose further cuts.
const SHIFT_AFTER_MS: u64 = 1000;
const SHIFT_TOLERANCE_MS: u32 = 15;
/// Queueing cuts at most this many in `CUT_WINDOW_MS`, whatever the round trips say.
const MAX_CUTS: usize = 3;
const CUT_WINDOW_MS: u64 = 10_000;
/// Crisp tiles turn themselves off below this bitrate, and for this long after the bitrate was cut (owner decision 2026-11-06).
pub const CRISP_MIN_KBPS: u32 = 1000;
pub const CRISP_CALM_MS: u64 = 10_000;

/// The bitrate the link takes, from what the phone acknowledges. **Queueing** (the round trip's floor over the last half second, counted from
/// after the last cut, above its floor over ten seconds by more than 50 ms) cuts 15 % at once; after a cut, a floor that rose further cuts
/// again half a second later, one that fell is a queue draining, and one that held for a second is a new base (another route, a slower
/// decoder), taken as the floor instead of being cut for; three queueing cuts in ten seconds at most, and a new connection forgets the round
/// trips before it. The link's window of frames **full** for a second cuts the same: full as the daemon sees it (`on_link_window`, frames it
/// queued for the phone and has no acknowledgement for), never as the helper counts it, since the helper's own count also holds the way to the
/// daemon and back and its own loop, which on a busy computer (a game, a build) are slow while the link is not: that slowness is the encoder
/// controller's (`Overuse`) and the window's gate (`Controller::may_encode`), not a cut. Ten seconds with neither, while frames are being
/// sent, add 10 %; an application-limited stream (a still screen, or frames held up on this computer) earns no increase, so a rate is never
/// raised on no evidence and motion that resumes after a pause starts from the last rate the link carried. An up-step undone within 10 s
/// doubles the wait before the next (to a minute), so a link that cannot hold a rate is not pushed to it every few seconds. Every cut is kept
/// with what its rule saw (`Cut`), for the daemon's log and the stats.
#[derive(Debug)]
pub struct Bandwidth {
    pub kbps: u32,
    rtts: VecDeque<(u64, u32)>,
    /// How long the daemon last said the link's window had been full (ms), and that figure when the window last made a cut.
    link_full_ms: u64,
    window_cut_at: Option<u64>,
    clean_since: u64,
    last_down: Option<u64>,
    last_up: Option<u64>,
    up_wait: u64,
    last_sent: Option<u64>,
    /// The round trip's recent floor when the last queueing cut was made, while that queue is being watched.
    cut_floor: Option<u32>,
    /// When the queueing cuts of the last `CUT_WINDOW_MS` were made.
    cuts: VecDeque<u64>,
    /// Every cut since the stream started, by rule (the stats' `link_cuts`: on a link with nothing to cut for, they stay 0).
    pub cut_counts: CutCounts,
    /// The cuts not yet taken for the stats (`take_cuts`), at most `MAX_CUT_LOG`.
    cut_log: Vec<Cut>,
    /// Bytes on the wire for each byte of media (the link pads every message to a few sizes), smoothed.
    wire: f64,
    /// The rung's frame rate and the window of frames in flight, as the controller last set them (`Controller::tick`): how many samples make
    /// a trend, and how many frames in flight mean the helper's own window is full.
    pub fps: u8,
    pub window: u32,
    /// The most this stream may send (`set_cap`).
    cap: u32,
}

/// Which rule cut the bitrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CutRule {
    /// The round trip's recent floor rose over its ten-second floor.
    Queueing,
    /// The link's window of frames was full, as the daemon saw it.
    Window,
    /// A frame came out too large for one message.
    Size,
}

/// A cut, with what its rule saw: the daemon logs each one, and the stats carry the last.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cut {
    pub rule: CutRule,
    pub from_kbps: u32,
    pub to_kbps: u32,
    /// The round trip's floor over the last half second and over ten seconds (ms), when there were samples.
    pub rtt_recent_ms: Option<u32>,
    pub rtt_base_ms: Option<u32>,
    /// How long the daemon had seen the link's window full (ms).
    pub link_full_ms: u64,
    /// The frames the helper counted in flight: its own count, which takes in this computer's part of the way.
    pub inflight: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CutCounts {
    pub queueing: u32,
    pub window: u32,
    pub size: u32,
}

impl CutCounts {
    pub fn total(&self) -> u32 {
        self.queueing + self.window + self.size
    }
}

/// Cuts kept for the next stats (a second) at most: the rules cut far less often than this.
const MAX_CUT_LOG: usize = 16;

impl Bandwidth {
    pub fn new(now: u64) -> Bandwidth {
        Bandwidth {
            kbps: START_KBPS,
            rtts: VecDeque::new(),
            link_full_ms: 0,
            window_cut_at: None,
            clean_since: now,
            last_down: None,
            last_up: None,
            up_wait: UP_WAIT_MS,
            last_sent: None,
            cut_floor: None,
            cuts: VecDeque::new(),
            cut_counts: CutCounts::default(),
            cut_log: Vec::new(),
            wire: 1.0,
            fps: RUNGS[0].fps,
            window: MIN_WINDOW,
            cap: MAX_KBPS,
        }
    }

    /// The most this stream may send, whatever the link carries (a way held to a rate): the rate is lowered to it now, and never raised past it.
    pub fn set_cap(&mut self, kbps: u32) {
        self.cap = kbps.clamp(MIN_KBPS, MAX_KBPS);
        self.kbps = self.kbps.min(self.cap);
    }

    /// The round trip's floor over the last ten seconds (ms), once there is one.
    pub fn rtt_base(&self) -> Option<u32> {
        self.rtts.iter().map(|(_, r)| *r).min()
    }

    /// The window the recent floor is taken over: half a second, or three frames at a low frame rate.
    fn trend_window_ms(&self) -> u64 {
        TREND_WINDOW_MS.max(3000 / u64::from(self.fps.max(1)))
    }

    /// Samples a trend needs: three, or two below 12 frames a second (`max(2, min(3, fps / 4))`).
    fn trend_samples(&self) -> usize {
        (usize::from(self.fps) / 4).clamp(2, TREND_MIN_SAMPLES)
    }

    /// How long the link's window must be full for a cut: twice the round trip's floor, between half a second and a second (a second without one).
    fn full_for_ms(&self) -> u64 {
        self.rtt_base().map_or(FULL_FOR_MS, |r| (2 * u64::from(r)).clamp(FULL_FOR_MIN_MS, FULL_FOR_MS))
    }

    /// The bitrate goes to `to` for `rule`, and the cut is kept with what was seen. False when it was already there (the floor).
    fn cut_to(&mut self, to: u32, rule: CutRule, inflight: u32) -> bool {
        if to == self.kbps {
            return false;
        }
        let cut = Cut { rule, from_kbps: self.kbps, to_kbps: to, rtt_recent_ms: self.recent_floor(), rtt_base_ms: self.rtt_base(), link_full_ms: self.link_full_ms, inflight };
        self.kbps = to;
        match rule {
            CutRule::Queueing => self.cut_counts.queueing += 1,
            CutRule::Window => self.cut_counts.window += 1,
            CutRule::Size => self.cut_counts.size += 1,
        }
        if self.cut_log.len() < MAX_CUT_LOG {
            self.cut_log.push(cut);
        }
        true
    }

    /// A frame too large for one message: a quarter off at once (the frame was not sent).
    pub fn cut_for_size(&mut self, inflight: u32) {
        self.cut_to((self.kbps * 3 / 4).max(MIN_KBPS), CutRule::Size, inflight);
    }

    /// The cuts since the last call, oldest first.
    pub fn take_cuts(&mut self) -> Vec<Cut> {
        std::mem::take(&mut self.cut_log)
    }

    /// What the daemon says of the link's window with each flow: how long it has been full (0: it is not). A figure lower than the one the
    /// last window cut was made at is a new spell of it.
    pub fn on_link_window(&mut self, full_ms: u64) {
        self.link_full_ms = full_ms;
        if self.window_cut_at.is_some_and(|at| full_ms < at) {
            self.window_cut_at = None;
        }
    }

    /// A new connection (the phone came back, maybe by another route: LAN to relay, Wi-Fi to LTE): the round trips before say nothing about it.
    pub fn new_route(&mut self) {
        self.rtts.clear();
        self.cut_floor = None;
    }

    /// The round trip's floor over the last trend window, counted from after the last cut, with enough samples to be a trend.
    fn recent_floor(&self) -> Option<u32> {
        let latest = self.rtts.back()?.0;
        let from = latest.saturating_sub(self.trend_window_ms()).max(self.last_down.map_or(0, |d| d + 1));
        let recent: Vec<u32> = self.rtts.iter().filter(|(t, _)| *t >= from).map(|(_, r)| *r).collect();
        (recent.len() >= self.trend_samples()).then(|| recent.into_iter().min()).flatten()
    }

    /// Whether the queueing seen now is worth a cut: half a second after the last one; once a cut's floor has held for a second it is a new
    /// base (the floor is taken again from what came after the cut, and nothing is cut); a floor that fell is a queue draining; at most
    /// `MAX_CUTS` in `CUT_WINDOW_MS`.
    fn queue_cut_due(&mut self, now: u64) -> bool {
        match self.queueing_ms() {
            Some(q) if q > QUEUE_LAG_MS => {}
            // No queue any more: the last cut's queue is over.
            Some(_) => {
                self.cut_floor = None;
                return false;
            }
            // Nothing to tell yet (no samples since the cut): wait.
            None => return false,
        }
        if self.last_down.is_some_and(|d| now.saturating_sub(d) < self.trend_window_ms()) {
            return false;
        }
        let Some(recent) = self.recent_floor() else { return false };
        if let (Some(before), Some(down)) = (self.cut_floor, self.last_down) {
            if recent + SHIFT_TOLERANCE_MS < before {
                return false;
            }
            if recent <= before + SHIFT_TOLERANCE_MS {
                if now.saturating_sub(down) >= SHIFT_AFTER_MS {
                    self.rtts.retain(|(t, _)| *t > down);
                    self.cut_floor = None;
                }
                return false;
            }
        }
        self.cuts.retain(|t| now.saturating_sub(*t) < CUT_WINDOW_MS);
        if self.cuts.len() >= MAX_CUTS {
            return false;
        }
        self.cut_floor = Some(recent);
        self.cuts.push_back(now);
        true
    }

    /// A frame's round trip, from the daemon sending it to the phone's acknowledgement.
    pub fn on_ack(&mut self, rtt_ms: u32, now: u64) {
        self.rtts.push_back((now, rtt_ms));
        while self.rtts.front().is_some_and(|(t, _)| now.saturating_sub(*t) > MIN_RTT_WINDOW_MS) {
            self.rtts.pop_front();
        }
    }

    /// A frame went out.
    pub fn on_sent(&mut self, now: u64) {
        self.last_sent = Some(now);
    }

    /// No frame went out lately: what the link carries is not being tested.
    pub fn app_limited(&self, now: u64) -> bool {
        self.last_sent.is_none_or(|s| now.saturating_sub(s) >= IDLE_AFTER_MS)
    }

    /// A media message of `raw` bytes went out as `wire` bytes (the daemon works it out from the link's padding).
    pub fn on_wire(&mut self, raw: u64, wire: u64) {
        if raw == 0 {
            return;
        }
        let ratio = (wire as f64 / raw as f64).clamp(1.0, 4.0);
        self.wire += 0.1 * (ratio - self.wire);
    }

    pub fn wire_factor(&self) -> f64 {
        self.wire.clamp(1.0, 4.0)
    }

    /// The queueing delay now: the floor of the round trips of the last `TREND_WINDOW_MS` (after the last cut) over the floor of ten seconds.
    pub fn queueing_ms(&self) -> Option<f64> {
        Some(f64::from(self.recent_floor()? - self.rtt_base()?))
    }

    pub fn lagging(&self, inflight: u32) -> bool {
        inflight >= self.window || self.queueing_ms().is_some_and(|q| q > QUEUE_LAG_MS)
    }

    /// Looked at in every turn with the frames in flight now, as the helper counts them. True when the bitrate changed.
    pub fn tick(&mut self, inflight: u32, now: u64) -> bool {
        let queueing = self.queueing_ms().is_some_and(|q| q > QUEUE_LAG_MS);
        let queue_cut = self.queue_cut_due(now);
        let full_for = self.full_for_ms();
        let link_full = self.link_full_ms >= full_for;
        // That long of it for the first cut, and a second more of the same spell for each cut after it (a link that stalls altogether, for a
        // dropped path, is not cut to the floor twice as fast).
        let window_cut = self.link_full_ms >= self.window_cut_at.map_or(full_for, |at| at + FULL_FOR_MS);
        if queueing || link_full {
            self.clean_since = now;
            if queue_cut || window_cut {
                if self.last_up.is_some_and(|u| now.saturating_sub(u) < UP_WAIT_MS) {
                    self.up_wait = (self.up_wait * 2).min(UP_WAIT_MAX_MS);
                }
                if link_full {
                    self.window_cut_at = Some(self.link_full_ms);
                }
                self.last_down = Some(now);
                let rule = if queue_cut { CutRule::Queueing } else { CutRule::Window };
                return self.cut_to((self.kbps * CUT_PERCENT / 100).max(MIN_KBPS), rule, inflight);
            }
            return false;
        }
        // The helper's own window full while the link's is not: frames wait on this computer, which says nothing of what the link carries.
        // No cut, and no increase either.
        if inflight >= self.window || self.app_limited(now) {
            self.clean_since = now;
            return false;
        }
        if let Some(up) = self.last_up {
            if now.saturating_sub(up) >= UP_HELD_MS && self.last_down.is_none_or(|d| d < up) {
                self.up_wait = UP_WAIT_MS;
            }
        }
        if now.saturating_sub(self.clean_since) >= self.up_wait && self.kbps < self.cap {
            self.kbps = (self.kbps * 11 / 10).max(self.kbps + 50).min(self.cap);
            self.last_up = Some(now);
            self.clean_since = now;
            return true;
        }
        false
    }

    /// Whether the link has room for crisp tiles on top of the video: at least `CRISP_MIN_KBPS`, no lag now, and no cut of the bitrate in the
    /// last `CRISP_CALM_MS`. Measured, so it holds for any network: a phone page cannot tell cellular from Wi-Fi on iOS.
    pub fn calm(&self, inflight: u32, now: u64) -> bool {
        self.kbps >= CRISP_MIN_KBPS && !self.lagging(inflight) && self.last_down.is_none_or(|d| now.saturating_sub(d) >= CRISP_CALM_MS)
    }

    /// The rung the bitrate affords: 1080p30 → 1080p15 → 1080p8 → 75 % at 8 → 67 % at 6 → half at 6 → a third at 5.
    pub fn rung(&self) -> usize {
        match self.kbps {
            k if k >= 1200 => 0,
            k if k >= 800 => 2,
            k if k >= 500 => 4,
            k if k >= 380 => 5,
            k if k >= 300 => 6,
            k if k >= 220 => 7,
            _ => 8,
        }
    }
}

// ---------- keyframes ----------

/// At most one keyframe a second, whoever asks: a request within the second waits for its end, and several are one.
#[derive(Debug, Default)]
pub struct KeyframeGate {
    last: Option<u64>,
    pending: bool,
}

pub const KEYFRAME_GAP_MS: u64 = 1000;

impl KeyframeGate {
    /// True when the next frame should be a keyframe now.
    pub fn request(&mut self, now: u64) -> bool {
        if self.last.is_none_or(|l| now.saturating_sub(l) >= KEYFRAME_GAP_MS) {
            self.last = Some(now);
            self.pending = false;
            true
        } else {
            self.pending = true;
            false
        }
    }

    /// A request that waited: true once its second is over.
    pub fn due(&mut self, now: u64) -> bool {
        if self.pending && self.last.is_none_or(|l| now.saturating_sub(l) >= KEYFRAME_GAP_MS) {
            self.pending = false;
            self.last = Some(now);
            return true;
        }
        false
    }

    /// The encoder made one on its own (the first frame, a new size).
    pub fn made(&mut self, now: u64) {
        self.last = Some(now);
        self.pending = false;
    }

    /// A request waits for its second to be over.
    pub fn waiting(&self) -> bool {
        self.pending
    }
}

/// The frames on their way to the phone and the keyframe owed, so that frames that are lost never hold the stream: the phone (or the daemon on its
/// way) drops everything after a lost frame until a keyframe, so when a keyframe is asked for, or the daemon says it dropped frames, those frames
/// will never be acknowledged and count as done. While a keyframe is owed but not yet allowed (at most one a second), no other frame is encoded:
/// it would only be dropped.
#[derive(Debug)]
pub struct Flight {
    seq: u32,
    acked: u32,
    key: bool,
    gate: KeyframeGate,
    /// The frames in flight and their sizes.
    sizes: VecDeque<(u32, usize)>,
}

impl Default for Flight {
    fn default() -> Flight {
        Flight { seq: 0, acked: 0, key: true, gate: KeyframeGate::default(), sizes: VecDeque::new() }
    }
}

impl Flight {
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Frames sent and neither acknowledged nor given up.
    pub fn inflight(&self) -> u32 {
        self.seq.saturating_sub(self.acked)
    }

    /// Their bytes.
    pub fn inflight_bytes(&self) -> usize {
        self.sizes.iter().map(|(_, b)| b).sum()
    }

    fn forget_through(&mut self, seq: u32) {
        while self.sizes.front().is_some_and(|(s, _)| *s <= seq) {
            self.sizes.pop_front();
        }
    }

    /// The next frame is to be a keyframe.
    pub fn wants_key(&self) -> bool {
        self.key
    }

    /// A keyframe is owed and not yet allowed: encode nothing now.
    pub fn gated(&self) -> bool {
        !self.key && self.gate.waiting()
    }

    /// A frame of `bytes` goes out: its number.
    pub fn sent(&mut self, bytes: usize) -> u32 {
        self.seq += 1;
        self.sizes.push_back((self.seq, bytes));
        self.seq
    }

    /// The encoder was handed a frame (a keyframe when `key`).
    pub fn encoded(&mut self, key: bool, now: u64) {
        if key {
            self.key = false;
            self.gate.made(now);
        }
    }

    /// The phone acknowledged frame `seq` (and so every one before it).
    pub fn acked(&mut self, seq: u32) {
        self.acked = self.acked.max(seq.min(self.seq));
        self.forget_through(self.acked);
    }

    /// The daemon dropped every frame up to `seq`: none of them will be acknowledged.
    pub fn dropped_through(&mut self, seq: u32) {
        self.acked(seq);
    }

    /// The phone or the daemon asks for a keyframe: what is on its way is lost to it. The keyframe goes now, or once its second is over.
    pub fn ask_key(&mut self, now: u64) {
        self.acked = self.seq;
        self.sizes.clear();
        if self.gate.request(now) {
            self.key = true;
        }
    }

    /// Looked at in every turn: a keyframe that waited for its second goes now.
    pub fn tick(&mut self, now: u64) {
        if self.gate.due(now) {
            self.key = true;
            self.acked = self.seq;
            self.sizes.clear();
        }
    }

    /// A new connection, or a decoder that starts over: nothing before counts, a keyframe goes first.
    pub fn restart(&mut self) {
        self.acked = self.seq;
        self.sizes.clear();
        self.key = true;
    }
}

// ---------- all of it ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    None,
    Cpu,
    Bandwidth,
    Load,
    Encoder,
}

/// What the encoder is asked for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub fps: u8,
    pub w: u32,
    pub h: u32,
    pub kbps: u32,
    pub reason: Reason,
    /// Fewer than 8 frames a second, or smaller than the source: the phone shows "reduced".
    pub degraded: bool,
}

/// The ladder's top rung: the encoder's picture is never larger than this, whatever the source (a 4K or 5K monitor is shown at 1080p; the crisp
/// tiles keep its text at the source's own resolution). 1920×1080 is H.264 level 4, which every encoder and phone decoder takes.
pub const TOP_LINES: u32 = 1080;
pub const TOP_WIDTH: u32 = 1920;

/// The encoder's picture at `scale` percent of the top rung's height (`TOP_LINES` for a taller source), at most `max_lines` tall and never wider
/// than `TOP_WIDTH` (an ultrawide monitor is shown smaller), even sides.
fn fit(src: (u32, u32), scale: u8, min_lines: u32, max_lines: u32) -> (u32, u32) {
    let lines = u64::from(src.1.min(TOP_LINES)) * u64::from(scale.clamp(1, 100)) / 100;
    let (w, h) = geom::fitted(src.0, src.1, lines as u32, min_lines, max_lines.min(TOP_LINES));
    if w <= TOP_WIDTH || w == 0 {
        return (w, h);
    }
    let h = ((u64::from(h) * u64::from(TOP_WIDTH) / u64::from(w)) as u32 & !1).max(2);
    (TOP_WIDTH, h)
}

/// L3 this long pauses the stream until the person asks for it again.
pub const L3_PAUSE_MS: u64 = 5 * 60 * 1000;
const HW_SLOW_MS: f64 = 33.0;
const HW_SLOW_FOR_MS: u64 = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// L3 held for five minutes: pause, and say why.
    PausedForLoad,
    /// The hardware encoder cannot keep up even at the lowest rung: use the software one.
    UseSoftware,
}

#[derive(Debug)]
pub struct Controller {
    pub overuse: Overuse,
    pub bandwidth: Bandwidth,
    src: (u32, u32),
    window: bool,
    level: u8,
    software: bool,
    l3_since: Option<u64>,
    paused_for_load: bool,
    hw_times: VecDeque<(u64, f64)>,
    hw_slow_since: Option<u64>,
    asked_software: bool,
    /// The daemon takes a window sized to the link (`win` in its flow): until it says so, the window stays `MIN_WINDOW`.
    win: bool,
}

impl Controller {
    pub fn new(src: (u32, u32), window: bool, software: bool, now: u64) -> Controller {
        Controller { overuse: Overuse::default(), bandwidth: Bandwidth::new(now), src, window, level: 0, software, l3_since: None, paused_for_load: false, hw_times: VecDeque::new(), hw_slow_since: None, asked_software: false, win: false }
    }

    /// Whether the daemon takes a window sized to the link.
    pub fn set_win(&mut self, on: bool) {
        self.win = on;
    }

    pub fn win(&self) -> bool {
        self.win
    }

    /// Whether a frame takes the big frame's bitrate dip whatever its size: a keyframe on a slow link (from `KEY_DIP_RUNG` down), so that it
    /// stays small.
    pub fn key_dip(&self, key: bool) -> bool {
        key && self.rung() >= KEY_DIP_RUNG
    }

    /// The frames and bytes that may wait for the phone's acknowledgement: enough for the round trip's floor at the rung's frame rate
    /// (`ceil(fps × (rtt + 50 ms)) + 1` frames, between 3 and 12; the rate × (rtt + 100 ms) in bytes, between 192 KiB and 2 MiB). A deeper
    /// window keeps a long link busy (3 frames cost 7.5 fps at 100 ms); the queueing cut still bounds how old a picture gets. 3 frames and
    /// 192 KiB until a round trip is known, and for a daemon that does not take another.
    pub fn window(&self) -> (u32, usize) {
        let Some(rtt) = self.bandwidth.rtt_base().filter(|_| self.win) else { return (MIN_WINDOW, MIN_WINDOW_BYTES) };
        let fps = u64::from(self.params().fps);
        let frames = ((fps * (u64::from(rtt) + 50)).div_ceil(1000) + 1).clamp(u64::from(MIN_WINDOW), u64::from(MAX_WINDOW)) as u32;
        let bytes = (u64::from(self.bandwidth.kbps) * (u64::from(rtt) + 100) / 8).clamp(MIN_WINDOW_BYTES as u64, MAX_WINDOW_BYTES as u64) as usize;
        (frames, bytes)
    }

    pub fn set_source(&mut self, src: (u32, u32)) {
        self.src = src;
    }

    pub fn set_software(&mut self, on: bool) {
        self.software = on;
    }

    pub fn software(&self) -> bool {
        self.software
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn paused_for_load(&self) -> bool {
        self.paused_for_load
    }

    /// The daemon's load level.
    pub fn set_level(&mut self, level: u8, now: u64) {
        self.level = level.min(3);
        if self.level >= 3 {
            self.l3_since.get_or_insert(now);
        } else {
            self.l3_since = None;
        }
    }

    /// The person asked to go on after a pause for load: five more minutes of L3 before the next.
    pub fn resume(&mut self, now: u64) {
        self.paused_for_load = false;
        if self.l3_since.is_some() {
            self.l3_since = Some(now);
        }
    }

    pub fn rung(&self) -> usize {
        self.overuse.rung.max(self.bandwidth.rung())
    }

    pub fn params(&self) -> Params {
        let at = self.rung();
        let r = RUNGS[at];
        let min_lines = if self.window { MIN_WINDOW_LINES } else { 0 };
        let mut reason = if at == 0 { Reason::None } else if self.overuse.rung >= self.bandwidth.rung() { Reason::Cpu } else { Reason::Bandwidth };
        let (free_w, free_h) = fit(self.src, r.scale, min_lines, u32::MAX);
        let (mut fps, mut max_lines) = (r.fps, u32::MAX);
        let mut cap = |c: (u8, u32), why: Reason, fps: &mut u8, max_lines: &mut u32| {
            if c.0 < *fps || c.1 < free_h.min(*max_lines) {
                reason = why;
            }
            *fps = (*fps).min(c.0);
            *max_lines = (*max_lines).min(c.1);
        };
        if let Some(c) = load_cap(self.level) {
            cap(c, Reason::Load, &mut fps, &mut max_lines);
        }
        if self.software {
            cap(SOFTWARE_CAP, Reason::Encoder, &mut fps, &mut max_lines);
        }
        let (w, h) = if max_lines == u32::MAX { (free_w, free_h) } else { fit(self.src, r.scale, min_lines, max_lines) };
        // What goes on the wire is more than what the encoder makes (the link pads each message): the encoder is asked for less.
        let link = (f64::from(self.bandwidth.kbps) / self.bandwidth.wire_factor()) as u32;
        let kbps = if self.software { link.min(SOFTWARE_MAX_KBPS) } else { link }.max(MIN_KBPS / 2);
        // Smaller than the top rung's own picture is "reduced"; a 4K monitor shown at 1080p is not.
        let degraded = fps < 8 || h < fit(self.src, 100, min_lines, u32::MAX).1;
        Params { fps, w, h, kbps, reason, degraded }
    }

    pub fn interval_ms(&self) -> f64 {
        1000.0 / f64::from(self.params().fps.max(1))
    }

    /// A frame was encoded in `encode_ms`.
    pub fn on_encoded(&mut self, encode_ms: f64, now: u64) -> Option<Event> {
        let interval = self.interval_ms();
        self.overuse.sample(encode_ms, interval, now);
        if self.software || self.asked_software {
            return None;
        }
        self.hw_times.push_back((now, encode_ms));
        while self.hw_times.front().is_some_and(|(t, _)| now.saturating_sub(*t) > HW_SLOW_FOR_MS) {
            self.hw_times.pop_front();
        }
        let p95 = percentile(self.hw_times.iter().map(|(_, ms)| *ms), 0.95);
        if self.rung() == LOWEST && p95.is_some_and(|p| p > HW_SLOW_MS) {
            let since = *self.hw_slow_since.get_or_insert(now);
            if now.saturating_sub(since) >= HW_SLOW_FOR_MS {
                self.asked_software = true;
                return Some(Event::UseSoftware);
            }
        } else {
            self.hw_slow_since = None;
        }
        None
    }

    /// Looked at once in a while: the link, and how long L3 has lasted.
    pub fn tick(&mut self, inflight: u32, now: u64) -> Option<Event> {
        self.bandwidth.fps = self.params().fps;
        self.bandwidth.window = self.window().0;
        self.bandwidth.tick(inflight, now);
        if !self.paused_for_load && self.l3_since.is_some_and(|s| now.saturating_sub(s) >= L3_PAUSE_MS) {
            self.paused_for_load = true;
            return Some(Event::PausedForLoad);
        }
        None
    }

    /// Whether crisp tiles may go now (the person's switch aside): a calm link, and a machine not at L3.
    pub fn crisp_ok(&self, inflight: u32, now: u64) -> bool {
        !self.paused_for_load && self.level < 3 && self.bandwidth.calm(inflight, now)
    }

    /// Whether a frame may be encoded now: the window has room for it (frames and bytes in flight), and the rung's interval has passed since
    /// the last one.
    pub fn may_encode(&self, inflight: u32, inflight_bytes: usize, since_last_ms: Option<u64>) -> bool {
        let (frames, bytes) = self.window();
        !self.paused_for_load && inflight < frames && inflight_bytes < bytes && since_last_ms.is_none_or(|ms| ms as f64 + 0.5 >= self.interval_ms())
    }
}

fn round1(v: Option<f64>) -> Option<f64> {
    v.map(|x| (x * 10.0).round() / 10.0)
}

/// The `q` quantile of the values, None for none.
pub fn percentile(values: impl Iterator<Item = f64>, q: f64) -> Option<f64> {
    let mut v: Vec<f64> = values.filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let i = ((v.len() as f64 - 1.0) * q.clamp(0.0, 1.0)).round() as usize;
    Some(v[i])
}

// ---------- what the phone hears each second ----------

#[derive(Debug, Default)]
pub struct Stats {
    since: u64,
    frames: u32,
    bytes: u64,
    encode_ms: Vec<f64>,
    pub dropped_encoder: u32,
    pub dropped_link: u32,
    key_bytes: u64,
    keys: u32,
    delta_bytes: u64,
    deltas: u32,
    capture_to_send: Vec<f64>,
    convert: Vec<f64>,
    send_intervals: Vec<f64>,
    ack_rtts: Vec<f64>,
    last_send: Option<u64>,
    pub top_off: u32,
    pub big_frames: u32,
    /// Whether DXGI's first frame after a duplication was made said everything changed (1) or not (0): kept from one report to the next.
    pub dxgi_first_full: Option<u8>,
    /// The link controller's cuts since the stream started (`Bandwidth::cut_counts`), set before each report.
    pub link_cuts: CutCounts,
    /// The window of frames in flight (`Controller::window`), set before each report.
    pub inflight_limit: u32,
    /// The cuts since the last report (`cut`), and the last one ever, kept from one report to the next.
    cuts: Vec<Cut>,
    last_cut: Option<Cut>,
}

/// Where a frame's time went in the helper over the last second (ms), for the measurement plan: from the picture being taken to its message
/// leaving for the app, the GPU conversion, the encoder alone, and the interval between frames sent; with how many were top-off passes or
/// had their quality lowered as too large (`pace::BigFrame`), and what the encoder held at the report.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stages {
    pub capture_to_send_ms_p50: Option<f64>,
    pub capture_to_send_ms_p95: Option<f64>,
    pub convert_ms_p95: Option<f64>,
    pub encode_ms_p50: Option<f64>,
    pub send_interval_ms_p95: Option<f64>,
    /// From the daemon queueing a frame to the phone acknowledging it (its decoder done): the daemon's queue, the wire and the phone.
    pub ack_rtt_ms_p50: Option<f64>,
    pub ack_rtt_ms_p95: Option<f64>,
    pub top_off: u32,
    pub big_frames: u32,
    pub held: u64,
    pub first_output_ms: Option<f64>,
    pub dxgi_first_full: Option<u8>,
    /// The bitrate's cuts since the stream started, in all and by rule, and what the last one saw: the round trip's recent floor over its
    /// ten-second floor, how long the daemon had seen the link's window full, and the frames the helper counted in flight.
    pub link_cuts: u32,
    pub link_cuts_queueing: u32,
    pub link_cuts_window: u32,
    pub link_cuts_size: u32,
    pub last_cut_queueing_ms: Option<u32>,
    pub last_cut_link_full_ms: Option<u64>,
    pub last_cut_inflight: Option<u32>,
    /// The window of frames that may wait for the phone (`Controller::window`).
    pub inflight_limit: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub fps: u32,
    pub w: u32,
    pub h: u32,
    pub kbps: u32,
    pub target_kbps: u32,
    pub encode_ms_p95: f64,
    pub dropped: u32,
    pub reason: Reason,
    pub degraded: bool,
    pub level: u8,
    pub encoder: &'static str,
    /// A keyframe's size over the mean other frame's, when both were seen.
    pub idr_ratio: Option<f64>,
    /// Crisp tiles: on, off for now because the link is slow (`false`), or switched off in Settings (`None`).
    pub crisp: Option<bool>,
    pub stages: Stages,
    /// Each cut of the bitrate since the last report, with what its rule saw: the daemon logs them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<Cut>,
}

impl Stats {
    pub fn new(now: u64) -> Stats {
        Stats { since: now, ..Default::default() }
    }

    pub fn frame(&mut self, bytes: usize, key: bool, encode_ms: f64) {
        self.frames += 1;
        self.bytes += bytes as u64;
        if self.encode_ms.len() < 240 {
            self.encode_ms.push(encode_ms);
        }
        if key {
            self.keys += 1;
            self.key_bytes += bytes as u64;
        } else {
            self.deltas += 1;
            self.delta_bytes += bytes as u64;
        }
    }

    /// A frame left for the app at `now`: `capture_to_send_ms` after its picture was taken, `convert_ms` of it on the GPU.
    pub fn timing(&mut self, now: u64, capture_to_send_ms: f64, convert_ms: f64) {
        if self.capture_to_send.len() < 240 {
            self.capture_to_send.push(capture_to_send_ms);
            self.convert.push(convert_ms);
        }
        if let Some(last) = self.last_send.replace(now) {
            if self.send_intervals.len() < 240 {
                self.send_intervals.push(now.saturating_sub(last) as f64);
            }
        }
    }

    /// A cut of the bitrate, for the next report.
    pub fn cut(&mut self, c: Cut) {
        if self.cuts.len() < MAX_CUT_LOG {
            self.cuts.push(c.clone());
        }
        self.last_cut = Some(c);
    }

    /// The phone acknowledged a frame `rtt_ms` after the daemon queued it.
    pub fn ack(&mut self, rtt_ms: u32) {
        if self.ack_rtts.len() < 240 {
            self.ack_rtts.push(f64::from(rtt_ms));
        }
    }

    /// The second's report, once a second has passed; the counters start again (the interval carries on from the last frame).
    pub fn report(&mut self, now: u64, p: &Params, level: u8, software: bool, crisp: Option<bool>) -> Option<Report> {
        self.report_with(now, p, level, software, crisp, 0, None)
    }

    /// `report`, with what the encoder holds and its first output's delay.
    #[allow(clippy::too_many_arguments)]
    pub fn report_with(&mut self, now: u64, p: &Params, level: u8, software: bool, crisp: Option<bool>, held: u64, first_output_ms: Option<f64>) -> Option<Report> {
        let span = now.saturating_sub(self.since);
        if span < 1000 {
            return None;
        }
        let per_s = |n: u64| (n * 1000 / span.max(1)) as u32;
        let idr_ratio = (self.keys > 0 && self.deltas > 0 && self.delta_bytes > 0).then(|| {
            let key = self.key_bytes as f64 / f64::from(self.keys);
            let delta = self.delta_bytes as f64 / f64::from(self.deltas);
            (key / delta * 10.0).round() / 10.0
        });
        let r = Report {
            fps: per_s(u64::from(self.frames)),
            w: p.w,
            h: p.h,
            kbps: per_s(self.bytes * 8) / 1000,
            target_kbps: p.kbps,
            encode_ms_p95: percentile(self.encode_ms.iter().copied(), 0.95).map_or(0.0, |v| (v * 10.0).round() / 10.0),
            dropped: self.dropped_encoder + self.dropped_link,
            reason: p.reason,
            degraded: p.degraded,
            level,
            encoder: if software { "sw" } else { "hw" },
            idr_ratio,
            crisp,
            stages: Stages {
                capture_to_send_ms_p50: round1(percentile(self.capture_to_send.iter().copied(), 0.5)),
                capture_to_send_ms_p95: round1(percentile(self.capture_to_send.iter().copied(), 0.95)),
                convert_ms_p95: round1(percentile(self.convert.iter().copied(), 0.95)),
                encode_ms_p50: round1(percentile(self.encode_ms.iter().copied(), 0.5)),
                send_interval_ms_p95: round1(percentile(self.send_intervals.iter().copied(), 0.95)),
                ack_rtt_ms_p50: round1(percentile(self.ack_rtts.iter().copied(), 0.5)),
                ack_rtt_ms_p95: round1(percentile(self.ack_rtts.iter().copied(), 0.95)),
                top_off: self.top_off,
                big_frames: self.big_frames,
                held,
                first_output_ms: round1(first_output_ms),
                dxgi_first_full: self.dxgi_first_full,
                link_cuts: self.link_cuts.total(),
                link_cuts_queueing: self.link_cuts.queueing,
                link_cuts_window: self.link_cuts.window,
                link_cuts_size: self.link_cuts.size,
                last_cut_queueing_ms: self.last_cut.as_ref().and_then(|c| c.rtt_recent_ms.zip(c.rtt_base_ms)).map(|(recent, base)| recent.saturating_sub(base)),
                last_cut_link_full_ms: self.last_cut.as_ref().map(|c| c.link_full_ms),
                last_cut_inflight: self.last_cut.as_ref().map(|c| c.inflight),
                inflight_limit: self.inflight_limit,
            },
            cuts: std::mem::take(&mut self.cuts),
        };
        let (last_send, dxgi_first_full, last_cut, inflight_limit) = (self.last_send, self.dxgi_first_full, self.last_cut.take(), self.inflight_limit);
        *self = Stats::new(now);
        self.inflight_limit = inflight_limit;
        self.last_send = last_send;
        self.dxgi_first_full = dxgi_first_full;
        self.last_cut = last_cut;
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_drops_frames_before_size_and_ends_at_a_third() {
        assert!(RUNGS.windows(2).all(|w| w[1].fps <= w[0].fps && w[1].scale <= w[0].scale && w[1] != w[0]));
        let first_scaled = RUNGS.iter().position(|r| r.scale < 100).unwrap();
        assert!(RUNGS[..first_scaled].iter().all(|r| r.scale == 100) && RUNGS[first_scaled - 1].fps == 8, "every frame rate down to 8 before any scaling");
        assert_eq!(RUNGS[LOWEST], rung(5, 33));
    }

    #[test]
    fn an_encoder_over_its_budget_for_two_seconds_steps_down_and_climbs_back_slowly() {
        let mut o = Overuse::default();
        // 30 fps: 33 ms a frame; 30 ms encodes are 0.9 of it.
        let mut t = 0;
        let mut changed_at = None;
        while t <= 3000 {
            if o.sample(30.0, 1000.0 / 30.0, t) {
                changed_at = Some(t);
                break;
            }
            t += 33;
        }
        assert_eq!(o.rung, 1);
        assert!(changed_at.is_some_and(|c| (2000..2100).contains(&c)), "{changed_at:?}");
        // Now cheap: 5 ms against 50 ms (20 fps). Not up before 10 s, and not with fewer than 60 frames.
        let start = t;
        let mut up_at = None;
        while t <= start + 15_000 {
            if o.sample(5.0, 50.0, t) {
                up_at = Some(t - start);
                break;
            }
            t += 50;
        }
        assert_eq!(o.rung, 0);
        assert!(up_at.is_some_and(|u| u >= 10_000), "{up_at:?}");
        // In between, nothing moves.
        let mut o = Overuse::default();
        for i in 0..1000 {
            assert!(!o.sample(20.0, 1000.0 / 30.0, i * 33), "0.6 of the budget is fine");
        }
    }

    #[test]
    fn a_test_delay_drives_the_encoder_down_the_whole_ladder() {
        // What `KELPIE_SCREEN_TEST_ENCODE_DELAY_MS` does on a runner with no GPU: a fixed 300 ms per frame.
        let mut c = Controller::new((1920, 1080), false, false, 0);
        let mut t = 0;
        while t < 60_000 {
            c.on_encoded(300.0, t);
            t += 100;
        }
        assert_eq!(c.overuse.rung, LOWEST);
        let p = c.params();
        assert_eq!((p.fps, p.h, p.reason, p.degraded), (5, 356, Reason::Cpu, true));
    }

    fn acks(b: &mut Bandwidth, rtt: u32, from: u64, to: u64) {
        let mut t = from;
        while t < to {
            b.on_ack(rtt, t);
            t += 33;
        }
    }

    #[test]
    fn a_full_window_for_twice_the_round_trip_takes_fifteen_percent_off_and_ten_seconds_of_sending_add_a_tenth() {
        // The daemon says how long the link's window has been full with each flow. A 400 ms round trip: a second of it (the most).
        let mut b = Bandwidth::new(0);
        acks(&mut b, 400, 0, 1000);
        let window = |b: &mut Bandwidth, full_ms: u64, t: u64| {
            b.on_link_window(full_ms);
            b.tick(3, t)
        };
        assert!(!window(&mut b, 0, 1000), "lag starts");
        assert!(!window(&mut b, 500, 1500));
        assert!(window(&mut b, 1000, 2000), "a second of it");
        assert_eq!(b.kbps, 1275);
        assert!(!window(&mut b, 1500, 2500), "not again within a second");
        assert!(window(&mut b, 2000, 3000));
        assert_eq!(b.kbps, 1083);
        assert_eq!((b.cut_counts.window, b.cut_counts.total()), (2, 2), "every cut counted by its rule, for the stats");
        let cuts = b.take_cuts();
        assert_eq!(cuts.iter().map(|c| (c.rule, c.from_kbps, c.to_kbps, c.link_full_ms, c.inflight)).collect::<Vec<_>>(), [(CutRule::Window, 1500, 1275, 1000, 3), (CutRule::Window, 1275, 1083, 2000, 3)], "with what the rule saw");
        // A 40 ms round trip: half a second of it is enough (twice the round trip, but never under half a second).
        let mut fast = Bandwidth::new(0);
        acks(&mut fast, 40, 0, 1000);
        assert!(!window(&mut fast, 400, 1400));
        assert!(window(&mut fast, 500, 1500), "half a second");
        assert!(!window(&mut fast, 1400, 2400) && window(&mut fast, 1500, 2500), "and a second more of the same spell for the next");
        // 300 ms: 600 ms of it.
        let mut mid = Bandwidth::new(0);
        acks(&mut mid, 300, 0, 1000);
        assert!(!window(&mut mid, 599, 1600) && window(&mut mid, 600, 1601));
        assert!(b.take_cuts().is_empty(), "each taken once");
        b.on_link_window(0);
        let mut t = 3500;
        let up = loop {
            b.on_sent(t);
            if b.tick(0, t) {
                break t;
            }
            t += 500;
            assert!(t < 20_000, "never went up");
        };
        assert_eq!((up, b.kbps), (13_000, 1191), "ten clean seconds of frames going out");
        // A still screen sends nothing: no evidence, so no increase, however long it lasts.
        let held = b.kbps;
        for t in (13_500..60_000).step_by(500) {
            assert!(!b.tick(0, t));
        }
        assert_eq!(b.kbps, held, "an application-limited stream earns nothing");
        let mut b2 = Bandwidth::new(0);
        let mut t = 0;
        while b2.kbps > MIN_KBPS {
            t += 1000;
            b2.on_link_window(t);
            b2.tick(3, t);
            if t > 10_000_000 {
                panic!("never reached the floor");
            }
        }
        assert_eq!(b2.kbps, MIN_KBPS);
        b2.on_link_window(t + 1000);
        assert!(!b2.tick(3, t + 1000), "no lower than the floor");
    }

    /// A busy computer (a game, a build) with a perfect link: the helper's loop turns slowly and hears the daemon's flows late and in bunches,
    /// so its own count of frames in flight sits at the window for seconds at a time, while the daemon sees every frame acknowledged within
    /// 2 ms and its window never full. That is this computer's slowness, not the link's: never a bandwidth cut.
    #[test]
    fn a_slow_helper_on_a_perfect_link_is_never_cut() {
        let mut b = Bandwidth::new(0);
        let (mut t, mut heard_at, mut owed) = (0u64, 0u64, Vec::<u32>::new());
        while t < 120_000 {
            // A frame goes out every 70 ms (a slow encoder); the daemon has it acknowledged 1-2 ms later.
            b.on_sent(t);
            owed.push(1 + (t % 2) as u32);
            // The flows reach the helper only every 1.5 s (its stdin unread while the loop is stuck in a slow capture or encode).
            if t.saturating_sub(heard_at) >= 1500 {
                heard_at = t;
                for rtt in owed.drain(..) {
                    b.on_ack(rtt, t);
                    b.on_link_window(0);
                }
            }
            let inflight = owed.len().min(MIN_WINDOW as usize) as u32;
            b.tick(inflight, t);
            t += 70;
        }
        let cuts = b.take_cuts();
        assert!(cuts.is_empty() && b.cut_counts == CutCounts::default(), "{cuts:?}");
        assert_eq!(b.kbps, START_KBPS, "and no increase on what a held-up stream cannot show either");
        // A figure the daemon gave once is not repeated by the helper's clock: a flow that said a second, then none, is one cut, not one a second.
        let mut c = Bandwidth::new(0);
        c.on_link_window(1200);
        assert!(c.tick(3, 1000));
        for t in (1100..10_000).step_by(100) {
            assert!(!c.tick(3, t), "no news from the daemon: no more cuts");
        }
        assert_eq!(c.cut_counts.window, 1);
    }

    #[test]
    fn a_queue_that_grows_is_cut_at_once_and_again_only_if_it_grows_after_the_cut() {
        let mut b = Bandwidth::new(0);
        acks(&mut b, 30, 0, 2000);
        assert!(!b.lagging(1));
        // 90 ms over the floor: the link's queue grows (Wi-Fi bufferbloat). Once the last half second is all above it, cut: not two
        // seconds later.
        acks(&mut b, 120, 2000, 2400);
        assert!(!b.tick(1, 2400), "the window still holds the floor");
        acks(&mut b, 120, 2400, 2600);
        assert!(b.tick(1, 2600), "half a second of queueing: cut now");
        assert_eq!(b.kbps, 1275);
        assert!(!b.tick(1, 2700), "nothing came after the cut yet");
        // It goes on growing after the cut: cut again, half a second on.
        acks(&mut b, 200, 2633, 3000);
        assert!(!b.tick(1, 3000), "half a second between cuts");
        assert!(b.tick(1, 3100), "growing still, on what came after the cut");
        assert_eq!(b.kbps, 1083);
        // It drains: no more cuts.
        acks(&mut b, 35, 3133, 3700);
        assert!(!b.tick(1, 3700), "the queue drained: no more cuts");
        assert!(!b.tick(1, 4500));
        // One late acknowledgement is not a trend.
        let mut c = Bandwidth::new(0);
        acks(&mut c, 30, 0, 2000);
        c.on_ack(300, 2010);
        assert!(!c.tick(1, 2010));
    }

    #[test]
    fn a_round_trip_that_steps_up_and_stays_is_a_new_base_not_a_queue_to_cut_for_minutes() {
        // Another route (LAN to the relay, Wi-Fi to LTE), or a phone's decoder that got slower: 30 ms, then 90 ms for good.
        let mut b = Bandwidth::new(0);
        acks(&mut b, 30, 0, 2000);
        let (mut cuts, mut t, mut next_tick) = (0, 2000u64, 2000u64);
        while t < 30_000 {
            b.on_ack(90, t);
            if t >= next_tick {
                next_tick += 100;
                if b.tick(1, t) {
                    cuts += 1;
                }
            }
            t += 33;
        }
        assert!(cuts <= 2, "{cuts} cuts for a step that held: the floor is taken again, not cut to the bottom");
        assert!(b.kbps >= START_KBPS * 70 / 100, "{} kbps", b.kbps);
        // Cuts are bounded whatever the round trips say: a link that keeps rising is cut three times in ten seconds at most.
        let mut c = Bandwidth::new(0);
        acks(&mut c, 30, 0, 2000);
        let (mut rtt, mut t, mut cuts, mut next_tick) = (100u32, 2000u64, 0, 2000u64);
        while t < 12_000 {
            c.on_ack(rtt, t);
            rtt += 2;
            if t >= next_tick {
                next_tick += 100;
                if c.tick(1, t) {
                    cuts += 1;
                }
            }
            t += 33;
        }
        assert!(cuts <= MAX_CUTS, "{cuts}");
    }

    /// What loopback looks like: every frame acknowledged within 2 ms, at 30 fps, with stills and bursts. Nothing to cut for: a cut here is a
    /// false one (the e2e's "early bandwidth cut" was its own phone not reading while the test waited on the event loop, not this).
    #[test]
    fn a_link_that_answers_every_frame_at_once_is_never_cut() {
        let mut b = Bandwidth::new(0);
        let (mut inflight, mut t) = (0u32, 0u64);
        while t < 120_000 {
            // A still second in every five (nothing sent), else a frame every 33 ms, acknowledged 1-2 ms later.
            if (t / 1000) % 5 != 4 {
                b.on_sent(t);
                inflight += 1;
                b.tick(inflight, t);
                b.on_ack(1 + (t % 2) as u32, t + 2);
                inflight -= 1;
            }
            b.tick(inflight, t + 2);
            t += 33;
        }
        assert_eq!(b.cut_counts.total(), 0, "{} kbps", b.kbps);
        assert!(b.kbps >= START_KBPS, "and it may rise: {} kbps", b.kbps);
    }

    #[test]
    fn a_new_connection_forgets_the_round_trips_of_the_one_before() {
        let mut b = Bandwidth::new(0);
        acks(&mut b, 30, 0, 2000);
        b.new_route();
        // The new route is 90 ms: its floor, not the old 30 ms, is the base.
        acks(&mut b, 90, 2000, 3000);
        assert_eq!(b.queueing_ms(), Some(0.0));
        assert!(!b.tick(1, 3000));
    }
    #[test]
    fn a_link_that_cannot_hold_a_rate_is_not_pushed_to_it_more_than_three_times_a_minute() {
        // Every up-step brings lag at once (10 % loss on cellular): count down-after-up cycles over five minutes.
        let mut b = Bandwidth::new(0);
        let mut cycles_per_min = [0u32; 5];
        let (mut t, mut lag_until) = (0u64, 0u64);
        while t < 300_000 {
            let was = b.kbps;
            b.on_sent(t);
            // The lag is the link's: the daemon sees its window full from the up-step on.
            let lagging = t < lag_until;
            b.on_link_window(if lagging { t + 1500 - lag_until } else { 0 });
            let changed = b.tick(if lagging { 3 } else { 0 }, t);
            if changed && b.kbps > was {
                lag_until = t + 1500;
            } else if changed && b.kbps < was {
                cycles_per_min[(t / 60_000) as usize] += 1;
            }
            t += 250;
        }
        assert!(cycles_per_min.iter().all(|c| *c <= 3), "{cycles_per_min:?}");
        assert!(cycles_per_min.iter().sum::<u32>() >= 3, "it still tries: {cycles_per_min:?}");
    }

    #[test]
    fn crisp_text_turns_itself_off_on_a_slow_or_lagging_link_and_back_on_once_it_is_calm() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        assert!(c.crisp_ok(0, 0), "1.5 Mbps to start: room for it");
        assert!(!c.crisp_ok(MIN_WINDOW, 0), "the window full: lag");
        // A second of lag on the link cuts the bitrate: off for ten seconds after the cut, even with the link quiet again.
        c.tick(MIN_WINDOW, 1000);
        c.bandwidth.on_link_window(1000);
        c.tick(MIN_WINDOW, 2000);
        c.bandwidth.on_link_window(0);
        assert_eq!(c.bandwidth.kbps, 1275);
        assert!(!c.crisp_ok(0, 2000 + CRISP_CALM_MS - 1));
        assert!(c.crisp_ok(0, 2000 + CRISP_CALM_MS));
        // A slow link (cellular, a weak Wi-Fi): below 1 Mbps, off whatever else.
        c.bandwidth.kbps = 900;
        assert!(!c.crisp_ok(0, 100_000));
        c.bandwidth.kbps = 2000;
        assert!(c.crisp_ok(0, 100_000));
        c.set_level(3, 100_000);
        assert!(!c.crisp_ok(0, 100_000), "a machine at L3 has no CPU for it");
    }

    #[test]
    fn the_bitrate_decides_the_rung_down_to_a_third_at_5_fps() {
        let mut b = Bandwidth::new(0);
        let mut at = |k: u32| {
            b.kbps = k;
            b.rung()
        };
        assert_eq!([at(4000), at(1000), at(600), at(400), at(350), at(250), at(180)], [0, 2, 4, 5, 6, 7, 8]);
        assert_eq!(RUNGS[at(250)], rung(6, 50), "250 kbps: half at 6 fps");
        assert_eq!(RUNGS[at(180)], rung(5, 33), "under 220 kbps: a third at 5 fps");
        // A 1080p monitor at 180 kbps is about 360p; a window keeps 540 lines; a 4K monitor's half is 540 lines, not 1080.
        let mut c = Controller::new((1920, 1080), false, false, 0);
        c.bandwidth.kbps = 180;
        assert_eq!((c.params().fps, c.params().h), (5, 356));
        let mut w = Controller::new((1920, 1080), true, false, 0);
        w.bandwidth.kbps = 180;
        assert_eq!(w.params().h, MIN_WINDOW_LINES);
        let mut k = Controller::new((3840, 2160), false, false, 0);
        k.bandwidth.kbps = 250;
        assert_eq!((k.params().w, k.params().h, k.params().degraded), (960, 540, true));
    }

    #[test]
    fn a_capped_stream_starts_at_its_cap_and_never_climbs_past_it() {
        let mut b = Bandwidth::new(0);
        b.set_cap(500);
        assert_eq!(b.kbps, 500);
        let mut t = 0;
        while t < 120_000 {
            b.on_sent(t);
            b.tick(0, t);
            t += 500;
        }
        assert_eq!(b.kbps, 500, "ten clean seconds add nothing past the cap");
        assert_eq!(b.rung(), 4, "500 kbps: 8 fps");
        b.set_cap(10);
        assert_eq!(b.kbps, MIN_KBPS, "never under the floor");
    }

    #[test]
    fn a_keyframe_on_a_slow_link_always_takes_the_dip() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        assert!(!c.key_dip(true), "at the top rung only a big frame does");
        c.bandwidth.kbps = 300;
        assert!(!c.key_dip(true), "two-thirds at 6 fps: not yet");
        c.bandwidth.kbps = 250;
        assert!(c.key_dip(true) && !c.key_dip(false), "from half at 6 fps, keyframes only");
        c.bandwidth.kbps = 4000;
        c.overuse.rung = LOWEST;
        assert!(c.key_dip(true), "whatever moved the ladder down");
    }

    #[test]
    fn the_window_of_frames_in_flight_follows_the_round_trip_and_the_frame_rate() {
        let at = |rtt: u32, fps_rung: usize, win: bool| {
            let mut c = Controller::new((1920, 1080), false, false, 0);
            c.set_win(win);
            c.overuse.rung = fps_rung;
            acks(&mut c.bandwidth, rtt, 0, 1000);
            c.window()
        };
        assert_eq!(at(100, 0, true).0, 6, "30 fps at 100 ms");
        assert_eq!(at(200, 0, true).0, 9, "30 fps at 200 ms");
        assert_eq!(at(1000, 0, true).0, MAX_WINDOW, "never more than 12");
        assert_eq!(at(10, 0, true).0, MIN_WINDOW, "never fewer than 3");
        assert_eq!(at(200, 2, true).0, 5, "15 fps at 200 ms");
        assert_eq!(at(200, 0, false), (MIN_WINDOW, MIN_WINDOW_BYTES), "a daemon that does not take another: 3 frames and 192 KiB");
        let c = {
            let mut c = Controller::new((1920, 1080), false, false, 0);
            c.set_win(true);
            c
        };
        assert_eq!(c.window(), (MIN_WINDOW, MIN_WINDOW_BYTES), "no round trip yet");
        // The bytes: the rate over the round trip and 100 ms, between 192 KiB and 2 MiB.
        let mut big = Controller::new((1920, 1080), false, false, 0);
        big.set_win(true);
        big.bandwidth.kbps = MAX_KBPS;
        acks(&mut big.bandwidth, 900, 0, 1000);
        assert_eq!(big.window().1, (MAX_KBPS as usize) * 1000 / 8);
        // The gate holds to it, and so does the lag test.
        let mut g = Controller::new((1920, 1080), false, false, 0);
        g.set_win(true);
        acks(&mut g.bandwidth, 200, 0, 1000);
        assert!(g.may_encode(8, 0, None) && !g.may_encode(9, 0, None), "nine frames at 200 ms");
        assert!(!g.may_encode(1, MIN_WINDOW_BYTES, None), "nor more bytes than its own");
        g.tick(8, 1000);
        assert!(!g.bandwidth.lagging(8) && g.bandwidth.lagging(9));
    }

    #[test]
    fn a_queue_is_seen_within_a_few_frames_at_any_rate_and_one_late_frame_is_not_one() {
        let first_cut = |fps: u8, gap: u64| {
            let mut b = Bandwidth::new(0);
            b.fps = fps;
            let mut t = 0;
            while t < 3000 {
                b.on_ack(30, t);
                t += gap;
            }
            let step = t;
            while t < step + 3000 {
                b.on_ack(90, t);
                if b.tick(1, t) {
                    return Some(t - step);
                }
                t += gap;
            }
            None
        };
        assert!(first_cut(30, 33).is_some_and(|ms| ms <= 600), "{:?}", first_cut(30, 33));
        assert!(first_cut(6, 167).is_some_and(|ms| ms <= 900), "{:?}", first_cut(6, 167));
        // One late acknowledgement among frames on time is not a trend, at 6 fps either.
        for (fps, gap) in [(30u8, 33u64), (6, 167)] {
            let mut b = Bandwidth::new(0);
            b.fps = fps;
            let mut t = 0;
            while t < 3000 {
                b.on_ack(if t == 2000 / gap * gap { 300 } else { 30 }, t);
                assert!(!b.tick(1, t), "{fps} fps at {t}");
                t += gap;
            }
        }
    }

    #[test]
    fn load_caps_and_the_software_encoder_only_ever_lower_and_say_why() {
        let mut c = Controller::new((3840, 2160), false, false, 0);
        let p = c.params();
        assert_eq!((p.fps, p.w, p.h, p.reason, p.degraded), (30, 1920, 1080, Reason::None, false), "a 4K monitor at the top rung is 1080p, and not reduced");
        c.set_level(1, 0);
        let p = c.params();
        assert_eq!((p.fps, p.h, p.reason), (20, 1080, Reason::Load));
        c.set_level(2, 0);
        assert_eq!((c.params().fps, c.params().h), (15, 720));
        c.set_level(3, 0);
        assert_eq!((c.params().fps, c.params().h, c.params().degraded), (8, 540, true));
        c.set_level(0, 0);
        c.set_software(true);
        let p = c.params();
        assert_eq!((p.fps, p.h, p.reason), (15, 720, Reason::Encoder));
        assert!(p.kbps <= SOFTWARE_MAX_KBPS);
        // A cap above what the ladder already asks changes nothing, and is not the reason.
        let mut c = Controller::new((1280, 720), false, false, 0);
        c.overuse.rung = 4;
        c.set_level(1, 0);
        let p = c.params();
        assert_eq!((p.fps, p.h, p.reason), (8, 720, Reason::Cpu));
    }

    #[test]
    fn a_5k_or_ultrawide_monitor_is_shown_within_the_top_rung() {
        assert_eq!(fit((5120, 2880), 100, 0, u32::MAX), (1920, 1080), "5K: level 4 instead of no encoder at all");
        assert_eq!(fit((5120, 1440), 100, 0, u32::MAX), (1920, 540), "ultrawide: no wider than 1920");
        assert_eq!(fit((2560, 1440), 50, 0, u32::MAX), (960, 540), "half of the top rung's 1080 lines");
        assert_eq!(fit((3840, 2160), 50, 0, u32::MAX), (960, 540), "4K at half: 540 lines");
        assert_eq!(fit((3840, 2160), 50, 0, 720), (960, 540), "under a cap it does not reach");
        assert_eq!(fit((1366, 768), 100, 0, u32::MAX), (1366, 768), "smaller ones as they are");
        let c = Controller::new((5120, 2880), false, false, 0);
        assert!(!c.params().degraded);
    }

    #[test]
    fn padding_on_the_wire_lowers_what_the_encoder_is_asked_for() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        assert_eq!(c.params().kbps, START_KBPS);
        for _ in 0..200 {
            c.bandwidth.on_wire(5_000, 10_000);
        }
        assert!((c.bandwidth.wire_factor() - 2.0).abs() < 0.01);
        assert!((c.params().kbps as i64 - i64::from(START_KBPS / 2)).abs() <= 2, "{}", c.params().kbps);
        c.bandwidth.on_wire(1, 100);
        assert!(c.bandwidth.wire_factor() <= 4.0, "a ratio is held to 4");
    }

    #[test]
    fn frames_lost_on_the_way_never_hold_the_stream() {
        // A keyframe at 0 ms, then three deltas the daemon drops (its lane was full): nothing is acknowledged, so nothing more may be encoded.
        let mut f = Flight::default();
        let c = Controller::new((1920, 1080), false, false, 0);
        assert!(f.wants_key());
        f.sent(50_000);
        f.encoded(true, 0);
        f.acked(1);
        assert_eq!(f.inflight_bytes(), 0);
        for _ in 0..3 {
            f.sent(1000);
            f.encoded(false, 0);
        }
        assert_eq!((f.inflight(), f.inflight_bytes()), (3, 3000));
        assert!(!c.may_encode(f.inflight(), f.inflight_bytes(), Some(1000)), "the window is full");
        // The daemon says what it dropped and asks for a keyframe within the keyframe's second: the frames are given up, and nothing but a
        // keyframe is encoded, once its second is over.
        f.dropped_through(4);
        f.ask_key(500);
        assert_eq!((f.inflight(), f.inflight_bytes()), (0, 0));
        assert!(f.gated() && !f.wants_key(), "a delta now would only be dropped");
        f.tick(999);
        assert!(f.gated());
        f.tick(1000);
        assert!(f.wants_key() && !f.gated() && c.may_encode(f.inflight(), f.inflight_bytes(), Some(1000)), "the next frame is a keyframe, and it may go");
        f.sent(40_000);
        f.encoded(true, 1000);
        assert!(!f.wants_key());
        // A phone whose decoder failed asks later: the keyframe goes at once, whatever is in flight.
        for _ in 0..3 {
            f.sent(1000);
        }
        f.ask_key(5000);
        assert!(f.wants_key() && f.inflight() == 0 && f.inflight_bytes() == 0);
        // An acknowledgement is never for a frame that was not sent.
        let mut g = Flight::default();
        g.sent(10);
        g.acked(99);
        assert_eq!((g.inflight(), g.inflight_bytes()), (0, 0));
        g.sent(10);
        g.restart();
        assert!(g.wants_key() && g.inflight() == 0 && g.inflight_bytes() == 0);
    }

    #[test]
    fn a_window_keeps_540_lines_at_the_lowest_rung() {
        let mut c = Controller::new((1280, 960), true, false, 0);
        c.overuse.rung = LOWEST;
        assert_eq!((c.params().w, c.params().h), (720, 540));
        let mut m = Controller::new((1280, 960), false, false, 0);
        m.overuse.rung = LOWEST;
        assert_eq!((m.params().w, m.params().h), (420, 316), "a monitor goes to a third");
    }

    #[test]
    fn five_minutes_of_l3_pause_the_stream_until_the_person_resumes_it() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        c.set_level(3, 1000);
        assert_eq!(c.tick(0, 1000 + L3_PAUSE_MS - 1), None);
        assert_eq!(c.tick(0, 1000 + L3_PAUSE_MS), Some(Event::PausedForLoad));
        assert!(c.paused_for_load() && !c.may_encode(0, 0, None));
        assert_eq!(c.tick(0, 2000 + L3_PAUSE_MS), None, "said once");
        c.resume(2000 + L3_PAUSE_MS);
        assert!(c.may_encode(0, 0, None));
        assert_eq!(c.tick(0, 3000 + L3_PAUSE_MS), None, "five more minutes before the next");
        c.set_level(2, 4000 + L3_PAUSE_MS);
        assert_eq!(c.tick(0, 4000 + 3 * L3_PAUSE_MS), None, "not at L2");
    }

    #[test]
    fn the_hardware_encoder_gives_way_only_when_slow_at_the_lowest_rung_for_five_seconds() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        let mut t = 0;
        // 50 ms a frame is shed by the ladder (it settles at 10 fps, where 50 ms is half the budget): no software for that.
        for i in 0..1200 {
            assert_eq!(c.on_encoded(50.0, i * 100), None);
        }
        assert!(c.rung() < LOWEST);
        // 300 ms a frame is too slow even at 5 fps: once at the lowest rung for five seconds, software.
        let mut c = Controller::new((1920, 1080), false, false, 0);
        let mut asked = None;
        while t < 120_000 && asked.is_none() {
            asked = c.on_encoded(300.0, t).map(|e| (e, t, c.rung()));
            t += 100;
        }
        let (event, at, rung_then) = asked.expect("asked for software");
        assert_eq!((event, rung_then), (Event::UseSoftware, LOWEST));
        assert!(at >= 5000);
        assert_eq!(c.on_encoded(300.0, t), None, "asked once");
        let mut fast = Controller::new((1920, 1080), false, false, 0);
        fast.overuse.rung = LOWEST;
        for i in 0..200 {
            assert_eq!(fast.on_encoded(20.0, i * 100), None);
        }
    }

    #[test]
    fn frames_wait_for_the_phone_and_for_the_rung_s_interval() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        assert!(c.may_encode(0, 0, None));
        assert!(c.may_encode(2, 0, Some(40)));
        assert!(!c.may_encode(MIN_WINDOW, 0, Some(1000)), "the window is full");
        assert!(!c.may_encode(0, 0, Some(20)), "30 fps: not before 33 ms");
        assert!(c.may_encode(0, 0, Some(33)));
        c.overuse.rung = 2;
        assert!(!c.may_encode(0, 0, Some(60)) && c.may_encode(0, 0, Some(67)), "15 fps");
    }

    #[test]
    fn keyframes_come_at_most_once_a_second_and_requests_are_not_lost() {
        let mut g = KeyframeGate::default();
        assert!(g.request(0));
        assert!(!g.request(200) && !g.request(400), "within the second: wait");
        assert!(!g.due(900));
        assert!(g.due(1000), "the waiting ones become one");
        assert!(!g.due(1500));
        g.made(3000);
        assert!(!g.request(3500));
        assert!(g.due(4000));
        assert!(g.request(6000));
    }

    #[test]
    fn a_second_s_report_has_the_rate_the_size_and_why() {
        let mut s = Stats::new(0);
        for i in 0..30 {
            s.frame(if i == 0 { 60_000 } else { 6_000 }, i == 0, 4.0 + f64::from(i % 5));
        }
        s.dropped_encoder = 2;
        for i in 0..30u64 {
            s.timing(i * 33, 40.0 + (i % 10) as f64, 2.0);
            s.ack(60 + (i % 3) as u32 * 10);
        }
        s.top_off = 4;
        s.dxgi_first_full = Some(1);
        s.link_cuts = CutCounts { queueing: 0, window: 1, size: 0 };
        s.inflight_limit = 6;
        s.cut(Cut { rule: CutRule::Window, from_kbps: 1500, to_kbps: 1275, rtt_recent_ms: Some(32), rtt_base_ms: Some(30), link_full_ms: 1020, inflight: 3 });
        let p = Params { fps: 30, w: 1920, h: 1080, kbps: 1500, reason: Reason::None, degraded: false };
        assert!(s.report(999, &p, 0, false, Some(true)).is_none());
        let r = s.report(1000, &p, 1, false, Some(false)).unwrap();
        assert_eq!((r.fps, r.kbps, r.dropped, r.encoder, r.level), (30, (60_000 + 29 * 6_000) * 8 / 1000, 2, "hw", 1));
        assert_eq!(r.idr_ratio, Some(10.0));
        assert_eq!(r.encode_ms_p95, 8.0);
        assert_eq!(serde_json::to_value(&r).unwrap()["reason"], "none");
        assert_eq!(serde_json::to_value(&r).unwrap()["crisp"], false, "crisp text off for now");
        let st = &r.stages;
        assert_eq!((st.capture_to_send_ms_p50, st.capture_to_send_ms_p95, st.convert_ms_p95, st.send_interval_ms_p95), (Some(45.0), Some(49.0), Some(2.0), Some(33.0)));
        assert_eq!((st.ack_rtt_ms_p50, st.ack_rtt_ms_p95, st.top_off, st.held), (Some(70.0), Some(80.0), 4, 0));
        // Which rule cut, and what it saw: by rule in the stages, each cut once in the report for the daemon's log.
        assert_eq!((st.link_cuts, st.link_cuts_window, st.link_cuts_queueing, st.link_cuts_size), (1, 1, 0, 0));
        assert_eq!((st.last_cut_queueing_ms, st.last_cut_link_full_ms, st.last_cut_inflight, st.inflight_limit), (Some(2), Some(1020), Some(3), 6));
        assert_eq!(serde_json::to_value(&r).unwrap()["cuts"][0]["rule"], "window");
        assert_eq!(s.report(1500, &p, 0, true, None), None, "counters started again");
        s.timing(2000, 10.0, 1.0);
        let later = s.report(2000, &p, 0, false, None).unwrap();
        assert!(later.cuts.is_empty() && serde_json::to_value(&later).unwrap().get("cuts").is_none(), "each cut is reported once");
        assert_eq!(later.stages.last_cut_link_full_ms, Some(1020), "the last cut's figures stay");
        let later = later.stages;
        assert_eq!(later.send_interval_ms_p95, Some(1043.0), "the interval carries on from the last frame before the report");
        assert_eq!(later.dxgi_first_full, Some(1), "said once, kept in every report after");
    }
}
