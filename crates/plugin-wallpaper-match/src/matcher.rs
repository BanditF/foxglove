//! What the lights should show, from the wallpapers that are connected: pure, for the plugin
//! loop and the tests.

use std::collections::BTreeMap;

use foxglove_plugin::Hsb;
use serde::Deserialize;

/// A message from a wallpaper (bandit-wallpapers `shared/match.js`, version 1).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Message {
    pub v: u32,
    #[serde(rename = "wallpaperId")]
    pub wallpaper_id: String,
    #[serde(default)]
    pub accent: String,
    #[serde(default)]
    pub paused: bool,
}

/// `#rrggbb` as the underglow's colour.
pub fn hsb(hex: &str) -> Option<Hsb> {
    let h = hex.strip_prefix('#').filter(|h| h.len() == 6)?;
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| f32::from(v) / 255.0);
    let (r, g, b) = (c(0)?, c(2)?, c(4)?);
    let max = r.max(g).max(b);
    let delta = max - r.min(g).min(b);
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    Some(Hsb { hue: (hue.round() as u16) % 360, saturation: (saturation * 100.0).round() as u8, brightness: (max * 100.0).round() as u8 })
}

/// The connected wallpapers (several monitors can each show one) and what each last said.
#[derive(Default)]
pub struct Matcher {
    /// Connection → (when it last spoke, wallpaper, colour); `None` while paused or silent.
    showing: BTreeMap<u64, Option<(u64, String, Hsb)>>,
    clock: u64,
}

impl Matcher {
    pub fn opened(&mut self, conn: u64) {
        self.showing.insert(conn, None);
    }

    pub fn closed(&mut self, conn: u64) {
        self.showing.remove(&conn);
    }

    pub fn message(&mut self, conn: u64, m: &Message) {
        self.clock += 1;
        let now = match hsb(&m.accent).filter(|_| !m.paused && m.v == 1) {
            Some(color) => Some((self.clock, m.wallpaper_id.clone(), color)),
            None => None,
        };
        self.showing.insert(conn, now);
    }

    /// The wallpaper and colour to show: the one that spoke last of those not paused.
    pub fn wanted(&self) -> Option<(String, Hsb)> {
        self.showing.values().flatten().max_by_key(|(at, _, _)| *at).map(|(_, w, c)| (w.clone(), *c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, accent: &str, paused: bool) -> Message {
        Message { v: 1, wallpaper_id: id.into(), accent: accent.into(), paused }
    }

    #[test]
    fn accents_become_colours() {
        assert_eq!(hsb("#ff0000"), Some(Hsb { hue: 0, saturation: 100, brightness: 100 }));
        assert_eq!(hsb("#e8742a").map(|c| c.hue), Some(23));
        assert_eq!(hsb("red"), None);
        assert_eq!(hsb("#12345"), None);
    }

    #[test]
    fn the_latest_wallpaper_that_isnt_paused_wins() {
        let mut m = Matcher::default();
        m.opened(1);
        assert_eq!(m.wanted(), None, "nothing said yet");
        m.message(1, &msg("fury-print", "#ff0000", false));
        m.opened(2);
        m.message(2, &msg("courier", "#00ff00", false));
        assert_eq!(m.wanted().unwrap().0, "courier");
        m.message(2, &msg("courier", "#00ff00", true));
        assert_eq!(m.wanted().unwrap().0, "fury-print", "the paused one gives way");
        m.closed(1);
        assert_eq!(m.wanted(), None, "the only one left is paused");
        m.message(2, &msg("courier", "not a colour", false));
        assert_eq!(m.wanted(), None);
    }
}
