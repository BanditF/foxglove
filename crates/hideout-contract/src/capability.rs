//! The §6.1 permission grammar: everything kept from Foxglove API v1 ([`Permission`]) plus the
//! hub's new permissions, and the §6.6 never-list.

use std::fmt;

use crate::error::{fail, Error, ErrorId};
use crate::permission::Permission;

/// A GPU tenant class (`gpu.request:<class>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpuClass {
    Chat,
    Image,
    Any,
}

/// One permission in the hub's grammar.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    /// A Foxglove API v1 permission, unchanged.
    Kept(Permission),
    /// `lighting.lease`
    LightingLease,
    /// `provide:<method pattern>`
    Provide(String),
    /// `channel.open:<module>/<channel>`
    ChannelOpen { module: String, channel: String },
    /// `channel.offer:<channel>`
    ChannelOffer(String),
    /// `device.hid:<usage or catalog class>`
    DeviceHid(String),
    /// `gpu.status`
    GpuStatus,
    /// `gpu.request:<class>`
    GpuRequest(GpuClass),
    /// `ai.ask:<agent>` (`*` for any)
    AiAsk(String),
    /// `image.generate`
    ImageGenerate,
    /// `secrets.ref:<scheme>`
    SecretsRef(String),
    /// `hub.notify`
    HubNotify,
    /// `ui.window`
    UiWindow,
    /// `modules.manage`
    ModulesManage,
}

/// Bodies whose argument a v1 parse error is about (anything else unknown is `permission.unknown`).
const V1_ARGUMENT_BODIES: &[&str] = &[
    "facts.read",
    "facts.write",
    "net.connect",
    "net.listen",
    "apps.watch",
];

fn name_ok(s: &str, extra: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || extra.contains(c))
}

/// A method or fact pattern: dot-separated names, optionally ending in `.*`.
pub fn pattern_ok(p: &str) -> bool {
    let body = p.strip_suffix(".*").unwrap_or(p);
    !body.is_empty() && body.split('.').all(|l| name_ok(l, ""))
}

impl Capability {
    /// Parses one permission string (§6.1). A legacy `foxglove:` prefix is stripped.
    pub fn parse(text: &str) -> Result<Capability, Error> {
        let text = text.strip_prefix("foxglove:").unwrap_or(text);
        let arg = |what: &str| Error::new(ErrorId::PermissionArgument, format!("{text:?}: {what}"));
        let cap = match text.split_once(':') {
            Some(("provide", p)) => {
                if !pattern_ok(p) {
                    return Err(arg("name a method or a prefix ending in .*"));
                }
                Capability::Provide(p.into())
            }
            Some(("channel.open", target)) => {
                let (module, channel) = target
                    .split_once('/')
                    .filter(|(m, c)| name_ok(m, ".") && name_ok(c, "."))
                    .ok_or_else(|| arg("write channel.open:<module>/<channel>"))?;
                Capability::ChannelOpen {
                    module: module.into(),
                    channel: channel.into(),
                }
            }
            Some(("channel.offer", c)) if name_ok(c, ".") => Capability::ChannelOffer(c.into()),
            Some(("device.hid", c)) if name_ok(c, ".:") => Capability::DeviceHid(c.into()),
            Some(("gpu.request", c)) => Capability::GpuRequest(match c {
                "chat" => GpuClass::Chat,
                "image" => GpuClass::Image,
                "any" => GpuClass::Any,
                _ => return Err(arg("class is chat, image or any")),
            }),
            Some(("ai.ask", a)) if a == "*" || name_ok(a, ".") => Capability::AiAsk(a.into()),
            Some(("secrets.ref", s)) if name_ok(s, "") => Capability::SecretsRef(s.into()),
            Some(("channel.offer" | "device.hid" | "ai.ask" | "secrets.ref", _)) => {
                return Err(arg("bad argument"))
            }
            Some(("facts.write", p)) if p == "*" || p == "device" || p.starts_with("device.") => {
                // Spec v1.1 (#918): asking to write device facts or every fact is a never-list item.
                return fail(
                    ErrorId::PermissionNever,
                    format!("{text:?}: device facts belong to the hub"),
                );
            }
            None => match text {
                "lighting.lease" => Capability::LightingLease,
                "gpu.status" => Capability::GpuStatus,
                "image.generate" => Capability::ImageGenerate,
                "hub.notify" => Capability::HubNotify,
                "ui.window" => Capability::UiWindow,
                "modules.manage" => Capability::ModulesManage,
                _ => Capability::kept(text)?,
            },
            Some(_) => Capability::kept(text)?,
        };
        Ok(cap)
    }

    fn kept(text: &str) -> Result<Capability, Error> {
        Permission::try_from(text.to_string())
            .map(Capability::Kept)
            .map_err(|message| {
                let body = text.split_once(':').map_or(text, |(b, _)| b);
                let id = if V1_ARGUMENT_BODIES.contains(&body) {
                    ErrorId::PermissionArgument
                } else {
                    ErrorId::PermissionUnknown
                };
                Error::new(id, message)
            })
    }

    /// Red in the permission dialog (§6.1). `keys`-class channels are sensitive too, but that
    /// comes from the catalog (§6.2).
    pub fn sensitive(&self) -> bool {
        match self {
            Capability::Kept(p) => p.sensitive(),
            Capability::AiAsk(a) => a == "*",
            Capability::SecretsRef(_) => true,
            _ => false,
        }
    }

    /// `key.events` or `hid.raw`: never over MCP, and for third-party or private modules only
    /// when sideloaded or signed by a user key.
    pub fn is_key_level(&self) -> bool {
        matches!(
            self,
            Capability::Kept(Permission::KeyEvents) | Capability::Kept(Permission::HidRaw)
        )
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Capability::Kept(p) => f.write_str(&String::from(p.clone())),
            Capability::LightingLease => f.write_str("lighting.lease"),
            Capability::Provide(p) => write!(f, "provide:{p}"),
            Capability::ChannelOpen { module, channel } => {
                write!(f, "channel.open:{module}/{channel}")
            }
            Capability::ChannelOffer(c) => write!(f, "channel.offer:{c}"),
            Capability::DeviceHid(c) => write!(f, "device.hid:{c}"),
            Capability::GpuStatus => f.write_str("gpu.status"),
            Capability::GpuRequest(c) => write!(
                f,
                "gpu.request:{}",
                match c {
                    GpuClass::Chat => "chat",
                    GpuClass::Image => "image",
                    GpuClass::Any => "any",
                }
            ),
            Capability::AiAsk(a) => write!(f, "ai.ask:{a}"),
            Capability::ImageGenerate => f.write_str("image.generate"),
            Capability::SecretsRef(s) => write!(f, "secrets.ref:{s}"),
            Capability::HubNotify => f.write_str("hub.notify"),
            Capability::UiWindow => f.write_str("ui.window"),
            Capability::ModulesManage => f.write_str("modules.manage"),
        }
    }
}

/// What a program outside Hideout may do with a permission (S1). The hub grants `Free` itself,
/// refuses `Never`, and says so for `Ask` (nothing can allow it yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolClass {
    Free,
    Ask,
    Never,
}

/// The class of `cap` for an outside tool. `risk` is the catalog's class of the channel a
/// `channel.open` names (`None` when the hub doesn't know it). Every variant is listed, so a new
/// one has to be placed here on purpose; nothing unknown is `Free`.
pub fn tool_class(cap: &Capability, risk: Option<&str>) -> ToolClass {
    use ToolClass::{Ask, Free, Never};
    match cap {
        Capability::Kept(p) => match p {
            Permission::FactsRead(pat) => {
                if crate::permission::tool_free_fact(pat) {
                    Free
                } else {
                    Ask
                }
            }
            Permission::DevicesRead | Permission::AudioInputsRead => Free,
            Permission::FactsWrite(_)
            | Permission::LightingWrite
            | Permission::BehaviorsInvoke
            | Permission::ScreensWrite
            | Permission::ArtWrite
            | Permission::ProfilesWrite
            | Permission::FocusWrite
            | Permission::AudioInputsMute
            | Permission::AppWindows
            | Permission::WallpaperControl
            | Permission::AppsWatch(_) => Ask,
            Permission::KeyEvents
            | Permission::HidRaw
            | Permission::Net
            | Permission::NetConnect { .. }
            | Permission::NetListenUdp(_)
            | Permission::NetListenTcp(_)
            | Permission::Files
            | Permission::Secrets => Never,
        },
        Capability::GpuStatus => Free,
        Capability::LightingLease
        | Capability::GpuRequest(_)
        | Capability::ImageGenerate
        | Capability::HubNotify
        | Capability::UiWindow => Ask,
        Capability::AiAsk(a) => {
            if a == "*" {
                Never
            } else {
                Ask
            }
        }
        // Only the catalog's own normal and sensitive classes; keys and unknown are never.
        Capability::ChannelOpen { .. } => match risk {
            Some("normal") | Some("sensitive") => Ask,
            _ => Never,
        },
        Capability::Provide(_)
        | Capability::ChannelOffer(_)
        | Capability::DeviceHid(_)
        | Capability::SecretsRef(_)
        | Capability::ModulesManage => Never,
    }
}

/// Where a module's files came from, for the never-list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A catalog signed by the Hideout root.
    Catalog,
    /// A catalog signed by a user trust anchor (§13.6).
    UserKey,
    /// Developer-mode sideload (§12.6).
    Sideload,
    /// A private source without a signature.
    Unsigned,
}

/// Who asks for a permission (§6.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    /// A first-party module (family hideout, foxglove, vulpes or digitalis).
    FirstParty,
    /// The `hideout.modules` module.
    ModulesModule,
    /// The first-party `hideout` CLI.
    HideoutCli,
    ThirdParty(Source),
    Private(Source),
    /// An outside tool through `initialize`.
    OutsideTool,
    /// An MCP client through the bridge.
    Mcp,
}

/// Whether `holder` may be granted `cap` (§6.6). A never-list item is left out of `granted`.
pub fn check_grantable(cap: &Capability, holder: Holder) -> Result<(), Error> {
    let never = |why: &str| fail(ErrorId::PermissionNever, format!("{cap}: {why}"));
    if *cap == Capability::ModulesManage
        && !matches!(holder, Holder::ModulesModule | Holder::HideoutCli)
    {
        return never("only hideout.modules and the hideout CLI manage modules");
    }
    // Pausing the user's wallpaper is for the user's own tools and modules, not an agent's (#1120).
    if *cap == Capability::Kept(Permission::WallpaperControl) && holder == Holder::Mcp {
        return never("never over MCP");
    }
    if cap.is_key_level() {
        match holder {
            Holder::Mcp => return never("never over MCP"),
            Holder::ThirdParty(s) | Holder::Private(s)
                if !matches!(s, Source::Sideload | Source::UserKey) =>
            {
                return never("only for a sideloaded or user-key-signed module")
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for s in [
            "provide:example.art.*",
            "channel.open:foxglove.example-link/art",
            "gpu.request:any",
            "ai.ask:*",
            "net.connect:127.0.0.1:4455",
            "lighting.lease",
        ] {
            assert_eq!(Capability::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(
            Capability::parse("lighting.read").unwrap_err().id,
            ErrorId::PermissionUnknown
        );
        assert_eq!(
            Capability::parse("facts.write:*").unwrap_err().id,
            ErrorId::PermissionNever
        );
    }

    fn cap(s: &str) -> Capability {
        Capability::parse(s).unwrap()
    }

    /// Every variant has a class, and the ones that matter are pinned.
    #[test]
    fn tool_class_every_variant() {
        use ToolClass::{Ask, Free, Never};
        let rows: &[(&str, Option<&str>, ToolClass)] = &[
            ("devices.read", None, Free),
            ("facts.read:keyboard.connected", None, Free),
            ("facts.read:module.foxglove.state", None, Free),
            ("facts.read:keyboard.layer", None, Ask),
            ("facts.read:keyboard.layers", None, Ask),
            ("facts.read:*", None, Ask),
            ("facts.read:module.*", None, Ask),
            ("facts.read:media.*", None, Ask),
            ("facts.write:media.*", None, Ask),
            ("lighting.write", None, Ask),
            ("lighting.lease", None, Ask),
            ("behaviors.invoke", None, Ask),
            ("screens.write", None, Ask),
            ("art.write", None, Ask),
            ("profiles.write", None, Ask),
            ("focus.write", None, Ask),
            ("app.windows", None, Ask),
            ("wallpaper.control", None, Ask),
            ("apps.watch:example.exe", None, Ask),
            ("audio.inputs.read", None, Free),
            ("audio.inputs.mute", None, Ask),
            ("gpu.status", None, Free),
            ("gpu.request:any", None, Ask),
            ("image.generate", None, Ask),
            ("ai.ask:agent", None, Ask),
            ("ai.ask:*", None, Never),
            ("hub.notify", None, Ask),
            ("ui.window", None, Ask),
            ("key.events", None, Never),
            ("hid.raw", None, Never),
            ("net", None, Never),
            ("net.connect:127.0.0.1:4455", None, Never),
            ("net.listen:udp:127.0.0.1:4455", None, Never),
            ("net.listen:tcp:127.0.0.1:4455", None, Never),
            ("files", None, Never),
            ("secrets", None, Never),
            ("secrets.ref:vault", None, Never),
            ("provide:example.*", None, Never),
            ("channel.offer:art", None, Never),
            ("device.hid:1", None, Never),
            ("modules.manage", None, Never),
            ("channel.open:m/c", Some("normal"), Ask),
            ("channel.open:m/c", Some("sensitive"), Ask),
            ("channel.open:m/c", Some("keys"), Never),
            ("channel.open:m/c", Some("surprise"), Never),
            ("channel.open:m/c", None, Never),
        ];
        for (text, risk, want) in rows {
            assert_eq!(tool_class(&cap(text), *risk), *want, "{text} {risk:?}");
        }
    }

    /// Nothing Free changes anything: only reads, no key level, nothing red.
    #[test]
    fn free_set_is_read_only() {
        let free = [
            "devices.read",
            "audio.inputs.read",
            "gpu.status",
            "facts.read:keyboard.link",
            "facts.read:keyboard.lighting.*",
            "facts.read:sensor.cpu",
            "facts.read:volume.master",
            "facts.read:mic.state",
            "facts.read:module.hideout.modules.state",
            "facts.read:module.foxglove.version",
        ];
        for s in free {
            let c = cap(s);
            assert_eq!(tool_class(&c, None), ToolClass::Free, "{s}");
            assert!(!c.sensitive() && !c.is_key_level(), "{s}");
            let text = c.to_string();
            assert!(
                text == "gpu.status" || text.starts_with("facts.read:") || text.ends_with(".read"),
                "{s}"
            );
        }
        // Wide, wildcard or look-alike patterns are not free.
        for s in [
            "facts.read:*",
            "facts.read:module.*",
            "facts.read:module.*.state",
            "facts.read:module..state",
            "facts.read:sensorx",
            "facts.read:keyboard.*",
            "facts.read:keyboard.layer",
            "facts.read:mic.*",
            "facts.write:sensor.cpu",
        ] {
            if let Ok(c) = Capability::parse(s) {
                assert_ne!(tool_class(&c, None), ToolClass::Free, "{s}");
            }
        }
        // The data list holds only read-side fact space.
        assert!(crate::permission::TOOL_FREE_FACTS
            .iter()
            .all(|e| !e.starts_with('*') && !e.starts_with("apps.") && !e.starts_with("media.")));
    }

    /// #1120: `wallpaper.control` is for the user's own tools, never an MCP client's.
    #[test]
    fn wallpaper_control_is_never_over_mcp() {
        let cap = Capability::parse("wallpaper.control").unwrap();
        assert!(!cap.sensitive());
        assert_eq!(
            check_grantable(&cap, Holder::Mcp).unwrap_err().id,
            ErrorId::PermissionNever
        );
        for holder in [Holder::OutsideTool, Holder::FirstParty, Holder::HideoutCli] {
            assert!(check_grantable(&cap, holder).is_ok());
        }
    }
}
