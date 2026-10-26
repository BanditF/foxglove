//! E1.31 (sACN, ANSI E1.31-2018) data packets: just enough to read one universe's DMX
//! channels. Our own small parser; the GPL OpenRGB client crates stay out of this repo.

/// The UDP port E1.31 uses.
pub const PORT: u16 = 5568;

const ACN_ID: &[u8; 12] = b"ASC-E1.17\0\0\0";
const VECTOR_ROOT_DATA: u32 = 0x0000_0004;
const VECTOR_FRAMING_DATA: u32 = 0x0000_0002;
const VECTOR_DMP_SET_PROPERTY: u8 = 0x02;
/// Options bit: the sender is stopping this stream.
const STREAM_TERMINATED: u8 = 0x40;
/// Options bit: preview data, not for output.
const PREVIEW: u8 = 0x80;
/// Offset of the DMX start code; channels follow it.
const DATA: usize = 126;

#[derive(Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    /// The sender's id (its CID): sequence numbers count per sender.
    pub cid: [u8; 16],
    pub universe: u16,
    pub sequence: u8,
    pub terminated: bool,
    pub preview: bool,
    /// DMX channels 1.. (start code 0 only; others aren't level data).
    pub channels: &'a [u8],
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The data packet in `b`, or `None` for anything else (sync and discovery packets, other
/// start codes, short or malformed packets).
pub fn parse(b: &[u8]) -> Option<Packet<'_>> {
    if b.len() < DATA + 1
        || u16_at(b, 0) != 0x0010
        || u16_at(b, 2) != 0
        || &b[4..16] != ACN_ID
        || u32_at(b, 18) != VECTOR_ROOT_DATA
        || u32_at(b, 40) != VECTOR_FRAMING_DATA
        || b[117] != VECTOR_DMP_SET_PROPERTY
        || b[118] != 0xa1
        || b[DATA] != 0
    {
        return None;
    }
    // Property value count includes the start code.
    let count = usize::from(u16_at(b, 123));
    if count == 0 || DATA + count > b.len() {
        return None;
    }
    Some(Packet {
        cid: b[22..38].try_into().expect("16 bytes"),
        universe: u16_at(b, 113),
        sequence: b[111],
        terminated: b[112] & STREAM_TERMINATED != 0,
        preview: b[112] & PREVIEW != 0,
        channels: &b[DATA + 1..DATA + count],
    })
}

/// E1.31 6.7.2: a packet whose sequence is 1-20 behind the last one is stale.
pub fn is_stale(last: u8, now: u8) -> bool {
    let diff = now.wrapping_sub(last) as i8;
    (-20..=0).contains(&diff)
}

/// A data packet for `channels` on `universe`, as a sender builds it (for tests and the
/// `--send-test` option).
pub fn build(cid: [u8; 16], universe: u16, sequence: u8, options: u8, channels: &[u8]) -> Vec<u8> {
    let count = channels.len() + 1;
    let total = DATA + count;
    let mut b = vec![0u8; total];
    let flags_len = |len: usize| (0x7000 | len as u16).to_be_bytes();
    b[0..2].copy_from_slice(&0x0010u16.to_be_bytes());
    b[4..16].copy_from_slice(ACN_ID);
    b[16..18].copy_from_slice(&flags_len(total - 16));
    b[18..22].copy_from_slice(&VECTOR_ROOT_DATA.to_be_bytes());
    b[22..38].copy_from_slice(&cid);
    b[38..40].copy_from_slice(&flags_len(total - 38));
    b[40..44].copy_from_slice(&VECTOR_FRAMING_DATA.to_be_bytes());
    b[44..52].copy_from_slice(b"foxglove");
    b[108] = 100;
    b[111] = sequence;
    b[112] = options;
    b[113..115].copy_from_slice(&universe.to_be_bytes());
    b[115..117].copy_from_slice(&flags_len(total - 115));
    b[117] = VECTOR_DMP_SET_PROPERTY;
    b[118] = 0xa1;
    b[121..123].copy_from_slice(&1u16.to_be_bytes());
    b[123..125].copy_from_slice(&(count as u16).to_be_bytes());
    b[DATA + 1..].copy_from_slice(channels);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const CID: &[u8; 16] = b"foxglove-testcid";

    #[test]
    fn parses_what_a_sender_builds() {
        let b = build(*CID, 3, 9, 0, &[10, 20, 30, 40]);
        assert_eq!(
            parse(&b),
            Some(Packet {
                cid: *CID,
                universe: 3,
                sequence: 9,
                terminated: false,
                preview: false,
                channels: &[10, 20, 30, 40]
            })
        );
        let b = build(*CID, 1, 0, STREAM_TERMINATED, &[0; 3]);
        let p = parse(&b).unwrap();
        assert!(p.terminated);
    }

    #[test]
    fn rejects_other_packets() {
        let mut b = build(*CID, 1, 0, 0, &[1, 2, 3]);
        assert!(parse(&b[..DATA]).is_none());
        b[DATA] = 0xdd; // per-channel priority start code
        assert!(parse(&b).is_none());
        let mut b = build(*CID, 1, 0, 0, &[1, 2, 3]);
        b[43] = 1; // sync packet vector
        assert!(parse(&b).is_none());
        let mut b = build(*CID, 1, 0, 0, &[1, 2, 3]);
        b[123..125].copy_from_slice(&600u16.to_be_bytes()); // count past the end
        assert!(parse(&b).is_none());
    }

    #[test]
    fn stale_sequence() {
        assert!(is_stale(10, 10));
        assert!(is_stale(10, 0));
        assert!(!is_stale(10, 11));
        assert!(!is_stale(255, 0)); // wraps forward
        assert!(!is_stale(10, 200));
    }
}
