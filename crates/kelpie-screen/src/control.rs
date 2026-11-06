//! How much picture to send: frames a second, size and bitrate, from three things that each may ask for less and never for more than
//! another allows: the encoder's own load (`Overuse`, libwebrtc's model), the phone's link (`Bandwidth`, from acknowledgements), and the
//! machine's load level as the daemon reports it (caps). Text is what people look at, so frames a second go first and the size last
//! (`RUNGS`); a window is never made shorter than 540 lines.
//!
//! The link is TCP under the Noise session: nothing is lost, only delayed. So frames are dropped before they are encoded, never after (a
//! dropped P-frame would break every frame after it): the stream encodes only while fewer than `MAX_INFLIGHT` frames wait for the phone's
//! acknowledgement, and the newest capture waits in a one-slot mailbox meanwhile. Keyframes are for a decoder that starts or lost its
//! place, at most one a second (`KeyframeGate`).
//!
//! Every clock here is milliseconds passed in by the caller, so that all of it is tested without waiting.

use std::collections::VecDeque;

use serde::Serialize;

use crate::geom;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub fps: u8,
    /// Percent of the source's height.
    pub scale: u8,
}

const fn rung(fps: u8, scale: u8) -> Rung {
    Rung { fps, scale }
}

/// Frames a second first, then size.
pub const RUNGS: [Rung; 9] = [rung(30, 100), rung(20, 100), rung(15, 100), rung(10, 100), rung(8, 100), rung(8, 75), rung(6, 67), rung(4, 67), rung(4, 50)];
pub const LOWEST: usize = RUNGS.len() - 1;
/// A window is never shown shorter than this (unless it is shorter itself).
pub const MIN_WINDOW_LINES: u32 = 540;
/// Frames sent and not yet acknowledged, at most: the encoder waits for the phone beyond it.
pub const MAX_INFLIGHT: u32 = 3;

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
/// Queueing delay (round trip over the smallest seen lately) that counts as lag.
const QUEUE_LAG_MS: f64 = 150.0;
const LAG_FOR_MS: u64 = 2000;
const MIN_RTT_WINDOW_MS: u64 = 10_000;
const UP_WAIT_MS: u64 = 10_000;
const UP_WAIT_MAX_MS: u64 = 60_000;
/// An up-step that held this long without a down-step resets the wait.
const UP_HELD_MS: u64 = 30_000;
/// Crisp tiles turn themselves off below this bitrate, and for this long after the bitrate was cut (owner decision 2026-11-06).
pub const CRISP_MIN_KBPS: u32 = 1000;
pub const CRISP_CALM_MS: u64 = 10_000;

/// The bitrate the link takes, from what the phone acknowledges: lag (the window of frames full, or the round trip grown by queueing) for
/// 2 s takes 25 % off at once; 10 s with none adds 10 %. An up-step undone within 10 s doubles the wait before the next (to a minute), so a
/// link that cannot hold a rate is not pushed to it every few seconds.
#[derive(Debug)]
pub struct Bandwidth {
    pub kbps: u32,
    rtts: VecDeque<(u64, u32)>,
    srtt: Option<f64>,
    lag_since: Option<u64>,
    clean_since: u64,
    last_down: Option<u64>,
    last_up: Option<u64>,
    up_wait: u64,
}

impl Bandwidth {
    pub fn new(now: u64) -> Bandwidth {
        Bandwidth { kbps: START_KBPS, rtts: VecDeque::new(), srtt: None, lag_since: None, clean_since: now, last_down: None, last_up: None, up_wait: UP_WAIT_MS }
    }

    /// A frame's round trip, from the daemon sending it to the phone's acknowledgement.
    pub fn on_ack(&mut self, rtt_ms: u32, now: u64) {
        self.rtts.push_back((now, rtt_ms));
        while self.rtts.front().is_some_and(|(t, _)| now.saturating_sub(*t) > MIN_RTT_WINDOW_MS) {
            self.rtts.pop_front();
        }
        let r = f64::from(rtt_ms);
        self.srtt = Some(self.srtt.map_or(r, |s| s + 0.125 * (r - s)));
    }

    pub fn queueing_ms(&self) -> Option<f64> {
        let min = self.rtts.iter().map(|(_, r)| *r).min()?;
        Some((self.srtt? - f64::from(min)).max(0.0))
    }

    pub fn lagging(&self, inflight: u32) -> bool {
        inflight >= MAX_INFLIGHT || self.queueing_ms().is_some_and(|q| q > QUEUE_LAG_MS)
    }

    /// Looked at once in a while with the frames in flight now. True when the bitrate changed.
    pub fn tick(&mut self, inflight: u32, now: u64) -> bool {
        if self.lagging(inflight) {
            let since = *self.lag_since.get_or_insert(now);
            self.clean_since = now;
            if now.saturating_sub(since) >= LAG_FOR_MS && self.last_down.is_none_or(|d| now.saturating_sub(d) >= LAG_FOR_MS) {
                if self.last_up.is_some_and(|u| now.saturating_sub(u) < UP_WAIT_MS) {
                    self.up_wait = (self.up_wait * 2).min(UP_WAIT_MAX_MS);
                }
                let next = (self.kbps * 3 / 4).max(MIN_KBPS);
                self.lag_since = Some(now);
                self.last_down = Some(now);
                if next != self.kbps {
                    self.kbps = next;
                    return true;
                }
            }
            return false;
        }
        self.lag_since = None;
        if let Some(up) = self.last_up {
            if now.saturating_sub(up) >= UP_HELD_MS && self.last_down.is_none_or(|d| d < up) {
                self.up_wait = UP_WAIT_MS;
            }
        }
        if now.saturating_sub(self.clean_since) >= self.up_wait && self.kbps < MAX_KBPS {
            self.kbps = (self.kbps * 11 / 10).max(self.kbps + 50).min(MAX_KBPS);
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

    /// The rung the bitrate affords: 1080p30 → 1080p15 → 1080p8 → 75 % at 8 → 67 % at 4.
    pub fn rung(&self) -> usize {
        match self.kbps {
            k if k >= 1200 => 0,
            k if k >= 800 => 2,
            k if k >= 500 => 4,
            k if k >= 300 => 5,
            _ => 7,
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
    pub keyframes: KeyframeGate,
    src: (u32, u32),
    window: bool,
    level: u8,
    software: bool,
    l3_since: Option<u64>,
    paused_for_load: bool,
    hw_times: VecDeque<(u64, f64)>,
    hw_slow_since: Option<u64>,
    asked_software: bool,
}

impl Controller {
    pub fn new(src: (u32, u32), window: bool, software: bool, now: u64) -> Controller {
        Controller { overuse: Overuse::default(), bandwidth: Bandwidth::new(now), keyframes: KeyframeGate::default(), src, window, level: 0, software, l3_since: None, paused_for_load: false, hw_times: VecDeque::new(), hw_slow_since: None, asked_software: false }
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
        let (free_w, free_h) = geom::scaled(self.src.0, self.src.1, r.scale, min_lines, u32::MAX);
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
        let (w, h) = if max_lines == u32::MAX { (free_w, free_h) } else { geom::scaled(self.src.0, self.src.1, r.scale, min_lines, max_lines) };
        let kbps = if self.software { self.bandwidth.kbps.min(SOFTWARE_MAX_KBPS) } else { self.bandwidth.kbps };
        let degraded = fps < 8 || h < (self.src.1 & !1);
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

    /// Whether a frame may be encoded now: the phone has room for it, and the rung's interval has passed since the last one.
    pub fn may_encode(&self, inflight: u32, since_last_ms: Option<u64>) -> bool {
        !self.paused_for_load && inflight < MAX_INFLIGHT && since_last_ms.is_none_or(|ms| ms as f64 + 0.5 >= self.interval_ms())
    }
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

    /// The second's report, once a second has passed; the counters start again.
    pub fn report(&mut self, now: u64, p: &Params, level: u8, software: bool, crisp: Option<bool>) -> Option<Report> {
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
        };
        *self = Stats::new(now);
        Some(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_drops_frames_before_size_and_ends_at_half() {
        assert!(RUNGS.windows(2).all(|w| w[1].fps <= w[0].fps && w[1].scale <= w[0].scale && w[1] != w[0]));
        let first_scaled = RUNGS.iter().position(|r| r.scale < 100).unwrap();
        assert!(RUNGS[..first_scaled].iter().all(|r| r.scale == 100) && RUNGS[first_scaled - 1].fps == 8, "every frame rate down to 8 before any scaling");
        assert_eq!(RUNGS[LOWEST], rung(4, 50));
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
        assert_eq!((p.fps, p.h, p.reason, p.degraded), (4, 540, Reason::Cpu, true));
    }

    fn acks(b: &mut Bandwidth, rtt: u32, from: u64, to: u64) {
        let mut t = from;
        while t < to {
            b.on_ack(rtt, t);
            t += 33;
        }
    }

    #[test]
    fn a_full_window_for_two_seconds_takes_a_quarter_off_and_ten_clean_seconds_add_a_tenth() {
        let mut b = Bandwidth::new(0);
        acks(&mut b, 40, 0, 1000);
        assert!(!b.tick(3, 1000), "lag starts");
        assert!(!b.tick(3, 2500));
        assert!(b.tick(3, 3000), "two seconds of it");
        assert_eq!(b.kbps, 1125);
        assert!(!b.tick(3, 3500), "not again within two seconds");
        assert!(b.tick(3, 5000));
        assert_eq!(b.kbps, 843);
        assert!(!b.tick(0, 6000), "clean from now");
        assert!(!b.tick(0, 14_000));
        assert!(b.tick(0, 15_000), "ten clean seconds");
        assert_eq!(b.kbps, 927);
        for _ in 0..100 {
            b.tick(3, 100_000);
            b.tick(3, 103_000);
        }
        let mut b2 = Bandwidth::new(0);
        let mut t = 0;
        while b2.kbps > MIN_KBPS {
            t += 2000;
            b2.tick(3, t);
            if t > 10_000_000 {
                panic!("never reached the floor");
            }
        }
        assert_eq!(b2.kbps, MIN_KBPS);
        assert!(!b2.tick(3, t + 2000), "no lower than the floor");
    }

    #[test]
    fn queueing_delay_counts_as_lag_even_with_room_in_the_window() {
        let mut b = Bandwidth::new(0);
        acks(&mut b, 30, 0, 2000);
        assert!(!b.lagging(1));
        acks(&mut b, 400, 2000, 5000);
        assert!(b.queueing_ms().unwrap() > QUEUE_LAG_MS && b.lagging(1));
        // The smallest round trip ages out of its window: a link that is slow for good is no longer "queueing".
        acks(&mut b, 400, 5000, 20_000);
        assert!(!b.lagging(1), "{:?}", b.queueing_ms());
    }

    #[test]
    fn a_link_that_cannot_hold_a_rate_is_not_pushed_to_it_more_than_three_times_a_minute() {
        // Every up-step brings lag at once (10 % loss on cellular): count down-after-up cycles over five minutes.
        let mut b = Bandwidth::new(0);
        let mut cycles_per_min = [0u32; 5];
        let (mut t, mut lag_until) = (0u64, 0u64);
        while t < 300_000 {
            let was = b.kbps;
            let changed = b.tick(if t < lag_until { 3 } else { 0 }, t);
            if changed && b.kbps > was {
                lag_until = t + 2500;
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
        assert!(!c.crisp_ok(MAX_INFLIGHT, 0), "the window full: lag");
        // Two seconds of lag cut the bitrate: off for ten seconds after the cut, even with the link quiet again.
        c.tick(MAX_INFLIGHT, 1000);
        c.tick(MAX_INFLIGHT, 3000);
        assert_eq!(c.bandwidth.kbps, 1125);
        assert!(!c.crisp_ok(0, 3000 + CRISP_CALM_MS - 1));
        assert!(c.crisp_ok(0, 3000 + CRISP_CALM_MS));
        // A slow link (cellular, a weak Wi-Fi): below 1 Mbps, off whatever else.
        c.bandwidth.kbps = 900;
        assert!(!c.crisp_ok(0, 100_000));
        c.bandwidth.kbps = 2000;
        assert!(c.crisp_ok(0, 100_000));
        c.set_level(3, 100_000);
        assert!(!c.crisp_ok(0, 100_000), "a machine at L3 has no CPU for it");
    }

    #[test]
    fn the_bitrate_decides_the_rung_down_to_720p_at_4_fps() {
        let mut b = Bandwidth::new(0);
        let at = |b: &mut Bandwidth, k: u32| {
            b.kbps = k;
            RUNGS[b.rung()]
        };
        assert_eq!(at(&mut b, 4000), rung(30, 100));
        assert_eq!(at(&mut b, 1000), rung(15, 100));
        assert_eq!(at(&mut b, 600), rung(8, 100));
        assert_eq!(at(&mut b, 350), rung(8, 75));
        assert_eq!(at(&mut b, 250), rung(4, 67), "under 300 kbps: 720p, few frames");
    }

    #[test]
    fn load_caps_and_the_software_encoder_only_ever_lower_and_say_why() {
        let mut c = Controller::new((3840, 2160), false, false, 0);
        let p = c.params();
        assert_eq!((p.fps, p.w, p.h, p.reason, p.degraded), (30, 3840, 2160, Reason::None, false));
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
    fn a_window_keeps_540_lines_at_the_lowest_rung() {
        let mut c = Controller::new((1280, 960), true, false, 0);
        c.overuse.rung = LOWEST;
        assert_eq!((c.params().w, c.params().h), (720, 540));
        let mut m = Controller::new((1280, 960), false, false, 0);
        m.overuse.rung = LOWEST;
        assert_eq!((m.params().w, m.params().h), (640, 480), "a monitor goes to half");
    }

    #[test]
    fn five_minutes_of_l3_pause_the_stream_until_the_person_resumes_it() {
        let mut c = Controller::new((1920, 1080), false, false, 0);
        c.set_level(3, 1000);
        assert_eq!(c.tick(0, 1000 + L3_PAUSE_MS - 1), None);
        assert_eq!(c.tick(0, 1000 + L3_PAUSE_MS), Some(Event::PausedForLoad));
        assert!(c.paused_for_load() && !c.may_encode(0, None));
        assert_eq!(c.tick(0, 2000 + L3_PAUSE_MS), None, "said once");
        c.resume(2000 + L3_PAUSE_MS);
        assert!(c.may_encode(0, None));
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
        // 300 ms a frame is too slow even at 4 fps: once at the lowest rung for five seconds, software.
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
        assert!(c.may_encode(0, None));
        assert!(c.may_encode(2, Some(40)));
        assert!(!c.may_encode(MAX_INFLIGHT, Some(1000)), "the window is full");
        assert!(!c.may_encode(0, Some(20)), "30 fps: not before 33 ms");
        assert!(c.may_encode(0, Some(33)));
        c.overuse.rung = 2;
        assert!(!c.may_encode(0, Some(60)) && c.may_encode(0, Some(67)), "15 fps");
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
        let p = Params { fps: 30, w: 1920, h: 1080, kbps: 1500, reason: Reason::None, degraded: false };
        assert!(s.report(999, &p, 0, false, Some(true)).is_none());
        let r = s.report(1000, &p, 1, false, Some(false)).unwrap();
        assert_eq!((r.fps, r.kbps, r.dropped, r.encoder, r.level), (30, (60_000 + 29 * 6_000) * 8 / 1000, 2, "hw", 1));
        assert_eq!(r.idr_ratio, Some(10.0));
        assert_eq!(r.encode_ms_p95, 8.0);
        assert_eq!(serde_json::to_value(&r).unwrap()["reason"], "none");
        assert_eq!(serde_json::to_value(&r).unwrap()["crisp"], false, "crisp text off for now");
        assert_eq!(s.report(1500, &p, 0, true, None), None, "counters started again");
    }
}
