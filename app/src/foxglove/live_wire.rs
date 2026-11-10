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
//! {"t":"state","layer":{"index":3,"name":"NAV"},"art":"Fox run","battery":{"left":80,"right":61},"link":{"connected":true,"output":"ble","profile":1},
//!  "lights":{"on":true,"color":"#ff6a00","brightness":60,"effect":"Solid","rainbow":false},
//!  "keyboard":{"name":"Eyelash Corne","source":"host_link","connected":true}}
//! {"t":"art","art":{"name":"Fox run","nva":"TlZBMUSM..."}}   the piece's frames, once per change
//! {"t":"panel","panel":{"w":64,"h":52,"bits":"AAAA..."}}     the left screen's PC panel, as shown
//! {"t":"keys","e":[[17,1,0],[17,0,9]]}      index, 1 down / 0 up, ms since the batch's first
//! {"t":"board","board":{...}}               the geometry changed (another keyboard, layout)
//! {"t":"quiet","why":"away"}                nothing more until "live": paused, away, fullscreen
//! {"t":"live"}
//! {"t":"notify","id":7,"title":"Right half battery low","body":"14% left.","level":"info"}
//! {"t":"refused","why":"token"}            the token was wrong; a close 4001 follows at once
//! ```
//!
//! Wallpaper → Foxglove: `{"t":"hi","wallpaper":"bandit-live-board","caps":["notify"]}`,
//! `{"t":"paused","paused":true}` (Wallpaper Engine's `setPaused`) and `{"t":"notified","id":7}`
//! (it drew notification 7). Anything else is ignored.
//!
//! **Notifications:** only a client that said `caps: ["notify"]` in its `hi` gets `notify`, and
//! only while the feed isn't quiet. Without its `notified` in [`ACK_LIMIT`] Foxglove shows the
//! Windows notification instead, so nothing is lost.

use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
/// The WebSocket sub-protocol a wallpaper offers (with its token as a second one).
pub const PROTOCOL: &str = "foxglove-live.v1";
/// The topics this build sends; reserved for later: `stats`, `notes`.
pub const TOPICS: &[&str] = &["keys", "state", "art", "panel"];
/// The left screen's PC panel at most this many bytes of bits (a 64x52 panel is 416).
pub const MAX_PANEL_BYTES: usize = 1024;
/// Panel frames at most this often (an animated panel changes a few times a second).
pub const PANEL_MS: u64 = 250;
/// Key events in one frame at most (a burst beyond it waits for the next frame).
pub const MAX_BATCH: usize = 64;
/// How long a batch collects after its first key: at most 60 frames a second.
pub const BATCH_MS: u64 = 16;
/// Wallpapers connected at once.
pub const MAX_CLIENTS: usize = 4;
/// The capability a wallpaper declares in its `hi` to be sent notifications.
pub const CAP_NOTIFY: &str = "notify";
/// How long a wallpaper has to say it drew a notification before Windows shows it instead.
pub const ACK_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);
/// The close code after `refused`: an application code (4000-4999), so a browser's `onclose`
/// sees it, where a refused upgrade only shows as 1006.
pub const CLOSE_REFUSED: u16 = 4001;
/// Refused connections are summed in the log at most this often, and the health warning about
/// them goes once none came for this long.
pub const REFUSAL_QUIET: std::time::Duration = std::time::Duration::from_secs(10 * 60);

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
    /// The active Bluetooth profile has its host connected (the left screen's LINKED / SEARCH,
    /// and its BT label struck through when not), once the keyboard said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_connected: Option<bool>,
}

/// The keyboard's lights as it reports them (#40: the wallpaper can take the keyboard's colour).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Lights {
    pub on: bool,
    /// The chosen colour, `#rrggbb`, at full brightness (the LEDs' brightness is apart).
    pub color: String,
    /// 0-100.
    pub brightness: u8,
    /// The effect's name (`Solid`, `Breathe`, …), or the keyboard's id for it when unknown.
    pub effect: String,
    /// The effect makes its own colours (Spectrum, Swirl), so `color` isn't what it shows.
    pub rainbow: bool,
}

/// The keyboard the feed is about (#801): the focused keyboard, or the one the feed is pinned
/// to. Its name and where Foxglove found it; never a serial number or a device path.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Keyboard {
    pub name: String,
    /// `host_link` (a Foxglove keyboard: layers, keys, battery and lights follow), `via` or
    /// `lamp_array` (its name and shape only).
    pub source: String,
    /// It is connected now; a keyboard that went this run stays named, `false`.
    pub connected: bool,
}

/// The slow facts, sent whole whenever one changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct State {
    pub layer: Option<Layer>,
    /// The right screen's piece, by name.
    pub art: Option<String>,
    pub battery: Battery,
    pub link: Link,
    /// Left out until the keyboard reported its lights.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lights: Option<Lights>,
    /// Which keyboard the rest is about (#801); left out before any keyboard was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard: Option<Keyboard>,
}

/// The right screen's piece with its frames (`live_art`): sent when the piece changes and to a
/// wallpaper as it joins, never per frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Art {
    /// The piece, by the same name as `state.art`.
    pub name: String,
    /// Where it lives: [`FROM_BUILT_IN`], [`FROM_KEPT`] or [`FROM_FILE`].
    pub from: &'static str,
    /// Its animation as an upright NVA1 blob in base64 (docs/live-feed.md), at most
    /// `live_art::MAX_BLOB` bytes before base64; `None` when Foxglove doesn't have its frames.
    pub nva: Option<String>,
    /// Why `nva` is `None` (one of the `MISSING_*` words); left out with frames.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<&'static str>,
}

/// `art.from`: built into the right half's firmware.
pub const FROM_BUILT_IN: &str = "built_in";
/// `art.from`: one of the keyboard's kept places.
pub const FROM_KEPT: &str = "kept";
/// `art.from`: a library file Foxglove sent, played from the right half's memory.
pub const FROM_FILE: &str = "file";
/// `art.missing`: a kept place whose bytes no piece on this PC has (by CRC, #863).
pub const MISSING_NOT_ON_PC: &str = "not_on_pc";
/// `art.missing`: a built-in of firmware whose art Foxglove doesn't know.
pub const MISSING_OTHER_FIRMWARE: &str = "other_firmware";
/// `art.missing`: the piece's file couldn't be read.
pub const MISSING_UNREADABLE: &str = "unreadable";
/// `art.missing`: the upright blob is past `live_art::MAX_BLOB`.
pub const MISSING_TOO_BIG: &str = "too_big";
/// `art.missing`: the right half plays a set pushed earlier (by an earlier Foxglove run, or
/// another program) that this PC has no record of, or whose file changed since (#1044).
pub const MISSING_PUSHED_UNKNOWN: &str = "pushed_unknown";

/// The left screen's PC panel as Foxglove last sent it to the keyboard (`screens`): the part of the
/// left screen Foxglove draws (clock, volume, media). `None` while the firmware draws its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Panel {
    pub w: u16,
    pub h: u16,
    /// Rows MSB first, `ceil(w / 8)` bytes a row, a set bit for ink, in base64.
    pub bits: String,
}

impl Panel {
    /// A frame as the screens service sent it, repacked to whole-byte rows; `None` past
    /// [`MAX_PANEL_BYTES`] or when the bits don't cover the frame.
    pub fn of(width: usize, height: usize, stride: usize, bits: &[u8]) -> Option<Panel> {
        use base64::Engine;
        let row = width.div_ceil(8);
        if width == 0 || height == 0 || row * height > MAX_PANEL_BYTES || stride < row {
            return None;
        }
        let mut out = Vec::with_capacity(row * height);
        for y in 0..height {
            out.extend_from_slice(bits.get(y * stride..y * stride + row)?);
        }
        Some(Panel {
            w: u16::try_from(width).ok()?,
            h: u16::try_from(height).ok()?,
            bits: base64::engine::general_purpose::STANDARD.encode(out),
        })
    }
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

/// How much a notification matters: `warn` for a battery about to go, `info` for the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Info,
    Warn,
}

/// Why a connection was refused, as the client is told: only that its token was wrong (or
/// missing), nothing about the right one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Refused {
    Token,
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
    /// `art: null`: nothing on the right screen Foxglove can name (no keyboard).
    Art {
        art: Option<Art>,
    },
    /// `panel: null`: the firmware draws that part of the left screen itself.
    Panel {
        panel: Option<Panel>,
    },
    Board {
        board: Option<Board>,
    },
    Quiet {
        why: Why,
    },
    Live,
    /// A notification for a wallpaper that declared [`CAP_NOTIFY`]; it answers `notified`.
    Notify {
        id: u64,
        title: String,
        body: String,
        level: Level,
    },
    /// The one message a client with a wrong or missing token gets, before a close
    /// [`CLOSE_REFUSED`].
    Refused {
        why: Refused,
    },
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
        /// What it can do beyond the feed: [`CAP_NOTIFY`].
        #[serde(default)]
        caps: Vec<String>,
    },
    Paused {
        paused: bool,
    },
    /// It drew notification `id`.
    Notified {
        id: u64,
    },
}

impl FromWallpaper {
    /// A `hi` that asks for notifications.
    pub fn wants_notify(caps: &[String]) -> bool {
        caps.iter().any(|c| c == CAP_NOTIFY)
    }
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
pub fn admit(origin: Option<&str>, token_given: Option<&str>, token: &str) -> Result<(), Denied> {
    match origin {
        None | Some("null") | Some("file://") => {}
        Some(o) => return Err(Denied::Origin(o.to_string())),
    }
    if token.is_empty() || !token_given.is_some_and(|g| same(g.as_bytes(), token.as_bytes())) {
        return Err(Denied::Token);
    }
    Ok(())
}

/// Why [`admit`] said no.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denied {
    /// A web page: refused with a plain HTTP 403, before the token is looked at.
    Origin(String),
    /// A local wallpaper or program with a wrong or missing token: told `refused`.
    Token,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::Origin(o) => write!(f, "origin {o} isn't a local wallpaper"),
            Denied::Token => f.write_str("wrong or missing token"),
        }
    }
}

/// Token refusals (a wallpaper still holding an old token retries every few seconds): what the
/// log says and whether the health view warns. No clock of its own: the log's summary goes out
/// with the next refusal once [`REFUSAL_QUIET`] passed since the last line, and the warning is
/// worked out when it is read.
#[derive(Debug, Default)]
pub struct Refusals {
    /// Refusals since the last log line.
    unlogged: u64,
    /// The last log line about them.
    last_line: Option<std::time::Instant>,
    /// The last refusal, until a wallpaper paired.
    last: Option<std::time::Instant>,
}

impl Refusals {
    pub const fn new() -> Self {
        Refusals {
            unlogged: 0,
            last_line: None,
            last: None,
        }
    }

    /// A token refusal at `now`: the log lines to write (without the `Foxglove: ` prefix). The
    /// first of a run in full, then at most one summary per [`REFUSAL_QUIET`]. A run ends with a
    /// pairing or [`REFUSAL_QUIET`] without refusals; the next refusal starts a new one, after a
    /// summary of the old run's unlogged ones.
    pub fn refused(&mut self, now: std::time::Instant) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.warns(now) {
            if self.unlogged > 0 {
                lines.push(self.summary());
            }
            self.last_line = Some(now);
            self.last = Some(now);
            lines.push(format!("live feed refused a connection: {}", Denied::Token));
            return lines;
        }
        self.last = Some(now);
        self.unlogged += 1;
        if self
            .last_line
            .is_none_or(|t| now.saturating_duration_since(t) >= REFUSAL_QUIET)
        {
            lines.push(self.summary());
            self.last_line = Some(now);
        }
        lines
    }

    fn summary(&mut self) -> String {
        let n = std::mem::take(&mut self.unlogged);
        let s = if n == 1 { "" } else { "s" };
        format!("live feed refused {n} connection{s}: wrong token")
    }

    /// A wallpaper paired: the warning goes; a refusal after this starts a new run.
    pub fn paired(&mut self) {
        self.last = None;
    }

    /// A wallpaper keeps presenting a wrong token: a refusal came less than [`REFUSAL_QUIET`]
    /// before `now`, and no pairing since.
    pub fn warns(&self, now: std::time::Instant) -> bool {
        self.last
            .is_some_and(|t| now.saturating_duration_since(t) < REFUSAL_QUIET)
    }
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
                profile_connected: Some(true),
            },
            lights: Some(Lights {
                on: true,
                color: "#ff6a00".into(),
                brightness: 60,
                effect: "Solid".into(),
                rainbow: false,
            }),
            keyboard: Some(Keyboard {
                name: "Eyelash Corne".into(),
                source: "host_link".into(),
                connected: true,
            }),
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
            Message::Art {
                art: Some(Art {
                    name: "Fox run".into(),
                    from: FROM_FILE,
                    nva: Some("TlZBMUSMBAEBAAEAAA==".into()),
                    missing: None,
                }),
            },
            Message::Quiet {
                why: Why::Fullscreen,
            },
            Message::Live,
            Message::Panel {
                panel: Panel::of(16, 2, 2, &[0x80, 0x01, 0xff, 0x00]),
            },
            Message::Notify {
                id: 7,
                title: "Right half battery low".into(),
                body: "14% left. Charge it soon.".into(),
                level: Level::Warn,
            },
            Message::Refused {
                why: Refused::Token,
            },
            Message::Art {
                art: Some(Art {
                    name: "Radar".into(),
                    from: FROM_KEPT,
                    nva: None,
                    missing: Some(MISSING_NOT_ON_PC),
                }),
            },
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
            "lights",
            "on",
            "color",
            "brightness",
            "effect",
            "rainbow",
            "e",
            "why",
            "nva",
            // `art`: where the piece lives, and why it has no frames.
            "from",
            "missing",
            "profile_connected",
            "panel",
            "w",
            "h",
            "bits",
            // `notify`: a notification's own words, only to a client that declared the cap.
            "id",
            "title",
            "body",
            "level",
            // `state.keyboard` (#801): which keyboard, by name and source.
            "keyboard",
            "source",
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
        assert_eq!(
            m[4].json(),
            r#"{"t":"art","art":{"name":"Fox run","from":"file","nva":"TlZBMUSMBAEBAAEAAA=="}}"#
        );
        assert_eq!(
            Message::Art {
                art: Some(Art {
                    name: "Radar".into(),
                    from: FROM_KEPT,
                    nva: None,
                    missing: Some(MISSING_NOT_ON_PC),
                })
            }
            .json(),
            r#"{"t":"art","art":{"name":"Radar","from":"kept","nva":null,"missing":"not_on_pc"}}"#
        );
        assert_eq!(m[5].json(), r#"{"t":"quiet","why":"fullscreen"}"#);
        assert_eq!(m[6].json(), r#"{"t":"live"}"#);
        assert_eq!(
            m[8].json(),
            r#"{"t":"notify","id":7,"title":"Right half battery low","body":"14% left. Charge it soon.","level":"warn"}"#
        );
        assert_eq!(m[9].json(), r#"{"t":"refused","why":"token"}"#);
        assert_eq!(
            Message::Art { art: None }.json(),
            r#"{"t":"art","art":null}"#
        );
        assert_eq!(
            m[7].json(),
            r#"{"t":"panel","panel":{"w":16,"h":2,"bits":"gAH/AA=="}}"#
        );
        let hello: Value = serde_json::from_str(&m[0].json()).unwrap();
        assert_eq!(hello["state"]["link"]["profile_connected"], true);
        assert_eq!(hello["t"], "hello");
        assert_eq!(hello["v"], 1);
        assert_eq!(
            hello["board"]["keys"][0],
            serde_json::json!([0.0, 0.37, 1.0, 1.0, 0.0])
        );
        assert_eq!(hello["state"]["layer"]["name"], "NAV");
        assert_eq!(
            hello["state"]["keyboard"],
            serde_json::json!({"name": "Eyelash Corne", "source": "host_link", "connected": true})
        );
        assert_eq!(hello["quiet"], "away");
        assert_eq!(
            hello["state"]["lights"],
            serde_json::json!({"on": true, "color": "#ff6a00", "brightness": 60, "effect": "Solid", "rainbow": false})
        );
        let empty = Message::State(State::default()).json();
        assert_eq!(
            empty,
            r#"{"t":"state","layer":null,"art":null,"battery":{},"link":{"connected":false}}"#
        );
    }

    /// The panel goes out as the keyboard shows it: whole-byte rows, bounded.
    #[test]
    fn a_panel_is_repacked_and_bounded() {
        // 12 px wide in 3-byte rows: the padding byte goes, the bits stay.
        let p = Panel::of(12, 2, 3, &[0xab, 0xc0, 0x77, 0x12, 0x30, 0x77]).unwrap();
        assert_eq!((p.w, p.h), (12, 2));
        assert_eq!(p.bits, "q8ASMA==", "ab c0 12 30");
        assert!(
            Panel::of(64, 52, 8, &[0; 8 * 52]).is_some(),
            "the Corne's 64x52"
        );
        assert!(
            Panel::of(128, 128, 16, &[0; 16 * 128]).is_none(),
            "past the bound"
        );
        assert!(Panel::of(64, 52, 8, &[0; 100]).is_none(), "short bits");
        assert!(Panel::of(0, 52, 8, &[]).is_none());
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
                wallpaper: "bandit-live-board".into(),
                caps: Vec::new(),
            })
        );
        let Some(FromWallpaper::Hi { caps, .. }) =
            parse(r#"{"t":"hi","wallpaper":"x","caps":["notify","later"]}"#)
        else {
            panic!()
        };
        assert!(FromWallpaper::wants_notify(&caps));
        assert!(!FromWallpaper::wants_notify(&["later".into()]));
        assert_eq!(
            parse(r#"{"t":"notified","id":7}"#),
            Some(FromWallpaper::Notified { id: 7 })
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
        assert_eq!(admit(Some("null"), None, t), Err(Denied::Token), "no token");
        assert_eq!(
            admit(Some("https://example.com"), Some("wrong"), t),
            Err(Denied::Origin("https://example.com".into())),
            "a web page is refused by its origin, never told about the token"
        );
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

    /// A wallpaper with an old token retries every few seconds: the log says the first in full,
    /// then sums them at most once per REFUSAL_QUIET, timed by the next refusal; the warning lasts
    /// while they come and goes with a pairing or REFUSAL_QUIET of quiet.
    #[test]
    fn token_refusals_are_summed_and_warn_while_they_come() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut r = Refusals::new();
        assert!(!r.warns(t0));
        assert_eq!(
            r.refused(t0),
            ["live feed refused a connection: wrong or missing token"]
        );
        assert!(r.warns(t0));
        // Every 8 s for just under 10 min: nothing more in the log.
        let mut s = 8;
        while s < 600 {
            assert!(r.refused(at(s)).is_empty(), "at {s} s");
            s += 8;
        }
        assert!(r.warns(at(599)));
        // The next one, 10 min after the full line: one summary of all since.
        assert_eq!(
            r.refused(at(600)),
            ["live feed refused 75 connections: wrong token"]
        );
        assert!(r.refused(at(608)).is_empty());
        // A pairing clears the warning; the next refusal starts a new run, after a summary of
        // the old run's unlogged one.
        r.paired();
        assert!(!r.warns(at(609)));
        assert_eq!(
            r.refused(at(700)),
            [
                "live feed refused 1 connection: wrong token",
                "live feed refused a connection: wrong or missing token"
            ]
        );
        // Ten minutes of quiet ends the warning, without a timer.
        assert!(r.warns(at(1299)));
        assert!(!r.warns(at(1300)));
        assert_eq!(
            r.refused(at(2000)),
            ["live feed refused a connection: wrong or missing token"]
        );
    }

    #[test]
    fn tokens_are_long_and_differ() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 24);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
