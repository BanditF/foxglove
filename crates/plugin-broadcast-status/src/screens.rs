//! What each state looks like: Art's whole frames with the scene name drawn in
//! (`assets/plugins/org.foxglove.broadcast-status/panel.json`, docs/plugin-screens.md), and the
//! underglow cue.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use foxglove_plugin::{Hsb, LeasePriority};
use serde::Deserialize;

use crate::obs::Status;

pub const W: u16 = 64;
pub const H: u16 = 52;
/// How long READY or NOT CONNECTED shows after a change.
pub const BRIEF: Duration = Duration::from_secs(3);
/// The scene name's length on the panel.
const LABEL_MAX: usize = 15;
const RED: Hsb = Hsb {
    hue: 0,
    saturation: 100,
    brightness: 80,
};
const AMBER: Hsb = Hsb {
    hue: 35,
    saturation: 100,
    brightness: 60,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disconnected,
    Idle,
    Recording,
    Paused,
    Streaming,
    Both,
}

impl State {
    pub fn of(connected: bool, s: &Status) -> State {
        match (connected, s.streaming, s.recording, s.paused) {
            (false, ..) => State::Disconnected,
            (_, true, true, _) => State::Both,
            (_, true, false, _) => State::Streaming,
            (_, false, true, true) => State::Paused,
            (_, false, true, false) => State::Recording,
            _ => State::Idle,
        }
    }

    /// Its frame in the assets.
    fn frame(self) -> &'static str {
        match self {
            State::Disconnected => "disconnected",
            State::Idle => "idle",
            State::Recording => "recording",
            State::Paused => "recording-paused",
            State::Streaming => "streaming",
            State::Both => "both",
        }
    }

    /// The lights held while it lasts: red on air (high priority while streaming), amber
    /// while a recording is paused.
    pub fn light(self) -> Option<(Hsb, LeasePriority)> {
        match self {
            State::Streaming | State::Both => Some((RED, LeasePriority::High)),
            State::Recording => Some((RED, LeasePriority::Normal)),
            State::Paused => Some((AMBER, LeasePriority::Normal)),
            State::Idle | State::Disconnected => None,
        }
    }

    /// Keep the screen up while something runs; READY and NOT CONNECTED show briefly.
    pub fn hold(self) -> bool {
        !matches!(self, State::Idle | State::Disconnected)
    }
}

#[derive(Deserialize)]
struct Sprite {
    w: usize,
    h: usize,
    bitmap: String,
}

#[derive(Deserialize)]
struct Label {
    x: i32,
    y: i32,
    ink: BTreeMap<String, u8>,
}

#[derive(Deserialize)]
struct Layout {
    label: Label,
}

#[derive(Deserialize)]
struct Font {
    advance: i32,
    glyphs: BTreeMap<char, Sprite>,
}

#[derive(Deserialize)]
struct Assets {
    format: String,
    frames: BTreeMap<String, Sprite>,
    layout: Layout,
    font: Font,
}

const JSON: &str = include_str!("../../../assets/plugins/org.foxglove.broadcast-status/panel.json");

/// A decoded bitmap: rows MSB-first, each padded to a byte.
struct Bits {
    w: usize,
    h: usize,
    bits: Vec<u8>,
}

impl Bits {
    fn decode(s: &Sprite) -> Result<Bits, String> {
        let bits = STANDARD.decode(&s.bitmap).map_err(|e| e.to_string())?;
        if bits.len() != s.w.div_ceil(8) * s.h {
            return Err(format!("a {}x{} bitmap of {} bytes", s.w, s.h, bits.len()));
        }
        Ok(Bits {
            w: s.w,
            h: s.h,
            bits,
        })
    }

    fn get(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.w.div_ceil(8) + x / 8] & (0x80 >> (x % 8)) != 0
    }
}

pub struct Art {
    frames: BTreeMap<String, Vec<u8>>,
    glyphs: BTreeMap<char, Bits>,
    advance: i32,
    label: (i32, i32),
    ink: BTreeMap<String, u8>,
}

impl Art {
    pub fn load() -> Result<Art, String> {
        let a: Assets = serde_json::from_str(JSON).map_err(|e| e.to_string())?;
        if a.format != "foxglove-panel-assets" {
            return Err("not panel assets".into());
        }
        let mut frames = BTreeMap::new();
        for s in [
            State::Disconnected,
            State::Idle,
            State::Recording,
            State::Paused,
            State::Streaming,
            State::Both,
        ] {
            let sprite = a
                .frames
                .get(s.frame())
                .ok_or(format!("no {} frame", s.frame()))?;
            let b = Bits::decode(sprite)?;
            if (b.w, b.h) != (W as usize, H as usize) {
                return Err(format!("the {} frame isn't {W}x{H}", s.frame()));
            }
            frames.insert(s.frame().to_string(), b.bits);
        }
        let glyphs = a
            .font
            .glyphs
            .iter()
            .map(|(c, s)| Ok((*c, Bits::decode(s)?)))
            .collect::<Result<_, String>>()?;
        Ok(Art {
            frames,
            glyphs,
            advance: a.font.advance,
            label: (a.layout.label.x, a.layout.label.y),
            ink: a.layout.label.ink,
        })
    }

    /// The state's frame, with the scene name centred at the label (not on NOT CONNECTED).
    pub fn frame(&self, state: State, scene: Option<&str>) -> Vec<u8> {
        let mut out = self.frames[state.frame()].clone();
        let (Some(scene), Some(&ink)) = (
            scene.filter(|_| state != State::Disconnected),
            self.ink.get(state.frame()),
        ) else {
            return out;
        };
        let text: Vec<char> = scene
            .trim()
            .chars()
            .map(|c| c.to_ascii_uppercase())
            .map(|c| if self.glyphs.contains_key(&c) { c } else { '?' })
            .take(LABEL_MAX)
            .collect();
        let width = text.len() as i32 * self.advance - 1;
        let (mut x, y) = (self.label.0 - width / 2, self.label.1);
        let stride = (W as usize).div_ceil(8);
        for c in text {
            let Some(g) = self.glyphs.get(&c) else {
                continue;
            };
            for gy in 0..g.h {
                for gx in 0..g.w {
                    let (px, py) = (x + gx as i32, y + gy as i32);
                    if !g.get(gx, gy)
                        || !(0..W as i32).contains(&px)
                        || !(0..H as i32).contains(&py)
                    {
                        continue;
                    }
                    let (i, bit) = (
                        py as usize * stride + px as usize / 8,
                        0x80 >> (px as usize % 8),
                    );
                    if ink == 1 {
                        out[i] |= bit;
                    } else {
                        out[i] &= !bit;
                    }
                }
            }
            x += self.advance;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(streaming: bool, recording: bool, paused: bool) -> Status {
        Status {
            streaming,
            recording,
            paused,
            scene: None,
        }
    }

    #[test]
    fn each_state_and_its_cue() {
        assert_eq!(
            State::of(false, &st(true, true, false)),
            State::Disconnected
        );
        assert_eq!(State::of(true, &st(false, false, false)), State::Idle);
        assert_eq!(State::of(true, &st(false, true, false)), State::Recording);
        assert_eq!(State::of(true, &st(false, true, true)), State::Paused);
        assert_eq!(State::of(true, &st(true, false, false)), State::Streaming);
        assert_eq!(State::of(true, &st(true, true, true)), State::Both);
        assert_eq!(State::Streaming.light(), Some((RED, LeasePriority::High)));
        assert_eq!(State::Recording.light(), Some((RED, LeasePriority::Normal)));
        assert_eq!(State::Paused.light().map(|l| l.0), Some(AMBER));
        assert!(State::Idle.light().is_none() && State::Disconnected.light().is_none());
        assert!(State::Both.hold() && !State::Idle.hold() && !State::Disconnected.hold());
    }

    #[test]
    fn the_frames_load_and_the_scene_is_drawn() {
        let art = Art::load().unwrap();
        let lit = |f: &[u8]| f.iter().map(|b| b.count_ones()).sum::<u32>();
        let streaming = art.frame(State::Streaming, None);
        assert_eq!(streaming.len(), 416);
        // Streaming is the bright one.
        assert!(lit(&streaming) > lit(&art.frame(State::Recording, None)));
        // The scene goes in the frame's ink: lit on REC, dark on the bright LIVE.
        let rec = art.frame(State::Recording, None);
        assert!(lit(&art.frame(State::Recording, Some("Gameplay"))) > lit(&rec));
        assert!(lit(&art.frame(State::Streaming, Some("Gameplay"))) < lit(&streaming));
        // Only below the label line.
        let named = art.frame(State::Recording, Some("Gameplay"));
        assert_eq!(named[..47 * 8], rec[..47 * 8]);
        // Long names are cut; unknown characters become ?; NOT CONNECTED has no label.
        assert_eq!(
            art.frame(State::Idle, Some("A VERY LONG SCENE NAME")),
            art.frame(State::Idle, Some("A VERY LONG SCENE"))
        );
        assert_ne!(
            art.frame(State::Idle, Some("é")),
            art.frame(State::Idle, None)
        );
        assert_eq!(
            art.frame(State::Disconnected, Some("Gameplay")),
            art.frame(State::Disconnected, None)
        );
    }
}
