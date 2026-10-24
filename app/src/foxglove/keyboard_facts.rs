//! The keyboard's state as short facts for plugins and the panel (#17), mirrored from the
//! device events the firmware already sends; nothing polls:
//!
//! - `keyboard.connected` (Bool)
//! - `keyboard.layer` (Int, the highest active layer) and `keyboard.layers` (Bytes, all of them)
//! - `keyboard.right.battery` (Int, percent; `keyboard.<part>.battery` for any part reported)
//! - `keyboard.link` (Text, `usb` or `ble`), `keyboard.profile` (Int, 1-based Bluetooth profile)
//!   and `keyboard.profile_connected` (Bool)
//!
//! The same values stay under `device.<id>.*` for anything that needs a particular device. On a
//! disconnect everything but `keyboard.connected` goes, as it's unknown until the keyboard is
//! back; the link is read once when it reconnects, then follows the firmware's events.

use foxglove_devices::facts::{Facts, Value};
use foxglove_devices::DeviceEvent;

/// The backend of the keyboard with Foxglove's host link (see `devices::device`).
const HOST_KEYBOARD: &str = "zmk";

pub fn start() {
    let events = super::devices::bus().subscribe(false);
    super::services::spawn_named("keyboard-facts", move || {
        let facts = super::devices::bus().facts();
        for e in events {
            if e.device.backend.0 != HOST_KEYBOARD {
                continue;
            }
            let connected = e.event == DeviceEvent::Connected;
            publish(facts, &e.event);
            if connected {
                // One read when it connects; the firmware reports changes after that.
                if let Some(info) = super::devices::device().and_then(|d| foxglove_devices::Device::power(&*d).and_then(|p| p.info().ok())) {
                    if let (Some(output), Some(profile), Some(connected)) = (info.output, info.profile, info.profile_connected) {
                        publish(facts, &DeviceEvent::Link { output, profile, connected });
                    }
                }
            }
        }
    });
}

fn publish(facts: &Facts, event: &DeviceEvent) {
    match event {
        DeviceEvent::Connected => facts.set("keyboard.connected", Value::Bool(true)),
        DeviceEvent::Disconnected { .. } => {
            facts.remove_prefix("keyboard.", &["keyboard.connected"]);
            facts.set("keyboard.connected", Value::Bool(false));
        }
        DeviceEvent::LayerChanged { active } => {
            facts.set("keyboard.layers", Value::Bytes(active.clone()));
            facts.set("keyboard.layer", Value::Int(active.iter().copied().max().unwrap_or(0).into()));
        }
        DeviceEvent::Battery { part, percent } => facts.set(format!("keyboard.{part}.battery"), Value::Int((*percent).into())),
        DeviceEvent::Link { output, profile, connected } => {
            facts.set("keyboard.link", Value::Text(output.to_ascii_lowercase()));
            facts.set("keyboard.profile", Value::Int(i64::from(*profile) + 1));
            facts.set("keyboard.profile_connected", Value::Bool(*connected));
        }
        DeviceEvent::Key { .. } | DeviceEvent::LightingChanged { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_keyboard_facts_and_a_disconnect_clears_them() {
        let facts = Facts::default();
        publish(&facts, &DeviceEvent::Connected);
        publish(&facts, &DeviceEvent::LayerChanged { active: vec![0, 2, 3] });
        publish(&facts, &DeviceEvent::Battery { part: "right".into(), percent: 61 });
        publish(&facts, &DeviceEvent::Link { output: "BLE".into(), profile: 1, connected: true });
        assert_eq!(facts.get("keyboard.layer"), Some(Value::Int(3)));
        assert_eq!(facts.get("keyboard.layers"), Some(Value::Bytes(vec![0, 2, 3])));
        assert_eq!(facts.get("keyboard.right.battery"), Some(Value::Int(61)));
        assert_eq!(facts.get("keyboard.link"), Some(Value::Text("ble".into())));
        assert_eq!(facts.get("keyboard.profile"), Some(Value::Int(2)), "1-based, as the app shows it");
        publish(&facts, &DeviceEvent::Disconnected { reason: "unplugged".into() });
        assert_eq!(facts.get("keyboard.connected"), Some(Value::Bool(false)));
        assert_eq!(facts.get("keyboard.layer"), None);
        assert_eq!(facts.get("keyboard.right.battery"), None);
        assert_eq!(facts.get("keyboard.link"), None);
    }
}
