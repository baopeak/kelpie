//! A phone's screen over WebRTC data channels, the computer's side: the answering peer (`peer`, str0m with the platform's own DTLS), the
//! STUN binding that finds this computer's public address (`stun`), the chunks media messages travel in (`frame`), and a test-only bad
//! network (`impair`).
//!
//! The peer is the daemon's: the phone offers, inside its end-to-end encrypted link (so the DTLS fingerprint in the offer is the phone's own,
//! and the one in the answer the daemon's), and three channels are negotiated by number: 0 `"m"` media (reliable, ordered), 1 `"c"` the
//! cursor (unordered, never sent again), 2 `"i"` the phone's acknowledgements and input (reliable, ordered).

pub mod frame;
pub mod impair;
pub mod stats;
pub mod stun;

#[cfg(windows)]
pub mod peer;
/// A phone's side, for tests here and in the daemon.
#[cfg(all(windows, any(test, feature = "test-phone")))]
pub mod test_phone;

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;

/// The channels, by the number both sides negotiate them with.
pub const MEDIA: u16 = 0;
pub const CURSOR: u16 = 1;
pub const INPUT: u16 = 2;

/// Cloudflare's STUN server: the phone's and the computer's public addresses (free; no account).
pub const STUN_SERVER: &str = "stun.cloudflare.com:3478";

/// The latest of something (the cursor's place): a newer one replaces one not yet taken, and whoever waits is woken.
#[derive(Clone, Default)]
pub struct Latest(Arc<LatestInner>);

#[derive(Default)]
struct LatestInner {
    value: Mutex<Option<Vec<u8>>>,
    ready: Notify,
}

impl Latest {
    pub fn put(&self, v: Vec<u8>) {
        *self.0.value.lock() = Some(v);
        self.0.ready.notify_one();
    }

    pub fn take(&self) -> Option<Vec<u8>> {
        self.0.value.lock().take()
    }

    /// Resolves once something may be waiting (a wake-up that comes while nobody waits is kept for the next).
    pub async fn ready(&self) {
        self.0.ready.notified().await;
    }
}
