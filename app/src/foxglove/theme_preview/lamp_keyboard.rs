//! #825: the Lights page with a LampArray-only keyboard in focus, dark and light. Kept with
//! `FOXGLOVE_UI_PREVIEW_DIR` set.

use super::{keep, shell_preview_in, Samples};
use crate::foxglove::keyboard_focus::{Keyboard, Source};
use crate::foxglove::shell::Page;
use crate::foxglove::theme::Choice;

const LAMP: &str = "2f1e:0c01";

fn keyboard(key: &str, name: &str, source: Source, vid: u16, pid: u16) -> Keyboard {
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

/// The desk with the Lumen TKL (a LampArray-only keyboard, 87 lamps through Windows, last set
/// Bandit orange) in focus, the Corne and the K5 Pro known too.
fn lamp_desk() -> Samples {
    use crate::foxglove::focus_ui::{Focused, Switcher};
    use crate::foxglove::other_keyboards::Keyboards;
    use crate::foxglove::pc_lights::LampLayout;
    use foxglove_devices::pc_lighting::{LampAccess, LampArrayInfo, LightCaps};
    let corne = keyboard(
        "zmk-1d50:615e-p31",
        "Eyelash Corne",
        Source::HostLink,
        0x1d50,
        0x615e,
    );
    let k5 = keyboard("via-k5", "K5 Pro", Source::Via, 0x3434, 0x0250);
    let lamp = keyboard(
        LAMP,
        "Lumen TKL keyboard",
        Source::LampArray,
        0x2f1e,
        0x0c01,
    );
    let orange = [0xf0, 0x8a, 0x3c];
    let mut row = crate::foxglove::fixtures::lamp_array_row(&lamp.key, &lamp.name);
    row.caps = LightCaps::from_lamp_array(LampArrayInfo {
        lamp_count: 87,
        positions: true,
        min_update_us: 16_000,
        kind: 1,
    });
    row.zones = (1..=87)
        .map(|i| (format!("Lamp {i}"), Some(orange)))
        .collect();
    row.zone_ids = (0..87).map(|i| i.to_string()).collect();
    row.lamp_access = Some(LampAccess {
        enabled: true,
        available: true,
    });
    let lamps: Vec<[f32; 2]> = (0..87)
        .map(|i| {
            let (r, c) = (i / 15, i % 15);
            [12.0 + 19.0 * c as f32, 14.0 + 19.0 * r as f32]
        })
        .collect();
    let corne_keys = crate::foxglove::key_layout::resolve(
        Some(&crate::foxglove::fixtures::users_desk::board()),
        &[],
        0,
    );
    let lamp_keys = crate::foxglove::key_layout::resolve(None, &[], 87);
    let guess = crate::foxglove::key_layout::resolve(None, &[], 0);
    let mut s = Samples::full();
    s.pc.pc.devices.push(row);
    s.pc.lamps.insert(
        (0x2f1e, 0x0c01),
        LampLayout {
            kind: 1,
            size_mm: [300.0, 120.0],
            lamps,
        },
    );
    s.pc.keyboards = Keyboards {
        all: vec![corne.clone(), k5.clone(), lamp.clone()],
        focus: Some(LAMP.into()),
        host_link: Some("USB".into()),
        layouts: [
            (corne.key.clone(), corne_keys.clone()),
            (k5.key.clone(), guess.clone()),
        ]
        .into_iter()
        .collect(),
    };
    let mut switcher = Switcher::new(
        &[corne.clone(), k5.clone(), lamp.clone()],
        Some(LAMP),
        Some("USB"),
    );
    switcher.rows[0].layout = Some(corne_keys);
    switcher.rows[1].layout = Some(guess);
    switcher.rows[2].layout = Some(lamp_keys.clone());
    s.strip.switcher = switcher;
    let focus = Focused {
        read_only: crate::foxglove::lights_page::lights_only_hover(Source::LampArray),
        ..Focused::of(&lamp)
    };
    let row = s.pc.pc.devices.last().cloned().unwrap();
    s.strip.switcher.rows[2].tint = Some(crate::foxglove::device_thumb::Tint::of_row(&row));
    s.lights = crate::foxglove::lights_page::lamp_view(
        focus,
        crate::foxglove::lights_page::Lamp {
            row,
            busy: false,
            status: None,
            layout: lamp_keys,
            identity: false,
        },
    );
    s
}

/// #825: the keyboard card drives the Lumen TKL's lights (EFFECT, all lamps, Apply colours,
/// Off, Identify, Calibrate…; "lights only" and Windows' chip in the header), its zone drawing
/// open in its colours, and Other devices without its row (`ui-825-after-*`); the switcher's
/// list open on its thumbnail in those colours (`ui-825-after-list-*`). The before images
/// (read-only card, the kept row with "shown in Foxglove") came from main at eed265d8.
#[test]
fn lamp_keyboard_in_focus_renders() {
    use crate::foxglove::focus_ui::PREVIEW_OPEN;
    use crate::foxglove::zones_ui::PREVIEW_OPEN as ZONES_OPEN;
    let s = lamp_desk();
    let mut shots = Vec::new();
    for (choice, theme) in [(Choice::Dark, "dark"), (Choice::Light, "light")] {
        ZONES_OPEN.with_borrow_mut(|o| o.push(crate::foxglove::lights_page::ZONES_KEY.into()));
        let img = shell_preview_in(Page::Lights, &s, 1280.0, choice);
        ZONES_OPEN.with_borrow_mut(Vec::clear);
        PREVIEW_OPEN.set(true);
        let list = shell_preview_in(Page::Lights, &s, 420.0, choice);
        PREVIEW_OPEN.set(false);
        keep(&format!("ui-825-after-{theme}.png"), &img);
        keep(&format!("ui-825-after-list-{theme}.png"), &list);
        shots.push(img);
    }
    assert_ne!(shots[0], shots[1], "dark and light differ");
}
