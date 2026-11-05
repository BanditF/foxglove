//! The keyboard's state as short facts for plugins and the panel (#17), mirrored from the
//! device events the firmware already sends; nothing polls:
//!
//! - `keyboard.connected` (Bool)
//! - `keyboard.layer` (Int, the highest active layer) and `keyboard.layers` (Bytes, all of them)
//! - `keyboard.right.battery` (Int, percent; `keyboard.<part>.battery` for any part reported)
//! - `keyboard.link` (Text, `usb` or `ble`), `keyboard.profile` (Int, 1-based Bluetooth profile)
//!   and `keyboard.profile_connected` (Bool)
//! - `keyboard.lighting.on` (Bool), `keyboard.lighting.color` (Text, `#rrggbb`: the chosen hue
//!   and saturation at full brightness), `keyboard.lighting.brightness` (Int, 0-100),
//!   `keyboard.lighting.effect` (Text, the effect's name, or its id when unknown) and
//!   `keyboard.lighting.rainbow` (Bool, the effect makes its own colours), from the keyboard's
//!   report of its lights (#40: the live feed gives them to the wallpaper)
//!
//! The same values stay under `device.<id>.*` for anything that needs a particular device. On a
//! disconnect everything but `keyboard.connected` goes, as it's unknown until the keyboard is
//! back; the link is read once when it reconnects, then follows the firmware's events.

use foxglove_devices::facts::{Facts, Value};
use foxglove_devices::{DeviceEvent, LightingState};

/// The backend of the keyboard with Foxglove's host link (see `devices::device`).
const HOST_KEYBOARD: &str = "zmk";

pub fn start() {
    // Supervised (#587): the first attempt gets the subscription made here, a restart its own.
    let events = super::services::First::new(super::devices::bus().subscribe(false));
    let _ = super::services::spawn_supervised("keyboard-facts", move || {
        let events = events.take_or(|| super::devices::bus().subscribe(false));
        let facts = super::devices::bus().facts();
        for e in events {
            if e.device.backend.0 != HOST_KEYBOARD {
                continue;
            }
            let connected = e.event == DeviceEvent::Connected;
            publish(facts, &e.event);
            if connected {
                // One read when it connects; the firmware reports changes after that.
                if let Some(info) = super::devices::device()
                    .and_then(|d| foxglove_devices::Device::power(&*d).and_then(|p| p.info().ok()))
                {
                    if let (Some(output), Some(profile), Some(connected)) =
                        (info.output, info.profile, info.profile_connected)
                    {
                        publish(
                            facts,
                            &DeviceEvent::Link {
                                output,
                                profile,
                                connected,
                            },
                        );
                    }
                }
            }
        }
    });
}

fn publish(facts: &Facts, event: &DeviceEvent) {
    match event {
        DeviceEvent::Connected => {
            facts.set("keyboard.connected", Value::Bool(true));
            // The Scenes page's map lights the keyboard now (#487).
            super::scene_room::changed();
        }
        DeviceEvent::Disconnected { .. } => {
            facts.remove_prefix("keyboard.", &["keyboard.connected"]);
            facts.set("keyboard.connected", Value::Bool(false));
            super::scene_room::changed();
        }
        DeviceEvent::LayerChanged { active } => {
            facts.set("keyboard.layers", Value::Bytes(active.clone()));
            facts.set(
                "keyboard.layer",
                Value::Int(active.iter().copied().max().unwrap_or(0).into()),
            );
        }
        DeviceEvent::Battery { part, percent } => facts.set(
            format!("keyboard.{part}.battery"),
            Value::Int((*percent).into()),
        ),
        DeviceEvent::Link {
            output,
            profile,
            connected,
        } => {
            facts.set("keyboard.link", Value::Text(output.to_ascii_lowercase()));
            facts.set("keyboard.profile", Value::Int(i64::from(*profile) + 1));
            facts.set("keyboard.profile_connected", Value::Bool(*connected));
        }
        // The keyboard's report of its lights (every RGB_STATE read or event, #605): the
        // case's "Match keyboard" follows it (#616). Its facts are `device.<id>.lighting.*`.
        DeviceEvent::LightingChanged { state } => {
            lighting(facts, state);
            super::pc_lights::keyboard_lights(state);
        }
        DeviceEvent::Key { .. } => {}
    }
}

/// The keyboard's lights as `keyboard.lighting.*`. Only what changed is sent on, so a new colour
/// is one change.
fn lighting(facts: &Facts, state: &LightingState) {
    use super::effect_controls::{spec, EffectClass};
    let [r, g, b] = foxglove_devices::calibration::hsv_to_rgb(
        f32::from(state.hue),
        f32::from(state.sat.min(100)) / 100.0,
        1.0,
    )
    .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
    let effect = spec(EffectClass::Underglow, &state.effect.id);
    facts.set("keyboard.lighting.on", Value::Bool(state.on));
    facts.set(
        "keyboard.lighting.color",
        Value::Text(format!("#{r:02x}{g:02x}{b:02x}")),
    );
    facts.set(
        "keyboard.lighting.brightness",
        Value::Int(state.brightness.min(100).into()),
    );
    facts.set(
        "keyboard.lighting.effect",
        Value::Text(effect.map_or_else(|| state.effect.id.clone(), |e| e.name.to_string())),
    );
    facts.set(
        "keyboard.lighting.rainbow",
        Value::Bool(effect.is_some_and(|e| e.rainbow())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_keyboard_facts_and_a_disconnect_clears_them() {
        let facts = Facts::default();
        publish(&facts, &DeviceEvent::Connected);
        publish(
            &facts,
            &DeviceEvent::LayerChanged {
                active: vec![0, 2, 3],
            },
        );
        publish(
            &facts,
            &DeviceEvent::Battery {
                part: "right".into(),
                percent: 61,
            },
        );
        publish(
            &facts,
            &DeviceEvent::Link {
                output: "BLE".into(),
                profile: 1,
                connected: true,
            },
        );
        assert_eq!(facts.get("keyboard.layer"), Some(Value::Int(3)));
        assert_eq!(
            facts.get("keyboard.layers"),
            Some(Value::Bytes(vec![0, 2, 3]))
        );
        assert_eq!(facts.get("keyboard.right.battery"), Some(Value::Int(61)));
        assert_eq!(facts.get("keyboard.link"), Some(Value::Text("ble".into())));
        assert_eq!(
            facts.get("keyboard.profile"),
            Some(Value::Int(2)),
            "1-based, as the app shows it"
        );
        publish(
            &facts,
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        assert_eq!(facts.get("keyboard.connected"), Some(Value::Bool(false)));
        assert_eq!(facts.get("keyboard.layer"), None);
        assert_eq!(facts.get("keyboard.right.battery"), None);
        assert_eq!(facts.get("keyboard.link"), None);
    }

    fn lights(hue: u16, sat: u8, effect: &str) -> LightingState {
        LightingState {
            on: true,
            hue,
            sat,
            brightness: 60,
            effect: foxglove_devices::Effect {
                id: effect.into(),
                params: Vec::new(),
            },
            speed: 2,
            settled: true,
        }
    }

    #[test]
    fn the_lights_become_facts_with_the_colour_at_full_brightness() {
        let facts = Facts::default();
        lighting(&facts, &lights(25, 100, "0"));
        let text = |k: &str| match facts.get(k) {
            Some(Value::Text(t)) => t,
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(text("keyboard.lighting.color"), "#ff6a00");
        assert_eq!(text("keyboard.lighting.effect"), "Solid");
        assert_eq!(facts.get("keyboard.lighting.on"), Some(Value::Bool(true)));
        assert_eq!(
            facts.get("keyboard.lighting.brightness"),
            Some(Value::Int(60))
        );
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(false))
        );

        // White, a rainbow effect, and an effect the table doesn't know.
        lighting(&facts, &lights(200, 0, "2"));
        assert_eq!(text("keyboard.lighting.color"), "#ffffff");
        assert_eq!(text("keyboard.lighting.effect"), "Spectrum");
        assert_eq!(
            facts.get("keyboard.lighting.rainbow"),
            Some(Value::Bool(true))
        );
        lighting(&facts, &lights(0, 100, "42"));
        assert_eq!(text("keyboard.lighting.color"), "#ff0000");
        assert_eq!(text("keyboard.lighting.effect"), "42");
    }

    #[test]
    fn a_new_colour_is_one_change_and_a_disconnect_clears_the_lights() {
        let facts = Facts::default();
        lighting(&facts, &lights(25, 100, "0"));
        let changes = facts.subscribe("keyboard.*");
        lighting(&facts, &lights(120, 100, "0"));
        let got: Vec<_> = changes.try_iter().map(|c| c.key).collect();
        assert_eq!(got, ["keyboard.lighting.color"]);
        publish(
            &facts,
            &DeviceEvent::Disconnected {
                reason: "unplugged".into(),
            },
        );
        assert_eq!(facts.get("keyboard.lighting.color"), None);
        assert_eq!(facts.get("keyboard.lighting.on"), None);
    }
}
