//! Thumbnails in place (#804, D2), dark and light: the Lights page's Other devices rows
//! (`ui-804-other-devices-{dark,light}.png`) and the Keyboard page's Batteries list
//! (`ui-804-batteries-{dark,light}.png`). Kept with `FOXGLOVE_UI_PREVIEW_DIR` set.

use super::{keep, shell_preview_in, Samples};
use crate::foxglove::shell::Page;
use crate::foxglove::theme::Choice;

#[test]
fn device_rows_render() {
    let mut images = Vec::new();
    for (choice, suffix) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let lights = shell_preview_in(Page::Lights, &Samples::full(), 1180.0, choice);
        let batteries = shell_preview_in(Page::Keyboard, &Samples::full(), 1300.0, choice);
        keep(&format!("ui-804-other-devices-{suffix}.png"), &lights);
        keep(&format!("ui-804-batteries-{suffix}.png"), &batteries);
        images.push((lights, batteries));
    }
    assert_ne!(images[0].0, images[1].0, "dark and light differ");
    assert_ne!(images[0].1, images[1].1, "dark and light differ");
}

/// #820: Other devices with a K5 Pro over VIA and a Dynamic Lighting keyboard, dark and light.
/// Before (`ui-820-before-*`): the focus model left out, which draws the card as it was; the
/// K5 Pro isn't there and the lamp keyboard is a plain lights row. After, with the Corne
/// focused (`ui-820-after-*`): the K5 Pro on a row of its own and the lamp keyboard's row, each
/// with "Show in Foxglove". With the lamp keyboard focused (`ui-820-after-lamp-focused-*`): its
/// row isn't listed (the keyboard card drives its lights, #825), and the Corne and the K5 Pro
/// carry the link.
#[test]
fn show_in_foxglove_renders() {
    use crate::foxglove::keyboard_focus::{Keyboard, Source};
    use crate::foxglove::other_keyboards::Keyboards;
    use crate::foxglove::pc_lights::LampLayout;
    use crate::foxglove::pc_lights_ui::tests::sample;
    use foxglove_devices::pc_lighting::{LampAccess, LampArrayInfo, LightCaps};
    let keyboard = |key: &str, name: &str, source, vid, pid| Keyboard {
        key: key.into(),
        name: name.into(),
        source,
        vid,
        pid,
        device: None,
        connected: true,
    };
    let corne = keyboard(
        "zmk-1d50:615e-p31",
        "Eyelash Corne",
        Source::HostLink,
        0x1d50,
        0x615e,
    );
    let k5 = keyboard("via-k5", "K5 Pro", Source::Via, 0x3434, 0x0250);
    let lamp = keyboard(
        "2f1e:0c01",
        "Lumen TKL keyboard",
        Source::LampArray,
        0x2f1e,
        0x0c01,
    );
    // The lamp keyboard's row as the Lights worker lists it: 87 lamps through Windows.
    let mut lamp_row = crate::foxglove::fixtures::lamp_array_row(&lamp.key, &lamp.name);
    lamp_row.caps = LightCaps::from_lamp_array(LampArrayInfo {
        lamp_count: 87,
        positions: true,
        min_update_us: 16_000,
        kind: 1,
    });
    lamp_row.lamp_access = Some(LampAccess {
        enabled: true,
        available: true,
    });
    // A tenkeyless grid: six rows of keys, 19 mm apart.
    let lamps: Vec<[f32; 2]> = (0..87)
        .map(|i| {
            let (row, col) = (i / 15, i % 15);
            [12.0 + 19.0 * col as f32, 14.0 + 19.0 * row as f32]
        })
        .collect();
    let mut before = sample();
    before.pc.devices.push(lamp_row);
    before.lamps.insert(
        (0x2f1e, 0x0c01),
        LampLayout {
            kind: 1,
            size_mm: [300.0, 120.0],
            lamps,
        },
    );
    // The K5 Pro's own keys, from its VIA definition (B2).
    let (via, layout) = crate::foxglove::via_host::tests::k5_pro_opened(1);
    let k5_keys = crate::foxglove::key_layout::resolve(
        None,
        &[crate::foxglove::via_host::layout_of(&via, &layout).unwrap()],
        0,
    );
    // The Corne's from its board profile.
    let corne_keys = crate::foxglove::key_layout::resolve(
        Some(&crate::foxglove::fixtures::users_desk::board()),
        &[],
        0,
    );
    let with_focus = |focus: &Keyboard| {
        let mut v = before.clone();
        v.keyboards = Keyboards {
            all: vec![corne.clone(), k5.clone(), lamp.clone()],
            focus: Some(focus.key.clone()),
            host_link: Some("USB".into()),
            layouts: [
                (k5.key.clone(), k5_keys.clone()),
                (corne.key.clone(), corne_keys.clone()),
            ]
            .into_iter()
            .collect(),
        };
        v
    };
    let after = with_focus(&corne);
    let lamp_focused = with_focus(&lamp);
    let none: Vec<String> = Vec::new();
    let mut shots = Vec::new();
    for (choice, theme) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        let (b, _) = super::other_devices_card(choice, &before, &none, 760.0);
        let (a, _) = super::other_devices_card(choice, &after, &none, 760.0);
        let (l, _) = super::other_devices_card(choice, &lamp_focused, &none, 760.0);
        keep(&format!("ui-820-before-{theme}.png"), &b);
        keep(&format!("ui-820-after-{theme}.png"), &a);
        keep(&format!("ui-820-after-lamp-focused-{theme}.png"), &l);
        assert_ne!(b, a, "{theme}: the K5 Pro's row and the links");
        assert_ne!(a, l, "{theme}: the focus moved");
        shots.push(a);
    }
    assert_ne!(shots[0], shots[1], "dark and light differ");
}
