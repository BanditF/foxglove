//! Permissions: what a client may touch. Written as strings in `plugin.toml` and in `initialize`
//! (`facts.read:clock.*`, `lighting.write`, ...). The app checks every call against the grant;
//! the user approves a plugin's permissions when installing it, and again when an update adds some.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub enum Permission {
    /// Read facts whose key matches the pattern (`*` alone, or a trailing `.*`).
    FactsRead(String),
    /// Write facts whose key matches the pattern. `device.*` keys are never writable.
    FactsWrite(String),
    /// List devices and read their state (lighting, layers, art status, batteries).
    DevicesRead,
    /// Change lights.
    LightingWrite,
    /// Invoke keymap actions. Actions that need confirmation are always refused for plugins.
    BehaviorsInvoke,
    /// Draw over the left panel for a while.
    ScreensWrite,
    /// Push, select, save and erase right-half art.
    ArtWrite,
    /// Every key press with its position: a keylogger, so the user is warned.
    KeyEvents,
    /// Send raw HID reports with unassigned first bytes. Warned like `key.events`.
    HidRaw,
    /// Switch Foxglove's profile (API 1.1).
    ProfilesWrite,
    /// Any network, unrestricted: the plugin runs outside the sandbox, so the user is warned.
    /// Prefer the scoped forms below.
    Net,
    /// TCP to one host and port, opened by Foxglove for the plugin (`net.connect:<host>:<port>`;
    /// the host may be `*.example.com`, the port `*`). The plugin itself stays sandboxed.
    NetConnect { host: String, port: Option<u16> },
    /// UDP datagrams to one local port, received by Foxglove for the plugin
    /// (`net.listen:udp:127.0.0.1:<port>`): loopback only.
    NetListenUdp(u16),
    /// Declared only: the plugin reads or writes the user's files.
    Files,
    /// Declared: the plugin reads and changes the microphone's mute (Core Audio). Windows lets
    /// any sandboxed process do this, so it's shown at install but not enforced (#215).
    AudioMic,
    /// Keep its own passwords and tokens in Windows' credential store (#213).
    Secrets,
}

impl Permission {
    /// Whether the user gets a warning before granting it.
    pub fn sensitive(&self) -> bool {
        matches!(self, Permission::KeyEvents | Permission::HidRaw | Permission::Net)
    }

    /// Plain words for the Plugins tab's install dialog.
    pub fn describe(&self) -> String {
        match self {
            Permission::FactsRead(p) => format!("read {p}"),
            Permission::FactsWrite(p) => format!("publish {p}"),
            Permission::DevicesRead => "see your keyboards and their state".into(),
            Permission::LightingWrite => "change your keyboards' lights".into(),
            Permission::BehaviorsInvoke => "run your keymap's actions (not risky ones)".into(),
            Permission::ScreensWrite => "draw on the left screen".into(),
            Permission::ArtWrite => "change the right screen's art".into(),
            Permission::KeyEvents => "see every key you press".into(),
            Permission::HidRaw => "send raw messages to your keyboard".into(),
            Permission::ProfilesWrite => "switch your Foxglove profile".into(),
            Permission::Net => "use any network, without the sandbox".into(),
            Permission::NetConnect { host, port } => match port {
                Some(port) => format!("connect to {host} port {port}"),
                None => format!("connect to {host}"),
            },
            Permission::NetListenUdp(port) => format!("receive network messages from this PC on port {port}"),
            Permission::Files => "read and write your files".into(),
            Permission::AudioMic => "see and change whether your microphone is muted".into(),
            Permission::Secrets => "keep its passwords or tokens in Windows' credential store".into(),
        }
    }
}

impl TryFrom<String> for Permission {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let pattern = |p: &str| -> Result<String, String> {
            let ok = p == "*"
                || (!p.is_empty()
                    && p.split('.').enumerate().all(|(i, part)| {
                        !part.is_empty()
                            && (part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                                || (part == "*" && i == p.split('.').count() - 1))
                    }));
            ok.then(|| p.to_string()).ok_or_else(|| format!("bad fact pattern {p:?}"))
        };
        Ok(match text.split_once(':') {
            Some(("facts.read", p)) => Permission::FactsRead(pattern(p)?),
            Some(("facts.write", p)) => {
                let p = pattern(p)?;
                if p == "*" || p == "device" || p.starts_with("device.") {
                    return Err(format!("facts.write:{p}: device facts belong to the app"));
                }
                Permission::FactsWrite(p)
            }
            Some(("net.connect", target)) => {
                let (host, port) = target.rsplit_once(':').ok_or_else(|| format!("{text:?}: write net.connect:<host>:<port>"))?;
                let port = if port == "*" { None } else { Some(port.parse().map_err(|_| format!("{text:?}: bad port"))?) };
                let label = |l: &str| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
                let ok = host.split('.').enumerate().all(|(i, l)| label(l) || (i == 0 && l == "*" && host.contains('.')));
                if !ok || host == "*" {
                    return Err(format!("{text:?}: name one host"));
                }
                Permission::NetConnect { host: host.to_ascii_lowercase(), port }
            }
            Some(("net.listen", target)) => match target.strip_prefix("udp:127.0.0.1:").map(str::parse) {
                Some(Ok(port)) if port != 0 => Permission::NetListenUdp(port),
                _ => return Err(format!("{text:?}: write net.listen:udp:127.0.0.1:<port>")),
            },
            Some(_) => return Err(format!("unknown permission {text:?}")),
            None => match text.as_str() {
                "devices.read" => Permission::DevicesRead,
                "lighting.write" => Permission::LightingWrite,
                "behaviors.invoke" => Permission::BehaviorsInvoke,
                "screens.write" => Permission::ScreensWrite,
                "art.write" => Permission::ArtWrite,
                "key.events" => Permission::KeyEvents,
                "hid.raw" => Permission::HidRaw,
                "profiles.write" => Permission::ProfilesWrite,
                "net" => Permission::Net,
                "files" => Permission::Files,
                "audio.mic" => Permission::AudioMic,
                "secrets" => Permission::Secrets,
                _ => return Err(format!("unknown permission {text:?}")),
            },
        })
    }
}

impl From<Permission> for String {
    fn from(p: Permission) -> String {
        match p {
            Permission::FactsRead(pat) => format!("facts.read:{pat}"),
            Permission::FactsWrite(pat) => format!("facts.write:{pat}"),
            Permission::DevicesRead => "devices.read".into(),
            Permission::LightingWrite => "lighting.write".into(),
            Permission::BehaviorsInvoke => "behaviors.invoke".into(),
            Permission::ScreensWrite => "screens.write".into(),
            Permission::ArtWrite => "art.write".into(),
            Permission::KeyEvents => "key.events".into(),
            Permission::HidRaw => "hid.raw".into(),
            Permission::ProfilesWrite => "profiles.write".into(),
            Permission::Net => "net".into(),
            Permission::NetConnect { host, port } => {
                format!("net.connect:{host}:{}", port.map_or("*".to_string(), |p| p.to_string()))
            }
            Permission::NetListenUdp(port) => format!("net.listen:udp:127.0.0.1:{port}"),
            Permission::Files => "files".into(),
            Permission::AudioMic => "audio.mic".into(),
            Permission::Secrets => "secrets".into(),
        }
    }
}

/// Whether fact `key` matches `pattern`: `*` matches everything, `a.b.*` everything under
/// `a.b.`, anything else only itself.
pub fn key_matches(pattern: &str, key: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some("") => true,
        Some(prefix) if prefix.ends_with('.') => key.starts_with(prefix),
        _ => pattern == key,
    }
}

/// A client's granted permissions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grant(pub Vec<Permission>);

impl Grant {
    pub fn has(&self, p: &Permission) -> bool {
        self.0.contains(p)
    }

    /// Whether a scoped `net.connect` covers `host`:`port` (`*.example.com` covers subdomains
    /// only, not the bare domain).
    pub fn can_connect(&self, host: &str, port: u16) -> bool {
        let host = host.to_ascii_lowercase();
        self.0.iter().any(|p| match p {
            Permission::NetConnect { host: h, port: allowed } => {
                allowed.is_none_or(|a| a == port)
                    && match h.strip_prefix("*.") {
                        Some(domain) => host.ends_with(&format!(".{domain}")),
                        None => *h == host,
                    }
            }
            _ => false,
        })
    }

    pub fn can_listen_udp(&self, port: u16) -> bool {
        self.has(&Permission::NetListenUdp(port))
    }

    /// Whether the plugin needs to run outside the sandbox (unrestricted network or files).
    pub fn unsandboxed(&self) -> bool {
        self.0.iter().any(|p| matches!(p, Permission::Net | Permission::Files))
    }

    pub fn can_read_fact(&self, key: &str) -> bool {
        self.0.iter().any(|p| matches!(p, Permission::FactsRead(pat) if key_matches(pat, key)))
    }

    pub fn can_write_fact(&self, key: &str) -> bool {
        !key.starts_with("device.")
            && self.0.iter().any(|p| matches!(p, Permission::FactsWrite(pat) if key_matches(pat, key)))
    }

    /// Whether a subscription pattern stays inside what may be read: the pattern itself must be
    /// covered by one granted read pattern.
    pub fn can_subscribe(&self, pattern: &str) -> bool {
        self.0.iter().any(|p| match p {
            Permission::FactsRead(granted) => {
                granted == "*"
                    || granted == pattern
                    || granted
                        .strip_suffix('*')
                        .is_some_and(|prefix| prefix.ends_with('.') && pattern.starts_with(prefix))
            }
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Permission, String> {
        Permission::try_from(s.to_string())
    }

    #[test]
    fn parses_round_trips_and_refuses_bad_ones() {
        for s in ["facts.read:clock.*", "facts.write:weather.temp", "lighting.write", "key.events", "facts.read:*", "audio.mic"] {
            assert_eq!(String::from(p(s).unwrap()), s);
        }
        assert!(p("facts.write:device.x").is_err(), "device facts are the app's");
        assert!(p("facts.write:*").is_err());
        assert!(p("facts.read:a.*.b").is_err());
        assert!(p("facts.read:").is_err());
        assert!(p("root").is_err());
        assert!(p("key.events").unwrap().sensitive());
    }

    #[test]
    fn scoped_network_permissions() {
        for s in ["net.connect:127.0.0.1:4455", "net.connect:*.example.com:443", "net.connect:api.example.com:*", "net.listen:udp:127.0.0.1:5568"] {
            assert_eq!(String::from(p(s).unwrap()), s);
        }
        for bad in ["net.connect:*:80", "net.connect:example.com", "net.connect:a b:1", "net.connect:*:*", "net.listen:udp:0.0.0.0:5568", "net.listen:tcp:127.0.0.1:1", "net.listen:udp:127.0.0.1:0"] {
            assert!(p(bad).is_err(), "{bad}");
        }
        let g = Grant(vec![p("net.connect:*.example.com:443").unwrap(), p("net.connect:127.0.0.1:*").unwrap(), p("net.listen:udp:127.0.0.1:5568").unwrap()]);
        assert!(g.can_connect("API.example.com", 443));
        assert!(!g.can_connect("example.com", 443), "a wildcard covers subdomains only");
        assert!(!g.can_connect("evil-example.com", 443));
        assert!(!g.can_connect("api.example.com", 80));
        assert!(g.can_connect("127.0.0.1", 4455));
        assert!(g.can_listen_udp(5568) && !g.can_listen_udp(5569));
        assert!(!g.unsandboxed(), "scoped network keeps the sandbox");
        assert!(Grant(vec![Permission::Net]).unsandboxed() && Permission::Net.sensitive());
    }

    #[test]
    fn grants_match_keys_and_subscriptions() {
        let g = Grant(vec![p("facts.read:media.*").unwrap(), p("facts.write:weather.*").unwrap()]);
        assert!(g.can_read_fact("media.title"));
        assert!(!g.can_read_fact("mediaplayer.title"));
        assert!(!g.can_read_fact("clock.time"));
        assert!(g.can_write_fact("weather.temp"));
        assert!(!g.can_write_fact("device.x.layers"));
        assert!(g.can_subscribe("media.*"));
        assert!(g.can_subscribe("media.title"));
        assert!(!g.can_subscribe("*"));
        assert!(!g.can_subscribe("clock.*"));
    }
}
