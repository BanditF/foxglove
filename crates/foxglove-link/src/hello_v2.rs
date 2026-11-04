//! HELLO v2: capability words past HELLO's one (firmware widgets/host_link.h, D5 08 CAPS).
//!
//! HELLO's reply is one full 32-byte report, and its capability word (word 1, bytes 21-24,
//! [`crate::protocol::capabilities`]) has no bit left: 29 colour effects, 30 art STORE (#522),
//! 31 KNOB (#719). Raw HID reports can't grow, so the next words come in a reply of their own:
//!
//! | Report | Bytes | What |
//! | --- | --- | --- |
//! | D5 81 HELLO reply | 2 | version: **2** from HELLO v2 firmware on (1 before); the rest as before |
//! | D5 08 CAPS (PC) | 0-1 | `D5 08`, nothing else |
//! | D5 88 CAPS reply | 2 | n: the words that follow (1 today, at most [`MAX_WORDS`]) |
//! | | 3-6 | word 2, u32 LE: bit n is capability 32 + n (none defined yet) |
//! | | 7-30 | words 3-8, when n says so |
//!
//! Both ways stay compatible:
//! - An old app with new firmware: the HELLO reply's layout is unchanged; only byte 2 says 2,
//!   and no app before this one gates on it (it is shown, "host link v2"). An old app never sends
//!   D5 08, so it never sees D5 88.
//! - A new app with old firmware: HELLO's version is 1, so the app doesn't ask D5 08 (old firmware
//!   wouldn't answer, and the read would only wait out a timeout). Word 2 is then 0: [`State`]
//!   says why, and nothing fails.
//! - A short or odd reply: words the reply doesn't hold in full are 0; a count past
//!   [`MAX_WORDS`] reads [`MAX_WORDS`]; a reply that isn't D5 88 is no reply.
//!
//! The health view's "HELLO v2" row ([`row`]) says which of these the keyboard is.

use std::time::Duration;

use crate::health::{HealthRow, Level};
use crate::protocol::{Hello, PANEL, REPORT};
use crate::Transport;

/// HELLO's version byte from which D5 08 answers.
pub const MIN_HELLO_VERSION: u8 = 2;
/// The panel command (D5 08); its reply is D5 88.
pub const CAPS: u8 = 0x08;
/// Words a D5 88 reply can hold (bytes 3-30).
pub const MAX_WORDS: usize = (REPORT - 3) / 4;

/// Capability bits of word 2: bit n here is capability 32 + n. None is defined yet; a feature that
/// takes one adds it here and in host_link.h (`HOST_LINK_CAPS2_*`).
pub mod capabilities {}

/// The PC's request: D5 08.
pub fn request() -> Vec<u8> {
    vec![PANEL, CAPS]
}

/// Whether this HELLO says the keyboard answers D5 08.
pub fn offered(hello: &Hello) -> bool {
    hello.version >= MIN_HELLO_VERSION
}

/// The capability words past HELLO's, as a D5 88 reply had them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Words {
    /// The words the reply said it holds (byte 2), at most [`MAX_WORDS`].
    pub count: u8,
    /// Word 2 first; words the reply didn't hold in full are 0.
    pub words: [u32; MAX_WORDS],
}

impl Words {
    /// Parses a D5 88 reply. `None` when it isn't one (another report, or under 3 bytes).
    pub fn parse(r: &[u8]) -> Option<Words> {
        if r.len() < 3 || r[0] != PANEL || r[1] != 0x80 | CAPS {
            return None;
        }
        let count = (r[2] as usize).min(MAX_WORDS);
        let mut words = [0u32; MAX_WORDS];
        for (i, w) in words.iter_mut().enumerate().take(count) {
            let at = 3 + 4 * i;
            if let Some(b) = r.get(at..at + 4) {
                *w = u32::from_le_bytes(b.try_into().unwrap());
            }
        }
        Some(Words {
            count: count as u8,
            words,
        })
    }

    /// Word `n` (2 on; word 1 is HELLO's own). 0 for one the keyboard didn't send.
    pub fn word(&self, n: usize) -> u32 {
        n.checked_sub(2)
            .and_then(|i| self.words.get(i))
            .copied()
            .unwrap_or(0)
    }
}

/// What a connect found out about HELLO v2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// HELLO's version is below [`MIN_HELLO_VERSION`]: firmware before HELLO v2; word 2 is 0.
    NotOffered { version: u8 },
    /// D5 88 came.
    Read(Words),
    /// HELLO said v2, but D5 08 got no usable reply (the reason); word 2 is taken as 0.
    Failed(String),
}

impl State {
    /// Word 2, 0 unless it was read.
    pub fn word2(&self) -> u32 {
        match self {
            State::Read(w) => w.word(2),
            _ => 0,
        }
    }
}

/// How long one D5 08 waits, as HELLO does.
const WAIT: Duration = Duration::from_millis(500);

/// Reads the words past HELLO's when `hello` offers them; asks nothing otherwise. Two tries: a
/// reply now and then goes missing while other reports are in flight.
pub fn read(t: &impl Transport, hello: &Hello) -> State {
    if !offered(hello) {
        return State::NotOffered {
            version: hello.version,
        };
    }
    let mut why = String::new();
    for _ in 0..2 {
        match t.request(&request(), PANEL, CAPS, WAIT) {
            Ok(r) => match Words::parse(&r) {
                Some(w) => return State::Read(w),
                None => why = "the reply wasn't D5 88".into(),
            },
            Err(e) => why = format!("{e:#}"),
        }
    }
    State::Failed(why)
}

/// The health view's group for the row.
pub const SUBSYSTEM: &str = "firmware";

/// The "HELLO v2" row: OK with word 2 when it was read, OK too for firmware before it (nothing to
/// read; it isn't a fault), a warning when the firmware offered it and the read failed. `None`:
/// no keyboard connected.
pub fn row(state: Option<&State>) -> HealthRow {
    let (level, detail) = match state {
        None => (Level::Ok, "no keyboard connected".to_string()),
        Some(State::Read(w)) => (
            Level::Ok,
            // Named as word 2: HELLO's own word 1 is the "HELLO caps" row, and an empty word 2
            // (no capability defined there yet) read as "caps=0" looked like a fault.
            format!(
                "v2 read: word 2 (capabilities 32-63) = 0x{:08X}{}; {} word(s) past HELLO's",
                w.word(2),
                if w.word(2) == 0 { ", none set" } else { "" },
                w.count
            ),
        ),
        Some(State::NotOffered { version }) => (
            Level::Ok,
            format!(
                "info: firmware before HELLO v2 (host link v{version}): no second capability \
                 word, taken as 0"
            ),
        ),
        Some(State::Failed(why)) => (
            Level::Warn,
            format!("the firmware offers HELLO v2 but D5 08 wasn't answered ({why}); taken as 0"),
        ),
    };
    HealthRow::new(SUBSYSTEM, "HELLO v2", level, detail)
}

/// The "HELLO caps" row: HELLO's capability word (word 1) by name, e.g. "PANEL, ART_RELAY, …,
/// KNOB (0xffffffff)". Always OK: the bits say what the firmware offers, not a fault. `None`: no
/// keyboard, or no HELLO.
pub fn caps_row(hello: Option<&Hello>) -> HealthRow {
    let detail = hello.map_or_else(
        || "no HELLO read".to_string(),
        |h| crate::protocol::capabilities::describe(h.capabilities),
    );
    HealthRow::new(SUBSYSTEM, "HELLO caps", Level::Ok, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// HELLO replies as firmware sends them: 32 bytes, version in byte 2.
    fn hello_reply(version: u8) -> [u8; REPORT] {
        let mut r = [0u8; REPORT];
        r[..11].copy_from_slice(&[0xD5, 0x81, version, 64, 52, 2, 106, 0, 0xff, 0, 0]);
        r[21..25].copy_from_slice(&0xB3F1u32.to_le_bytes());
        r[27..31].copy_from_slice(&[0, 2, 0, 0]);
        r
    }

    /// A keyboard that answers each request with the next canned reply, and counts requests.
    struct Board {
        replies: RefCell<Vec<anyhow::Result<[u8; REPORT]>>>,
        asked: RefCell<Vec<Vec<u8>>>,
    }

    impl Board {
        fn new(replies: Vec<anyhow::Result<[u8; REPORT]>>) -> Board {
            Board {
                replies: RefCell::new(replies),
                asked: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for Board {
        fn send(&self, _: &[u8]) -> anyhow::Result<()> {
            Ok(())
        }
        fn request(&self, r: &[u8], _: u8, _: u8, _: Duration) -> anyhow::Result<[u8; REPORT]> {
            self.asked.borrow_mut().push(r.to_vec());
            let mut replies = self.replies.borrow_mut();
            if replies.is_empty() {
                anyhow::bail!("no reply from the keyboard");
            }
            replies.remove(0)
        }
    }

    fn caps_reply(bytes: &[u8]) -> [u8; REPORT] {
        let mut r = [0u8; REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    /// Old firmware (HELLO version 1, the 0.1/0.2 layout): HELLO parses exactly as before, D5 08
    /// is never sent, word 2 is 0 and the row is OK (info).
    #[test]
    fn old_length_reply_is_v1_and_never_asked() {
        let h = Hello::parse(&hello_reply(1));
        assert_eq!((h.version, h.width, h.height), (1, 64, 52));
        assert_eq!(h.capabilities, 0xB3F1);
        assert!(!offered(&h));
        let board = Board::new(vec![]);
        let s = read(&board, &h);
        assert_eq!(s, State::NotOffered { version: 1 });
        assert!(board.asked.borrow().is_empty(), "old firmware isn't asked");
        assert_eq!(s.word2(), 0);
        let row = row(Some(&s));
        assert_eq!((row.item.as_str(), row.level), ("HELLO v2", Level::Ok));
        assert!(
            row.detail.starts_with("info: firmware before HELLO v2"),
            "{}",
            row.detail
        );
    }

    /// The old parser and a v2 firmware: the HELLO reply's fields read the same; only the version
    /// differs. Nothing in HELLO moved, so every old parser reads the same values.
    #[test]
    fn v2_hello_parses_as_before() {
        let (v1, v2) = (Hello::parse(&hello_reply(1)), Hello::parse(&hello_reply(2)));
        assert_eq!(v2.version, 2);
        assert_eq!(Hello { version: 1, ..v2 }, v1);
        assert!(offered(&v2));
    }

    /// New firmware: D5 08 is asked once, its word 2 read, and the row shows it.
    #[test]
    fn new_length_reply_reads_word_2() {
        let h = Hello::parse(&hello_reply(2));
        let board = Board::new(vec![Ok(caps_reply(&[0xD5, 0x88, 1, 0x05, 0, 0, 0x80]))]);
        let s = read(&board, &h);
        assert_eq!(*board.asked.borrow(), vec![vec![0xD5u8, 0x08]]);
        assert_eq!(s.word2(), 0x8000_0005);
        let State::Read(w) = &s else { panic!("{s:?}") };
        assert_eq!(
            (w.count, w.word(2), w.word(3), w.word(1)),
            (1, 0x8000_0005, 0, 0)
        );
        let row = row(Some(&s));
        assert_eq!(row.level, Level::Ok);
        assert_eq!(
            row.detail,
            "v2 read: word 2 (capabilities 32-63) = 0x80000005; 1 word(s) past HELLO's"
        );
        // Today's firmware: one word, no bits.
        let w = Words::parse(&caps_reply(&[0xD5, 0x88, 1])).unwrap();
        assert_eq!((w.count, w.word(2)), (1, 0));
        // A later firmware with more words: an app reads what it knows, the rest is ignored.
        let mut more = caps_reply(&[0xD5, 0x88, 3]);
        more[3..7].copy_from_slice(&1u32.to_le_bytes());
        more[7..11].copy_from_slice(&2u32.to_le_bytes());
        more[11..15].copy_from_slice(&3u32.to_le_bytes());
        let w = Words::parse(&more).unwrap();
        assert_eq!((w.word(2), w.word(3), w.word(4), w.word(5)), (1, 2, 3, 0));
    }

    /// Short and odd replies: never a panic, missing words are 0, a wrong report is no reply.
    #[test]
    fn garbage_and_short_replies() {
        assert_eq!(Words::parse(&[]), None);
        assert_eq!(Words::parse(&[0xD5, 0x88]), None);
        assert_eq!(
            Words::parse(&[0xD5, 0x81, 1, 1, 1, 1, 1]),
            None,
            "a HELLO reply"
        );
        assert_eq!(
            Words::parse(&[0xD7, 0x88, 1, 1, 1, 1, 1]),
            None,
            "another protocol"
        );
        // Word 2 cut short: 0.
        let w = Words::parse(&[0xD5, 0x88, 1, 0xff, 0xff]).unwrap();
        assert_eq!((w.count, w.word(2)), (1, 0));
        // Zero words.
        let w = Words::parse(&caps_reply(&[0xD5, 0x88, 0, 0xff, 0xff, 0xff, 0xff])).unwrap();
        assert_eq!(
            (w.count, w.word(2)),
            (0, 0),
            "a word past the count is ignored"
        );
        // A count past what a report holds reads what it holds.
        let mut full = [0xffu8; REPORT];
        full[..2].copy_from_slice(&[0xD5, 0x88]);
        let w = Words::parse(&full).unwrap();
        assert_eq!(w.count as usize, MAX_WORDS);
        assert_eq!(w.word(2 + MAX_WORDS - 1), u32::MAX);
        assert_eq!(w.word(2 + MAX_WORDS), 0);

        // HELLO said v2 but D5 08 got nothing, or the wrong report: a warning, word 2 is 0.
        let h = Hello::parse(&hello_reply(2));
        let board = Board::new(vec![]);
        let s = read(&board, &h);
        assert_eq!(board.asked.borrow().len(), 2, "asked twice");
        assert!(matches!(s, State::Failed(_)));
        assert_eq!(s.word2(), 0);
        assert_eq!(row(Some(&s)).level, Level::Warn);
        let board = Board::new(vec![
            Ok(caps_reply(&[0xD5, 0x86])),
            Ok(caps_reply(&[0xD5, 0x88, 1, 7])),
        ]);
        assert_eq!(read(&board, &h).word2(), 7, "the second try is read");
        assert_eq!(row(None).detail, "no keyboard connected");
    }

    /// Word 1 by name: every bit, a few, none, and no HELLO.
    #[test]
    fn caps_row_names_hellos_word() {
        use crate::protocol::capabilities as c;
        let mut h = Hello::parse(&hello_reply(2));
        h.capabilities = u32::MAX;
        let row = caps_row(Some(&h));
        assert_eq!(row.subsystem, "firmware");
        assert_eq!(row.item, "HELLO caps");
        assert_eq!(row.level, Level::Ok);
        let all = row.detail;
        assert!(all.starts_with("PANEL, ART_RELAY, "), "{all}");
        assert!(all.ends_with(", KNOB (0xffffffff)"), "{all}");
        h.capabilities = c::PANEL | c::HEALTH;
        assert_eq!(caps_row(Some(&h)).detail, "PANEL, HEALTH (0x10000001)");
        h.capabilities = 0;
        assert_eq!(caps_row(Some(&h)).detail, "none (0x00000000)");
        assert_eq!(caps_row(None).detail, "no HELLO read");
    }
}
