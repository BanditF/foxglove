//! Corsair iCUE's Hideout tile and page (docs/hideout-plugin-modules-ux.md). A missing SDK file
//! is a "Needs setup" state with the steps on the page, not a crash (contract §7.7, S6); the
//! file setting takes effect when it changes.

use foxglove_plugin::ui::{self, Button, Press, Tile};
use serde_json::Value;

/// Where the mirror stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Mirror {
    /// No SDK file set (`bad = false`) or the file set isn't Corsair's SDK (`bad = true`).
    NeedsSetup { bad: bool },
    /// No session with iCUE: it isn't running (or refused the SDK).
    Closed,
    /// A session but no device to follow; `wanted` is the device filter.
    NoDevice { wanted: String },
    /// Following this device.
    Following { model: String },
}

pub fn tile(m: &Mirror) -> Tile {
    match m {
        Mirror::NeedsSetup { bad } => Tile {
            value: "Needs setup".into(),
            tone: "attention",
            sub: Some(if *bad {
                "That file isn't Corsair's iCUE SDK".into()
            } else {
                "Corsair's iCUE SDK file".into()
            }),
            button: Some(Button::new("Set up", Press::Opens("page#connect"))),
            ..Tile::default()
        },
        Mirror::Closed => Tile {
            value: "iCUE isn't open".into(),
            tone: "off",
            sub: Some("Follows again when it opens".into()),
            ..Tile::default()
        },
        Mirror::NoDevice { wanted } => Tile {
            value: "No device found".into(),
            tone: "attention",
            sub: Some(if wanted.is_empty() {
                "No Corsair device with lights".into()
            } else {
                let mut w = wanted.clone();
                if w.len() > 32 {
                    let mut end = 32;
                    while !w.is_char_boundary(end) {
                        end -= 1;
                    }
                    w.truncate(end);
                }
                format!("Nothing matches \"{w}\"")
            }),
            button: Some(Button::new("Change device", Press::Opens("settings"))),
            ..Tile::default()
        },
        Mirror::Following { model } => Tile {
            value: "Following".into(),
            tone: "active",
            sub: Some(model.chars().take(60).collect()),
            ..Tile::default()
        },
    }
}

/// The tile and the page, for `ui.register`.
pub fn panels(m: &Mirror) -> Vec<Value> {
    let t = tile(m);
    let mut page = vec![ui::now_section(&t, vec![])];
    if matches!(m, Mirror::NeedsSetup { .. } | Mirror::Closed) {
        page.push(ui::section(
            "connect",
            "How to connect",
            vec![
                ui::markdown(
                    "connect-steps",
                    "1. Download the iCUE SDK from Corsair's download page (opens corsair.com in \
                     your browser).\n\
                     2. Unzip it.\n\
                     3. Click Choose file in Settings below and pick iCUESDK.x64_2019.dll.\n\
                     4. In iCUE, open Settings and turn on SDK.",
                ),
                ui::button(
                    "connect-file",
                    &Button::new("Choose file", Press::Opens("settings")),
                ),
            ],
        ));
    }
    page.push(ui::section(
        "keyboard",
        "On your keyboard",
        vec![ui::label(
            "keyboard-lights",
            "Lights: your underglow follows the Corsair device's colours",
        )],
    ));
    page.push(ui::section(
        "about",
        "About",
        vec![ui::markdown(
            "about-text",
            "Mirroring only: Hideout reads colours and never takes control of Corsair lighting. \
             Foxglove ships no Corsair files.",
        )],
    ));
    vec![
        ui::panel("tile", "tile", "Corsair iCUE", "palette", t.tree()),
        ui::panel(
            "page",
            "page",
            "Corsair iCUE",
            "palette",
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
            "icue.json",
            (&m.id, &m.name, &declared, &actions),
            vec![
                (
                    "Following",
                    panels(&Mirror::Following {
                        model: "K70 RGB PRO".into(),
                    }),
                ),
                ("Needs setup", panels(&Mirror::NeedsSetup { bad: false })),
                ("Wrong file", panels(&Mirror::NeedsSetup { bad: true })),
                ("iCUE closed", panels(&Mirror::Closed)),
                (
                    "No device",
                    panels(&Mirror::NoDevice {
                        wanted: "k95".into(),
                    }),
                ),
            ],
        );
    }

    #[test]
    fn the_sdk_path_is_a_file_setting_in_hideout_only() {
        let m = Module::parse(include_str!("../module.toml"), &Context::default()).unwrap();
        let s = m
            .settings
            .iter()
            .find(|s| s.decl().name == "sdk-dll")
            .unwrap();
        assert_eq!(serde_json::to_value(s).unwrap()["kind"], "file");
        assert!(include_str!("../plugin.toml").contains("kind = \"text\""));
    }
}
