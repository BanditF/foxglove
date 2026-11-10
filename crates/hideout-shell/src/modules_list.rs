// SPDX-License-Identifier: MIT
//! The Modules page's rows (docs/hideout-supervisor-ux.md, column **M**): one row per installed
//! module with its name, version, the state in plain words, one main button for the state and
//! **Open**. Generic: the hub hands in what is installed and reads facts; this file names no
//! module (§2.2).
//!
//! Layout (designer review of #1028, #1029): `[mark] Name  version` over the state words; the
//! red warning mark before a row that needs the user, so the warning doesn't rest on colour;
//! those rows first, then the others A–Z, Off last and muted. In safe mode the banner sits above
//! the rows with **Start normally** (§7.10).

use std::collections::HashMap;
use std::time::Instant;

use hideout_panels::state_words;
use hideout_supervisor::words;
use serde_json::Value;

use crate::egui;
use crate::theme::{Rgb, Tokens};

/// An installed module, as the registry has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

/// What a row's main button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    /// The ladder (§7.8): Start again, Start with default settings, Go back, Turn off.
    StartAgain,
    StartDefaults,
    GoBack,
    TurnOff,
    /// Off: Turn on.
    TurnOn,
    /// Needs a newer Hideout: Update Hideout.
    UpdateHub,
    /// Couldn't start: Try again.
    TryAgain,
    /// Withdrawn by its publisher (#1026), on or off: Check for updates.
    CheckUpdates,
}

/// A row's one main button (never two on a row).
#[derive(Clone, Debug, PartialEq)]
pub struct Primary {
    pub label: String,
    pub does: Does,
}

/// One drawn row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The state in plain words (never an id).
    pub state: String,
    /// It needs the user (stopped, couldn't start or install, needs a newer Hideout): the red
    /// mark before the name, the state in the danger colour, and the top of the list.
    pub stopped: bool,
    /// Off: last in the list, muted.
    pub off: bool,
    /// The row's accessible name: "Art, stopped after 5 crashes".
    pub label: String,
    /// The main button for the state; Open is always there besides.
    pub primary: Option<Primary>,
}

/// What a click on the page asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// **Open** on a row: the module's page.
    Open(String),
    /// A row's main button.
    Primary(String, Does),
    /// The safe-mode banner's **Start normally**.
    StartNormally,
}

/// Seconds left of a `restarting` module's backoff, counted from when its
/// `module.<id>.restart` fact was first seen (as the crash bar counts).
#[derive(Default)]
pub struct Restarts(HashMap<String, (Value, Instant)>);

impl Restarts {
    /// (seconds left, crash number) for the module's restart fact, or `None` without one.
    pub fn left(&mut self, id: &str, fact: Option<&Value>, now: Instant) -> Option<(u64, usize)> {
        let Some(v) = fact.filter(|v| !v.is_null()) else {
            self.0.remove(id);
            return None;
        };
        let e = self.0.entry(id.to_string()).or_insert((v.clone(), now));
        if e.0 != *v {
            *e = (v.clone(), now);
        }
        let delay = v.get("delay_s").and_then(Value::as_u64).unwrap_or(0);
        let k = v.get("crash").and_then(Value::as_u64).unwrap_or(1) as usize;
        let gone = now.saturating_duration_since(e.1).as_secs();
        Some((delay.saturating_sub(gone).min(60), k))
    }
}

/// The row for a module from its facts (`fact` takes the full fact name). `restart` is
/// [`Restarts::left`]'s answer for it.
pub fn row(
    m: &Installed,
    fact: &dyn Fn(&str) -> Option<Value>,
    restart: Option<(u64, usize)>,
) -> Row {
    let get =
        |k: &str| fact(&format!("module.{}.{k}", m.id)).and_then(|v| v.as_str().map(String::from));
    let state = get("state");
    // The version its publisher withdrew, while the router's fact says so (#1026).
    let withdrawn = fact(&format!("module.{}.revoked", m.id))
        .and_then(|v| v.get("version").and_then(Value::as_str).map(String::from));
    let safe_mode = fact("hub.safe_mode").and_then(|v| v.as_bool()) == Some(true);
    let primary = |label: &str, does| {
        Some(Primary {
            label: label.into(),
            does,
        })
    };
    let (words, stopped, main) = match state.as_deref() {
        Some("revoked") => (
            words::withdrawn_row(withdrawn.as_deref().unwrap_or(&m.version)),
            true,
            primary(words::CHECK_FOR_UPDATES, Does::CheckUpdates),
        ),
        // Turned off after the withdrawal: no Turn on, Check for updates instead.
        None | Some("off") if !m.enabled && withdrawn.is_some() => (
            words::off_withdrawn(withdrawn.as_deref().unwrap_or(&m.version)),
            false,
            primary(words::CHECK_FOR_UPDATES, Does::CheckUpdates),
        ),
        None | Some("running") | Some("off") if !m.enabled => (
            "Off".to_string(),
            false,
            primary(words::TURN_ON, Does::TurnOn),
        ),
        None if safe_mode => ("On \u{b7} not started in safe mode".into(), false, None),
        None | Some("running") => ("On".into(), false, None),
        Some("starting") => ("Starting\u{2026}".into(), false, None),
        Some("updating") => ("Updating\u{2026}".into(), false, None),
        Some("restarting") => {
            let (secs, k) = restart.unwrap_or((0, 1));
            (words::restarting_row(secs, k), false, None)
        }
        Some("crash-stopped") | Some("crash-looped") => {
            // The ladder's step, worded as the stopped card's main button.
            let step = words::Recovery::from_id(get("recovery").as_deref());
            let previous = get("previous");
            let does = match (step, previous.is_some()) {
                (words::Recovery::StartAgain, _) => Does::StartAgain,
                (words::Recovery::Defaults, _) => Does::StartDefaults,
                (words::Recovery::GoBack, true) => Does::GoBack,
                (words::Recovery::GoBack, false) | (words::Recovery::Last, _) => Does::TurnOff,
            };
            let label = step.button(&m.name, previous.as_deref());
            (words::STOPPED.into(), true, primary(&label, does))
        }
        Some("install-failed") if get("failed").as_deref() == Some("launch") => (
            words::COULDNT_START.into(),
            true,
            primary(words::TRY_AGAIN, Does::TryAgain),
        ),
        Some("needs-hub") => (
            state_words("needs-hub").to_string(),
            true,
            primary(words::UPDATE_HIDEOUT, Does::UpdateHub),
        ),
        Some("off") => ("Off".into(), false, primary(words::TURN_ON, Does::TurnOn)),
        Some(s) => (state_words(s).to_string(), s == "install-failed", None),
    };
    Row {
        id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        label: words::row_label(&m.name, &words),
        off: words == "Off" || words.starts_with("Off \u{b7}"),
        state: words,
        stopped,
        primary: main,
    }
}

/// The list's order: rows that need the user first, then the others A–Z, Off last. No
/// headings (fewer than about 8 modules).
pub fn order(rows: &mut [Row]) {
    rows.sort_by_cached_key(|r| {
        let group = if r.stopped {
            0
        } else if r.off {
            2
        } else {
            1
        };
        (group, r.name.to_lowercase())
    });
}

/// The page's empty state.
pub const EMPTY_HEADING: &str = "No modules yet";
pub const EMPTY_BODY: &str = "Modules add pages, tray tiles, status items and commands to \
Hideout. Installed modules will be listed here, with their state and version.";
/// The rows' secondary button.
pub const OPEN: &str = words::OPEN;

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

/// The safe-mode banner (§7.10): "Safe mode" / the line / **Start normally** · Find the module
/// that breaks Hideout. `line` is [`words::safe_mode_line`]'s. Bisect isn't built: its link is
/// greyed with "Comes in a later Hideout." as its tooltip and its accessible description (a
/// screen reader reads it without hovering). Amber, like the tray's dot. True when Start
/// normally was clicked.
pub fn safe_mode_banner(ui: &mut egui::Ui, t: &Tokens, line: &str) -> bool {
    use egui::RichText;
    let mut start = false;
    egui::Frame::new()
        .fill(c(crate::theme::warn_fill(t)))
        .stroke(egui::Stroke::new(1.0_f32, c(t.warn)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(words::SAFE_MODE)
                    .size(16.0)
                    .strong()
                    .color(c(t.text)),
            );
            ui.add_space(2.0);
            ui.label(RichText::new(line).color(c(t.text)));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let b =
                    egui::Button::new(RichText::new(words::START_NORMALLY).color(c(t.on_accent)))
                        .fill(c(t.accent));
                start = ui.add(b).clicked();
                ui.label(RichText::new("\u{b7}").color(c(t.text_muted)));
                let later = ui
                    .add_enabled(
                        false,
                        egui::Link::new(RichText::new(words::FIND_CULPRIT).color(c(t.text_muted))),
                    )
                    .on_disabled_hover_text(words::COMES_LATER);
                ui.ctx().accesskit_node_builder(later.id, |n| {
                    n.set_description(words::COMES_LATER);
                });
            });
        });
    ui.add_space(16.0);
    start
}

/// Draws the page: title, the safe-mode banner with its line when `banner` is set, then the rows
/// (or the empty state). Returns what was clicked.
pub fn page_ui(ui: &mut egui::Ui, t: &Tokens, rows: &[Row], banner: Option<&str>) -> Option<Click> {
    use egui::RichText;
    ui.label(
        RichText::new("Modules")
            .size(24.0)
            .strong()
            .color(c(t.text)),
    );
    ui.add_space(20.0);
    let mut click = None;
    if banner.is_some_and(|line| safe_mode_banner(ui, t, line)) {
        click = Some(Click::StartNormally);
    }
    if rows.is_empty() {
        ui.label(RichText::new(EMPTY_HEADING).size(17.0).color(c(t.text)));
        ui.add_space(4.0);
        ui.label(RichText::new(EMPTY_BODY).color(c(t.text_muted)));
        return click;
    }
    for r in rows {
        let (name_col, state_col) = match (r.stopped, r.off) {
            (true, _) => (t.text, t.danger),
            (false, true) => (t.text_muted, t.text_muted),
            (false, false) => (t.text, t.text_muted),
        };
        ui.push_id(("hub.modules.row", &r.id), |ui| {
            egui::Frame::new()
                .fill(c(t.surface))
                .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if r.stopped {
                                    ui.label(
                                        RichText::new("\u{26A0}").size(15.0).color(c(t.danger)),
                                    );
                                }
                                let name = ui.label(
                                    RichText::new(&r.name)
                                        .size(15.0)
                                        .strong()
                                        .color(c(name_col)),
                                );
                                let said = r.label.clone();
                                ui.ctx()
                                    .accesskit_node_builder(name.id, |n| n.set_label(said));
                                ui.label(RichText::new(&r.version).color(c(t.text_muted)));
                            });
                            ui.label(RichText::new(&r.state).color(c(state_col)));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = ui.button(OPEN).on_hover_text(words::open_module(&r.name));
                            if b.clicked() {
                                click = Some(Click::Open(r.id.clone()));
                            }
                            if let Some(p) = &r.primary {
                                let b = egui::Button::new(
                                    RichText::new(&p.label).color(c(t.on_accent)),
                                )
                                .fill(c(t.accent));
                                if ui.add(b).clicked() {
                                    click = Some(Click::Primary(r.id.clone(), p.does));
                                }
                            }
                        });
                    });
                });
        });
        ui.add_space(8.0);
    }
    click
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn m(id: &str, enabled: bool) -> Installed {
        Installed {
            id: id.into(),
            name: "Art".into(),
            version: "0.5.0".into(),
            enabled,
        }
    }

    fn with(facts: &[(&str, Value)]) -> impl Fn(&str) -> Option<Value> {
        let f: HashMap<String, Value> = facts
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        move |k| f.get(k).cloned()
    }

    type Case<'a> = (
        bool,
        Vec<(&'a str, Value)>,
        Option<(u64, usize)>,
        &'a str,
        bool,
    );

    #[test]
    fn each_state_reads_the_row_words_and_never_an_id() {
        let cases: Vec<Case> = vec![
            (true, vec![], None, "On", false),
            (
                true,
                vec![("module.m.art.state", json!("running"))],
                None,
                "On",
                false,
            ),
            (false, vec![], None, "Off", false),
            (
                false,
                vec![("module.m.art.state", json!("off"))],
                None,
                "Off",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("starting"))],
                None,
                "Starting\u{2026}",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                Some((4, 2)),
                "Restarting in 4 s (2 of 5)",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                Some((0, 2)),
                "Restarting\u{2026} (2 of 5)",
                false,
            ),
            (
                true,
                vec![("module.m.art.state", json!("crash-stopped"))],
                None,
                "Stopped after 5 crashes",
                true,
            ),
            (
                true,
                vec![
                    ("module.m.art.state", json!("install-failed")),
                    ("module.m.art.failed", json!("launch")),
                ],
                None,
                "Couldn't start",
                true,
            ),
            (
                true,
                vec![("module.m.art.state", json!("install-failed"))],
                None,
                "Couldn't install or update",
                true,
            ),
            (
                true,
                vec![("module.m.art.state", json!("needs-hub"))],
                None,
                "Needs a newer Hideout",
                true,
            ),
            (
                true,
                vec![("hub.safe_mode", json!(true))],
                None,
                "On \u{b7} not started in safe mode",
                false,
            ),
        ];
        for (enabled, facts, restart, want, stopped) in cases {
            let r = row(&m("m.art", enabled), &with(&facts), restart);
            assert_eq!((r.state.as_str(), r.stopped), (want, stopped), "{facts:?}");
            let mut drawn = vec![r.state.clone(), r.label.clone()];
            drawn.extend(r.primary.iter().map(|p| p.label.clone()));
            for s in &drawn {
                for never in words::NEVER_SHOWN {
                    assert!(!s.contains(never), "{never} in {s}");
                }
                assert!(!s.contains("m.art"), "{s}");
            }
        }
    }

    type ButtonCase<'a> = (bool, Vec<(&'a str, Value)>, Option<(&'a str, Does)>);

    /// One main button per state (designer review of #1028): the ladder's wording for a
    /// stopped row, Turn on, Update Hideout, Try again; none for the other states.
    #[test]
    fn each_state_has_at_most_one_main_button() {
        let stopped = |extra: Vec<(&'static str, Value)>| {
            let mut f = vec![("module.m.art.state", json!("crash-stopped"))];
            f.extend(extra);
            f
        };
        let cases: Vec<ButtonCase> = vec![
            (
                true,
                stopped(vec![]),
                Some(("Start again", Does::StartAgain)),
            ),
            (
                true,
                stopped(vec![("module.m.art.recovery", json!("defaults"))]),
                Some(("Start with default settings", Does::StartDefaults)),
            ),
            (
                true,
                stopped(vec![
                    ("module.m.art.recovery", json!("go-back")),
                    ("module.m.art.previous", json!("0.4.2")),
                ]),
                Some(("Go back to 0.4.2", Does::GoBack)),
            ),
            (
                true,
                stopped(vec![("module.m.art.recovery", json!("last"))]),
                Some(("Turn off Art", Does::TurnOff)),
            ),
            (false, vec![], Some(("Turn on", Does::TurnOn))),
            (
                true,
                vec![("module.m.art.state", json!("off"))],
                Some(("Turn on", Does::TurnOn)),
            ),
            (
                true,
                vec![("module.m.art.state", json!("needs-hub"))],
                Some(("Update Hideout", Does::UpdateHub)),
            ),
            (
                true,
                vec![
                    ("module.m.art.state", json!("install-failed")),
                    ("module.m.art.failed", json!("launch")),
                ],
                Some(("Try again", Does::TryAgain)),
            ),
            (
                true,
                vec![("module.m.art.state", json!("install-failed"))],
                None,
            ),
            (true, vec![], None),
            (true, vec![("module.m.art.state", json!("starting"))], None),
            (
                true,
                vec![("module.m.art.state", json!("restarting"))],
                None,
            ),
            (true, vec![("hub.safe_mode", json!(true))], None),
        ];
        for (enabled, facts, want) in cases {
            let r = row(&m("m.art", enabled), &with(&facts), None);
            let got = r.primary.as_ref().map(|p| (p.label.as_str(), p.does));
            assert_eq!(got, want, "{facts:?}");
        }
        let r = row(&m("m.art", true), &with(&stopped(vec![])), None);
        assert_eq!(r.label, "Art, stopped after 5 crashes");
    }

    /// Rows that need the user first, then A–Z, Off last.
    #[test]
    fn needs_you_first_then_a_to_z_then_off() {
        let mk = |name: &str, stopped: bool, off: bool| Row {
            id: name.to_lowercase(),
            name: name.into(),
            version: "1".into(),
            state: String::new(),
            stopped,
            off,
            label: String::new(),
            primary: None,
        };
        let mut rows = vec![
            mk("Beat", false, false),
            mk("Alpha", false, true),
            mk("Zed", true, false),
            mk("art", false, false),
            mk("Dock", true, false),
        ];
        order(&mut rows);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Dock", "Zed", "art", "Beat", "Alpha"]);
    }

    /// The banner's words and the tray item's are the UX plan's, and none is an id.
    #[test]
    fn safe_mode_words_never_show_an_id() {
        for s in [
            words::SAFE_MODE,
            words::SAFE_MODE_LINE,
            words::SAFE_MODE_LINE_SET,
            words::START_NORMALLY,
            words::FIND_CULPRIT,
            words::COMES_LATER,
            crate::tray::START_NORMALLY_LABEL,
        ] {
            for never in words::NEVER_SHOWN {
                assert!(!s.to_lowercase().contains(&never.to_lowercase()), "{s}");
            }
        }
        assert_eq!(crate::tray::START_NORMALLY_LABEL, "Start normally");
    }

    #[test]
    fn the_restart_countdown_runs_from_when_the_fact_was_seen() {
        let mut r = Restarts::default();
        let t0 = Instant::now();
        let f = json!({"delay_s": 8, "crash": 2, "of": 5});
        assert_eq!(r.left("m", Some(&f), t0), Some((8, 2)));
        assert_eq!(
            r.left("m", Some(&f), t0 + Duration::from_millis(2500)),
            Some((6, 2))
        );
        assert_eq!(
            r.left("m", Some(&f), t0 + Duration::from_secs(20)),
            Some((0, 2))
        );
        let g = json!({"delay_s": 16, "crash": 3, "of": 5});
        assert_eq!(
            r.left("m", Some(&g), t0 + Duration::from_secs(21)),
            Some((16, 3))
        );
        assert_eq!(r.left("m", None, t0), None);
    }

    /// The banner says the line it's given, and the greyed bisect link carries "Comes in a later
    /// Hideout." as its accessible description, not only as a hover tooltip (#1033).
    #[test]
    fn the_banner_says_its_line_and_the_greyed_link_why() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::tokens(false);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                page_ui(ui, &t, &[], Some(words::SAFE_MODE_LINE_SET));
            });
        });
        let nodes = out.platform_output.accesskit_update.unwrap().nodes;
        let link = nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.label() == Some(words::FIND_CULPRIT))
            .expect("no bisect link node");
        assert!(link.is_disabled());
        assert_eq!(link.description(), Some(words::COMES_LATER));
        // Labels carry their text as the value, buttons and links as the label.
        let labels: Vec<_> = nodes
            .iter()
            .filter_map(|(_, n)| n.label().or(n.value()))
            .collect();
        assert!(labels.contains(&words::SAFE_MODE_LINE_SET), "{labels:?}");
        assert!(!labels.contains(&words::SAFE_MODE_LINE), "{labels:?}");
    }
}
