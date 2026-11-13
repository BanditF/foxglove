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
    // The app watch's call and events (§8.12, v1.13).
    "apps.*",
    "app.window.*",
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
    // Every other area of Foxglove's v1 method table (`foxglove_plugin_api::methods`, #1156):
    // the app serves these, so no third-party or other adopted module may provide them, as
    // methods or as facts. A test derives the area list from the table, so it cannot drift.
    "key.*",
    "secrets.*",
    "lights.*",
    "scenes.*",
    "pc_lighting.*",
    "health.*",
    "tray.*",
    "overlay.*",
    "wallpaper-engine.*",
];

/// The 11 v1 methods the legacy adopted module `foxglove` serves through the router today
/// (#1143 P6). TEMPORARY (#1155): until the user can grant the legacy module its first-party
/// areas explicitly, [`legacy_foxglove_ok`] lets exactly these names through the reserved
/// areas above, and only for the adopted module `foxglove` holding the user's `provide:` for
/// each. `hub_link::SERVED` must equal this list (a test pins it).
pub const LEGACY_FOXGLOVE_METHODS: &[&str] = &[
    "health.read",
    "lights.page",
    "scenes.page",
    "overlay.window",
    "overlay.restore",
    "overlay.shown",
    "overlay.set_shown",
    "tray.panel",
    "tray.set_panel",
    "pc_lighting.list",
    "pc_lighting.set_zone",
];

/// The id of the legacy module (#1155).
pub const LEGACY_FOXGLOVE_ID: &str = "foxglove";

/// Whether `pattern` is one of the legacy methods, exactly (never a wildcard or a longer
/// pattern that merely covers one). The caller must also have checked that `module` is the
/// adopted module with that id; the id alone grants nothing (#1155, #1156).
pub fn legacy_foxglove_ok(module: &str, pattern: &str) -> bool {
    module == LEGACY_FOXGLOVE_ID && LEGACY_FOXGLOVE_METHODS.contains(&pattern)
}

/// Words that make a method name one the keyboard grant never carries (#1155-B): nothing that
/// resets, flashes, erases or re-bonds a device, or enters its bootloader, is routed to the
/// legacy module through the grant, whatever area it lies in.
pub const GRANT_NEVER: &[&str] = &["reset", "bootloader", "flash", "erase", "dfu", "bond"];

/// Whether a method name or pattern has a [`GRANT_NEVER`] word as a part (split at anything that
/// is not a letter or digit, so `devices.reset_all` and `devices.reset-all` both count; case
/// ignored).
pub fn never_name(method: &str) -> bool {
    method
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| GRANT_NEVER.contains(&w) || w.starts_with("bootload"))
}

/// The first-party areas an adopted module may still provide when no catalog gives them to
/// another module: Arete's hub is the GPU broker (§8.9). Every other first-party area is closed
/// to it, with or without a root catalog (§6.3).
pub const ADOPTED_AREAS: &[&str] = &["gpu.*"];

/// Whether an adopted module may provide `pattern` as far as the first-party areas go.
pub fn adopted_area_ok(pattern: &str) -> bool {
    !FIRST_PARTY_AREAS.iter().any(|a| overlap(a, pattern))
        || ADOPTED_AREAS.iter().any(|a| covers(a, pattern))
}

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
        // A routed name with no row in the method table, or a row without a permission, is open
        // to every caller (#1156 LOW, left as is on purpose): it is how a module's own methods
        // (outside the v1 table) are callable at all, and the contract has no caller grant for
        // them yet. It reaches nobody it should not: every v1 area is reserved
        // ([`FIRST_PARTY_AREAS`], [`HUB_OWNED`]), so a name outside the table can only be routed
        // to a module that is allowed to provide it, and a test pins that every v1 method is
        // covered by a reserved area. Gating module-defined methods per caller is future work.
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
