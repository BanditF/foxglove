//! #825: the keyboard card drives a focused LampArray keyboard's lights, with fake LampArray
//! rows: no device, no window, no worker.

use super::*;
use crate::foxglove::keyboard_focus::Source;
use crate::foxglove::pc_lights::{Msg, Row};
use crate::foxglove::pc_lights_ui::{self as pcu, LAST_CARD_BUTTONS};
use foxglove_devices::pc_lighting::{LampAccess, LampArrayInfo, LightCaps};

const LAMP: &str = "2f1e:0c01abc";
const NAME: &str = "Lumen TKL keyboard";
const ORANGE: [u8; 3] = [0xf0, 0x8a, 0x3c];
const LAMPS: u32 = 30;

/// The keyboard's lights as the Lights worker lists them: 30 lamps through Windows, last set
/// orange, with Windows' `access` (`None`: listed, not opened).
fn lamp_row(access: Option<LampAccess>) -> Row {
    let mut row = crate::foxglove::fixtures::lamp_array_row(LAMP, NAME);
    row.caps = LightCaps::from_lamp_array(LampArrayInfo {
        lamp_count: LAMPS,
        positions: true,
        min_update_us: 16_000,
        kind: 1,
    });
    row.zones = (1..=LAMPS)
        .map(|i| (format!("Lamp {i}"), Some(ORANGE)))
        .collect();
    row.zone_ids = (0..LAMPS).map(|i| i.to_string()).collect();
    row.lamp_access = access;
    row
}

fn on() -> Option<LampAccess> {
    Some(LampAccess {
        enabled: true,
        available: true,
    })
}

/// The card with the LampArray keyboard in focus.
fn card(access: Option<LampAccess>, identity: bool) -> View {
    let focus = crate::foxglove::focus_ui::Focused {
        name: NAME.into(),
        read_only: lights_only_hover(Source::LampArray),
    };
    lamp_view(
        focus,
        Lamp {
            row: lamp_row(access),
            busy: false,
            status: None,
            layout: crate::foxglove::key_layout::resolve(None, &[], LAMPS as usize),
            identity,
        },
    )
}

/// One frame of the card at a window's size with `events`: what it asked for, and its shapes.
fn frame(
    ctx: &egui::Context,
    v: &View,
    draft: &mut Draft,
    events: Vec<egui::Event>,
) -> (Vec<Intent>, Vec<egui::epaint::ClippedShape>) {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 1200.0),
        )),
        events,
        ..Default::default()
    };
    LAST_CARD_BUTTONS.with_borrow_mut(Vec::clear);
    let mut asked = Vec::new();
    let mut out = ctx.run_ui(raw, |ui| {
        asked = draw(ui, v, draft);
    });
    out.textures_delta.clear();
    (asked, out.shapes)
}

fn text_of(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn collect(shape: &egui::Shape, out: &mut String) {
        match shape {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let mut text = String::new();
    for c in shapes {
        collect(&c.shape, &mut text);
    }
    text
}

/// The card's text as the casual viewer sees it, and with everything one hover away.
fn texts(v: &View) -> (String, String) {
    let ctx = pcu::tests::test_ctx();
    let mut draft = Draft::default();
    let mut shown = String::new();
    for _ in 0..3 {
        shown = text_of(&frame(&ctx, v, &mut draft, vec![]).1);
    }
    ctx.memory_mut(|m| m.set_everything_is_visible(true));
    let mut all = String::new();
    for _ in 0..3 {
        all = text_of(&frame(&ctx, v, &mut draft, vec![]).1);
    }
    (shown, all)
}

/// A click on the card's `label` button: what the card asked for.
fn click(v: &View, draft: &mut Draft, label: &str) -> Vec<Intent> {
    let ctx = pcu::tests::test_ctx();
    frame(&ctx, v, draft, vec![]);
    frame(&ctx, v, draft, vec![]);
    let at = LAST_CARD_BUTTONS
        .with_borrow(|b| b.iter().find(|(l, _)| *l == label).map(|(_, r)| r.center()))
        .unwrap_or_else(|| panic!("no {label} button"));
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut asked = frame(
        &ctx,
        v,
        draft,
        vec![egui::Event::PointerMoved(at), button(true)],
    )
    .0;
    asked.extend(frame(&ctx, v, draft, vec![button(false)]).0);
    asked
}

/// The worker requests a card's intents are, as `pc_lights_ui::act` sends them.
fn msgs(asked: &[Intent]) -> Vec<Msg> {
    asked
        .iter()
        .filter_map(|i| match i {
            Intent::Lamp(i) => pcu::row_msg(i),
            _ => None,
        })
        .collect()
}

/// #825 AC: with a LampArray keyboard in focus the card has working lights: EFFECT with the
/// LampArray class's one effect, the colour for all lamps, Apply colours, Off, Identify and
/// Calibrate…; its chips say "lights only" (why the rest is muted) and what Windows allows. The
/// underglow's own words are gone, and nothing reads "read-only".
#[test]
fn the_card_has_working_lights_for_a_lamp_array_keyboard() {
    let v = card(on(), false);
    let (shown, all) = texts(&v);
    for want in [
        NAME,
        LIGHTS_ONLY,
        "in front only",
        "30 lamps",
        "Show zones",
        "EFFECT",
        "Colour",
        "ALL LAMPS",
        "Apply colours",
        "Off",
        "Identify",
        "Calibrate…",
    ] {
        assert!(shown.contains(want), "{want}: {shown}");
    }
    for gone in [
        NOT_READ,
        "Solid",
        "Breathe",
        crate::foxglove::focus_ui::READ_ONLY,
    ] {
        assert!(!shown.contains(gone), "{gone}: {shown}");
    }
    // The chips' hovers: why the rest is muted, and the row's Windows sentence word for word.
    let why = lights_only_hover(Source::LampArray).unwrap();
    for want in [
        why.as_str(),
        "Windows shows Foxglove's colours while Foxglove's window is in front; in the \
         background its own Dynamic Lighting takes over.",
        pcu::OFF_TIP,
        pcu::IDENTIFY_TIP,
    ] {
        assert!(all.contains(want), "{want}: {all}");
    }
    // What it can't have stays on the card, muted: never a blank card.
    assert!(shown.contains("Refresh") && shown.contains("Follow the wallpaper"));
}

/// #825 AC: the card's Off, Identify and Apply colours send the same worker requests for that
/// keyboard's key as its Other devices row did (`row_msg` of the row's own intents).
#[test]
fn the_card_sends_the_rows_requests_for_its_key() {
    let v = card(on(), false);
    let mut draft = Draft::default();
    let off = click(&v, &mut draft, "Off");
    assert_eq!(off, [Intent::Lamp(pcu::Intent::Off(LAMP.into()))]);
    assert_eq!(
        msgs(&off),
        [pcu::row_msg(&pcu::Intent::Off(LAMP.into())).unwrap()]
    );
    assert_eq!(msgs(&off), [Msg::Off(LAMP.into())]);
    let identify = click(&v, &mut draft, "Identify");
    assert_eq!(msgs(&identify), [Msg::Identify(LAMP.into())]);
    // Nothing picked: Apply colours waits, as on the row.
    assert!(click(&v, &mut draft, "Apply colours").is_empty());
    // Steel blue picked for every lamp (a swatch's pick): Apply sends it for each lamp.
    let blue = [0x7f, 0xb0, 0xe0];
    draft
        .lamp
        .colors
        .insert(LAMP.into(), vec![Some(blue); LAMPS as usize]);
    let apply = click(&v, &mut draft, "Apply colours");
    let colors: Vec<(usize, [u8; 3])> = (0..LAMPS as usize).map(|i| (i, blue)).collect();
    assert_eq!(
        msgs(&apply),
        [Msg::Set {
            key: LAMP.into(),
            colors
        }]
    );
    // Busy, or not opened by Foxglove: the buttons are muted.
    let mut busy = card(on(), false);
    busy.lamp.as_mut().unwrap().busy = true;
    assert!(click(&busy, &mut Draft::default(), "Off").is_empty());
    assert!(click(&card(None, false), &mut Draft::default(), "Identify").is_empty());
}

/// #825 AC: the card's chips say what Windows allows, with the row's chips and hovers: "off in
/// Windows", "another app has it" with "Open Dynamic Lighting settings", "in front only", and
/// "not in Windows right now" with every control muted.
#[test]
fn the_cards_chips_say_what_windows_allows() {
    let off = LampAccess {
        enabled: false,
        available: true,
    };
    let taken = LampAccess {
        enabled: true,
        available: false,
    };
    let (text, all) = texts(&card(Some(off), true));
    assert!(text.contains("off in Windows"), "{text}");
    assert!(all.contains("Turned off in Windows' Dynamic Lighting settings."));
    let (text, all) = texts(&card(Some(taken), true));
    assert!(text.contains("another app has it"), "{text}");
    assert!(
        text.contains(crate::foxglove::lights_owner::DL_BUTTON),
        "{text}"
    );
    assert!(all.contains("Another app has these lights now."), "{all}");
    let (text, _) = texts(&card(Some(taken), false));
    assert!(text.contains("in front only"), "{text}");
    assert!(!text.contains(crate::foxglove::lights_owner::DL_BUTTON));
    let (text, _) = texts(&card(None, false));
    assert!(text.contains("not in Windows right now"), "{text}");
    assert!(text.contains("Apply colours"), "muted, never blank: {text}");
}

/// #825 AC: the zone drawing shows the keyboard's colours from its last write, as its row's
/// thumbnail does; a row with nothing written draws outlines only.
#[test]
fn the_zone_drawing_shows_its_colours() {
    let lit_strokes = |v: &View| {
        crate::foxglove::zones_ui::PREVIEW_OPEN.with_borrow_mut(|o| o.push(ZONES_KEY.into()));
        let ctx = pcu::tests::test_ctx();
        let mut draft = Draft::default();
        frame(&ctx, v, &mut draft, vec![]);
        let shapes = frame(&ctx, v, &mut draft, vec![]).1;
        crate::foxglove::zones_ui::PREVIEW_OPEN.with_borrow_mut(Vec::clear);
        let want = match crate::foxglove::device_thumb::Tint::of_row(&v.lamp.as_ref().unwrap().row)
        {
            crate::foxglove::device_thumb::Tint::Lit(cs) => cs.first().copied(),
            _ => None,
        };
        shapes
            .iter()
            .filter(|c| match (&c.shape, want) {
                (egui::Shape::Rect(r), Some(w)) => r.stroke.color == w,
                _ => false,
            })
            .count()
    };
    let v = card(on(), false);
    assert!(
        lit_strokes(&v) >= LAMPS as usize,
        "a key per lamp in its colour"
    );
    let mut blank = card(on(), false);
    blank
        .lamp
        .as_mut()
        .unwrap()
        .row
        .zones
        .iter_mut()
        .for_each(|z| z.1 = None);
    assert_eq!(lit_strokes(&blank), 0);
}

/// A VIA keyboard's own LampArray: covered when Windows lists it as a device row (the card
/// drives it, with VIA's own hover), ruled out otherwise (no row: the worker leaves a Raw HID
/// keyboard's LampArray to its link, #754, so the card stays read-only). The host link's
/// keyboard keeps its own card.
#[test]
fn a_via_keyboards_lamp_array_is_covered_only_as_a_row() {
    use crate::foxglove::keyboard_focus::Keyboard;
    let k5 = Keyboard {
        key: "via-k5".into(),
        name: "K5 Pro".into(),
        source: Source::Via,
        vid: 0x3434,
        pid: 0x0250,
        device: None,
        connected: true,
    };
    let row = crate::foxglove::fixtures::lamp_array_row("3434:0250", "K5 Pro lights");
    assert!(crate::foxglove::other_keyboards::is_row_of(&k5, &row));
    assert!(!crate::foxglove::other_keyboards::is_row_of(
        &k5,
        &lamp_row(on())
    ));
    let via = lights_only_hover(Source::Via).unwrap();
    assert!(
        via.contains("lights through Windows Dynamic Lighting"),
        "{via}"
    );
    assert!(!via.contains("lights or firmware"), "{via}");
    assert_eq!(lights_only_hover(Source::HostLink), None);
    // No row (the usual VIA keyboard): the read-only card, as before.
    let focus = crate::foxglove::focus_ui::Focused::of(&k5);
    let layout = crate::foxglove::key_layout::resolve(None, &[], 0);
    let ro = read_only_view(focus, crate::foxglove::light_zones::line(None, &layout));
    assert_eq!(ro.lamp, None);
    let (text, _) = texts(&ro);
    assert!(
        text.contains(crate::foxglove::focus_ui::READ_ONLY),
        "{text}"
    );
    assert!(!text.contains(LIGHTS_ONLY), "{text}");
}

/// The card's lights read caches only and queue the row's requests: nothing on the UI thread
/// asks a device, waits or starts a thread.
#[test]
fn the_cards_lights_ask_no_device() {
    let page = include_str!("lights_page.rs");
    let row = include_str!("pc_lights_ui.rs");
    let part = |src: &'static str, from: &str, to: &str| -> &'static str {
        let start = src.find(from).unwrap_or_else(|| panic!("{from}"));
        let end = start + src[start..].find(to).unwrap_or_else(|| panic!("{to}"));
        &src[start..end]
    };
    for (name, src) in [
        (
            "lights_page colour_side..live_view",
            part(page, "fn colour_side(", "/// The page as it shows now"),
        ),
        (
            "pc_lights_ui card_lights",
            part(
                row,
                "pub fn card_lights(",
                "/// What \"Match keyboard\" starts from",
            ),
        ),
    ] {
        for banned in [
            "ks::",
            "keyboard_settings",
            "pc_lights::send",
            "via_host",
            "spawn",
            "sleep(",
            "recv",
            "lamp_arrays()",
        ] {
            assert!(!src.contains(banned), "{name} uses {banned}");
        }
    }
    // The live card reads the worker's cache: one row, no device list or scan.
    let live = part(page, "pub fn live_view(", "fn underglow(");
    assert!(live.contains("pc_lights::find_row"));
    assert!(!live.contains("Msg::"), "live_view sends nothing");
}
