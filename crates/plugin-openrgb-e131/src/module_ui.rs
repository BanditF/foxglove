//! OpenRGB's Hideout tile and page (docs/hideout-plugin-modules-ux.md). A busy port is a state
//! with Try again, not an exit (contract §7.7, S6).

use std::time::Duration;

use foxglove_plugin::ui::{self, Button, Press, Tile};
use serde_json::Value;

/// Where the colour feed stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Feed {
    /// Nothing received yet (`ever = false`: first run) or not since the last stream ended,
    /// `ago` before now (`None` until the tile is on screen to show it).
    Waiting {
        ever: bool,
        ago: Option<Duration>,
    },
    Following,
    /// Another program holds the port.
    PortBusy {
        port: u16,
    },
}

pub fn tile(f: &Feed) -> Tile {
    match f {
        Feed::Following => Tile {
            value: "Following".into(),
            tone: "active",
            sub: Some("OpenRGB on this PC".into()),
            ..Tile::default()
        },
        Feed::Waiting { ever: false, .. } => Tile {
            value: "Waiting for OpenRGB".into(),
            tone: "attention",
            sub: Some("Add Foxglove in OpenRGB".into()),
            button: Some(Button::new("How to connect", Press::Opens("page#connect"))),
            ..Tile::default()
        },
        Feed::Waiting { ever: true, ago } => Tile {
            value: "Waiting for OpenRGB".into(),
            tone: "off",
            sub: Some(match ago {
                Some(age) => format!("Last colours {} ago", age_words(*age)),
                None => "Follows again when OpenRGB sends".into(),
            }),
            ..Tile::default()
        },
        Feed::PortBusy { port } => Tile {
            value: format!("Port {port} is busy"),
            tone: "attention",
            sub: Some("Another app uses it".into()),
            button: Some(Button::new("Try again", Press::Event("retry"))),
            ..Tile::default()
        },
    }
}

/// An age as the quiet caption says it: "12 s", "3 min", "2 h" (the hub's "Last seen" steps).
pub fn age_words(age: Duration) -> String {
    let s = age.as_secs();
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", s / 60)
    } else {
        format!("{} h", s / 3600)
    }
}

/// When the caption for `age` next reads differently: the next whole second, minute or hour.
pub fn next_change(age: Duration) -> Duration {
    let step = match age.as_secs() {
        0..60 => 1,
        60..3600 => 60,
        _ => 3600,
    };
    Duration::from_secs((age.as_secs() / step + 1) * step).saturating_sub(age)
}

/// The tile and the page, for `ui.register`.
pub fn panels(f: &Feed) -> Vec<Value> {
    let t = tile(f);
    let mut page = vec![ui::now_section(&t, vec![])];
    if matches!(f, Feed::Waiting { ever: false, .. }) {
        page.push(ui::section(
            "connect",
            "How to connect",
            vec![ui::markdown(
                "connect-steps",
                "1. In OpenRGB, open Settings > E1.31 Devices.\n\
                 2. Click Add.\n\
                 3. Set IP to 127.0.0.1 and Universe to 1.\n\
                 4. Save, then click Rescan Devices.",
            )],
        ));
    }
    page.push(ui::section(
        "keyboard",
        "On your keyboard",
        vec![ui::label(
            "keyboard-lights",
            "Lights: OpenRGB's colour while it sends; your own lighting otherwise",
        )],
    ));
    page.push(ui::section(
        "about",
        "About",
        vec![ui::markdown(
            "about-text",
            "Lets OpenRGB colour your keyboard's lights. Listens on this PC only; nothing leaves \
             your PC.",
        )],
    ));
    vec![
        ui::panel("tile", "tile", "OpenRGB", "lights", t.tree()),
        ui::panel(
            "page",
            "page",
            "OpenRGB",
            "lights",
            ui::column("page", page),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_contract::module::{Context, Module};

    #[test]
    fn the_panels_match_the_hub_vectors() {
        let m = Module::parse(include_str!("../module.toml"), &Context::default()).unwrap();
        let declared: Vec<String> = m.panels.iter().map(|p| p.id.clone()).collect();
        let actions: Vec<String> = m.actions.iter().map(|a| a.decl.id.clone()).collect();
        foxglove_plugin::ui::check_vectors(
            "openrgb.json",
            (&m.id, &m.name, &declared, &actions),
            vec![
                ("Following", panels(&Feed::Following)),
                (
                    "First run",
                    panels(&Feed::Waiting {
                        ever: false,
                        ago: None,
                    }),
                ),
                (
                    "Quiet",
                    panels(&Feed::Waiting {
                        ever: true,
                        ago: Some(Duration::from_secs(180)),
                    }),
                ),
                ("Port busy", panels(&Feed::PortBusy { port: 5568 })),
            ],
        );
    }

    #[test]
    fn e131_shows_only_in_the_steps() {
        for f in [
            Feed::Following,
            Feed::Waiting {
                ever: false,
                ago: None,
            },
            Feed::PortBusy { port: 5568 },
        ] {
            let text = serde_json::to_string(&panels(&f)).unwrap();
            let steps = text.find("connect-steps").map_or(0, |_| 1);
            assert_eq!(text.matches("E1.31").count(), steps, "{f:?}");
        }
    }

    #[test]
    fn the_quiet_caption_says_the_age_and_wakes_only_when_it_changes() {
        let quiet = |s: u64| {
            tile(&Feed::Waiting {
                ever: true,
                ago: Some(Duration::from_secs(s)),
            })
            .sub
        };
        assert_eq!(quiet(12).as_deref(), Some("Last colours 12 s ago"));
        assert_eq!(quiet(180).as_deref(), Some("Last colours 3 min ago"));
        assert_eq!(quiet(7300).as_deref(), Some("Last colours 2 h ago"));
        assert_eq!(
            tile(&Feed::Waiting {
                ever: true,
                ago: None
            })
            .sub
            .as_deref(),
            Some("Follows again when OpenRGB sends")
        );
        let ms = Duration::from_millis;
        assert_eq!(next_change(ms(12_300)), ms(700));
        assert_eq!(next_change(ms(59_000)), ms(1_000));
        assert_eq!(next_change(ms(61_000)), ms(59_000));
        assert_eq!(next_change(ms(3_600_000)), ms(3_600_000));
    }
}
