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

/// The badge a module sets on its page panel's tab (v1.3, §5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Badge {
    /// No badge.
    None,
    /// Something new wants a look (a dot).
    Attention,
    /// Unsaved work: closing the tab asks first (§2.5).
    Dirty,
    /// `count` new items, 1 or more (the hub shows "99+" above 99).
    Count,
}

/// `ui.badge` (M → H, notification, v1.3). `count` is set with `badge: "count"` only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UiBadge {
    pub panel: String,
    pub badge: Badge,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// On screen.
    Visible,
    /// Still open but not on screen: a background tab, a minimised window, a tab of the hub's
    /// main window while it is in the tray. A pop-out on screen stays visible (spec v1.6).
    Hidden,
    /// The user closed the page (its tab, or Close page). Closing a pop-out docks it back
    /// instead and is never `closed` (spec v1.6).
    Closed,
}

/// `ui.visibility` (H → M, notification, v1.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UiVisibility {
    pub panel: String,
    pub state: Visibility,
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

/// Crashes in ten minutes that stop a module (§7.7).
pub const CRASH_LIMIT: u32 = 5;

/// The fact `module.<id>.restart` (§7.8, spec v1.4): set by the supervisor while the module waits
/// to be restarted, `null` once the restart starts. `crash` is this crash's number in the
/// ten-minute window, 1 to `of` - 1 (the `of`-th stops it instead).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RestartFact {
    /// Seconds until the restart: one of the §7.7 backoff steps.
    pub delay_s: u64,
    pub crash: u32,
    pub of: u32,
}

/// The fact `module.<id>.recovery` (§7.8, spec v1.4): the step of the recovery ladder a
/// `crash-stopped` module's card offers (its one main button).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Recovery {
    /// **Start again** (the first stop this sign-in).
    StartAgain,
    /// **Start with default settings** (the second stop).
    Defaults,
    /// **Go back to <previous>** (the third stop and later, when a previous version is kept).
    GoBack,
    /// **Copy diagnostics** and **Turn off <Module>** (the third stop and later, with no
    /// previous version; #983).
    Last,
}

/// Checks a value of a fact the hub's supervisor publishes (`module.<id>.restart`,
/// `module.<id>.recovery`, `module.<id>.previous`, `module.<id>.failed`; spec v1.4;
/// `module.<id>.revoked`, v1.6, and `module.<id>.check`, v1.9). Other keys
/// are not this check's: `Ok`. Also the app watch's facts (§4.6): `apps.seen.<name>` is `true`
/// or `null`, `apps.running.<name>` (v1.7) is `true`, `false` or `null`, and `<name>` is one
/// app label.
pub fn check_hub_fact(key: &str, value: &serde_json::Value) -> Result<(), String> {
    if key.starts_with("apps.") {
        let Some(label) = crate::module::app_fact_label(key) else {
            return Err(format!(
                "{key} is not apps.seen.<name> or apps.running.<name>"
            ));
        };
        if crate::module::app_label(label).as_deref() != Some(label) {
            return Err(format!("{label:?} is not an app label"));
        }
        let running = key.starts_with("apps.running.");
        return match value {
            serde_json::Value::Null | serde_json::Value::Bool(true) => Ok(()),
            serde_json::Value::Bool(false) if running => Ok(()),
            _ if running => Err("apps.running.<name> is true, false or null".into()),
            _ => Err("apps.seen.<name> is true or null".into()),
        };
    }
    let Some(rest) = key.strip_prefix("module.") else {
        return Ok(());
    };
    if let Some(id) = rest.strip_suffix(".restart") {
        if id.is_empty() {
            return Err("module.<id>.restart needs an id".into());
        }
        if value.is_null() {
            return Ok(());
        }
        let r: RestartFact = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if r.of != CRASH_LIMIT || r.crash == 0 || r.crash >= r.of {
            return Err(format!("crash {} of {} is not a restart", r.crash, r.of));
        }
        if !crate::BACKOFF_STEPS_S.contains(&r.delay_s) {
            return Err(format!("delay_s {} is not a backoff step", r.delay_s));
        }
        return Ok(());
    }
    if let Some(id) = rest.strip_suffix(".recovery") {
        if id.is_empty() {
            return Err("module.<id>.recovery needs an id".into());
        }
        if value.is_null() {
            return Ok(());
        }
        return serde_json::from_value::<Recovery>(value.clone())
            .map(|_| ())
            .map_err(|e| e.to_string());
    }
    if let Some(id) = rest.strip_suffix(".previous") {
        if id.is_empty() {
            return Err("module.<id>.previous needs an id".into());
        }
        return match value {
            serde_json::Value::Null => Ok(()),
            serde_json::Value::String(v) if !v.is_empty() => Ok(()),
            _ => Err("module.<id>.previous is a version or null".into()),
        };
    }
    if let Some(id) = rest.strip_suffix(".check") {
        if id.is_empty() {
            return Err("module.<id>.check needs an id".into());
        }
        return match value.as_str() {
            _ if value.is_null() => Ok(()),
            Some("checking" | "none" | "failed") => Ok(()),
            _ => Err("module.<id>.check is \"checking\", \"none\", \"failed\" or null".into()),
        };
    }
    if let Some(id) = rest.strip_suffix(".revoked") {
        if id.is_empty() {
            return Err("module.<id>.revoked needs an id".into());
        }
        if value.is_null() {
            return Ok(());
        }
        let ok = value.as_object().is_some_and(|o| {
            o.len() == 2
                && o.get("version")
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| !v.is_empty())
                && o.get("reason").is_some_and(|r| r.is_string())
        });
        return if ok {
            Ok(())
        } else {
            Err("module.<id>.revoked is {version, reason} or null".into())
        };
    }
    if let Some(id) = rest.strip_suffix(".failed") {
        if id.is_empty() {
            return Err("module.<id>.failed needs an id".into());
        }
        return match value.as_str() {
            _ if value.is_null() => Ok(()),
            Some("launch") => Ok(()),
            _ => Err("module.<id>.failed is \"launch\" or null".into()),
        };
    }
    Ok(())
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
    /// (v1.10, #1092) The provider's process id, when the hub knows it: the consumer checks
    /// that the pipe's server is that process (`GetNamedPipeServerProcessId`) before it sends
    /// the nonce, as a module checks `HIDEOUT_HUB_PID` (§7.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// `channel.create` (H → M): the provider makes the pipe for `consumer`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelCreate {
    pub channel: String,
    pub consumer: String,
    pub nonce: String,
    /// (v1.10, #1092) The consumer's process id, when the hub knows it: the provider admits
    /// only that process (`GetNamedPipeClientProcessId`) before it reads the nonce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer_pid: Option<u32>,
    /// (v1.10, #1092) The consumer's AppContainer SID, when it runs in one (§7.7): the pipe
    /// lets that SID in with client rights and carries a Low label, and nothing else changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer_sid: Option<String>,
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
        "ui.badge" => {
            let b = UiBadge::deserialize(params).map_err(|e| e.to_string())?;
            match (b.badge, b.count) {
                (Badge::Count, Some(n)) if n >= 1 => Ok(()),
                (Badge::Count, _) => Err("badge count needs count >= 1".into()),
                (_, None) => Ok(()),
                (_, Some(_)) => Err("count goes with badge \"count\" only".into()),
            }
        }
        "ui.visibility" => typed::<UiVisibility>(params),
        "action.invoke" => typed::<ActionInvoke>(params),
        "settings.set" => typed::<SettingsSet>(params),
        "hub.notify" => typed::<HubNotify>(params),
        "channel.open" => typed::<ChannelOpen>(params),
        "channel.create" => typed::<ChannelCreate>(params),
        "modules.enable" | "modules.disable" | "modules.reload" => typed::<ModuleIdParams>(params),
        // The host services (§8.10, §8.11, v1.12).
        "net.open" => typed::<crate::services::NetOpen>(params),
        "net.send" => typed::<crate::services::NetSend>(params),
        "net.close" => typed::<crate::services::NetStream>(params),
        "net.data" => typed::<crate::services::NetData>(params),
        "net.accepted" => typed::<crate::services::NetAccepted>(params),
        "net.closed" => typed::<crate::services::NetClosed>(params),
        "audio.inputs.set_mute" => typed::<crate::services::SetMute>(params),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn badge_and_visibility_params() {
        for ok in [
            json!({"panel": "p", "badge": "none"}),
            json!({"panel": "p", "badge": "attention"}),
            json!({"panel": "p", "badge": "dirty"}),
            json!({"panel": "p", "badge": "count", "count": 3}),
        ] {
            assert!(check_params("ui.badge", &ok).is_ok(), "{ok}");
        }
        for bad in [
            json!({"panel": "p", "badge": "count"}),
            json!({"panel": "p", "badge": "count", "count": 0}),
            json!({"panel": "p", "badge": "red"}),
            json!({"badge": "dirty"}),
            json!({"panel": "p", "badge": "dirty", "count": 2}),
        ] {
            assert!(check_params("ui.badge", &bad).is_err(), "{bad}");
        }
        for state in ["visible", "hidden", "closed"] {
            assert!(check_params("ui.visibility", &json!({"panel": "p", "state": state})).is_ok());
        }
        assert!(check_params("ui.visibility", &json!({"panel": "p", "state": "gone"})).is_err());
    }
}
