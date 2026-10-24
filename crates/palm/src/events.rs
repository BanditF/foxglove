use crate::{facts::Facts, DeviceId, LightingState, Value};
use std::sync::{
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};

/// A device change delivered to subscribers.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceEvent {
    Connected,
    Disconnected { reason: String },
    LayerChanged { active: Vec<u8> },
    Key { row: u16, col: u16, pressed: bool },
    LightingChanged { state: LightingState },
    Battery { part: String, percent: u8 },
}

/// A device identity paired with its event.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub device: DeviceId,
    pub event: DeviceEvent,
}

struct Subscriber {
    keys: bool,
    tx: Sender<Event>,
}

/// Shares device events and their corresponding facts.
#[derive(Clone, Default)]
pub struct EventBus {
    facts: Facts,
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

impl EventBus {
    pub fn new(facts: Facts) -> Self {
        Self {
            facts,
            ..Self::default()
        }
    }

    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// Key events require explicit opt-in. Events use unbounded queues.
    pub fn subscribe(&self, include_keys: bool) -> Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        self.subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Subscriber {
                keys: include_keys,
                tx,
            });
        rx
    }

    pub fn sink(&self, device: DeviceId) -> EventSink {
        EventSink {
            device,
            bus: self.clone(),
        }
    }
}

/// Publishes events and facts for one device.
#[derive(Clone)]
pub struct EventSink {
    device: DeviceId,
    bus: EventBus,
}

impl EventSink {
    pub fn device_id(&self) -> &DeviceId {
        &self.device
    }

    pub fn publish_fact(&self, name: &str, value: Value) {
        self.bus
            .facts
            .set(format!("device.{}.{name}", self.device), value);
    }

    pub fn remove_fact(&self, name: &str) {
        self.bus
            .facts
            .remove(&format!("device.{}.{name}", self.device));
    }

    pub fn emit(&self, event: DeviceEvent) {
        match &event {
            DeviceEvent::Connected => {
                self.remove_fact("disconnect_reason");
                self.publish_fact("connected", Value::Bool(true));
            }
            DeviceEvent::Disconnected { reason } => {
                // Layers, battery and lighting are unknown until the device is back.
                let prefix = format!("device.{}.", self.device);
                let keep = [format!("{prefix}connected"), format!("{prefix}disconnect_reason")];
                let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
                self.bus.facts.remove_prefix(&prefix, &keep);
                self.publish_fact("connected", Value::Bool(false));
                self.publish_fact("disconnect_reason", Value::Text(reason.clone()));
            }
            DeviceEvent::LayerChanged { active } => {
                self.publish_fact("layers", Value::Bytes(active.clone()))
            }
            DeviceEvent::Battery { part, percent } => {
                self.publish_fact(&format!("battery.{part}"), Value::Int((*percent).into()))
            }
            DeviceEvent::LightingChanged { state } => {
                self.publish_fact("lighting.on", Value::Bool(state.on));
                self.publish_fact("lighting.hue", Value::Int(state.hue.into()));
                self.publish_fact("lighting.sat", Value::Int(state.sat.into()));
                self.publish_fact("lighting.brightness", Value::Int(state.brightness.into()));
                self.publish_fact("lighting.effect", Value::Text(state.effect.id.clone()));
                self.publish_fact("lighting.speed", Value::Int(state.speed.into()));
            }
            // Keys never enter the facts store, where subscriptions have no key opt-in.
            DeviceEvent::Key { .. } => {}
        }
        let key = matches!(event, DeviceEvent::Key { .. });
        let event = Event {
            device: self.device.clone(),
            event,
        };
        self.bus
            .subscribers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| (key && !s.keys) || s.tx.send(event.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_require_opt_in_and_never_become_facts() {
        let bus = EventBus::default();
        let normal = bus.subscribe(false);
        let keys = bus.subscribe(true);
        let facts = bus.facts().subscribe("device.*");
        let id = DeviceId::new(crate::BackendId("mock".into()), 1, 2, None, "path");
        let sink = bus.sink(id.clone());
        sink.emit(DeviceEvent::Key {
            row: 0,
            col: 0,
            pressed: true,
        });
        assert!(normal.try_recv().is_err());
        assert!(facts.try_recv().is_err());
        assert_eq!(keys.recv().unwrap().device, id);
        sink.emit(DeviceEvent::Connected);
        assert_eq!(normal.recv().unwrap().event, DeviceEvent::Connected);
        assert_eq!(facts.recv().unwrap().key, format!("device.{id}.connected"));
    }

    #[test]
    fn disconnect_clears_stale_facts() {
        let bus = EventBus::default();
        let id = DeviceId::new(crate::BackendId("mock".into()), 1, 2, None, "path");
        let sink = bus.sink(id.clone());
        sink.emit(DeviceEvent::Connected);
        sink.emit(DeviceEvent::LayerChanged { active: vec![0, 2] });
        sink.emit(DeviceEvent::Battery { part: "right".into(), percent: 50 });
        sink.emit(DeviceEvent::Disconnected { reason: "unplugged".into() });
        let fact = |name: &str| bus.facts().get(&format!("device.{id}.{name}"));
        assert_eq!(fact("layers"), None);
        assert_eq!(fact("battery.right"), None);
        assert_eq!(fact("connected"), Some(Value::Bool(false)));
        assert_eq!(fact("disconnect_reason"), Some(Value::Text("unplugged".into())));
        sink.emit(DeviceEvent::Connected);
        assert_eq!(fact("disconnect_reason"), None);
    }
}
