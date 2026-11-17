// SPDX-License-Identifier: MIT
//! What the tray icon says about the modules (docs/hideout-supervisor-ux.md, "Tray quiet rule"):
//! a red dot only for a stopped module and one that needs a newer Hideout, an amber dot for safe
//! mode, and nothing for restarts, updates or starts. While a search for the module that breaks
//! Hideout runs (#1033) the amber dot stays and the line says where it is. Fed from the hub's facts on the router's
//! thread; the UI thread applies a change once (`take_change`), so a restart never wakes it.

use crate::icon::State;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The menu's status line with nothing to say.
pub const QUIET_LINE: &str = "No modules yet";

/// Lease families the tray menu offers Resume for (#1142, contract §8.5 v1.17): the family, the
/// item's words and how the holders are introduced. One item, only while a lease of it is held;
/// the status line and the icon stay quiet (a pause somebody asked for doesn't need the user).
const RESUMES: &[(&str, &str, &str)] = &[("wallpaper-engine", "Resume wallpapers", "paused by")];

/// "Arete", "Arete and Render queue", "Arete and 2 more".
fn holder_words(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, rest @ ..] => format!("{a} and {} more", rest.len()),
    }
}

/// Why a module needs the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    Stopped,
    NeedsHub,
    /// Its publisher withdrew the running version (#1026).
    Withdrawn,
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
    /// Whether the tray menu has **Start normally** now (the last `take_safe_change`).
    shown_safe: Option<bool>,
    /// The search's tray words (#1033), `None` without one.
    search: Option<hideout_supervisor::bisect::Tray>,
    /// What the menu's search items show now (the last `take_search_change`).
    shown_search: Option<Option<hideout_supervisor::bisect::Tray>>,
    /// Who holds leases, per provider and family (the hub fact `module.<id>.lease_holders`).
    holders: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// What the menu's Resume item shows now (the last `take_resume_change`).
    shown_resume: Option<Option<String>>,
}

impl Marks {
    /// A module's name for people, from its manifest.
    pub fn name(&mut self, id: &str, name: &str) {
        self.names.insert(id.into(), name.into());
    }

    /// A hub fact. Only `module.<id>.state`, `module.<id>.lease_holders` and `hub.safe_mode`
    /// matter here.
    pub fn fact(&mut self, key: &str, value: &serde_json::Value) {
        if key == "hub.safe_mode" {
            self.safe_mode = value.as_bool() == Some(true);
            return;
        }
        let holders_suffix = format!(".{}", hideout_contract::lease::HOLDERS_FACT);
        if let Some(id) = key
            .strip_prefix("module.")
            .and_then(|r| r.strip_suffix(holders_suffix.as_str()))
        {
            let families: BTreeMap<String, Vec<String>> =
                serde_json::from_value(value.clone()).unwrap_or_default();
            if families.is_empty() {
                self.holders.remove(id);
            } else {
                self.holders.insert(id.into(), families);
            }
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
            Some("revoked") => Some(Need::Withdrawn),
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

    /// The search's tray words (#1033).
    pub fn set_search(&mut self, s: Option<hideout_supervisor::bisect::Tray>) {
        self.search = s;
    }

    /// The menu's Resume item (#1142): its words while a family it has words for is held.
    pub fn resume_item(&self) -> Option<String> {
        RESUMES.iter().find_map(|(family, words, by)| {
            let mut names: Vec<String> = self
                .holders
                .values()
                .filter_map(|f| f.get(*family))
                .flatten()
                .cloned()
                .collect();
            names.sort();
            names.dedup();
            (!names.is_empty()).then(|| format!("{words} ({by} {})", holder_words(&names)))
        })
    }

    /// What Resume ends: (provider, family) for each held family the item stands for.
    pub fn resume_targets(&self) -> Vec<(String, String)> {
        let Some((family, ..)) = RESUMES
            .iter()
            .find(|(f, ..)| self.holders.values().any(|h| h.contains_key(*f)))
        else {
            return Vec::new();
        };
        self.holders
            .iter()
            .filter(|(_, h)| h.contains_key(*family))
            .map(|(p, _)| (p.clone(), family.to_string()))
            .collect()
    }

    /// The Resume item, when it differs from what the menu has.
    pub fn take_resume_change(&mut self) -> Option<Option<String>> {
        let now = self.resume_item();
        if self.shown_resume.clone().unwrap_or(None) == now {
            return None;
        }
        self.shown_resume = Some(now.clone());
        Some(now)
    }

    /// Start normally is in the menu: safe mode, and no search running (its own items then).
    fn safe_item(&self) -> bool {
        self.safe_mode && !self.search.as_ref().is_some_and(|s| s.stop)
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
                Need::Withdrawn => {
                    let (a, b) = hideout_supervisor::words::withdrawn_tray_lines(&name);
                    format!("{a} \u{b7} {b}")
                }
            };
            let tip = format!("{product} \u{b7} {line}");
            return (State::NeedsYou, line, tip);
        }
        if let Some(s) = &self.search {
            // "Hideout · finding the module (round 2 of 4)": the line and the tooltip.
            return (State::SafeMode, s.line.clone(), s.line.clone());
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

    /// Safe mode, for the menu's **Start normally**, when it differs from what the menu has.
    /// Kept apart from the look: a stopped module is named over safe mode, but the way out of
    /// safe mode stays in the menu.
    pub fn take_safe_change(&mut self) -> Option<bool> {
        let on = self.safe_item();
        if self.shown_safe == Some(on) {
            return None;
        }
        self.shown_safe = Some(on);
        Some(on)
    }

    /// The search's menu items, when they differ from what the menu has.
    pub fn take_search_change(&mut self) -> Option<Option<hideout_supervisor::bisect::Tray>> {
        if self.shown_search.clone().unwrap_or(None) == self.search {
            return None;
        }
        self.shown_search = Some(self.search.clone());
        Some(self.search.clone())
    }

    /// Whether the look or the menu differs from what the tray shows (a wake is worth it).
    pub fn changed(&self) -> bool {
        self.shown.as_ref() != Some(&self.look())
            || self.shown_safe != Some(self.safe_item())
            || self.shown_search.clone().unwrap_or(None) != self.search
            || self.shown_resume.clone().unwrap_or(None) != self.resume_item()
    }
}

static MARKS: Mutex<Option<Marks>> = Mutex::new(None);
static WAKER: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);
/// Ends a family's leases at a provider (the hub's router, set by the hub at start).
static RESUMER: Mutex<Option<Resumer>> = Mutex::new(None);

/// Ends the leases of `family` at `provider` (the tray's Resume, #1142).
pub type Resumer = Box<dyn Fn(&str, &str) + Send>;

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

/// The search's tray words (#1033); the hub calls it on every change of the search.
pub fn note_search(s: Option<hideout_supervisor::bisect::Tray>) {
    with(|m| m.set_search(s));
    wake_if_changed();
}

/// How the tray's Resume reaches the router: `f(provider, family)` ends those leases (#1142).
pub fn set_resumer(f: Resumer) {
    *RESUMER.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// The tray's Resume: every lease of the family the item stands for ends, at every provider.
pub(crate) fn resume() {
    let targets = with(|m| m.resume_targets());
    let g = RESUMER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(f) = g.as_ref() else {
        log::warn!("Resume from the tray: no router in this hub");
        return;
    };
    for (provider, family) in targets {
        log::info!("Resume from the tray: {family} leases at {provider} end");
        f(&provider, &family);
    }
}

/// The Resume item, when it changed.
pub(crate) fn take_resume_change() -> Option<Option<String>> {
    with(|m| m.take_resume_change())
}

/// How the UI thread gets woken for a change.
pub(crate) fn set_waker(w: Box<dyn Fn() + Send>) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
}

/// The change for the UI thread to apply to the tray, if any.
pub(crate) fn take_change() -> Option<(State, String, String)> {
    with(|m| m.take_change())
}

/// Safe mode for the tray menu's **Start normally**, when it changed.
pub(crate) fn take_safe_change() -> Option<bool> {
    with(|m| m.take_safe_change())
}

/// The search's menu items, when they changed.
pub(crate) fn take_search_change() -> Option<Option<hideout_supervisor::bisect::Tray>> {
    with(|m| m.take_search_change())
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

    /// A withdrawn version (#1026): the red dot and "Art was withdrawn", root cause only; the
    /// dot goes once it is turned off or updated. The publisher's note never reaches the tray.
    #[test]
    fn a_withdrawn_module_puts_a_red_dot_on_the_tray() {
        let mut m = Marks::default();
        m.name("a.art", "Art");
        let _ = m.take_change();
        m.fact(
            "module.a.art.revoked",
            &json!({"version": "0.5.0", "reason": "secret note"}),
        );
        assert_eq!(m.take_change(), None, "the router's fact alone is no state");
        m.fact("module.a.art.state", &json!("revoked"));
        let (state, line, tip) = m.take_change().unwrap();
        assert_eq!(state, State::NeedsYou);
        assert_eq!(
            line,
            "Art was withdrawn \u{b7} Open Art to check for updates"
        );
        assert!(!tip.contains("secret") && !line.contains("secret"));
        m.fact("module.a.art.state", &json!("off"));
        assert_eq!(m.take_change().unwrap().0, State::Normal);
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

    /// The menu's Start normally follows safe mode even while a stopped module is named.
    #[test]
    fn start_normally_stays_in_the_menu_while_safe_mode_lasts() {
        let mut m = Marks::default();
        assert_eq!(m.take_safe_change(), Some(false));
        assert_eq!(m.take_safe_change(), None);
        m.fact("module.a.art.state", &json!("crash-stopped"));
        let _ = m.take_change();
        m.fact("hub.safe_mode", &json!(true));
        assert!(m.changed());
        assert_eq!(m.take_change(), None, "the stopped module stays named");
        assert_eq!(m.take_safe_change(), Some(true));
        m.fact("hub.safe_mode", &json!(false));
        assert_eq!(m.take_safe_change(), Some(false));
        assert!(!m.changed());
    }

    /// During a search (#1033): the amber dot with the search's line as line and tooltip;
    /// Start normally leaves the menu while it runs and comes back at a result.
    #[test]
    fn a_search_keeps_the_amber_dot_and_names_the_round() {
        use hideout_supervisor::bisect::Tray;
        let mut m = Marks::default();
        m.fact("hub.safe_mode", &json!(true));
        assert_eq!(m.take_safe_change(), Some(true));
        let _ = m.take_search_change();
        let round = Tray {
            line: "Hideout \u{b7} finding the module (round 2 of 4)".into(),
            start_next: None,
            stop: true,
        };
        m.set_search(Some(round.clone()));
        assert!(m.changed());
        let (state, line, tip) = m.take_change().unwrap();
        assert_eq!(state, State::SafeMode);
        assert_eq!(
            (line.as_str(), tip.as_str()),
            (round.line.as_str(), round.line.as_str())
        );
        assert_eq!(
            m.take_safe_change(),
            Some(false),
            "Start normally out of the menu"
        );
        assert_eq!(m.take_search_change(), Some(Some(round)));
        assert_eq!(m.take_search_change(), None);
        m.set_search(Some(Tray {
            line: "Hideout \u{b7} found the module".into(),
            start_next: None,
            stop: false,
        }));
        assert_eq!(m.take_safe_change(), Some(true), "back with the result");
        m.set_search(None);
        assert_eq!(m.take_change().unwrap().1, "Safe mode");
        assert_eq!(m.take_search_change(), Some(None));
        assert_eq!(m.take_search_change(), None);
    }

    /// Resume (#1142): one item while somebody holds a wallpaper pause, naming who; the status
    /// line and the dot stay quiet; it goes when nothing is held. Other families get no item.
    #[test]
    fn resume_shows_only_while_a_pause_is_held_and_keeps_the_tray_quiet() {
        let mut m = Marks::default();
        let quiet = m.take_change().unwrap();
        let _ = (m.take_safe_change(), m.take_search_change());
        assert_eq!(m.take_resume_change(), None, "nothing held: no item");
        let key = "module.foxglove.lease_holders";
        m.fact(key, &json!({"lighting": ["Arete"]}));
        assert_eq!(m.take_resume_change(), None, "no words for lighting");
        assert!(m.resume_targets().is_empty());
        m.fact(
            key,
            &json!({"wallpaper-engine": ["Arete"], "lighting": ["X"]}),
        );
        assert!(m.changed());
        assert_eq!(
            m.take_resume_change(),
            Some(Some("Resume wallpapers (paused by Arete)".into()))
        );
        assert_eq!(m.take_change(), None, "status line and dot stay quiet");
        assert_eq!(m.look(), quiet);
        m.fact(key, &json!({"wallpaper-engine": ["Arete", "Render queue"]}));
        assert_eq!(
            m.resume_item().unwrap(),
            "Resume wallpapers (paused by Arete and Render queue)"
        );
        m.fact(
            "module.other.lease_holders",
            &json!({"wallpaper-engine": ["Zed"]}),
        );
        assert_eq!(
            m.resume_item().unwrap(),
            "Resume wallpapers (paused by Arete and 2 more)"
        );
        assert_eq!(
            m.resume_targets(),
            [
                ("foxglove".to_string(), "wallpaper-engine".to_string()),
                ("other".to_string(), "wallpaper-engine".to_string())
            ]
        );
        m.fact(key, &serde_json::Value::Null);
        m.fact("module.other.lease_holders", &serde_json::Value::Null);
        assert_eq!(m.take_resume_change(), Some(None));
        assert!(!m.changed());
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
