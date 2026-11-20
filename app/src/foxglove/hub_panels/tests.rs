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
        [
            "Bandit scanline · 80 %",
            "A Foxglove scene is drawing them",
            OPEN
        ]
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
        .map(|p| serde_json::to_value(p.slot).unwrap())
        .collect();
    assert_eq!(slots, ["tile", "tile", "tile", "page"]);
    let sent = panels(&view(&[]));
    for (decl, panel) in m.panels.iter().zip(&sent) {
        assert_eq!(decl.id, panel["id"]);
        assert_eq!(decl.title, panel["title"]);
        assert_eq!(decl.icon, panel["icon"]);
        assert_eq!(serde_json::to_value(decl.slot).unwrap(), panel["slot"]);
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
        (
            "A long effect name at full brightness",
            panels(&view(&with(
                connected(),
                vec![
                    t("keyboard.lighting.effect", &"Ä".repeat(70)),
                    i("keyboard.lighting.brightness", 100),
                ],
            ))),
        ),
        ("A Foxglove page stopped", panels(&stopped)),
        ("A long piece name", panels(&view(&long))),
        (
            "Game mode on",
            panels(&view(&with(
                connected(),
                vec![b("keyboard.game_mode", true)],
            ))),
        ),
        (
            "Game mode off",
            panels(&view(&with(
                connected(),
                vec![
                    b("keyboard.game_mode", false),
                    t("keyboard.layer_name", "BASE"),
                    i("keyboard.layer", 0),
                ],
            ))),
        ),
        (
            "Wallpaper Engine playing",
            panels(&we_view(Wallpaper {
                running: true,
                ..Wallpaper::default()
            })),
        ),
        (
            "Wallpapers paused by Arete",
            panels(&we_view(held(&["Arete"]))),
        ),
        (
            "Wallpapers paused by two apps",
            panels(&we_view(Wallpaper {
                reasons: vec!["loading a model".into(), "rendering".into()],
                ..held(&["Arete", "Render queue"])
            })),
        ),
        (
            "Wallpapers paused with a full-screen app in front",
            panels(&we_view(Wallpaper {
                full_screen: true,
                ..held(&["Arete"])
            })),
        ),
        (
            "Wallpapers paused by you, an app waiting too",
            panels(&we_view(Wallpaper {
                user_paused: true,
                ..held(&["Arete"])
            })),
        ),
        (
            "Wallpaper Engine not running",
            panels(&we_view(Wallpaper::default())),
        ),
        (
            "Wallpapers: no app is pausing them",
            panels(&we_view(Wallpaper {
                running: true,
                ..Wallpaper::default()
            })),
        ),
        (
            "Wallpapers: just resumed",
            panels(&we_view(Wallpaper {
                running: true,
                after: Some(After::Resumed(vec!["Arete".into()])),
                ..Wallpaper::default()
            })),
        ),
        (
            "Wallpapers: the app that paused them stopped",
            panels(&we_view(Wallpaper {
                running: true,
                after: Some(After::Stopped(Some("Arete".into()))),
                ..Wallpaper::default()
            })),
        ),
        (
            "Wallpapers: the wallpaper app quit during a pause",
            panels(&we_view(Wallpaper {
                running: false,
                ..held(&["Arete"])
            })),
        ),
        (
            "Wallpapers paused by three apps",
            panels(&we_view(Wallpaper {
                reasons: vec!["loading a model".into(), "rendering".into(), "x".into()],
                ..held(&["Arete", "Render queue", "Courier"])
            })),
        ),
        (
            "Allow list: connected, off, not on screen",
            panels(&we_view(allow_case("connected"))),
        ),
        (
            "Allow list: needs you, Reconnect",
            panels(&we_view(allow_case("capped"))),
        ),
        (
            "Allow list: after Disconnect all",
            panels(&we_view(allow_case("held"))),
        ),
        (
            "Allow list: connected the old way",
            panels(&we_view(allow_case("old"))),
        ),
        (
            "Allow list: feed off",
            panels(&we_view(allow_case("feed-off"))),
        ),
        (
            "Allow list: nothing asked yet",
            panels(&we_view(allow_case("none"))),
        ),
        (
            "Allow list: connecting and reconnecting",
            panels(&we_view(allow_case("connecting"))),
        ),
        (
            "Allow list: held while Foxglove is paused",
            panels(&we_view(allow_case("hold"))),
        ),
        (
            "Allow list: another app tried to connect",
            panels(&we_view(allow_case("shared"))),
        ),
        (
            "Allow list: another app tried to connect, one wallpaper reconnecting",
            panels(&we_view(allow_case("shared-reloading"))),
        ),
        (
            "Allow list: needs an update",
            panels(&we_view(allow_case("update"))),
        ),
        (
            "Allow list: Wallpaper Engine not running, rows kept",
            panels(&we_view(allow_case("engine-off"))),
        ),
    ];
    for (name, all) in &states {
        assert_valid(all);
        let _ = name;
    }
    foxglove_plugin::ui::check_vectors("foxglove.json", (&m.id, &m.name, &declared, &[]), states);
}

/// The allow list's states for the vectors and the review images (S5b).
fn allow_case(case: &str) -> Wallpaper {
    use super::super::live_pairing::{Link, Links};
    use super::super::we_panel::{Ask, Inputs};
    let ask = |key: &str, title: &str, on_screen, allowed| Ask {
        key: key.into(),
        title: title.into(),
        on_screen,
        allowed,
        needs_update: false,
    };
    let asks = vec![
        ask("d:/we/hud", "Bandit HUD", true, true),
        ask("d:/we/courier", "Courier", false, true),
        ask("d:/we/neon", "Neon Rain (Workshop)", false, false),
    ];
    let mut links = Links::default();
    let mut inputs = Inputs {
        feed_on: true,
        engine_running: true,
        asks,
        ..Inputs::default()
    };
    match case {
        "connected" => {
            links.by_dir.insert("d:/we/hud".into(), Link::Connected);
        }
        "capped" => {
            links.by_dir.insert("d:/we/hud".into(), Link::Capped);
        }
        "held" => links.held = true,
        "old" => {
            links.old_way = 2;
            inputs.asks.iter_mut().for_each(|a| a.allowed = false);
        }
        "feed-off" => inputs.feed_on = false,
        "none" => inputs.asks.clear(),
        "connecting" => {
            inputs.asks[1].on_screen = true;
            links.by_dir.insert("d:/we/hud".into(), Link::Connecting);
            links
                .by_dir
                .insert("d:/we/courier".into(), Link::Reconnecting);
        }
        "hold" => {
            links.by_dir.insert("d:/we/hud".into(), Link::Connected);
            inputs.hold = Some(super::super::we_panel::Hold::Paused);
        }
        "update" => inputs.asks[2].needs_update = true,
        // Both allowed wallpapers are on screen and connected; a refusal came.
        "shared" => {
            inputs.asks[1].on_screen = true;
            links.by_dir.insert("d:/we/hud".into(), Link::Connected);
            links.by_dir.insert("d:/we/courier".into(), Link::Connected);
            links.foreign = true;
        }
        // The flag alone is not enough: one wallpaper is still reconnecting.
        "shared-reloading" => {
            inputs.asks[1].on_screen = true;
            links.by_dir.insert("d:/we/hud".into(), Link::Connected);
            links
                .by_dir
                .insert("d:/we/courier".into(), Link::Reconnecting);
            links.foreign = true;
        }
        "engine-off" => {}
        other => panic!("{other}"),
    }
    inputs.links = links;
    Wallpaper {
        running: case != "engine-off",
        allow: Some(inputs),
        ..Wallpaper::default()
    }
}

/// The data the section's wallpaper app name comes from, in these tests.
const ENGINE: &str = "Wallpaper Engine";

/// The page with a Wallpaper Engine section (#1142).
fn we_view(mut w: Wallpaper) -> View {
    // The wallpaper app's name is data: the fixture gives it.
    w.engine = ENGINE.into();
    view(&connected()).with_wallpaper(Some(w))
}

/// Wallpaper Engine running, held by `names` for "loading a model".
fn held(names: &[&str]) -> Wallpaper {
    Wallpaper {
        running: true,
        holders: names.iter().map(|n| n.to_string()).collect(),
        reasons: vec!["loading a model".into()],
        ..Wallpaper::default()
    }
}

/// The Wallpaper Engine section's texts (value, line, button), or `None` without one.
fn we_texts(v: &View) -> Option<Vec<String>> {
    let all = panels(v);
    let page = &panel(&all, PAGE)["tree"];
    let section = page["children"]
        .as_array()?
        .iter()
        .find(|n| n["id"] == WALLPAPER)?;
    Some(texts(section))
}

/// #1142, UX plan 2: every state's words, Resume only while an app holds it (never over the
/// user's own pause), and no section without the wallpaper app.
#[test]
fn the_wallpaper_section_says_who_holds_it() {
    assert_eq!(
        we_texts(&view(&connected())),
        None,
        "not installed: no section"
    );
    // The intro names the wallpaper app only while its section is there, and from data.
    let intro = |v: &View| panel(&panels(v), PAGE)["tree"]["children"][0]["props"]["text"].clone();
    assert_eq!(intro(&view(&connected())), INTRO);
    assert_eq!(
        intro(&we_view(Wallpaper::default())),
        intro_we(ENGINE).as_str()
    );
    let says = |w: Wallpaper| we_texts(&we_view(w)).unwrap();
    // G5: "Playing" only when a paired wallpaper says so; otherwise the honest line is about apps.
    assert_eq!(
        says(Wallpaper {
            running: true,
            ..Wallpaper::default()
        }),
        [NONE_PAUSING]
    );
    assert_eq!(
        says(Wallpaper {
            running: true,
            playing: true,
            ..Wallpaper::default()
        }),
        [PLAYING]
    );
    assert_eq!(
        says(held(&["Arete"])),
        [
            "Wallpapers paused by Arete",
            "\u{201c}loading a model\u{201d}",
            "Resume wallpapers"
        ]
    );
    assert_eq!(
        says(Wallpaper {
            reasons: vec!["loading a model".into(), "rendering".into()],
            ..held(&["Arete", "Render queue"])
        })[0..2],
        [
            "Wallpapers paused by Arete and Render queue",
            "\u{201c}loading a model\u{201d} \u{b7} \u{201c}rendering\u{201d}"
        ]
    );
    let three = says(Wallpaper {
        reasons: vec!["a".into(), "b".into(), "c".into()],
        ..held(&["A", "B", "C"])
    });
    assert_eq!(three[0], "Wallpapers paused by A and 2 more");
    assert_eq!(
        three[1], "\u{201c}a\u{201d} \u{b7} \u{201c}b\u{201d} \u{b7} \u{2026}",
        "the first two reasons, then more"
    );
    assert_eq!(
        says(Wallpaper {
            full_screen: true,
            ..held(&["Arete"])
        })[..],
        [
            "Wallpapers paused by Arete",
            "They play again when the full-screen app closes.",
            "Resume wallpapers"
        ]
    );
    assert_eq!(
        says(Wallpaper {
            user_paused: true,
            ..held(&["Arete"])
        })[..],
        [
            "Paused by you",
            "Arete is waiting too. It won't press Play for you."
        ]
    );
    assert_eq!(
        says(Wallpaper {
            running: true,
            user_paused: true,
            ..Wallpaper::default()
        })[..],
        ["Paused by you", "Press Play in Wallpaper Engine."]
    );
    assert_eq!(
        says(Wallpaper::default())[..],
        ["Not running", "Wallpaper Engine isn't running."]
    );
    // An app the hub and the local list could not name.
    assert_eq!(
        says(Wallpaper {
            holders: vec![],
            ..held(&[])
        })[0],
        "Wallpapers paused by an app"
    );
    // A long reason is cut; it stays in quotes after the name for the app.
    let long = says(Wallpaper {
        reasons: vec!["x".repeat(200)],
        ..held(&["Arete"])
    });
    assert!(
        long[1].len() < 60 && long[1].ends_with("\u{2026}\u{201d}"),
        "{long:?}"
    );
    // A long name is cut at 32 bytes.
    let name = says(held(&["n".repeat(80).as_str()]));
    assert_eq!(
        name[0],
        format!("Wallpapers paused by {}\u{2026}", "n".repeat(32))
    );
    assert!(is_resume(PAGE, RESUME_NODE) && !is_resume(TILE_ART, RESUME_NODE));
    assert_valid(&panels(&we_view(held(&["Arete"]))));
}

/// UX plan 2-4: the after-lines, the quit line and what a screen reader hears.
#[test]
fn the_after_lines_and_the_quit_line() {
    let says = |w: Wallpaper| we_texts(&we_view(w)).unwrap();
    let after = |a: After| Wallpaper {
        running: true,
        after: Some(a),
        ..Wallpaper::default()
    };
    assert_eq!(
        says(after(After::Resumed(vec![])))[1],
        "Resumed. An app can pause them again.",
        "a sentence starts with a capital"
    );
    assert_eq!(
        says(after(After::Resumed(vec!["Arete".into()]))),
        [NONE_PAUSING, "Resumed. Arete can pause them again."]
    );
    assert_eq!(
        says(after(After::Resumed(vec![
            "Arete".into(),
            "Render queue".into()
        ])))[1],
        "Resumed. Arete and Render queue can pause them again."
    );
    assert_eq!(
        says(Wallpaper {
            full_screen: true,
            ..after(After::Resumed(vec!["Arete".into()]))
        })[1],
        "Resumed. They play again when the full-screen app closes."
    );
    assert_eq!(
        says(after(After::Stopped(Some("Arete".into()))))[..],
        [NONE_PAUSING, "Arete stopped, so its pause ended."]
    );
    assert_eq!(
        says(after(After::Stopped(None)))[1],
        "An app stopped, so its pause ended."
    );
    // The wallpaper app quit during a pause: the section names who it ended for.
    let quit = |holders: &[&str]| {
        says(Wallpaper {
            running: false,
            ..held(holders)
        })
    };
    assert_eq!(
        quit(&["Arete"])[..],
        [
            "Not running",
            "Wallpaper Engine isn't running. Arete's pause ended with it."
        ]
    );
    assert_eq!(
        quit(&[])[1],
        "Wallpaper Engine isn't running. The pause ended with it."
    );
    assert_eq!(
        quit(&["A", "B"])[1],
        "Wallpaper Engine isn't running. Their pauses ended with it."
    );
}

/// The state line is a polite status that says its words; Resume's name carries whose pause it
/// ends, and pressing it sends the focus to the state line.
#[test]
fn the_state_line_is_a_status_and_resume_names_the_holder() {
    let all = panels(&we_view(held(&["Arete", "Render queue"])));
    let page = &panel(&all, PAGE)["tree"];
    let section = page["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == WALLPAPER)
        .unwrap();
    let kids = section["children"].as_array().unwrap();
    let state = kids.iter().find(|n| n["id"] == STATE_NODE).unwrap();
    assert_eq!(state["props"]["live"], true);
    let resume = kids.iter().find(|n| n["id"] == RESUME_NODE).unwrap();
    assert_eq!(
        resume["props"]["name"],
        "Resume wallpapers, paused by Arete and Render queue"
    );
    assert_eq!(resume["props"]["label"], RESUME, "the visible words stay");
    assert_eq!(resume["props"]["focus_after"], STATE_NODE);
    // The after-line is what the status announces.
    let all = panels(&we_view(Wallpaper {
        running: true,
        after: Some(After::Resumed(vec!["Arete".into()])),
        ..Wallpaper::default()
    }));
    let page = &panel(&all, PAGE)["tree"];
    let state = page["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == WALLPAPER)
        .unwrap()["children"][0]
        .clone();
    assert_eq!(
        state["props"]["name"],
        "Resumed. Arete can pause them again."
    );
}

/// Every string the section can draw, over every state, is free of the words a user never sees
/// (UX plan 7), and the wallpaper app's name appears only where the data puts it.
#[test]
fn the_words_have_no_jargon_and_no_hard_coded_engine_name() {
    let never = [
        "lease",
        "holder",
        "caller",
        "client",
        "module",
        "tool",
        "token",
        "pipe",
        "wallpaper.control",
        "wallpaper-engine",
        "paused_by",
        "mcp",
        "program",
        "verified",
    ];
    let names = ["Arete", "Render queue", "an app"];
    let mut all: Vec<Wallpaper> = Vec::new();
    for running in [false, true] {
        for user_paused in [false, true] {
            for full_screen in [false, true] {
                for holders in [&[][..], &names[..1], &names[..2], &names[..]] {
                    for after in [
                        None,
                        Some(After::Resumed(
                            names[..2].iter().map(|n| n.to_string()).collect(),
                        )),
                        Some(After::Stopped(Some("Arete".into()))),
                        Some(After::Stopped(None)),
                    ] {
                        all.push(Wallpaper {
                            running,
                            user_paused,
                            full_screen,
                            playing: !user_paused,
                            holders: holders.iter().map(|n| n.to_string()).collect(),
                            reasons: if holders.is_empty() {
                                vec![]
                            } else {
                                vec!["loading a model".into()]
                            },
                            after,
                            ..Wallpaper::default()
                        });
                    }
                }
            }
        }
    }
    for w in all {
        let v = we_view(w.clone());
        let texts = we_texts(&v).unwrap();
        let tree = serde_json::to_string(&panels(&v)).unwrap();
        for t in &texts {
            // The names and the reason are the fixture's; the words around them are the code's.
            let words = t
                .replace("Arete", "")
                .replace("Render queue", "")
                .replace("loading a model", "")
                .replace("an app", "")
                .replace(ENGINE, "")
                .to_lowercase();
            for bad in never {
                assert!(!words.contains(bad), "{bad:?} in {t:?} for {w:?}");
            }
        }
        // The accessible names too.
        for name in tree.split("\"name\":\"").skip(1) {
            let name = name.split('"').next().unwrap().to_lowercase();
            for bad in never {
                assert!(
                    !name
                        .replace("arete", "")
                        .replace("render queue", "")
                        .contains(bad),
                    "{bad:?} in the name {name:?}"
                );
            }
        }
    }
    // The wallpaper app's name is data: not one literal of it in this file's code, outside
    // comments (the module constant is the data table).
    let code: String = include_str!("../hub_panels.rs")
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .take_while(|l| !l.starts_with("#[cfg(test)]"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("Wallpaper Engine"),
        "the name belongs in data"
    );
    assert!(!code.contains("wallpaper-engine\"") || code.contains("WALLPAPER: &str"));
}

/// A user-chosen effect name never takes a tile past its 64-byte text cap, with the brightness.
#[test]
fn a_long_effect_name_fits_the_lights_tile_at_full_brightness_and_as_a_scene() {
    for effect in ["x".repeat(70), "Ä".repeat(70), "日".repeat(30)] {
        for source in ["keyboard", "scene"] {
            let f = with(
                connected(),
                vec![
                    t("keyboard.lighting.effect", &effect),
                    i("keyboard.lighting.brightness", 100),
                    t("keyboard.lighting.source", source),
                ],
            );
            let all = panels(&view(&f));
            assert_valid(&all);
            let value = &tile_texts(&all, TILE_LIGHTS)[0];
            assert!(value.len() <= 64, "{} bytes: {value}", value.len());
            assert!(value.ends_with(" · 100 %"));
        }
    }
}

/// Game mode (#1234): a row on the page only, after the layer, "on" with its help and "off"
/// without; nothing when the fact isn't reported or the keyboard is away. The Keyboard tile never
/// changes for it and never says GPU, reserved or free (Hub pick: page only).
#[test]
fn game_mode_shows_on_the_page_only() {
    let page_rows = |f: &[(String, Fact)]| {
        let all = panels(&view(f));
        assert_valid(&all);
        let kids = panel(&all, PAGE)["tree"]["children"]
            .as_array()
            .unwrap()
            .clone();
        texts(&kids[1])
    };
    let on = with(connected(), vec![b("keyboard.game_mode", true)]);
    assert_eq!(
        page_rows(&on),
        [
            "Eyelash Corne",
            "Connected over Bluetooth, profile 2",
            "Layer GAME",
            GAME_ON,
            GAME_HELP,
            "Left battery 77 %",
            "Right battery 91 %",
            OPEN
        ]
    );
    let off = with(connected(), vec![b("keyboard.game_mode", false)]);
    let rows = page_rows(&off);
    assert!(rows.contains(&GAME_OFF.to_string()));
    assert!(!rows.contains(&GAME_HELP.to_string()), "help only while on");
    // Not reported: no row (an older app or board).
    let rows = page_rows(&connected());
    assert!(!rows.iter().any(|r| r.starts_with("Game mode")));
    // Away: "Not connected", no game row even with a stale fact.
    let away = vec![
        b("keyboard.connected", false),
        b("keyboard.game_mode", true),
    ];
    assert_eq!(page_rows(&away), [NOT_CONNECTED, OPEN]);
    // The node ids the plan names.
    let all = panels(&view(&on));
    let mut ids = Vec::new();
    let mut stack = vec![&panel(&all, PAGE)["tree"]];
    while let Some(n) = stack.pop() {
        if let Some(id) = n["id"].as_str() {
            ids.push(id.to_string());
        }
        stack.extend(n["children"].as_array().into_iter().flatten());
    }
    assert!(ids.contains(&"keyboard-game".to_string()));
    assert!(ids.contains(&"keyboard-game-help".to_string()));
    // The tile is the same with game mode on, off or unreported, and never names the GPU.
    for f in [&on, &off, &connected()] {
        let all = panels(&view(f));
        let tile = tile_texts(&all, TILE_KEYBOARD);
        assert_eq!(
            tile,
            ["Connected over Bluetooth", "Left 77 % · Right 91 %", OPEN]
        );
        for s in &tile {
            assert!(s.len() <= 64, "{s}");
            let l = s.to_lowercase();
            for w in ["gpu", "reserved", "free", "game"] {
                assert!(!l.contains(w), "tile says {w:?}: {s}");
            }
        }
    }
    // The help names the GPU only to say game mode doesn't touch it, and where Free GPU is.
    assert!(GAME_HELP.contains("doesn't free the GPU"));
    assert!(!GAME_HELP.to_lowercase().contains("reserv"));
}

/// The meters carry their own words, so the page has no second row saying the same.
#[test]
fn the_page_meters_are_named_in_words_and_not_said_twice() {
    let all = panels(&view(&connected()));
    let page = texts(&panel(&all, PAGE)["tree"]);
    for words in [
        "Left battery 77 %",
        "Right battery 91 %",
        "Brightness: 80 %",
    ] {
        assert_eq!(page.iter().filter(|s| *s == words).count(), 1, "{words}");
    }
    let mut meters = 0;
    let mut stack = vec![&panel(&all, PAGE)["tree"]];
    while let Some(n) = stack.pop() {
        if n["type"] == "meter" {
            meters += 1;
            assert!(n["props"]["label"]
                .as_str()
                .is_some_and(|l| l.contains('%')));
        }
        stack.extend(n["children"].as_array().into_iter().flatten());
    }
    assert_eq!(meters, 3);
}

/// S5b: the allow list sits in the Wallpaper Engine section after the state and Resume; with
/// Wallpaper Engine off, its own line is not said twice.
#[test]
fn the_allow_list_is_part_of_the_wallpaper_engine_section() {
    use super::super::we_panel::{Ask, Inputs};
    let inputs = |engine_running| Inputs {
        feed_on: true,
        engine_running,
        asks: vec![Ask {
            key: "d:/we/hud".into(),
            title: "Bandit HUD".into(),
            on_screen: true,
            allowed: false,
            needs_update: false,
        }],
        ..Inputs::default()
    };
    let mut w = held(&[]);
    w.holders.clear();
    w.reasons.clear();
    w.allow = Some(inputs(true));
    let all = panels(&we_view(w));
    let text = serde_json::to_string(&panel(&all, PAGE)["tree"]).unwrap();
    for want in [
        "Wallpapers that want your keyboard",
        "we-allow:0",
        "Let Bandit HUD use your keyboard",
        "Bandit HUD can see which key positions you press and when, never the letters.",
        "No app is pausing your wallpapers",
    ] {
        assert!(text.contains(want), "{want}");
    }
    assert!(!text.contains("d:/we"), "no path on the page");
    let off = Wallpaper {
        allow: Some(inputs(false)),
        ..Wallpaper::default()
    };
    let all = panels(&we_view(off));
    let text = serde_json::to_string(&panel(&all, PAGE)["tree"]).unwrap();
    assert_eq!(text.matches("Wallpaper Engine isn't running.").count(), 1);
}

/// Words with no engine name still start their sentence with a capital, and a real holder name
/// is shown as given (#1142).
#[test]
fn fallback_words_start_a_sentence_and_real_names_stay_as_given() {
    assert_eq!(
        isnt_running("your wallpaper app"),
        "Your wallpaper app isn't running."
    );
    assert_eq!(
        after_words(&After::Resumed(vec![])),
        "Resumed. An app can pause them again."
    );
    assert_eq!(
        after_words(&After::Resumed(vec!["obs".into()])),
        "Resumed. obs can pause them again."
    );
    assert_eq!(
        after_words(&After::Stopped(Some("obs".into()))),
        "obs stopped, so its pause ended."
    );
}

/// A local caller that gave no name is stored as "An app": it reads "an app" mid-sentence and
/// "An app" at the start of one (#1142).
#[test]
fn a_nameless_local_caller_reads_an_app() {
    let who = vec!["An app".to_string()];
    assert_eq!(holder_words(&who), "an app");
    assert_eq!(
        resume_name(&holder_words(&who)),
        "Resume wallpapers, paused by an app"
    );
    assert_eq!(
        after_words(&After::Resumed(who.clone())),
        "Resumed. An app can pause them again."
    );
    assert_eq!(
        after_words(&After::Stopped(Some("An app".into()))),
        "An app stopped, so its pause ended."
    );
}
