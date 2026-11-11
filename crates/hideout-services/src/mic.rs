// SPDX-License-Identifier: MIT
//! The microphone (contract §8.11, v1.12): the default communications microphone's endpoint
//! mute as the hub facts `mic.state`, `mic.endpoint_id`, `mic.endpoint_name` and `mic.error`,
//! and `audio.inputs.set_mute`.
//!
//! [`Mic`] is the logic, over a [`Backend`] (Core Audio on Windows, a fake in tests). It runs on
//! one thread of its own that sleeps on a channel: Core Audio's callbacks
//! (`IMMNotificationClient`, `IAudioEndpointVolumeCallback::OnNotify`) and the calls put
//! [`Event`]s on it. Never polled. The hub's own `SetMute` carries its event context, a GUID drawn fresh for each run
//! (#1107 review: a fixed one would let any process hide its change); its
//! `OnNotify` echo is skipped, since the hub published the new state when the call succeeded.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use hideout_contract::services::{error_id, MicState};
use hideout_router::ServiceError;
use serde_json::{json, Value};

/// A fresh event context for this run: 128 random bits (the hub's own `SetMute` carries it,
/// so its `OnNotify` echo is known; nobody else can guess it).
pub fn fresh_context() -> u128 {
    u128::from_str_radix(&hideout_router::fresh_secret(), 16).unwrap_or(1)
}

/// The default communications microphone, as the backend found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
}

/// Why a mute change didn't happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MuteError {
    /// No default communications microphone.
    Unavailable,
    /// Core Audio refused, or can't be reached.
    Backend(String),
}

/// What [`Mic`] needs from Core Audio. Every method runs on the microphone's thread.
pub trait Backend {
    /// Finds the default communications microphone again and moves the endpoint-volume callback
    /// to it (so `OnNotify` follows the new default). `Ok(None)`: there is none.
    fn bind_default(&mut self) -> Result<Option<Endpoint>, String>;
    /// The bound endpoint's mute.
    fn muted(&mut self) -> Result<bool, String>;
    /// Sets the bound endpoint's mute, with `context` as the event context.
    fn set_mute(&mut self, muted: bool, context: u128) -> Result<(), MuteError>;
}

/// The answer to a `set_mute`, run on the microphone's thread.
pub type Reply = Box<dyn FnOnce(Result<Value, ServiceError>) + Send>;

/// What wakes the microphone's thread.
pub enum Event {
    /// A device was added, removed or changed state, or the default capture endpoint for
    /// `eCommunications` changed.
    Devices,
    /// `OnNotify` on the bound endpoint: its mute now, and the change's event context.
    Volume { muted: bool, context: u128 },
    /// `audio.inputs.set_mute`, already permitted.
    SetMute { muted: bool, reply: Reply },
}

/// The facts as last published.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Facts {
    state: Option<MicState>,
    id: Option<String>,
    name: Option<String>,
    error: Option<String>,
}

impl Facts {
    fn entries(&self) -> [(&'static str, Value); 4] {
        let s = |v: &Option<String>| v.clone().map_or(Value::Null, Value::from);
        [
            (
                "mic.state",
                self.state.map_or(Value::Null, |m| json!(m.as_str())),
            ),
            ("mic.endpoint_id", s(&self.id)),
            ("mic.endpoint_name", s(&self.name)),
            ("mic.error", s(&self.error)),
        ]
    }
}

/// Publishes one hub fact (`null` removes it).
pub type Publish = Box<dyn FnMut(&str, Value) + Send>;

/// The microphone's logic.
pub struct Mic<B: Backend> {
    backend: B,
    publish: Publish,
    shown: Facts,
    /// This run's event context (see [`fresh_context`]).
    context: u128,
}

impl<B: Backend> Mic<B> {
    pub fn new(backend: B, publish: Publish, context: u128) -> Mic<B> {
        Mic {
            backend,
            publish,
            shown: Facts::default(),
            context,
        }
    }

    /// Publishes the facts that differ from what was shown.
    fn show(&mut self, now: Facts) {
        let before = self.shown.entries();
        for ((key, value), (_, was)) in now.entries().into_iter().zip(before) {
            if value != was {
                (self.publish)(key, value);
            }
        }
        self.shown = now;
    }

    /// Reads the default microphone again (at start and after a device event).
    pub fn refresh(&mut self) {
        let now = match self.backend.bind_default() {
            Ok(None) => Facts {
                state: Some(MicState::Unavailable),
                ..Facts::default()
            },
            Ok(Some(ep)) => {
                let (state, error) = match self.backend.muted() {
                    Ok(true) => (MicState::Muted, None),
                    Ok(false) => (MicState::Unmuted, None),
                    Err(e) => (MicState::Unknown, Some(e)),
                };
                Facts {
                    state: Some(state),
                    id: Some(ep.id),
                    name: Some(ep.name),
                    error,
                }
            }
            Err(e) => Facts {
                state: Some(MicState::Unknown),
                error: Some(e),
                ..Facts::default()
            },
        };
        self.show(now);
    }

    fn set_state(&mut self, muted: bool) {
        let now = Facts {
            state: Some(if muted {
                MicState::Muted
            } else {
                MicState::Unmuted
            }),
            error: None,
            ..self.shown.clone()
        };
        self.show(now);
    }

    pub fn handle(&mut self, event: Event) {
        match event {
            Event::Devices => self.refresh(),
            // The hub's own change: published when `SetMute` succeeded.
            Event::Volume { context, .. } if context == self.context => {}
            Event::Volume { muted, .. } => self.set_state(muted),
            Event::SetMute { muted, reply } => {
                let result = match self.backend.set_mute(muted, self.context) {
                    Ok(()) => {
                        self.set_state(muted);
                        Ok(json!({}))
                    }
                    Err(MuteError::Unavailable) => Err(ServiceError::new(
                        6,
                        error_id::AUDIO_UNAVAILABLE,
                        "There's no microphone to mute.",
                    )),
                    Err(MuteError::Backend(e)) => Err(ServiceError::new(
                        6,
                        error_id::AUDIO_BACKEND,
                        format!("Windows said no: {e}"),
                    )),
                };
                reply(result);
            }
        }
    }

    /// Runs until every sender is gone. Device events that pile up are read as one.
    pub fn run(mut self, events: Receiver<Event>) {
        while let Ok(e) = events.recv() {
            let mut devices = matches!(e, Event::Devices);
            if !devices {
                self.handle(e);
            }
            // Drain what's already queued (no waiting): one refresh for a burst of device events.
            while let Ok(more) = events.try_recv() {
                match more {
                    Event::Devices => devices = true,
                    other => self.handle(other),
                }
            }
            if devices {
                self.refresh();
            }
        }
    }
}

/// A running microphone service: where its events go.
#[derive(Clone)]
pub struct MicService {
    events: Sender<Event>,
    context: u128,
}

impl MicService {
    /// This run's event context: the hub's own changes carry it.
    pub fn context(&self) -> u128 {
        self.context
    }

    /// Asks for a mute change; `reply` runs on the microphone's thread.
    pub fn set_mute(&self, muted: bool, reply: Reply) {
        if let Err(mpsc::SendError(Event::SetMute { reply, .. })) =
            self.events.send(Event::SetMute { muted, reply })
        {
            reply(Err(ServiceError::new(
                6,
                error_id::AUDIO_BACKEND,
                "the microphone service isn't running",
            )));
        }
    }

    /// Starts the service on a thread of its own: `make` builds the backend there (Core Audio
    /// lives on that thread), given where its callbacks send events. The first facts are
    /// published before this returns (or after `first` at most, as a fallback), so they are
    /// there before any module starts (§8.11).
    pub fn start<B, F>(make: F, mut publish: Publish, first: Duration) -> MicService
    where
        B: Backend,
        F: FnOnce(Sender<Event>) -> Result<B, String> + Send + 'static,
    {
        let context = fresh_context();
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel::<()>();
        let callbacks = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("hideout-mic".into())
            .spawn(move || {
                let backend = match make(callbacks) {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("mic: Core Audio not reached: {e}");
                        publish("mic.state", json!(MicState::Unknown.as_str()));
                        publish("mic.error", json!(e));
                        let _ = ready_tx.send(());
                        // Calls are still answered, with audio.backend.
                        for ev in rx {
                            if let Event::SetMute { reply, .. } = ev {
                                reply(Err(ServiceError::new(
                                    6,
                                    error_id::AUDIO_BACKEND,
                                    e.clone(),
                                )));
                            }
                        }
                        return;
                    }
                };
                let mut mic = Mic::new(backend, publish, context);
                mic.refresh();
                let _ = ready_tx.send(());
                mic.run(rx);
            });
        match spawned {
            Ok(_) => {
                if ready.recv_timeout(first).is_err() {
                    log::warn!("mic: the first read took over {first:?}; going on without it");
                }
            }
            Err(e) => log::warn!("mic: no thread: {e}"),
        }
        MicService {
            events: tx,
            context,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A fake Core Audio: a list of endpoints with their mutes. Never touches a real device.
    #[derive(Default)]
    struct Fake {
        default: Option<(Endpoint, Result<bool, String>)>,
        bind_fails: Option<String>,
        refuse: Option<String>,
        bound: Option<String>,
        binds: usize,
        set_calls: Vec<bool>,
    }

    #[derive(Clone)]
    struct Shared(Arc<Mutex<Fake>>);

    impl Backend for Shared {
        fn bind_default(&mut self) -> Result<Option<Endpoint>, String> {
            let mut f = self.0.lock().unwrap();
            f.binds += 1;
            if let Some(e) = &f.bind_fails {
                return Err(e.clone());
            }
            f.bound = f.default.as_ref().map(|d| d.0.id.clone());
            Ok(f.default.as_ref().map(|d| d.0.clone()))
        }
        fn muted(&mut self) -> Result<bool, String> {
            let f = self.0.lock().unwrap();
            f.default
                .as_ref()
                .map(|d| d.1.clone())
                .unwrap_or(Err("none bound".into()))
        }
        fn set_mute(&mut self, muted: bool, context: u128) -> Result<(), MuteError> {
            assert_ne!(context, 0, "an event context");
            let mut f = self.0.lock().unwrap();
            f.set_calls.push(muted);
            if let Some(e) = &f.refuse {
                return Err(MuteError::Backend(e.clone()));
            }
            match &mut f.default {
                None => Err(MuteError::Unavailable),
                Some(d) => {
                    d.1 = Ok(muted);
                    Ok(())
                }
            }
        }
    }

    const CTX: u128 = 0x1234;

    fn ep(id: &str) -> Endpoint {
        Endpoint {
            id: id.into(),
            name: format!("Microphone {id}"),
        }
    }

    type Log = Arc<Mutex<Vec<(String, Value)>>>;

    fn mic(fake: Fake) -> (Mic<Shared>, Shared, Log) {
        let shared = Shared(Arc::new(Mutex::new(fake)));
        let log: Log = Arc::default();
        let l = log.clone();
        let m = Mic::new(
            shared.clone(),
            Box::new(move |k, v| l.lock().unwrap().push((k.to_string(), v))),
            CTX,
        );
        (m, shared, log)
    }

    fn take(log: &Log) -> Vec<(String, Value)> {
        std::mem::take(&mut *log.lock().unwrap())
    }

    fn reply_to(slot: &Arc<Mutex<Option<Result<Value, ServiceError>>>>) -> Reply {
        let slot = slot.clone();
        Box::new(move |r| *slot.lock().unwrap() = Some(r))
    }

    #[test]
    fn start_publishes_the_default_microphone() {
        let (mut m, _, log) = mic(Fake {
            default: Some((ep("A"), Ok(true))),
            ..Fake::default()
        });
        m.refresh();
        assert_eq!(
            take(&log),
            [
                ("mic.state".into(), json!("muted")),
                ("mic.endpoint_id".into(), json!("A")),
                ("mic.endpoint_name".into(), json!("Microphone A")),
            ]
        );
        // Nothing changed: nothing published.
        m.handle(Event::Devices);
        assert!(take(&log).is_empty());
    }

    #[test]
    fn no_microphone_is_unavailable_and_a_failed_read_is_unknown_never_muted() {
        let (mut m, fake, log) = mic(Fake::default());
        m.refresh();
        assert_eq!(take(&log), [("mic.state".into(), json!("unavailable"))]);
        fake.0.lock().unwrap().default = Some((ep("A"), Err("read failed".into())));
        m.handle(Event::Devices);
        assert_eq!(
            take(&log),
            [
                ("mic.state".into(), json!("unknown")),
                ("mic.endpoint_id".into(), json!("A")),
                ("mic.endpoint_name".into(), json!("Microphone A")),
                ("mic.error".into(), json!("read failed")),
            ]
        );
        fake.0.lock().unwrap().bind_fails = Some("no Core Audio".into());
        m.handle(Event::Devices);
        assert_eq!(
            take(&log),
            [
                ("mic.endpoint_id".into(), Value::Null),
                ("mic.endpoint_name".into(), Value::Null),
                ("mic.error".into(), json!("no Core Audio")),
            ]
        );
    }

    #[test]
    fn a_new_default_device_is_followed() {
        let (mut m, fake, log) = mic(Fake {
            default: Some((ep("A"), Ok(false))),
            ..Fake::default()
        });
        m.refresh();
        take(&log);
        fake.0.lock().unwrap().default = Some((ep("B"), Ok(true)));
        m.handle(Event::Devices);
        assert_eq!(fake.0.lock().unwrap().bound.as_deref(), Some("B"));
        assert_eq!(
            take(&log),
            [
                ("mic.state".into(), json!("muted")),
                ("mic.endpoint_id".into(), json!("B")),
                ("mic.endpoint_name".into(), json!("Microphone B")),
            ]
        );
        // Unplugged: unavailable, and the endpoint facts go.
        fake.0.lock().unwrap().default = None;
        m.handle(Event::Devices);
        assert_eq!(
            take(&log),
            [
                ("mic.state".into(), json!("unavailable")),
                ("mic.endpoint_id".into(), Value::Null),
                ("mic.endpoint_name".into(), Value::Null),
            ]
        );
    }

    #[test]
    fn someone_elses_mute_arrives_and_the_hubs_own_echo_is_skipped() {
        let (mut m, fake, log) = mic(Fake {
            default: Some((ep("A"), Ok(false))),
            ..Fake::default()
        });
        m.refresh();
        take(&log);
        // Windows' own mute (another context).
        m.handle(Event::Volume {
            muted: true,
            context: 7,
        });
        assert_eq!(take(&log), [("mic.state".into(), json!("muted"))]);
        // The hub's set_mute: published at once, the echo skipped.
        let got = Arc::new(Mutex::new(None));
        m.handle(Event::SetMute {
            muted: false,
            reply: reply_to(&got),
        });
        assert_eq!(got.lock().unwrap().take(), Some(Ok(json!({}))));
        assert_eq!(take(&log), [("mic.state".into(), json!("unmuted"))]);
        m.handle(Event::Volume {
            muted: false,
            context: CTX,
        });
        assert!(take(&log).is_empty());
        assert_eq!(fake.0.lock().unwrap().set_calls, [false]);
    }

    #[test]
    fn set_mute_refusals_carry_their_ids() {
        let (mut m, fake, log) = mic(Fake::default());
        m.refresh();
        take(&log);
        let got = Arc::new(Mutex::new(None));
        m.handle(Event::SetMute {
            muted: true,
            reply: reply_to(&got),
        });
        let e = got.lock().unwrap().take().unwrap().unwrap_err();
        assert_eq!((e.code, e.id), (6, error_id::AUDIO_UNAVAILABLE));
        fake.0.lock().unwrap().default = Some((ep("A"), Ok(false)));
        fake.0.lock().unwrap().refuse = Some("E_ACCESSDENIED".into());
        m.handle(Event::SetMute {
            muted: true,
            reply: reply_to(&got),
        });
        let e = got.lock().unwrap().take().unwrap().unwrap_err();
        assert_eq!((e.code, e.id), (6, error_id::AUDIO_BACKEND));
        // A refused change publishes nothing.
        assert!(take(&log).is_empty());
    }

    #[test]
    fn the_service_publishes_before_start_returns_and_answers_calls() {
        let fake = Shared(Arc::new(Mutex::new(Fake {
            default: Some((ep("A"), Ok(false))),
            ..Fake::default()
        })));
        let log: Log = Arc::default();
        let l = log.clone();
        let f = fake.clone();
        let svc = MicService::start(
            move |_events| Ok(f),
            Box::new(move |k, v| l.lock().unwrap().push((k.to_string(), v))),
            Duration::from_secs(10),
        );
        assert_eq!(
            log.lock().unwrap()[0],
            ("mic.state".into(), json!("unmuted"))
        );
        let (tx, rx) = mpsc::channel();
        svc.set_mute(
            true,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Ok(json!({}))
        );
        assert_eq!(fake.0.lock().unwrap().set_calls, [true]);
    }

    #[test]
    fn without_core_audio_the_state_is_unknown_and_calls_are_refused() {
        let log: Log = Arc::default();
        let l = log.clone();
        let svc = MicService::start::<Shared, _>(
            |_| Err("CoCreateInstance failed".into()),
            Box::new(move |k, v| l.lock().unwrap().push((k.to_string(), v))),
            Duration::from_secs(10),
        );
        assert_eq!(
            *log.lock().unwrap(),
            [
                ("mic.state".to_string(), json!("unknown")),
                ("mic.error".to_string(), json!("CoCreateInstance failed")),
            ]
        );
        let (tx, rx) = mpsc::channel();
        svc.set_mute(
            false,
            Box::new(move |r| {
                let _ = tx.send(r);
            }),
        );
        let e = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap_err();
        assert_eq!(e.id, error_id::AUDIO_BACKEND);
    }
}
