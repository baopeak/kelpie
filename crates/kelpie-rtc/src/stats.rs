//! What a peer says about itself once a second: the way its packets take (the selected pair's candidate types), the round trip, the bytes,
//! and how long its media channel has been over its budget. The remote candidate's type is read from the candidates the phone gave (its
//! offer, then `sc_rtc_ice`): one the phone never named was learnt from its checks (peer-reflexive).

use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    /// `host`, `srflx`, `prflx` or `relay`; empty before a pair is chosen.
    pub local: &'static str,
    pub remote: &'static str,
    pub rtt_ms: Option<u32>,
    /// Bytes sent and received on the wire since the peer started.
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    /// Bytes waiting on the media channel, and how long it has stayed at its budget or over it (0: it is not).
    pub media_buffered: usize,
    pub media_full_ms: u64,
}

impl Stats {
    /// The way, as the phone and the daemon's log say it: `p2p` for a direct or NAT-mapped pair, `turn` through a TURN server.
    pub fn path(&self) -> &'static str {
        if self.remote == "relay" || self.local == "relay" {
            "turn"
        } else {
            "p2p"
        }
    }
}

/// The type of each candidate address the phone named.
#[derive(Default)]
pub struct Kinds(HashMap<SocketAddr, &'static str>);

impl Kinds {
    /// An SDP candidate line (`candidate:… typ relay …`, with or without `a=`).
    pub fn add_line(&mut self, line: &str) {
        let line = line.trim().trim_start_matches("a=");
        let Some(rest) = line.strip_prefix("candidate:") else { return };
        let f: Vec<&str> = rest.split_whitespace().collect();
        // foundation component transport priority address port "typ" type
        let (Some(ip), Some(port), Some(typ)) = (f.get(4), f.get(5).and_then(|p| p.parse::<u16>().ok()), f.iter().position(|w| *w == "typ").and_then(|i| f.get(i + 1))) else { return };
        let Ok(ip) = ip.parse() else { return };
        self.0.insert(SocketAddr::new(ip, port), word(typ));
    }

    /// Every candidate line of an SDP.
    pub fn add_sdp(&mut self, sdp: &str) {
        for line in sdp.lines().filter(|l| l.trim_start().starts_with("a=candidate:")) {
            self.add_line(line);
        }
    }

    /// The type of the phone's address `addr`: one it never named was learnt from its checks.
    pub fn of(&self, addr: SocketAddr) -> &'static str {
        self.0.get(&addr).copied().unwrap_or("prflx")
    }
}

fn word(typ: &str) -> &'static str {
    match typ {
        "host" => "host",
        "srflx" => "srflx",
        "relay" => "relay",
        _ => "prflx",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_the_phones_lines() {
        let mut k = Kinds::default();
        k.add_sdp("v=0\r\na=candidate:1 1 udp 2122260223 192.168.1.20 50000 typ host generation 0\r\na=candidate:2 1 udp 1686052607 203.0.113.9 61000 typ srflx raddr 192.168.1.20 rport 50000\r\na=candidate:3 1 udp 41885439 104.30.0.5 3478 typ relay raddr 203.0.113.9 rport 61000\r\na=candidate:4 1 udp 2122260223 abcd.local 50001 typ host\r\n");
        k.add_line("candidate:5 1 udp 2122260223 10.0.0.3 4000 typ host");
        assert_eq!(k.of("192.168.1.20:50000".parse().unwrap()), "host");
        assert_eq!(k.of("203.0.113.9:61000".parse().unwrap()), "srflx");
        assert_eq!(k.of("104.30.0.5:3478".parse().unwrap()), "relay");
        assert_eq!(k.of("10.0.0.3:4000".parse().unwrap()), "host", "a trickled one");
        assert_eq!(k.of("198.51.100.1:9".parse().unwrap()), "prflx", "an mDNS name or an address learnt from checks");
        assert_eq!(Stats { remote: "relay", ..Stats::default() }.path(), "turn");
        assert_eq!(Stats { remote: "prflx", local: "host", ..Stats::default() }.path(), "p2p");
    }
}
