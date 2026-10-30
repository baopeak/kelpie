//! What stands between the daemon's work for a phone and its socket: the bounded bulk lane, and the
//! heartbeat that tells a phone that is there from one that is not.
//!
//! One writer owns the socket. Replies to the phone's requests, heartbeats, the session list and
//! `load.status` it writes itself, ahead of the bulk lane, so none of them waits for more than the send
//! in progress. The bulk lane carries what other tasks produce: screen frames and everything about a
//! chat, in order (a resolution is queued after the lines it is about). It does not grow: a frame is made
//! only when the lane has room for it (latest state only), a chat message waits for room, and a lane
//! without room for `SLOW_AFTER` ends the connection (`slow_consumer`), as does a socket that takes
//! nothing for that long. Nothing is dropped silently.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, Notify};

/// Bulk messages waiting for the socket: frames and chat messages are about 64 KiB at most, so this
/// is the lane's 256 KiB (a chat line longer than that goes alone).
const BULK_CAPACITY: usize = 4;
/// A socket that takes nothing for this long, or a bulk lane without room for this long, is cut.
pub const SLOW_AFTER: Duration = Duration::from_secs(10);

/// How often a connection checks its peer.
pub const TICK: Duration = Duration::from_secs(1);
/// Silence before the daemon sends a heartbeat; how long before it sends the next if none comes
/// back; silence after which the phone is gone.
const IDLE: Duration = Duration::from_secs(20);
const PONG: Duration = Duration::from_secs(8);
const DEAD: Duration = Duration::from_secs(45);
/// Deadlines that fell in a stretch the daemon itself was stalled do not count against the phone,
/// but a phone that has not been heard from for this long is gone whatever the machine was doing.
const EXCUSE_LIMIT: Duration = Duration::from_secs(600);

/// The bulk lane's two ends (the writer owns the receiving one), and the signal that something decided
/// the consumer is too slow.
pub struct Lanes {
    pub bulk: mpsc::Sender<String>,
    pub bulk_rx: mpsc::Receiver<String>,
    /// Fires when something decided the consumer is too slow.
    pub slow: Arc<Notify>,
}

pub fn lanes() -> Lanes {
    let (bulk, bulk_rx) = mpsc::channel(BULK_CAPACITY);
    Lanes { bulk, bulk_rx, slow: Arc::new(Notify::new()) }
}

/// Whether the phone on the other end is there. Anything it sends counts; the daemon asks (`Beat::Ping`)
/// when it has been quiet, and gives up when it stays so.
pub struct Liveness {
    /// When the phone last said anything, or when the daemon last had no way of knowing.
    heard: Instant,
    /// When it last really did.
    really: Instant,
    ticked: Instant,
    asked: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Beat {
    Quiet,
    /// Send a heartbeat.
    Ping,
    /// Silent for too long.
    Dead,
}

impl Liveness {
    pub fn new(now: Instant) -> Liveness {
        Liveness { heard: now, really: now, ticked: now, asked: None }
    }

    /// The phone sent something.
    pub fn heard(&mut self, now: Instant) {
        self.heard = now;
        self.really = now;
        self.asked = None;
    }

    /// One look at the phone, once per `TICK`. `patience` stretches the deadlines (2 when the machine is
    /// loaded); `healthy_since` says whether the daemon's own event loop ran on time since a moment.
    pub fn tick(&mut self, now: Instant, patience: u32, healthy_since: impl Fn(Instant) -> bool) -> Beat {
        let gap = now.saturating_duration_since(self.ticked);
        self.ticked = now;
        let unwatched = gap > 2 * TICK;
        // The daemon was not running (stalled, or the machine slept), or ran late in the window the
        // silence is measured over: nothing is known about the phone, so the clock starts again.
        if (unwatched || !healthy_since(self.heard)) && now.saturating_duration_since(self.really) < EXCUSE_LIMIT {
            self.heard = now;
            self.asked = None;
        }
        let silent = now.saturating_duration_since(self.heard);
        if silent >= DEAD * patience {
            return Beat::Dead;
        }
        if silent >= IDLE * patience && self.asked.is_none_or(|at| now.saturating_duration_since(at) >= PONG * patience) {
            self.asked = Some(now);
            return Beat::Ping;
        }
        Beat::Quiet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, s: u64) -> Instant {
        start + Duration::from_secs(s)
    }

    /// Ticks once a second until `until` (or until the phone is dropped), returning each beat that was
    /// not `Quiet` with its time.
    fn watch(l: &mut Liveness, start: Instant, from: u64, until: u64, patience: u32) -> Vec<(u64, Beat)> {
        let mut beats = Vec::new();
        for s in from..=until {
            match l.tick(at(start, s), patience, |_| true) {
                Beat::Quiet => {}
                Beat::Dead => {
                    beats.push((s, Beat::Dead));
                    break;
                }
                beat => beats.push((s, beat)),
            }
        }
        beats
    }

    #[test]
    fn a_quiet_phone_is_asked_after_twenty_seconds_again_every_eight_and_dropped_after_forty_five() {
        let start = Instant::now();
        let mut l = Liveness::new(start);
        let beats = watch(&mut l, start, 1, 50, 1);
        assert_eq!(beats.iter().map(|(s, b)| (*s, matches!(b, Beat::Ping))).collect::<Vec<_>>(), [(20, true), (28, true), (36, true), (44, true), (45, false)]);
        assert_eq!(beats.last().map(|(_, b)| b), Some(&Beat::Dead));
    }

    #[test]
    fn anything_it_sends_resets_the_silence() {
        let start = Instant::now();
        let mut l = Liveness::new(start);
        assert_eq!(watch(&mut l, start, 1, 19, 1), []);
        l.heard(at(start, 19));
        assert_eq!(watch(&mut l, start, 20, 38, 1), [], "twenty seconds start again from the last message");
        assert_eq!(watch(&mut l, start, 39, 39, 1)[0].1, Beat::Ping);
        l.heard(at(start, 40));
        assert_eq!(watch(&mut l, start, 41, 59, 1), []);
    }

    #[test]
    fn a_loaded_machine_waits_twice_as_long() {
        let start = Instant::now();
        let mut l = Liveness::new(start);
        let beats = watch(&mut l, start, 1, 95, 2);
        assert_eq!(beats.first().map(|(s, _)| *s), Some(40));
        assert_eq!(beats.get(1).map(|(s, _)| *s), Some(56));
        assert_eq!(beats.last().map(|(s, b)| (*s, matches!(b, Beat::Dead))), Some((90, true)));
    }

    #[test]
    fn a_stall_of_the_daemon_is_not_the_phones_silence() {
        let start = Instant::now();
        let mut l = Liveness::new(start);
        // The machine slept for a while: the next look finds nothing is known, and starts over.
        assert_eq!(l.tick(at(start, 500), 1, |_| true), Beat::Quiet);
        assert_eq!(watch(&mut l, start, 501, 519, 1), []);
        assert_eq!(watch(&mut l, start, 520, 520, 1)[0].1, Beat::Ping);
        // The event loop was late somewhere in the window: the deadline does not count either.
        let mut l = Liveness::new(start);
        assert_eq!(watch(&mut l, start, 1, 30, 1)[0].0, 20, "asked once");
        assert_eq!(l.tick(at(start, 31), 1, |_| false), Beat::Quiet);
        assert_eq!(watch(&mut l, start, 32, 50, 1), []);
        assert_eq!(watch(&mut l, start, 51, 51, 1)[0].1, Beat::Ping, "twenty seconds from the stall");
        // But a phone silent for ten minutes is gone, stalls or not.
        let mut l = Liveness::new(start);
        for s in 1..=700 {
            if l.tick(at(start, s), 1, |_| false) == Beat::Dead {
                return;
            }
        }
        panic!("a phone silent for ten minutes was never dropped");
    }

    #[tokio::test]
    async fn the_bulk_lane_is_bounded() {
        let lanes = lanes();
        for _ in 0..BULK_CAPACITY {
            lanes.bulk.try_send("frame".into()).unwrap();
        }
        assert!(lanes.bulk.try_send("frame".into()).is_err());
    }
}
