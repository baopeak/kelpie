//! Media messages on the `"m"` channel, cut into chunks every browser takes whole (16 KiB): each chunk is `[0x02, msg_seq u32 LE, idx u16
//! LE, count u16 LE]` and a piece of the message. The phone puts them back together and reads the result as it reads a media message on the
//! link (`0x01` first). The channel is reliable and ordered, so the chunks of a message come in order; a message whose chunks were cut short
//! by a newer one is dropped whole.

/// The first byte of a chunk.
pub const CHUNK: u8 = 0x02;
/// The most bytes a chunk takes, its header included.
pub const MAX_CHUNK: usize = 16 * 1024;
pub const HEADER: usize = 9;

/// The chunks of message `seq`.
pub fn chunks(seq: u32, msg: &[u8]) -> Vec<Vec<u8>> {
    let room = MAX_CHUNK - HEADER;
    let count = msg.len().div_ceil(room).max(1);
    let count16 = u16::try_from(count).unwrap_or(u16::MAX);
    (0..count)
        .map(|i| {
            let part = &msg[(i * room).min(msg.len())..((i + 1) * room).min(msg.len())];
            let mut c = Vec::with_capacity(HEADER + part.len());
            c.push(CHUNK);
            c.extend_from_slice(&seq.to_le_bytes());
            c.extend_from_slice(&(i as u16).to_le_bytes());
            c.extend_from_slice(&count16.to_le_bytes());
            c.extend_from_slice(part);
            c
        })
        .collect()
}

/// Puts chunks back into messages.
#[derive(Default)]
pub struct Reassembly {
    seq: Option<u32>,
    count: u16,
    next: u16,
    buf: Vec<u8>,
}

impl Reassembly {
    /// A chunk: the whole message once its last chunk came. A chunk out of place (a newer message started, or one missing) drops what was
    /// gathered.
    pub fn push(&mut self, c: &[u8]) -> Option<Vec<u8>> {
        if c.len() < HEADER || c[0] != CHUNK {
            return None;
        }
        let seq = u32::from_le_bytes([c[1], c[2], c[3], c[4]]);
        let idx = u16::from_le_bytes([c[5], c[6]]);
        let count = u16::from_le_bytes([c[7], c[8]]);
        if count == 0 || idx >= count {
            return None;
        }
        if idx == 0 {
            self.seq = Some(seq);
            self.count = count;
            self.next = 0;
            self.buf.clear();
        } else if self.seq != Some(seq) || self.count != count || self.next != idx {
            self.seq = None;
            self.buf.clear();
            return None;
        }
        self.buf.extend_from_slice(&c[HEADER..]);
        self.next = idx + 1;
        if self.next == count {
            self.seq = None;
            return Some(std::mem::take(&mut self.buf));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn every_size_from_one_byte_to_a_megabyte_comes_back_whole() {
        let mut r = Reassembly::default();
        for (n, len) in [1usize, 2, MAX_CHUNK - HEADER - 1, MAX_CHUNK - HEADER, MAX_CHUNK - HEADER + 1, MAX_CHUNK, 3 * MAX_CHUNK, 100_000, 1000 * 1024].into_iter().enumerate() {
            let msg = message(len);
            let parts = chunks(n as u32, &msg);
            assert!(parts.iter().all(|c| c.len() <= MAX_CHUNK && c[0] == CHUNK), "{len}");
            let mut whole = None;
            for (i, c) in parts.iter().enumerate() {
                let got = r.push(c);
                assert_eq!(got.is_some(), i + 1 == parts.len(), "{len}: only the last chunk completes it");
                whole = whole.or(got);
            }
            assert_eq!(whole.unwrap(), msg, "{len}");
        }
    }

    #[test]
    fn a_message_cut_short_by_a_newer_one_is_dropped_whole() {
        let mut r = Reassembly::default();
        let old = chunks(1, &message(40_000));
        let new = chunks(2, &message(20_000));
        assert!(r.push(&old[0]).is_none());
        // The newer message starts before the old one ends: the old is gone, the newer comes whole.
        assert!(r.push(&new[0]).is_none());
        assert!(r.push(&old[1]).is_none(), "a chunk of the old message after the newer started");
        assert!(r.push(&new[1]).is_none(), "the newer lost its place too: a chunk was out of order");
        let again = chunks(3, &message(20_000));
        assert_eq!(r.push(&again[0]), None);
        assert_eq!(r.push(&again[1]).unwrap(), message(20_000));
    }

    #[test]
    fn junk_is_ignored() {
        let mut r = Reassembly::default();
        assert_eq!(r.push(&[]), None);
        assert_eq!(r.push(&[0x01, 0, 0, 0, 0, 0, 0, 1, 0]), None, "a link media message is not a chunk");
        assert_eq!(r.push(&[CHUNK, 0, 0, 0, 0, 0, 0, 0, 0]), None, "no chunks");
        assert_eq!(r.push(&[CHUNK, 0, 0, 0, 0, 2, 0, 1, 0]), None, "an index past the count");
    }
}
