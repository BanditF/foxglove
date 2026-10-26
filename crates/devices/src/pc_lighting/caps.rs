//! What a lighting device can do, so the page can say it in a few chips and the app can turn
//! features on or off from it: how colour is addressed (none up to per-LED), onboard effects,
//! brightness, and screens (size, colour depth, frame-rate limit, images, animations, a live
//! stream).
//!
//! It comes from the catalog (`lighting-catalog.toml`), from a LampArray's own attributes
//! ([`LampArrayInfo`]), or from a keyboard's HELLO (Digitalis builds it there).

use serde::Deserialize;

/// How finely the device's colour can be set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Color {
    /// No lights, or none Foxglove knows of.
    #[default]
    None,
    /// Lights that only switch on and off.
    OnOff,
    /// One colour for the whole device.
    Single,
    /// A colour per zone (the count is the zone list's length).
    PerZone,
    /// A colour per key or LED.
    PerLed,
}

/// A screen on the device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Screen {
    pub width: Option<u16>,
    pub height: Option<u16>,
    /// Bits per pixel: 1 for a monochrome panel, 16 for RGB565, 24 for full colour.
    pub depth: Option<u8>,
    /// The most frames per second it takes.
    pub fps: Option<u16>,
    /// Still pictures.
    pub images: bool,
    /// Sprite or animation sets stored on the device.
    pub animations: bool,
    /// Live frames from the PC.
    pub stream: bool,
}

/// Everything a device can do with light and pictures.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightCaps {
    pub color: Color,
    /// Zones (per-zone) or lamps (per-LED); 0 when unknown.
    pub count: u32,
    /// Per-LED devices: each lamp has a physical position.
    pub positions: bool,
    /// Onboard effects by name.
    pub effects: Vec<String>,
    pub brightness: bool,
    /// The fastest the lights take updates, in frames per second.
    pub fps: Option<u16>,
    pub screens: Vec<Screen>,
}

/// A LampArray's attributes, as Windows (or its HID feature report) reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LampArrayInfo {
    pub lamp_count: u32,
    /// Some lamp has a non-zero position.
    pub positions: bool,
    /// The shortest update interval, in microseconds.
    pub min_update_us: u64,
    /// LampArrayKind: 1 keyboard, 2 mouse, 3 game controller, 4 peripheral, 5 scene,
    /// 6 notification, 7 chassis, 8 wearable, 9 furniture, 10 art.
    pub kind: u32,
}

impl LightCaps {
    /// A LampArray: per-LED colour (one lamp is one colour), Windows' brightness.
    pub fn from_lamp_array(info: LampArrayInfo) -> Self {
        let fps = (info.min_update_us > 0)
            .then(|| (1_000_000 / info.min_update_us).min(u64::from(u16::MAX)) as u16);
        LightCaps {
            color: match info.lamp_count {
                0 => Color::None,
                1 => Color::Single,
                _ => Color::PerLed,
            },
            count: info.lamp_count,
            positions: info.positions,
            brightness: info.lamp_count > 0,
            fps,
            ..Default::default()
        }
    }

    /// Whether the device takes any colour (single, per zone or per LED).
    pub fn takes_color(&self) -> bool {
        matches!(self.color, Color::Single | Color::PerZone | Color::PerLed)
    }

    /// Whether each zone or LED can have its own colour.
    pub fn per_zone(&self) -> bool {
        matches!(self.color, Color::PerZone | Color::PerLed)
    }

    /// Whether the lights can be turned off.
    pub fn switchable(&self) -> bool {
        self.color != Color::None
    }

    /// The page's chips: "Per-key RGB · 48 lamps", "Screen 160×68 · 1-bit · images & animations".
    /// `keys` says the LEDs are keys (a keyboard).
    pub fn chips(&self, keys: bool) -> Vec<String> {
        let mut chips = Vec::new();
        let plural =
            |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let light = match self.color {
            Color::None => None,
            Color::OnOff => Some("On/off light".to_string()),
            Color::Single => Some("Single-colour RGB".to_string()),
            Color::PerZone => Some(if self.count > 0 {
                format!("Per-zone RGB · {}", plural(self.count, "zone", "zones"))
            } else {
                "Per-zone RGB".to_string()
            }),
            Color::PerLed => {
                let what = if keys { "Per-key RGB" } else { "Per-LED RGB" };
                Some(if self.count > 0 {
                    format!("{what} · {}", plural(self.count, "lamp", "lamps"))
                } else {
                    what.to_string()
                })
            }
        };
        if let Some(mut light) = light {
            if let Some(fps) = self.fps {
                light.push_str(&format!(" · up to {fps} fps"));
            }
            chips.push(light);
        }
        if self.positions {
            chips.push("Lamp positions".into());
        }
        if !self.effects.is_empty() {
            chips.push(plural(
                self.effects.len() as u32,
                "onboard effect",
                "onboard effects",
            ));
        }
        if self.brightness {
            chips.push("Brightness".into());
        }
        for s in &self.screens {
            chips.push(screen_chip(s));
        }
        if chips.is_empty() {
            chips.push("No lights Foxglove knows of".into());
        }
        chips
    }
}

fn screen_chip(s: &Screen) -> String {
    let mut parts = vec![match (s.width, s.height) {
        (Some(w), Some(h)) => format!("Screen {w}×{h}"),
        _ => "Screen".to_string(),
    }];
    if let Some(d) = s.depth {
        parts.push(format!("{d}-bit"));
    }
    if let Some(fps) = s.fps {
        parts.push(format!("{fps} fps"));
    }
    let takes: Vec<&str> = [
        (s.images, "images"),
        (s.animations, "animations"),
        (s.stream, "live"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| *name)
    .collect();
    match takes.as_slice() {
        [] => {}
        [one] => parts.push((*one).to_string()),
        [rest @ .., last] => parts.push(format!("{} & {last}", rest.join(", "))),
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_say_it_simply() {
        let corne = LightCaps {
            color: Color::PerLed,
            count: 48,
            screens: vec![Screen {
                width: Some(160),
                height: Some(68),
                depth: Some(1),
                images: true,
                animations: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            corne.chips(true),
            vec![
                "Per-key RGB · 48 lamps",
                "Screen 160×68 · 1-bit · images & animations"
            ]
        );
        let case = LightCaps {
            color: Color::PerZone,
            count: 5,
            effects: vec!["breathing".into(), "colour cycle".into()],
            brightness: true,
            ..Default::default()
        };
        assert_eq!(
            case.chips(false),
            vec!["Per-zone RGB · 5 zones", "2 onboard effects", "Brightness"]
        );
        let lcd = Screen {
            images: true,
            animations: true,
            stream: true,
            fps: Some(30),
            ..Default::default()
        };
        assert_eq!(
            screen_chip(&lcd),
            "Screen · 30 fps · images, animations & live"
        );
        assert_eq!(
            LightCaps::default().chips(false),
            vec!["No lights Foxglove knows of"]
        );
    }

    #[test]
    fn lamp_arrays_become_caps() {
        let mouse = LightCaps::from_lamp_array(LampArrayInfo {
            lamp_count: 2,
            positions: true,
            min_update_us: 33_000,
            kind: 2,
        });
        assert_eq!(mouse.color, Color::PerLed);
        assert_eq!(
            mouse.chips(false),
            vec![
                "Per-LED RGB · 2 lamps · up to 30 fps",
                "Lamp positions",
                "Brightness"
            ]
        );
        assert!(mouse.per_zone() && mouse.takes_color());
        let one = LightCaps::from_lamp_array(LampArrayInfo {
            lamp_count: 1,
            ..Default::default()
        });
        assert_eq!(one.color, Color::Single);
        assert!(!one.per_zone());
        let none = LightCaps::from_lamp_array(LampArrayInfo::default());
        assert!(!none.switchable());
    }
}
