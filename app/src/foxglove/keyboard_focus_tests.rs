//! The focus model with fake keyboards (#799): no device, no window.

use super::*;

const CORNE: &str = "zmk-1d50:615e-p31";
const K5: &str = "via-k5";
const LAMP: &str = "045e:0b13";

fn seen(key: &str, name: &str, vid: u16, pid: u16) -> Seen {
    Seen {
        key: key.into(),
        name: name.into(),
        vid,
        pid,
        device: Some(format!("dev-{key}")),
    }
}

fn corne() -> Seen {
    seen(CORNE, "Eyelash Corne", 0x1d50, 0x615e)
}

fn k5() -> Seen {
    seen(K5, "K5 Pro", 0x3434, 0x0250)
}

fn lamp() -> Seen {
    Seen {
        device: None,
        ..seen(LAMP, "Lamp keyboard", 0x045e, 0x0b13)
    }
}

fn focus(m: &Model) -> Option<&str> {
    m.focus().map(|k| k.key.as_str())
}

fn remembering(key: &str) -> Model {
    Model::new(&Saved {
        keyboard: Some(key.into()),
        ..Saved::default()
    })
}

/// With one keyboard nothing changes: it is the focus, and every view shows it.
#[test]
fn one_keyboard_is_the_focus_everywhere() {
    let mut m = Model::default();
    assert_eq!(m.focus(), None);
    assert!(m.report(Source::HostLink, &[corne()]));
    assert_eq!(focus(&m), Some(CORNE));
    for view in View::ALL {
        assert_eq!(m.for_view(view).map(|k| k.key.as_str()), Some(CORNE));
    }
    assert!(!m.focus().unwrap().read_only());
    assert!(
        !m.report(Source::HostLink, &[corne()]),
        "the same report changes nothing"
    );
}

/// The start rule: the remembered keyboard if it connects, else the host-link one, else the
/// first one found; and it finishes as keyboards arrive in any order.
#[test]
fn the_start_rule_remembered_then_host_link_then_first() {
    // Remembered, among others.
    let mut m = remembering(K5);
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::Via, &[k5()]);
    assert_eq!(focus(&m), Some(K5));

    // Remembered isn't here: the host-link keyboard, even when found after another one.
    let mut m = remembering("gone-for-good");
    m.report(Source::LampArray, &[lamp()]);
    assert_eq!(
        focus(&m),
        Some(LAMP),
        "nothing was focused: the first found"
    );
    m.report(Source::Via, &[k5()]);
    assert_eq!(
        focus(&m),
        Some(LAMP),
        "no host link yet: the first found stays"
    );
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(
        focus(&m),
        Some(CORNE),
        "the start rule prefers the host link"
    );

    // Neither: the first found.
    let mut m = Model::default();
    m.report(Source::Via, &[k5()]);
    m.report(Source::LampArray, &[lamp()]);
    assert_eq!(focus(&m), Some(K5));

    // The remembered keyboard arriving late finishes the start rule.
    let mut m = remembering(K5);
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(focus(&m), Some(CORNE));
    m.report(Source::Via, &[k5()]);
    assert_eq!(focus(&m), Some(K5));
}

/// A new keyboard never steals the focus, except when nothing was focused.
#[test]
fn a_new_keyboard_does_not_steal_the_focus() {
    // An automatic focus on the host link: a VIA and a LampArray keyboard arrive.
    let mut m = Model::default();
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::Via, &[k5()]);
    m.report(Source::LampArray, &[lamp()]);
    assert_eq!(focus(&m), Some(CORNE));

    // A picked focus: not even the host link takes it.
    let mut m = Model::default();
    m.report(Source::Via, &[k5()]);
    m.pick(K5).unwrap();
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(focus(&m), Some(K5));

    // The remembered keyboard holds it against a host link arriving later.
    let mut m = remembering(K5);
    m.report(Source::Via, &[k5()]);
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(focus(&m), Some(K5));
}

/// A gone keyboard stays listed, greyed, in its place; a picked focus stays on it.
#[test]
fn a_gone_keyboard_is_greyed_and_keeps_its_place() {
    let mut m = Model::default();
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::Via, &[k5()]);
    m.pick(K5).unwrap();
    assert!(m.report(Source::Via, &[]));
    let keys: Vec<_> = m
        .keyboards()
        .iter()
        .map(|k| (k.key.as_str(), k.connected))
        .collect();
    assert_eq!(keys, [(CORNE, true), (K5, false)]);
    assert_eq!(focus(&m), Some(K5), "the pick stays, greyed");
    assert!(!m.focus().unwrap().connected);
    let facts = m.facts();
    let fact = |k: &str| facts.iter().find(|(n, _)| *n == k).unwrap().1.clone();
    assert_eq!(fact("focus.keyboard"), Some(Value::Text(K5.into())));
    assert_eq!(fact("focus.connected"), Some(Value::Bool(false)));
    assert_eq!(fact("focus.keyboards"), Some(Value::Int(2)));
    assert_eq!(fact("focus.connected_keyboards"), Some(Value::Int(1)));
}

/// A replug keeps the pick and the list's order, picked or automatic.
#[test]
fn a_replug_keeps_the_pick() {
    // Picked: stays through the unplug and the replug.
    let mut m = Model::default();
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::Via, &[k5()]);
    m.pick(K5).unwrap();
    m.report(Source::Via, &[]);
    m.report(Source::Via, &[k5()]);
    assert_eq!(focus(&m), Some(K5));
    assert!(m.focus().unwrap().connected);
    let order: Vec<_> = m.keyboards().iter().map(|k| k.key.as_str()).collect();
    assert_eq!(order, [CORNE, K5]);

    // The only keyboard: greyed while away, the focus again when back.
    let mut m = Model::default();
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::HostLink, &[]);
    assert_eq!(focus(&m), Some(CORNE));
    assert!(!m.focus().unwrap().connected);
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(focus(&m), Some(CORNE));
    assert!(m.focus().unwrap().connected);

    // Automatic, with another keyboard there: it covers while the host link is away, and the
    // host link takes it back on its replug.
    let mut m = Model::default();
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::Via, &[k5()]);
    m.report(Source::HostLink, &[]);
    assert_eq!(focus(&m), Some(K5));
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(focus(&m), Some(CORNE));
}

/// Read-only VIA keyboards and LampArray-only keyboards can be focused, with their limits.
#[test]
fn read_only_and_lamp_array_keyboards_can_be_focused() {
    let mut m = Model::default();
    m.report(Source::Via, &[k5()]);
    m.report(Source::LampArray, &[lamp()]);
    m.pick(LAMP).unwrap();
    let k = m.focus().unwrap();
    assert!(k.read_only());
    assert_eq!(
        k.source.limit(),
        Some("lights only, through Windows Dynamic Lighting")
    );
    m.pick(K5).unwrap();
    assert_eq!(
        m.focus().unwrap().source.limit(),
        Some("read-only over VIA")
    );
    assert!(m.pick("nobody").is_err());
    assert_eq!(focus(&m), Some(K5));
}

/// A LampArray with a known keyboard's VID/PID is that keyboard, listed once; a focus on it
/// moves to that keyboard.
#[test]
fn a_keyboards_own_lamp_array_is_listed_once() {
    let mut m = Model::default();
    let corne_lamps = Seen {
        device: None,
        ..seen("1d50:615e", "Corne lights", 0x1d50, 0x615e)
    };
    m.report(Source::LampArray, std::slice::from_ref(&corne_lamps));
    m.pin(View::Overlay, Some("1d50:615e")).unwrap();
    assert_eq!(focus(&m), Some("1d50:615e"));
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(m.keyboards().len(), 1);
    assert_eq!(focus(&m), Some(CORNE));
    assert_eq!(m.pins().overlay.as_deref(), Some(CORNE));
    m.report(Source::LampArray, &[corne_lamps]);
    assert_eq!(m.keyboards().len(), 1, "a later scan doesn't add it again");
}

/// The overlay and the feed may pin a keyboard; unpinned they follow the focus.
#[test]
fn a_pinned_view_keeps_its_keyboard_while_the_focus_moves() {
    let mut m = Model::default();
    m.report(Source::HostLink, &[corne()]);
    m.report(Source::Via, &[k5()]);
    m.pin(View::Overlay, Some(CORNE)).unwrap();
    m.pick(K5).unwrap();
    assert_eq!(m.for_view(View::Overlay).unwrap().key, CORNE);
    assert_eq!(m.for_view(View::Feed).unwrap().key, K5);
    assert!(m.pin(View::Feed, Some("nobody")).is_err());
    m.pin(View::Overlay, None).unwrap();
    assert_eq!(m.for_view(View::Overlay).unwrap().key, K5);

    // A saved pin to a keyboard not found yet follows the focus until it is.
    let mut m = Model::new(&Saved {
        pins: Pins {
            feed: Some(K5.into()),
            overlay: None,
        },
        ..Saved::default()
    });
    m.report(Source::HostLink, &[corne()]);
    assert_eq!(m.for_view(View::Feed).unwrap().key, CORNE);
    m.report(Source::Via, &[k5()]);
    assert_eq!(m.for_view(View::Feed).unwrap().key, K5);
    assert_eq!(focus(&m), Some(CORNE));
}

/// `[focus]`: follow-typing is off by default, and an empty section saves as nothing.
#[test]
fn the_saved_section_round_trips_and_follow_typing_is_off() {
    assert!(!Saved::default().follow_typing);
    assert!(!Model::default().follow_typing());
    let saved = Saved {
        keyboard: Some(K5.into()),
        follow_typing: false,
        pins: Pins {
            overlay: Some(CORNE.into()),
            feed: None,
        },
    };
    let text = toml::to_string(&saved).unwrap();
    assert_eq!(toml::from_str::<Saved>(&text).unwrap(), saved);
    assert_eq!(toml::from_str::<Saved>("").unwrap(), Saved::default());
}

/// The keyboards among a lighting scan: LampArrays of the keyboard kind, and keyboards' own
/// LampArrays once each; a LampArray mouse isn't one.
#[test]
fn a_lighting_scans_keyboards() {
    use crate::foxglove::pc_lights::{LampLayout, Snapshot};
    use foxglove_devices::pc_lighting::KeyboardLampArray;
    let layout = |kind| LampLayout {
        kind,
        ..LampLayout::default()
    };
    let layouts = [((0x045e, 0x0b13), layout(1)), ((0x046d, 0xc08b), layout(2))]
        .into_iter()
        .collect();
    let own = |key: &str, bluetooth| KeyboardLampArray {
        key: key.into(),
        name: "Raw HID board".into(),
        vid: 0x1234,
        pid: 0x5678,
        bluetooth,
    };
    let snapshot = Snapshot {
        devices: vec![
            crate::foxglove::fixtures::lamp_array_row("045e:0b13abc", "Lamp keyboard"),
            crate::foxglove::fixtures::lamp_array_row("046d:c08b", "Lamp mouse"),
        ],
        keyboard_lamp_arrays: vec![own("1234:5678", false), own("1234:5678ble", true)],
        ..Snapshot::default()
    };
    let found = lamp_array_keyboards(&snapshot, &layouts);
    let keys: Vec<_> = found
        .iter()
        .map(|s| (s.key.as_str(), s.vid, s.pid))
        .collect();
    assert_eq!(
        keys,
        [
            ("045e:0b13abc", 0x045e, 0x0b13),
            ("1234:5678", 0x1234, 0x5678)
        ]
    );
}

/// A keyboard's layout comes from B1's ladder for its own VID/PID: what its source handed over,
/// and never the host keyboard's keymap count.
#[test]
fn each_keyboard_has_its_own_layout() {
    use crate::foxglove::key_layout::{self, LayoutSource};
    let via = Keyboard {
        key: K5.into(),
        name: "K5 Pro".into(),
        source: Source::Via,
        vid: 0x3434,
        pid: 0x0250,
        device: None,
        connected: true,
    };
    let studio = crate::protocols::zmk::studio_layouts(&key_layout::tests::corne_studio());
    key_layout::set_for_test(0x3434, 0x0250, Some(studio));
    assert_eq!(layout(&via).source, LayoutSource::Keyboard);
    key_layout::set_for_test(0x1d50, 0x615e, None);
    // Not handed over and no profile: the default grid, whatever the host keyboard's keymap.
    assert_eq!(
        layout(&via).source,
        key_layout::resolve(None, &[], 0).source
    );
}

/// Through the store and the bus: a pick is saved in `[focus]` read-modify-write (a pin saved
/// before survives), announced as `focus.*` facts, and remembered by the next start.
#[test]
fn a_pick_is_saved_announced_and_remembered() {
    crate::foxglove::store::test_config_dir();
    let changes = subscribe();
    report(Source::HostLink, vec![corne()]);
    report(Source::Via, vec![k5()]);
    assert_eq!(current().map(|k| k.key), Some(CORNE.into()));
    pin(View::Feed, Some(CORNE)).unwrap();
    let picked = set(K5).unwrap();
    assert_eq!(picked.key, K5);
    assert!(set("nobody").is_err());
    let saved: Saved = crate::foxglove::store::try_load(SECTION).unwrap();
    assert_eq!(saved.keyboard.as_deref(), Some(K5));
    assert_eq!(
        saved.pins.feed.as_deref(),
        Some(CORNE),
        "the pin survived the pick"
    );
    assert!(!saved.follow_typing);
    let got: Vec<_> = changes.try_iter().collect();
    assert!(got
        .iter()
        .any(|c| c.key == "focus.keyboard" && c.value == Some(Value::Text(K5.into()))));
    assert_eq!(for_view(View::Feed).map(|k| k.key), Some(CORNE.into()));
    assert_eq!(keyboards().len(), 2);
    // The next start: the remembered keyboard wins over the host link.
    let mut next = loaded();
    next.report(Source::HostLink, &[corne()]);
    next.report(Source::Via, &[k5()]);
    assert_eq!(focus(&next), Some(K5));
}

/// Event-driven: the module waits on nothing and runs no thread of its own.
#[test]
fn the_focus_never_polls() {
    let src = include_str!("keyboard_focus.rs");
    for banned in [
        "sleep(",
        "recv_timeout",
        "Instant::",
        "spawn",
        "wait_timeout",
    ] {
        assert!(!src.contains(banned), "keyboard_focus.rs uses {banned}");
    }
}
