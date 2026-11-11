// SPDX-License-Identifier: MIT
//! The Hideout hub's host services (docs/hideout-contract.md §8.3, §8.10, §8.11, v1.12),
//! clean-room MIT: what the hub answers itself for modules, contained or not.
//!
//! - [`net::Broker`]: `net.open`, `net.send`, `net.close` and the events `net.data`,
//!   `net.accepted`, `net.closed`; streams per connection, capped, ended with the connection.
//! - [`mic::MicService`]: the `mic.*` facts and `audio.inputs.set_mute`, from Core Audio's
//!   callbacks on Windows.
//!
//! The router checks every call (params and grant) before it gets here; [`HostServices`] is
//! what the hub's [`hideout_router::server::Hub::set_services`] takes.

pub mod mic;
#[cfg(windows)]
mod mic_win;
pub mod net;

use std::sync::Arc;
use std::time::Duration;

use hideout_contract::services::{NET_METHODS, SET_MUTE};
use hideout_router::server::{Hub, Services, Sink};
use hideout_router::{ConnId, ServiceCall, ServiceError};
use serde_json::Value;

/// The hub's host services; either may be left out.
pub struct HostServices {
    pub net: Option<net::Broker>,
    pub mic: Option<mic::MicService>,
}

impl Services for HostServices {
    fn methods(&self) -> Vec<&'static str> {
        let mut m = Vec::new();
        if self.net.is_some() {
            m.extend_from_slice(NET_METHODS);
        }
        if self.mic.is_some() {
            m.push(SET_MUTE);
        }
        m
    }

    fn call(&self, call: ServiceCall, sink: &Sink) {
        match (call.method.as_str(), &self.net, &self.mic) {
            (m, Some(net), _) if NET_METHODS.contains(&m) => net.call(call, sink),
            (SET_MUTE, _, Some(mic)) => {
                let muted = call
                    .params
                    .get("muted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let (sink, n) = (sink.clone(), call.call);
                log::info!(
                    "mic: {} asks to {}",
                    call.caller,
                    if muted { "mute" } else { "unmute" }
                );
                mic.set_mute(
                    muted,
                    Box::new(move |r| {
                        sink.answer(n, r);
                    }),
                );
            }
            (m, _, _) => {
                sink.answer(
                    call.call,
                    Err(ServiceError::new(-32601, "", format!("{m}: not served"))),
                );
            }
        }
    }

    fn gone(&self, conn: ConnId) {
        if let Some(net) = &self.net {
            net.gone(conn);
        }
    }
}

/// Starts the host services on `hub`: the network broker always, the microphone unless
/// `mic` is false (lab hubs and tests leave the real microphone alone). The first `mic.*` facts
/// are published before this returns.
pub fn start(hub: &Hub, mic: bool) -> Arc<HostServices> {
    let mic = mic.then(|| start_mic(hub.sink()));
    let services = Arc::new(HostServices {
        net: Some(net::Broker::default()),
        mic,
    });
    hub.set_services(services.clone());
    services
}

#[cfg(windows)]
fn start_mic(sink: Sink) -> mic::MicService {
    mic::MicService::start(
        mic_win::CoreAudio::new,
        Box::new(move |k, v| sink.publish(k, v)),
        Duration::from_secs(5),
    )
}

#[cfg(not(windows))]
fn start_mic(sink: Sink) -> mic::MicService {
    struct None_;
    impl mic::Backend for None_ {
        fn bind_default(&mut self) -> Result<Option<mic::Endpoint>, String> {
            Ok(None)
        }
        fn muted(&mut self) -> Result<bool, String> {
            Err("no microphone control on this system".into())
        }
        fn set_mute(&mut self, _: bool) -> Result<(), mic::MuteError> {
            Err(mic::MuteError::Unavailable)
        }
    }
    mic::MicService::start(
        |_| Ok(None_),
        Box::new(move |k, v| sink.publish(k, v)),
        Duration::from_secs(5),
    )
}
