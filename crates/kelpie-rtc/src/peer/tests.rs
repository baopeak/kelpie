//! Two str0m peers in one process over 127.0.0.1: the daemon's answerer (`answer`) and a test offerer standing in for the phone, with the
//! phone's three negotiated channels.

use super::*;

use crate::test_phone::{self, TestPhone};

fn local_config(impair: Option<Impair>, open_limit: Duration) -> Config {
    Config { host: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), stun: None, impair, budget: 256 * 1024, open_limit }
}

/// Makes the phone's offer; `tamper` may change it before the daemon sees it (a fingerprint that is not the phone's).
async fn connect(impair: Option<Impair>, open_limit: Duration, tamper: impl FnOnce(String) -> String) -> (Peer, TestPhone) {
    let (offering, offer) = test_phone::offer().await;
    let (ans, peer) = answer(&tamper(offer), local_config(impair, open_limit)).await.unwrap();
    (peer, offering.accept(&ans))
}
async fn next_event(peer: &mut Peer, within: Duration) -> Option<Event> {
    loop {
        match tokio::time::timeout(within, peer.events.recv()).await.ok()? {
            Some(Event::Stats(_)) => continue,
            other => return other,
        }
    }
}

fn message(n: u8, len: usize) -> Vec<u8> {
    let mut m = vec![n; len];
    m[0] = 0x01;
    m
}

async fn exchange(impair: Option<Impair>) {
    let (mut peer, mut phone) = connect(impair, Duration::from_secs(10), |s| s).await;
    assert_eq!(next_event(&mut peer, Duration::from_secs(10)).await, Some(Event::Open), "the three channels open");
    phone.opened.wait_for(|o| *o).await.unwrap();
    // The phone's first message on "i", then acknowledgements: each reaches the daemon in order.
    for msg in [&b"{\"type\":\"rtc_hello\"}"[..], b"{\"type\":\"sc_ack\",\"seq\":1}"] {
        phone.sent_i.send(msg.to_vec()).unwrap();
    }
    assert_eq!(next_event(&mut peer, Duration::from_secs(5)).await, Some(Event::Input(b"{\"type\":\"rtc_hello\"}".to_vec())));
    assert_eq!(next_event(&mut peer, Duration::from_secs(5)).await, Some(Event::Input(b"{\"type\":\"sc_ack\",\"seq\":1}".to_vec())));
    // Media, small and larger than a chunk, come whole and in order.
    let sent: Vec<Vec<u8>> = vec![message(1, 2000), message(2, 150_000), message(3, 10), message(4, 70_000)];
    for m in &sent {
        peer.media.send(m.clone()).await.unwrap();
    }
    for m in &sent {
        let got = tokio::time::timeout(Duration::from_secs(10), phone.media.recv()).await.expect("media in time").unwrap();
        assert_eq!(got.len(), m.len());
        assert_eq!(&got, m);
    }
    // The cursor: its latest place, on its own channel.
    peer.cursor.put(vec![0x01, 3, 0, 1]);
    let c = tokio::time::timeout(Duration::from_secs(5), phone.cursor.recv()).await;
    if impair.is_none_or(|i| i.loss == 0.0) {
        assert_eq!(c.unwrap().unwrap(), vec![0x01, 3, 0, 1]);
    }
}

#[tokio::test]
async fn the_three_channels_carry_media_cursor_and_input() {
    exchange(None).await;
}

#[tokio::test]
async fn a_lossy_slow_network_still_delivers_media_whole() {
    exchange(Some(Impair { loss: 0.01, delay: Duration::from_millis(20), kbps: 0 })).await;
}

/// The daemon polls after every packet: a burst of input from the phone arrives whole, none lost to the DTLS layer's short queue.
#[tokio::test]
async fn a_burst_of_input_arrives_whole() {
    let (mut peer, phone) = connect(None, Duration::from_secs(10), |s| s).await;
    assert_eq!(next_event(&mut peer, Duration::from_secs(10)).await, Some(Event::Open));
    let mut opened = phone.opened.clone();
    opened.wait_for(|o| *o).await.unwrap();
    for n in 0..300u32 {
        phone.sent_i.send(format!("{{\"n\":{n},\"pad\":\"{}\"}}", "x".repeat(600)).into_bytes()).unwrap();
    }
    for n in 0..300u32 {
        match next_event(&mut peer, Duration::from_secs(5)).await {
            Some(Event::Input(d)) => assert!(String::from_utf8(d).unwrap().starts_with(&format!("{{\"n\":{n},")), "in order"),
            other => panic!("message {n}: {other:?}"),
        }
    }
}

/// An answer to an offer whose fingerprint is not the phone's certificate: DTLS never completes, no channel opens, and the peer gives up.
#[tokio::test]
async fn a_peer_with_another_certificate_never_gets_a_channel() {
    let swap = |sdp: String| {
        sdp.lines()
            .map(|l| {
                if let Some(rest) = l.strip_prefix("a=fingerprint:sha-256 ") {
                    // Another certificate's fingerprint: every byte changed.
                    let other: Vec<String> = rest.split(':').map(|b| format!("{:02X}", u8::from_str_radix(b, 16).unwrap() ^ 0x5A)).collect();
                    format!("a=fingerprint:sha-256 {}", other.join(":"))
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\r\n")
            + "\r\n"
    };
    let (mut peer, phone) = connect(None, Duration::from_secs(4), swap).await;
    match next_event(&mut peer, Duration::from_secs(8)).await {
        Some(Event::Failed(_)) => {}
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(!*phone.opened.borrow(), "the phone's channels never opened");
}

/// The public address comes from a STUN server's answer on the peer's own socket.
#[tokio::test]
async fn the_public_address_comes_from_stun() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let at = server.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 512];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!(n, 20);
        // XOR-MAPPED-ADDRESS 198.51.100.7:40000.
        let mut answer = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
        answer.extend_from_slice(&buf[8..20]);
        let port = 40000u16 ^ 0x2112;
        let ip = [198u8 ^ 0x21, 51 ^ 0x12, 100 ^ 0xa4, 7 ^ 0x42];
        answer.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
        answer.extend_from_slice(&port.to_be_bytes());
        answer.extend_from_slice(&ip);
        server.send_to(&answer, from).await.unwrap();
    });
    let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    assert_eq!(reflexive(&sock, &at.to_string()).await, Some("198.51.100.7:40000".parse().unwrap()));
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let nobody = silent.local_addr().unwrap();
    assert_eq!(reflexive(&sock, &nobody.to_string()).await, None, "no answer in time");
}

#[test]
fn tailscale_addresses_are_never_offered() {
    assert!(tailscale(Ipv4Addr::new(100, 64, 0, 1)) && tailscale(Ipv4Addr::new(100, 127, 255, 254)));
    assert!(!tailscale(Ipv4Addr::new(100, 128, 0, 1)) && !tailscale(Ipv4Addr::new(192, 168, 1, 71)));
    if let Some(IpAddr::V4(ip)) = host_ip() {
        assert!(!tailscale(ip) && !ip.is_loopback(), "{ip}");
    }
}