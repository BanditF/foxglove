// SPDX-License-Identifier: MIT
//! What the tray icon says about the modules (docs/hideout-supervisor-ux.md, "Tray quiet rule"):
//! a red dot only for a stopped module and one that needs a newer Hideout, an amber dot for safe
//! mode, and nothing for restarts, updates or starts. Fed from the hub's facts on the router's
//! thread; the UI thread applies a change once (`take_change`), so a restart never wakes it.

use crate::icon::State;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The menu's status line with nothing to say.
pub const QUIET_LINE: &str = "No modules yet";

/// Why a module needs the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    Stopped,
    NeedsHub,
}

/// The tray's view of the modules.
#[derive(Debug, Default)]
pub struct Marks {
    names: BTreeMap<String, String>,
    /// In the order the modules came to need the user: the first is the root cause named.
    needs: Vec<(String, Need)>,
    safe_mode: bool,
    paused: bool,
    /// What the tray shows now (the last `take_change`).
    shown: Option<(State, String, String)>,
}

impl Marks {
    /// A module's name for people, from its manifest.
    pub fn name(&mut self, id: &str, name: &str) {
        self.names.insert(id.into(), name.into());
    }

    /// A hub fact. Only `module.<id>.state` and `hub.safe_mode` matter here.
    pub fn fact(&mut self, key: &str, value: &serde_json::Value) {
        if key == "hub.safe_mode" {
            self.safe_mode = value.as_bool() == Some(true);
            return;
        }
        let Some(id) = key
            .strip_prefix("module.")
            .and_then(|r| r.strip_suffix(".state"))
        else {
            return;
        };
        let need = match value.as_str() {
            Some("crash-stopped" | "crash-looped") => Some(Need::Stopped),
            Some("needs-hub") => Some(Need::NeedsHub),
            _ => None,
        };
        let at = self.needs.iter().position(|(i, _)| i == id);
        match (need, at) {
            // Still needing the user keeps its place in the order.
            (Some(n), Some(i)) => self.needs[i].1 = n,
            (Some(n), None) => self.needs.push((id.into(), n)),
            (None, Some(i)) => {
                self.needs.remove(i);
            }
            (None, None) => {}
        }
    }

    pub fn set_paused(&mut self, on: bool) {
        self.paused = on;
    }

    /// The glyph, the menu's status line and the icon's tooltip. Root cause once: one module is
    /// named, the first that needs the user.
    pub fn look(&self) -> (State, String, String) {
        let product = crate::PRODUCT;
        if self.paused {
            return (State::Paused, "Paused".into(), product.into());
        }
        if let Some((id, need)) = self.needs.first() {
            let name = self.names.get(id).cloned().unwrap_or_else(|| id.clone());
            let line = match need {
                Need::Stopped => {
                    let (a, b) = hideout_supervisor::words::tray_lines(&name);
                    format!("{a} \u{b7} {b}")
                }
                Need::NeedsHub => format!("{name} needs a newer Hideout"),
            };
            let tip = format!("{product} \u{b7} {line}");
            return (State::NeedsYou, line, tip);
        }
        if self.safe_mode {
            return (
                State::SafeMode,
                "Safe mode".into(),
                format!("{product} \u{b7} safe mode"),
            );
        }
        (State::Normal, QUIET_LINE.into(), product.into())
    }

    /// The look, when it differs from what the tray shows; it then counts as shown.
    pub fn take_change(&mut self) -> Option<(State, String, String)> {
        let look = self.look();
        if self.shown.as_ref() == Some(&look) {
            return None;
        }
        self.shown = Some(look.clone());
        Some(look)
    }

    /// Whether the look differs from what the tray shows (a wake is worth it).
    pub fn changed(&self) -> bool {
        self.shown.as_ref() != Some(&self.look())
    }
}

static MARKS: Mutex<Option<Marks>> = Mutex::new(None);
static WAKER: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Marks) -> R) -> R {
    let mut g = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Marks::default))
}

fn wake_if_changed() {
    if with(|m| m.changed()) {
        if let Some(w) = WAKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            w();
        }
    }
}

/// A module's name for the tray lines (the hub calls it for each manifest).
pub fn note_name(id: &str, name: &str) {
    with(|m| m.name(id, name));
    wake_if_changed();
}

/// A hub fact (the hub calls it for every fact). Wakes the UI thread only when the tray's look
/// changes.
pub fn note_fact(key: &str, value: &serde_json::Value) {
    with(|m| m.fact(key, value));
    wake_if_changed();
}

/// Pause Hideout from the tray menu.
pub(crate) fn note_paused(on: bool) {
    with(|m| m.set_paused(on));
    wake_if_changed();
}

/// How the UI thread gets woken for a change.
pub(crate) fn set_waker(w: Box<dyn Fn() + Send>) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
}

/// The change for the UI thread to apply to the tray, if any.
pub(crate) fn take_change() -> Option<(State, String, String)> {
    with(|m| m.take_change())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_stopped_needs_hub_and_safe_mode_touch_the_tray() {
        let mut m = Marks::default();
        m.name("a.art", "Art");
        assert_eq!(m.take_change().unwrap().0, State::Normal);
        for s in ["starting", "running", "restarting", "updating", "off"] {
            m.fact("module.a.art.state", &json!(s));
            m.fact(
                "module.a.art.restart",
                &json!({"delay_s": 4, "crash": 2, "of": 5}),
            );
            assert_eq!(m.take_change(), None, "{s} touched the tray");
        }
        m.fact("module.a.art.state", &json!("crash-stopped"));
        let (state, line, tip) = m.take_change().unwrap();
        assert_eq!(state, State::NeedsYou);
        assert_eq!(line, "Art stopped \u{b7} Open Art to start it again");
        assert!(tip.starts_with("Hideout"));
        m.fact("module.a.art.state", &json!("starting"));
        assert_eq!(m.take_change().unwrap().0, State::Normal);
        m.fact("module.b.x.state", &json!("needs-hub"));
        assert_eq!(m.take_change().unwrap().0, State::NeedsYou);
        m.fact("module.b.x.state", &serde_json::Value::Null);
        m.fact("hub.safe_mode", &json!(true));
        let (state, line, tip) = m.take_change().unwrap();
        assert_eq!(
            (state, line.as_str(), tip.as_str()),
            (State::SafeMode, "Safe mode", "Hideout \u{b7} safe mode")
        );
        // A stopped module is named over safe mode; pause wins over both.
        m.fact("module.a.art.state", &json!("crash-looped"));
        assert_eq!(m.look().0, State::NeedsYou);
        m.set_paused(true);
        assert_eq!(m.look().0, State::Paused);
    }

    #[test]
    fn the_tray_names_the_first_module_that_stopped() {
        let mut m = Marks::default();
        m.name("z.last", "Zed");
        m.name("a.first", "Art");
        m.fact("module.z.last.state", &json!("crash-stopped"));
        m.fact("module.a.first.state", &json!("crash-stopped"));
        assert!(m.look().1.starts_with("Zed stopped"), "{:?}", m.look());
        // Still stopped (the fact again): it keeps its place.
        m.fact("module.z.last.state", &json!("crash-looped"));
        assert!(m.look().1.starts_with("Zed stopped"));
        m.fact("module.z.last.state", &json!("starting"));
        assert!(m.look().1.starts_with("Art stopped"));
    }

    #[test]
    fn tray_lines_hold_no_ids() {
        let mut m = Marks::default();
        m.name("x.y", "Art");
        m.fact("module.x.y.state", &json!("crash-stopped"));
        let (_, line, tip) = m.look();
        for s in [line, tip] {
            assert!(!s.contains("x.y") && !s.contains("crash-stopped"), "{s}");
        }
    }
}
