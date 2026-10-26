//! The microphone broker (#237): Foxglove watches the default communications microphone and
//! publishes its state as facts, and mutes it when a plugin with `audio.inputs.mute` asks. The
//! plugin stays sandboxed; Foxglove makes the Core Audio calls.
//!
//! Facts: `mic.state` (`muted`, `unmuted`, `unavailable` or `unknown`), `mic.endpoint_id`,
//! `mic.endpoint_name`, `mic.error` (only with `unknown`), and `mic.app_state` = `unknown` (no
//! app reports its own mute yet). A failed read is never shown as muted. `FOXGLOVE_DISABLE=mic`
//! turns it off.

use foxglove_devices::facts::{Facts, Value};
use foxglove_panel::mic::{Mic, MicState};

fn publish(facts: &Facts, mic: &Mic) {
    let state = match &mic.state {
        MicState::Muted => "muted",
        MicState::Unmuted => "unmuted",
        MicState::Unavailable => "unavailable",
        MicState::Unknown(_) => "unknown",
    };
    facts.set("mic.state", Value::Text(state.into()));
    facts.set("mic.app_state", Value::Text("unknown".into()));
    let text = |key: &str, v: &str| {
        if v.is_empty() {
            facts.remove(key);
        } else {
            facts.set(key, Value::Text(v.into()));
        }
    };
    text("mic.endpoint_id", &mic.id);
    text("mic.endpoint_name", &mic.name);
    match &mic.state {
        MicState::Unknown(e) => text("mic.error", e),
        _ => text("mic.error", ""),
    }
}

pub fn start() {
    if super::repaint_trace::disabled("mic") {
        return;
    }
    let facts = super::devices::bus().facts().clone();
    foxglove_panel::mic::watch(move |mic| publish(&facts, &mic));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(f: &Facts, key: &str) -> Option<Value> {
        f.get(key)
    }

    #[test]
    fn states_become_facts_and_a_failure_is_never_muted() {
        let f = Facts::default();
        publish(
            &f,
            &Mic {
                state: MicState::Muted,
                id: "{0.0.1}".into(),
                name: "Headset".into(),
            },
        );
        assert_eq!(state(&f, "mic.state"), Some(Value::Text("muted".into())));
        assert_eq!(
            state(&f, "mic.endpoint_name"),
            Some(Value::Text("Headset".into()))
        );
        publish(
            &f,
            &Mic {
                state: MicState::Unknown("E_ACCESSDENIED".into()),
                id: "{0.0.1}".into(),
                name: "Headset".into(),
            },
        );
        assert_eq!(state(&f, "mic.state"), Some(Value::Text("unknown".into())));
        assert_eq!(
            state(&f, "mic.error"),
            Some(Value::Text("E_ACCESSDENIED".into()))
        );
        publish(
            &f,
            &Mic {
                state: MicState::Unavailable,
                id: String::new(),
                name: String::new(),
            },
        );
        assert_eq!(
            state(&f, "mic.state"),
            Some(Value::Text("unavailable".into()))
        );
        assert_eq!(state(&f, "mic.endpoint_id"), None, "no stale device");
        assert_eq!(state(&f, "mic.error"), None);
        assert_eq!(
            state(&f, "mic.app_state"),
            Some(Value::Text("unknown".into()))
        );
    }
}
