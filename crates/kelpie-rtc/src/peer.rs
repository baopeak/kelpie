//! The computer's peer for one phone's screen: a str0m `Rtc` that answers the phone's offer, on one UDP socket, driven by one task.
//!
//! - **Candidates:** the socket is bound to one address, the one this computer reaches the internet from (so every packet's destination is
//!   known, and a peer's checks pair with the candidate they came to), offered as the host candidate, with the address the internet sees from
//!   Cloudflare's STUN server; never a Tailscale address (100.64.0.0/10), which a page through the relay is not told. The phone's candidates
//!   come in its offer and, late ones, through `Peer::candidate`; an address it reaches us from that it never named is learnt from its checks
//!   (mDNS-hidden ones on a phone's LAN).
//! - **DTLS:** the platform's own (SChannel); str0m refuses a peer whose certificate does not match the offer's fingerprint, and the offer
//!   reaches the daemon only inside the phone's end-to-end encrypted link.
//! - **One packet in, then everything it gave out** (`drain`): the DTLS layer holds only a few unread records, so feeding several packets
//!   before polling fails ("receive queue full").
//! - **Media** come whole on `Peer::media` (at most a few: the daemon's rule drops video when it is full, as for the link's media lane) and go
//!   out in chunks while `"m"` holds less than its budget; the **cursor** goes on `"c"` as its latest place only, never behind video; what
//!   comes on `"i"` goes to the daemon as `Event::Input`.
//! - **Failure** (`Event::Failed`): not open within the limit, ICE disconnected for 3 s, a channel or the association closed.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Once;
use std::time::{Duration, Instant};

use str0m::change::SdpOffer;
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event as RtcEvent, IceConnectionState, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::sleep_until;

use crate::frame;
use crate::impair::{Impair, Lane};
use crate::stats::{Kinds, Stats};
use crate::{stun, Latest, CURSOR, INPUT, MEDIA};

/// Media messages waiting for the peer: as many as the link's media lane holds (`kelpied::mobile::lanes`).
pub const MEDIA_QUEUE: usize = 12;
/// ICE disconnected this long is a failure.
const DISCONNECTED_FOR: Duration = Duration::from_secs(3);
/// The cursor is dropped rather than queued behind this much on its channel.
const CURSOR_ROOM: usize = 16 * 1024;
const STUN_WAIT: Duration = Duration::from_millis(600);

pub struct Config {
    /// The address the socket is bound to and offered as the host candidate: None for the one this computer reaches the internet from (a
    /// test gives the loopback).
    pub host: Option<IpAddr>,
    /// The STUN server asked for this computer's public address (`host:port`), if any.
    pub stun: Option<String>,
    pub impair: Option<Impair>,
    /// The most `"m"` may hold unsent (bytes).
    pub budget: usize,
    /// The channels must be open within this.
    pub open_limit: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config { host: test_host(), stun: Some(crate::STUN_SERVER.to_string()), impair: Impair::from_env(), budget: 256 * 1024, open_limit: Duration::from_secs(10) }
    }
}

#[derive(Debug, PartialEq)]
pub enum Event {
    /// The three channels are open.
    Open,
    /// A message on `"i"`.
    Input(Vec<u8>),
    Stats(Stats),
    /// The peer is gone, and why (`not_open`, `ice_disconnected`, `closed`, `error`).
    Failed(&'static str),
}

enum Cmd {
    Candidate(String),
}

/// The daemon's end of a running peer. Dropping it ends the peer.
pub struct Peer {
    /// Whole media messages (`0x01` first), sent on `"m"` in chunks.
    pub media: mpsc::Sender<Vec<u8>>,
    /// The cursor's latest place, sent on `"c"`.
    pub cursor: Latest,
    pub events: mpsc::Receiver<Event>,
    cmd: mpsc::UnboundedSender<Cmd>,
    /// The host candidate's address (the socket's own).
    pub local_addr: SocketAddr,
}

impl Peer {
    /// A late candidate of the phone's (an SDP `candidate:` line).
    pub fn candidate(&self, line: &str) {
        let _ = self.cmd.send(Cmd::Candidate(line.to_string()));
    }
}

pub(crate) fn crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| str0m::crypto::from_feature_flags().install_process_default());
}

/// `KELPIE_RTC_TEST_HOST`: the address a test's peers use (the loopback), in a debug build or an e2e run only, as the impairment.
fn test_host() -> Option<IpAddr> {
    let allowed = cfg!(debug_assertions) || std::env::var("KELPIE_E2E").is_ok_and(|v| v == "1");
    allowed.then(|| std::env::var("KELPIE_RTC_TEST_HOST").ok()?.parse().ok()).flatten()
}

/// Tailscale's addresses (100.64.0.0/10): never offered.
fn tailscale(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    a == 100 && (64..128).contains(&b)
}

/// The IPv4 address a phone could reach this computer at: the one its default route leaves from (a connected UDP socket sends nothing), else
/// the first of its interfaces' that is not loopback, link-local or Tailscale's.
fn host_ip() -> Option<IpAddr> {
    let usable = |ip: &Ipv4Addr| !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified() && !tailscale(*ip);
    let routed = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect("1.1.1.1:53").and_then(|()| s.local_addr())).ok().and_then(|a| match a.ip() {
        IpAddr::V4(v4) if usable(&v4) => Some(IpAddr::V4(v4)),
        _ => None,
    });
    routed.or_else(|| {
        if_addrs::get_if_addrs().unwrap_or_default().into_iter().find_map(|i| match i.ip() {
            IpAddr::V4(v4) if usable(&v4) => Some(IpAddr::V4(v4)),
            _ => None,
        })
    })
}
/// This socket's address as the internet sees it, from `server`; None when it does not answer in time.
pub async fn reflexive(socket: &UdpSocket, server: &str) -> Option<SocketAddr> {
    let to = tokio::time::timeout(STUN_WAIT, tokio::net::lookup_host(server)).await.ok()?.ok()?.find(SocketAddr::is_ipv4)?;
    let mut tid = [0u8; 12];
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos());
    for (i, b) in tid.iter_mut().enumerate() {
        *b = (seed >> ((i % 8) * 8)) as u8 ^ (i as u8).wrapping_mul(97);
    }
    socket.send_to(&stun::request(tid), to).await.ok()?;
    let mut buf = [0u8; 512];
    let end = tokio::time::Instant::now() + STUN_WAIT;
    loop {
        let (n, from) = tokio::time::timeout_at(end, socket.recv_from(&mut buf)).await.ok()?.ok()?;
        if from == to {
            if let Some(addr) = stun::mapped(&buf[..n], tid) {
                return Some(addr);
            }
        }
    }
}

/// Answers the phone's offer (`sdp`): the answer to send back, and the running peer.
pub async fn answer(sdp: &str, cfg: Config) -> Result<(String, Peer), String> {
    crypto();
    let offer = SdpOffer::from_sdp_string(sdp).map_err(|e| format!("not an offer: {e}"))?;
    let mut kinds = Kinds::default();
    kinds.add_sdp(sdp);
    let host = cfg.host.or_else(host_ip).ok_or("no network address to offer")?;
    let socket = UdpSocket::bind(SocketAddr::new(host, 0)).await.map_err(|e| format!("no UDP socket: {e}"))?;
    let base = socket.local_addr().map_err(|e| e.to_string())?;
    let mut rtc = RtcConfig::new().set_stats_interval(Some(Duration::from_secs(1))).build(Instant::now());
    rtc.add_local_candidate(Candidate::host(base, "udp").map_err(|e| format!("no host candidate: {e}"))?);
    if let Some(server) = cfg.stun.as_deref() {
        if let Some(public) = reflexive(&socket, server).await.filter(|a| a.ip() != host) {
            if let Ok(c) = Candidate::server_reflexive(public, base, "udp") {
                rtc.add_local_candidate(c);
            }
        }
    }
    let answer = rtc.sdp_api().accept_offer(offer).map_err(|e| format!("the offer was refused: {e}"))?;
    let mut api = rtc.direct_api();
    let m = api.create_data_channel(ChannelConfig { label: "m".into(), negotiated: Some(MEDIA), ..Default::default() });
    let c = api.create_data_channel(ChannelConfig { label: "c".into(), negotiated: Some(CURSOR), ordered: false, reliability: Reliability::MaxRetransmits { retransmits: 0 }, ..Default::default() });
    let i = api.create_data_channel(ChannelConfig { label: "i".into(), negotiated: Some(INPUT), ..Default::default() });
    let (media_tx, media_rx) = mpsc::channel(MEDIA_QUEUE);
    let (events_tx, events) = mpsc::channel(256);
    let (cmd, cmd_rx) = mpsc::unbounded_channel();
    let cursor = Latest::default();
    let seed = u64::from(base.port()) * 0x9E37_79B9;
    let run = Run {
        rtc,
        socket,
        base,
        ids: [m, c, i],
        open: [false; 3],
        told_open: false,
        started: Instant::now(),
        open_limit: cfg.open_limit,
        disconnected_at: None,
        budget: cfg.budget,
        pending: Vec::new(),
        next_seq: 0,
        full_since: None,
        kinds,
        impair: cfg.impair.map(|i| (Lane::new(i, seed), Lane::new(i, seed ^ 0xA5A5), Vec::new(), Vec::new())),
        events: events_tx,
        cursor: cursor.clone(),
    };
    tokio::spawn(run.go(media_rx, cmd_rx));
    Ok((answer.to_sdp_string(), Peer { media: media_tx, cursor, events, cmd, local_addr: base }))
}

/// What is waiting to go each way when the network is impaired: the send and receive lanes and their queues (when, where, what).
type Impaired = (Lane, Lane, Vec<(Instant, SocketAddr, Vec<u8>)>, Vec<(Instant, SocketAddr, Vec<u8>)>);

struct Run {
    rtc: Rtc,
    socket: UdpSocket,
    base: SocketAddr,
    ids: [ChannelId; 3],
    open: [bool; 3],
    told_open: bool,
    started: Instant,
    open_limit: Duration,
    disconnected_at: Option<Instant>,
    budget: usize,
    /// Chunks of the message being sent that did not fit yet.
    pending: Vec<Vec<u8>>,
    next_seq: u32,
    full_since: Option<Instant>,
    kinds: Kinds,
    impair: Option<Impaired>,
    events: mpsc::Sender<Event>,
    cursor: Latest,
}

impl Run {
    async fn go(mut self, mut media: mpsc::Receiver<Vec<u8>>, mut cmd: mpsc::UnboundedReceiver<Cmd>) {
        let mut buf = vec![0u8; 2048];
        let why = loop {
            let now = Instant::now();
            if let Err(why) = self.due(now).await {
                break why;
            }
            if let Some(why) = self.failed(now) {
                break why;
            }
            self.send_cursor();
            self.send_media();
            let next = match self.drain().await {
                Ok(t) => t,
                Err(why) => break why,
            };
            let room = self.told_open && self.pending.is_empty() && self.media_room();
            self.note_full(room);
            let wake = [Some(next), self.impair.as_ref().and_then(|q| q.2.iter().chain(q.3.iter()).map(|p| p.0).min()), Some(now + Duration::from_millis(250))].into_iter().flatten().min().unwrap_or(next);
            tokio::select! {
                got = self.socket.recv_from(&mut buf) => {
                    let Ok((n, from)) = got else { continue };
                    if let Some(q) = self.impair.as_mut() {
                        if let Some(at) = q.1.due(Instant::now(), n) {
                            q.3.push((at, from, buf[..n].to_vec()));
                        }
                    } else if let Err(why) = self.receive(from, &buf[..n]).await {
                        break why;
                    }
                }
                _ = sleep_until(tokio::time::Instant::from_std(wake)) => {
                    if self.rtc.handle_input(Input::Timeout(Instant::now())).is_err() {
                        break "error";
                    }
                }
                m = media.recv(), if room => match m {
                    Some(m) => {
                        self.pending = frame::chunks(self.next_seq, &m);
                        self.pending.reverse();
                        self.next_seq = self.next_seq.wrapping_add(1);
                    }
                    // The daemon let go of the peer.
                    None => break "closed",
                },
                _ = self.cursor.ready() => {}
                c = cmd.recv() => match c {
                    Some(Cmd::Candidate(line)) => {
                        let line = line.trim().trim_start_matches("a=");
                        self.kinds.add_line(line);
                        if let Ok(c) = Candidate::from_sdp_string(line) {
                            self.rtc.add_remote_candidate(c);
                        }
                    }
                    None => break "closed",
                },
            }
        };
        self.rtc.disconnect();
        let _ = self.events.try_send(Event::Failed(why));
        tracing::debug!(why, "screen peer ended");
    }

    /// The impaired queues: what is due goes now.
    async fn due(&mut self, now: Instant) -> Result<(), &'static str> {
        let Some(q) = self.impair.as_mut() else { return Ok(()) };
        let (out, inn): (Vec<_>, Vec<_>) = (q.2.extract_if(.., |p| p.0 <= now).collect(), q.3.extract_if(.., |p| p.0 <= now).collect());
        for (_, to, data) in out {
            let _ = self.socket.send_to(&data, to).await;
        }
        for (_, from, data) in inn {
            self.receive(from, &data).await?;
        }
        Ok(())
    }

    /// One packet in, then all it gave out.
    async fn receive(&mut self, from: SocketAddr, data: &[u8]) -> Result<(), &'static str> {
        let Ok(contents) = data.try_into() else { return Ok(()) };
        let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: from, destination: self.base, contents });
        if !self.rtc.accepts(&input) {
            return Ok(());
        }
        self.rtc.handle_input(input).map_err(|_| "error")?;
        self.drain().await.map(|_| ())
    }

    /// Everything the `Rtc` has to give out now: packets, events. Its next timeout.
    async fn drain(&mut self) -> Result<Instant, &'static str> {
        loop {
            match self.rtc.poll_output().map_err(|_| "error")? {
                Output::Timeout(t) => return Ok(t),
                Output::Transmit(t) => {
                    if let Some(q) = self.impair.as_mut() {
                        if let Some(at) = q.0.due(Instant::now(), t.contents.len()) {
                            q.2.push((at, t.destination, t.contents.to_vec()));
                        }
                    } else {
                        let _ = self.socket.send_to(&t.contents, t.destination).await;
                    }
                }
                Output::Event(e) => self.event(e)?,
            }
        }
    }

    fn event(&mut self, e: RtcEvent) -> Result<(), &'static str> {
        match e {
            RtcEvent::ChannelOpen(id, _) => {
                if let Some(k) = self.ids.iter().position(|x| *x == id) {
                    self.open[k] = true;
                }
                if !self.told_open && self.open.iter().all(|o| *o) {
                    self.told_open = true;
                    let _ = self.events.try_send(Event::Open);
                }
            }
            RtcEvent::ChannelClose(id) if self.ids.contains(&id) => return Err("closed"),
            RtcEvent::ChannelData(d) if d.id == self.ids[2] => {
                // Input is a few messages a second: one that finds the daemon's queue full is dropped, as a slow link's would be.
                let _ = self.events.try_send(Event::Input(d.data));
            }
            RtcEvent::IceConnectionStateChange(s) => match s {
                IceConnectionState::Disconnected => {
                    self.disconnected_at.get_or_insert_with(Instant::now);
                }
                IceConnectionState::Connected | IceConnectionState::Completed => self.disconnected_at = None,
                _ => {}
            },
            RtcEvent::PeerStats(s) => {
                let pair = s.selected_candidate_pair.as_ref();
                let local = pair.map_or("", |p| if p.local.addr.ip() == self.base.ip() { "host" } else { "srflx" });
                let remote = pair.map_or("", |p| self.kinds.of(p.remote.addr));
                let buffered = self.rtc.channel(self.ids[0]).map_or(0, |mut c| c.buffered_amount());
                let full_ms = self.full_since.map_or(0, |at| at.elapsed().as_millis() as u64);
                let rtt = pair.and_then(|p| p.current_round_trip_time).or(s.rtt).map(|d| d.as_millis().min(u128::from(u32::MAX)) as u32);
                let _ = self.events.try_send(Event::Stats(Stats { local, remote, rtt_ms: rtt, bytes_tx: s.peer_bytes_tx, bytes_rx: s.peer_bytes_rx, media_buffered: buffered, media_full_ms: full_ms }));
            }
            _ => {}
        }
        Ok(())
    }


    fn failed(&self, now: Instant) -> Option<&'static str> {
        if !self.rtc.is_alive() {
            return Some("closed");
        }
        if !self.told_open && now.duration_since(self.started) >= self.open_limit {
            return Some("not_open");
        }
        if self.disconnected_at.is_some_and(|at| now.duration_since(at) >= DISCONNECTED_FOR) {
            return Some("ice_disconnected");
        }
        None
    }

    fn media_room(&mut self) -> bool {
        let budget = self.budget;
        self.rtc.channel(self.ids[0]).is_some_and(|mut c| c.buffered_amount() < budget)
    }

    /// How long `"m"` has been at its budget, for the stats (the daemon's ceiling under loss reads it).
    fn note_full(&mut self, room: bool) {
        if room || !self.told_open {
            self.full_since = None;
        } else {
            self.full_since.get_or_insert_with(Instant::now);
        }
    }

    /// The chunks of the message under way, while `"m"` takes them.
    fn send_media(&mut self) {
        if !self.told_open {
            return;
        }
        let id = self.ids[0];
        while let Some(chunk) = self.pending.last() {
            let Some(mut ch) = self.rtc.channel(id) else { return };
            match ch.write(true, chunk) {
                Ok(true) => {
                    self.pending.pop();
                }
                _ => return,
            }
        }
    }

    fn send_cursor(&mut self) {
        if !self.told_open {
            return;
        }
        let Some(msg) = self.cursor.take() else { return };
        if let Some(mut ch) = self.rtc.channel(self.ids[1]) {
            if ch.buffered_amount() < CURSOR_ROOM {
                let _ = ch.write(true, &msg);
            }
        }
    }
}


#[cfg(test)]
mod tests;
