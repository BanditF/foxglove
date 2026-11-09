//! Registration, UI and channel handshake messages (§8.8), and the JSON-RPC 2.0 envelope (§8.1).

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// JSON-RPC error codes of §8.1 used by the hub.
pub mod code {
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// `initialize` from a module (M → H).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InitializeModule {
    pub id: String,
    pub version: String,
    pub instance: String,
    pub token: String,
    pub hub: String,
    #[serde(default)]
    pub api: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// `initialize` from an outside tool (T → H).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InitializeTool {
    pub client: ClientInfo,
    #[serde(default)]
    pub hub: Option<String>,
    pub api: serde_json::Value,
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// `module.register` (M → H).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModuleRegister {
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub facts: Vec<String>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub apis: BTreeMap<String, String>,
}

/// One refused item in a `module.register` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RejectedItem {
    pub item: String,
    pub error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RegisterResult {
    #[serde(default)]
    pub rejected: Vec<RejectedItem>,
}

/// `module.ready` (M → H, notification).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModuleReady {
    pub instance: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DrainReason {
    Update,
    Rollback,
    Reload,
    Disable,
    Remove,
}

/// `module.drain` (H → M).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModuleDrain {
    pub reason: DrainReason,
}

/// The `module.drain` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum DrainResult {
    Done,
    Busy {
        #[serde(default)]
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PatchOpKind {
    Set,
    Insert,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PatchOp {
    pub op: PatchOpKind,
    pub node: String,
    #[serde(default)]
    pub props: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub tree: Option<serde_json::Value>,
}

/// `ui.patch` (M → H, notification).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UiPatch {
    pub panel: String,
    pub ops: Vec<PatchOp>,
}

/// `ui.event` (H → M).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UiEvent {
    pub panel: String,
    pub node: String,
    pub kind: String,
    #[serde(default)]
    pub value: serde_json::Value,
}

/// `ui.asset.put` (M → H): base64 `data`, at most 256 KiB decoded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UiAssetPut {
    pub hash: String,
    pub mime: String,
    pub data: String,
}

/// `ui.window.open` (H → M).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UiWindowOpen {
    pub window: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActionSource {
    Key,
    Tray,
    Palette,
    Panel,
}

/// `action.invoke` (H → M).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ActionInvoke {
    pub action: String,
    pub source: ActionSource,
}

/// `settings.set` (H → M).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SettingsSet {
    pub name: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NotifyLevel {
    Info,
    Warn,
    Error,
}

/// `hub.notify` (M → H).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HubNotify {
    pub title: String,
    pub body: String,
    pub level: NotifyLevel,
}

/// `channel.open` (C → H): the channel handshake's first half.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelOpen {
    pub module: String,
    pub channel: String,
}

/// The `channel.open` result: the pipe to connect to and the nonce to present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelOpened {
    pub pipe: String,
    pub nonce: String,
}

/// `channel.create` (H → M): the provider makes the pipe for `consumer`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelCreate {
    pub channel: String,
    pub consumer: String,
    pub nonce: String,
}

/// `modules.enable`, `modules.disable`, `modules.reload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModuleIdParams {
    pub id: String,
}

fn typed<T: DeserializeOwned>(params: &serde_json::Value) -> Result<(), String> {
    T::deserialize(params)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Checks a request's or notification's params against §8.8. Methods not in §8.8 (provider
/// methods, Foxglove API v1 methods) pass.
pub fn check_params(method: &str, params: &serde_json::Value) -> Result<(), String> {
    match method {
        "initialize" if params.get("client").is_some() => typed::<InitializeTool>(params),
        "initialize" => typed::<InitializeModule>(params),
        "module.register" => typed::<ModuleRegister>(params),
        "module.ready" => typed::<ModuleReady>(params),
        "module.drain" => typed::<ModuleDrain>(params),
        "ui.register" => typed::<crate::panel::Register>(params),
        "ui.patch" => typed::<UiPatch>(params),
        "ui.event" => typed::<UiEvent>(params),
        "ui.asset.put" => typed::<UiAssetPut>(params),
        "ui.window.open" => typed::<UiWindowOpen>(params),
        "action.invoke" => typed::<ActionInvoke>(params),
        "settings.set" => typed::<SettingsSet>(params),
        "hub.notify" => typed::<HubNotify>(params),
        "channel.open" => typed::<ChannelOpen>(params),
        "channel.create" => typed::<ChannelCreate>(params),
        "modules.enable" | "modules.disable" | "modules.reload" => typed::<ModuleIdParams>(params),
        _ => Ok(()),
    }
}

/// What a JSON-RPC 2.0 message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Request,
    Notification,
    Result,
    Error,
}

/// Checks the JSON-RPC 2.0 envelope (§8.1) and, for calls, the §8.8 params.
pub fn check_message(msg: &serde_json::Value) -> Result<Shape, String> {
    let obj = msg.as_object().ok_or("a message is an object")?;
    if obj.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        return Err("jsonrpc must be \"2.0\"".into());
    }
    let id_ok = obj
        .get("id")
        .is_some_and(|i| i.is_number() || i.is_string() || i.is_null());
    if let Some(method) = obj.get("method") {
        let method = method.as_str().ok_or("method is a string")?;
        let empty = serde_json::Value::Object(Default::default());
        check_params(method, obj.get("params").unwrap_or(&empty))
            .map_err(|e| format!("{method}: {e}"))?;
        return Ok(if obj.contains_key("id") {
            if !id_ok {
                return Err("bad id".into());
            }
            Shape::Request
        } else {
            Shape::Notification
        });
    }
    if !id_ok {
        return Err("a response has an id".into());
    }
    match (obj.get("result"), obj.get("error")) {
        (Some(_), None) => Ok(Shape::Result),
        (None, Some(e)) if e.get("code").is_some_and(|c| c.is_i64()) => Ok(Shape::Error),
        _ => Err("a response has exactly one of result and error (with an integer code)".into()),
    }
}
