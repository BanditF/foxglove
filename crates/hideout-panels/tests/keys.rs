//! Set key (#991), window-free: a Keys row captures the next key press, binds it, refuses a
//! key another action has or one that isn't bindable, and Remove unbinds; each change shows
//! once in `take_keys_changed` for the hub to save and register.
use egui::{Key, Modifiers};
use hideout_contract::module::{Context, Module};
use hideout_panels::keys::{taken_here, Chord, Press, NEEDS_MODIFIER};
use hideout_panels::PanelHost;

fn manifest(id: &str) -> Module {
    let text = format!(
        r#"
id = "{id}"
name = "Name {id}"
version = "1.0.0"
kind = "service"
family = "foxglove"
hub = "1.1"
transport = "stdio"
api = {{ foxglove = "1.2" }}
run = ["x.exe"]
description = "An example."

[[actions]]
id = "toggle"
name = "Toggle it"

[[actions]]
id = "other"
name = "Other thing"
"#
    );
    Module::parse(&text, &Context::default()).expect("manifest parses")
}

fn ctrl_alt() -> Modifiers {
    Modifiers {
        ctrl: true,
        alt: true,
        command: true,
        ..Modifiers::NONE
    }
}

#[test]
fn set_key_binds_the_next_press_and_remove_unbinds() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.a"));
    host.start_capture("m.a", "toggle");
    assert_eq!(host.capturing(), Some(("m.a", "toggle")));
    // A plain letter is refused with the rule, and the row keeps waiting.
    host.capture_press(Chord::press(Key::M, Modifiers::NONE));
    assert_eq!(host.key_note("m.a", "toggle"), Some(NEEDS_MODIFIER));
    assert!(host.capturing().is_some());
    host.capture_press(Chord::press(Key::M, ctrl_alt()));
    assert_eq!(host.capturing(), None);
    assert_eq!(host.key("m.a", "toggle"), Some("Ctrl+Alt+M"));
    assert_eq!(host.key_note("m.a", "toggle"), None);
    assert_eq!(
        host.take_keys_changed(),
        Some(vec![("m.a".into(), "toggle".into(), "Ctrl+Alt+M".into())])
    );
    assert_eq!(host.take_keys_changed(), None, "reported once");
    host.unbind_key("m.a", "toggle");
    assert_eq!(host.key("m.a", "toggle"), None);
    assert_eq!(host.take_keys_changed(), Some(vec![]));
}

#[test]
fn a_key_another_action_has_is_refused_and_esc_cancels() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.a"));
    host.start_capture("m.a", "toggle");
    host.capture_press(Chord::press(Key::F13, Modifiers::NONE));
    let _ = host.take_keys_changed();
    host.start_capture("m.a", "other");
    host.capture_press(Chord::press(Key::F13, Modifiers::NONE));
    assert_eq!(
        host.key_note("m.a", "other").map(String::from),
        Some(taken_here("F13", "Toggle it", "Name m.a"))
    );
    assert_eq!(host.key("m.a", "other"), None);
    assert_eq!(host.take_keys_changed(), None);
    host.capture_press(Press::Cancel);
    assert_eq!(host.capturing(), None);
    // Binding the same key to the same action again is no conflict.
    host.start_capture("m.a", "toggle");
    host.capture_press(Chord::press(Key::F13, Modifiers::NONE));
    assert_eq!(host.key("m.a", "toggle"), Some("F13"));
}

#[test]
fn a_removed_module_takes_its_keys_with_it() {
    let mut host = PanelHost::default();
    host.set_manifest(manifest("m.a"));
    host.set_key("m.a", "toggle", Some("F14".into()));
    assert_eq!(host.take_keys_changed(), None, "loading keys is no change");
    host.module_removed("m.a");
    assert_eq!(host.take_keys_changed(), Some(vec![]));
}
