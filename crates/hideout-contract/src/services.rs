// SPDX-License-Identifier: MIT
//! The hub's host services (§8.10, §8.11, spec v1.12): the network broker's messages and
//! limits, and the microphone's call and facts. The wire is Foxglove API 1.2 and 1.3's, as the
//! SDK speaks it; these types are the contract's own copy of that shape.

use serde::{Deserialize, Serialize};

/// The methods the hub answers itself as host services (§8.3, v1.12).
pub const NET_METHODS: &[&str] = &["net.open", "net.send", "net.close"];

/// The microphone's call (§8.11).
pub const SET_MUTE: &str = "audio.inputs.set_mute";

/// Open streams one connection may hold: listeners, accepted connections and opens still
/// connecting included (§8.10).
pub const NET_STREAMS_PER_CONNECTION: usize = 16;

/// Open streams in the whole hub (§8.10).
pub const NET_STREAMS_PER_HUB: usize = 256;

/// The most bytes one `net.data` carries for a `tcp` stream (§8.10).
pub const NET_READ_CHUNK: usize = 16 * 1024;

/// The largest datagram a `udp` stream receives (§8.10).
pub const NET_DATAGRAM: usize = 64 * 1024;

/// How long one connect attempt and one write may take, as fallbacks only (§8.10).
pub const NET_TIMEOUT_S: u64 = 10;

/// The most one `net.open` may take, name lookup and every connect attempt included: well
/// under the call's 30 s (§8.1), so the caller always gets the answer (§8.10, #1107 review).
pub const NET_OPEN_DEADLINE_S: u64 = 20;

/// When more than this many bytes wait to be written to a connection, a stream whose event
/// can't be queued ends (§8.10).
pub const NET_BACKLOG_BYTES: usize = 8 * 1024 * 1024;

/// The only host a `udp` or `tcp-listen` stream may use (§8.10).
pub const LOOPBACK: &str = "127.0.0.1";

/// The kind of stream `net.open` makes (§8.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum NetKind {
    /// A TCP connection to `host`:`port`.
    #[serde(rename = "tcp")]
    Tcp,
    /// Datagrams sent to 127.0.0.1:`port`, received only.
    #[serde(rename = "udp")]
    Udp,
    /// Accepts TCP connections on 127.0.0.1:`port`.
    #[serde(rename = "tcp-listen")]
    TcpListen,
}

/// `net.open` (C → H).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NetOpen {
    pub kind: NetKind,
    /// For `udp` and `tcp-listen`, only `127.0.0.1`.
    pub host: String,
    pub port: u16,
}

/// `net.open`'s result, and `net.close`'s params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NetStream {
    pub stream: u64,
}

/// `net.send` (C → H): bytes, base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NetSend {
    pub stream: u64,
    pub data: String,
}

/// `net.data` (H → C): bytes that arrived, base64; `from` is the sender for `udp`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NetData {
    pub stream: u64,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// `net.accepted` (H → C): a listener accepted a connection, the new `stream`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NetAccepted {
    pub listener: u64,
    pub stream: u64,
    pub from: String,
}

/// `net.closed` (H → C): a stream ended other than by `net.close`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NetClosed {
    pub stream: u64,
    pub reason: String,
}

/// `audio.inputs.set_mute` (C → H, §8.11).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SetMute {
    pub muted: bool,
}

/// `mic.state`'s values (§8.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MicState {
    Muted,
    Unmuted,
    /// No active capture endpoint.
    Unavailable,
    /// It couldn't be read; never shown as muted.
    Unknown,
}

impl MicState {
    pub fn as_str(self) -> &'static str {
        match self {
            MicState::Muted => "muted",
            MicState::Unmuted => "unmuted",
            MicState::Unavailable => "unavailable",
            MicState::Unknown => "unknown",
        }
    }
}

/// The facts the microphone publishes (§8.11).
pub const MIC_FACTS: &[&str] = &[
    "mic.state",
    "mic.endpoint_id",
    "mic.endpoint_name",
    "mic.error",
];

/// The app watch's call (§8.12, v1.13): a module asks to be told when an app opens and closes.
pub const APPS_WATCH: &str = "apps.watch";

/// The app watch's events (§8.12): the app opened, and the app closed.
pub const APP_WINDOW_APPEARED: &str = "app.window.appeared";
pub const APP_WINDOW_CLOSED: &str = "app.window.closed";

/// `apps.watch` (M → H, §8.12): the app's exe file name, such as `obs64.exe`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AppsWatch {
    pub exe: String,
}

/// `apps.watch`'s result and the `app.window.appeared` and `app.window.closed` events
/// (§8.12): `exe` as asked (ASCII lower case) and `open`, 1 while the app runs and 0 while it
/// doesn't. Foxglove API 1.3's shape, where `open` counts windows; the hub's watch follows
/// processes, so 0 means the same in both: the app isn't open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AppWindows {
    pub exe: String,
    pub open: u32,
}

/// The `data.id`s a host service's refusal carries (§15).
pub mod error_id {
    /// (v1.13) `apps.watch` without `apps.watch:<name>.exe` for that app.
    pub const APPS_DENIED: &str = "apps.denied";
    /// (v1.13) `apps.watch` with an `exe` that isn't `<name>.exe`.
    pub const APPS_ARGUMENT: &str = "apps.argument";
    pub const NET_DENIED: &str = "net.denied";
    pub const NET_ARGUMENT: &str = "net.argument";
    pub const NET_STREAM: &str = "net.stream";
    pub const NET_IO: &str = "net.io";
    pub const NET_LIMIT: &str = "net.limit";
    pub const AUDIO_DENIED: &str = "audio.denied";
    pub const AUDIO_UNAVAILABLE: &str = "audio.unavailable";
    pub const AUDIO_BACKEND: &str = "audio.backend";
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_wire_is_foxglove_api_1_3s() {
        let open: NetOpen =
            serde_json::from_value(json!({"kind": "tcp-listen", "host": "127.0.0.1", "port": 9}))
                .unwrap();
        assert_eq!(open.kind, NetKind::TcpListen);
        assert_eq!(
            serde_json::to_value(NetData {
                stream: 1,
                data: "aGk=".into(),
                from: None
            })
            .unwrap(),
            json!({"stream": 1, "data": "aGk="})
        );
        assert!(
            serde_json::from_value::<NetOpen>(json!({"kind": "icmp", "host": "a", "port": 1}))
                .is_err()
        );
        assert_eq!(MicState::Unavailable.as_str(), "unavailable");
        assert_eq!(
            serde_json::to_value(MicState::Muted).unwrap(),
            json!("muted")
        );
    }
}
