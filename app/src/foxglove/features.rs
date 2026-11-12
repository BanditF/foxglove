//! The feature table (#1143, P3): what the app starts, in what order, and what each feature owns.
//!
//! One table drives start-up: `main` calls [`start_all`], which runs [`STEPS`] in order. The
//! order is exactly the order `main.rs` called the starts in before the table; the test
//! `start_order_is_todays` pins it. Nothing starts or stops differently by default.
//!
//! - [`FEATURES`] says, per feature, its `FOXGLOVE_DISABLE` keys, the pages and tray tiles it
//!   draws and the store sections it owns. The sections are enforced (#1143, P10):
//!   [`section_owner`] answers `store::section`, so a feature opens only its own.
//! - [`STEPS`] is the ordered list of start functions, each tied to a feature. A step with a
//!   `gate` is skipped at the table when that `FOXGLOVE_DISABLE` key is set (`screens`, `art`:
//!   the two `main` gated itself). Every other start still checks its own key inside, as before,
//!   so the table never changes which key does what.
//!
//! `FOXGLOVE_DISABLE` is still read in one place, [`super::repaint_trace::disabled`]; the table
//! asks it. Left out of the table: `foxglove.firmware` has no start (it works on demand), and
//! the parts of start-up before `screens::start` (single instance, store split, updater, GPU
//! reset, away, calibration, settings, device discovery) stay in `main` because they decide
//! whether the app runs at all.

// Pages and tiles are read only by tests until the modules use them (#1143).
#![allow(dead_code)]

/// A feature: what it is called, how it is switched off and what it owns.
pub struct Feature {
    pub id: &'static str,
    /// Every `FOXGLOVE_DISABLE` key that leaves a part of this feature out.
    pub disable_keys: &'static [&'static str],
    /// Settings pages it draws (the shell's page labels).
    pub pages: &'static [&'static str],
    /// Tray tiles it draws (page-guard keys).
    pub tiles: &'static [&'static str],
    /// Store sections it owns.
    pub store_sections: &'static [&'static str],
}

/// The feature that owns store section `name`, if any (#1143, P10).
pub fn section_owner(name: &str) -> Option<&'static str> {
    FEATURES
        .iter()
        .find(|f| f.store_sections.contains(&name))
        .map(|f| f.id)
}

/// Whether `id` is a feature in the table.
pub fn is_feature(id: &str) -> bool {
    FEATURES.iter().any(|f| f.id == id)
}

/// One start function, in start-up order.
pub struct Step {
    pub feature: &'static str,
    /// The start function's name, for the order snapshot.
    pub name: &'static str,
    /// Skipped at the table when this `FOXGLOVE_DISABLE` key is set.
    pub gate: Option<&'static str>,
    pub start: fn(),
}

const fn f(
    id: &'static str,
    disable_keys: &'static [&'static str],
    pages: &'static [&'static str],
    tiles: &'static [&'static str],
    store_sections: &'static [&'static str],
) -> Feature {
    Feature {
        id,
        disable_keys,
        pages,
        tiles,
        store_sections,
    }
}

/// "core" is the plumbing no one switches off (store, API pipe, identity, autostart, ...).
pub static FEATURES: &[Feature] = &[
    f(
        "core",
        &["api", "apps-watch", "identity", "away"],
        &["App"],
        &["tile:profile"],
        &["autostart", "gpu_resets", "settings_window"],
    ),
    f(
        "foxglove.link",
        &[
            "status",
            "keepalive",
            "health",
            "reassert",
            "pc-state",
            "knob",
            "hold-lease",
            "mic",
        ],
        &[],
        &["tile:batteries", "tile:knob"],
        &[
            "connection",
            "battery",
            "health_notices",
            "knob",
            "master_volume",
            "focus",
        ],
    ),
    f(
        "foxglove.overlay",
        &["foreground", "accent"],
        &[],
        &[],
        &["overlay", "overlay_place", "overlay_shown", "window", "ui"],
    ),
    f(
        "foxglove.screens",
        &["screens", "providers", "notifications"],
        &["Screens"],
        &["tile:screens", "tile:screen-glow"],
        &["screens", "screen_glow", "notifications"],
    ),
    f(
        "foxglove.art",
        &["art"],
        &[],
        &[],
        &[
            "art",
            "art.cycle",
            "art.seen",
            "art.polarity",
            "art_library",
            "art_pushed",
        ],
    ),
    f(
        "foxglove.lights",
        &["lamp-hold"],
        &["Lights"],
        &["tile:lights"],
        &["lighting", "lights_palette", "led_map", "calibration"],
    ),
    f(
        "foxglove.keymap",
        &[],
        &["Keyboard"],
        &[],
        &["layers", "layout_files"],
    ),
    f("foxglove.firmware", &[], &[], &[], &["flash", "updates"]),
    f(
        "foxglove.plugins",
        &["plugins", "kits", "actions", "catalog"],
        &["Plugins"],
        &[],
        &[
            "plugins.installed",
            "plugins.refresh",
            "plugins.bindings",
            "plugins.settings",
            "plugins.trust",
            "plugins.dev",
            "plugins.grandfather",
        ],
    ),
    f(
        "hideout.scenes",
        &[],
        &["Scenes"],
        &[],
        &[
            "scenes",
            "scene_presets",
            "scene_resume",
            "scene_keyboard",
            "scene",
        ],
    ),
    f(
        "hideout.pclights",
        &["pc-lights"],
        &[],
        &[],
        &["pc_lighting", "openrgb"],
    ),
    f("hideout.peripherals", &[], &[], &[], &[]),
    f("qmk.via", &["via"], &[], &[], &["via"]),
    f(
        "wallpaper-engine",
        &["wallpaper", "live"],
        &[],
        &[],
        &["live_wallpaper", "wallpaper_engine"],
    ),
];

macro_rules! step {
    ($feature:literal, $path:path) => {
        step!($feature, $path, None)
    };
    ($feature:literal, $path:path, $gate:expr) => {
        Step {
            feature: $feature,
            name: stringify!($path),
            gate: $gate,
            start: || {
                let _ = $path();
            },
        }
    };
}

/// Every start `main` ran after discovering the devices, in the order it ran them.
pub static STEPS: &[Step] = &[
    step!("foxglove.screens", super::screens::start, Some("screens")),
    step!("foxglove.art", super::art_service::start, Some("art")),
    step!("core", super::store::init_profiles),
    step!("foxglove.overlay", super::foreground::start),
    step!("foxglove.overlay", super::accent::start),
    step!("foxglove.overlay", super::windows_theme::start),
    step!("wallpaper-engine", super::wallpaper::start),
    step!("core", super::api::start),
    step!("foxglove.link", super::health::start),
    step!("foxglove.link", super::reassert::start),
    step!("foxglove.link", super::pc_state::start),
    step!("foxglove.link", super::knob::start),
    step!("foxglove.link", super::hold_lease::start),
    step!("foxglove.link", super::devices::start),
    step!("foxglove.link", super::keyboard_facts::start),
    step!("foxglove.link", super::focus_pins::start),
    step!("foxglove.link", super::battery::start),
    step!("hideout.peripherals", super::peripherals::start),
    step!("wallpaper-engine", super::live_feed::start),
    step!("foxglove.keymap", super::layout_files::start),
    step!("qmk.via", super::via_host::start),
    step!("foxglove.link", super::follow_typing::start),
    step!("hideout.pclights", super::pc_lights::start),
    step!("foxglove.lights", super::lamp_hold::start),
    step!("hideout.scenes", super::scene_run::start_from_env),
    step!("hideout.scenes", super::scene_triggers::start),
    step!("hideout.scenes", super::scenes_ui::startup),
    step!("foxglove.plugins", super::plugin_install::autoinstall),
    step!("foxglove.plugins", super::plugin_runner::sync),
    step!("foxglove.plugins", super::plugin_actions::start),
    step!("foxglove.plugins", super::plugin_settings::start),
    step!("core", super::app_windows::start),
    step!("foxglove.link", super::mic_broker::start),
    step!("foxglove.screens", super::notifications::start),
    step!("foxglove.plugins", super::catalog_refresh::start),
    step!("core", super::identity::start),
    step!("core", super::autostart::start),
];

/// Runs `steps` in order, skipping a gated step whose key `disabled` says is off.
pub fn run(steps: &[Step], disabled: impl Fn(&str) -> bool) {
    for step in steps {
        if step.gate.is_some_and(&disabled) {
            continue;
        }
        (step.start)();
    }
}

/// Starts every feature (what `main` used to spell out).
pub fn start_all() {
    run(STEPS, super::repaint_trace::disabled);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;

    #[test]
    fn start_order_is_todays() {
        let names: Vec<String> = STEPS.iter().map(|s| s.name.replace(' ', "")).collect();
        let want = [
            "super::screens::start",
            "super::art_service::start",
            "super::store::init_profiles",
            "super::foreground::start",
            "super::accent::start",
            "super::windows_theme::start",
            "super::wallpaper::start",
            "super::api::start",
            "super::health::start",
            "super::reassert::start",
            "super::pc_state::start",
            "super::knob::start",
            "super::hold_lease::start",
            "super::devices::start",
            "super::keyboard_facts::start",
            "super::focus_pins::start",
            "super::battery::start",
            "super::peripherals::start",
            "super::live_feed::start",
            "super::layout_files::start",
            "super::via_host::start",
            "super::follow_typing::start",
            "super::pc_lights::start",
            "super::lamp_hold::start",
            "super::scene_run::start_from_env",
            "super::scene_triggers::start",
            "super::scenes_ui::startup",
            "super::plugin_install::autoinstall",
            "super::plugin_runner::sync",
            "super::plugin_actions::start",
            "super::plugin_settings::start",
            "super::app_windows::start",
            "super::mic_broker::start",
            "super::notifications::start",
            "super::catalog_refresh::start",
            "super::identity::start",
            "super::autostart::start",
        ];
        assert_eq!(names, want);
    }

    #[test]
    fn ids_keys_tiles_pages_and_sections_are_unique() {
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        let mut tiles = HashSet::new();
        let mut pages = HashSet::new();
        let mut sections = HashSet::new();
        for feat in FEATURES {
            assert!(ids.insert(feat.id), "duplicate feature id {}", feat.id);
            for k in feat.disable_keys {
                assert!(keys.insert(*k), "duplicate disable key {k}");
            }
            for t in feat.tiles {
                assert!(tiles.insert(*t), "duplicate tile {t}");
            }
            for p in feat.pages {
                assert!(pages.insert(*p), "duplicate page {p}");
            }
            for s in feat.store_sections {
                assert!(sections.insert(*s), "duplicate store section {s}");
            }
        }
    }

    #[test]
    fn every_step_names_a_feature_and_every_gate_is_its_key() {
        for s in STEPS {
            let feat = FEATURES.iter().find(|f| f.id == s.feature);
            let feat = feat.unwrap_or_else(|| panic!("{} has no feature {}", s.name, s.feature));
            if let Some(g) = s.gate {
                assert!(
                    feat.disable_keys.contains(&g),
                    "{} gate {g} not in {}",
                    s.name,
                    feat.id
                );
            }
        }
    }

    /// Every `disabled("key")` in the app's source is a key some feature lists.
    #[test]
    fn every_disable_key_in_the_code_is_in_the_table() {
        let known: HashSet<&str> = FEATURES
            .iter()
            .flat_map(|f| f.disable_keys.iter().copied())
            .collect();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = HashSet::new();
        let mut dirs = vec![root];
        while let Some(dir) = dirs.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let Ok(text) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    let pat = "disabled(\"";
                    for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
                        let mut rest = line;
                        while let Some(i) = rest.find(pat) {
                            let before = rest[..i].chars().last();
                            let after = &rest[i + pat.len()..];
                            if before.is_none_or(|c| !(c.is_alphanumeric() || c == '_')) {
                                if let Some(end) = after.find('"') {
                                    found.insert(after[..end].to_string());
                                }
                            }
                            rest = after;
                        }
                    }
                }
            }
        }
        assert!(found.len() >= 20, "scan found too little: {found:?}");
        let mut missing: Vec<_> = found
            .iter()
            .filter(|k| !known.contains(k.as_str()))
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "FOXGLOVE_DISABLE keys not in the table: {missing:?}"
        );
        // The two the old main gated itself are table gates.
        assert!(STEPS.iter().any(|s| s.gate == Some("screens")));
        assert!(STEPS.iter().any(|s| s.gate == Some("art")));
    }

    thread_local! {
        static RAN: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn fake_table() -> Vec<Step> {
        vec![
            Step {
                feature: "a",
                name: "a",
                gate: Some("ka"),
                start: || push("a"),
            },
            Step {
                feature: "b",
                name: "b",
                gate: None,
                start: || push("b"),
            },
            Step {
                feature: "c",
                name: "c",
                gate: Some("kc"),
                start: || push("c"),
            },
        ]
    }

    fn push(s: &'static str) {
        RAN.with(|r| r.borrow_mut().push(s));
    }

    fn ran_with(off: &[&str]) -> Vec<&'static str> {
        RAN.with(|r| r.borrow_mut().clear());
        run(&fake_table(), |k| off.contains(&k));
        RAN.with(|r| r.borrow().clone())
    }

    #[test]
    fn disabling_a_key_skips_exactly_its_start() {
        assert_eq!(ran_with(&[]), ["a", "b", "c"]);
        assert_eq!(ran_with(&["ka"]), ["b", "c"]);
        assert_eq!(ran_with(&["kc", "unrelated"]), ["a", "b"]);
    }
}
