//! OBS's Hideout tile and page (docs/hideout-plugin-modules-ux.md). It only listens, so the
//! tile has no Start or Stop: only "How to" while OBS can't be reached and "Enter password".

use foxglove_plugin::ui::{self, Button, Press, Tile};
use serde_json::Value;

use crate::obs::Status;

/// Where the link to OBS stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// OBS isn't open (the app watch says so).
    Closed,
    /// OBS is open but its WebSocket server didn't answer.
    Unreachable,
    /// OBS refused the password, or asks for one and none is set.
    NeedsPassword,
    On(Status),
}

pub fn tile(link: &Link) -> Tile {
    match link {
        Link::Closed => Tile {
            value: "OBS isn't open".into(),
            tone: "off",
            sub: Some("Shows here when you open it".into()),
            ..Tile::default()
        },
        Link::Unreachable => Tile {
            value: "Can't reach OBS".into(),
            tone: "attention",
            sub: Some("Turn on its WebSocket server".into()),
            button: Some(Button::new("How to", Press::Opens("page#connect"))),
            ..Tile::default()
        },
        Link::NeedsPassword => Tile {
            value: "Needs password".into(),
            tone: "attention",
            sub: Some("OBS asks for one".into()),
            button: Some(Button::new("Enter password", Press::Opens("settings"))),
            ..Tile::default()
        },
        Link::On(s) => {
            let (value, tone) = match (s.streaming, s.recording, s.paused) {
                (true, true, _) => ("Live · Recording", "live"),
                (true, false, _) => ("Live", "live"),
                (false, true, true) => ("Recording paused", "attention"),
                (false, true, false) => ("Recording", "live"),
                (false, false, _) => ("Off air", "neutral"),
            };
            Tile {
                value: value.into(),
                tone,
                sub: s.scene.as_ref().map(|sc| format!("Scene: {sc}")),
                ..Tile::default()
            }
        }
    }
}

/// The tile and the page, for `ui.register`. Scene names are cut to fit the tile's text cap.
pub fn panels(link: &Link) -> Vec<Value> {
    let mut t = tile(link);
    if let Some(sub) = &mut t.sub {
        if sub.len() > 60 {
            let mut end = 57;
            while !sub.is_char_boundary(end) {
                end -= 1;
            }
            sub.truncate(end);
            sub.push('…');
        }
    }
    let mut page = vec![ui::now_section(&t, vec![])];
    if matches!(link, Link::Unreachable | Link::NeedsPassword) {
        page.push(ui::section(
            "connect",
            "How to connect",
            vec![ui::markdown(
                "connect-steps",
                "1. In OBS, open Tools > WebSocket Server Settings.\n\
                 2. Tick Enable WebSocket server.\n\
                 3. Leave Server Port at 4455.\n\
                 4. If Enable Authentication is ticked, copy the password, then enter it in \
                 Settings below.\n\n\
                 Hideout connects by itself.",
            )],
        ));
    }
    page.push(ui::section(
        "keyboard",
        "On your keyboard",
        vec![
            ui::label("keyboard-screen", "Info screen: REC or LIVE and the scene"),
            ui::label("keyboard-lights", "Lights: red while recording or live"),
        ],
    ));
    page.push(ui::section(
        "about",
        "About",
        vec![ui::markdown(
            "about-text",
            "Hideout only listens to OBS. It never starts, stops or switches anything.",
        )],
    ));
    vec![
        ui::panel("tile", "tile", "OBS", "video", t.tree()),
        ui::panel("page", "page", "OBS", "video", ui::column("page", page)),
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
        let on = |streaming, recording, paused| {
            Link::On(Status {
                streaming,
                recording,
                paused,
                scene: Some("Gameplay".into()),
            })
        };
        foxglove_plugin::ui::check_vectors(
            "broadcast-status.json",
            (&m.id, &m.name, &declared, &actions),
            vec![
                ("Off air", panels(&on(false, false, false))),
                ("Recording", panels(&on(false, true, false))),
                ("Recording paused", panels(&on(false, true, true))),
                ("Live", panels(&on(true, false, false))),
                ("Live and recording", panels(&on(true, true, false))),
                ("OBS closed", panels(&Link::Closed)),
                ("Can't reach", panels(&Link::Unreachable)),
                ("Needs password", panels(&Link::NeedsPassword)),
            ],
        );
    }
}
