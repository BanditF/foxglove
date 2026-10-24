//! Whether to read colours, and from which device: started by iCUE's session connecting with
//! the chosen device present, stopped by the session ending, the device going, or the plugin
//! stopping (#283). Pure apart from the [`Sdk`] it asks, for the tests.

use foxglove_plugin::Hsb;

use crate::sdk::{Notice, Sdk};

#[derive(Default)]
pub struct Follow {
    /// Part of the model name to follow; empty: the first device with lights.
    pub wanted: String,
    session: bool,
    /// The device followed and its LED ids.
    device: Option<(String, Vec<u32>)>,
}

impl Follow {
    pub fn new(wanted: &str) -> Follow {
        Follow { wanted: wanted.trim().to_ascii_lowercase(), ..Follow::default() }
    }

    /// Whether colours are read now.
    pub fn following(&self) -> bool {
        self.session && self.device.is_some()
    }

    pub fn device(&self) -> Option<&str> {
        self.device.as_ref().map(|(id, _)| id.as_str())
    }

    /// Picks the device to follow, if the session is up and none is followed.
    fn pick(&mut self, sdk: &dyn Sdk) {
        if !self.session || self.device.is_some() {
            return;
        }
        let devices = sdk.devices().unwrap_or_default();
        let chosen = devices.iter().find(|d| d.leds > 0 && (self.wanted.is_empty() || d.model.to_ascii_lowercase().contains(&self.wanted)));
        if let Some(d) = chosen {
            match sdk.leds(&d.id) {
                Ok(leds) if !leds.is_empty() => self.device = Some((d.id.clone(), leds)),
                Ok(_) => {}
                Err(e) => eprintln!("icue-mirror: {e}"),
            }
        }
    }

    /// The user chose another device: picked now if the session is up.
    pub fn want(&mut self, wanted: &str, sdk: &dyn Sdk) {
        self.wanted = wanted.trim().to_ascii_lowercase();
        self.device = None;
        self.pick(sdk);
    }

    /// What iCUE said.
    pub fn notice(&mut self, n: &Notice, sdk: &dyn Sdk) {
        match n {
            Notice::Session(up) => {
                self.session = *up;
                if !up {
                    self.device = None;
                }
            }
            Notice::Device { id, connected: false } => {
                if self.device() == Some(id.as_str()) {
                    self.device = None;
                }
            }
            Notice::Device { connected: true, .. } => {}
        }
        self.pick(sdk);
    }

    /// The device's colour now: the average of its lit LEDs, weighted by opacity.
    pub fn read(&self, sdk: &dyn Sdk) -> Option<Hsb> {
        let (id, leds) = self.device.as_ref().filter(|_| self.session)?;
        let colors = sdk.colors(id, leds).map_err(|e| eprintln!("icue-mirror: {e}")).ok()?;
        average(&colors)
    }
}

/// The average colour, weighted by each LED's opacity; `None` for no LEDs or all transparent.
pub fn average(colors: &[(u8, u8, u8, u8)]) -> Option<Hsb> {
    let total: u32 = colors.iter().map(|c| u32::from(c.3)).sum();
    if total == 0 {
        return None;
    }
    let channel = |f: fn(&(u8, u8, u8, u8)) -> u8| colors.iter().map(|c| u32::from(f(c)) * u32::from(c.3)).sum::<u32>() as f32 / total as f32 / 255.0;
    let (r, g, b) = (channel(|c| c.0), channel(|c| c.1), channel(|c| c.2));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::Device;
    use std::cell::Cell;

    struct Fake {
        devices: Vec<Device>,
        reads: Cell<usize>,
    }

    impl Sdk for Fake {
        fn devices(&self) -> Result<Vec<Device>, String> {
            Ok(self.devices.clone())
        }
        fn leds(&self, _: &str) -> Result<Vec<u32>, String> {
            Ok(vec![1, 2])
        }
        fn colors(&self, _: &str, _: &[u32]) -> Result<Vec<(u8, u8, u8, u8)>, String> {
            self.reads.set(self.reads.get() + 1);
            Ok(vec![(255, 0, 0, 255), (255, 0, 0, 255)])
        }
    }

    fn fake() -> Fake {
        let dev = |id: &str, model: &str, leds| Device { id: id.into(), model: model.into(), leds };
        Fake { devices: vec![dev("{fan}", "LL120", 0), dev("{kb}", "K70 RGB PRO", 120), dev("{mouse}", "DARK CORE", 3)], reads: Cell::new(0) }
    }

    #[test]
    fn follows_only_while_the_session_and_the_device_are_there() {
        let sdk = fake();
        let mut f = Follow::new("");
        assert!(!f.following(), "nothing before iCUE connects");
        assert_eq!(f.read(&sdk), None);
        assert_eq!(sdk.reads.get(), 0, "no reads without a session");
        f.notice(&Notice::Session(true), &sdk);
        assert_eq!(f.device(), Some("{kb}"), "the first device with lights");
        assert_eq!(f.read(&sdk), Some(Hsb { hue: 0, saturation: 100, brightness: 100 }));
        // Another device going changes nothing; ours going stops it.
        f.notice(&Notice::Device { id: "{mouse}".into(), connected: false }, &sdk);
        assert!(f.following());
        f.notice(&Notice::Device { id: "{kb}".into(), connected: false }, &sdk);
        // It's picked again at once if iCUE still lists it; a fake that lists nothing stops.
        let empty = Fake { devices: vec![], reads: Cell::new(0) };
        f.notice(&Notice::Device { id: "{kb}".into(), connected: false }, &empty);
        let mut g = Follow::new("");
        g.notice(&Notice::Session(true), &empty);
        assert!(!g.following(), "no device, no reads");
        // The session ending stops it.
        f.notice(&Notice::Session(false), &sdk);
        assert!(!f.following());
        assert_eq!(f.read(&sdk), None);
    }

    #[test]
    fn the_wanted_model_is_followed() {
        let sdk = fake();
        let mut f = Follow::new("dark core");
        f.notice(&Notice::Session(true), &sdk);
        assert_eq!(f.device(), Some("{mouse}"));
        let mut none = Follow::new("scimitar");
        none.notice(&Notice::Session(true), &sdk);
        assert!(!none.following());
    }

    #[test]
    fn choosing_another_device_switches_within_the_session() {
        let sdk = fake();
        let mut f = Follow::new("");
        f.notice(&Notice::Session(true), &sdk);
        f.want("dark core", &sdk);
        assert_eq!(f.device(), Some("{mouse}"), "no new session event needed");
    }

    #[test]
    fn colours_average_by_opacity() {
        assert_eq!(average(&[(255, 0, 0, 255), (0, 0, 255, 0)]).map(|c| c.hue), Some(0), "a transparent LED doesn't count");
        assert_eq!(average(&[(0, 0, 0, 0)]), None);
        assert_eq!(average(&[]), None);
    }
}
