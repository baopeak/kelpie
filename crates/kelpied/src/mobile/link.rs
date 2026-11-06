//! `/link`: a phone's WebSocket carrying kelpie-link (see the `kelpie-link` crate), or one of the relay's slots (`relay`), which carries the
//! same messages and is handled the same way. Binary messages only; the first one is the phone's half of a handshake, and nothing readable
//! passes before the second.
//!
//! What a first message costs is bounded before it costs anything: a connection counts against the handshakes pending (16, the
//! oldest giving way to a newcomer; 4 from one address) from its upgrade until its first valid sealed frame or 5 s, it carries exactly one handshake, its `mac1` is checked before
//! any Diffie-Hellman (`kelpie_link::prefilter`), and only then does it take one of the 20 handshakes a second, 5 of them for one
//! address. Upgrades are limited per address before any of this. Whatever fails one of these is dropped with no reply and no close
//! code. What holds a place to no end counts against its address (`held_silently`): a connection that said nothing to the end or was let
//! go, one that repeats a first message seen before, and one that passed the check and never sent a sealed frame. A flood goes in the
//! audit log (`Mobile::note_flood`). Once a new first message has passed the check a connection keeps its place, whoever comes.
//!
//! A first message may be a replay, so **nothing happens until the phone's first valid sealed frame**, which only the holder of
//! the handshake's keys can make:
//! - **Pairing** (`MODE_PAIR`, Noise IKpsk1): a code answers, the frame carries the phone's name and what it asks for, and only
//!   then is the code spent and the person asked: the phone shows six digits derived from the handshake, which the person
//!   types into Kelpie (`mobile.approve`, the verified app only), and the daemon asks the OS to verify the person. The phone
//!   hears `paired`, `declined` or `timeout` over the encrypted channel, and connects again as a paired device.
//! - **Reconnecting** (`MODE_RECONNECT`, Noise IK): the phone's static key must be a paired device's (a record whose MAC checks
//!   out); anything else is closed without a reply. The frame is its `{caps}`; only then does it count as seen, take over from
//!   its older connection, and count against its 5 handshakes in 10 s.
//! - **Afterwards**: [`Wire`] seals each JSON message of the phone protocol (and a screen's binary media) into binary messages and opens what comes in; a
//!   message that does not decrypt, or chunks that stop coming, end the connection (both ends then handshake again). It sends from a task of its own,
//!   so what the phone sends is read while a large message is on its way to it.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use axum::http::{header, HeaderMap};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::lanes::SLOW_AFTER;
use super::store::Kind;
use super::{listen, webauthn, Generation, HandshakeSlot, KeyLookup, CLOSE_TIMEOUT, CODE_TTL, NOT_PAIRED, RECONNECT, UNCONFIRMED};
use crate::session::now_millis as now;
use crate::daemon::Daemon;

/// How long either half of a handshake may take.
pub const HANDSHAKE: Duration = Duration::from_millis(kelpie_link::HANDSHAKE_TIMEOUT_MS);
/// Close codes before a session exists: the pairing code is not (any longer) valid; too many attempts; the machine is busy.
pub const BAD_CODE: u16 = 4403;
pub const TOO_MANY: u16 = 4429;
pub const BUSY: u16 = 4503;
/// Over the relay: this device may not use it (it paired over plain HTTP, or does not answer heartbeats). It may still connect directly.
pub const RELAY_REFUSED: u16 = 4412;
/// From the phone: it closes this socket because another of its routes (the relay, or its own address) finished its handshake first.
pub const ROUTE_LOST: u16 = 4000;
/// The phone's connection details, from the listener that accepted it.
#[derive(Clone, Debug)]
pub struct Peer {
    pub addr: SocketAddr,
    /// The phone reached us over HTTPS: this listener's own, or Tailscale Serve's in front of the loopback listener.
    pub tls: bool,
    /// Accepted by the loopback listener Tailscale Serve forwards to.
    pub through_serve: bool,
    /// Who is calling through serve, from the header serve sets.
    pub client: Option<IpAddr>,
    /// A page whose origin is this computer's HTTPS origin, at a name: a page at a number is not, as it can never have a passkey, and a record that says it was would hold that phone at
    /// the lock for good. A program on this machine can claim it through the loopback listener, and then still needs the pairing secret and the person, so it records how a person
    /// paired, and is no proof against that program.
    pub secure_origin: bool,
    /// The `Origin` of the upgrade: where the browser says the page is. The route has already checked it against `Host`, and on a TLS listener the browser has checked the certificate for that name. Only this device's own alerts go by it.
    pub origin: Option<String>,
}

impl Peer {
    /// A connection accepted directly from the network (`tls`: by a listener with a certificate).
    pub fn direct(addr: SocketAddr, tls: bool) -> Peer {
        Peer { addr, tls, through_serve: false, client: None, secure_origin: false, origin: None }
    }

    /// A connection accepted by the loopback listener behind Tailscale Serve.
    pub fn behind_serve(addr: SocketAddr) -> Peer {
        Peer { addr, tls: true, through_serve: true, client: None, secure_origin: false, origin: None }
    }

    /// A phone on one of the relay's slots. Its address is the one `relay::phone_ip` gives, which no network address can be, and its page is at the origin the
    /// room lets phones in from, which a browser cannot lie about. The relay could, and still has no session without the phone's keys.
    pub fn relayed(ip: IpAddr, origin: Option<String>) -> Peer {
        debug_assert!(super::relay::is_slot_ip(ip));
        Peer { addr: SocketAddr::new(ip, 0), tls: false, through_serve: false, client: None, secure_origin: origin.is_some(), origin }
    }

    /// It came through the relay.
    pub fn via_relay(&self) -> bool {
        !self.through_serve && super::relay::is_slot_ip(self.addr.ip())
    }

    /// The address the limits and the desktop go by. Never this machine for what comes through serve: every phone behind it
    /// would look local and be held to no limit. Serve's traffic with no word of who sent it shares one bucket.
    pub fn ip(&self) -> IpAddr {
        if self.through_serve {
            self.client.unwrap_or(listen::BEHIND_SERVE)
        } else {
            self.addr.ip().to_canonical()
        }
    }

    /// With what the upgrade's headers say, `ts_origin` being the origin Tailscale publishes now. `X-Forwarded-For` counts on the loopback listener alone:
    /// anywhere else anyone can send it. There it is as good as the programs on this machine, which can connect to that port and say what they like: that buys
    /// them another limiter bucket, and no session.
    pub fn with_headers(mut self, headers: &HeaderMap, ts_origin: Option<&str>) -> Peer {
        let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok());
        self.origin = origin.map(str::to_string);
        if self.through_serve {
            self.client = forwarded_client(headers);
            self.secure_origin = ts_origin.is_some_and(|ts| origin.and_then(|o| o.strip_prefix("https://")) == Some(ts));
        } else {
            self.secure_origin = self.tls && origin.and_then(webauthn::rp_id).is_some();
        }
        self
    }
}

/// The address serve put in `X-Forwarded-For`: the last one, nearest to us, and only a Tailscale address, which is all a tailnet peer
/// can be. Should serve ever pass on what a phone sent, the worst a phone can do is pick another tailnet address, not pass for this machine.
fn forwarded_client(headers: &HeaderMap) -> Option<IpAddr> {
    let last = headers.get_all("x-forwarded-for").iter().next_back()?.to_str().ok()?;
    let ip: IpAddr = last.rsplit(',').next()?.trim().parse().ok()?;
    let ip = ip.to_canonical();
    listen::tailscale(ip).then_some(ip)
}

/// What carries a phone's messages: its own WebSocket on `/link`, or a slot of the relay's (`relay`). Both give and take what a WebSocket does, so everything
/// above them is the same code.
// One per phone connection, made once and never moved about in bulk: its size is no cost worth a box.
#[allow(clippy::large_enum_variant)]
pub enum Carrier {
    Socket(WebSocket),
    Relay(super::relay::Slot),
}

impl Carrier {
    /// The next message; None once the carrier is gone.
    pub async fn recv(&mut self) -> Option<Result<Message, axum::Error>> {
        match self {
            Carrier::Socket(ws) => ws.recv().await,
            // The phone's close code comes in the relay's `close`, so a route that lost the race says so here too.
            Carrier::Relay(slot) => slot.recv().await.map(|got| {
                Ok(match got {
                    Ok(frame) => Message::Binary(frame.into()),
                    Err(code) => Message::Close(Some(CloseFrame { code, reason: "".into() })),
                })
            }),
        }
    }

    pub async fn send(&mut self, msg: Message) -> Result<(), axum::Error> {
        match self {
            Carrier::Socket(ws) => ws.send(msg).await,
            Carrier::Relay(slot) => match msg {
                Message::Binary(frame) => {
                    if slot.send(frame.to_vec()).await {
                        Ok(())
                    } else {
                        Err(axum::Error::new(std::io::Error::other("the relay connection is gone")))
                    }
                }
                Message::Close(frame) => {
                    let (code, reason) = frame.map_or((1000, String::new()), |f| (f.code, f.reason.as_str().to_string()));
                    slot.close(code, reason).await;
                    Ok(())
                }
                // The relay keeps its own connection to the phone alive.
                _ => Ok(()),
            },
        }
    }
}

/// What a carrier gives once its session is established: the phone's messages. Its other half is the writer's (`Wire`).
pub enum CarrierRx {
    Socket(SplitStream<WebSocket>),
    Relay(super::relay::Slot),
}

impl CarrierRx {
    /// The next message; None once the carrier is gone.
    pub async fn recv(&mut self) -> Option<Result<Message, axum::Error>> {
        match self {
            CarrierRx::Socket(ws) => ws.next().await,
            CarrierRx::Relay(slot) => slot.recv().await.map(|got| {
                Ok(match got {
                    Ok(frame) => Message::Binary(frame.into()),
                    Err(code) => Message::Close(Some(CloseFrame { code, reason: "".into() })),
                })
            }),
        }
    }
}

/// The half of a carrier the writer sends on.
enum CarrierTx {
    Socket(SplitSink<WebSocket, Message>),
    Relay(super::relay::SlotSender),
}

impl CarrierTx {
    async fn send(&mut self, msg: Message) -> Result<(), axum::Error> {
        match self {
            CarrierTx::Socket(ws) => ws.send(msg).await,
            CarrierTx::Relay(slot) => match msg {
                Message::Binary(frame) => {
                    if slot.send(frame.to_vec()).await {
                        Ok(())
                    } else {
                        Err(axum::Error::new(std::io::Error::other("the relay connection is gone")))
                    }
                }
                Message::Close(frame) => {
                    let (code, reason) = frame.map_or((1000, String::new()), |f| (f.code, f.reason.as_str().to_string()));
                    slot.close(code, reason).await;
                    Ok(())
                }
                _ => Ok(()),
            },
        }
    }
}

/// What the writer is given, in the order it is to go out.
enum Outgoing {
    /// A whole message, sealed by the writer as it goes (so that the session's numbering is the socket's order).
    Message(Vec<u8>),
    /// The screen's media lane: taken only when no message waits, and dropped while `locked` (nothing of it goes to a locked phone).
    Media(mpsc::Receiver<Vec<u8>>, Arc<AtomicBool>),
    Close(u16, &'static str, oneshot::Sender<()>),
}

/// Messages waiting for the writer, at most: a sender waits for room (`SLOW_AFTER` at most) once they are there.
const WRITER_QUEUE: usize = 8;

/// A carrier with an established kelpie-link session. **What comes in never waits for what goes out**: a writer task of its own sends, so
/// that a large frame of a screen on its way to a slow phone (a keyframe takes most of a second at 1.5 Mbps) does not hold back the phone's
/// clicks and acknowledgements, which the connection's task reads meanwhile. The writer seals each message as it sends it and takes the
/// screen's media only when no other message waits, so a message waits at most for the one media message being written.
pub struct Wire {
    pub ws: CarrierRx,
    session: Arc<parking_lot::Mutex<kelpie_link::Session>>,
    out: mpsc::Sender<Outgoing>,
    writer: tokio::task::JoinHandle<()>,
    /// The phone closed it saying another route won the race (`ROUTE_LOST`).
    pub route_lost: bool,
}

/// What a message from the phone turned out to be.
pub enum Opened {
    /// A whole application message.
    Text(String),
    /// A chunk of one, or a control frame.
    Nothing,
    /// The connection is over (closed, not decrypting, not this protocol).
    End,
}

impl Drop for Wire {
    /// The connection goes with it, as the socket did before the writer had one half of it.
    fn drop(&mut self) {
        self.writer.abort();
    }
}

impl Wire {
    pub fn new(ws: Carrier, session: kelpie_link::Session) -> Wire {
        let (rx, tx) = match ws {
            Carrier::Socket(ws) => {
                let (sink, stream) = ws.split();
                (CarrierRx::Socket(stream), CarrierTx::Socket(sink))
            }
            Carrier::Relay(slot) => {
                let sender = slot.sender();
                (CarrierRx::Relay(slot), CarrierTx::Relay(sender))
            }
        };
        let session = Arc::new(parking_lot::Mutex::new(session));
        let (out, queue) = mpsc::channel(WRITER_QUEUE);
        let writer = tokio::spawn(write(tx, session.clone(), queue));
        Wire { ws: rx, session, out, writer, route_lost: false }
    }

    /// The session, for what is not sending or opening (its pairing code, padding, reassembly).
    pub fn session(&self) -> parking_lot::MutexGuard<'_, kelpie_link::Session> {
        self.session.lock()
    }

    /// Queues one message; false when the connection is over (the writer stopped: the socket is gone, or a message took longer than
    /// `SLOW_AFTER` to go), or no room came within `SLOW_AFTER`. A message too long to seal is reported and dropped by the writer, and the
    /// session carries on; any other failure to seal tells the phone to connect again.
    pub async fn send(&mut self, text: &str) -> bool {
        self.send_bytes(text.as_bytes()).await
    }

    /// A binary message: a screen's media (`screen`) starts with a byte no JSON message starts with (`kelpie_screen::wire::MEDIA`).
    pub async fn send_bytes(&mut self, msg: &[u8]) -> bool {
        matches!(tokio::time::timeout(SLOW_AFTER, self.out.send(Outgoing::Message(msg.to_vec()))).await, Ok(Ok(())))
    }

    /// Hands the screen's media lane to the writer: sent behind every other message, and dropped while `locked` is set.
    pub async fn attach_media(&mut self, media: mpsc::Receiver<Vec<u8>>, locked: Arc<AtomicBool>) -> bool {
        matches!(tokio::time::timeout(SLOW_AFTER, self.out.send(Outgoing::Media(media, locked))).await, Ok(Ok(())))
    }

    /// Opens one message from the socket.
    pub fn open(&mut self, msg: Message) -> Opened {
        match msg {
            Message::Binary(b) => match self.session.lock().open(&b, now()) {
                Ok(Some(bytes)) => String::from_utf8(bytes).map_or(Opened::End, Opened::Text),
                Ok(None) => Opened::Nothing,
                Err(_) => Opened::End,
            },
            Message::Ping(_) | Message::Pong(_) => Opened::Nothing,
            Message::Close(frame) => {
                self.route_lost = frame.is_some_and(|f| f.code == ROUTE_LOST);
                Opened::End
            }
            // Plain text has no business here: someone is not speaking kelpie-link.
            Message::Text(_) => Opened::End,
        }
    }

    /// The next whole message within `limit`.
    pub async fn recv_text(&mut self, limit: Duration) -> Option<String> {
        let end = tokio::time::Instant::now() + limit;
        loop {
            let msg = tokio::time::timeout_at(end, self.ws.recv()).await.ok()??.ok()?;
            match self.open(msg) {
                Opened::Text(t) => return Some(t),
                Opened::Nothing => continue,
                Opened::End => return None,
            }
        }
    }

    /// Ends the connection, after what was queued before. A close frame is not authenticated, so what tells a phone it was removed is the
    /// sealed message before it: only that makes the phone forget its pairing. A writer that cannot finish within `CLOSE_TIMEOUT` is stopped,
    /// which drops the connection.
    pub async fn close(&mut self, code: u16, reason: &'static str) {
        let end = tokio::time::Instant::now() + CLOSE_TIMEOUT;
        if code == NOT_PAIRED {
            let _ = tokio::time::timeout_at(end, self.out.send(Outgoing::Message(br#"{"type":"revoked"}"#.to_vec()))).await;
        }
        let (done, closed) = oneshot::channel();
        if matches!(tokio::time::timeout_at(end, self.out.send(Outgoing::Close(code, reason, done))).await, Ok(Ok(()))) {
            let _ = tokio::time::timeout_at(end, closed).await;
        }
        self.writer.abort();
    }
}

/// The writer: messages first, in order, then the screen's media when nothing else waits. Each message is sealed as it goes and must be on
/// the socket within `SLOW_AFTER`; a socket that takes longer, or is gone, ends the writer, and with it the connection.
async fn write(mut tx: CarrierTx, session: Arc<parking_lot::Mutex<kelpie_link::Session>>, mut queue: mpsc::Receiver<Outgoing>) {
    let mut media: Option<(mpsc::Receiver<Vec<u8>>, Arc<AtomicBool>)> = None;
    loop {
        let next = tokio::select! {
            biased;
            o = queue.recv() => match o {
                Some(o) => o,
                None => return,
            },
            Some(m) = async { media.as_mut()?.0.recv().await }, if media.is_some() => {
                if media.as_ref().is_some_and(|(_, locked)| locked.load(Ordering::SeqCst)) {
                    continue;
                }
                Outgoing::Message(m)
            }
        };
        match next {
            Outgoing::Media(rx, locked) => media = Some((rx, locked)),
            Outgoing::Close(code, reason, done) => {
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, tx.send(Message::Close(Some(CloseFrame { code, reason: reason.into() })))).await;
                let _ = done.send(());
                return;
            }
            Outgoing::Message(msg) => {
                let sealed = session.lock().seal(&msg);
                let frames = match sealed {
                    Ok(frames) => frames,
                    Err(kelpie_link::LinkError::TooLarge) => {
                        tracing::warn!("phone link: dropped a message of {} bytes, too long for a phone", msg.len());
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("phone link: could not seal a message: {e}");
                        let _ = tokio::time::timeout(CLOSE_TIMEOUT, tx.send(Message::Close(Some(CloseFrame { code: RECONNECT, reason: "seal".into() })))).await;
                        return;
                    }
                };
                let sent = tokio::time::timeout(SLOW_AFTER, async {
                    for f in frames {
                        tx.send(Message::Binary(f.into())).await?;
                    }
                    Ok::<(), axum::Error>(())
                })
                .await;
                if !matches!(sent, Ok(Ok(()))) {
                    return;
                }
            }
        }
    }
}

pub async fn close(ws: &mut Carrier, code: u16, reason: &'static str) {
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, ws.send(Message::Close(Some(CloseFrame { code, reason: reason.into() })))).await;
}

/// What the pairing phone calls itself (its first sealed message).
fn hello_name(v: &Value) -> String {
    let name: String = v["name"].as_str().unwrap_or_default().trim().chars().filter(|c| !c.is_control()).take(40).collect();
    if name.is_empty() {
        "Phone".into()
    } else {
        name
    }
}

/// A phone's connection, from its first message on.
pub async fn accept(daemon: Arc<Daemon>, mut ws: Carrier, generation: Arc<Generation>, peer: Peer, browser: bool) {
    let m = &daemon.mobile;
    let ip = peer.ip();
    // Too many handshakes under way, in all or from this address: dropped, unanswered.
    let Some(slot) = m.handshake_slot(ip) else { return };
    if m.pairing_blocked(ip) {
        close(&mut ws, TOO_MANY, "too_many").await;
        return;
    }
    let first = tokio::select! {
        biased;
        _ = generation.cancelled() => return,
        _ = slot.given_way() => return held_silently(&daemon, ip),
        first = tokio::time::timeout(HANDSHAKE, ws.recv()) => match first {
            Ok(Some(Ok(Message::Binary(b)))) => b,
            // A connection that held its place and said nothing counts against its address, unlike garbage (which holds no
            // place for long): an address that keeps doing it is locked out, and the desktop hears of it.
            Err(_) => return held_silently(&daemon, ip),
            _ => return,
        },
    };
    if generation.is_cancelled() {
        return;
    }
    let key = match m.link_key() {
        Ok(k) => k,
        Err(e) => {
            tracing::warn!("phone access: {e:#}");
            close(&mut ws, 1011, "").await;
            return;
        }
    };
    let live = m.live_codes();
    let psks: Vec<[u8; 32]> = live.iter().map(|(psk, _)| *psk).collect();
    // A hash, before any Diffie-Hellman: garbage, another version, a code that is not live. Dropped with no reply, no event.
    let Ok(filtered) = kelpie_link::prefilter(&key.public, &first, &psks) else { return };
    // Seen before, it is someone replaying what they saw: it earns no place. A phone's own retry is a new message (a new ephemeral key).
    if m.first_message_seen(&first) {
        m.note_flood("a first message seen before", ip);
        return held_silently(&daemon, ip);
    }
    slot.answered();
    match filtered.psk_index.map(|i| psks[i]) {
        Some(psk) => pair(daemon, ws, generation, peer, browser, &key, &first, psks, psk, slot).await,
        None => reconnect(daemon, ws, generation, peer, browser, &key, &first, slot).await,
    }
}

/// A connection held a place among the handshakes for nothing: it said nothing, or nothing new.
pub fn held_silently(daemon: &Daemon, ip: IpAddr) {
    if !daemon.mobile.this_machine(ip) {
        failed(daemon, ip);
    }
}

/// A connection that got the reply and then said nothing sealed counts against its address, unless the phone closed it saying another of
/// its routes won the race (`app/phone/src/link.ts`). Through the relay the code comes in the relay's `close` (`relay::on_control`).
fn silent_after_reply(daemon: &Daemon, ip: IpAddr, wire: &Wire) {
    if !wire.route_lost {
        held_silently(daemon, ip);
    }
}

/// A failed pairing or handshake from `ip`; the desktop hears when the address is locked out.
fn failed(daemon: &Daemon, ip: IpAddr) {
    if daemon.mobile.pairing_failed(ip) {
        let who = listen::shown(ip);
        daemon.broadcast("mobile.lockout", json!({"reason": "attempts", "address": who}));
        daemon.mobile.audit("-", "lockout", &format!("too many failed attempts from {who}"));
    }
}

#[allow(clippy::too_many_arguments)]
async fn pair(daemon: Arc<Daemon>, mut ws: Carrier, generation: Arc<Generation>, peer: Peer, browser: bool, key: &kelpie_link::Keypair, first: &[u8], psks: Vec<[u8; 32]>, psk: [u8; 32], slot: HandshakeSlot) {
    let m = &daemon.mobile;
    // Over the relay only a page at the origin the room lets in may pair: what pairs there is a device the relay may serve.
    if peer.via_relay() && !peer.secure_origin {
        return;
    }
    // Kelpie is short of CPU or memory: one handshake at a time, spaced out. The code is not spent, and it says so: the phone
    // proved it holds the code.
    if m.pairing_paced() {
        close(&mut ws, BUSY, "busy").await;
        return;
    }
    // Its share of the machine, then the Diffie-Hellman. A first message can be a replay: a code answers one address a few
    // times at most, and nothing else is done on its account.
    if !m.handshake_allowed(peer.ip()) || !m.code_attempt(&psk, peer.ip()) {
        return;
    }
    let opened = match kelpie_link::read_first(key, first, &psks) {
        Ok(o) => o,
        Err(_) => {
            failed(&daemon, peer.ip());
            close(&mut ws, BAD_CODE, "code").await;
            return;
        }
    };
    let remote = opened.remote;
    let Ok((session, reply)) = opened.accept() else { return };
    if tokio::time::timeout(HANDSHAKE, ws.send(Message::Binary(reply.into()))).await.is_err() {
        return;
    }
    let mut wire = Wire::new(ws, session);
    // The phone's first sealed message proves it finished this handshake (a replayed first message cannot make one): only
    // now is the code spent and the person asked.
    let said = tokio::select! {
        biased;
        _ = generation.cancelled() => return,
        said = wire.recv_text(HANDSHAKE) => said.and_then(|t| serde_json::from_str::<Value>(&t).ok()),
    };
    drop(slot);
    let Some(said) = said.filter(|v| v["type"] == "pair") else { return silent_after_reply(&daemon, peer.ip(), &wire) };
    // Single use: spent now, whatever happens next. Another connection may have spent it a moment ago.
    let spent = m.spend_code(&psk);
    m.relay_pair_spent(&psk);
    let Some(kind) = spent else {
        wire.send(&json!({"type": "declined", "reason": "code"}).to_string()).await;
        wire.close(1000, "").await;
        return;
    };
    // A page in a browser is held to what a page may do, whatever code it used.
    let kind = if browser { Kind::Pwa } else { kind };
    let name = hello_name(&said);
    let want = said["want"].as_str().and_then(super::Permission::parse).unwrap_or(super::Permission::Read).min(kind.ceiling());
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    let fingerprint = super::fingerprint(&remote);
    m.add_pending(&id, &name, kind, remote, wire.session().pair_code(), want, peer.ip(), peer.secure_origin, tx);
    daemon.broadcast("mobile.pair_request", json!({"id": id, "name": name, "kind": kind.name(), "fingerprint": fingerprint, "want": want.name(), "address": listen::shown(peer.ip())}));
    // The person types the phone's code on the desktop. Meanwhile the phone may only wait (or go away).
    let decision = tokio::select! {
        biased;
        _ = generation.cancelled() => None,
        d = tokio::time::timeout(CODE_TTL, rx) => Some(d),
        _ = gone(&mut wire.ws) => None,
    };
    m.remove_pending(&id);
    daemon.broadcast("mobile.updated", json!({}));
    let told = match decision {
        // A phone paired on a secure origin gets its relay token now: the next time it connects, the relay may be its only way.
        Some(Ok(Ok(Some(device)))) => {
            let relay = m.relay_grant(&device, &json!({"caps": ["relay"]})).and_then(|g| serde_json::from_str::<Value>(&g).ok());
            json!({"type": "paired", "device": {"id": device.id, "name": device.name, "permission": device.permission.name(), "kind": device.kind.name()}, "relay": relay})
        }
        Some(Ok(Ok(None))) => json!({"type": "declined"}),
        Some(_) => json!({"type": "timeout"}),
        None => return,
    };
    wire.send(&told.to_string()).await;
    wire.close(1000, "").await;
}
/// Resolves when the phone closes its side (anything it sends meanwhile is ignored: it has nothing to say yet).
async fn gone(ws: &mut CarrierRx) {
    while let Some(Ok(msg)) = ws.recv().await {
        if matches!(msg, Message::Close(_)) {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn reconnect(daemon: Arc<Daemon>, mut ws: Carrier, generation: Arc<Generation>, peer: Peer, browser: bool, key: &kelpie_link::Keypair, first: &[u8], slot: HandshakeSlot) {
    let m = &daemon.mobile;
    if !m.handshake_allowed(peer.ip()) {
        return;
    }
    let opened = kelpie_link::read_first(key, first, &[]).ok();
    // Only a paired device's key is answered; for anything else the phone learns only that it is not paired here, or (a record
    // that cannot be trusted now) that the computer cannot confirm it. That is no failed attempt: such a phone tries again.
    let known = opened.as_ref().map(|o| m.lookup_key(&o.remote));
    let (opened, device) = match (opened, known) {
        (Some(opened), Some(KeyLookup::Paired(device))) => (opened, *device),
        (_, Some(KeyLookup::Unconfirmed)) => {
            close(&mut ws, UNCONFIRMED, "").await;
            return;
        }
        _ => {
            failed(&daemon, peer.ip());
            close(&mut ws, NOT_PAIRED, "").await;
            return;
        }
    };
    // The relay serves only a device that paired on a secure origin: a page that could have been swapped on the way has no business there.
    if peer.via_relay() && !device.secure_origin {
        close(&mut ws, RELAY_REFUSED, "relay").await;
        return;
    }
    let Ok((session, reply)) = opened.accept() else { return };
    if tokio::time::timeout(HANDSHAKE, ws.send(Message::Binary(reply.into()))).await.is_err() {
        return;
    }
    let mut wire = Wire::new(ws, session);
    // The phone's first sealed message says what it can do (`caps`), and proves it finished this handshake: until then nothing
    // about the device changes. Cancellation covers it too, including an upgrade completed before disable.
    let said = tokio::select! {
        biased;
        _ = generation.cancelled() => return,
        said = wire.recv_text(HANDSHAKE) => said.and_then(|t| serde_json::from_str::<Value>(&t).ok()),
    };
    drop(slot);
    if generation.is_cancelled() {
        return;
    }
    let Some(said) = said else { return silent_after_reply(&daemon, peer.ip(), &wire) };
    // The relay keeps its own connection to the phone alive, so only the heartbeats inside the session would find a phone gone.
    if peer.via_relay() && !super::acknowledges(Some(&said)) {
        wire.close(RELAY_REFUSED, "caps").await;
        return;
    }
    if !m.reconnect_allowed(&device.id) {
        wire.close(TOO_MANY, "too_many").await;
        return;
    }
    super::client(daemon, wire, generation, device, peer, browser, said).await;
}