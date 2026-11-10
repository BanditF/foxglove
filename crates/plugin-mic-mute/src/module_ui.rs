//! Mic mute's Hideout tile and page (docs/hideout-plugin-modules-ux.md): the state as one word,
//! one Mute/Unmute button tied to the `toggle` action, and Try again after a refusal.

use foxglove_plugin::ui::{self, Button, Press, Tile};
use serde_json::Value;

use crate::screens::Mic;

/// The tile's state: the mic, and whether Windows refused the last change.
pub fn tile(mic: Mic, refused: bool) -> Tile {
    let sub = Some("Default communications mic".to_string());
    if refused {
        return Tile {
            // The change Windows refused: unmuting while muted, muting otherwise.
            value: if mic == Mic::Muted {
                "Couldn't unmute"
            } else {
                "Couldn't mute"
            }
            .into(),
            tone: "attention",
            sub: Some("Windows said no. Try again.".into()),
            button: Some(Button::new("Try again", Press::Event("retry"))),
            ..Tile::default()
        };
    }
    match mic {
        Mic::Unmuted => Tile {
            value: "Mic on".into(),
            tone: "neutral",
            sub,
            button: Some(Button::new("Mute", Press::Action("toggle"))),
            ..Tile::default()
        },
        Mic::Muted => Tile {
            value: "Muted".into(),
            tone: "live",
            sub,
            button: Some(Button::new("Unmute", Press::Action("toggle"))),
            ..Tile::default()
        },
        Mic::Unavailable => Tile {
            value: "No microphone".into(),
            tone: "off",
            sub: Some("Windows has no default mic".into()),
            ..Tile::default()
        },
        Mic::Unknown => Tile {
            value: "Can't tell".into(),
            tone: "attention",
            sub: Some("Windows didn't say if it's muted".into()),
            ..Tile::default()
        },
    }
}

/// The tile and the page, for `ui.register`.
pub fn panels(mic: Mic, refused: bool) -> Vec<Value> {
    let t = tile(mic, refused);
    let mut page = vec![ui::now_section(&t, vec![])];
    if mic == Mic::Unavailable {
        page.push(ui::section(
            "connect",
            "How to connect",
            vec![ui::markdown(
                "connect-steps",
                "Plug in a microphone, or pick one in Windows Sound settings, Recording tab.",
            )],
        ));
    }
    page.push(ui::section(
        "keyboard",
        "On your keyboard",
        vec![
            ui::label("keyboard-screen", "Info screen: MUTED while muted"),
            ui::label("keyboard-lights", "Lights: red while muted"),
        ],
    ));
    page.push(ui::section(
        "about",
        "About",
        vec![ui::markdown(
            "about-text",
            "This is Windows' mute for the default communications mic. Apps like Discord can \
             still mute themselves. While Mic mute isn't running, Windows keeps the mic as it was.",
        )],
    ));
    vec![
        ui::panel("tile", "tile", "Mic mute", "mic", t.tree()),
        ui::panel("page", "page", "Mic mute", "mic", ui::column("page", page)),
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
            "mic-mute.json",
            (&m.id, &m.name, &declared, &actions),
            vec![
                ("On", panels(Mic::Unmuted, false)),
                ("Muted", panels(Mic::Muted, false)),
                ("No microphone", panels(Mic::Unavailable, false)),
                ("Refused", panels(Mic::Muted, true)),
            ],
        );
    }
}
