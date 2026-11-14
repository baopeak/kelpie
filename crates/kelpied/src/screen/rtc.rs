//! The screen peer to peer: WebRTC data channels straight between the phone and this computer (`kelpie_rtc`), set up over the phone's
//! end-to-end encrypted link.
//!
//! - **Signalling, inside the link:** the phone's offer (`sc_rtc_offer`) is answered with this computer's SDP, a nonce and the STUN servers;
//!   late candidates of the phone's come as `sc_rtc_ice`. Since the offer and the answer travel only inside the link, the DTLS fingerprints in
//!   them are the two ends' own, and str0m refuses a peer whose certificate is another.
//! - **Bound to the screen:** the first message on `"i"` must be `{type: "rtc_hello", sc, nonce}` with this offer's nonce; anything else
//!   closes the peer.
//! - **The switch:** once the channels are open and the hello is right, the hold's picture moves onto the peer under its lock, and the helper
//!   starts again from a keyframe (as after a rebind); the link keeps the JSON (`sc_state`, `sc_stats`, `sc_end`) and says `sc_path`.
//! - **Input and acknowledgements on `"i"`** go through the same code as the link's verbs, with the screen's rule looked at again for each
//!   (`Mobile::screen_allowed`), and only while the hold's link is there.
//! - **Failure:** back to the link when that way may be used and the screen is not "peer to peer only", else the screen ends (`p2p_failed`).
//! - **Rate:** the relay WebSocket's cap is lifted peer to peer and put back on the link (`screen.cap`); under loss the stream is held under
//!   what the phone acknowledged while `"m"` stays full (`ceiling`).

use std::sync::atomic::AtomicU64;
#[cfg(windows)]
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use super::{pause_notice, refused, Broker, Hold, Phone, State};
use crate::daemon::Daemon;
use crate::mobile::screen_policy::Via;
use crate::session::now_millis;
use crate::takeover::Refused;

/// A phone's SDP and a candidate line, at most, and the late candidates one peer takes.
const MAX_SDP: usize = 16 * 1024;
const MAX_CANDIDATE: usize = 512;
const MAX_CANDIDATES: u32 = 32;
/// A screen that waits for its peer to peer way with no peer being opened ends after this.
pub(super) const P2P_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg_attr(not(windows), allow(dead_code))]
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A peer the hold has: the one its picture goes on, or one being opened. Dropping it ends the peer.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct Side {
    pub(super) id: u64,
    pub(super) media: mpsc::Sender<Vec<u8>>,
    pub(super) cursor: kelpie_rtc::Latest,
    candidates: mpsc::UnboundedSender<String>,
    candidates_left: u32,
    /// Offered through TURN alone: its rule is TURN's from the start.
    pub(super) relay_only: bool,
    pub(super) stop: Arc<Notify>,
}

impl Drop for Side {
    fn drop(&mut self) {
        self.stop.notify_one();
    }
}

/// The ICE servers the phone is given: Cloudflare's STUN (TURN comes with the relay's own credentials).
#[cfg_attr(not(windows), allow(dead_code))]
fn ice_servers() -> Value {
    json!([{"urls": [format!("stun:{}", kelpie_rtc::STUN_SERVER)]}])
}

/// TURN credentials are renewed this long before they end, while a screen's pair goes through TURN.
const RENEW_BEFORE_MS: u64 = 5 * 60 * 1000;

/// The way a hold's picture goes now, which its rule is looked at again for: the link's, or peer to peer (through TURN when its pair is).
pub(super) fn way_now(hold: &Hold, st: &State) -> Via {
    // The pair as measured; until a new peer's first stats, the way it was offered (a peer offered through TURN alone that turns out direct is
    // held to peer to peer's rule from then on).
    let turn = st.turn_path.unwrap_or_else(|| st.rtc.as_ref().is_some_and(|s| s.relay_only));
    match (st.on_p2p, turn) {
        (false, _) => link_way(hold),
        (true, true) => Via::Turn,
        (true, false) => Via::P2p,
    }
}

/// Ends a TURN credential at the relay, off the async threads; best effort.
pub(super) fn revoke_later(d: &Daemon, username: String) {
    let Some(d) = d.this() else { return };
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn_blocking(move || d.mobile.revoke_turn(&username));
    }
}

/// The STUN servers with TURN's minted ones after them.
fn with_turn(turn: &crate::mobile::turn::Minted) -> Value {
    let mut all = ice_servers().as_array().cloned().unwrap_or_default();
    all.extend(turn.ice_servers.as_array().cloned().unwrap_or_default());
    json!(all)
}

/// The way the hold's link goes: the relay's WebSocket or the direct link.
pub(super) fn link_way(hold: &Hold) -> Via {
    if hold.relay {
        Via::Relay
    } else {
        Via::Tailscale
    }
}

/// The bytes a media message takes peer to peer: its chunks' headers on top (DTLS and SCTP are the same for every way out of here).
pub(super) fn rtc_wire_len(len: usize) -> usize {
    let room = kelpie_rtc::frame::MAX_CHUNK - kelpie_rtc::frame::HEADER;
    len + len.div_ceil(room).max(1) * kelpie_rtc::frame::HEADER
}

impl Broker {
    /// `sc_rtc_offer`: the phone's offer for its screen, checked now; the answer is what is returned, which the caller may await elsewhere (the
    /// phone's connection does: its acknowledgements must not wait for a STUN look-up). The picture moves once the peer is open (`ready`).
    pub fn rtc_offer_begin(&self, d: &Arc<Daemon>, phone: Phone<'_>, req: &Value) -> Result<impl std::future::Future<Output = Result<Value, Refused>> + Send + 'static, Refused> {
        let hold = self.held_here(&phone, req)?;
        let sdp = req["sdp"].as_str().filter(|s| !s.is_empty() && s.len() <= MAX_SDP).ok_or_else(|| refused("bad_request", "missing offer"))?.to_string();
        // A phone that offers through TURN alone asks that way's switch; any other offer, peer to peer's.
        let way = if req["relay_only"] == true { Via::Turn } else { Via::P2p };
        d.mobile.screen_way(&hold.device, hold.browser, hold.relay, way).map_err(|(code, message)| refused(code, message))?;
        let offer = {
            let mut st = hold.state.lock();
            st.offers += 1;
            st.offers
        };
        let d = d.clone();
        let relay_only = way == Via::Turn;
        Ok(async move { d.screen.open_peer(&d, &hold, &sdp, offer, relay_only).await })
    }

    #[cfg(not(windows))]
    async fn open_peer(&self, _d: &Arc<Daemon>, _hold: &Arc<Hold>, _sdp: &str, _offer: u64, _relay_only: bool) -> Result<Value, Refused> {
        Err(refused("no_p2p_here", "This computer cannot send its screen peer to peer."))
    }

    #[cfg(windows)]
    async fn open_peer(&self, d: &Arc<Daemon>, hold: &Arc<Hold>, sdp: &str, offer: u64, relay_only: bool) -> Result<Value, Refused> {
        let mut cfg = kelpie_rtc::peer::Config::default();
        // Offline test runs ask no STUN server.
        if std::env::var("KELPIE_RTC_STUN").is_ok_and(|v| v == "off") {
            cfg.stun = None;
        }
        let (answer, peer) = kelpie_rtc::peer::answer(sdp, cfg).await.map_err(|e| refused("p2p_failed", format!("Peer to peer could not start: {e}")))?;
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (candidates, candidates_rx) = mpsc::unbounded_channel();
        let stop = Arc::new(Notify::new());
        let side = Side { id, media: peer.media.clone(), cursor: peer.cursor.clone(), candidates, candidates_left: MAX_CANDIDATES, relay_only, stop: stop.clone() };
        let replaced = {
            let mut st = hold.state.lock();
            if !hold.live() {
                return Err(refused("sc_gone", "That screen has ended."));
            }
            // A newer offer came while this one was answered: that one is the phone's now (this peer is dropped with `side`).
            if st.offers != offer {
                return Err(refused("superseded", "A newer offer replaced this one."));
            }
            st.rtc_next.replace(side)
        };
        drop(replaced);
        tracing::info!(sc = %hold.sc, "screen: peer to peer offered");
        tokio::spawn(pump(d.clone(), hold.clone(), peer, id, nonce.clone(), stop, candidates_rx));
        Ok(json!({"type": "ack", "result": "ok", "sdp": answer, "nonce": nonce, "ice_servers": ice_servers()}))
    }

    /// `sc_rtc_servers`: the ICE servers for the phone's next offer, STUN and, for a screen through the relay with TURN allowed and left in its
    /// allowances, TURN's short-lived credentials (minted at the relay, or the screen's own while they last). Answered `{ice_servers, turn}`,
    /// `turn` being null or why there is none; the caller may await it elsewhere (a mint is a request to the relay).
    pub fn rtc_servers_begin(&self, d: &Arc<Daemon>, phone: Phone<'_>, req: &Value) -> Result<impl std::future::Future<Output = Result<Value, Refused>> + Send + 'static, Refused> {
        let hold = self.held_here(&phone, req)?;
        let d = d.clone();
        Ok(async move {
            let answer = |servers: Value, why: Option<&str>| json!({"type": "ack", "result": "ok", "ice_servers": servers, "turn": why});
            if let Some(why) = d.screen.turn_why(&d, &hold) {
                return Ok(answer(ice_servers(), Some(why)));
            }
            if let Some(m) = hold.state.lock().turn.clone().filter(|m| m.expires > now_millis() + RENEW_BEFORE_MS) {
                return Ok(answer(with_turn(&m), None));
            }
            let dd = d.clone();
            let minted = tokio::task::spawn_blocking(move || dd.mobile.mint_turn()).await.map_err(|_| refused("not_kept", "That did not finish."))?;
            match minted {
                Ok(m) => {
                    let servers = with_turn(&m);
                    {
                        let mut st = hold.state.lock();
                        // The screen ended or spent its allowance while the relay answered: that credential ends now, unused.
                        if !hold.live() || st.turn_capped {
                            drop(st);
                            revoke_later(&d, m.username);
                            return Ok(answer(ice_servers(), Some("turn_budget_spent")));
                        }
                        // The one it replaces may still be in a peer's use: it ends on its own (the screen's end revokes the newest).
                        st.turn = Some(m);
                    }
                    Ok(answer(servers, None))
                }
                Err(why) => Ok(answer(ice_servers(), Some(why.code()))),
            }
        })
    }

    /// Why this screen gets no TURN now: not through the relay, the way turned off, or an allowance spent.
    fn turn_why(&self, d: &Daemon, hold: &Hold) -> Option<&'static str> {
        if !hold.relay {
            return Some("no_turn_direct");
        }
        if d.mobile.screen_way(&hold.device, hold.browser, hold.relay, Via::Turn).is_err() {
            return Some("method_off");
        }
        if hold.state.lock().turn_capped || d.mobile.turn_spent() {
            return Some("turn_budget_spent");
        }
        None
    }

    /// The hold's task, at each look: what went through TURN is counted, a credential near its end renewed (and the phone given it), and past
    /// an allowance the credential ends and the picture goes back to the link (or the screen ends). True when the screen ended.
    pub(super) fn turn_look(&self, d: &Arc<Daemon>, hold: &Arc<Hold>) -> bool {
        let (bytes, on_turn, session, renew) = {
            let mut st = hold.state.lock();
            let renew = st.turn_path == Some(true) && !st.turn_renewing && st.turn.as_ref().is_some_and(|m| m.expires <= now_millis() + RENEW_BEFORE_MS);
            if renew {
                st.turn_renewing = true;
            }
            (std::mem::take(&mut st.turn_unsaved), st.on_p2p && st.turn_path == Some(true), st.turn_bytes, renew)
        };
        if bytes > 0 {
            d.mobile.count_turn(bytes);
        }
        if on_turn && (session >= turn_session_cap(d) || d.mobile.turn_spent()) {
            return self.turn_cap(d, hold);
        }
        if renew {
            let (d, hold) = (d.clone(), hold.clone());
            tokio::spawn(async move {
                let dd = d.clone();
                let minted = tokio::task::spawn_blocking(move || dd.mobile.mint_turn()).await;
                let mut st = hold.state.lock();
                st.turn_renewing = false;
                if let Ok(Ok(m)) = minted {
                    // Ended or capped meanwhile: the new credential ends now, and the phone never hears of it.
                    if !hold.live() || st.turn_capped {
                        drop(st);
                        return revoke_later(&d, m.username);
                    }
                    // The phone opens a new peer with these (the computer swaps to it once open, as after a network change): a peer's TURN
                    // allocation is not known to take new credentials.
                    let servers = with_turn(&m);
                    st.turn = Some(m);
                    if let Some(link) = st.conn.as_ref() {
                        let _ = link.out.try_send(json!({"type": "sc_rtc_servers", "sc": hold.sc, "ice_servers": servers, "server_ts": now_millis()}).to_string());
                    }
                }
            });
        }
        false
    }

    /// An allowance of TURN is spent: its credential ends, and the picture goes back to the link when that way may be used, else the screen
    /// ends. True when it ended.
    fn turn_cap(&self, d: &Arc<Daemon>, hold: &Arc<Hold>) -> bool {
        let link_ok = d.mobile.screen_way(&hold.device, hold.browser, hold.relay, link_way(hold)).is_ok();
        let (gone, end, turn) = {
            let mut st = hold.state.lock();
            st.turn_capped = true;
            st.turn_path = None;
            let turn = st.turn.take();
            if !st.p2p_only && link_ok {
                (self.to_link(d, hold, &mut st, "turn_budget_spent"), false, turn)
            } else {
                (st.rtc.take(), true, turn)
            }
        };
        tracing::info!(sc = %hold.sc, "screen: TURN allowance reached");
        drop(gone);
        if let Some(turn) = turn {
            revoke_later(d, turn.username);
        }
        if end {
            self.end(d, &hold.sc, "turn_budget_spent", true);
        }
        end
    }

    /// `sc_rtc_ice`: a late candidate of the phone's, for the peer being opened (or the one in use).
    pub(super) fn rtc_ice(&self, phone: &Phone<'_>, req: &Value) -> Result<(), Refused> {
        let hold = self.held_here(phone, req)?;
        let cand = req["cand"].as_str().filter(|c| c.len() <= MAX_CANDIDATE && c.trim_start_matches("a=").starts_with("candidate:")).ok_or_else(|| refused("bad_request", "not a candidate"))?;
        let mut st = hold.state.lock();
        let st = &mut *st;
        if let Some(side) = st.rtc_next.as_mut().or(st.rtc.as_mut()) {
            // A phone has a few: more is no phone's.
            if side.candidates_left == 0 {
                return Err(refused("bad_request", "too many candidates"));
            }
            side.candidates_left -= 1;
            let _ = side.candidates.send(cand.to_string());
        }
        Ok(())
    }

    /// `sc_via`: the way from now on. `auto`: peer to peer when it opens, the link when it fails; `p2p`: peer to peer only (a failure ends the
    /// screen); the link's own way (`relay` or `tailscale`): back on the link now, and no peer.
    pub(super) fn rtc_via(&self, d: &Daemon, phone: &Phone<'_>, req: &Value) -> Result<(), Refused> {
        let hold = self.held_here(phone, req)?;
        let via = Via::parse(req["via"].as_str()).ok_or_else(|| refused("bad_request", "unknown way"))?;
        let link = link_way(&hold);
        let way = match via {
            Via::Auto => None,
            Via::P2p => Some(Via::P2p),
            v if v == link => Some(link),
            Via::Tailscale => return Err(refused("no_direct_link", "Tailscale is off on this phone or not reachable now.")),
            _ => return Err(refused("not_on_relay", "This phone is connected directly now: choose Automatic.")),
        };
        if let Some(way) = way {
            d.mobile.screen_way(&hold.device, hold.browser, hold.relay, way).map_err(|(code, message)| refused(code, message))?;
        }
        let gone = {
            let mut st = hold.state.lock();
            st.p2p_only = via == Via::P2p;
            if way == Some(link) {
                let next = st.rtc_next.take();
                let old = if st.on_p2p { self.to_link(d, &hold, &mut st, "chosen") } else { None };
                (next, old)
            } else {
                (None, None)
            }
        };
        drop(gone);
        Ok(())
    }

    /// The picture goes on the hold's link again (from a failed peer, or by choice): the helper starts again from a keyframe, held to the
    /// relay WebSocket's rate when that is the way. The peer it went on is returned, to be dropped outside the lock.
    pub(super) fn to_link(&self, d: &Daemon, hold: &Hold, st: &mut State, why: &str) -> Option<Side> {
        let old = st.rtc.take();
        st.on_p2p = false;
        st.p2p_waiting = None;
        st.paused_for_p2p = false;
        st.ceiling = None;
        restart_flow(st);
        self.cap(d, hold, st);
        if st.conn.is_some() {
            d.providers.notify("screen", "screen.pause", pause_notice(st, &hold.sc, false));
        }
        tracing::info!(sc = %hold.sc, why, "screen: back on the link");
        tell(st, json!({"type": "sc_path", "sc": hold.sc, "path": link_way(hold).word(), "state": "live", "why": why}));
        old
    }

    /// What the stream may send now: the relay WebSocket's rate on that way (for a screen held to it from its start), the ceiling under loss peer
    /// to peer; told under the hold's lock,
    /// so in order.
    pub(super) fn cap(&self, d: &Daemon, hold: &Hold, st: &State) {
        let way_cap = (!st.on_p2p && hold.capped).then(|| d.mobile.relay_ws_kbps());
        let cap = match (way_cap, st.ceiling) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        d.providers.notify("screen", "screen.cap", json!({"sc": hold.sc, "max_kbps": cap}));
    }

    /// The peer `id` is open and said its hello: the picture moves onto it.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn ready(&self, d: &Daemon, hold: &Hold, id: u64) {
        let old = {
            let mut st = hold.state.lock();
            if st.rtc_next.as_ref().is_none_or(|s| s.id != id) || !hold.live() {
                return;
            }
            let next = st.rtc_next.take();
            let old = std::mem::replace(&mut st.rtc, next);
            st.on_p2p = true;
            st.p2p_waiting = None;
            st.ceiling = None;
            st.acked_bytes = 0;
            st.turn_path = None;
            restart_flow(&mut st);
            self.cap(d, hold, &st);
            // A keyframe first, every tile painted again: as on a new connection. While the link is away the helper stays paused, and its
            // rebind resumes it.
            st.paused_for_p2p = false;
            if st.conn.is_some() {
                d.providers.notify("screen", "screen.pause", pause_notice(&mut st, &hold.sc, false));
            }
            tell(&mut st, json!({"type": "sc_path", "sc": hold.sc, "path": "p2p", "state": "live"}));
            old
        };
        drop(old);
        tracing::info!(sc = %hold.sc, "screen: peer to peer");
    }

    /// The peer `id` failed (or was refused): if the picture was on it, back to the link when that may be used, else the screen ends.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn failed(&self, d: &Daemon, hold: &Hold, id: u64, why: &str) {
        let link_ok = d.mobile.screen_way(&hold.device, hold.browser, hold.relay, link_way(hold)).is_ok();
        let (gone, end) = {
            let mut st = hold.state.lock();
            let next = st.rtc_next.as_ref().is_some_and(|s| s.id == id);
            let current = st.rtc.as_ref().is_some_and(|s| s.id == id);
            if !next && !current {
                return;
            }
            let gone = if next { st.rtc_next.take() } else { st.rtc.take() };
            tracing::info!(sc = %hold.sc, why, "screen: peer to peer failed");
            if !st.on_p2p || st.rtc.is_some() {
                // On the link, or still on a working peer: only the phone hears it (it may keep the way from offering again for a while).
                let path = if st.on_p2p { "p2p" } else { link_way(hold).word() };
                tell(&mut st, json!({"type": "sc_path", "sc": hold.sc, "path": path, "state": "failed", "why": why}));
                (gone, false)
            } else if st.rtc_next.is_some() {
                // A new peer is being opened (the phone's network changed): the picture waits for it, the helper paused meanwhile.
                Self::wait_for_p2p(d, hold, &mut st);
                (gone, false)
            } else if !st.p2p_only && link_ok {
                tell(&mut st, json!({"type": "sc_path", "sc": hold.sc, "path": "p2p", "state": "failed", "why": why}));
                let old = self.to_link(d, hold, &mut st, why);
                drop(old);
                (gone, false)
            } else {
                (gone, true)
            }
        };
        drop(gone);
        if end {
            self.end(d, &hold.sc, "p2p_failed", true);
        }
    }

    /// The picture waits for a peer: nothing goes, the helper paused (resumed by `ready`).
    pub(super) fn wait_for_p2p(d: &Daemon, hold: &Hold, st: &mut State) {
        st.on_p2p = true;
        st.p2p_waiting.get_or_insert_with(Instant::now);
        if !st.paused_for_p2p {
            st.paused_for_p2p = true;
            restart_flow(st);
            d.providers.notify("screen", "screen.pause", pause_notice(st, &hold.sc, true));
        }
    }

    /// A message on `"i"`: an acknowledgement or input, as the link's verbs carry them, under the same rules (the screen's rule with the peer to
    /// peer way still on, looked at again for each).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn rtc_input(&self, d: &Arc<Daemon>, hold: &Hold, data: &[u8]) {
        let way = {
            let st = hold.state.lock();
            st.conn.is_some().then(|| way_now(hold, &st))
        };
        if !hold.live() || !way.is_some_and(|way| d.mobile.screen_held(&hold.device, hold.browser, hold.relay, way)) {
            return;
        }
        let Ok(req) = serde_json::from_slice::<Value>(data) else { return };
        match req["type"].as_str() {
            Some("sc_ack") => {
                if let Some(seq) = req["seq"].as_u64().filter(|s| *s <= u64::from(u32::MAX)) {
                    self.ack(d, hold, seq as u32);
                }
            }
            Some("sc_input") => {
                let _ = self.input(d, hold, &req);
            }
            _ => {}
        }
    }

    /// Once a second from the peer: under loss the media channel stays full, and the stream is held to a little under what the phone
    /// acknowledged (no second controller: the helper's own cap); once it drains the hold is let out a quarter every two seconds.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn rtc_stats(&self, d: &Daemon, hold: &Hold, id: u64, s: &kelpie_rtc::stats::Stats) {
        use kelpie_screen::control::{MAX_KBPS, MIN_KBPS};
        let mut st = hold.state.lock();
        if !st.on_p2p || st.rtc.as_ref().is_none_or(|x| x.id != id) {
            return;
        }
        // What went through TURN: the peer's own count since its last stats, while its pair goes through TURN.
        let total = s.bytes_tx + s.bytes_rx;
        let delta = if st.peer_bytes.0 == id { total.saturating_sub(st.peer_bytes.1) } else { 0 };
        st.peer_bytes = (id, total);
        // Known once a pair is chosen.
        if !s.remote.is_empty() {
            st.turn_path = Some(s.path() == "turn");
        }
        if st.turn_path == Some(true) {
            st.turn_bytes += delta;
            st.turn_unsaved += delta;
        }
        let acked_kbps = (std::mem::take(&mut st.acked_bytes) * 8 / 1000).min(u64::from(MAX_KBPS)) as u32;
        let rtt = u64::from(s.rtt_ms.unwrap_or(100).max(50));
        let next = if s.media_full_ms > 2 * rtt {
            st.clear_stats = 0;
            let held = (f64::from(acked_kbps) * 0.85) as u32;
            Some(held.max(MIN_KBPS).min(st.ceiling.unwrap_or(MAX_KBPS)))
        } else if s.media_full_ms == 0 {
            st.clear_stats = st.clear_stats.saturating_add(1);
            if st.clear_stats >= 2 {
                st.clear_stats = 0;
                st.ceiling.map(|c| c.saturating_mul(5) / 4).filter(|c| *c < MAX_KBPS)
            } else {
                st.ceiling
            }
        } else {
            st.ceiling
        };
        tracing::debug!(sc = %hold.sc, path = s.path(), local = s.local, remote = s.remote, rtt_ms = ?s.rtt_ms, buffered = s.media_buffered, full_ms = s.media_full_ms, acked_kbps, "screen peer");
        if next != st.ceiling {
            tracing::info!(sc = %hold.sc, from = ?st.ceiling, to = ?next, full_ms = s.media_full_ms, acked_kbps, "screen: peer to peer ceiling");
            st.ceiling = next;
            self.cap(d, hold, &st);
        }
    }
}

/// The flow starts over on a new way: nothing in flight, a keyframe owed (the helper sends one when resumed).
pub(super) fn restart_flow(st: &mut State) {
    st.sent.clear();
    st.full_since = None;
    st.unacked = 0;
    st.need_key = true;
    st.need_key_since = Instant::now();
    st.lost_tiles = true;
}

/// An `sc_path` to the hold's phone; while its link is away (or full), the latest is kept for the next connection (`rebind`).
fn tell(st: &mut State, msg: Value) {
    let mut msg = msg;
    msg["server_ts"] = json!(now_millis());
    let msg = msg.to_string();
    let unsent = match st.conn.as_ref() {
        Some(link) => link.out.try_send(msg).err().map(|e| e.into_inner()),
        None => Some(msg),
    };
    if unsent.is_some() {
        st.unsent_path = unsent;
    }
}

/// One peer's events, until it fails or the hold lets go of it (its `Side` dropped).
#[cfg(windows)]
async fn pump(d: Arc<Daemon>, hold: Arc<Hold>, mut peer: kelpie_rtc::peer::Peer, id: u64, nonce: String, stop: Arc<Notify>, mut candidates: mpsc::UnboundedReceiver<String>) {
    use kelpie_rtc::peer::Event;
    let (mut open, mut hello) = (false, false);
    loop {
        tokio::select! {
            _ = stop.notified() => return,
            c = candidates.recv() => match c {
                Some(line) => peer.candidate(&line),
                None => return,
            },
            ev = peer.events.recv() => match ev {
                Some(Event::Open) => {
                    open = true;
                    if hello {
                        d.screen.ready(&d, &hold, id);
                    }
                }
                Some(Event::Input(data)) if !hello => {
                    let v: Value = serde_json::from_slice(&data).unwrap_or_default();
                    if v["type"] == "rtc_hello" && v["sc"].as_str() == Some(&hold.sc) && v["nonce"].as_str() == Some(&nonce) {
                        hello = true;
                        if open {
                            d.screen.ready(&d, &hold, id);
                        }
                    } else {
                        // Not this screen's peer: closed before anything goes on it.
                        tracing::warn!(sc = %hold.sc, "screen: a peer to peer hello that does not match: closed");
                        return d.screen.failed(&d, &hold, id, "bad_hello");
                    }
                }
                Some(Event::Input(data)) => d.screen.rtc_input(&d, &hold, &data),
                Some(Event::Stats(s)) => d.screen.rtc_stats(&d, &hold, id, &s),
                Some(Event::Failed(why)) => return d.screen.failed(&d, &hold, id, why),
                None => return d.screen.failed(&d, &hold, id, "closed"),
            },
        }
    }
}


/// One screen's TURN allowance (bytes): `KELPIE_TURN_TEST_CAP_KB` in a debug build or an e2e run, so a test reaches it in seconds.
fn turn_session_cap(d: &Daemon) -> u64 {
    let test = (cfg!(debug_assertions) || std::env::var("KELPIE_E2E").is_ok_and(|v| v == "1")).then(|| std::env::var("KELPIE_TURN_TEST_CAP_KB").ok()?.parse::<u64>().ok()).flatten();
    test.map_or(u64::from(d.mobile.screen_options().turn_session_mb) * 1024 * 1024, |kb| kb * 1024)
}