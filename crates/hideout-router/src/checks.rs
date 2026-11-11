// SPDX-License-Identifier: MIT
//! What each call needs (§6.1, §8.8, §8.9) and the method areas nobody else may provide (§6.3).

use foxglove_plugin_api::methods::{self, Kind};
use hideout_contract::capability::{Capability, GpuClass};
use hideout_contract::permission::{Grant, Permission};
use serde_json::Value;

/// Methods the hub answers itself: no module may provide them.
pub const HUB_OWNED: &[&str] = &[
    "initialize",
    "ping",
    "facts.*",
    "module.*",
    "ui.*",
    "channel.*",
    "hub.*",
    "state.*",
    "settings.*",
    "action.*",
    // The host services (§8.3, v1.12).
    "net.*",
    "audio.inputs.*",
];

/// Method areas a first-party catalog assigns (§6.3): a third-party or private module never
/// provides them, so it can't capture other modules' calls.
pub const FIRST_PARTY_AREAS: &[&str] = &[
    "lighting.*",
    "screens.*",
    "art.*",
    "gpu.*",
    "ai.*",
    "image.*",
    "devices.*",
    "behaviors.*",
    "profiles.*",
    "focus.*",
    "modules.*",
];

/// Whether pattern `outer` (`a.b.*`, `*` or an exact name) covers `inner` (a name or a pattern).
pub fn covers(outer: &str, inner: &str) -> bool {
    if outer == inner {
        return true;
    }
    match outer.strip_suffix('*') {
        Some("") => true,
        Some(prefix) => prefix.ends_with('.') && inner.starts_with(prefix),
        None => false,
    }
}

pub fn overlap(a: &str, b: &str) -> bool {
    covers(a, b) || covers(b, a)
}

/// A connection's granted permissions.
#[derive(Clone, Debug, Default)]
pub struct Caps(pub Vec<Capability>);

impl Caps {
    pub fn parse<'a>(list: impl IntoIterator<Item = &'a String>) -> Caps {
        Caps(
            list.into_iter()
                .filter_map(|s| match Capability::parse(s) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        log::warn!("router: grant {s:?} not understood: {e}");
                        None
                    }
                })
                .collect(),
        )
    }

    /// The Foxglove v1 part, for fact checks.
    pub fn kept(&self) -> Grant {
        Grant(
            self.0
                .iter()
                .filter_map(|c| match c {
                    Capability::Kept(p) => Some(p.clone()),
                    _ => None,
                })
                .collect(),
        )
    }

    pub fn has(&self, c: &Capability) -> bool {
        self.0.contains(c)
    }

    pub fn provides(&self, pattern: &str) -> bool {
        self.0
            .iter()
            .any(|c| matches!(c, Capability::Provide(p) if covers(p, pattern)))
    }

    pub fn offers(&self, channel: &str) -> bool {
        self.has(&Capability::ChannelOffer(channel.into()))
    }

    pub fn opens(&self, module: &str, channel: &str) -> bool {
        self.has(&Capability::ChannelOpen {
            module: module.into(),
            channel: channel.into(),
        })
    }

    /// Canonical strings, sorted (§6.4).
    pub fn strings(&self) -> Vec<String> {
        let mut v: Vec<String> = self.0.iter().map(|c| c.to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The v1 permissions only, for a client speaking API v1 (its `granted` is `[Permission]`).
    pub fn v1_strings(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .kept()
            .0
            .into_iter()
            .map(|p: Permission| String::from(p))
            .collect();
        v.sort();
        v
    }
}

/// Whether `caps` may call `method` with `params`. Facts, channels and the hub's own messages
/// are checked where they are handled; this covers routed methods.
pub fn may_call(caps: &Caps, method: &str, params: &Value) -> bool {
    let arg = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or_default();
    match method {
        "gpu.status" => caps.has(&Capability::GpuStatus),
        "gpu.request" => {
            let class = match arg("class") {
                "chat" => GpuClass::Chat,
                "image" => GpuClass::Image,
                _ => GpuClass::Any,
            };
            caps.has(&Capability::GpuRequest(class))
                || caps.has(&Capability::GpuRequest(GpuClass::Any))
        }
        "ai.ask" => {
            caps.has(&Capability::AiAsk(arg("agent").into()))
                || caps.has(&Capability::AiAsk("*".into()))
        }
        "image.generate" => caps.has(&Capability::ImageGenerate),
        m if m.starts_with("modules.") => caps.has(&Capability::ModulesManage),
        m => match methods::find(m).and_then(|row| row.permission) {
            Some(p) => Capability::parse(p).is_ok_and(|c| caps.has(&c)),
            None => true,
        },
    }
}

/// Who may receive a provider's event: holders of this permission. `None` = not broadcast (an
/// event carrying `to: <lease>` is not broadcast: the router delivers it to that lease's holder,
/// §8.8).
pub fn event_permission(method: &str) -> Option<Capability> {
    if method == "gpu.changed" {
        return Some(Capability::GpuStatus);
    }
    let row = methods::find(method)?;
    if row.kind != Kind::Event {
        return None;
    }
    Capability::parse(row.permission?).ok()
}

/// Whether `key` is an app fact (`apps.seen.<name>` or `apps.running.<name>`, §4.6 v1.7) of an
/// app the grant's `apps.watch:<name>.exe` names: watching an app lets a module read its facts.
pub fn watches_app(grant: &hideout_contract::permission::Grant, key: &str) -> bool {
    use hideout_contract::module::{app_fact_label, app_label};
    use hideout_contract::permission::Permission;
    let Some(label) = (!key.starts_with('!'))
        .then(|| app_fact_label(key))
        .flatten()
    else {
        return false;
    };
    grant.0.iter().any(|p| match p {
        Permission::AppsWatch(exe) => app_label(exe).as_deref() == Some(label),
        _ => false,
    })
}
