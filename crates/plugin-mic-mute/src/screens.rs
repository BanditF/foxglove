//! What each microphone state looks like: Art's whole frames
//! (`assets/plugins/org.foxglove.mic-mute/panel.json`, docs/plugin-screens.md) and the
//! underglow cue.

use std::collections::BTreeMap;
use std::time::Duration;

use foxglove_plugin::{Hsb, LeasePriority};
use serde::Deserialize;

pub const W: u16 = 64;
pub const H: u16 = 52;
/// How long a held screen lives; it's re-sent at half that.
pub const HOLD_TTL: Duration = Duration::from_secs(60);
/// How long LIVE or `?` shows after a change.
pub const BRIEF: Duration = Duration::from_secs(3);
/// The white acknowledgement on unmute.
pub const FLASH: Duration = Duration::from_millis(600);
pub const WHITE: Hsb = Hsb { hue: 0, saturation: 0, brightness: 60 };
const RED: Hsb = Hsb { hue: 0, saturation: 100, brightness: 80 };
const AMBER: Hsb = Hsb { hue: 35, saturation: 100, brightness: 60 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mic {
    Muted,
    Unmuted,
    Unknown,
    Unavailable,
}

impl Mic {
    /// From `mic.state`; anything else is unknown, never muted.
    pub fn from_fact(s: &str) -> Mic {
        match s {
            "muted" => Mic::Muted,
            "unmuted" => Mic::Unmuted,
            "unavailable" => Mic::Unavailable,
            _ => Mic::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    Muted,
    Live,
    Unknown,
}

/// How a state is shown.
#[derive(Debug, PartialEq)]
pub struct Shown {
    /// The lights held while it lasts.
    pub light: Option<(Hsb, LeasePriority)>,
    pub frame: Option<Frame>,
    /// Keep the screen up (muted), or show it briefly.
    pub hold: bool,
}

impl Shown {
    pub fn of(mic: Mic) -> Shown {
        match mic {
            Mic::Muted => Shown { light: Some((RED, LeasePriority::High)), frame: Some(Frame::Muted), hold: true },
            Mic::Unmuted => Shown { light: None, frame: Some(Frame::Live), hold: false },
            Mic::Unknown => Shown { light: Some((AMBER, LeasePriority::Normal)), frame: Some(Frame::Unknown), hold: false },
            Mic::Unavailable => Shown { light: None, frame: None, hold: false },
        }
    }
}

#[derive(Deserialize)]
struct Sprite {
    w: usize,
    h: usize,
    bitmap: String,
}

#[derive(Deserialize)]
struct Assets {
    format: String,
    frames: BTreeMap<String, Sprite>,
}

const JSON: &str = include_str!("../../../assets/plugins/org.foxglove.mic-mute/panel.json");

/// The three frames, decoded.
pub struct Art {
    muted: Vec<u8>,
    live: Vec<u8>,
    unknown: Vec<u8>,
}

impl Art {
    pub fn load() -> Result<Art, String> {
        use base64::Engine;
        let a: Assets = serde_json::from_str(JSON).map_err(|e| e.to_string())?;
        if a.format != "foxglove-panel-assets" {
            return Err("not panel assets".into());
        }
        let frame = |name: &str| -> Result<Vec<u8>, String> {
            let s = a.frames.get(name).ok_or(format!("no {name} frame"))?;
            let b = base64::engine::general_purpose::STANDARD.decode(&s.bitmap).map_err(|e| e.to_string())?;
            if (s.w, s.h) != (W as usize, H as usize) || b.len() != s.w.div_ceil(8) * s.h {
                return Err(format!("the {name} frame isn't {W}x{H}"));
            }
            Ok(b)
        };
        Ok(Art { muted: frame("muted")?, live: frame("live")?, unknown: frame("unknown")? })
    }

    pub fn frame(&self, f: Frame) -> &[u8] {
        match f {
            Frame::Muted => &self.muted,
            Frame::Live => &self.live,
            Frame::Unknown => &self.unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_never_muted() {
        assert_eq!(Mic::from_fact("muted"), Mic::Muted);
        for odd in ["", "MUTED", "mixed", "error"] {
            assert_eq!(Mic::from_fact(odd), Mic::Unknown, "{odd}");
        }
    }

    #[test]
    fn each_state_has_its_cue() {
        assert_eq!(Shown::of(Mic::Muted).light, Some((RED, LeasePriority::High)));
        assert!(Shown::of(Mic::Muted).hold);
        assert_eq!(Shown::of(Mic::Unknown).light.map(|l| l.0), Some(AMBER));
        assert_eq!(Shown::of(Mic::Unavailable), Shown { light: None, frame: None, hold: false });
        assert!(Shown::of(Mic::Unmuted).light.is_none() && !Shown::of(Mic::Unmuted).hold);
    }

    #[test]
    fn the_frames_load_and_differ() {
        let art = Art::load().unwrap();
        assert_eq!(art.frame(Frame::Muted).len(), 416);
        assert_ne!(art.frame(Frame::Muted), art.frame(Frame::Live));
        assert_ne!(art.frame(Frame::Live), art.frame(Frame::Unknown));
        // The muted frame is the bright one (inverted).
        let lit = |f: &[u8]| f.iter().map(|b| b.count_ones()).sum::<u32>();
        assert!(lit(art.frame(Frame::Muted)) > lit(art.frame(Frame::Live)));
    }
}
