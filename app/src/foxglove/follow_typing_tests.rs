//! "Follow the keyboard I type on" (#802) with synthetic device events: no Raw Input, no device,
//! no window.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::foxglove::keyboard_focus::{self as kf, Model, Saved, Seen};

const CORNE: &str = "zmk-1d50:615e-p31";
const K5: &str = "via-k5";
const CORNE_PATH: &str =
    r"\\?\HID#VID_1D50&PID_615E&MI_00#7&2a1b3c4d&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
const K5_PATH: &str = r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&023434_PID&0250_REV&0001_c8a1b2c3d4e5#9&1c2d3e4f&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";

fn seen(key: &str, name: &str, vid: u16, pid: u16) -> Seen {
    Seen {
        key: key.into(),
        name: name.into(),
        vid,
        pid,
        device: None,
    }
}

/// The Corne over the host link (the focus by the start rule) and a K5 Pro over VIA.
fn two_keyboards(follow: bool) -> Model {
    let mut m = Model::new(&Saved {
        follow_typing: follow,
        ..Saved::default()
    });
    m.report(
        kf::Source::HostLink,
        &[seen(CORNE, "Eyelash Corne", 0x1d50, 0x615e)],
    );
    m.report(kf::Source::Via, &[seen(K5, "K5 Pro", 0x3434, 0x0250)]);
    m
}

fn focus(m: &Model) -> &str {
    m.focus().map(|k| k.key.as_str()).unwrap_or("none")
}

/// `n` events from a keyboard, `every` apart from `from`; when the last one came.
fn type_on(m: &mut Model, ids: (u16, u16), n: u32, from: Instant, every: Duration) -> Instant {
    let mut at = from;
    for i in 0..n {
        at = from + every * i;
        m.typed(ids.0, ids.1, at);
    }
    at
}

const CORNE_IDS: (u16, u16) = (0x1d50, 0x615e);
const K5_IDS: (u16, u16) = (0x3434, 0x0250);
const KEY: Duration = Duration::from_millis(60);

#[test]
fn the_setting_is_off_by_default_and_off_follows_nothing() {
    assert!(!Saved::default().follow_typing);
    let mut m = two_keyboards(false);
    assert!(!m.follow_typing());
    type_on(&mut m, K5_IDS, 40, Instant::now(), KEY);
    assert_eq!(focus(&m), CORNE, "off: typing never moves the focus");
}

#[test]
fn a_few_keys_on_another_keyboard_move_the_focus() {
    let mut m = two_keyboards(true);
    let t = Instant::now();
    // Two keys (four events) aren't enough.
    type_on(&mut m, K5_IDS, FOLLOW_AFTER - 1, t, KEY);
    assert_eq!(focus(&m), CORNE);
    // The third key's release is.
    assert!(m.typed(K5_IDS.0, K5_IDS.1, t + KEY * FOLLOW_AFTER));
    assert_eq!(focus(&m), K5);
    // Typing on it changes nothing more.
    assert!(!m.typed(K5_IDS.0, K5_IDS.1, t + KEY * (FOLLOW_AFTER + 1)));
}

#[test]
fn a_long_pause_starts_the_count_again() {
    let mut m = two_keyboards(true);
    let t = Instant::now();
    // One stray key every few seconds never adds up.
    for i in 0..10 {
        m.typed(K5_IDS.0, K5_IDS.1, t + STREAK_GAP * 2 * i);
    }
    assert_eq!(focus(&m), CORNE);
}

#[test]
fn typing_on_two_boards_at_once_never_flickers() {
    let mut m = two_keyboards(true);
    let t = Instant::now();
    let mut moves = 0;
    // Interleaved events, both boards at once for ten seconds: no run from either.
    for i in 0..400u32 {
        let ids = if i % 2 == 0 { K5_IDS } else { CORNE_IDS };
        moves += u32::from(m.typed(ids.0, ids.1, t + Duration::from_millis(25) * i));
    }
    assert_eq!(moves, 0);
    assert_eq!(focus(&m), CORNE);
}

#[test]
fn it_switches_at_most_once_per_second() {
    let mut m = two_keyboards(true);
    let t = Instant::now();
    let mut switches: Vec<Instant> = Vec::new();
    // Bursts of three keys, switching boards each burst, as fast as anyone types.
    let mut at = t;
    for burst in 0..60 {
        let ids = if burst % 2 == 0 { K5_IDS } else { CORNE_IDS };
        for _ in 0..FOLLOW_AFTER {
            at += Duration::from_millis(20);
            if m.typed(ids.0, ids.1, at) {
                switches.push(at);
            }
        }
    }
    assert!(switches.len() > 1, "it does follow: {}", switches.len());
    for pair in switches.windows(2) {
        assert!(pair[1] - pair[0] >= MIN_SWITCH, "{:?}", pair[1] - pair[0]);
    }
    // A run held back by the second still moves it once the second is up, with no timer.
    let mut m = two_keyboards(true);
    let t = type_on(&mut m, K5_IDS, FOLLOW_AFTER, Instant::now(), KEY);
    assert_eq!(focus(&m), K5);
    let held = type_on(&mut m, CORNE_IDS, FOLLOW_AFTER, t + KEY, KEY);
    assert_eq!(focus(&m), K5, "less than a second after the last switch");
    let mut at = held;
    while at < t + MIN_SWITCH {
        at += KEY;
        m.typed(CORNE_IDS.0, CORNE_IDS.1, at);
    }
    assert_eq!(focus(&m), CORNE);
}

#[test]
fn a_pick_wins_until_the_user_types_on_another_keyboard() {
    let mut m = two_keyboards(true);
    let t = type_on(&mut m, CORNE_IDS, FOLLOW_AFTER, Instant::now(), KEY);
    assert_eq!(focus(&m), CORNE);
    // Picked while typing on the Corne: the pick holds however long they type there.
    m.pick(K5).unwrap();
    let t = type_on(&mut m, CORNE_IDS, 100, t + MIN_SWITCH, KEY);
    assert_eq!(focus(&m), K5);
    // Typing on another keyboard follows again.
    m.pick(CORNE).unwrap();
    type_on(&mut m, K5_IDS, FOLLOW_AFTER, t + KEY, KEY);
    assert_eq!(focus(&m), K5);
}

#[test]
fn unknown_or_gone_keyboards_and_turning_it_off_change_nothing() {
    let mut m = two_keyboards(true);
    // A laptop's own keyboard Foxglove doesn't know.
    type_on(&mut m, (0x046d, 0xc31c), 40, Instant::now(), KEY);
    assert_eq!(focus(&m), CORNE);
    // A K5 that went is not typing.
    m.report(kf::Source::Via, &[]);
    type_on(&mut m, K5_IDS, 40, Instant::now(), KEY);
    assert_eq!(focus(&m), CORNE);
    // Off forgets the run.
    let mut m = two_keyboards(true);
    let t = type_on(&mut m, K5_IDS, FOLLOW_AFTER - 1, Instant::now(), KEY);
    m.set_follow_typing(false);
    m.set_follow_typing(true);
    m.typed(K5_IDS.0, K5_IDS.1, t + KEY);
    assert_eq!(focus(&m), CORNE);
}

#[test]
fn device_paths_give_their_ids() {
    assert_eq!(ids(CORNE_PATH), Some(CORNE_IDS));
    assert_eq!(ids(K5_PATH), Some(K5_IDS), "Bluetooth LE");
    assert_eq!(
        ids(
            r"\\?\HID#{00001124-0000-1000-8000-00805f9b34fb}_VID&0002046d_PID&b342&Col01#8&1&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"
        ),
        Some((0x046d, 0xb342)),
        "Bluetooth"
    );
    assert_eq!(
        ids(r"\\?\Root#RDP_KBD#0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}"),
        None
    );
    assert_eq!(ids(""), None);
}

/// A source the test types on: what Raw Input would send, device paths only.
#[derive(Clone, Default)]
struct Synthetic {
    sink: Arc<Mutex<Option<Sink>>>,
    stopped: Arc<Mutex<bool>>,
}

impl Synthetic {
    fn type_on(&self, path: &str, events: u32) {
        let mut sink = self.sink.lock().unwrap();
        for _ in 0..events {
            (sink.as_mut().expect("started"))(path);
        }
    }
}

impl Source for Synthetic {
    fn start(self: Box<Self>, sink: Sink) -> Result<Stop, String> {
        *self.sink.lock().unwrap() = Some(sink);
        let stopped = self.stopped.clone();
        let held = self.sink.clone();
        Ok(Box::new(move || {
            *held.lock().unwrap() = None;
            *stopped.lock().unwrap() = true;
        }))
    }
}

/// Through the injectable source, the store and the bus: the setting is saved, a run of events
/// moves the app's focus and announces it, a pick holds, and off stops the source.
#[test]
fn a_synthetic_source_drives_the_focus() {
    crate::foxglove::store::test_config_dir();
    kf::TEST_SECTION.set(Some("focus-follow-typing-test"));
    kf::report(
        kf::Source::HostLink,
        vec![seen(CORNE, "Eyelash Corne", 0x1d50, 0x615e)],
    );
    kf::report(kf::Source::Via, vec![seen(K5, "K5 Pro", 0x3434, 0x0250)]);
    assert!(!kf::follow_typing(), "off by default");
    let changes = kf::subscribe();
    let source = Synthetic::default();
    kf::set_follow_typing(true);
    listen(Box::new(source.clone()));
    assert!(listening());
    let saved: Saved = crate::foxglove::store::try_load("focus-follow-typing-test").unwrap();
    assert!(saved.follow_typing);

    source.type_on(K5_PATH, FOLLOW_AFTER);
    assert_eq!(kf::current().map(|k| k.key), Some(K5.into()));
    assert!(changes.try_iter().any(|c| c.key == "focus.keyboard"
        && c.value == Some(foxglove_devices::facts::Value::Text(K5.into()))));
    // Picked while typing on the K5: more K5 typing keeps the pick.
    kf::set(CORNE).unwrap();
    source.type_on(K5_PATH, 40);
    assert_eq!(kf::current().map(|k| k.key), Some(CORNE.into()));

    kf::set_follow_typing(false);
    quiet();
    assert!(!listening());
    assert!(*source.stopped.lock().unwrap(), "off stops the source");
    let saved: Saved = crate::foxglove::store::try_load("focus-follow-typing-test").unwrap();
    assert!(!saved.follow_typing);
    kf::TEST_SECTION.set(None);
}

/// The source, without these tests: everything outside `#[cfg(test)]`.
fn source() -> &'static str {
    let src = include_str!("follow_typing.rs");
    &src[..src.rfind("#[cfg(test)]").expect("the tests' module")]
}

/// Privacy (#802 AC): Foxglove reads only each event's header (which device), never the key:
/// no keyboard data is read, and no path or key is logged or kept beyond the handle's path.
#[test]
fn only_the_device_handle_is_read() {
    let src = source();
    assert!(src.contains("RID_HEADER"), "the header only");
    assert_eq!(
        src.matches("GetRawInputData(").count(),
        1,
        "one read of an event, the header's"
    );
    for banned in [
        "RID_INPUT",
        "RAWKEYBOARD",
        ".keyboard",
        "VKey",
        "MakeCode",
        "ScanCode",
        "WM_KEYDOWN",
        "WM_CHAR",
        "GetKeyState",
        "ToUnicode",
        "SetWindowsHookEx",
    ] {
        assert!(!src.contains(banned), "follow_typing.rs uses {banned}");
    }
    // The only logs: a failed start. The focus log names keyboards only.
    for line in src.lines().filter(|l| l.contains("eprintln!")) {
        assert!(
            line.contains("can't follow the keyboard you type on"),
            "{line}"
        );
    }
    let focus_src = include_str!("keyboard_focus.rs");
    let at = focus_src.find("(typed on)").expect("the focus log");
    let log = &focus_src[at..at + 160];
    assert!(log.contains("before") && log.contains("k.name"), "{log}");
}

/// Event-driven: Raw Input's WM_INPUT on a message loop; nothing sleeps, ticks or polls.
#[test]
fn it_waits_on_events_only() {
    let src = source();
    assert!(src.contains("WM_INPUT") && src.contains("GetMessageW"));
    for banned in [
        "sleep(",
        "SetTimer",
        "PeekMessage",
        "GetAsyncKeyState",
        "MsgWaitForMultipleObjects",
    ] {
        assert!(!src.contains(banned), "follow_typing.rs uses {banned}");
    }
}

#[test]
fn the_hover_starts_with_what_it_does() {
    for on in [false, true] {
        let h = hover(on);
        assert!(h.starts_with("Shows the keyboard you last typed on"), "{h}");
        assert!(!h.contains(LABEL), "the label isn't repeated: {h}");
    }
    assert!(hover(false).ends_with(OFF));
    assert!(!hover(true).contains(OFF));
}

/// winit's registrations as winit 0.30 makes them: the mouse and the keyboard, on its
/// event-target window, with plug events.
fn winits(target: isize) -> Vec<Registration> {
    vec![
        Registration {
            page: DESKTOP,
            usage: 0x02,
            flags: DEV_NOTIFY,
            target,
        },
        Registration {
            page: DESKTOP,
            usage: KEYBOARD,
            flags: DEV_NOTIFY,
            target,
        },
    ]
}

/// Only the keyboard's usage moves to the listener, and it goes back to winit's exactly when the
/// setting goes off; with no registration before (or its window gone) the keyboard's is removed.
#[test]
fn only_the_keyboard_is_registered_and_it_is_handed_back() {
    let ours = ours(0x1234);
    assert_eq!((ours.page, ours.usage), (DESKTOP, KEYBOARD));
    assert_eq!(ours.flags, INPUT_SINK | DEV_NOTIFY);
    let before = keyboard_of(&winits(0x99));
    assert_eq!(
        before,
        Some(winits(0x99)[1]),
        "the keyboard's, not the mouse's"
    );
    assert_eq!(
        on_stop(before, |w| w == 0x99),
        winits(0x99)[1],
        "winit's back"
    );
    let removed = Registration {
        page: DESKTOP,
        usage: KEYBOARD,
        flags: REMOVE,
        target: 0,
    };
    assert_eq!(on_stop(before, |_| false), removed, "its window went");
    assert_eq!(on_stop(None, |_| true), removed, "nothing was registered");
    assert_eq!(keyboard_of(&winits(0x99)[..1]), None);
    // The raw module registers only these two, so the mouse's (winit's) is never touched.
    let src = source();
    assert_eq!(src.matches("register(super::ours(").count(), 1);
    assert_eq!(src.matches("register(super::on_stop(").count(), 2);
    assert_eq!(
        src.matches("register(").count() - src.matches("fn register(").count(),
        3,
        "no other registration"
    );
    assert_eq!(
        src.matches("RegisterRawInputDevices(").count(),
        1,
        "one call site"
    );
}

/// Typing and IME in Foxglove's own windows don't come from raw keyboard input: winit 0.30 makes
/// `WindowEvent::KeyboardInput` and `Ime` from window messages (key, character, IME), and raw
/// keyboard input only into `DeviceEvent::Key`, which nothing in the app reads. A winit bump
/// re-checks.
#[test]
fn the_app_never_reads_winits_raw_keyboard() {
    let lock = include_str!("../../Cargo.lock");
    let winit = lock
        .split("[[package]]")
        .find(|p| p.contains("name = \"winit\""))
        .expect("winit in app/Cargo.lock");
    assert!(
        winit.contains("version = \"0.30."),
        "winit changed: check its raw keyboard use before bumping (#802): {winit}"
    );
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name.starts_with("follow_typing") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for banned in [
                "winit::event::DeviceEvent",
                "RawKeyEvent",
                "fn device_event",
                "listen_device_events",
                "DeviceEvents::",
            ] {
                if text.contains(banned) {
                    found.push(format!("{} uses {banned}", path.display()));
                }
            }
        }
    }
    assert!(found.is_empty(), "{found:?}");
}

/// With the setting on, a text field still takes typed text, keys and an IME's commit: egui
/// gets them as window events, which the listener never touches.
#[test]
fn text_and_ime_still_reach_egui_while_it_is_on() {
    let mut m = two_keyboards(true);
    type_on(&mut m, K5_IDS, FOLLOW_AFTER, Instant::now(), KEY);
    assert_eq!(focus(&m), K5, "on and following");
    let ctx = egui::Context::default();
    let mut text = String::new();
    let id = egui::Id::new("follow-typing-text");
    let frame = |events: Vec<egui::Event>, text: &mut String| {
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let r = ui.add(egui::TextEdit::singleline(text).id(id));
            r.request_focus();
        });
        out.textures_delta.clear();
    };
    frame(Vec::new(), &mut text);
    frame(vec![egui::Event::Text("hé".into())], &mut text);
    frame(
        vec![egui::Event::Key {
            key: egui::Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        &mut text,
    );
    frame(
        vec![egui::Event::Ime(egui::ImeEvent::Commit("日本".into()))],
        &mut text,
    );
    assert_eq!(text, "h日本");
}
