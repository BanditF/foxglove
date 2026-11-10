// SPDX-License-Identifier: MIT
//! The Modules page's rows (docs/hideout-supervisor-ux.md, column **M**): one row per installed
//! module with its name, version, the state in plain words and **Open**. Generic: the hub hands
//! in what is installed and reads facts; this file names no module (§2.2).
//!
//! Layout is a minimal list: the UX docs define the row words, not the page's layout, so the
//! product designer reviews it.

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

/// One drawn row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The state in plain words (never an id).
    pub state: String,
    /// Drawn in the danger colour: stopped, couldn't start, needs a newer Hideout.
    pub stopped: bool,
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
    let get = |k: &str| fact(&format!("module.{}.{k}", m.id));
    let state = get("state").and_then(|v| v.as_str().map(String::from));
    let safe_mode = fact("hub.safe_mode").and_then(|v| v.as_bool()) == Some(true);
    let (words, stopped) = match state.as_deref() {
        None | Some("running") if !m.enabled => ("Off".to_string(), false),
        None if safe_mode => ("On \u{b7} not started in safe mode".into(), false),
        None | Some("running") => ("On".into(), false),
        Some("starting") => ("Starting\u{2026}".into(), false),
        Some("updating") => ("Updating\u{2026}".into(), false),
        Some("restarting") => {
            let (secs, k) = restart.unwrap_or((0, 1));
            (words::restarting_row(secs, k), false)
        }
        Some("crash-stopped") | Some("crash-looped") => (words::STOPPED.into(), true),
        Some("install-failed")
            if get("failed")
                .and_then(|v| v.as_str().map(String::from))
                .as_deref()
                == Some("launch") =>
        {
            (words::couldnt_start_title(&m.name), true)
        }
        Some(s) => (
            state_words(s).to_string(),
            matches!(s, "install-failed" | "needs-hub"),
        ),
    };
    Row {
        id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        state: words,
        stopped,
    }
}

/// The page's empty state.
pub const EMPTY_HEADING: &str = "No modules yet";
pub const EMPTY_BODY: &str = "Modules add pages, tray tiles, status items and commands to \
Hideout. Installed modules will be listed here, with their state and version.";
/// The rows' button.
pub const OPEN: &str = "Open";

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

/// Draws the page: title, then the rows (or the empty state). Returns the module whose **Open**
/// was clicked.
pub fn page_ui(ui: &mut egui::Ui, t: &Tokens, rows: &[Row]) -> Option<String> {
    use egui::RichText;
    ui.label(
        RichText::new("Modules")
            .size(24.0)
            .strong()
            .color(c(t.text)),
    );
    ui.add_space(20.0);
    if rows.is_empty() {
        ui.label(RichText::new(EMPTY_HEADING).size(17.0).color(c(t.text)));
        ui.add_space(4.0);
        ui.label(RichText::new(EMPTY_BODY).color(c(t.text_muted)));
        return None;
    }
    let mut open = None;
    for r in rows {
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
                            ui.label(RichText::new(&r.name).size(15.0).strong().color(c(t.text)));
                            ui.label(RichText::new(&r.version).color(c(t.text_muted)));
                        });
                        let col = if r.stopped { t.danger } else { t.text_muted };
                        ui.label(RichText::new(&r.state).color(c(col)));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = ui.button(OPEN).on_hover_text(words::open_module(&r.name));
                        if b.clicked() {
                            open = Some(r.id.clone());
                        }
                    });
                });
            });
        ui.add_space(8.0);
    }
    open
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
                "Couldn't start Art",
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
            for never in words::NEVER_SHOWN {
                assert!(!r.state.contains(never), "{never} in {}", r.state);
            }
            assert!(!r.state.contains("m.art"));
        }
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
}
