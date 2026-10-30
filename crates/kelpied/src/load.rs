//! How busy this computer is, as one level that everything a phone costs reads: L0 calm, L1 busy,
//! L2 loaded, L3 overloaded. Agents running builds can peg every core; a daemon nobody sees the
//! window of is throttled by the OS. The daemon measures itself: how late its own timer fires (the
//! event loop's lag, the one signal that says its work is waiting for a core), the whole machine's
//! CPU and free memory, and the power saver and thermal state as inputs that only raise the level.
//! A level changes only after it has held (hysteresis), so the phone's view does not flap.
//!
//! Started only while phone access is on (`Load::start`); nothing runs otherwise. Phone sync reads
//! the level and sheds rate and size by it (`Level`'s methods are that ladder); input, answers,
//! revoke, stop and heartbeats are never shed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::watch;

#[cfg(target_os = "macos")]
#[path = "load/macos.rs"]
mod os;
#[cfg(windows)]
#[path = "load/windows.rs"]
mod os;
#[cfg(not(any(windows, target_os = "macos")))]
mod os {
    #[derive(Default)]
    pub struct Probe;

    impl Probe {
        pub fn read(&mut self) -> super::Reading {
            super::Reading::default()
        }
    }

    pub struct Activity;

    impl Activity {
        pub fn begin() -> Activity {
            Activity
        }
    }

    pub fn opt_out_of_throttling() {}

    pub fn thread_report() -> String {
        String::new()
    }
}

/// How often the event loop's lag is measured.
const PROBE: Duration = Duration::from_millis(100);
/// Lags kept: five seconds' worth, whose 95th percentile is the lag.
const LAGS_KEPT: usize = 50;
/// Upper edges of the lag histogram's buckets (ms); a last bucket takes the rest.
const LAG_EDGES_MS: [f64; 12] = [1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 50.0, 75.0, 100.0, 200.0, 500.0, 1000.0];
/// A timer this late means the event loop was stalled (not merely busy).
const STALL: Duration = Duration::from_secs(1);
/// Samples in a row above a level before it is entered; time below before it is left.
const ENTER_AFTER: u32 = 2;
const LEAVE_AFTER_MS: u64 = 10_000;
/// Falls of level allowed in a minute (rises are never held back).
const MOVES_PER_MINUTE: usize = 2;
/// The runtime has not run the probe for this long: log it; and tell the app.
const STALLED_AFTER: Duration = Duration::from_secs(5);
const UNRESPONSIVE_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    #[default]
    L0,
    L1,
    L2,
    L3,
}

impl Level {
    pub fn number(self) -> u8 {
        self as u8
    }

    /// `L2` (or `2`) as the test hook writes it.
    fn parse(text: &str) -> Option<Level> {
        match text.trim().trim_start_matches(['L', 'l']) {
            "0" => Some(Level::L0),
            "1" => Some(Level::L1),
            "2" => Some(Level::L2),
            "3" => Some(Level::L3),
            _ => None,
        }
    }

    fn pick<T: Copy>(self, by_level: [T; 4]) -> T {
        by_level[self as usize]
    }

    /// Least time between two frames of the session a phone is looking at.
    pub fn focused_gap(self) -> Duration {
        Duration::from_millis(self.pick([100, 100, 250, 1000]))
    }

    /// The same for the other sessions it follows; none at all at L3 (paused).
    pub fn background_gap(self) -> Option<Duration> {
        self.pick([Some(100), Some(250), Some(1000), None]).map(Duration::from_millis)
    }

    /// Least time between frames of a session that redraws most of its screen each time: five a
    /// second, one at L3.
    pub fn heavy_gap(self) -> Duration {
        Duration::from_millis(self.pick([200, 200, 250, 1000]))
    }

    /// Bytes of rows one frame may carry (a frame is that plus a little: 64, 64, 32 and 16 KiB at most).
    pub fn frame_rows_bytes(self) -> usize {
        self.pick([56, 56, 28, 12]) * 1024
    }

    /// Chat logs that may be on their way to phones at once (starting a chat over costs up to 2 MiB).
    pub fn chat_resets(self) -> usize {
        self.pick([usize::MAX, usize::MAX, 2, 1])
    }

    /// Least time between two pairing handshakes.
    pub fn pairing_gap(self) -> Duration {
        Duration::from_millis(self.pick([0, 0, 1000, 5000]))
    }

    /// Heartbeats and their deadlines are twice as patient when the machine is loaded: replies are late
    /// because everything is.
    pub fn patience(self) -> u32 {
        if self >= Level::L2 {
            2
        } else {
            1
        }
    }
}

/// The level, why, and since when (ms since the epoch).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub level: Level,
    pub reasons: Vec<String>,
    pub since: u64,
}

impl Status {
    /// L3 because memory ran out. Freeing what is kept for away phones helps only then: when L3 comes from
    /// CPU or the event loop's lag, those positions hold almost nothing and dropping them would make every
    /// reconnecting phone fetch the whole screen at the worst moment.
    pub fn short_of_memory(&self) -> bool {
        self.level == Level::L3 && self.reasons.iter().any(|r| r == "memory")
    }

    pub fn json(&self) -> Value {
        json!({"level": self.level.number(), "reason": self.reasons, "since": self.since})
    }
}

/// What the operating system says; each is `None` where this platform cannot tell.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// Percent of all cores busy over the last second.
    pub cpu: Option<f64>,
    /// Percent of memory available.
    pub memory_free: Option<f64>,
    pub low_memory: bool,
    pub battery_saver: bool,
    pub thermal: bool,
}

/// One second's measurements.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub lag_p95_ms: f64,
    pub reading: Reading,
}

impl Sample {
    /// The level these measurements ask for, and the reasons, with whether the CPU has been above 95 %
    /// for three samples.
    fn wants(&self, cpu_hot: bool) -> (Level, Vec<String>) {
        let mut level = Level::L0;
        let mut why: Vec<String> = Vec::new();
        let mut ask = |at: Level, reason: &str| {
            level = level.max(at);
            if !why.iter().any(|w| w == reason) {
                why.push(reason.to_string());
            }
        };
        let r = &self.reading;
        if self.lag_p95_ms > 200.0 {
            ask(Level::L3, "lag");
        } else if self.lag_p95_ms > 75.0 {
            ask(Level::L2, "lag");
        } else if self.lag_p95_ms > 30.0 {
            ask(Level::L1, "lag");
        }
        if cpu_hot {
            ask(Level::L2, "cpu");
        } else if r.cpu.is_some_and(|c| c > 85.0) {
            ask(Level::L1, "cpu");
        }
        if r.low_memory || r.memory_free.is_some_and(|m| m < 8.0) {
            ask(Level::L3, "memory");
        } else if r.memory_free.is_some_and(|m| m < 15.0) {
            ask(Level::L2, "memory");
        }
        if r.thermal {
            ask(Level::L2, "thermal");
        }
        if r.battery_saver {
            ask(Level::L1, "battery_saver");
        }
        (level, why)
    }
}

/// Turns a second-by-second stream of samples into levels that hold.
#[derive(Default)]
pub struct Machine {
    status: Status,
    /// Since when (monotonic ms) they ask for less, and the most they asked for.
    below: Option<(u64, Level)>,
    /// Seconds in a row the CPU was above 95 %.
    hot: u32,
    /// Samples in a row that asked for more than the current level, and the least they asked for.
    above: Option<(u32, Level)>,
    /// When (monotonic ms) the level last fell.
    moves: VecDeque<u64>,
}

impl Machine {
    /// Takes the sample of the second ending at `now` (monotonic ms; `wall` is the same moment on
    /// the clock). Returns the new status when the level or the reasons changed.
    pub fn feed(&mut self, now: u64, wall: u64, sample: &Sample) -> Option<Status> {
        self.hot = if sample.reading.cpu.is_some_and(|c| c > 95.0) { self.hot + 1 } else { 0 };
        let (wants, why) = sample.wants(self.hot >= 3);
        let mut to = None;
        if wants > self.status.level {
            // Rises are not rate-limited, but need two samples in a row (one spike is not load).
            self.below = None;
            let (n, least) = self.above.map_or((1, wants), |(n, least)| (n + 1, least.min(wants)));
            self.above = Some((n, least));
            if n >= ENTER_AFTER {
                to = Some(least);
            }
        } else if wants < self.status.level {
            self.above = None;
            let (since, most) = self.below.map_or((now, wants), |(since, most)| (since, most.max(wants)));
            self.below = Some((since, most));
            if now - since >= LEAVE_AFTER_MS {
                self.moves.retain(|at| now.saturating_sub(*at) < 60_000);
                if self.moves.len() < MOVES_PER_MINUTE {
                    to = Some(most);
                    self.moves.push_back(now);
                }
            }
        } else {
            self.above = None;
            self.below = None;
        }
        if let Some(level) = to {
            self.above = None;
            self.below = None;
            self.status = Status { level, reasons: if level == Level::L0 { Vec::new() } else { why }, since: wall };
            return Some(self.status.clone());
        }
        // The level holds, but what asks for it may change.
        if wants == self.status.level && why != self.status.reasons {
            self.status.reasons = why;
            return Some(self.status.clone());
        }
        None
    }
}

/// `L2:thermal`: a level and the reason to give for it.
fn parse_force(text: &str) -> Option<(Level, String)> {
    let (level, reason) = text.split_once(':').unwrap_or((text, "forced"));
    Some((Level::parse(level)?, reason.trim().to_string()))
}

/// The test hook `KELPIE_LOAD_FORCE=L2:thermal` pins the level, whatever is measured.
fn forced() -> Option<(Level, String)> {
    parse_force(&std::env::var("KELPIE_LOAD_FORCE").ok()?)
}

pub struct Load {
    status: watch::Sender<Arc<Status>>,
    began: Instant,
    /// Measuring tasks alive.
    tasks: AtomicUsize,
    /// Milliseconds after `began`, plus one, of the last probe tick and of the last stalled one (0: never).
    beat: AtomicU64,
    stall: AtomicU64,
    last: Mutex<Sample>,
    /// Every probe since the start (or the last reset), by how late it was: for the lag percentiles over a long run.
    lags: [AtomicU64; LAG_EDGES_MS.len() + 1],
    phones: Mutex<(usize, Option<os::Activity>)>,
}

impl Load {
    pub fn new() -> Arc<Load> {
        Arc::new(Load {
            status: watch::channel(Arc::new(Status::default())).0,
            began: Instant::now(),
            tasks: AtomicUsize::new(0),
            beat: AtomicU64::new(0),
            stall: AtomicU64::new(0),
            last: Mutex::new(Sample::default()),
            lags: std::array::from_fn(|_| AtomicU64::new(0)),
            phones: Mutex::new((0, None)),
        })
    }

    pub fn level(&self) -> Level {
        self.status.borrow().level
    }

    pub fn status(&self) -> Arc<Status> {
        self.status.borrow().clone()
    }

    /// Pins the level and its reasons.
    #[cfg(test)]
    pub fn set_reasons_for_test(&self, level: Level, reasons: &[&str]) {
        self.status.send_replace(Arc::new(Status { level, reasons: reasons.iter().map(|r| r.to_string()).collect(), since: 0 }));
    }

    /// Pins the level, as the test hook does.
    #[cfg(test)]
    pub fn set_for_test(&self, level: Level) {
        self.status.send_replace(Arc::new(Status { level, reasons: vec!["test".into()], since: 0 }));
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<Status>> {
        self.status.subscribe()
    }

    /// Whether the measuring runs (it does only while phone access is on).
    pub fn running(&self) -> bool {
        self.tasks.load(Ordering::Relaxed) > 0
    }

    /// Whether the event loop has run on time since `t`: a deadline that fell in a stall does not count against a peer.
    pub fn healthy_since(&self, t: Instant) -> bool {
        match self.stall.load(Ordering::Relaxed) {
            0 => true,
            at => self.began + Duration::from_millis(at - 1) <= t,
        }
    }

    /// What the last second measured, for the `mobile.load` call.
    pub fn report(&self) -> Value {
        let (status, sample) = (self.status(), self.last.lock().clone());
        let mut v = status.json();
        v["running"] = json!(self.running());
        v["lag_p95_ms"] = json!(sample.lag_p95_ms);
        v["cpu"] = json!(sample.reading.cpu);
        v["memory_free"] = json!(sample.reading.memory_free);
        v["lag_p99_ms"] = json!(self.lag_percentile(0.99));
        v["lag_p999_ms"] = json!(self.lag_percentile(0.999));
        v["probes"] = json!(self.lags.iter().map(|b| b.load(Ordering::Relaxed)).sum::<u64>());
        v["phones"] = json!(self.phones.lock().0);
        v
    }

    /// The upper edge of the bucket the `q` quantile of the lags since the start (or the last
    /// `reset_lags`) falls in, in ms.
    pub fn lag_percentile(&self, q: f64) -> f64 {
        let counts: Vec<u64> = self.lags.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        let total: u64 = counts.iter().sum();
        let mut seen = 0;
        for (i, n) in counts.iter().enumerate() {
            seen += n;
            if total > 0 && seen as f64 >= q * total as f64 {
                return LAG_EDGES_MS.get(i).copied().unwrap_or(f64::INFINITY);
            }
        }
        0.0
    }

    /// Starts the lag percentiles over (a test run measuring only its own load).
    pub fn reset_lags(&self) {
        for bucket in &self.lags {
            bucket.store(0, Ordering::Relaxed);
        }
    }

    fn ms(&self) -> u64 {
        self.began.elapsed().as_millis() as u64
    }

    /// Starts measuring until `stopped` turns true. `unresponsive` is told (from the watchdog's
    /// own thread) how long the event loop has been silent, once it has been for 30 s.
    pub fn start(self: &Arc<Self>, mut stopped: watch::Receiver<bool>, unresponsive: Arc<dyn Fn(u64) + Send + Sync>) {
        os::opt_out_of_throttling();
        let forced = forced();
        self.status.send_replace(Arc::new(Status::default()));
        self.tasks.fetch_add(1, Ordering::Relaxed);
        let done = Arc::new(AtomicBool::new(false));
        self.beat.store(self.ms() + 1, Ordering::Relaxed);
        let (load, flag) = (self.clone(), done.clone());
        let handle = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("kelpie-watchdog".into())
            .spawn(move || load.watch_the_loop(&flag, &handle, &*unresponsive))
            .ok();
        let load = self.clone();
        tokio::spawn(async move {
            // Dropped on any way out, including an abort: the watchdog stops with the measuring.
            struct Done(Arc<Load>, Arc<AtomicBool>);
            impl Drop for Done {
                fn drop(&mut self) {
                    self.1.store(true, Ordering::Relaxed);
                    self.0.tasks.fetch_sub(1, Ordering::Relaxed);
                }
            }
            let _done = Done(load.clone(), done);
            load.measure(&mut stopped, forced).await;
        });
    }

    async fn measure(&self, stopped: &mut watch::Receiver<bool>, forced: Option<(Level, String)>) {
        let mut probe = tokio::time::interval(PROBE);
        let mut second = tokio::time::interval(Duration::from_secs(1));
        probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        second.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut lags: VecDeque<f64> = VecDeque::with_capacity(LAGS_KEPT);
        let (mut machine, mut os) = (Machine::default(), os::Probe::default());
        let mut last = Instant::now();
        if let Some((level, reason)) = &forced {
            self.status.send_replace(Arc::new(Status { level: *level, reasons: vec![reason.clone()], since: crate::session::now_millis() }));
        }
        loop {
            tokio::select! {
                biased;
                _ = stopped.wait_for(|s| *s) => return,
                _ = probe.tick() => {
                    let now = Instant::now();
                    let late = now.saturating_duration_since(last + PROBE);
                    last = now;
                    self.beat.store(self.ms() + 1, Ordering::Relaxed);
                    if late >= STALL {
                        self.stall.store(self.ms() + 1, Ordering::Relaxed);
                    }
                    if lags.len() == LAGS_KEPT {
                        lags.pop_front();
                    }
                    let late_ms = late.as_secs_f64() * 1000.0;
                    lags.push_back(late_ms);
                    let bucket = LAG_EDGES_MS.iter().position(|edge| late_ms <= *edge).unwrap_or(LAG_EDGES_MS.len());
                    self.lags[bucket].fetch_add(1, Ordering::Relaxed);
                }
                _ = second.tick() => {
                    let sample = Sample { lag_p95_ms: p95(&lags), reading: os.read() };
                    *self.last.lock() = sample.clone();
                    if forced.is_none() {
                        if let Some(status) = machine.feed(self.ms(), crate::session::now_millis(), &sample) {
                            tracing::info!(level = status.level.number(), reasons = ?status.reasons, lag_p95_ms = sample.lag_p95_ms, "load level");
                            self.status.send_replace(Arc::new(status));
                        }
                    }
                }
            }
        }
    }

    /// A plain thread that notices the event loop going silent, which nothing on it can: it logs
    /// what the runtime and the threads are doing, and at 30 s tells the app. It never touches a session.
    fn watch_the_loop(&self, done: &AtomicBool, runtime: &tokio::runtime::Handle, unresponsive: &(dyn Fn(u64) + Send + Sync)) {
        let (mut reported, mut told) = (false, false);
        while !done.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            let silent = Duration::from_millis(self.ms().saturating_sub(self.beat.load(Ordering::Relaxed).saturating_sub(1)));
            if silent < STALLED_AFTER {
                (reported, told) = (false, false);
                continue;
            }
            if !reported {
                reported = true;
                let m = runtime.metrics();
                tracing::error!(
                    silent_s = silent.as_secs(), workers = m.num_workers(), tasks = m.num_alive_tasks(), queued = m.global_queue_depth(),
                    threads = %os::thread_report(), "runtime stalled"
                );
            }
            if silent >= UNRESPONSIVE_AFTER && !told {
                told = true;
                unresponsive(silent.as_secs());
            }
        }
    }

    /// A phone connected; dropped when it goes. While any is, the OS is asked not to nap the daemon.
    pub fn phone(self: &Arc<Self>) -> PhoneGuard {
        let mut phones = self.phones.lock();
        phones.0 += 1;
        if phones.1.is_none() {
            phones.1 = Some(os::Activity::begin());
        }
        PhoneGuard(self.clone())
    }
}

pub struct PhoneGuard(Arc<Load>);

impl Drop for PhoneGuard {
    fn drop(&mut self) {
        let mut phones = self.0.phones.lock();
        phones.0 -= 1;
        if phones.0 == 0 {
            phones.1 = None;
        }
    }
}

/// The 95th percentile of the lags, in ms.
fn p95(lags: &VecDeque<f64>) -> f64 {
    let mut sorted: Vec<f64> = lags.iter().copied().collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    sorted.get((sorted.len() * 95).div_ceil(100).saturating_sub(1)).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(lag: f64, cpu: f64, free: f64) -> Sample {
        Sample { lag_p95_ms: lag, reading: Reading { cpu: Some(cpu), memory_free: Some(free), ..Default::default() } }
    }

    /// Feeds one sample a second from `from` for `seconds`; the status after each change.
    fn run(m: &mut Machine, from: u64, seconds: u64, s: &Sample) -> Vec<(u64, Level)> {
        (from..from + seconds).filter_map(|t| m.feed(t * 1000, t * 1000, s).map(|st| (t, st.level))).collect()
    }

    #[test]
    fn a_level_is_entered_after_two_samples_and_left_after_ten_seconds() {
        let mut m = Machine::default();
        assert!(run(&mut m, 0, 5, &sample(5.0, 20.0, 60.0)).is_empty(), "calm stays calm");
        // One spike is not a change; two in a row are.
        assert!(m.feed(5_000, 5_000, &sample(300.0, 20.0, 60.0)).is_none());
        assert_eq!(m.feed(6_000, 6_000, &sample(300.0, 20.0, 60.0)).map(|s| s.level), Some(Level::L3));
        // Calm again: nothing changes for ten seconds, then it falls to what the calm asks for.
        let falls = run(&mut m, 7, 12, &sample(5.0, 20.0, 60.0));
        assert_eq!(falls, vec![(17, Level::L0)], "left after ten seconds below");
    }

    #[test]
    fn rises_are_never_held_back_by_earlier_changes() {
        let mut m = Machine::default();
        // Load that ramps up: L1, then L2, with a second between samples; every rise takes two.
        let ramp = [(40.0, 60.0), (40.0, 60.0), (90.0, 60.0), (90.0, 60.0), (10.0, 6.0), (10.0, 6.0)];
        let levels: Vec<Option<Level>> = ramp.iter().enumerate().map(|(i, (lag, free))| m.feed(i as u64 * 1000, i as u64 * 1000, &sample(*lag, 10.0, *free)).map(|s| s.level)).collect();
        assert_eq!(levels, [None, Some(Level::L1), None, Some(Level::L2), None, Some(Level::L3)], "free memory under 8 % is L3 two seconds on, though two changes were just made");
    }

    #[test]
    fn the_thresholds_of_each_level() {
        let at = |lag: f64, cpu: f64, free: f64| {
            let mut m = Machine::default();
            let s = sample(lag, cpu, free);
            m.feed(0, 0, &s);
            m.feed(1000, 1000, &s).map_or(Level::L0, |st| st.level)
        };
        assert_eq!(at(31.0, 10.0, 50.0), Level::L1);
        assert_eq!(at(10.0, 86.0, 50.0), Level::L1);
        assert_eq!(at(76.0, 10.0, 50.0), Level::L2);
        assert_eq!(at(10.0, 10.0, 14.0), Level::L2, "free memory below 15 %");
        assert_eq!(at(201.0, 10.0, 50.0), Level::L3);
        assert_eq!(at(10.0, 10.0, 7.0), Level::L3, "free memory below 8 %");
        assert_eq!(at(10.0, 90.0, 50.0), Level::L1, "busy CPU alone is L1");
        // Pegged for three samples is L2.
        let mut m = Machine::default();
        let hot = sample(10.0, 97.0, 50.0);
        run(&mut m, 0, 5, &hot);
        assert_eq!(m.status.level, Level::L2, "{:?}", m.status);
        assert_eq!(m.status.reasons, vec!["cpu".to_string()]);
    }

    #[test]
    fn power_saver_and_heat_only_raise() {
        let mut m = Machine::default();
        let mut s = sample(5.0, 10.0, 60.0);
        s.reading.battery_saver = true;
        run(&mut m, 0, 3, &s);
        assert_eq!((m.status.level, m.status.reasons.clone()), (Level::L1, vec!["battery_saver".to_string()]));
        s.reading.battery_saver = false;
        s.reading.thermal = true;
        run(&mut m, 3, 3, &s);
        assert_eq!(m.status.level, Level::L2);
    }

    #[test]
    fn at_most_two_falls_a_minute() {
        let mut m = Machine::default();
        let (calm, busy) = (sample(5.0, 10.0, 60.0), sample(300.0, 10.0, 60.0));
        let mut moves = Vec::new();
        // Alternate twelve seconds of each for two minutes: it wants to move every twelve seconds.
        for round in 0..10u64 {
            let s = if round % 2 == 0 { &busy } else { &calm };
            moves.extend(run(&mut m, round * 12, 12, s));
        }
        let falls: Vec<u64> = moves.iter().filter(|(_, level)| *level < Level::L3).map(|(t, _)| *t).collect();
        for window in falls.windows(3) {
            assert!(window[2] - window[0] >= 60, "three falls within a minute: {moves:?}");
        }
        assert!(falls.len() >= 2 && moves.len() > falls.len() + 1, "{moves:?}");
    }

    #[test]
    fn from_calm_to_loaded_takes_two_seconds_and_back_under_twenty_five() {
        let mut m = Machine::default();
        let up = run(&mut m, 0, 5, &sample(120.0, 99.0, 50.0));
        assert!(up[0].0 <= 2 && up[0].1 >= Level::L2, "{up:?}");
        // The last change was at t=1; a minute of history is irrelevant to the first descent.
        let down = run(&mut m, 5, 25, &sample(5.0, 5.0, 60.0));
        assert!(down.last().is_some_and(|(t, l)| *l == Level::L0 && *t <= 5 + 14), "{down:?}");
    }

    #[test]
    fn only_a_shortage_of_memory_clears_what_is_kept_for_away_phones() {
        let status = |level: Level, reasons: &[&str]| Status { level, reasons: reasons.iter().map(|r| r.to_string()).collect(), since: 0 };
        assert!(status(Level::L3, &["memory"]).short_of_memory());
        assert!(status(Level::L3, &["cpu", "memory"]).short_of_memory());
        assert!(!status(Level::L3, &["lag"]).short_of_memory(), "lag");
        assert!(!status(Level::L3, &["cpu"]).short_of_memory(), "CPU");
        assert!(!status(Level::L2, &["memory"]).short_of_memory(), "memory under 15 % is L2: nothing is dropped yet");
        assert!(!Status::default().short_of_memory());
        // The sample that asks for it: free memory under 8 % or the low-memory event, and nothing else.
        let mut m = Machine::default();
        let low = sample(5.0, 10.0, 6.0);
        m.feed(0, 0, &low);
        assert!(m.feed(1000, 1000, &low).expect("L3").short_of_memory());
        let mut m = Machine::default();
        let slow = sample(300.0, 10.0, 60.0);
        m.feed(0, 0, &slow);
        assert!(!m.feed(1000, 1000, &slow).expect("L3").short_of_memory());
    }

    #[test]
    fn the_test_hook_names_a_level_and_a_reason() {
        assert_eq!(parse_force("L2:thermal"), Some((Level::L2, "thermal".to_string())));
        assert_eq!(parse_force("3"), Some((Level::L3, "forced".to_string())));
        assert_eq!(parse_force("L9"), None);
    }

    #[test]
    fn the_ladder_sheds_by_level() {
        assert_eq!([Level::L0, Level::L1, Level::L2, Level::L3].map(|l| l.focused_gap().as_millis()), [100, 100, 250, 1000]);
        assert_eq!([Level::L0, Level::L1, Level::L2, Level::L3].map(|l| l.background_gap().map(|g| g.as_millis())), [Some(100), Some(250), Some(1000), None]);
        assert_eq!([Level::L0, Level::L1, Level::L2, Level::L3].map(|l| l.frame_rows_bytes() / 1024), [56, 56, 28, 12]);
        assert_eq!([Level::L2, Level::L3].map(|l| l.chat_resets()), [2, 1]);
        assert_eq!([Level::L1, Level::L2, Level::L3].map(|l| l.pairing_gap().as_secs()), [0, 1, 5]);
        assert_eq!((Level::L1.patience(), Level::L2.patience()), (1, 2));
    }

    #[test]
    fn the_lag_since_the_start_is_kept_as_percentiles() {
        let load = Load::new();
        assert_eq!(load.lag_percentile(0.99), 0.0, "nothing measured");
        // 990 on-time probes and 10 that were 150 ms late: p99 is still the quick bucket, p99.9 the slow one.
        load.lags[0].store(990, Ordering::Relaxed);
        load.lags[9].store(10, Ordering::Relaxed);
        assert_eq!((load.lag_percentile(0.99), load.lag_percentile(0.999)), (1.0, 200.0));
        load.reset_lags();
        assert_eq!(load.report()["probes"], 0);
    }

    #[test]
    fn the_lag_is_a_percentile_of_the_last_samples() {
        let lags: VecDeque<f64> = (1..=50).map(f64::from).collect();
        assert_eq!(p95(&lags), 48.0);
        assert_eq!(p95(&VecDeque::new()), 0.0);
    }

    #[tokio::test]
    async fn nothing_runs_until_started_and_stops_with_its_owner() {
        let load = Load::new();
        assert!(!load.running(), "feature off: no probe");
        let (stop, stopped) = watch::channel(false);
        load.start(stopped, Arc::new(|_| {}));
        assert!(load.running());
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(load.healthy_since(Instant::now()));
        stop.send_replace(true);
        for _ in 0..40 {
            if !load.running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!load.running(), "the probe ends with phone access");
        // A phone connected asks the OS not to nap the daemon, and lets go when it leaves.
        let guard = load.phone();
        assert_eq!(load.report()["phones"], 1);
        drop(guard);
        assert_eq!(load.report()["phones"], 0);
    }
}
