//! Panel stream frames on a direct channel (§5.4, §8.4).
//!
//! On the wire: `u32 LE length` + payload. Payload: a 12-byte header (`u8 kind`, `u8 flags = 0`,
//! `u16 width_or_count`, `u16 height`, `u16 reserved = 0`, `u32 seq`, little-endian) + body.
//! A frame whose body doesn't match its header is dropped.

use crate::panel::MAX_FRAME_BYTES;

pub const HEADER_BYTES: usize = 12;

/// One decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// Kind 1: `width` × `height` RGBA pixels.
    Image {
        seq: u32,
        width: u16,
        height: u16,
        rgba: Vec<[u8; 4]>,
    },
    /// Kind 2: one RGBA colour per shape, in shape order.
    Canvas { seq: u32, colours: Vec<[u8; 4]> },
    /// Kind 3: UTF-8 text to append. `width_or_count` is its byte length (spec v1.1).
    Text { seq: u32, append: String },
}

/// Why a frame was dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dropped(pub String);

fn drop<T>(why: impl Into<String>) -> Result<T, Dropped> {
    Err(Dropped(why.into()))
}

/// Decodes one payload (without the length prefix).
pub fn decode_payload(p: &[u8]) -> Result<Frame, Dropped> {
    if p.len() < HEADER_BYTES {
        return drop("shorter than the header");
    }
    if p.len() > MAX_FRAME_BYTES {
        return drop("over the frame size cap");
    }
    let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
    let (kind, flags, count, height, reserved) = (p[0], p[1], u16_at(2), u16_at(4), u16_at(6));
    let seq = u32::from_le_bytes([p[8], p[9], p[10], p[11]]);
    if flags != 0 || reserved != 0 {
        return drop("flags and reserved are 0");
    }
    let body = &p[HEADER_BYTES..];
    let rgba = |n: usize| -> Result<Vec<[u8; 4]>, Dropped> {
        if body.len() != n * 4 {
            return drop("body length does not match header");
        }
        Ok(body.as_chunks::<4>().0.to_vec())
    };
    match kind {
        1 => Ok(Frame::Image {
            seq,
            width: count,
            height,
            rgba: rgba(count as usize * height as usize)?,
        }),
        2 => Ok(Frame::Canvas {
            seq,
            colours: rgba(count as usize)?,
        }),
        3 => {
            if body.len() != count as usize {
                return drop("text length does not match header");
            }
            match std::str::from_utf8(body) {
                Ok(s) => Ok(Frame::Text {
                    seq,
                    append: s.to_string(),
                }),
                Err(_) => drop("text isn't UTF-8"),
            }
        }
        _ => drop("unknown frame kind"),
    }
}

/// Decodes one length-prefixed frame as read from the channel.
pub fn decode_wire(w: &[u8]) -> Result<Frame, Dropped> {
    if w.len() < 4 {
        return drop("no length prefix");
    }
    let len = u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize;
    if w.len() - 4 != len {
        return drop("length prefix does not match");
    }
    decode_payload(&w[4..])
}

/// Encodes a frame with its length prefix.
pub fn encode(frame: &Frame) -> Vec<u8> {
    let (kind, count, height, seq, body): (u8, u16, u16, u32, Vec<u8>) = match frame {
        Frame::Image {
            seq,
            width,
            height,
            rgba,
        } => (1, *width, *height, *seq, rgba.concat()),
        Frame::Canvas { seq, colours } => (2, colours.len() as u16, 0, *seq, colours.concat()),
        Frame::Text { seq, append } => {
            (3, append.len() as u16, 0, *seq, append.as_bytes().to_vec())
        }
    };
    let mut out = Vec::with_capacity(4 + HEADER_BYTES + body.len());
    out.extend_from_slice(&((HEADER_BYTES + body.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[kind, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_with_its_byte_length() {
        let f = Frame::Text {
            seq: 9,
            append: "hällo".into(),
        };
        assert_eq!(decode_wire(&encode(&f)), Ok(f));
    }
}
