//! The direct channel's wire (§8.4, v1.4 clarifications of #977), with no I/O: the pipe names a
//! consumer connects to, the nonce frame, and the `u32 LE length` + payload framing.
//!
//! - [`is_channel_pipe`]: a provider's `{pipe}` answer must be a local pipe named
//!   `\\.\pipe\hideout-ch-<rest>`; the router refuses any other (`channel.pipe`), and the SDKs
//!   never connect to one.
//! - [`nonce_matches`]: the consumer's first frame is the nonce, byte for byte, compared in
//!   constant time.
//! - [`read_stream`]: what the provider reads from a whole byte stream, for the shared vectors
//!   (`tests/hideout-vectors/channels/`); the SDKs read frame by frame with the same rules.

/// The most one channel frame's payload may be. A longer length prefix ends the channel.
pub const MAX_CHANNEL_FRAME: usize = 64 * 1024 * 1024;

/// The most the nonce frame may be. A longer one ends the channel before anything is buffered.
pub const MAX_NONCE: usize = 256;

/// How long a provider waits for the consumer to connect and send the nonce, in seconds (the
/// call timeout of §8.1). Then the pipe is closed and the channel is spent.
pub const ACCEPT_TIMEOUT_S: u64 = 30;

/// The prefix every channel pipe name has.
pub const PIPE_PREFIX: &str = r"\\.\pipe\hideout-ch-";

/// The longest pipe name accepted (Windows allows 256 characters).
pub const MAX_PIPE_NAME: usize = 256;

/// Whether `pipe` is a channel pipe a consumer may connect to: `\\.\pipe\hideout-ch-` (exact
/// case, the local machine only), then one or more ASCII letters, digits, `.`, `_` or `-`, at
/// most [`MAX_PIPE_NAME`] characters in all.
pub fn is_channel_pipe(pipe: &str) -> bool {
    let Some(rest) = pipe.strip_prefix(PIPE_PREFIX) else {
        return false;
    };
    !rest.is_empty()
        && pipe.len() <= MAX_PIPE_NAME
        && rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether the first frame `got` is `nonce`, in time that does not depend on where they differ.
pub fn nonce_matches(got: &[u8], nonce: &str) -> bool {
    let want = nonce.as_bytes();
    if got.len() != want.len() || want.is_empty() {
        return false;
    }
    got.iter().zip(want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// Encodes one frame: `u32` little-endian payload length, then the payload.
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// How a channel's byte stream ended, as the provider reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// The other side closed between frames.
    Closed,
    /// The first frame was missing, too long or not the nonce: the provider closes at once.
    Nonce,
    /// The stream stopped inside a length prefix or a payload.
    Truncated,
    /// A length prefix over [`MAX_CHANNEL_FRAME`]: the reader closes the channel.
    TooLong,
}

impl End {
    /// The vectors' name for it.
    pub fn as_str(self) -> &'static str {
        match self {
            End::Closed => "closed",
            End::Nonce => "nonce",
            End::Truncated => "truncated",
            End::TooLong => "too-long",
        }
    }
}

/// Reads a whole stream the consumer sent: the nonce frame, then frames. Returns the frames
/// after the nonce that arrived whole, and how the stream ended.
pub fn read_stream(bytes: &[u8], nonce: &str) -> (Vec<Vec<u8>>, End) {
    let mut frames = Vec::new();
    let mut at = 0usize;
    let mut first = true;
    loop {
        if at == bytes.len() {
            return (frames, if first { End::Nonce } else { End::Closed });
        }
        let Some(head) = bytes.get(at..at + 4) else {
            return (frames, if first { End::Nonce } else { End::Truncated });
        };
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let limit = if first { MAX_NONCE } else { MAX_CHANNEL_FRAME };
        if len > limit {
            return (frames, if first { End::Nonce } else { End::TooLong });
        }
        let Some(payload) = bytes.get(at + 4..at + 4 + len) else {
            return (frames, if first { End::Nonce } else { End::Truncated });
        };
        at += 4 + len;
        if first {
            if !nonce_matches(payload, nonce) {
                return (frames, End::Nonce);
            }
            first = false;
        } else {
            frames.push(payload.to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_names() {
        assert!(is_channel_pipe(
            r"\\.\pipe\hideout-ch-12-0-example.art-previews"
        ));
        assert!(!is_channel_pipe(r"\\.\pipe\hideout-ch-"));
        assert!(!is_channel_pipe(r"\\host\pipe\hideout-ch-x"));
        assert!(!is_channel_pipe(r"\\.\pipe\hideout-ch-a\b"));
        assert!(!is_channel_pipe(&format!(
            "{PIPE_PREFIX}{}",
            "a".repeat(300)
        )));
    }

    #[test]
    fn nonce_then_frames() {
        let mut s = encode_frame(b"n1");
        s.extend(encode_frame(b"hi"));
        s.extend(encode_frame(b""));
        assert_eq!(
            read_stream(&s, "n1"),
            (vec![b"hi".to_vec(), vec![]], End::Closed)
        );
        assert_eq!(read_stream(&s, "n2"), (vec![], End::Nonce));
        assert!(!nonce_matches(b"", ""));
    }
}
