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

use super::lanes::{CursorSlot, SLOW_AFTER};
use super::store::{Device, Kind, PageKind};
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

/// How a connection reaches this computer, as the desktop names it: `relay`, `tailscale` (through Tailscale Serve, or from a Tailscale address), `local`
/// (from this machine itself) or `lan` (anything else: this network). Serve's listener is on loopback, so it is looked at before loopback is.
pub fn route_of(peer: &Peer) -> &'static str {
    if peer.via_relay() {
        "relay"
    } else if peer.through_serve || listen::tailscale(peer.ip()) {
        "tailscale"
    } else if peer.ip().is_loopback() {
        "local"
    } else {
        "lan"
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
    /// The screen's media lane and cursor slot: media taken only when no message waits, the cursor before it, both dropped while `locked`
    /// (nothing of them goes to a locked phone).
    Media(mpsc::Receiver<Vec<u8>>, CursorSlot, Arc<AtomicBool>),
    Close(u16, &'static str, oneshot::Sender<()>),
}

/// Control messages waiting for the writer (replies, the lock, heartbeats, a screen's state and stats), at most: a sender waits for room
/// (`SLOW_AFTER` at most) once they are there, which a phone that reads takes long to reach.
const WRITER_QUEUE: usize = 64;
/// Bulk messages waiting for the writer (the session list, chats): the connection's task moves bulk only while there is room, so it never
/// waits on it.
const BULK_QUEUE: usize = 8;

/// A carrier with an established kelpie-link session. **What comes in never waits for what goes out**: a writer task of its own sends, so
/// that a large frame of a screen on its way to a slow phone (a keyframe takes most of a second at 1.5 Mbps) does not hold back the phone's
/// clicks and acknowledgements, which the connection's task reads meanwhile. The writer seals each message as it sends it and takes, in this
/// order, control messages (`send`), bulk (`send_bulk`) and the screen's media, so a reply waits at most for the one message being written.
pub struct Wire {
    pub ws: CarrierRx,
    session: Arc<parking_lot::Mutex<kelpie_link::Session>>,
    out: mpsc::Sender<Outgoing>,
    bulk: mpsc::Sender<Vec<u8>>,
    writer: tokio::task::JoinHandle<()>,
    /// Resolves (with an error) once the writer has stopped: the connection is over, and its task hears it at once.
    pub dead: oneshot::Receiver<()>,
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
        let (bulk, bulk_queue) = mpsc::channel(BULK_QUEUE);
        let (alive, dead) = oneshot::channel();
        let writer = tokio::spawn(write(tx, session.clone(), queue, bulk_queue, alive));
        Wire { ws: rx, session, out, bulk, writer, dead, route_lost: false }
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

    /// Queues a bulk message, behind control messages; false when the connection is over. It never waits: the caller moves bulk only while
    /// `bulk_room` says there is room, and a message that finds none anyway is dropped (the bulk lane's own limit decides a phone is slow).
    pub fn send_bulk(&mut self, msg: &str) -> bool {
        !matches!(self.bulk.try_send(msg.as_bytes().to_vec()), Err(mpsc::error::TrySendError::Closed(_)))
    }

    /// Whether the writer's bulk queue has room now.
    pub fn bulk_room(&self) -> bool {
        self.bulk.capacity() > 0
    }

    /// Hands the screen's media lane and its cursor slot to the writer: the cursor behind control and bulk messages, media behind every other
    /// message, both dropped while `locked` is set.
    pub async fn attach_media(&mut self, media: mpsc::Receiver<Vec<u8>>, cursor: CursorSlot, locked: Arc<AtomicBool>) -> bool {
        matches!(tokio::time::timeout(SLOW_AFTER, self.out.send(Outgoing::Media(media, cursor, locked))).await, Ok(Ok(())))
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

/// The writer: control messages first, in order, then bulk, then the screen's cursor (its latest position), then the screen's media when nothing
/// else waits. Each message is sealed as it goes and must be on
/// the socket within `SLOW_AFTER`; a socket that takes longer, or is gone, ends the writer, and with it the connection.
async fn write(mut tx: CarrierTx, session: Arc<parking_lot::Mutex<kelpie_link::Session>>, mut queue: mpsc::Receiver<Outgoing>, mut bulk: mpsc::Receiver<Vec<u8>>, _alive: oneshot::Sender<()>) {
    // The screen's media lane and cursor slot, once attached, and whether the phone is locked (nothing of either goes then).
    let mut media: Option<mpsc::Receiver<Vec<u8>>> = None;
    let mut cursor: Option<CursorSlot> = None;
    let mut locked = Arc::new(AtomicBool::new(false));
    loop {
        let next = tokio::select! {
            biased;
            o = queue.recv() => match o {
                Some(o) => o,
                None => return,
            },
            Some(b) = bulk.recv() => Outgoing::Message(b),
            Some(c) = async { let slot = cursor.as_ref()?; slot.ready().await; slot.take() }, if cursor.is_some() => {
                if locked.load(Ordering::SeqCst) {
                    continue;
                }
                Outgoing::Message(c)
            }
            Some(m) = async { media.as_mut()?.recv().await }, if media.is_some() => {
                if locked.load(Ordering::SeqCst) {
                    continue;
                }
                Outgoing::Message(m)
            }
        };
        match next {
            Outgoing::Media(rx, slot, lock) => {
                media = Some(rx);
                cursor = Some(slot);
                locked = lock;
            }
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
    // The code is for one page, the one the desktop issued it for: from anywhere else it is a failed attempt, and stays live for that page.
    let Some(page) = m.code_page(&psk) else { return };
    if !code_fits(&page, &peer, m.relay_v2()) {
        failed(&daemon, peer.ip());
        close(&mut ws, BAD_CODE, "page").await;
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
    let Some((kind, page)) = spent else {
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
    let origin_kind = page.kind.name();
    m.add_pending(&id, &name, kind, remote, wire.session().pair_code(), want, peer.ip(), peer.secure_origin, page, tx);
    daemon.broadcast("mobile.pair_request", json!({"id": id, "name": name, "kind": kind.name(), "fingerprint": fingerprint, "want": want.name(), "address": listen::shown(peer.ip()), "origin_kind": origin_kind}));
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
/// Whether a phone may pair with a code issued for `page`, coming as `peer`. Over the relay its page must be the code's (the relay names it, from the
/// origins this computer registered: `relay::on_control`), and the hosted app's only on a relay that takes several origins; directly on `/link`, a code for
/// the hosted app is never used, as that page reaches this computer only through the relay.
pub(super) fn code_fits(page: &super::PageOrigin, peer: &Peer, relay_v2: bool) -> bool {
    let hosted = page.kind == PageKind::Hosted;
    if peer.via_relay() {
        peer.origin.as_deref() == Some(page.origin.as_str()) && (!hosted || relay_v2)
    } else {
        !hosted
    }
}

/// The page a device's connection through the relay goes by, or None when it may not come from where it does (`came`, the origin the relay named). A
/// device that recorded its page comes from there and nowhere else. One from before pages were recorded paired at the Tailscale origin the room lets in
/// (`room`), and goes by it as it always did; a relay that names another origin for it is refused.
pub(super) fn relay_page(device: &Device, came: Option<&str>, room: Option<&str>) -> Option<Option<String>> {
    match (device.page.as_deref(), came) {
        (Some(page), Some(came)) if page == came => Some(Some(page.to_string())),
        (Some(_), _) => None,
        (None, Some(came)) if Some(came) != room => None,
        (None, _) => Some(room.map(str::to_string)),
    }
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
    // And only from the page it paired on: the connection goes by that page from here on (its alerts, its passkey).
    let mut peer = peer;
    if peer.via_relay() {
        let room = m.relay_live().and_then(|r| r.origin);
        let Some(page) = relay_page(&device, peer.origin.as_deref(), room.as_deref()) else {
            close(&mut ws, RELAY_REFUSED, "page").await;
            return;
        };
        peer.secure_origin = page.is_some();
        peer.origin = page;
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
#[cfg(test)]
mod tests {
    use super::super::relay::{Inbound, Out, Slot};
    use super::*;

    const TS: &str = "https://desk.tail1234.ts.net:8443";
    const HOSTED: &str = "https://kelpie-link-relay.example.workers.dev";

    fn page(kind: PageKind, origin: &str) -> super::super::PageOrigin {
        super::super::PageOrigin { kind, origin: origin.into() }
    }

    fn relayed(o: Option<&str>) -> Peer {
        Peer::relayed(super::super::relay::slot_ip(1), o.map(str::to_string))
    }

    #[test]
    fn a_code_is_used_only_from_the_page_it_was_issued_for() {
        let hosted = page(PageKind::Hosted, HOSTED);
        let ts = page(PageKind::Ts, TS);
        assert!(code_fits(&hosted, &relayed(Some(HOSTED)), true));
        assert!(!code_fits(&hosted, &relayed(Some(HOSTED)), false), "a relay of the first version never carries the hosted app");
        assert!(!code_fits(&hosted, &relayed(Some(TS)), true), "a hosted code from the Tailscale page");
        assert!(!code_fits(&ts, &relayed(Some(HOSTED)), true), "a Tailscale code from the hosted page");
        assert!(code_fits(&ts, &relayed(Some(TS)), true) && code_fits(&ts, &relayed(Some(TS)), false), "as before, on either version");
        assert!(!code_fits(&ts, &relayed(None), true));
        let direct = Peer::direct("192.168.1.9:5000".parse().unwrap(), true);
        assert!(!code_fits(&hosted, &direct, true), "never on this computer's own /link");
        for kind in [PageKind::Ts, PageKind::Own, PageKind::Plain] {
            assert!(code_fits(&page(kind, "https://192.168.1.2:4817"), &direct, false), "{kind:?} on /link as before");
        }
    }

    #[test]
    fn each_way_a_phone_comes_in_is_named_for_the_desktop() {
        let direct = |addr: &str, tls: bool| route_of(&Peer::direct(addr.parse().unwrap(), tls));
        assert_eq!(direct("192.168.1.9:5000", false), "lan", "plain HTTP on this network");
        assert_eq!(direct("192.168.1.9:5000", true), "lan", "HTTPS on this network");
        assert_eq!(direct("100.101.102.103:5000", false), "tailscale", "a Tailscale address, straight to the listener");
        assert_eq!(direct("[fd7a:115c:a1e0::5]:5000", true), "tailscale");
        assert_eq!(direct("127.0.0.1:5000", false), "local");
        assert_eq!(direct("[::1]:5000", false), "local");
        let serve = Peer::behind_serve("127.0.0.1:5000".parse().unwrap());
        assert_eq!(route_of(&serve), "tailscale", "Tailscale Serve, though its listener is on loopback");
        let mut told = serve.clone();
        told.client = Some("100.101.102.103".parse().unwrap());
        assert_eq!(route_of(&told), "tailscale");
        assert_eq!(route_of(&relayed(Some(TS))), "relay");
        assert_eq!(route_of(&relayed(None)), "relay");
    }

    #[test]
    fn a_device_on_the_relay_goes_by_the_page_it_paired_on() {
        let mut d = Device { id: "d".into(), name: "n".into(), pk: String::new(), permission: super::super::Permission::Write, kind: Kind::Pwa, created_at: 0, last_seen: 0, mac: String::new(), secure_origin: true, secure_mac: String::new(), page: None, page_kind: None, page_mac: String::new(), push: None, lock: None, alerts: None, verified: true };
        // From before pages: the room's Tailscale origin, as always, and no other page the relay names.
        assert_eq!(relay_page(&d, Some(TS), Some(TS)), Some(Some(TS.to_string())));
        assert_eq!(relay_page(&d, None, Some(TS)), Some(Some(TS.to_string())), "a relay of the first version, or no Origin");
        assert_eq!(relay_page(&d, Some(HOSTED), Some(TS)), None, "the hosted page is not its");
        assert_eq!(relay_page(&d, None, None), Some(None));
        // With its page: from there only.
        d.page = Some(HOSTED.into());
        d.page_kind = Some(PageKind::Hosted);
        assert_eq!(relay_page(&d, Some(HOSTED), Some(TS)), Some(Some(HOSTED.to_string())));
        assert_eq!(relay_page(&d, Some(TS), Some(TS)), None);
        assert_eq!(relay_page(&d, None, Some(TS)), None);
    }

    /// The daemon's session and the phone's, after a reconnect handshake.
    fn sessions() -> (kelpie_link::Session, kelpie_link::Session) {
        let (desktop, phone) = (kelpie_link::Keypair::generate().unwrap(), kelpie_link::Keypair::generate().unwrap());
        let (init, first) = kelpie_link::Initiator::reconnect(&phone, &desktop.public).unwrap();
        let (ours, reply) = kelpie_link::read_first(&desktop, &first, &[]).unwrap().accept().unwrap();
        (ours, init.finish(&reply).unwrap())
    }

    /// What the phone gets, opened, as far as it has come.
    fn received(phone: &mut kelpie_link::Session, out: &mut mpsc::Receiver<Out>) -> Vec<Vec<u8>> {
        let mut got = Vec::new();
        while let Ok(Out::Frame(_, _, frame)) = out.try_recv() {
            if let Some(msg) = phone.open(&frame, now()).unwrap() {
                got.push(msg);
            }
        }
        got
    }

    #[tokio::test]
    async fn messages_go_before_media_none_to_a_locked_phone_and_what_the_phone_sends_is_read_while_the_writer_waits() {
        let (ours, mut phone) = sessions();
        let (frames_tx, frames_rx) = mpsc::channel::<Inbound>(8);
        // Room for one frame: the writer waits on a slow phone until the test takes what it wrote.
        let (out_tx, mut out_rx) = mpsc::channel::<Out>(1);
        let mut wire = Wire::new(Carrier::Relay(Slot::for_test(frames_rx, out_tx)), ours);
        let (media_tx, media_rx) = mpsc::channel(6);
        let locked = Arc::new(AtomicBool::new(false));
        media_tx.try_send(b"\x01first frame".to_vec()).unwrap();
        assert!(wire.attach_media(media_rx, CursorSlot::default(), locked.clone()).await);
        assert!(wire.send("one").await && wire.send("two").await);
        // The writer is held by the phone's slow side: the connection still reads what the phone sends meanwhile.
        tokio::time::sleep(Duration::from_millis(50)).await;
        for frame in phone.seal(br#"{"type":"sc_input"}"#).unwrap() {
            frames_tx.send(Ok(frame)).await.unwrap();
        }
        let msg = tokio::time::timeout(Duration::from_secs(1), wire.ws.recv()).await.expect("read while the writer waits").unwrap().unwrap();
        assert!(matches!(wire.open(msg), Opened::Text(t) if t.contains("sc_input")));
        // In order: the messages, then the media that waited behind them.
        let mut got = Vec::new();
        for _ in 0..50 {
            got.extend(received(&mut phone, &mut out_rx));
            if got.len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(got, [b"one".to_vec(), b"two".to_vec(), b"\x01first frame".to_vec()]);
        // A locked phone gets no media; messages still go.
        locked.store(true, Ordering::SeqCst);
        media_tx.try_send(b"\x01while locked".to_vec()).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(wire.send("three").await);
        let mut got = Vec::new();
        for _ in 0..50 {
            got.extend(received(&mut phone, &mut out_rx));
            if !got.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(got, [b"three".to_vec()]);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(received(&mut phone, &mut out_rx).is_empty(), "nothing of the media lane to a locked phone");
        // Closing goes after what was queued, and the writer is done.
        wire.close(1000, "").await;
        assert!(!wire.send("after").await, "a closed wire takes nothing more");
    }
}
#[cfg(test)]
mod queue_tests {
    use super::super::relay::{Inbound, Out, Slot};
    use super::*;

    fn sessions() -> (kelpie_link::Session, kelpie_link::Session) {
        let (desktop, phone) = (kelpie_link::Keypair::generate().unwrap(), kelpie_link::Keypair::generate().unwrap());
        let (init, first) = kelpie_link::Initiator::reconnect(&phone, &desktop.public).unwrap();
        let (ours, reply) = kelpie_link::read_first(&desktop, &first, &[]).unwrap().accept().unwrap();
        (ours, init.finish(&reply).unwrap())
    }

    #[tokio::test]
    async fn a_reply_goes_before_bulk_behind_a_large_keyframe_and_the_connection_never_waits_on_bulk() {
        let (ours, mut phone) = sessions();
        let (_frames_tx, frames_rx) = mpsc::channel::<Inbound>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<Out>(1);
        let mut wire = Wire::new(Carrier::Relay(Slot::for_test(frames_rx, out_tx)), ours);
        let (media_tx, media_rx) = mpsc::channel(6);
        let mut keyframe = vec![0x01u8];
        keyframe.resize(300_000, 7);
        media_tx.try_send(keyframe.clone()).unwrap();
        assert!(wire.attach_media(media_rx, CursorSlot::default(), Arc::new(AtomicBool::new(false))).await);
        // The writer starts on the keyframe and waits on the slow phone, part of it written.
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Bulk fills its queue without the connection ever waiting on it; a reply goes on the control queue.
        let mut taken = 0;
        while wire.bulk_room() {
            assert!(wire.send_bulk(&format!("bulk {taken}")));
            taken += 1;
        }
        assert_eq!(taken, BULK_QUEUE, "room is said, and kept to");
        assert!(wire.send("reply").await);
        let mut got = Vec::new();
        for _ in 0..200 {
            while let Ok(Out::Frame(_, _, frame)) = out_rx.try_recv() {
                if let Some(msg) = phone.open(&frame, now()).unwrap() {
                    got.push(msg);
                }
            }
            if got.len() >= 2 + BULK_QUEUE {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(got[0], keyframe, "the message being written is finished first");
        assert_eq!(got[1], b"reply".to_vec(), "then the reply, ahead of every bulk message");
        assert_eq!(got[2], b"bulk 0".to_vec());
    }

    #[tokio::test]
    async fn the_cursor_goes_ahead_of_the_video_queued_before_it_and_only_its_latest_place() {
        let (ours, mut phone) = sessions();
        let (_frames_tx, frames_rx) = mpsc::channel::<Inbound>(8);
        let (out_tx, mut out_rx) = mpsc::channel::<Out>(1);
        let mut wire = Wire::new(Carrier::Relay(Slot::for_test(frames_rx, out_tx)), ours);
        let (media_tx, media_rx) = mpsc::channel(12);
        // A large frame being written, and six more behind it.
        let mut first = vec![0x01u8, 1];
        first.resize(200_000, 7);
        media_tx.try_send(first.clone()).unwrap();
        for i in 0..6u8 {
            media_tx.try_send(vec![0x01, 1, i]).unwrap();
        }
        let cursor = CursorSlot::default();
        assert!(wire.attach_media(media_rx, cursor.clone(), Arc::new(AtomicBool::new(false))).await);
        tokio::time::sleep(Duration::from_millis(30)).await;
        // Two places while the frames wait: only the newer goes, and before every frame not yet begun.
        cursor.put(vec![0x01, 3, 0]);
        cursor.put(vec![0x01, 3, 1]);
        let mut got = Vec::new();
        for _ in 0..200 {
            while let Ok(Out::Frame(_, _, frame)) = out_rx.try_recv() {
                if let Some(msg) = phone.open(&frame, now()).unwrap() {
                    got.push(msg);
                }
            }
            if got.len() >= 8 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(got[0], first, "the frame being written is finished first");
        assert_eq!(got[1], vec![0x01, 3, 1], "then the cursor's latest place, ahead of the frames queued before it");
        assert_eq!(got[2..].to_vec(), (0..6u8).map(|i| vec![0x01, 1, i]).collect::<Vec<_>>(), "and no older place after them");
    }

    #[tokio::test]
    async fn the_connection_hears_at_once_that_the_writer_stopped() {
        let (ours, _phone) = sessions();
        let (_frames_tx, frames_rx) = mpsc::channel::<Inbound>(8);
        let (out_tx, out_rx) = mpsc::channel::<Out>(1);
        let mut wire = Wire::new(Carrier::Relay(Slot::for_test(frames_rx, out_tx)), ours);
        // The relay's side is gone: the next write fails and the writer stops.
        drop(out_rx);
        assert!(wire.send("anything").await);
        assert!(tokio::time::timeout(Duration::from_secs(1), &mut wire.dead).await.is_ok(), "heard without another send");
        assert!(!wire.send("after").await, "and nothing more is taken");
    }
}