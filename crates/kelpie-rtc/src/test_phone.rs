//! A phone's side of the screen peer to peer, for tests (this crate's and the daemon's): a str0m offerer with the phone's three negotiated
//! channels, on 127.0.0.1, that sends what it is given on `"i"` and hands back what comes on `"m"` (put together) and `"c"`.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::channel::{ChannelConfig, ChannelId, Reliability};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event as RtcEvent, Input, Output, Rtc, RtcConfig};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::time::sleep_until;

use crate::{frame, CURSOR, INPUT, MEDIA};

/// The phone before the computer answered.
pub struct Offering {
    rtc: Rtc,
    sock: UdpSocket,
    local: SocketAddr,
    ids: [ChannelId; 3],
    pending: SdpPendingOffer,
}

/// The phone once answered: its channels, while it lives (dropped: it is gone, as a phone that left).
pub struct TestPhone {
    pub sent_i: mpsc::UnboundedSender<Vec<u8>>,
    pub media: mpsc::UnboundedReceiver<Vec<u8>>,
    pub cursor: mpsc::UnboundedReceiver<Vec<u8>>,
    pub opened: watch::Receiver<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestPhone {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn channel_configs() -> [ChannelConfig; 3] {
    [
        ChannelConfig { label: "m".into(), negotiated: Some(MEDIA), ..Default::default() },
        ChannelConfig { label: "c".into(), negotiated: Some(CURSOR), ordered: false, reliability: Reliability::MaxRetransmits { retransmits: 0 }, ..Default::default() },
        ChannelConfig { label: "i".into(), negotiated: Some(INPUT), ..Default::default() },
    ]
}

/// The phone's offer (an SDP), and the phone waiting for its answer.
pub async fn offer() -> (Offering, String) {
    #[cfg(windows)]
    crate::peer::crypto();
    let mut rtc = RtcConfig::new().build(Instant::now());
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let local = sock.local_addr().unwrap();
    rtc.add_local_candidate(Candidate::host(local, "udp").unwrap());
    let mut change = rtc.sdp_api();
    let ids = channel_configs().map(|c| change.add_channel_with_config(c));
    let (offer, pending) = change.apply().unwrap();
    (Offering { rtc, sock, local, ids, pending }, offer.to_sdp_string())
}

impl Offering {
    /// Takes the computer's answer, and runs.
    pub fn accept(mut self, answer: &str) -> TestPhone {
        self.rtc.sdp_api().accept_answer(self.pending, SdpAnswer::from_sdp_string(answer).unwrap()).unwrap();
        let (sent_i, mut to_send) = mpsc::unbounded_channel::<Vec<u8>>();
        let (media_tx, media) = mpsc::unbounded_channel();
        let (cursor_tx, cursor) = mpsc::unbounded_channel();
        let (open_tx, opened) = watch::channel(false);
        let (mut b, sock, local, ids) = (self.rtc, self.sock, self.local, self.ids);
        let task = tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            let mut open = [false; 3];
            let mut waiting: Vec<Vec<u8>> = Vec::new();
            let mut parts = frame::Reassembly::default();
            loop {
                // Everything out, after every packet in.
                let next = loop {
                    match b.poll_output() {
                        Ok(Output::Timeout(t)) => break t,
                        Ok(Output::Transmit(t)) => {
                            let _ = sock.send_to(&t.contents, t.destination).await;
                        }
                        Ok(Output::Event(RtcEvent::ChannelOpen(id, _))) => {
                            if let Some(k) = ids.iter().position(|x| *x == id) {
                                open[k] = true;
                            }
                            let _ = open_tx.send(open.iter().all(|o| *o));
                        }
                        Ok(Output::Event(RtcEvent::ChannelData(d))) if d.id == ids[0] => {
                            if let Some(whole) = parts.push(&d.data) {
                                let _ = media_tx.send(whole);
                            }
                        }
                        Ok(Output::Event(RtcEvent::ChannelData(d))) if d.id == ids[1] => {
                            let _ = cursor_tx.send(d.data);
                        }
                        Ok(_) => {}
                        Err(_) => return,
                    }
                };
                if open[2] {
                    while let Some(msg) = waiting.first() {
                        match b.channel(ids[2]).map(|mut ch| ch.write(true, msg)) {
                            Some(Ok(true)) => {
                                waiting.remove(0);
                            }
                            _ => break,
                        }
                    }
                }
                let wake = tokio::time::Instant::from_std(next.min(Instant::now() + Duration::from_millis(20)));
                tokio::select! {
                    got = sock.recv_from(&mut buf) => {
                        let Ok((n, from)) = got else { continue };
                        let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: from, destination: local, contents: buf[..n].try_into().unwrap() });
                        if b.handle_input(input).is_err() {
                            return;
                        }
                    }
                    _ = sleep_until(wake) => {
                        if b.handle_input(Input::Timeout(Instant::now())).is_err() {
                            return;
                        }
                    }
                    m = to_send.recv() => match m {
                        Some(m) => waiting.push(m),
                        None => return,
                    },
                }
            }
        });
        TestPhone { sent_i, media, cursor, opened, task }
    }
}
