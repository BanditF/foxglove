//! Pill rows as radio groups (#1085, item 3), page by page: the pages' own samples drawn
//! headless with AccessKit on, and the tree they give checked. One representative row per page,
//! and a sweep that every radio button anywhere sits in a radio group (and that no row of pills
//! is drawn outside one, in the source).

use egui::accesskit::{Node, NodeId, Role, Toggled, TreeUpdate};

use super::{clear_samples, preview_ctx, shell_window, show_samples, Samples};
use crate::foxglove::shell::Page;

/// The tree after a few frames of `draw` (so windows finish appearing).
fn tree(ctx: &egui::Context, size: egui::Vec2, mut draw: impl FnMut(&mut egui::Ui)) -> TreeUpdate {
    ctx.enable_accesskit();
    let mut last = None;
    for frame in 0..6 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(frame as f64),
            predicted_dt: 1.0,
            focused: true,
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, &mut draw);
        out.textures_delta.clear(); // nothing is painted here
        last = out.platform_output.accesskit_update;
    }
    last.expect("an accessibility tree")
}

/// A page of the settings window, drawn with `samples`.
fn page_tree(page: Page, samples: &Samples, height: f32) -> TreeUpdate {
    let ctx = preview_ctx();
    show_samples(page, samples);
    let t = tree(&ctx, egui::vec2(1072.0, height + 40.0), |ui| {
        shell_window(ui.ctx(), egui::vec2(1040.0, height));
    });
    clear_samples();
    t
}

/// A radio group as the tree has it: its name and its pills (name, checked).
#[derive(Debug)]
struct Group {
    name: String,
    pills: Vec<(String, bool)>,
}

fn nodes(t: &TreeUpdate) -> std::collections::HashMap<NodeId, &Node> {
    t.nodes.iter().map(|(id, n)| (*id, n)).collect()
}

/// Every radio group in `t`, in tree order.
fn groups(t: &TreeUpdate) -> Vec<Group> {
    let all = nodes(t);
    t.nodes
        .iter()
        .filter(|(_, n)| n.role() == Role::RadioGroup)
        .map(|(_, g)| Group {
            name: g.label().unwrap_or_default().to_owned(),
            pills: g
                .children()
                .iter()
                .filter_map(|c| all.get(c))
                .filter(|n| n.role() == Role::RadioButton)
                .map(|n| {
                    (
                        n.label().unwrap_or_default().to_owned(),
                        n.toggled() == Some(Toggled::True),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The group named `name` in `t`, with the groups there are when it's missing.
fn group<'a>(gs: &'a [Group], name: &str) -> &'a Group {
    gs.iter()
        .find(|g| g.name == name)
        .unwrap_or_else(|| panic!("no radio group {name:?} in {gs:#?}"))
}

/// The sweep's rule for one tree: every radio button's parent is a radio group, every group
/// holds only radio buttons, and each button says whether it's checked.
fn every_radio_in_a_group(t: &TreeUpdate, what: &str) {
    let all = nodes(t);
    let mut parent = std::collections::HashMap::new();
    for (id, n) in &t.nodes {
        for c in n.children() {
            assert!(
                parent.insert(*c, *id).is_none(),
                "{what}: {c:?} has two parents"
            );
        }
    }
    let mut radios = 0;
    for (id, n) in &t.nodes {
        match n.role() {
            Role::RadioButton => {
                radios += 1;
                let p = parent.get(id).and_then(|p| all.get(p));
                assert_eq!(
                    p.map(|p| p.role()),
                    Some(Role::RadioGroup),
                    "{what}: {:?} isn't in a radio group",
                    n.label()
                );
                assert!(
                    n.toggled().is_some(),
                    "{what}: {:?} has no checked state",
                    n.label()
                );
            }
            Role::RadioGroup => {
                assert!(
                    n.label().is_some_and(|l| !l.is_empty()),
                    "{what}: a radio group without a name"
                );
                assert!(!n.children().is_empty(), "{what}: {:?} is empty", n.label());
                for c in n.children() {
                    assert_eq!(
                        all.get(c).map(|n| n.role()),
                        Some(Role::RadioButton),
                        "{what}: {:?} holds something else",
                        n.label()
                    );
                }
            }
            _ => {}
        }
    }
    assert!(radios > 0, "{what}: no radio buttons drawn");
}

/// Lights: the keyboard's Effect row and its Off/On row.
#[test]
fn the_lights_page_rows_are_radio_groups() {
    let t = page_tree(Page::Lights, &Samples::full(), 1180.0);
    let gs = groups(&t);
    let effect = group(&gs, "Effect");
    let names: Vec<_> = effect.pills.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        &names[..crate::foxglove::lights_page::EFFECTS.len()],
        &crate::foxglove::lights_page::EFFECTS[..],
        "{gs:#?}"
    );
    assert_eq!(
        effect.pills.iter().filter(|(_, on)| *on).count(),
        1,
        "{gs:#?}"
    );
    let lights = group(&gs, "Lights");
    assert_eq!(
        lights
            .pills
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Off", "On"]
    );
    every_radio_in_a_group(&t, "Lights page");
}

/// Scenes: the effect kinds, and a parameter row under them.
#[test]
fn the_scenes_page_rows_are_radio_groups() {
    use crate::foxglove::scene_looks::Kind;
    let t = page_tree(Page::Scenes, &Samples::full(), 1100.0);
    let gs = groups(&t);
    let kinds = group(&gs, "Effect");
    assert_eq!(kinds.pills.len(), Kind::ALL.len(), "{gs:#?}");
    assert_eq!(
        kinds.pills.iter().filter(|(_, on)| *on).count(),
        1,
        "{gs:#?}"
    );
    every_radio_in_a_group(&t, "Scenes page");
}

/// Key rings (#490, docs/key-rings-ux.md "Accessibility"): "Rings start" is a radio group of
/// two, the picked one checked, each pill described by its hover; the greyed boxes say why as
/// their description (state A: the covered ripple box; B: "Keep running" too).
#[test]
fn the_rings_start_row_and_the_greyed_boxes_speak() {
    use crate::foxglove::scene_looks::Starts;
    use crate::foxglove::scenes_ui::{tests, KEEP_NO_RINGS, RINGS_COVERED};
    let all = |t: &TreeUpdate| nodes(t).into_values().cloned().collect::<Vec<Node>>();
    let named = |ns: &[Node], name: &str| -> Node {
        ns.iter()
            .find(|n| n.label() == Some(name))
            .cloned()
            .unwrap_or_else(|| panic!("no node {name:?}"))
    };
    for keyboard in [true, false] {
        let mut s = Samples::full();
        s.scenes = (tests::rings_sample(keyboard, false), Default::default());
        let t = page_tree(Page::Scenes, &s, 1100.0);
        let gs = groups(&t);
        let rings = group(&gs, Starts::CAPTION);
        assert_eq!(
            rings.pills,
            [
                ("On a loop".to_string(), false),
                ("On each key press".to_string(), true)
            ],
            "{gs:#?}"
        );
        let ns = all(&t);
        for (i, s) in Starts::ALL.iter().enumerate() {
            let pill = named(&ns, s.label());
            assert_eq!(pill.role(), Role::RadioButton);
            assert_eq!(pill.position_in_set(), Some(i + 1));
            assert_eq!(pill.size_of_set(), Some(2));
            assert_eq!(pill.description(), Some(s.hint()), "{}", s.label());
        }
        let ripple = named(&ns, "Ripple on key press");
        assert_eq!(ripple.role(), Role::CheckBox);
        assert_eq!(ripple.toggled(), Some(Toggled::True), "shown ticked");
        assert!(ripple.is_disabled());
        assert_eq!(ripple.description(), Some(RINGS_COVERED));
        let keep = named(&ns, "Keep running on the keyboard");
        if keyboard {
            assert!(!keep.is_disabled(), "A: works as today");
            assert_eq!(keep.description(), None);
        } else {
            assert!(keep.is_disabled(), "B");
            assert_eq!(keep.toggled(), Some(Toggled::False));
            assert_eq!(keep.description(), Some(KEEP_NO_RINGS));
        }
        every_radio_in_a_group(&t, "Scenes page, key rings");
    }
}

/// Screen glow's Smoothing row.
#[test]
fn the_screen_glow_smoothing_row_is_a_radio_group() {
    use crate::foxglove::scene_looks::{Kind, Look};
    use crate::foxglove::scenes_ui::{tests, View};
    let mut s = Samples::full();
    s.scenes = (
        View {
            look: Look {
                kind: Kind::ScreenGlow,
                ..Look::default()
            },
            ..tests::sample()
        },
        Default::default(),
    );
    let t = page_tree(Page::Scenes, &s, 1100.0);
    let gs = groups(&t);
    let smoothing = group(&gs, crate::foxglove::screen_glow::SMOOTHING);
    assert_eq!(
        smoothing.pills.len(),
        foxglove_scene::Smoothing::ALL.len(),
        "{gs:#?}"
    );
    assert_eq!(
        smoothing.pills.iter().filter(|(_, on)| *on).count(),
        1,
        "{gs:#?}"
    );
    every_radio_in_a_group(&t, "Screen glow");
}

/// Settings: the window theme picker.
#[test]
fn the_settings_page_theme_row_is_a_radio_group() {
    let t = page_tree(Page::Settings, &Samples::full(), 1400.0);
    let gs = groups(&t);
    let theme = group(&gs, "Window theme");
    assert_eq!(
        theme
            .pills
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["Dark", "Light", "System"]
    );
    every_radio_in_a_group(&t, "Settings page");
}

/// Screens: the art library's filter row.
#[test]
fn the_screens_page_rows_are_radio_groups() {
    let t = page_tree(Page::Screens, &Samples::full(), 1400.0);
    let gs = groups(&t);
    let show = group(&gs, "Show");
    assert_eq!(
        show.pills.iter().filter(|(_, on)| *on).count(),
        1,
        "{gs:#?}"
    );
    every_radio_in_a_group(&t, "Screens page");
}

/// The colour calibrator: what to compare with, and the test colours.
#[test]
fn the_calibrator_rows_are_radio_groups() {
    use crate::foxglove::calibrate_ui::{tests::sample, Step};
    let mut s = Samples::full();
    s.calibrate = Some(sample(Step::Red, false));
    let t = page_tree(Page::Lights, &s, 1180.0);
    let gs = groups(&t);
    let compare = group(&gs, "Compare with");
    assert_eq!(
        compare.pills.first().map(|(n, _)| n.as_str()),
        Some("Screen"),
        "{gs:#?}"
    );
    let colours = group(&gs, "Test colour");
    assert_eq!(colours.pills.len(), Step::ALL.len(), "{gs:#?}");
    every_radio_in_a_group(&t, "Calibrator");
}

/// The tray panel: Profile and the keyboard's effect.
#[test]
fn the_tray_panel_rows_are_radio_groups() {
    let ctx = preview_ctx();
    let v = crate::foxglove::tray_panel::tests::sample();
    let size = egui::vec2(crate::foxglove::tray_panel::WIDTH + 40.0, 720.0);
    let t = tree(&ctx, size, |ui| {
        egui::Area::new(egui::Id::new("tray-panel-preview"))
            .fixed_pos(egui::pos2(20.0, 20.0))
            .show(ui.ctx(), |ui| {
                crate::foxglove::tray_panel::panel(
                    ui,
                    &v,
                    &mut Default::default(),
                    &mut Vec::new(),
                );
            });
    });
    let gs = groups(&t);
    let profile = group(&gs, "Profile");
    assert_eq!(
        profile.pills.first().map(|(n, _)| n.as_str()),
        Some("Default"),
        "{gs:#?}"
    );
    assert_eq!(
        profile.pills.iter().filter(|(_, on)| *on).count(),
        1,
        "{gs:#?}"
    );
    let effect = group(&gs, "Effect");
    assert_eq!(
        effect.pills.len(),
        crate::foxglove::lights_page::EFFECTS.len(),
        "{gs:#?}"
    );
    every_radio_in_a_group(&t, "Tray panel");
}

/// The sweep, in the source: a row of pills is drawn through `theme::pill_group`. A bare
/// `theme::pill` is only for a pill on its own (a toggle), and those are listed here by what
/// they say; `pill_button` stays inside the theme.
#[test]
fn no_pill_row_is_drawn_outside_a_group() {
    const LONE: [(&str, &str); 2] = [
        // Pause / Resume: one toggle.
        ("tray_panel.rs", "theme::pill(ui, v.paused, label)"),
        // The theme review's sample of a pill beside a chip.
        ("theme_preview.rs", "theme::pill(ui, true, \"Solid\")"),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/foxglove");
    let mut bad = Vec::new();
    let mut seen = Vec::new();
    let mut files = vec![dir.clone()];
    while let Some(d) = files.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                files.push(path);
                continue;
            }
            if path.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "pill_groups.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.starts_with("//") || line.starts_with("(\"") {
                    continue;
                }
                if line.contains("theme::pill(") {
                    match LONE.iter().find(|(f, l)| *f == name && line.contains(l)) {
                        Some(l) => seen.push(*l),
                        None => bad.push(format!("{name}:{}: {line}", n + 1)),
                    }
                }
                if name != "theme.rs" && line.contains("pill_button(") {
                    bad.push(format!("{name}:{}: {line}", n + 1));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "pills outside a pill_group:\n{}",
        bad.join("\n")
    );
    for l in LONE {
        assert!(seen.contains(&l), "{l:?} is gone: drop it from the list");
    }
}
