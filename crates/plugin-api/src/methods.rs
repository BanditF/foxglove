//! Every method and event of API v1, with its params, result and the permission it needs.
//! [`METHODS`] is the table the app's dispatcher and the schema both come from.

use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::permission::Permission;

/// A fact's value. Bytes travel as base64 under `{"base64": ...}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Fact {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes { base64: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// The first call on every connection; nothing else is accepted before it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InitializeParams {
    /// The API version the client was built for, `major.minor`.
    pub api: String,
    pub client: ClientInfo,
    /// What an outside tool asks for; the user approves it once per tool. Plugins get what their
    /// manifest declares and leave this empty.
    #[serde(default)]
    pub permissions: Vec<Permission>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InitializeResult {
    /// The API version the app speaks.
    pub api: String,
    /// The app's own version.
    pub app: String,
    /// What this connection may do.
    pub granted: Vec<Permission>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyParams {
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FactResult {
    pub value: Option<Fact>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetFactParams {
    pub key: String,
    /// `null` removes the fact.
    pub value: Option<Fact>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubscribeParams {
    /// An exact key, `prefix.*`, or `*`.
    pub pattern: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Subscription {
    pub subscription: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FactEntry {
    pub key: String,
    pub value: Fact,
}

/// The facts matching a pattern, sorted by key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FactList {
    pub facts: Vec<FactEntry>,
}

/// Event: a subscribed fact changed (`value` null = removed). Only future changes are sent;
/// read the current value with `facts.get`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FactChanged {
    pub subscription: u64,
    pub key: String,
    pub value: Option<Fact>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Device {
    /// Stable across reconnects; facts about it live under `device.<id>.*`.
    pub id: String,
    pub name: String,
    pub kind: String,
    pub backend: String,
    pub connected: bool,
    /// device capabilities: keymap, layers, behaviors, lighting, color_in, screens, power, settings.
    pub caps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Devices {
    pub devices: Vec<Device>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceParams {
    pub device: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LightingState {
    pub on: bool,
    /// 0-359.
    pub hue: u16,
    /// 0-100.
    pub saturation: u8,
    /// 0-100.
    pub brightness: u8,
    pub effect: String,
    pub speed: u8,
    /// Every part of the device shows this; false while a split keyboard's other half is still
    /// catching up (it can lag seconds). Older apps don't send it: then true.
    #[serde(default = "settled_default")]
    pub settled: bool,
}

fn settled_default() -> bool {
    true
}

/// Changes only what's given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetLightingParams {
    pub device: String,
    #[serde(default)]
    pub on: Option<bool>,
    #[serde(default)]
    pub color: Option<Hsb>,
    #[serde(default)]
    pub effect: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Hsb {
    pub hue: u16,
    pub saturation: u8,
    pub brightness: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Behavior {
    pub id: u32,
    pub param1: u32,
    pub param2: u32,
    pub name: String,
    /// Plugins can never invoke these; they need the user's click in Foxglove.
    pub needs_confirmation: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Behaviors {
    pub behaviors: Vec<Behavior>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InvokeParams {
    pub device: String,
    pub id: u32,
    pub param1: u32,
    pub param2: u32,
}

/// Draws over an info screen (the panel the PC draws) until `ttl_ms` passes (at most 60 000),
/// then it goes back to what Foxglove draws. `ttl_ms` 0 holds it until the plugin replaces it,
/// releases it (`screens.panel_release`) or its connection closes (API 1.3), so a steady screen
/// needs no re-sending (#281). A later override replaces an earlier one. `width` and `height`
/// are the screen's, from `screens.list` (API 1.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PanelOverrideParams {
    pub device: String,
    pub width: u16,
    pub height: u16,
    /// 1 bit per pixel, rows MSB first, padded to whole bytes, 1 = ink; base64.
    pub bitmap: String,
    pub ttl_ms: u32,
    /// The info screen to draw on, by its `screens.list` id (API 1.4). Absent: the keyboard's
    /// first info screen, as before 1.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<u8>,
}

/// One of a device's screens (API 1.4, #560), named by what it is for, never by side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenInfo {
    /// The id `screens.panel_override` takes.
    pub id: u8,
    /// `info` (the panel the PC draws; plugins draw here), `art` (where art plays) or `display`
    /// (one the keyboard's firmware draws itself).
    pub role: String,
    /// What Foxglove calls it: "Info screen", "Art screen", or the board profile's name.
    pub name: String,
    /// As the screen stores its picture.
    pub width: u16,
    pub height: u16,
    /// `1bit`; frames for other formats aren't taken yet.
    pub format: String,
    /// Quarter turns clockwise from upright to how it's stored (an art screen on its side: 1).
    pub rotation: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Screens {
    pub screens: Vec<ScreenInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArtStatus {
    /// The built-in set shown, by its manifest name when known.
    pub built_in: Option<String>,
    /// The flash slot shown.
    pub slot: Option<u8>,
    /// A pushed (RAM) set is shown.
    pub pushed: bool,
    pub flash_slots: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtTarget {
    Random,
    /// A built-in set by its manifest name.
    BuiltIn(String),
    Slot(u8),
    /// An art blob file (NVA1) on this PC.
    File(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArtSelectParams {
    pub device: String,
    pub target: ArtTarget,
}

/// Foxglove's profiles and the active one (`None` = the base settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Profiles {
    pub names: Vec<String>,
    pub active: Option<String>,
}

/// Switches profile; `name` null = the base settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActivateProfileParams {
    pub name: Option<String>,
}

/// Event (needs `key.events`): one key went down or up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyEvent {
    pub device: String,
    pub row: u16,
    pub col: u16,
    pub pressed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Empty {}

/// Event: the user pressed the key bound to one of the plugin's actions (`[[actions]]` in
/// `plugin.toml`); sent only to that plugin (API 1.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionInvoked {
    pub action: String,
}

/// Asks to be told when a program's windows open and close (`apps.watch:<exe>`, API 1.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AppsWatchParams {
    /// The program's file name, such as `obs64.exe`.
    pub exe: String,
}

/// A watched program's visible top-level windows: the answer to `apps.watch` (how many are open
/// now), and the `app.window.appeared` and `app.window.closed` events (how many are open after
/// it). `open` 0 after a close means the app has probably quit (API 1.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AppWindows {
    pub exe: String,
    pub open: u32,
}

/// A plugin's settings (`[[settings]]` in `plugin.toml`): the answer to `settings.get` and the
/// `settings.changed` event, every setting with its current value (API 1.3).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SettingValues {
    pub values: serde_json::Map<String, serde_json::Value>,
}

/// Mutes or unmutes the default communications microphone (#237, API 1.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetMuteParams {
    pub muted: bool,
}

/// How much a lease matters against others on the same lights (#236).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum LeasePriority {
    /// Ambient light (a colour sync).
    Low,
    #[default]
    Normal,
    /// Alerts (the microphone is muted).
    High,
}

/// Holds a device's lights at a colour until released (#236, API 1.2). The highest-priority
/// lease shows, the newest among equals; when the last one ends the lights go back as they were.
/// A plugin's leases end with its connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseParams {
    pub device: String,
    pub color: Hsb,
    /// The effect while leased; the static colour (`"0"`) if absent.
    #[serde(default)]
    pub effect: Option<String>,
    #[serde(default)]
    pub priority: LeasePriority,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Lease {
    pub lease: u64,
}

/// Changes a held lease's colour (and effect) in place, for a stream of colours (a colour sync)
/// without taking a new lease each time (API 1.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseUpdate {
    pub lease: u64,
    pub color: Hsb,
    #[serde(default)]
    pub effect: Option<String>,
}

/// One of the plugin's own secrets (#213, API 1.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SecretName {
    /// 1-64 of a-z, 0-9, - _ .
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SecretValue {
    /// `null` when it isn't set.
    pub value: Option<String>,
}

/// Saves a secret (`null` removes it); at most 2048 bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SetSecretParams {
    pub name: String,
    pub value: Option<String>,
}

/// The kind of connection `net.open` makes (API 1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    /// A TCP connection to `host`:`port`; needs a `net.connect` permission covering it.
    Tcp,
    /// Datagrams sent to 127.0.0.1:`port`; needs `net.listen:udp:127.0.0.1:<port>`.
    Udp,
    /// Accepts TCP connections on 127.0.0.1:`port` (API 1.3); needs
    /// `net.listen:tcp:127.0.0.1:<port>`. Each connection arrives as `net.accepted` with its own
    /// stream, then `net.data` and `net.closed` like any other; closing the listener's stream
    /// stops accepting.
    #[serde(rename = "tcp-listen")]
    TcpListen,
}

/// Opens a connection through Foxglove: the plugin stays sandboxed and only reaches what its
/// permissions name (API 1.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetOpenParams {
    pub kind: NetKind,
    /// For `udp`, only `127.0.0.1`.
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetStream {
    pub stream: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetSendParams {
    pub stream: u64,
    /// Bytes, base64.
    pub data: String,
}

/// Event: bytes arrived on a stream (base64); `from` is the sender for `udp`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetData {
    pub stream: u64,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// Event: a listener (`tcp-listen`) accepted a connection, which is the new `stream` (API 1.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetAccepted {
    pub listener: u64,
    pub stream: u64,
    /// The other side's address (always on this PC).
    pub from: String,
}

/// Event: a stream ended (the other side closed it, or an error).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NetClosed {
    pub stream: u64,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Client → app, answered.
    Call,
    /// App → client, unanswered.
    Event,
}

/// One row of the API: name, direction, the permission it needs (as written in a manifest;
/// `facts.*` methods are checked per key instead), and its schemas.
pub struct Method {
    pub name: &'static str,
    pub kind: Kind,
    pub permission: Option<&'static str>,
    pub params: fn() -> Value,
    pub result: Option<fn() -> Value>,
}

macro_rules! schema {
    ($t:ty) => {
        || serde_json::to_value(schema_for!($t)).unwrap()
    };
}

pub const METHODS: &[Method] = &[
    Method {
        name: "initialize",
        kind: Kind::Call,
        permission: None,
        params: schema!(InitializeParams),
        result: Some(schema!(InitializeResult)),
    },
    Method {
        name: "ping",
        kind: Kind::Call,
        permission: None,
        params: schema!(Empty),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "facts.get",
        kind: Kind::Call,
        permission: None,
        params: schema!(KeyParams),
        result: Some(schema!(FactResult)),
    },
    Method {
        name: "facts.list",
        kind: Kind::Call,
        permission: None,
        params: schema!(SubscribeParams),
        result: Some(schema!(FactList)),
    },
    Method {
        name: "facts.set",
        kind: Kind::Call,
        permission: None,
        params: schema!(SetFactParams),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "facts.subscribe",
        kind: Kind::Call,
        permission: None,
        params: schema!(SubscribeParams),
        result: Some(schema!(Subscription)),
    },
    Method {
        name: "facts.unsubscribe",
        kind: Kind::Call,
        permission: None,
        params: schema!(Subscription),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "facts.changed",
        kind: Kind::Event,
        permission: None,
        params: schema!(FactChanged),
        result: None,
    },
    Method {
        name: "devices.list",
        kind: Kind::Call,
        permission: Some("devices.read"),
        params: schema!(Empty),
        result: Some(schema!(Devices)),
    },
    Method {
        name: "devices.changed",
        kind: Kind::Event,
        permission: Some("devices.read"),
        params: schema!(Devices),
        result: None,
    },
    Method {
        name: "lighting.state",
        kind: Kind::Call,
        permission: Some("devices.read"),
        params: schema!(DeviceParams),
        result: Some(schema!(LightingState)),
    },
    Method {
        name: "lighting.set",
        kind: Kind::Call,
        permission: Some("lighting.write"),
        params: schema!(SetLightingParams),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "behaviors.list",
        kind: Kind::Call,
        permission: Some("devices.read"),
        params: schema!(DeviceParams),
        result: Some(schema!(Behaviors)),
    },
    Method {
        name: "behaviors.invoke",
        kind: Kind::Call,
        permission: Some("behaviors.invoke"),
        params: schema!(InvokeParams),
        result: Some(schema!(Empty)),
    },
    // A device's screens by role, id and size (API 1.4, #560).
    Method {
        name: "screens.list",
        kind: Kind::Call,
        permission: Some("devices.read"),
        params: schema!(DeviceParams),
        result: Some(schema!(Screens)),
    },
    Method {
        name: "screens.panel_override",
        kind: Kind::Call,
        permission: Some("screens.write"),
        params: schema!(PanelOverrideParams),
        result: Some(schema!(Empty)),
    },
    // Gives the panel back after a held override (API 1.3).
    Method {
        name: "screens.panel_release",
        kind: Kind::Call,
        permission: Some("screens.write"),
        params: schema!(DeviceParams),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "art.status",
        kind: Kind::Call,
        permission: Some("devices.read"),
        params: schema!(DeviceParams),
        result: Some(schema!(ArtStatus)),
    },
    Method {
        name: "art.select",
        kind: Kind::Call,
        permission: Some("art.write"),
        params: schema!(ArtSelectParams),
        result: Some(schema!(ArtStatus)),
    },
    Method {
        name: "profiles.list",
        kind: Kind::Call,
        permission: None,
        params: schema!(Empty),
        result: Some(schema!(Profiles)),
    },
    Method {
        name: "profiles.activate",
        kind: Kind::Call,
        permission: Some("profiles.write"),
        params: schema!(ActivateProfileParams),
        result: Some(schema!(Profiles)),
    },
    Method {
        name: "key.event",
        kind: Kind::Event,
        permission: Some("key.events"),
        params: schema!(KeyEvent),
        result: None,
    },
    // Checked against the scoped net permissions by the host (API 1.2).
    Method {
        name: "net.open",
        kind: Kind::Call,
        permission: None,
        params: schema!(NetOpenParams),
        result: Some(schema!(NetStream)),
    },
    Method {
        name: "net.send",
        kind: Kind::Call,
        permission: None,
        params: schema!(NetSendParams),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "net.close",
        kind: Kind::Call,
        permission: None,
        params: schema!(NetStream),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "net.data",
        kind: Kind::Event,
        permission: None,
        params: schema!(NetData),
        result: None,
    },
    Method {
        name: "net.closed",
        kind: Kind::Event,
        permission: None,
        params: schema!(NetClosed),
        result: None,
    },
    Method {
        name: "net.accepted",
        kind: Kind::Event,
        permission: None,
        params: schema!(NetAccepted),
        result: None,
    },
    // The microphone, through Foxglove (#237); its state is the `mic.*` facts.
    Method {
        name: "audio.inputs.set_mute",
        kind: Kind::Call,
        permission: Some("audio.inputs.mute"),
        params: schema!(SetMuteParams),
        result: Some(schema!(Empty)),
    },
    // Hold and give back the lights (#236).
    Method {
        name: "lighting.lease",
        kind: Kind::Call,
        permission: Some("lighting.write"),
        params: schema!(LeaseParams),
        result: Some(schema!(Lease)),
    },
    Method {
        name: "lighting.release",
        kind: Kind::Call,
        permission: Some("lighting.write"),
        params: schema!(Lease),
        result: Some(schema!(Empty)),
    },
    Method {
        name: "lighting.update",
        kind: Kind::Call,
        permission: Some("lighting.write"),
        params: schema!(LeaseUpdate),
        result: Some(schema!(Empty)),
    },
    // A plugin's own secrets; outside tools get none (#213).
    Method {
        name: "secrets.get",
        kind: Kind::Call,
        permission: Some("secrets"),
        params: schema!(SecretName),
        result: Some(schema!(SecretValue)),
    },
    Method {
        name: "secrets.set",
        kind: Kind::Call,
        permission: Some("secrets"),
        params: schema!(SetSecretParams),
        result: Some(schema!(Empty)),
    },
    // Sent to one plugin when the key bound to its action is pressed (#212).
    Method {
        name: "action.invoked",
        kind: Kind::Event,
        permission: None,
        params: schema!(ActionInvoked),
        result: None,
    },
    // A plugin's own settings (#69): no permission, plugins only (API 1.3).
    Method {
        name: "settings.get",
        kind: Kind::Call,
        permission: None,
        params: schema!(Empty),
        result: Some(schema!(SettingValues)),
    },
    Method {
        name: "settings.changed",
        kind: Kind::Event,
        permission: None,
        params: schema!(SettingValues),
        result: None,
    },
    // Waiting for an app's windows instead of polling for it (#277); checked against the
    // scoped apps.watch permissions by the host (API 1.3).
    Method {
        name: "apps.watch",
        kind: Kind::Call,
        permission: None,
        params: schema!(AppsWatchParams),
        result: Some(schema!(AppWindows)),
    },
    Method {
        name: "app.window.appeared",
        kind: Kind::Event,
        permission: None,
        params: schema!(AppWindows),
        result: None,
    },
    Method {
        name: "app.window.closed",
        kind: Kind::Event,
        permission: None,
        params: schema!(AppWindows),
        result: None,
    },
];

pub fn find(name: &str) -> Option<&'static Method> {
    METHODS.iter().find(|m| m.name == name)
}

pub(crate) fn schema() -> Value {
    let rows: serde_json::Map<String, Value> = METHODS
        .iter()
        .map(|m| {
            let mut row = json!({
                "kind": match m.kind { Kind::Call => "call", Kind::Event => "event" },
                "params": (m.params)(),
            });
            if let Some(p) = m.permission {
                row["permission"] = json!(p);
            }
            if let Some(r) = m.result {
                row["result"] = r();
            }
            (m.name.to_string(), row)
        })
        .collect();
    json!({
        "api": crate::API_VERSION.to_string(),
        "transport": "JSON-RPC 2.0, one message per line",
        "methods": rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_consistent() {
        let mut names: Vec<_> = METHODS.iter().map(|m| m.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), METHODS.len(), "duplicate method names");
        for m in METHODS {
            if let Some(p) = m.permission {
                assert!(
                    Permission::try_from(p.to_string()).is_ok(),
                    "{}: bad permission {p}",
                    m.name
                );
            }
            assert_eq!(
                m.kind == Kind::Event,
                m.result.is_none(),
                "{}: events have no result",
                m.name
            );
        }
    }

    #[test]
    fn facts_carry_bytes_as_base64() {
        assert_eq!(serde_json::to_value(Fact::Int(3)).unwrap(), json!(3));
        let b: Fact = serde_json::from_value(json!({"base64": "AAE="})).unwrap();
        assert_eq!(
            b,
            Fact::Bytes {
                base64: "AAE=".into()
            }
        );
        let t: ArtTarget = serde_json::from_value(json!({"built_in": "ZMK_1"})).unwrap();
        assert_eq!(t, ArtTarget::BuiltIn("ZMK_1".into()));
        assert_eq!(
            serde_json::to_value(ArtTarget::Random).unwrap(),
            json!("random")
        );
    }
}
