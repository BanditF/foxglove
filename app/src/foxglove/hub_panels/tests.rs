//! Each body state of the UX plan's table (§4), the exact wording (§5), the reading order (§6)
//! and the vectors the hub's render tests draw (`tests/hideout-vectors/modules/foxglove.json`).

use super::*;
use crate::foxglove::hub_link::manifest;
use hideout_contract::module::{Context, Module};

fn parsed() -> Module {
    Module::parse(&manifest(r"C:\x\foxglove.exe"), &Context::default()).expect("manifest")
}
use hideout_contract::panel::{register, Register, LIMITS};

fn t(k: &str, v: &str) -> (String, Fact) {
    (k.into(), Fact::Text(v.into()))
}
fn i(k: &str, v: i64) -> (String, Fact) {
    (k.into(), Fact::Int(v))
}
fn b(k: &str, v: bool) -> (String, Fact) {
    (k.into(), Fact::Bool(v))
}

/// A connected board over Bluetooth with both halves, orange solid lights at 80 %, built-in art.
fn connected() -> Vec<(String, Fact)> {
    vec![
        b("keyboard.connected", true),
        t("keyboard.name", "Eyelash Corne"),
        t("keyboard.link", "ble"),
        i("keyboard.profile", 2),
        i("keyboard.left.battery", 77),
        i("keyboard.right.battery", 91),
        i("keyboard.layer", 3),
        t("keyboard.layer_name", "GAME"),
        b("keyboard.lighting.on", true),
        t("keyboard.lighting.effect", "Solid"),
        i("keyboard.lighting.brightness", 80),
        b("keyboard.lighting.rainbow", false),
        t("keyboard.lighting.color_name", "Orange"),
        t("keyboard.lighting.source", "keyboard"),
        t("keyboard.art.name", "Fox walk"),
        t("keyboard.art.from", "built_in"),
    ]
}

fn without(mut f: Vec<(String, Fact)>, prefixes: &[&str]) -> Vec<(String, Fact)> {
    f.retain(|(k, _)| !prefixes.iter().any(|p| k.starts_with(p)));
    f
}

fn with(mut f: Vec<(String, Fact)>, set: Vec<(String, Fact)>) -> Vec<(String, Fact)> {
    for (k, v) in set {
        f.retain(|(key, _)| *key != k);
        f.push((k, v));
    }
    f
}

fn view(f: &[(String, Fact)]) -> View {
    View::of(f, Stopped::default())
}

fn panel<'a>(all: &'a [Value], id: &str) -> &'a Value {
    all.iter().find(|p| p["id"] == id).expect("panel")
}

/// Every text of a tree, in reading order, and its button labels.
fn texts(tree: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![tree];
    while let Some(n) = stack.pop() {
        if let Some(s) = n["props"]["text"]
            .as_str()
            .or_else(|| n["props"]["value"].as_str())
            .or_else(|| n["props"]["label"].as_str())
        {
            out.push(s.to_string());
        }
        if let Some(kids) = n["children"].as_array() {
            stack.extend(kids.iter().rev());
        }
    }
    out
}

fn tile_texts(all: &[Value], id: &str) -> Vec<String> {
    texts(&panel(all, id)["tree"])
}

/// What the hub accepts: every panel passes the contract's own checks, with the tile caps.
fn assert_valid(all: &[Value]) {
    let (ok, rejected) = register(
        &Register {
            instance: "x".into(),
            panels: all.to_vec(),
        },
        Some(&declared()),
        &LIMITS,
    );
    assert!(rejected.is_empty(), "rejected: {rejected:?}");
    assert_eq!(ok.len(), 4);
}

#[test]
fn the_connected_tiles_say_what_the_plan_words() {
    let all = panels(&view(&connected()));
    assert_valid(&all);
    assert_eq!(
        tile_texts(&all, TILE_KEYBOARD),
        ["Connected over Bluetooth", "Left 77 % · Right 91 %", OPEN]
    );
    assert_eq!(
        tile_texts(&all, TILE_LIGHTS),
        ["Solid · 80 %", "Orange", OPEN]
    );
    assert_eq!(
        tile_texts(&all, TILE_ART),
        ["Fox walk", "Built-in set", OPEN]
    );
    // The titles are not "Lights" and "Art": the hub has pages of those names.
    let titles: Vec<_> = all.iter().map(|p| p["title"].as_str().unwrap()).collect();
    assert_eq!(
        titles,
        ["Keyboard", "Keyboard lights", "Keyboard art", "Foxglove"]
    );
    let icons: Vec<_> = all.iter().map(|p| p["icon"].as_str().unwrap()).collect();
    assert_eq!(icons, ["keyboard", "lights", "image", "keyboard"]);
}

#[test]
fn usb_with_no_battery_yet_and_the_other_art_sources() {
    let f = with(
        without(
            connected(),
            &["keyboard.left", "keyboard.right", "keyboard.profile"],
        ),
        vec![t("keyboard.link", "usb"), t("keyboard.art.from", "kept")],
    );
    let all = panels(&view(&f));
    assert_eq!(
        tile_texts(&all, TILE_KEYBOARD),
        [
            "Connected over USB",
            "Battery shows once the keyboard reports it.",
            OPEN
        ]
    );
    assert_eq!(tile_texts(&all, TILE_ART)[1], "Kept on the keyboard");
    let f = with(f, vec![t("keyboard.art.from", "sent")]);
    assert_eq!(
        tile_texts(&panels(&view(&f)), TILE_ART)[1],
        "Sent from this PC"
    );
    // Only the parts that reported.
    let f = with(connected(), vec![]);
    let f = without(f, &["keyboard.left"]);
    assert_eq!(
        tile_texts(&panels(&view(&f)), TILE_KEYBOARD)[1],
        "Right 91 %"
    );
}

#[test]
fn waiting_for_the_first_facts() {
    let all = panels(&view(&[]));
    assert_valid(&all);
    for (id, line) in [
        (
            TILE_KEYBOARD,
            "Its state shows here once the keyboard reports it.",
        ),
        (
            TILE_LIGHTS,
            "The lights show here once the keyboard reports them.",
        ),
        (TILE_ART, "The art shows here once the keyboard reports it."),
    ] {
        assert_eq!(
            tile_texts(&all, id),
            ["Waiting for the keyboard", line, OPEN],
            "{id}"
        );
    }
    // Connected but the lights and the art not reported yet: still waiting, not "Off".
    let f = vec![b("keyboard.connected", true), t("keyboard.link", "usb")];
    let all = panels(&view(&f));
    assert_eq!(tile_texts(&all, TILE_LIGHTS)[0], "Waiting for the keyboard");
    assert_eq!(tile_texts(&all, TILE_ART)[0], "Waiting for the keyboard");
    assert_eq!(tile_texts(&all, TILE_KEYBOARD)[0], "Connected over USB");
}

#[test]
fn not_connected_says_so_on_all_three_and_the_page_loses_its_rows() {
    let f = vec![b("keyboard.connected", false)];
    let all = panels(&view(&f));
    assert_valid(&all);
    for id in [TILE_KEYBOARD, TILE_LIGHTS, TILE_ART] {
        assert_eq!(
            tile_texts(&all, id),
            ["Not connected", "Turn it on or plug it in.", OPEN],
            "{id}"
        );
        assert_eq!(
            panel(&all, id)["tree"]["children"][0]["props"]["tone"],
            "off"
        );
    }
    // The page: one line "Not connected" per section, then its button.
    assert_eq!(
        texts(&panel(&all, PAGE)["tree"]),
        [
            INTRO,
            "Not connected",
            OPEN,
            "Not connected",
            OPEN,
            "Not connected",
            OPEN
        ]
    );
}

#[test]
fn lights_off_and_lights_held_by_windows_or_a_scene() {
    let off = with(connected(), vec![b("keyboard.lighting.on", false)]);
    let all = panels(&view(&off));
    assert_eq!(
        tile_texts(&all, TILE_LIGHTS),
        ["Off", "The keyboard's lights are off.", OPEN]
    );
    assert!(texts(&panel(&all, PAGE)["tree"]).contains(&"Lights: Off".to_string()));

    let windows = with(connected(), vec![t("keyboard.lighting.source", "windows")]);
    let all = panels(&view(&windows));
    assert_eq!(
        tile_texts(&all, TILE_LIGHTS),
        [
            "Solid · 80 %",
            "Windows Dynamic Lighting is drawing them",
            OPEN
        ]
    );
    // The same line on the page, under the colour.
    let page = texts(&panel(&all, PAGE)["tree"]);
    let at = |s: &str| {
        page.iter()
            .position(|x| x == s)
            .unwrap_or_else(|| panic!("{s}"))
    };
    assert!(at("Colour: Orange") < at("Windows Dynamic Lighting is drawing them"));

    let scene = with(
        connected(),
        vec![
            t("keyboard.lighting.source", "scene"),
            t("keyboard.lighting.effect", "Bandit scanline"),
            t("keyboard.lighting.color_name", "Custom colour"),
        ],
    );
    let all = panels(&view(&scene));
    assert_eq!(
        tile_texts(&all, TILE_LIGHTS),
        ["Bandit scanline · 80 %", "Bandit scanline is running", OPEN]
    );
}

#[test]
fn rainbow_effects_make_their_own_colours() {
    let f = with(
        without(connected(), &["keyboard.lighting.color_name"]),
        vec![
            t("keyboard.lighting.effect", "Spectrum"),
            b("keyboard.lighting.rainbow", true),
        ],
    );
    let all = panels(&view(&f));
    assert_eq!(
        tile_texts(&all, TILE_LIGHTS),
        ["Spectrum · 80 %", "Makes its own colours", OPEN]
    );
}

#[test]
fn a_stopped_foxglove_page_puts_one_notice_on_its_own_tile_and_section() {
    let v = View::of(
        &connected(),
        Stopped {
            lights: true,
            ..Stopped::default()
        },
    );
    let all = panels(&v);
    assert_valid(&all);
    assert_eq!(
        tile_texts(&all, TILE_LIGHTS),
        ["Solid · 80 %", STOPPED, OPEN]
    );
    // The values stay; the others are untouched.
    assert_eq!(tile_texts(&all, TILE_KEYBOARD).len(), 3);
    assert_eq!(tile_texts(&all, TILE_ART).len(), 3);
    let notice = &panel(&all, TILE_LIGHTS)["tree"]["children"][1];
    assert_eq!(notice["props"]["tone"], "attention");
    let page = texts(&panel(&all, PAGE)["tree"]);
    assert_eq!(page.iter().filter(|s| *s == STOPPED).count(), 1);
    assert_eq!(STOPPED, "Foxglove's page stopped working.");
    // Art lives on Screens, so that page's guard is the art tile's.
    let v = View::of(
        &connected(),
        Stopped {
            screens: true,
            ..Stopped::default()
        },
    );
    assert_eq!(tile_texts(&panels(&v), TILE_ART)[1], STOPPED);
}

#[test]
fn a_long_name_is_cut_on_a_whole_character_and_fits_a_tile() {
    let long = "Ä".repeat(80);
    let f = with(connected(), vec![t("keyboard.art.name", &long)]);
    let all = panels(&view(&f));
    assert_valid(&all);
    let shown = &tile_texts(&all, TILE_ART)[0];
    assert!(shown.ends_with('…') && shown.len() <= 64, "{shown}");
    assert!(shown.chars().all(|c| c == 'Ä' || c == '…'));
    assert_eq!(cut("short"), "short");
}

/// §6: the page reads "Foxglove", its intro, then each section: its rows (label then value),
/// then its button, in the order Keyboard, Keyboard lights, Keyboard art; each tile is its glance,
/// its line, then its button.
#[test]
fn the_reading_order_is_the_plans() {
    let all = panels(&view(&connected()));
    let page = panel(&all, PAGE);
    assert_eq!(page["title"], "Foxglove");
    let kids = page["tree"]["children"].as_array().unwrap();
    assert_eq!(kids[0]["props"]["text"], INTRO);
    let sections: Vec<_> = kids[1..]
        .iter()
        .map(|s| s["props"]["title"].as_str().unwrap())
        .collect();
    assert_eq!(sections, ["Keyboard", "Keyboard lights", "Keyboard art"]);
    for s in &kids[1..] {
        let rows = s["children"].as_array().unwrap();
        let last = rows.last().unwrap();
        assert_eq!(last["type"], "button", "{}", s["id"]);
        assert_eq!(last["props"]["label"], OPEN);
        assert!(rows[..rows.len() - 1].iter().all(|r| r["type"] != "button"));
    }
    let keyboard = texts(&kids[1]);
    assert_eq!(
        keyboard,
        [
            "Eyelash Corne",
            "Connected over Bluetooth, profile 2",
            "Layer GAME",
            "Left battery 77 %",
            "Right battery 91 %",
            OPEN
        ]
    );
    assert_eq!(
        texts(&kids[2]),
        ["Effect: Solid", "Brightness: 80 %", "Colour: Orange", OPEN]
    );
    assert_eq!(texts(&kids[3]), ["Fox walk", "Built-in set", OPEN]);
    for id in [TILE_KEYBOARD, TILE_LIGHTS, TILE_ART] {
        let kinds: Vec<_> = panel(&all, id)["tree"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["value", "label", "button"], "{id}");
    }
}

/// A click names the panel and the node; the tile's one button and each section's open the
/// page of that part, and nothing else opens anything.
#[test]
fn a_click_opens_the_matching_page() {
    assert_eq!(Part::of_event("keyboard", "go"), Some(Part::Keyboard));
    assert_eq!(Part::of_event("lights", "go"), Some(Part::Lights));
    assert_eq!(Part::of_event("art", "go"), Some(Part::Art));
    assert_eq!(Part::of_event("page", "go-lights"), Some(Part::Lights));
    assert_eq!(Part::of_event("page", "go-art"), Some(Part::Art));
    assert_eq!(Part::of_event("page", "go"), None);
    assert_eq!(Part::of_event("lights", "go-art"), None);
    assert_eq!(Part::of_event("nope", "go"), None);
    assert_eq!(Part::Keyboard.settings_page(), Page::Keyboard);
    assert_eq!(Part::Lights.settings_page(), Page::Lights);
    assert_eq!(
        Part::Art.settings_page(),
        Page::Screens,
        "art lives on Screens"
    );
    // Every button the panels draw is one of those.
    let all = panels(&view(&connected()));
    for p in &all {
        let id = p["id"].as_str().unwrap();
        let mut stack = vec![&p["tree"]];
        while let Some(n) = stack.pop() {
            if n["type"] == "button" {
                let node = n["id"].as_str().unwrap();
                assert!(Part::of_event(id, node).is_some(), "{id}/{node}");
                assert!(n["props"].get("action").is_none() && n["props"].get("opens").is_none());
            }
            if let Some(k) = n["children"].as_array() {
                stack.extend(k);
            }
        }
    }
}

#[test]
fn the_view_reads_the_facts_the_app_publishes() {
    let mut f = connected();
    f.push(i("keyboard.central.battery", 50));
    f.push(i("keyboard.lighting.brightness_x", 1));
    let v = view(&f);
    assert_eq!(v.connected, Some(true));
    assert_eq!(
        v.batteries,
        [
            ("left".to_string(), 77),
            ("right".to_string(), 91),
            ("central".to_string(), 50)
        ],
        "halves first"
    );
    assert_eq!(v.art, Some(("Fox walk".into(), "built_in".into())));
    assert_eq!(v.lights.source, Some(Source::Keyboard));
    assert_eq!(view(&[]).connected, None);
}

/// The manifest declares exactly the panels the module registers (like the `SERVED` test), in
/// the slots the plan names.
#[test]
fn the_manifest_declares_what_ui_register_sends() {
    let m = parsed();
    let declared: Vec<_> = m.panels.iter().map(|p| p.id.clone()).collect();
    assert_eq!(declared, super::declared());
    let slots: Vec<_> = m
        .panels
        .iter()
        .map(|p| serde_json::to_value(&p.slot).unwrap())
        .collect();
    assert_eq!(slots, ["tile", "tile", "tile", "page"]);
    let sent = panels(&view(&[]));
    for (decl, panel) in m.panels.iter().zip(&sent) {
        assert_eq!(decl.id, panel["id"]);
        assert_eq!(decl.title, panel["title"]);
        assert_eq!(decl.icon, panel["icon"]);
        assert_eq!(serde_json::to_value(&decl.slot).unwrap(), panel["slot"]);
    }
    // No new permission: the facts grant covers it.
    assert!(
        !m.permissions
            .iter()
            .any(|p| format!("{p:?}").contains("ui")),
        "{:?}",
        m.permissions
    );
}

/// The vectors the hub's window-free tests draw: one per body state.
#[test]
fn the_panels_match_the_hub_vectors() {
    let m = parsed();
    let declared: Vec<String> = m.panels.iter().map(|p| p.id.clone()).collect();
    let stopped = View::of(
        &connected(),
        Stopped {
            lights: true,
            ..Stopped::default()
        },
    );
    let long = with(connected(), vec![t("keyboard.art.name", &"Ä".repeat(80))]);
    let states: Vec<(&str, Vec<Value>)> = vec![
        ("Waiting for the keyboard", panels(&view(&[]))),
        ("Connected", panels(&view(&connected()))),
        (
            "Connected over USB, no battery yet",
            panels(&view(&with(
                without(
                    connected(),
                    &["keyboard.left", "keyboard.right", "keyboard.profile"],
                ),
                vec![t("keyboard.link", "usb"), t("keyboard.art.from", "kept")],
            ))),
        ),
        (
            "Not connected",
            panels(&view(&[b("keyboard.connected", false)])),
        ),
        (
            "Lights off",
            panels(&view(&with(
                connected(),
                vec![b("keyboard.lighting.on", false)],
            ))),
        ),
        (
            "Lights held by Windows",
            panels(&view(&with(
                connected(),
                vec![t("keyboard.lighting.source", "windows")],
            ))),
        ),
        (
            "Lights held by a scene",
            panels(&view(&with(
                connected(),
                vec![
                    t("keyboard.lighting.source", "scene"),
                    t("keyboard.lighting.effect", "Bandit scanline"),
                    t("keyboard.lighting.color_name", "Custom colour"),
                ],
            ))),
        ),
        (
            "Lights make their own colours",
            panels(&view(&with(
                without(connected(), &["keyboard.lighting.color_name"]),
                vec![
                    t("keyboard.lighting.effect", "Spectrum"),
                    b("keyboard.lighting.rainbow", true),
                ],
            ))),
        ),
        ("A Foxglove page stopped", panels(&stopped)),
        ("A long piece name", panels(&view(&long))),
    ];
    for (name, all) in &states {
        assert_valid(all);
        let _ = name;
    }
    foxglove_plugin::ui::check_vectors("foxglove.json", (&m.id, &m.name, &declared, &[]), states);
}
