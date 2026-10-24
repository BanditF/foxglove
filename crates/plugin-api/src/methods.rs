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

/// Draws over the left panel until `ttl_ms` passes (at most 60 000), then the panel goes back
/// to what Foxglove draws. A later override replaces an earlier one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PanelOverrideParams {
    pub device: String,
    pub width: u16,
    pub height: u16,
    /// 1 bit per pixel, rows MSB first, padded to whole bytes, 1 = ink; base64.
    pub bitmap: String,
    pub ttl_ms: u32,
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
    Method { name: "initialize", kind: Kind::Call, permission: None, params: schema!(InitializeParams), result: Some(schema!(InitializeResult)) },
    Method { name: "ping", kind: Kind::Call, permission: None, params: schema!(Empty), result: Some(schema!(Empty)) },
    Method { name: "facts.get", kind: Kind::Call, permission: None, params: schema!(KeyParams), result: Some(schema!(FactResult)) },
    Method { name: "facts.list", kind: Kind::Call, permission: None, params: schema!(SubscribeParams), result: Some(schema!(FactList)) },
    Method { name: "facts.set", kind: Kind::Call, permission: None, params: schema!(SetFactParams), result: Some(schema!(Empty)) },
    Method { name: "facts.subscribe", kind: Kind::Call, permission: None, params: schema!(SubscribeParams), result: Some(schema!(Subscription)) },
    Method { name: "facts.unsubscribe", kind: Kind::Call, permission: None, params: schema!(Subscription), result: Some(schema!(Empty)) },
    Method { name: "facts.changed", kind: Kind::Event, permission: None, params: schema!(FactChanged), result: None },
    Method { name: "devices.list", kind: Kind::Call, permission: Some("devices.read"), params: schema!(Empty), result: Some(schema!(Devices)) },
    Method { name: "devices.changed", kind: Kind::Event, permission: Some("devices.read"), params: schema!(Devices), result: None },
    Method { name: "lighting.state", kind: Kind::Call, permission: Some("devices.read"), params: schema!(DeviceParams), result: Some(schema!(LightingState)) },
    Method { name: "lighting.set", kind: Kind::Call, permission: Some("lighting.write"), params: schema!(SetLightingParams), result: Some(schema!(Empty)) },
    Method { name: "behaviors.list", kind: Kind::Call, permission: Some("devices.read"), params: schema!(DeviceParams), result: Some(schema!(Behaviors)) },
    Method { name: "behaviors.invoke", kind: Kind::Call, permission: Some("behaviors.invoke"), params: schema!(InvokeParams), result: Some(schema!(Empty)) },
    Method { name: "screens.panel_override", kind: Kind::Call, permission: Some("screens.write"), params: schema!(PanelOverrideParams), result: Some(schema!(Empty)) },
    Method { name: "art.status", kind: Kind::Call, permission: Some("devices.read"), params: schema!(DeviceParams), result: Some(schema!(ArtStatus)) },
    Method { name: "art.select", kind: Kind::Call, permission: Some("art.write"), params: schema!(ArtSelectParams), result: Some(schema!(ArtStatus)) },
    Method { name: "profiles.list", kind: Kind::Call, permission: None, params: schema!(Empty), result: Some(schema!(Profiles)) },
    Method { name: "profiles.activate", kind: Kind::Call, permission: Some("profiles.write"), params: schema!(ActivateProfileParams), result: Some(schema!(Profiles)) },
    Method { name: "key.event", kind: Kind::Event, permission: Some("key.events"), params: schema!(KeyEvent), result: None },
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
                assert!(Permission::try_from(p.to_string()).is_ok(), "{}: bad permission {p}", m.name);
            }
            assert_eq!(m.kind == Kind::Event, m.result.is_none(), "{}: events have no result", m.name);
        }
    }

    #[test]
    fn facts_carry_bytes_as_base64() {
        assert_eq!(serde_json::to_value(Fact::Int(3)).unwrap(), json!(3));
        let b: Fact = serde_json::from_value(json!({"base64": "AAE="})).unwrap();
        assert_eq!(b, Fact::Bytes { base64: "AAE=".into() });
        let t: ArtTarget = serde_json::from_value(json!({"built_in": "ZMK_1"})).unwrap();
        assert_eq!(t, ArtTarget::BuiltIn("ZMK_1".into()));
        assert_eq!(serde_json::to_value(ArtTarget::Random).unwrap(), json!("random"));
    }
}
