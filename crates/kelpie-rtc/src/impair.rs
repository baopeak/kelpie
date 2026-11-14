//! A bad network on purpose, for tests: `KELPIE_RTC_TEST_IMPAIR=loss:0.05,delay_ms:50,kbps:300` (or the same in the file that
//! `KELPIE_RTC_TEST_IMPAIR_FILE` names) loses, delays and slows the peer's UDP
//! packets, both ways. Read only in debug builds or with `KELPIE_E2E=1` (as `KELPIE_LOAD_FORCE` is): a release daemon a person runs never
//! drops packets of its own.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Impair {
    /// Each packet is lost with this chance (0 to 1).
    pub loss: f64,
    /// Each packet waits this long, one way.
    pub delay: Duration,
    /// Each direction carries at most this much (0: no limit).
    pub kbps: u32,
}

impl Impair {
    /// `loss:0.05,delay_ms:50,kbps:300`, any of them; None when nothing in it is understood or nothing would change.
    pub fn parse(s: &str) -> Option<Impair> {
        let mut i = Impair::default();
        for part in s.split(',') {
            let (k, v) = part.split_once(':')?;
            match k.trim() {
                "loss" => i.loss = v.trim().parse::<f64>().ok().filter(|l| (0.0..=1.0).contains(l))?,
                "delay_ms" => i.delay = Duration::from_millis(v.trim().parse().ok()?),
                "kbps" => i.kbps = v.trim().parse().ok()?,
                _ => return None,
            }
        }
        (i != Impair::default()).then_some(i)
    }

    /// The impairment the environment asks for, where it may be asked for at all: `KELPIE_RTC_TEST_IMPAIR`, or the file
    /// `KELPIE_RTC_TEST_IMPAIR_FILE` names, read for each new peer (so a test changes the condition without restarting the daemon).
    pub fn from_env() -> Option<Impair> {
        let allowed = cfg!(debug_assertions) || std::env::var("KELPIE_E2E").is_ok_and(|v| v == "1");
        if !allowed {
            return None;
        }
        let spec = match std::env::var("KELPIE_RTC_TEST_IMPAIR_FILE") {
            Ok(path) => std::fs::read_to_string(path).ok()?,
            Err(_) => std::env::var("KELPIE_RTC_TEST_IMPAIR").ok()?,
        };
        Impair::parse(spec.trim())
    }
}

/// One direction's queue: what is lost, and when what is not may go.
pub struct Lane {
    cfg: Impair,
    rng: u64,
    /// When the link is free again (the rate's queue).
    free_at: Instant,
}

impl Lane {
    pub fn new(cfg: Impair, seed: u64) -> Lane {
        Lane { cfg, rng: seed | 1, free_at: Instant::now() }
    }

    fn chance(&mut self) -> f64 {
        // xorshift64*: enough for a test's coin.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// When a packet of `len` bytes handed over `now` arrives on the other side; None when it is lost.
    pub fn due(&mut self, now: Instant, len: usize) -> Option<Instant> {
        if self.cfg.loss > 0.0 && self.chance() < self.cfg.loss {
            return None;
        }
        let mut leaves = now;
        if self.cfg.kbps > 0 {
            let start = self.free_at.max(now);
            self.free_at = start + Duration::from_secs_f64(len as f64 * 8.0 / (f64::from(self.cfg.kbps) * 1000.0));
            leaves = self.free_at;
        }
        Some(leaves + self.cfg.delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_it_knows_only() {
        assert_eq!(Impair::parse("loss:0.05,delay_ms:50,kbps:300"), Some(Impair { loss: 0.05, delay: Duration::from_millis(50), kbps: 300 }));
        assert_eq!(Impair::parse("delay_ms:25"), Some(Impair { delay: Duration::from_millis(25), ..Impair::default() }));
        assert_eq!(Impair::parse("loss:2"), None, "a chance past 1");
        assert_eq!(Impair::parse("jitter:5"), None);
        assert_eq!(Impair::parse("loss:0"), None, "nothing to change");
        assert_eq!(Impair::parse(""), None);
    }

    #[test]
    fn loses_about_the_share_asked_and_holds_the_rate() {
        let now = Instant::now();
        let mut lane = Lane::new(Impair { loss: 0.1, ..Impair::default() }, 7);
        let lost = (0..10_000).filter(|_| lane.due(now, 1000).is_none()).count();
        assert!((800..1200).contains(&lost), "{lost}");
        // 100 kbps: 100 packets of 125 bytes take a second to leave.
        let mut slow = Lane::new(Impair { kbps: 100, delay: Duration::from_millis(50), ..Impair::default() }, 7);
        let last = (0..100).map(|_| slow.due(now, 125).unwrap()).last().unwrap();
        let took = last.duration_since(now);
        assert!(took >= Duration::from_millis(1040) && took <= Duration::from_millis(1060), "{took:?}");
    }
}
