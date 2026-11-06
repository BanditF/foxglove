//! The switcher with fake keyboards (#800): no device, no window.

use super::*;
use crate::foxglove::keyboard_focus::Seen;

const CORNE: &str = "zmk-1d50:615e-p31";
const K5: &str = "via-k5";

fn seen(key: &str, name: &str, vid: u16, pid: u16) -> Seen {
    Seen {
        key: key.into(),
        name: name.into(),
        vid,
        pid,
        device: Some(format!("dev-{key}")),
    }
}

fn keyboard(key: &str, name: &str, source: Source, connected: bool) -> Keyboard {
    Keyboard {
        key: key.into(),
        name: name.into(),
        source,
        vid: 0x3434,
        pid: 0x0250,
        device: None,
        connected,
    }
}

/// The Corne over the host link and a K5 Pro over VIA, as the device layer reports them.
pub fn two_keyboards() {
    kf::report(
        Source::HostLink,
        vec![seen(CORNE, "Eyelash Corne", 0x1d50, 0x615e)],
    );
    kf::report(Source::Via, vec![seen(K5, "K5 Pro", 0x3434, 0x0250)]);
}

/// The switcher the previews show: the Corne and a read-only K5 Pro, `focus` focused.
pub fn sample(focus: &str) -> Switcher {
    Switcher::new(
        &[
            Keyboard {
                vid: 0x1d50,
                pid: 0x615e,
                ..keyboard(CORNE, "Eyelash Corne", Source::HostLink, true)
            },
            keyboard(K5, "K5 Pro", Source::Via, true),
        ],
        Some(focus),
        Some("USB"),
    )
}

pub const SAMPLE_CORNE: &str = CORNE;
pub const SAMPLE_K5: &str = K5;

/// The list rows say "protocol · link · what Foxglove can do".
#[test]
fn rows_say_protocol_link_and_what_foxglove_can_do() {
    let corne = keyboard(CORNE, "Eyelash Corne", Source::HostLink, true);
    assert_eq!(detail(&corne, Some("USB")), "ZMK · USB · Foxglove firmware");
    assert_eq!(
        detail(&corne, Some("BLE")),
        "ZMK · Bluetooth · Foxglove firmware"
    );
    let gone = Keyboard {
        connected: false,
        ..corne.clone()
    };
    assert_eq!(
        detail(&gone, Some("BLE")),
        "ZMK · Bluetooth · not connected"
    );
    assert_eq!(
        detail(&keyboard(K5, "K5 Pro", Source::Via, true), None),
        "VIA · USB · read-only"
    );
    assert_eq!(
        detail(&keyboard("lamp", "Lamp", Source::LampArray, true), None),
        "Dynamic Lighting · lights only"
    );
}

/// VIA and LampArray-only keyboards carry the read-only chip's reason; the host link has none.
#[test]
fn read_only_keyboards_say_why() {
    assert_eq!(read_only_hover(Source::HostLink), None);
    let via = read_only_hover(Source::Via).unwrap();
    assert!(via.starts_with("Read-only over VIA: "), "{via}");
    let lamp = read_only_hover(Source::LampArray).unwrap();
    assert!(
        lamp.starts_with("Lights only, through Windows Dynamic Lighting: "),
        "{lamp}"
    );
    let s = sample(K5);
    assert!(s.focused().unwrap().read_only.is_some());
    assert!(sample(CORNE).focused().unwrap().read_only.is_none());
    assert_eq!(title(None, "Keyboard"), "Keyboard");
    assert_eq!(
        title(
            Some(&Focused::of(&keyboard(K5, "K5 Pro", Source::Via, true))),
            "Keyboard"
        ),
        "K5 Pro"
    );
}

/// A frame of the switcher with `events`; picks go to the focus as the strip sends them. Returns
/// the chip's rect.
fn frame(ctx: &egui::Context, events: Vec<egui::Event>) -> egui::Rect {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 400.0),
        )),
        events,
        ..Default::default()
    };
    let mut chip = egui::Rect::NOTHING;
    let mut out = ctx.run_ui(raw, |ui| {
        let s = Switcher::now();
        if let Some(key) = switcher(ui, &s) {
            pick(ui.ctx(), &key);
        }
        chip = ui.min_rect();
    });
    out.textures_delta.clear();
    chip
}

fn click(ctx: &egui::Context, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(ctx, vec![egui::Event::PointerMoved(at), button(true)]);
    frame(ctx, vec![button(false)]);
}

/// #800 AC: every action is one click. One click on the chip opens the list, one click on a row
/// makes that keyboard the focus (saved and announced by C1), and the list closes.
#[test]
fn one_click_opens_the_list_and_one_click_picks() {
    kf::TEST_SECTION.set(Some("focus-test-one-click"));
    crate::foxglove::store::test_config_dir();
    two_keyboards();
    assert_eq!(kf::current().map(|k| k.key), Some(CORNE.into()));
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    let chip = frame(&ctx, vec![]);
    assert!(!egui::Popup::is_id_open(&ctx, popup_id()));
    click(&ctx, chip.center());
    assert!(
        egui::Popup::is_id_open(&ctx, popup_id()),
        "one click opens the list"
    );
    // The list sizes itself on its first frame; a frame later its rows are where they stay.
    frame(&ctx, vec![]);
    let rows = LAST_ROWS.take();
    assert_eq!(rows.len(), 2, "both keyboards are listed");
    click(&ctx, rows[1].center());
    assert_eq!(
        kf::current().map(|k| k.key),
        Some(K5.into()),
        "one click picks"
    );
    frame(&ctx, vec![]);
    assert!(
        !egui::Popup::is_id_open(&ctx, popup_id()),
        "the list closed"
    );
    // Clicking the focused row changes nothing.
    click(&ctx, chip.center());
    frame(&ctx, vec![]);
    let rows = LAST_ROWS.take();
    click(&ctx, rows[1].center());
    assert_eq!(kf::current().map(|k| k.key), Some(K5.into()));
}

/// With one keyboard there is no choice: a flat chip without the ▾, and a click opens nothing.
#[test]
fn one_keyboard_has_no_caret_and_no_list() {
    kf::report(
        Source::HostLink,
        vec![seen(CORNE, "Eyelash Corne", 0x1d50, 0x615e)],
    );
    let s = Switcher::now();
    assert!(!s.can_switch());
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    let text = |ctx: &egui::Context| {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            switcher(ui, &Switcher::now());
        });
        out.textures_delta.clear();
        let mut text = String::new();
        for c in &out.shapes {
            if let egui::Shape::Text(t) = &c.shape {
                text.push_str(t.galley.text());
            }
        }
        text
    };
    let one = text(&ctx);
    assert!(
        one.contains("Eyelash Corne") && !one.contains(CARET),
        "{one}"
    );
    let chip = frame(&ctx, vec![]);
    click(&ctx, chip.center());
    assert!(!egui::Popup::is_id_open(&ctx, popup_id()));
    two_keyboards();
    let two = text(&ctx);
    assert!(two.contains(&format!("Eyelash Corne {CARET}")), "{two}");
}

/// #800 AC: switching repaints from cached state. After a pick, the strip, the Keyboard page's
/// drawing and the Lights card come from the focus model and B1's layout cache: the K5 Pro's
/// name, its read-only chip and its own layout, with no device asked.
#[test]
fn switching_repaints_from_the_caches() {
    kf::TEST_SECTION.set(Some("focus-test-repaint"));
    crate::foxglove::store::test_config_dir();
    two_keyboards();
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    pick(&ctx, K5);
    let focus = Focused::now().unwrap();
    assert_eq!(focus.name, "K5 Pro");
    assert!(focus.read_only.is_some());
    assert_eq!(
        Switcher::now().focused().map(|r| r.detail.as_str()),
        Some("VIA · USB · read-only")
    );
    let k5 = kf::current().unwrap();
    let lights = crate::foxglove::lights_page::live_view(None);
    assert_eq!(lights.focus.as_ref(), Some(&focus));
    assert!(!lights.controls && !lights.leds.0 && lights.calibration.is_none());
    assert_eq!(
        lights.zones.as_ref().map(|z| z.layout.clone()),
        Some(kf::layout(&k5)),
        "the zone drawing is on the K5 Pro's own layout"
    );
    let board = crate::foxglove::keyboard_page::read_only_board(&focus, kf::layout(&k5));
    assert_eq!(board.title, "K5 Pro");
    assert!(board.panel.is_none() && board.glow.is_none());
}

/// The switcher reads caches only: no device call, no wait, no thread.
#[test]
fn the_switcher_asks_no_device() {
    let src = include_str!("focus_ui.rs");
    for banned in [
        "hid",
        "via_host",
        "pc_lights::send",
        "keyboard_settings",
        "boards::find",
        "spawn",
        "sleep(",
        "recv",
    ] {
        assert!(!src.contains(banned), "focus_ui.rs uses {banned}");
    }
}
