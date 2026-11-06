//! Other devices' keyboards with fake keyboards (#820): no device, no window.

use super::*;
use crate::foxglove::keyboard_focus::Source;
use crate::foxglove::pc_lights_ui::tests::{reachable, sample, shown, test_ctx};
use crate::foxglove::pc_lights_ui::{draw, row_key, Draft, Intent, View};

const CORNE: &str = "zmk-1d50:615e-p31";
const K5: &str = "via-k5";
const LAMP: &str = "045e:0b13abc";

fn keyboard(key: &str, name: &str, source: Source, (vid, pid): (u16, u16)) -> Keyboard {
    Keyboard {
        key: key.into(),
        name: name.into(),
        source,
        vid,
        pid,
        device: None,
        connected: true,
    }
}

fn corne() -> Keyboard {
    keyboard(CORNE, "Eyelash Corne", Source::HostLink, (0x1d50, 0x615e))
}

fn k5() -> Keyboard {
    keyboard(K5, "K5 Pro", Source::Via, (0x3434, 0x0250))
}

fn lamp() -> Keyboard {
    keyboard(LAMP, "Lamp keyboard", Source::LampArray, (0x045e, 0x0b13))
}

/// The user's desk (the case, the cooler, the mouse) with a Dynamic Lighting keyboard's row,
/// and the Corne, the K5 Pro and that keyboard known to the focus, `focus` focused.
fn desk(focus: &str) -> View {
    let mut v = sample();
    v.pc.devices.push(crate::foxglove::fixtures::lamp_array_row(
        LAMP,
        "Lamp keyboard",
    ));
    v.keyboards = Keyboards {
        all: vec![corne(), k5(), lamp()],
        focus: Some(focus.into()),
        host_link: Some("USB".into()),
        layouts: BTreeMap::new(),
    };
    v
}

fn keys(list: Vec<&Keyboard>) -> Vec<&str> {
    list.into_iter().map(|k| k.key.as_str()).collect()
}

/// #820 AC: every keyboard that isn't the focus has "Show in Foxglove": the K5 Pro on a row of
/// its own ("VIA · USB · read-only"), the Dynamic Lighting keyboard on its lights' row. The
/// focused one isn't listed: it has the page's keyboard card.
#[test]
fn keyboards_that_are_not_the_focus_get_the_action() {
    let v = desk(CORNE);
    assert_eq!(keys(v.keyboards.own_rows(&v.pc.devices)), [K5]);
    let lamp_row = v.pc.devices.last().unwrap();
    assert_eq!(
        v.keyboards.show_on(lamp_row).map(|k| k.key.as_str()),
        Some(LAMP)
    );
    assert!(v.pc.devices.iter().all(|r| v.keyboards.listed(r)));
    let text = shown(&v);
    assert_eq!(text.matches(focus_ui::SHOW).count(), 2, "{text}");
    for want in ["K5 Pro", "VIA · USB · read-only", "Lamp keyboard"] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert!(
        !text.contains("Eyelash Corne"),
        "the focus isn't listed: {text}"
    );
    // The hover is the switcher rows' own words.
    let reach = reachable(&v);
    for want in ["Show K5 Pro in Foxglove", "Show Lamp keyboard in Foxglove"] {
        assert!(reach.contains(want), "{want}: {reach}");
    }
    // The other devices' rows carry no link.
    let mut alone = v.clone();
    alone.pc.devices.pop();
    alone.keyboards.all.truncate(1);
    assert!(!shown(&alone).contains(focus_ui::SHOW));
    assert!(sample().keyboards.own_rows(&sample().pc.devices).is_empty());
}

/// #820 AC: a focused LampArray-only keyboard isn't listed twice, and its colours stay
/// reachable (the keyboard card mutes them): its lights row stays, once, with a flat muted
/// "shown in Foxglove" chip in place of the link. The Corne and the K5 Pro get rows of their
/// own with the link.
#[test]
fn a_focused_lamp_array_keyboard_keeps_its_lights_row_with_a_chip() {
    let v = desk(LAMP);
    let lamp_row = v.pc.devices.last().unwrap();
    assert!(v.keyboards.listed(lamp_row) && v.keyboards.shown_here(lamp_row));
    assert_eq!(v.keyboards.show_on(lamp_row), None);
    assert_eq!(v.keyboards.row_focus(lamp_row), RowFocus::Shown);
    assert_eq!(keys(v.keyboards.own_rows(&v.pc.devices)), [CORNE, K5]);
    let text = shown(&v);
    assert_eq!(text.matches("Lamp keyboard").count(), 1, "{text}");
    assert_eq!(text.matches(SHOWN).count(), 1, "{text}");
    for want in ["Eyelash Corne", "ZMK · USB · Foxglove firmware", "K5 Pro"] {
        assert!(text.contains(want), "{want}: {text}");
    }
    assert_eq!(
        text.matches(focus_ui::SHOW).count(),
        2,
        "the Corne's and the K5 Pro's links, none on the focus: {text}"
    );
    // The chip says where another keyboard is picked.
    let open = crate::foxglove::pc_lights_ui::tests::reachable(&v);
    assert!(
        open.contains(focus_ui::TITLE_HOVER),
        "the chip's hover: {open}"
    );
    // Not focused: the link, no chip.
    assert!(!shown(&desk(CORNE)).contains(SHOWN));
    // The other devices stay.
    for want in ["HP OMEN 45L case lights", "G502 Proteus Spectrum"] {
        assert!(text.contains(want), "{want}: {text}");
    }
}

/// A Dynamic Lighting row with a VIA keyboard's VID/PID is that keyboard (C1 lists it once): it
/// carries the K5 Pro's link instead of a second K5 Pro row, and goes while the K5 Pro is shown.
/// A keyboard that went this run isn't offered.
#[test]
fn a_keyboards_own_lights_row_is_that_keyboard() {
    let mut v = desk(CORNE);
    v.pc.devices.push(crate::foxglove::fixtures::lamp_array_row(
        "3434:0250",
        "Keychron K5 Pro lights",
    ));
    let k5_row = v.pc.devices.last().unwrap();
    assert_eq!(
        v.keyboards.show_on(k5_row).map(|k| k.key.as_str()),
        Some(K5)
    );
    assert!(v.keyboards.own_rows(&v.pc.devices).is_empty());
    let text = shown(&v);
    assert!(!text.contains("VIA · USB"), "no second K5 Pro row: {text}");
    assert_eq!(text.matches(focus_ui::SHOW).count(), 2, "{text}");
    v.keyboards.focus = Some(K5.into());
    let text = shown(&v);
    // A focused VIA keyboard's lights row stays out (only a LampArray-only one stays, #820).
    assert!(!text.contains("Keychron K5 Pro lights"), "{text}");
    assert!(!text.contains(SHOWN), "{text}");
    assert!(text.contains("Eyelash Corne"), "{text}");
    // Gone: neither a row of its own nor the link on its lights' row.
    let mut gone = desk(CORNE);
    gone.keyboards.all[1].connected = false;
    gone.keyboards.all[2].connected = false;
    assert!(gone.keyboards.own_rows(&gone.pc.devices).is_empty());
    assert!(!shown(&gone).contains(focus_ui::SHOW));
}

/// One frame of the card at a window's size with `events`: what it asked for.
fn frame(ctx: &egui::Context, v: &View, events: Vec<egui::Event>) -> Vec<Intent> {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 1400.0),
        )),
        events,
        ..Default::default()
    };
    LAST_LINKS.with_borrow_mut(Vec::clear);
    let mut asked = Vec::new();
    let mut out = ctx.run_ui(raw, |ui| {
        asked = draw(ui, v, &mut Draft::default(), &mut Default::default());
    });
    out.textures_delta.clear();
    asked
}

/// A click on `key`'s link: what the card asked for.
fn click_link(ctx: &egui::Context, v: &View, key: &str) -> Vec<Intent> {
    // Twice: a thumbnail's room is settled on the second pass.
    frame(ctx, v, vec![]);
    frame(ctx, v, vec![]);
    let at = LAST_LINKS
        .with_borrow(|l| l.iter().find(|(k, _)| k == key).map(|(_, r)| r.center()))
        .unwrap_or_else(|| panic!("no link for {key}"));
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut asked = frame(ctx, v, vec![egui::Event::PointerMoved(at), button(true)]);
    asked.extend(frame(ctx, v, vec![button(false)]));
    asked
}

/// #820 AC: the action switches the focus, and the pages follow. A click on the K5 Pro's link
/// asks for it, and the card's handler (the switcher's own pick) makes it the focus: the
/// Keyboard and Lights pages name it, and Other devices now offers the Corne instead.
#[test]
fn the_action_switches_the_focus_and_the_pages_follow() {
    kf::TEST_SECTION.set(Some("focus-test-show-820"));
    crate::foxglove::store::test_config_dir();
    focus_ui::tests::two_keyboards();
    assert_eq!(kf::current().map(|k| k.key), Some(CORNE.into()));
    let ctx = test_ctx();
    let v = View {
        keyboards: Keyboards::now(&[]),
        ..View::default()
    };
    assert_eq!(keys(v.keyboards.own_rows(&[])), [K5]);
    assert!(v.keyboards.layouts.contains_key(K5), "its thumbnail's keys");
    let asked = click_link(&ctx, &v, K5);
    assert_eq!(asked, [Intent::Show(K5.into())]);
    // What `pc_lights_ui::show` does with it.
    focus_ui::pick(&ctx, K5);
    assert_eq!(kf::current().map(|k| k.key), Some(K5.into()));
    assert_eq!(
        focus_ui::Focused::now().map(|f| f.name),
        Some("K5 Pro".into())
    );
    let lights = crate::foxglove::lights_page::live_view(None);
    assert_eq!(lights.focus.map(|f| f.name), Some("K5 Pro".into()));
    let v = View {
        keyboards: Keyboards::now(&[]),
        ..View::default()
    };
    assert_eq!(keys(v.keyboards.own_rows(&[])), [CORNE]);
    let text = shown(&v);
    assert!(
        text.contains("Eyelash Corne") && !text.contains("K5 Pro"),
        "{text}"
    );
}

/// The link on a device row (the Dynamic Lighting keyboard's) asks for the focus and doesn't
/// open or close the row under it.
#[test]
fn the_link_on_a_device_row_does_not_toggle_it() {
    let ctx = test_ctx();
    let v = desk(CORNE);
    let asked = click_link(&ctx, &v, LAMP);
    assert_eq!(asked, [Intent::Show(LAMP.into())]);
    assert!(!crate::foxglove::settings_ui::expander_open(
        &ctx,
        &row_key(LAMP)
    ));
}

/// Other devices' keyboards read caches only: no device call, no wait, no thread.
#[test]
fn other_devices_keyboards_ask_no_device() {
    let src = include_str!("other_keyboards.rs");
    for banned in [
        "via_host",
        "pc_lights::send",
        "keyboard_settings",
        "boards::find",
        "spawn",
        "sleep(",
        "recv",
    ] {
        assert!(!src.contains(banned), "other_keyboards.rs uses {banned}");
    }
}
