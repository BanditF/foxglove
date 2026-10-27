//! `foxglove-live` v1 (#499): what the live keyboard feed tells a paired wallpaper, as JSON text
//! frames on a loopback WebSocket. The runtime is `live_feed`; this file is the format, the
//! connection checks and the key batching, with no I/O so the tests can hold all of it.
//!
//! **Privacy:** a key is its index into the board's geometry (`board.keys`), whether it went
//! down or up, and a few milliseconds of timing inside its batch. Never a keycode, a binding, a
//! label or a character: [`Message`] has no field that could carry one, and the tests check
//! every message's keys against an allowlist. An index on a known keymap still says which key
//! it was, so only a wallpaper holding this install's token gets the feed.
//!
//! Foxglove → wallpaper (`t` names the message; a client ignores types it doesn't know, so
//! later topics such as PC stats or notifications can be added without a new version):
//!
//! ```text
//! {"t":"hello","v":1,"topics":["keys","state"],"board":{"name":"Eyelash Corne","keys":[[x,y,w,h,r],...]},"state":{...},"quiet":null}
//! {"t":"state","layer":{"index":3,"name":"NAV"},"art":"Fox run","battery":{"left":80,"right":61},"link":{"connected":true,"output":"ble","profile":1}}
//! {"t":"keys","e":[[17,1,0],[17,0,9]]}      index, 1 down / 0 up, ms since the batch's first
//! {"t":"board","board":{...}}               the geometry changed (another keyboard, layout)
//! {"t":"quiet","why":"away"}                nothing more until "live": paused, away, fullscreen
//! {"t":"live"}
//! ```
//!
//! Wallpaper → Foxglove: `{"t":"hi","wallpaper":"bandit-live-board"}` and
//! `{"t":"paused","paused":true}` (Wallpaper Engine's `setPaused`). Anything else is ignored.

use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
/// The WebSocket sub-protocol a wallpaper offers (with its token as a second one).
pub const PROTOCOL: &str = "foxglove-live.v1";
/// The topics this build sends; reserved for later: `stats`, `notes`.
pub const TOPICS: &[&str] = &["keys", "state"];
/// Key events in one frame at most (a burst beyond it waits for the next frame).
pub const MAX_BATCH: usize = 64;
/// How long a batch collects after its first key: at most 60 frames a second.
pub const BATCH_MS: u64 = 16;
/// Wallpapers connected at once.
pub const MAX_CLIENTS: usize = 4;

/// One key on the board: top-left corner, size and rotation (degrees, clockwise about the key's
/// centre), in key units, as the layer overlay draws it.
pub type KeyRect = [f32; 5];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Board {
    pub name: String,
    pub keys: Vec<KeyRect>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Layer {
    pub index: u8,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Battery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Link {
    pub connected: bool,
    /// `usb` or `ble`, once the keyboard said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The 1-based Bluetooth profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<u8>,
}

/// The slow facts, sent whole whenever one changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct State {
    pub layer: Option<Layer>,
    /// The right screen's piece, by name.
    pub art: Option<String>,
    pub battery: Battery,
    pub link: Link,
}

/// Why the feed is quiet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    /// Foxglove is paused.
    Paused,
    /// The session is locked or the display is off.
    Away,
    /// A full-screen program (a game, a presentation) is in front.
    Fullscreen,
}

/// One key event: index, 1 down / 0 up, ms since the batch's first event.
pub type KeyEvent = (u16, u8, u16);

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Message {
    Hello {
        v: u8,
        topics: &'static [&'static str],
        board: Option<Board>,
        state: State,
        quiet: Option<Why>,
    },
    State(State),
    Keys {
        e: Vec<KeyEvent>,
    },
    Board {
        board: Option<Board>,
    },
    Quiet {
        why: Why,
    },
    Live,
}

impl Message {
    pub fn json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// What a wallpaper says.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum FromWallpaper {
    Hi {
        #[serde(default)]
        wallpaper: String,
    },
    Paused {
        paused: bool,
    },
}

pub fn parse(text: &str) -> Option<FromWallpaper> {
    serde_json::from_str(text).ok()
}

/// The key's index for a matrix position: its place in the geometry; without geometry, ZMK's
/// key position (ZMK reports row 0 and the position as the column).
pub fn key_index(positions: &[(usize, usize)], row: u16, col: u16) -> Option<u16> {
    let (row, col) = (usize::from(row), usize::from(col));
    if positions.is_empty() {
        return (row == 0).then(|| u16::try_from(col).ok()).flatten();
    }
    positions
        .iter()
        .position(|&p| p == (row, col))
        .and_then(|i| u16::try_from(i).ok())
}

/// Rounds geometry to hundredths of a key, so the hello stays short.
pub fn key_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> KeyRect {
    let round = |v: f32| {
        if v.is_finite() {
            (v * 100.0).round() / 100.0
        } else {
            0.0
        }
    };
    [round(x), round(y), round(w), round(h), round(r)]
}

/// Collects key events for one frame.
#[derive(Debug, Default)]
pub struct Batch {
    events: Vec<KeyEvent>,
    first_ms: u64,
}

impl Batch {
    /// Adds an event at `now_ms`; true when it was the batch's first (start the frame's clock).
    pub fn push(&mut self, index: u16, down: bool, now_ms: u64) -> bool {
        let first = self.events.is_empty();
        if first {
            self.first_ms = now_ms;
        }
        if self.events.len() < MAX_BATCH {
            let dt = now_ms
                .saturating_sub(self.first_ms)
                .min(u64::from(u16::MAX));
            self.events.push((index, u8::from(down), dt as u16));
        }
        first
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.events.len() >= MAX_BATCH
    }

    /// The frame, emptying the batch.
    pub fn take(&mut self) -> Option<Message> {
        (!self.events.is_empty()).then(|| Message::Keys {
            e: std::mem::take(&mut self.events),
        })
    }
}

/// Which gate holds the feed, first by importance; `None` when it may send.
pub fn quiet(paused: bool, away: bool, fullscreen: bool) -> Option<Why> {
    if paused {
        Some(Why::Paused)
    } else if away {
        Some(Why::Away)
    } else if fullscreen {
        Some(Why::Fullscreen)
    } else {
        None
    }
}

/// Who may connect: a page loaded from a file (Wallpaper Engine loads web wallpapers from
/// `file:///`, and Chromium sends `Origin: null` for them; #320) or a program that sends no
/// Origin, and in every case only with this install's token, as `?token=` or as a second
/// sub-protocol. Any web page has a real origin and is refused before the token is looked at.
pub fn admit(origin: Option<&str>, token_given: Option<&str>, token: &str) -> Result<(), String> {
    match origin {
        None | Some("null") | Some("file://") => {}
        Some(o) => return Err(format!("origin {o} isn't a local wallpaper")),
    }
    if token.is_empty() || !token_given.is_some_and(|g| same(g.as_bytes(), token.as_bytes())) {
        return Err("wrong or missing token".into());
    }
    Ok(())
}

/// The token from the path's query (`/live?token=…`) or the offered sub-protocols
/// (`foxglove-live.v1, <token>`).
pub fn token_from<'a>(path: &'a str, protocols: Option<&'a str>) -> Option<&'a str> {
    let query = path.split_once('?').and_then(|(_, q)| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == "token")
            .map(|(_, v)| v)
    });
    query.or_else(|| {
        protocols?
            .split(',')
            .map(str::trim)
            .find(|p| !p.is_empty() && *p != PROTOCOL)
    })
}

/// Compares without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A fresh token: 24 characters from the OS's random source.
pub fn new_token() -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).ok()?;
    Some(
        bytes
            .iter()
            .map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn sample_state() -> State {
        State {
            layer: Some(Layer {
                index: 3,
                name: "NAV".into(),
            }),
            art: Some("Fox run".into()),
            battery: Battery {
                left: Some(80),
                right: Some(61),
            },
            link: Link {
                connected: true,
                output: Some("ble".into()),
                profile: Some(1),
            },
        }
    }

    fn every_message() -> Vec<Message> {
        let board = Board {
            name: "Eyelash Corne".into(),
            keys: vec![key_rect(0.0, 0.37, 1.0, 1.0, 0.0)],
        };
        vec![
            Message::Hello {
                v: VERSION,
                topics: TOPICS,
                board: Some(board.clone()),
                state: sample_state(),
                quiet: Some(Why::Away),
            },
            Message::State(sample_state()),
            Message::Keys {
                e: vec![(17, 1, 0), (17, 0, 9)],
            },
            Message::Board { board: Some(board) },
            Message::Quiet {
                why: Why::Fullscreen,
            },
            Message::Live,
        ]
    }

    fn keys_of(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                for (k, v) in m {
                    out.push(k.clone());
                    keys_of(v, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| keys_of(v, out)),
            _ => {}
        }
    }

    /// The privacy test (#498, #499): no message has a field that could carry a keycode, a
    /// binding or a character, and a key event is three small numbers.
    #[test]
    fn no_message_carries_a_keycode_or_character() {
        const ALLOWED: &[&str] = &[
            "t",
            "v",
            "topics",
            "board",
            "name",
            "keys",
            "state",
            "quiet",
            "layer",
            "index",
            "art",
            "battery",
            "left",
            "right",
            "link",
            "connected",
            "output",
            "profile",
            "e",
            "why",
        ];
        for m in every_message() {
            let json = m.json();
            let v: Value = serde_json::from_str(&json).unwrap();
            let mut keys = Vec::new();
            keys_of(&v, &mut keys);
            for k in keys {
                assert!(ALLOWED.contains(&k.as_str()), "{k} in {json}");
            }
            for banned in ["keycode", "code", "char", "label", "binding", "usage"] {
                assert!(!json.contains(banned), "{banned} in {json}");
            }
            if let Message::Keys { .. } = m {
                let e = &v["e"];
                for ev in e.as_array().unwrap() {
                    let ev = ev.as_array().unwrap();
                    assert_eq!(ev.len(), 3);
                    assert!(ev.iter().all(Value::is_u64), "numbers only: {json}");
                }
            }
        }
    }

    #[test]
    fn the_messages_read_as_documented() {
        let m = every_message();
        assert_eq!(m[2].json(), r#"{"t":"keys","e":[[17,1,0],[17,0,9]]}"#);
        assert_eq!(m[4].json(), r#"{"t":"quiet","why":"fullscreen"}"#);
        assert_eq!(m[5].json(), r#"{"t":"live"}"#);
        let hello: Value = serde_json::from_str(&m[0].json()).unwrap();
        assert_eq!(hello["t"], "hello");
        assert_eq!(hello["v"], 1);
        assert_eq!(
            hello["board"]["keys"][0],
            serde_json::json!([0.0, 0.37, 1.0, 1.0, 0.0])
        );
        assert_eq!(hello["state"]["layer"]["name"], "NAV");
        assert_eq!(hello["quiet"], "away");
        let empty = Message::State(State::default()).json();
        assert_eq!(
            empty,
            r#"{"t":"state","layer":null,"art":null,"battery":{},"link":{"connected":false}}"#
        );
    }

    #[test]
    fn wallpaper_messages() {
        assert_eq!(
            parse(r#"{"t":"paused","paused":true}"#),
            Some(FromWallpaper::Paused { paused: true })
        );
        assert_eq!(
            parse(r#"{"t":"hi","wallpaper":"bandit-live-board","extra":1}"#),
            Some(FromWallpaper::Hi {
                wallpaper: "bandit-live-board".into()
            })
        );
        assert_eq!(
            parse(r#"{"t":"keys"}"#),
            None,
            "not the wallpaper's to send"
        );
        assert_eq!(parse("not json"), None);
    }

    #[test]
    fn keys_become_indices() {
        let positions = [(0, 0), (0, 1), (1, 0)];
        assert_eq!(key_index(&positions, 1, 0), Some(2));
        assert_eq!(key_index(&positions, 5, 5), None, "not on the board");
        // Without geometry: ZMK's position, row 0 only.
        assert_eq!(key_index(&[], 0, 41), Some(41));
        assert_eq!(key_index(&[], 2, 3), None);
    }

    #[test]
    fn a_batch_is_one_frame_and_bounded() {
        let mut b = Batch::default();
        assert!(b.take().is_none());
        assert!(b.push(4, true, 1000), "the first starts the frame");
        assert!(!b.push(4, false, 1009));
        assert_eq!(
            b.take(),
            Some(Message::Keys {
                e: vec![(4, 1, 0), (4, 0, 9)]
            })
        );
        assert!(b.is_empty());
        for i in 0..100 {
            b.push(i, true, 5000);
        }
        assert!(b.is_full());
        let Some(Message::Keys { e }) = b.take() else {
            panic!()
        };
        assert_eq!(e.len(), MAX_BATCH);
    }

    #[test]
    fn gates_in_order() {
        assert_eq!(quiet(false, false, false), None);
        assert_eq!(quiet(true, true, true), Some(Why::Paused));
        assert_eq!(quiet(false, true, true), Some(Why::Away));
        assert_eq!(quiet(false, false, true), Some(Why::Fullscreen));
    }

    #[test]
    fn only_a_local_wallpaper_with_the_token_is_admitted() {
        let t = "s3cretS3cretS3cretS3cret";
        assert!(
            admit(Some("null"), Some(t), t).is_ok(),
            "a file:// wallpaper"
        );
        assert!(admit(Some("file://"), Some(t), t).is_ok());
        assert!(admit(None, Some(t), t).is_ok(), "a local program");
        assert!(
            admit(Some("null"), Some("wrong"), t).is_err(),
            "a wrong token"
        );
        assert!(admit(Some("null"), None, t).is_err(), "no token");
        assert!(
            admit(Some("https://example.com"), Some(t), t).is_err(),
            "a web page, even with the token"
        );
        assert!(admit(Some("null"), Some(""), "").is_err(), "no token set");
        assert_eq!(token_from("/live?token=abc&x=1", None), Some("abc"));
        assert_eq!(
            token_from("/live", Some("foxglove-live.v1, abc")),
            Some("abc")
        );
        assert_eq!(token_from("/live", Some("foxglove-live.v1")), None);
    }

    #[test]
    fn tokens_are_long_and_differ() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 24);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
