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

/// How a feature gives way to another module that provides its methods (#1143, P9).
#[derive(Clone, Copy)]
pub enum How {
    /// Its device duties end as a quit ends them (lights back to the firmware or the next
    /// controller). Nothing starts them again in this process: when the other module goes, the
    /// legacy module asks the hub for a planned restart ([`Applied::restart`]) and the feature
    /// starts with it.
    Release(fn()),
    /// The feature asks [`is_off`] before it writes and stops writing at once; it is on again
    /// the moment the other module is gone.
    Gate,
}

/// A feature that can hand its job to a module: the v1 method patterns it serves for the hub
/// (the areas such a module would provide) and how it gives way. Features not listed here are
/// never switched off by a hand-off: the keyboard link, screens, art, the overlay and the
/// core (their areas are the keyboard grant's, #1155-B, or the app's own plumbing), and the
/// never-names methods (reset, bootloader, flash, erase, dfu, bond) stay out of the hub link
/// whatever this table says.
pub struct HandOff {
    pub feature: &'static str,
    pub methods: &'static [&'static str],
    pub how: How,
}

pub static HAND_OFFS: &[HandOff] = &[
    HandOff {
        feature: "foxglove.lights",
        methods: &["lights.*"],
        how: How::Release(super::lamp_hold::stop),
    },
    HandOff {
        feature: "hideout.scenes",
        methods: &["scenes.*"],
        how: How::Release(|| {
            super::scenes_ui::quitting();
            super::scene_run::quit();
        }),
    },
    HandOff {
        feature: "hideout.pclights",
        methods: &["pc_lighting.*"],
        how: How::Gate,
    },
    HandOff {
        feature: "wallpaper-engine",
        methods: &["wallpaper-engine.*"],
        how: How::Release(|| {
            super::live_feed::stop();
            super::wallpaper_sync::quit();
        }),
    },
];

/// Whether method pattern `a` covers `b` (`x.*` covers `x.y`; a name covers itself).
fn covers(a: &str, b: &str) -> bool {
    match a.strip_suffix('*') {
        Some(prefix) => b.starts_with(prefix),
        None => a == b,
    }
}

fn overlap(a: &str, b: &str) -> bool {
    covers(a, b) || covers(b, a)
}

/// The features whose methods another module provides: `provided` is each other registered
/// module's method patterns (the hub's `module.<id>.provides`), `own` the legacy module's id
/// (never counted against itself). Sorted by table order.
pub fn plan(
    own: &str,
    provided: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<&'static str> {
    HAND_OFFS
        .iter()
        .filter(|h| {
            provided.iter().any(|(module, patterns)| {
                module != own
                    && patterns
                        .iter()
                        .any(|p| h.methods.iter().any(|m| overlap(m, p)))
            })
        })
        .map(|h| h.feature)
        .collect()
}

/// Whether `method` belongs to one of the `off` features' areas.
pub fn withheld(method: &str, off: &[&str]) -> bool {
    HAND_OFFS
        .iter()
        .filter(|h| off.contains(&h.feature))
        .any(|h| h.methods.iter().any(|m| covers(m, method)))
}

static OFF: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());

fn off_now() -> std::sync::MutexGuard<'static, Vec<&'static str>> {
    OFF.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether feature `id` is handed off right now (legacy module only; always false standalone).
pub fn is_off(id: &str) -> bool {
    off_now().contains(&id)
}

/// What [`apply`] changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Features switched off by this call.
    pub off: Vec<&'static str>,
    /// Features that are on again.
    pub back: Vec<&'static str>,
    /// A released feature came back: it cannot restart inside this process, so the caller asks
    /// the hub for a planned restart.
    pub restart: bool,
}

/// Makes `want` the set of handed-off features: runs the release of each newly off feature
/// (once) and clears the others. Pure bookkeeping for [`How::Gate`]; `release` runs the stop
/// of a [`How::Release`] feature (tests count calls instead of stopping devices).
pub fn apply_with(want: &[&'static str], release: &mut dyn FnMut(&HandOff)) -> Applied {
    let mut now = off_now();
    let mut done = Applied::default();
    for h in HAND_OFFS {
        let (is, wanted) = (now.contains(&h.feature), want.contains(&h.feature));
        if wanted && !is {
            now.push(h.feature);
            done.off.push(h.feature);
            if matches!(h.how, How::Release(_)) {
                release(h);
            }
        } else if is && !wanted {
            now.retain(|f| *f != h.feature);
            done.back.push(h.feature);
            done.restart |= matches!(h.how, How::Release(_));
        }
    }
    done
}

/// [`apply_with`] stopping the devices of a released feature.
pub fn apply(want: &[&'static str]) -> Applied {
    apply_with(want, &mut |h| {
        if let How::Release(stop) = h.how {
            stop();
        }
    })
}

/// Forgets every hand-off (tests; a process starts with none).
#[cfg(test)]
pub fn reset_off() {
    off_now().clear();
}

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

/// Runs `steps` in order, skipping a gated step whose key `disabled` says is off and the steps of
/// a feature that is handed off to another module (P9; nothing is, standalone).
pub fn run(steps: &[Step], disabled: impl Fn(&str) -> bool) {
    for step in steps {
        if step.gate.is_some_and(&disabled) || is_off(step.feature) {
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

    fn provided(pairs: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(m, ps)| (m.to_string(), ps.iter().map(|p| p.to_string()).collect()))
            .collect()
    }

    #[test]
    fn the_hand_off_table_names_features_and_never_the_keyboard_areas() {
        let mut seen = HashSet::new();
        for h in HAND_OFFS {
            assert!(is_feature(h.feature), "{} is no feature", h.feature);
            for m in h.methods {
                assert!(seen.insert(*m), "{m} listed twice");
                assert!(
                    ![
                        "keyboard",
                        "layers",
                        "reset",
                        "bootloader",
                        "flash",
                        "erase",
                        "dfu",
                        "bond"
                    ]
                    .iter()
                    .any(|n| m.starts_with(n)),
                    "{m} is a keyboard or never-names area"
                );
            }
        }
        for keep in ["foxglove.link", "foxglove.screens", "foxglove.art", "core"] {
            assert!(!HAND_OFFS.iter().any(|h| h.feature == keep));
        }
    }

    #[test]
    fn a_provider_of_a_v1_method_turns_that_feature_off_and_others_stay() {
        let own = "foxglove";
        assert_eq!(
            plan(own, &provided(&[("lamps", &["lights.set"])])),
            ["foxglove.lights"]
        );
        assert_eq!(
            plan(own, &provided(&[("x", &["scenes.*"])])),
            ["hideout.scenes"]
        );
        assert_eq!(
            plan(own, &provided(&[("x", &["*"])])).len(),
            HAND_OFFS.len()
        );
        // Not overlapping: nothing changes.
        assert!(plan(own, &provided(&[("x", &["other.thing", "lightshow.go"])])).is_empty());
        assert!(plan(own, &provided(&[])).is_empty());
        // Its own registration never turns itself off.
        assert!(plan(own, &provided(&[("foxglove", &["lights.*", "scenes.*"])])).is_empty());
    }

    #[test]
    fn withheld_covers_only_the_areas_of_the_off_features() {
        assert!(withheld("lights.set", &["foxglove.lights"]));
        assert!(!withheld("scenes.run", &["foxglove.lights"]));
        assert!(!withheld("lights.set", &[]));
        assert!(!withheld(
            "keyboard.layers",
            &["foxglove.lights", "hideout.scenes"]
        ));
    }

    #[test]
    fn handing_off_releases_once_skips_the_start_and_coming_back_asks_for_a_restart() {
        reset_off();
        let released = RefCell::new(Vec::new());
        let mut rel = |h: &HandOff| released.borrow_mut().push(h.feature);
        let a = apply_with(&["hideout.scenes", "hideout.pclights"], &mut rel);
        assert_eq!(a.off, ["hideout.scenes", "hideout.pclights"]);
        // Only the Release feature is stopped; the Gate one is a flag.
        assert_eq!(*released.borrow(), ["hideout.scenes"]);
        assert!(is_off("hideout.scenes") && is_off("hideout.pclights"));
        // Again: no second release.
        let again = apply_with(&["hideout.scenes", "hideout.pclights"], &mut rel);
        assert_eq!(again, Applied::default());
        assert_eq!(released.borrow().len(), 1);
        // A start of an off feature is not called.
        let table = [
            Step {
                feature: "hideout.scenes",
                name: "s",
                gate: None,
                start: || push("scenes"),
            },
            Step {
                feature: "core",
                name: "c",
                gate: None,
                start: || push("core"),
            },
        ];
        RAN.with(|r| r.borrow_mut().clear());
        run(&table, |_| false);
        assert_eq!(RAN.with(|r| r.borrow().clone()), ["core"]);
        // The Gate feature comes back at once, no restart.
        let back = apply_with(&["hideout.scenes"], &mut rel);
        assert_eq!(back.back, ["hideout.pclights"]);
        assert!(!back.restart && !is_off("hideout.pclights"));
        // The Release feature returns: a restart is asked for.
        let back = apply_with(&[], &mut rel);
        assert_eq!(back.back, ["hideout.scenes"]);
        assert!(back.restart);
        RAN.with(|r| r.borrow_mut().clear());
        run(&table, |_| false);
        assert_eq!(RAN.with(|r| r.borrow().clone()), ["scenes", "core"]);
        reset_off();
    }
}
