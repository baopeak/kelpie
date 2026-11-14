//! STUN binding (RFC 5389), just enough to learn this computer's address as the internet sees it: a request, and the mapped address out of
//! the answer. The peer adds it as its server-reflexive candidate, so a phone on another network can reach this computer through a NAT that
//! keeps the mapping (most home routers do).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

const MAGIC: u32 = 0x2112_A442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const HEADER: usize = 20;

/// A binding request with transaction id `tid`.
pub fn request(tid: [u8; 12]) -> Vec<u8> {
    let mut b = Vec::with_capacity(HEADER);
    b.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes());
    b.extend_from_slice(&MAGIC.to_be_bytes());
    b.extend_from_slice(&tid);
    b
}

/// The address a binding success answer for `tid` says the request came from; None for anything else (another message, another
/// transaction, a malformed one).
pub fn mapped(msg: &[u8], tid: [u8; 12]) -> Option<SocketAddr> {
    if msg.len() < HEADER || u16::from_be_bytes([msg[0], msg[1]]) != BINDING_SUCCESS || u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) != MAGIC || msg[8..20] != tid {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([msg[2], msg[3]]));
    let body = msg.get(HEADER..HEADER + len)?;
    let (mut at, mut plain) = (0, None);
    while at + 4 <= body.len() {
        let kind = u16::from_be_bytes([body[at], body[at + 1]]);
        let size = usize::from(u16::from_be_bytes([body[at + 2], body[at + 3]]));
        let value = body.get(at + 4..at + 4 + size)?;
        match kind {
            XOR_MAPPED_ADDRESS => return address(value, Some(&msg[4..20])),
            MAPPED_ADDRESS => plain = address(value, None),
            _ => {}
        }
        // Attributes are padded to four bytes.
        at += 4 + size.div_ceil(4) * 4;
    }
    plain
}

/// A (XOR-)MAPPED-ADDRESS value; `xor` is the cookie and transaction id the XOR form is masked with.
fn address(v: &[u8], xor: Option<&[u8]>) -> Option<SocketAddr> {
    let mask = |i: usize| xor.map_or(0, |x| x[i]);
    let port = u16::from_be_bytes([v.get(2)? ^ mask(0), v.get(3)? ^ mask(1)]);
    let ip = match v.get(1)? {
        0x01 => {
            let a = v.get(4..8)?;
            IpAddr::V4(Ipv4Addr::new(a[0] ^ mask(0), a[1] ^ mask(1), a[2] ^ mask(2), a[3] ^ mask(3)))
        }
        0x02 => {
            let a = v.get(4..20)?;
            let mut o = [0u8; 16];
            for (i, b) in a.iter().enumerate() {
                o[i] = b ^ mask(i);
            }
            IpAddr::V6(Ipv6Addr::from(o))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TID: [u8; 12] = [0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae];

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    /// RFC 5769 2.2: a response with an IPv4 XOR-MAPPED-ADDRESS (192.0.2.1:32853), with SOFTWARE, MESSAGE-INTEGRITY and FINGERPRINT around it.
    #[test]
    fn rfc5769_ipv4_response() {
        let msg = hex(
            "01 01 00 3c 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 0b 74 65 73 74 20 76 65 63 74 6f 72 20
             00 20 00 08 00 01 a1 47 e1 12 a6 43
             00 08 00 14 2b 91 f5 99 fd 9e 90 c3 8c 74 89 f9 2a f9 ba 53 f0 6b e7 d7
             80 28 00 04 c0 7d 4c 96",
        );
        assert_eq!(mapped(&msg, TID), Some("192.0.2.1:32853".parse().unwrap()));
        let mut other = TID;
        other[0] ^= 1;
        assert_eq!(mapped(&msg, other), None, "another transaction's answer is not ours");
        assert_eq!(mapped(&msg[..30], TID), None, "a cut message");
    }

    /// RFC 5769 2.3: the same with an IPv6 address (2001:db8:1234:5678:11:2233:4455:6677, port 32853).
    #[test]
    fn rfc5769_ipv6_response() {
        let msg = hex(
            "01 01 00 48 21 12 a4 42 b7 e7 a7 01 bc 34 d6 86 fa 87 df ae
             80 22 00 0b 74 65 73 74 20 76 65 63 74 6f 72 20
             00 20 00 14 00 02 a1 47 01 13 a9 fa a5 d3 f1 79 bc 25 f4 b5 be d2 b9 d9
             00 08 00 14 a3 82 95 4e 4b e6 7b f1 17 84 c9 7c 82 92 c2 75 bf e3 ed 41
             80 28 00 04 c8 fb 0b 4c",
        );
        assert_eq!(mapped(&msg, TID), Some("[2001:db8:1234:5678:11:2233:4455:6677]:32853".parse().unwrap()));
    }

    #[test]
    fn a_request_is_a_bare_binding_with_the_cookie() {
        let r = request(TID);
        assert_eq!(r.len(), 20);
        assert_eq!(&r[..8], &[0x00, 0x01, 0x00, 0x00, 0x21, 0x12, 0xa4, 0x42]);
        assert_eq!(&r[8..], &TID);
        assert_eq!(mapped(&r, TID), None, "a request is not an answer");
    }

    #[test]
    fn plain_mapped_address_when_no_xor_one() {
        let mut msg = vec![0x01, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xa4, 0x42];
        msg.extend_from_slice(&TID);
        msg.extend_from_slice(&[0x00, 0x01, 0x00, 0x08, 0x00, 0x01, 0x0d, 0x3d, 10, 0, 0, 7]);
        assert_eq!(mapped(&msg, TID), Some("10.0.0.7:3389".parse().unwrap()));
    }
}
