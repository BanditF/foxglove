//! The PC sensors page (UX plan slice 3b): the tree for each state, the words and units, and
//! when the hub is sent a register or a patch. No pipe, no window, no hardware.

use std::time::Duration;

use foxglove_sensors::host::{registered_pattern, Host};
use foxglove_sensors::ui::{
    page, tile, tile_panel, GroupState, PageInput, PageOut, PageView, PanelsOut, PanelsView, PANEL,
    TILE,
};
use foxglove_sensors::{Config, Group, Metric, Reading, Source, SourceError, SourceFactory};
use hideout_contract::module::{Context, Module};
use serde_json::{json, Value};

fn all(v: GroupState) -> std::collections::BTreeMap<Group, GroupState> {
    Group::ALL.iter().map(|g| (*g, v)).collect()
}

fn working() -> PageInput {
    let mut i = PageInput {
        states: all(GroupState::Ok),
        game_mode_visible: true,
        ..PageInput::default()
    };
    for (m, v) in [
        (Metric::CpuLoad, 23),
        (Metric::GpuLoad, 87),
        (Metric::RamUsed, 62),
        (Metric::GpuTemp, 64),
        (Metric::GpuClock, 1845),
        (Metric::GpuPower, 185),
        (Metric::GpuFan, 1450),
    ] {
        i.values.insert(m, v);
    }
    i
}

fn reading() -> PageInput {
    PageInput {
        game_mode_visible: true,
        ..PageInput::default()
    }
}

fn game() -> PageInput {
    PageInput {
        game_mode: true,
        ..working()
    }
}

/// A card Windows gives nothing for (not NVIDIA): the rest "Not reported".
fn missing() -> PageInput {
    let mut i = working();
    i.states.insert(Group::Gpu, GroupState::Unavailable);
    for m in [
        Metric::GpuLoad,
        Metric::GpuTemp,
        Metric::GpuClock,
        Metric::GpuPower,
        Metric::GpuFan,
    ] {
        i.values.remove(&m);
    }
    i
}

/// An NVIDIA card that gives no power: only that row is missing, and the card is not blamed.
fn partial() -> PageInput {
    let mut i = working();
    i.values.remove(&Metric::GpuPower);
    i
}

/// A fan that reports its duty only.
fn fan_percent() -> PageInput {
    let mut i = working();
    i.values.remove(&Metric::GpuFan);
    i.values.insert(Metric::GpuFanPct, 45);
    i
}

fn none() -> PageInput {
    PageInput {
        states: all(GroupState::Unavailable),
        game_mode_visible: true,
        ..PageInput::default()
    }
}

fn blind() -> PageInput {
    PageInput {
        game_mode_visible: false,
        ..working()
    }
}

fn every_state() -> Vec<(&'static str, PageInput)> {
    vec![
        ("Reading", reading()),
        ("Working", working()),
        ("Game mode on", game()),
        ("Fan as percent", fan_percent()),
        ("Graphics card details missing", missing()),
        ("Some details missing", partial()),
        ("No sensors", none()),
        ("Game mode not visible", blind()),
    ]
}

fn texts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| texts(x, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| {
            // ids and types are not shown
            if k != "id" && k != "type" && k != "tone" && k != "op" && k != "node" {
                texts(x, out)
            }
        }),
        _ => {}
    }
}

fn shown(v: &Value) -> String {
    let mut out = Vec::new();
    texts(v, &mut out);
    out.join("\n")
}

fn node<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
    if v["id"] == id {
        return Some(v);
    }
    v["children"].as_array()?.iter().find_map(|c| node(c, id))
}

fn value_of(tree: &Value, id: &str) -> String {
    node(tree, id).unwrap_or_else(|| panic!("no {id}"))["props"]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn the_panels_match_the_hub_vectors() {
    let m = Module::parse(
        &std::fs::read_to_string(format!("{}/module.toml", env!("CARGO_MANIFEST_DIR"))).unwrap(),
        &Context::default(),
    )
    .unwrap();
    let declared: Vec<String> = m.panels.iter().map(|p| p.id.clone()).collect();
    let actions: Vec<String> = m.actions.iter().map(|a| a.decl.id.clone()).collect();
    foxglove_plugin::ui::check_vectors(
        "sensors.json",
        (&m.id, &m.name, &declared, &actions),
        every_state()
            .into_iter()
            .map(|(n, i)| (n, vec![tile_panel(&i), foxglove_sensors::ui::panel(&i)]))
            .collect(),
    );
}

#[test]
fn units_and_spacing_are_exact() {
    let t = page(&working());
    assert_eq!(value_of(&t, "now-cpu-value"), "23%");
    assert_eq!(value_of(&t, "now-gpu-value"), "87%");
    assert_eq!(value_of(&t, "now-ram-value"), "62% used");
    assert_eq!(value_of(&t, "gpu-temp-value"), "64 °C");
    assert_eq!(value_of(&t, "gpu-clock-value"), "1845 MHz");
    assert_eq!(value_of(&t, "gpu-power-value"), "185 W");
    assert_eq!(value_of(&t, "gpu-fan-value"), "1450 RPM");
}

#[test]
fn the_rate_line_follows_game_mode() {
    let t = page(&working());
    assert_eq!(
        node(&t, "now-rate").unwrap()["props"]["text"],
        "Updates once a second while this page is open."
    );
    let t = page(&game());
    let n = node(&t, "now-rate").unwrap();
    assert_eq!(
        n["props"]["text"],
        "Game mode is on, so this updates every 10 seconds."
    );
    assert_eq!(n["props"]["tone"], "off");
}

/// The fan's duty is "% of full speed", never RPM; RPM wins when both are there.
#[test]
fn a_fan_percent_never_carries_rpm() {
    let mut i = working();
    i.values.remove(&Metric::GpuFan);
    i.values.insert(Metric::GpuFanPct, 45);
    let t = page(&i);
    assert_eq!(value_of(&t, "gpu-fan-value"), "45% of full speed");
    assert!(!shown(node(&t, "gpu-fan").unwrap()).contains("RPM"));
    i.values.insert(Metric::GpuFan, 1450);
    assert_eq!(value_of(&page(&i), "gpu-fan-value"), "1450 RPM");
}

#[test]
fn an_absent_value_is_not_reported_and_never_zero() {
    let t = page(&missing());
    for id in [
        "gpu-temp-value",
        "gpu-clock-value",
        "gpu-power-value",
        "gpu-fan-value",
    ] {
        assert_eq!(value_of(&t, id), "Not reported", "{id}");
    }
    assert!(node(&t, "gpu-note").is_some());
    let text = shown(node(&t, "gpu").unwrap());
    assert!(!text.contains(" 0 ") && !text.contains("0 °C") && !text.contains("0 MHz"));
    assert!(!text.contains("0 W") && !text.contains("0 RPM"));
    // A value that is really 0 is shown as it is; only an absent one is "Not reported".
    let mut i = working();
    i.values.insert(Metric::GpuFanPct, 0);
    i.values.remove(&Metric::GpuFan);
    assert_eq!(value_of(&page(&i), "gpu-fan-value"), "0% of full speed");
    // Reading, then no row says a number.
    let t = page(&reading());
    assert_eq!(value_of(&t, "now-cpu-value"), "Reading…");
    assert_eq!(value_of(&t, "gpu-temp-value"), "Reading…");
    assert!(node(&t, "gpu-note").is_none());
}

#[test]
fn no_sensors_offers_try_again() {
    let t = page(&none());
    let n = node(&t, "now-none").unwrap();
    assert_eq!(
        n["props"]["text"],
        "Windows isn't giving any readings right now."
    );
    assert_eq!(n["props"]["tone"], "attention");
    assert_eq!(node(&t, "retry").unwrap()["props"]["label"], "Try again");
    assert!(node(&t, "now-cpu").is_none());
}

#[test]
fn a_module_blind_to_games_says_so_in_about() {
    let t = page(&blind());
    let about = node(&t, "about-text").unwrap()["props"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(about.contains("PC sensors can't see when you're gaming"));
    assert!(about.contains("see whether game mode is on"));
    let t = page(&working());
    assert!(!node(&t, "about-text").unwrap()["props"]["text"]
        .as_str()
        .unwrap()
        .contains("can't see when you're gaming"));
}

/// The words a person never sees: keys, jargon, the group ids, the error ids.
#[test]
fn no_internal_word_is_ever_shown() {
    for (name, i) in every_state() {
        let text = shown(&page(&i));
        for word in [
            "sensor.",
            "facts",
            "subscribe",
            "demand",
            "module",
            "plugin",
            "NVML",
            "PDH",
            "WMI",
            "AppContainer",
            "unavailable",
            "unknown",
            "GPU",
            "CPU",
            "RAM",
            "load_pct",
            "neutral",
            "attention",
            "org.foxglove",
            "ui.",
        ] {
            assert!(!text.contains(word), "{name}: {word:?} shows in\n{text}");
        }
    }
}

#[test]
fn every_value_has_an_accessible_name() {
    let t = page(&working());
    assert_eq!(
        node(&t, "now-cpu-value").unwrap()["props"]["name"],
        "Processor, 23 percent busy"
    );
    assert_eq!(
        node(&t, "gpu-temp-value").unwrap()["props"]["name"],
        "Graphics card temperature, 64 degrees Celsius"
    );
    assert_eq!(
        node(&t, "now-cpu-meter").unwrap()["props"]["name"],
        "Processor, 23 percent busy"
    );
}

// The patches.

fn registered(view: &mut PageView, i: &PageInput) -> Value {
    match view.next(page(i)) {
        Some(PageOut::Register(p)) => p,
        other => panic!("expected a register, got {other:?}"),
    }
}

#[test]
fn the_first_tree_registers_and_the_same_tree_sends_nothing() {
    let mut v = PageView::default();
    let p = registered(&mut v, &reading());
    assert_eq!(p["id"], PANEL);
    assert_eq!(p["slot"], "page");
    assert_eq!(p["title"], "PC sensors");
    assert!(v.next(page(&reading())).is_none());
}

#[test]
fn one_patch_per_changed_node() {
    let mut v = PageView::default();
    registered(&mut v, &working());
    let mut i = working();
    i.values.insert(Metric::CpuLoad, 24);
    let Some(PageOut::Patch(ops)) = v.next(page(&i)) else {
        panic!("a patch")
    };
    // The value, its accessible name, and the meter (value and label) are all one row's nodes;
    // nothing outside the Processor row changes.
    assert!(!ops.is_empty());
    for op in &ops {
        let id = op["node"].as_str().unwrap();
        assert!(id.starts_with("now-cpu"), "{id}");
    }
    let ids: std::collections::BTreeSet<_> =
        ops.iter().map(|o| o["node"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), ops.len(), "one op per node: {ops:?}");
}

#[test]
fn a_new_shape_registers_again() {
    let mut v = PageView::default();
    registered(&mut v, &working());
    registered(&mut v, &none());
    // And back, with the details-missing line appearing.
    registered(&mut v, &working());
    registered(&mut v, &missing());
}

// The host: no patch while not listening, and a release leaves no number.

struct Fixed;
impl Source for Fixed {
    fn read(&mut self) -> Result<Reading, SourceError> {
        Ok(Reading::new()
            .with(Metric::CpuLoad, 23)
            .with(Metric::GpuLoad, 87)
            .with(Metric::RamUsed, 62)
            .with(Metric::GpuTemp, 64)
            .with(Metric::GpuClock, 1845)
            .with(Metric::GpuPower, 185)
            .with(Metric::GpuFan, 1450))
    }
}

fn host() -> Host {
    let f: SourceFactory = Box::new(|| Box::new(Fixed) as Box<dyn Source>);
    Host::new(vec![f], Config::default())
}

fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn digits(v: &Value) -> bool {
    shown(v).chars().any(|c| c.is_ascii_digit())
}

fn visible(h: &mut Host, state: &str, now: Duration) {
    h.notify(
        "ui.visibility",
        &json!({"panel": PANEL, "state": state}),
        now,
    );
}

#[test]
fn no_patch_while_not_listening() {
    let mut h = host();
    let mut v = PageView::default();
    registered_first(&mut v, &h);
    // Game mode flips, time passes, other apps' facts change: the page is not on screen.
    h.game_mode(true, s(1));
    h.tick(s(2));
    h.take_writes();
    assert!(v.next(h.page_tree()).is_none());
    h.game_mode(false, s(3));
    assert!(v.next(h.page_tree()).is_none());
}

fn registered_first(v: &mut PageView, h: &Host) {
    assert!(matches!(v.next(h.page_tree()), Some(PageOut::Register(_))));
}

#[test]
fn a_release_patches_back_to_reading_and_leaves_no_number() {
    let mut h = host();
    let mut v = PageView::default();
    registered_first(&mut v, &h);
    visible(&mut h, "visible", s(0));
    h.tick(s(0));
    h.take_writes();
    // Now the numbers are in.
    let out = v.next(h.page_tree());
    assert!(out.is_some());
    assert_eq!(value_of(&h.page_tree(), "now-cpu-value"), "23%");
    // Hidden: sampling stops, the withdrawals go out, the page is all "Reading…".
    visible(&mut h, "hidden", s(5));
    assert!(!h.sampling());
    h.take_writes();
    let tree = h.page_tree();
    assert_eq!(value_of(&tree, "now-cpu-value"), "Reading…");
    assert_eq!(value_of(&tree, "gpu-temp-value"), "Reading…");
    // The patch carries no number.
    let Some(PageOut::Patch(ops)) = v.next(tree) else {
        panic!("a release patch")
    };
    assert!(!ops.is_empty() && !digits(&Value::Array(ops)));
    // And then nothing more while hidden.
    assert!(v.next(h.page_tree()).is_none());
    h.tick(s(60));
    assert!(v.next(h.page_tree()).is_none());
}

#[test]
fn the_page_reads_again_when_it_returns() {
    let mut h = host();
    visible(&mut h, "visible", s(0));
    h.tick(s(0));
    h.take_writes();
    visible(&mut h, "hidden", s(1));
    h.take_writes();
    visible(&mut h, "visible", s(2));
    // The stale numbers are gone: Reading… until the first read.
    assert_eq!(value_of(&h.page_tree(), "now-cpu-value"), "Reading…");
    h.tick(s(2));
    h.take_writes();
    assert_eq!(value_of(&h.page_tree(), "now-cpu-value"), "23%");
}

/// Another app's demand alone keeps sampling, but the page (not on screen) still says "Reading…".
#[test]
fn an_unseen_page_never_holds_numbers() {
    let mut h = host();
    h.demand(&registered_pattern(), 1, s(0));
    h.tick(s(0));
    h.take_writes();
    assert!(h.sampling());
    assert_eq!(value_of(&h.page_tree(), "now-cpu-value"), "Reading…");
}

#[test]
fn retry_restarts_the_sources_once() {
    let mut h = host();
    visible(&mut h, "visible", s(0));
    h.tick(s(0));
    h.take_writes();
    h.retry(s(3));
    assert!(h.sampling());
    h.tick(s(3));
    assert!(!h.take_writes().is_empty());
}

#[test]
fn the_manifest_declares_the_page() {
    let m = Module::parse(
        &std::fs::read_to_string(format!("{}/module.toml", env!("CARGO_MANIFEST_DIR"))).unwrap(),
        &Context::default(),
    )
    .unwrap();
    assert_eq!(m.panels.len(), 2);
    assert_eq!(m.panels[0].id, TILE);
    assert_eq!(m.panels[1].id, PANEL);
    for p in &m.panels {
        assert_eq!(p.title, "PC sensors");
        assert_eq!(p.icon, "chart");
    }
}

/// The NVIDIA sentence only when the whole card is unavailable (and something else reads);
/// some values missing blames nothing; no readings at all adds no note.
#[test]
fn the_nvidia_sentence_is_only_for_a_card_that_gives_nothing() {
    let note = |i: &PageInput| {
        node(&page(i), "gpu-note").map(|n| n["props"]["text"].as_str().unwrap().to_string())
    };
    assert_eq!(
        note(&missing()).as_deref(),
        Some("This graphics card doesn't report these to Windows. Today only NVIDIA cards do.")
    );
    assert_eq!(
        note(&partial()).as_deref(),
        Some("This graphics card doesn't report these to Windows.")
    );
    assert_eq!(note(&none()), None);
    assert_eq!(note(&working()), None);
}

// The tray tile (slice 3c).

fn tile_visibility(h: &mut Host, state: &str, now: Duration) {
    h.notify(
        "ui.visibility",
        &json!({"panel": TILE, "state": state}),
        now,
    );
}

fn depth(v: &Value) -> usize {
    1 + v["children"]
        .as_array()
        .map(|c| c.iter().map(depth).max().unwrap_or(0))
        .unwrap_or(0)
}

fn count(v: &Value) -> usize {
    1 + v["children"]
        .as_array()
        .map(|c| c.iter().map(count).sum())
        .unwrap_or(0)
}

fn kinds(v: &Value, kind: &str) -> usize {
    usize::from(v["type"] == kind)
        + v["children"]
            .as_array()
            .map(|c| c.iter().map(|x| kinds(x, kind)).sum())
            .unwrap_or(0)
}

#[test]
fn the_tile_shows_the_busiest_and_names_the_others() {
    let t = tile(&working());
    assert_eq!(value_of(&t, "glance"), "Graphics card 87%");
    assert_eq!(
        node(&t, "sub").unwrap()["props"]["text"],
        "Processor 23% · Memory 62%"
    );
    assert_eq!(node(&t, "glance").unwrap()["props"]["tone"], "neutral");
    let meter = node(&t, "meter").unwrap();
    assert_eq!(meter["props"]["value"], 87.0);
    assert_eq!(meter["props"]["name"], "Graphics card, 87 percent busy");
    // Memory in front when it is the busiest.
    let mut i = working();
    i.values.insert(Metric::RamUsed, 95);
    assert_eq!(value_of(&tile(&i), "glance"), "Memory 95%");
}

#[test]
fn the_tile_never_draws_a_missing_value_as_zero() {
    let r = tile(&reading());
    assert_eq!(value_of(&r, "glance"), "Reading…");
    assert!(!digits(&r));
    assert_eq!(
        node(&r, "sub").unwrap()["props"]["text"],
        "Processor · Graphics card · Memory"
    );
    let n = tile(&none());
    assert_eq!(value_of(&n, "glance"), "Not reported");
    assert!(!digits(&n));
    // A card Windows gives nothing for: the others are named, the card says so.
    let m = tile(&missing());
    assert_eq!(value_of(&m, "glance"), "Memory 62%");
    assert_eq!(
        node(&m, "sub").unwrap()["props"]["text"],
        "Processor 23% · Graphics card not reported"
    );
}

#[test]
fn the_tile_stays_inside_the_tile_caps() {
    for (name, i) in every_state() {
        let t = tile(&i);
        assert!(count(&t) <= 12, "{name}: nodes");
        assert!(depth(&t) <= 3, "{name}: depth");
        assert_eq!(kinds(&t, "value"), 1, "{name}: one value");
        for k in ["label", "meter", "button"] {
            assert!(kinds(&t, k) <= 1, "{name}: at most one {k}");
        }
        assert_eq!(kinds(&t, "button") + kinds(&t, "stream"), 0, "{name}");
        let mut out = Vec::new();
        texts(&t, &mut out);
        assert!(out.iter().all(|s| s.len() <= 64), "{name}: {out:?}");
    }
}

#[test]
fn a_visible_tile_starts_sampling_and_hidden_releases() {
    let mut h = host();
    tile_visibility(&mut h, "visible", s(0));
    assert!(h.sampling());
    h.tick(s(0));
    h.take_writes();
    assert_eq!(value_of(&h.tile_tree(), "glance"), "Graphics card 87%");
    // The page as well: hiding the tile alone keeps the page's sampling.
    visible(&mut h, "visible", s(1));
    tile_visibility(&mut h, "hidden", s(2));
    assert!(h.sampling());
    visible(&mut h, "hidden", s(3));
    assert!(!h.sampling());
    h.take_writes();
    assert_eq!(value_of(&h.tile_tree(), "glance"), "Reading…");
    // Hidden then closed: closed releases too.
    tile_visibility(&mut h, "visible", s(4));
    assert!(h.sampling());
    tile_visibility(&mut h, "closed", s(5));
    assert!(!h.sampling());
    assert!(h.next_due().is_none());
}

#[test]
fn an_unseen_tile_never_holds_numbers() {
    let mut h = host();
    h.demand(&registered_pattern(), 1, s(0));
    h.tick(s(0));
    h.take_writes();
    assert_eq!(value_of(&h.tile_tree(), "glance"), "Reading…");
}

#[test]
fn both_panels_register_together_and_then_patch_apart() {
    let mut h = host();
    let mut v = PanelsView::new();
    let out = v.next(h.tile_tree(), h.page_tree());
    let [PanelsOut::Register(panels)] = &out[..] else {
        panic!("one register, got {out:?}")
    };
    assert_eq!(panels.len(), 2);
    assert_eq!(
        (panels[0]["id"].as_str(), panels[0]["slot"].as_str()),
        (Some("tile"), Some("tile"))
    );
    assert_eq!(
        (panels[1]["id"].as_str(), panels[1]["slot"].as_str()),
        (Some("page"), Some("page"))
    );
    assert!(v.next(h.tile_tree(), h.page_tree()).is_empty());
    // Only the tile is on screen: only the tile patches.
    tile_visibility(&mut h, "visible", s(0));
    h.tick(s(0));
    h.take_writes();
    let out = v.next(h.tile_tree(), h.page_tree());
    assert!(
        matches!(&out[..], [PanelsOut::Patch { panel: "tile", .. }]),
        "{out:?}"
    );
    // The release patch leaves no number in the tile.
    tile_visibility(&mut h, "hidden", s(5));
    h.take_writes();
    let out = v.next(h.tile_tree(), h.page_tree());
    let [PanelsOut::Patch { panel: "tile", ops }] = &out[..] else {
        panic!("a tile release patch, got {out:?}")
    };
    assert!(!digits(&Value::Array(ops.clone())));
}

#[test]
fn a_shape_change_registers_both_panels_again() {
    let mut v = PanelsView::new();
    v.next(tile(&working()), page(&working()));
    // The page changes shape (no readings at all): both panels go out again.
    let out = v.next(tile(&working()), page(&none()));
    assert!(
        matches!(&out[..], [PanelsOut::Register(p)] if p.len() == 2),
        "{out:?}"
    );
}
