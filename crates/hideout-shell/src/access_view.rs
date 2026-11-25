// SPDX-License-Identifier: MIT
//! The hub's **Access** page: one list of the things on this PC that apps can touch, and under
//! each thing every app that asked for it, with its state in words. It is a second view of the
//! apps' own Permissions sections ([`crate::permission_dialog::Section`]) and nothing more: it
//! grants nothing. **Turn off** is the section's own Turn off, and **Change…** opens that app's
//! permission dialog, where Allow is still the user's click. Things with a sensitive item come
//! first and start open; the rest start folded.
//!
//! Which thing a permission belongs to is the table in [`thing_of`]: prefixes of the published
//! permission strings, so the page names no app and no vendor.

use crate::permission_dialog::{Group, ItemWord, Section, CHANGE, TURN_OFF};
use crate::theme::{Rgb, Tokens};
use eframe::egui::{self, RichText};

pub const ID: &str = "hub.access";
pub const TITLE: &str = "Access";
/// A published icon name (the lock).
pub const ICON: &str = "lock";

pub const INTRO: &str = "What your apps can touch on this PC, by thing. These are the same \
     switches as on each app: turning one off here turns it off there.";
pub const SENSITIVE: &str = "Sensitive";
pub const EMPTY: &str = "No app has asked for anything yet.";

const STATE_W: f32 = 130.0;
const BUTTON_W: f32 = 84.0;

fn c(rgb: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// A thing on the PC that an app can touch, in the page's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Thing {
    Keys,
    Microphone,
    Network,
    Files,
    Ai,
    Keyboard,
    PcLights,
    Programs,
    Windows,
    Tray,
    BetweenApps,
    Other,
}

impl Thing {
    pub fn title(self) -> &'static str {
        match self {
            Thing::Keys => "Keys in every app",
            Thing::Microphone => "Microphone and sound",
            Thing::Network => "Network",
            Thing::Files => "Files and saved secrets",
            Thing::Ai => "AI",
            Thing::Keyboard => "Keyboard, its lights and screen",
            Thing::PcLights => "PC lights",
            Thing::Programs => "Other programs",
            Thing::Windows => "Windows on screen",
            Thing::Tray => "Tray and notifications",
            Thing::BetweenApps => "Status and questions between apps",
            Thing::Other => "Other",
        }
    }
}

/// Starts of permission strings and the thing each belongs to; the first match wins. A
/// `provide:` item is matched by what it provides.
const THINGS: &[(&str, Thing)] = &[
    ("key.events", Thing::Keys),
    ("audio.", Thing::Microphone),
    ("net", Thing::Network),
    ("files", Thing::Files),
    ("secrets", Thing::Files),
    ("ai.", Thing::Ai),
    ("hid.raw", Thing::Keyboard),
    ("device.hid:", Thing::Keyboard),
    ("devices.", Thing::Keyboard),
    ("lighting.", Thing::Keyboard),
    ("lights.", Thing::Keyboard),
    ("screens.", Thing::Keyboard),
    ("health.", Thing::Keyboard),
    ("pc_lighting.", Thing::PcLights),
    ("apps.", Thing::Programs),
    ("ui.window", Thing::Windows),
    ("overlay.", Thing::Windows),
    ("tray.", Thing::Tray),
    ("hub.notify", Thing::Tray),
    ("facts.", Thing::BetweenApps),
    ("channel.", Thing::BetweenApps),
];

/// The thing a permission string belongs to.
pub fn thing_of(raw: &str) -> Thing {
    let (what, provided) = match raw.strip_prefix("provide:") {
        Some(m) => (m, true),
        None => (raw, false),
    };
    THINGS
        .iter()
        .find(|(p, _)| what.starts_with(p))
        .map(|(_, t)| *t)
        .unwrap_or(if provided {
            Thing::BetweenApps
        } else {
            Thing::Other
        })
}

/// One app's item under a thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessRow {
    /// The app's id and its name.
    pub app: String,
    pub name: String,
    pub raw: String,
    pub line: String,
    pub word: ItemWord,
    pub sensitive: bool,
}

/// A thing and every app's item on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThingGroup {
    pub thing: Thing,
    pub rows: Vec<AccessRow>,
}

impl ThingGroup {
    /// Any item here is a sensitive one: the group comes first and starts open.
    pub fn sensitive(&self) -> bool {
        self.rows.iter().any(|r| r.sensitive)
    }

    pub fn allowed(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.word == ItemWord::Allowed)
            .count()
    }

    fn apps(&self) -> usize {
        let mut ids: Vec<&str> = self.rows.iter().map(|r| r.app.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// "2 apps, 3 allowed" (also the fold's accessible name, after the thing's title).
    pub fn summary(&self) -> String {
        let apps = match self.apps() {
            1 => "1 app".to_string(),
            n => format!("{n} apps"),
        };
        match self.allowed() {
            0 => format!("{apps}, nothing allowed"),
            n => format!("{apps}, {n} allowed"),
        }
    }
}

/// The apps' sections as things: sensitive things first, then the page's order; inside a thing
/// by app name. Items that can't be allowed are left out (no app can touch them).
pub fn group(sections: &[Section]) -> Vec<ThingGroup> {
    let mut out: Vec<ThingGroup> = Vec::new();
    for s in sections {
        for i in &s.items {
            if i.word == ItemWord::CantBeAllowed {
                continue;
            }
            let thing = thing_of(&i.raw);
            let row = AccessRow {
                app: s.id.clone(),
                name: s.name.clone(),
                raw: i.raw.clone(),
                line: i.line.clone(),
                word: i.word,
                sensitive: i.group == Group::Sensitive,
            };
            match out.iter_mut().find(|g| g.thing == thing) {
                Some(g) => g.rows.push(row),
                None => out.push(ThingGroup {
                    thing,
                    rows: vec![row],
                }),
            }
        }
    }
    for g in &mut out {
        g.rows
            .sort_by(|a, b| (&a.name, &a.app, &a.raw).cmp(&(&b.name, &b.app, &b.raw)));
    }
    out.sort_by_key(|g| (!g.sensitive(), g.thing));
    out
}

/// What the user pressed on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccessAsk {
    /// Turn off one allowed item of an app.
    TurnOff { app: String, raw: String },
    /// Open the app's permission dialog.
    Change { app: String },
}

fn word_colour(t: &Tokens, w: ItemWord) -> egui::Color32 {
    c(match w {
        ItemWord::Allowed => t.ok,
        ItemWord::Off => t.text_muted,
        ItemWord::Waits => t.attention,
        ItemWord::CantBeAllowed => t.text,
    })
}

fn cell<R>(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(w, crate::kit::CONTROL),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(w);
            add(ui)
        },
    )
    .inner
}

fn row_ui(ui: &mut egui::Ui, t: &Tokens, g: &ThingGroup, r: &AccessRow) -> Option<AccessAsk> {
    let mut out = None;
    let line_w = (ui.available_width() - STATE_W - BUTTON_W - 16.0).max(160.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let l = ui
            .allocate_ui_with_layout(
                egui::vec2(line_w, crate::kit::CONTROL),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(line_w);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        RichText::new(&r.name)
                            .size(crate::kit::SMALL)
                            .color(c(t.text_muted)),
                    );
                    ui.add(egui::Label::new(RichText::new(&r.line).color(c(t.text))).wrap())
                },
            )
            .inner;
        let said = format!(
            "{}: {}, {}, {}",
            g.thing.title(),
            r.name,
            r.line,
            r.word.word()
        );
        ui.ctx().accesskit_node_builder(l.id, |n| n.set_label(said));
        cell(ui, STATE_W, |ui| {
            ui.label(
                RichText::new(r.word.word())
                    .strong()
                    .color(word_colour(t, r.word)),
            );
        });
        cell(ui, BUTTON_W, |ui| {
            if r.word == ItemWord::Allowed {
                let b = ui.button(TURN_OFF);
                let said = format!("{TURN_OFF}: {}, {}", r.name, r.line);
                ui.ctx().accesskit_node_builder(b.id, |n| n.set_label(said));
                if b.clicked() {
                    out = Some(AccessAsk::TurnOff {
                        app: r.app.clone(),
                        raw: r.raw.clone(),
                    });
                }
            } else {
                let b = ui.button(CHANGE);
                let said = format!("Change what {} is allowed", r.name);
                ui.ctx().accesskit_node_builder(b.id, |n| n.set_label(said));
                if b.clicked() {
                    out = Some(AccessAsk::Change { app: r.app.clone() });
                }
            }
        });
    });
    out
}

/// Draws the page. `note` is a line about the last action (a turn-off that couldn't be saved).
pub fn access_ui(
    ui: &mut egui::Ui,
    t: &Tokens,
    groups: &[ThingGroup],
    note: Option<&str>,
) -> Option<AccessAsk> {
    let mut out = None;
    ui.set_max_width(crate::kit::COLUMN);
    ui.add(egui::Label::new(RichText::new(INTRO).color(c(t.text_muted))).wrap());
    ui.add_space(crate::kit::GAP);
    if let Some(n) = note {
        ui.label(RichText::new(n).color(c(t.attention)));
        ui.add_space(crate::kit::GAP);
    }
    if groups.is_empty() {
        ui.add_space(crate::kit::GAP);
        ui.label(RichText::new(EMPTY).color(c(t.text)));
        return None;
    }
    for g in groups {
        let mut frame = crate::kit::card(t);
        if g.sensitive() {
            frame = frame.stroke(egui::Stroke::new(1.0_f32, c(t.danger)));
        }
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            let head = egui::CollapsingHeader::new(
                RichText::new(g.thing.title()).strong().color(c(t.text)),
            )
            .id_salt(("hideout.access", g.thing))
            .default_open(g.sensitive())
            .show(ui, |ui| {
                for (k, r) in g.rows.iter().enumerate() {
                    if k > 0 {
                        ui.add_space(4.0);
                    }
                    if let Some(a) = ui.push_id(k, |ui| row_ui(ui, t, g, r)).inner {
                        out = Some(a);
                    }
                }
            });
            // Sensitive in words, never by the border's colour alone.
            let summary = if g.sensitive() {
                format!("{SENSITIVE} \u{b7} {}", g.summary())
            } else {
                g.summary()
            };
            let said = format!("{}, {summary}", g.thing.title());
            ui.ctx()
                .accesskit_node_builder(head.header_response.id, |n| n.set_label(said));
            ui.label(
                RichText::new(summary)
                    .size(crate::kit::SMALL)
                    .color(c(t.text_muted)),
            );
        });
        ui.add_space(crate::kit::GAP);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission_dialog::SectionItem;

    fn section(id: &str, name: &str, items: &[(&str, Group, ItemWord)]) -> Section {
        Section {
            id: id.into(),
            name: name.into(),
            items: items
                .iter()
                .map(|(raw, group, word)| SectionItem {
                    raw: (*raw).into(),
                    line: format!("line {raw}"),
                    group: *group,
                    word: *word,
                })
                .collect(),
            allowed_on: None,
            history: Vec::new(),
            kept_void: None,
            keep: None,
            can_change: true,
            note: None,
        }
    }

    #[test]
    fn a_permission_goes_under_its_thing() {
        for (raw, thing) in [
            ("key.events", Thing::Keys),
            ("audio.inputs.mute", Thing::Microphone),
            ("net", Thing::Network),
            ("net.connect:127.0.0.1:4455", Thing::Network),
            ("files", Thing::Files),
            ("secrets.ref:obs", Thing::Files),
            ("ai.ask:local", Thing::Ai),
            ("device.hid:keyboard", Thing::Keyboard),
            ("lighting.write", Thing::Keyboard),
            ("screens.write", Thing::Keyboard),
            ("provide:lights.page", Thing::Keyboard),
            ("provide:health.read", Thing::Keyboard),
            ("provide:pc_lighting.set_zone", Thing::PcLights),
            ("apps.watch:obs64.exe", Thing::Programs),
            ("provide:overlay.set_shown", Thing::Windows),
            ("ui.window", Thing::Windows),
            ("provide:tray.panel", Thing::Tray),
            ("hub.notify", Thing::Tray),
            ("facts.write:broadcast.*", Thing::BetweenApps),
            ("channel.open:/offer:", Thing::BetweenApps),
            ("provide:scene.start", Thing::BetweenApps),
            ("something.new", Thing::Other),
        ] {
            assert_eq!(thing_of(raw), thing, "{raw}");
        }
    }

    #[test]
    fn sensitive_things_come_first_and_apps_share_a_thing() {
        let a = section(
            "m.b",
            "Beta",
            &[
                ("audio.inputs.mute", Group::MakeChanges, ItemWord::Allowed),
                ("net.connect:1.2.3.4:5", Group::Sensitive, ItemWord::Off),
                ("hid.raw", Group::CantBeAllowed, ItemWord::CantBeAllowed),
            ],
        );
        let b = section(
            "m.a",
            "Alpha",
            &[
                ("audio.inputs.read", Group::MakeChanges, ItemWord::Waits),
                ("facts.write:x.*", Group::ShareStatus, ItemWord::Allowed),
            ],
        );
        let g = group(&[a, b]);
        let things: Vec<Thing> = g.iter().map(|g| g.thing).collect();
        assert_eq!(
            things,
            vec![Thing::Network, Thing::Microphone, Thing::BetweenApps]
        );
        assert!(g[0].sensitive() && !g[1].sensitive());
        // Both apps under the microphone, by name; the item that can't be allowed is not listed.
        let mic: Vec<&str> = g[1].rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(mic, vec!["Alpha", "Beta"]);
        assert_eq!(g[1].summary(), "2 apps, 1 allowed");
        assert_eq!(g[0].summary(), "1 app, nothing allowed");
        assert!(g.iter().all(|g| g.thing != Thing::Keyboard));
    }
}
