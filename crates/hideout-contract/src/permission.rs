//! Permissions: what a client may touch. Written as strings in `plugin.toml` and in `initialize`
//! (`facts.read:clock.*`, `lighting.write`, ...). The app checks every call against the grant;
//! the user approves a plugin's permissions when installing it, and again when an update adds some.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
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
    /// Draw over the Info screen for a while.
    ScreensWrite,
    /// Push, select, save and erase right-half art.
    ArtWrite,
    /// Every key press with its position: a keylogger, so the user is warned.
    KeyEvents,
    /// Send raw HID reports with unassigned first bytes. Warned like `key.events`.
    HidRaw,
    /// Switch Foxglove's profile (API 1.1).
    ProfilesWrite,
    /// Choose which keyboard Foxglove shows, and pin the overlay's or feed's (API 1.12, #799).
    FocusWrite,
    /// Any network, unrestricted: the plugin runs outside the sandbox, so the user is warned.
    /// Prefer the scoped forms below.
    Net,
    /// TCP to one host and port, opened by Foxglove for the plugin (`net.connect:<host>:<port>`;
    /// the host may be `*.example.com`, the port `*`). The plugin itself stays sandboxed.
    NetConnect { host: String, port: Option<u16> },
    /// UDP datagrams to one local port, received by Foxglove for the plugin
    /// (`net.listen:udp:127.0.0.1:<port>`): loopback only.
    NetListenUdp(u16),
    /// TCP connections to one local port, accepted by Foxglove for the plugin
    /// (`net.listen:tcp:127.0.0.1:<port>`, API 1.3): loopback only, for a local page or tool
    /// (a Wallpaper Engine wallpaper, #298) to reach the plugin.
    NetListenTcp(u16),
    /// Declared only: the plugin reads or writes the user's files.
    Files,
    /// Read the microphone's state (`mic.*` facts), which Foxglove publishes (#237).
    AudioInputsRead,
    /// Mute or unmute the microphone through Foxglove (`audio.inputs.set_mute`, #237).
    AudioInputsMute,
    /// Keep its own passwords and tokens in Windows' credential store (#213).
    Secrets,
    /// Open and close Foxglove's tray panel and bring its overlay window back (API 1.6, #618).
    AppWindows,
    /// Hold Wallpaper Engine paused through Foxglove, which sends the commands (API 1.15,
    /// #1120). Never over MCP (`check_grantable`).
    WallpaperControl,
    /// Be told when a program's windows open and close (`apps.watch:<name>.exe`, API 1.3), so a
    /// plugin waits for an app instead of polling for it (#277).
    AppsWatch(String),
}

impl Permission {
    /// Whether the user gets a warning before granting it.
    pub fn sensitive(&self) -> bool {
        matches!(
            self,
            Permission::KeyEvents | Permission::HidRaw | Permission::Net
        )
    }

    /// Plain words for the Plugins tab's install dialog. No line names a file: an app watch says
    /// the app's name without `.exe` (#991). [`Permission::describe_spelled`] keeps a manifest's
    /// own spelling of that name.
    pub fn describe(&self) -> String {
        self.describe_spelled(&[])
    }

    /// [`Permission::describe`], with an app watch's name spelled as one of `names` spells it
    /// when one matches it without regard to case (`ExampleApp.exe` gives "see whether
    /// ExampleApp is running", where the stored `exampleapp.exe` alone gives "exampleapp").
    pub fn describe_spelled(&self, names: &[&str]) -> String {
        match self {
            Permission::FactsRead(p) => fact_words(true, p),
            Permission::FactsWrite(p) => fact_words(false, p),
            Permission::DevicesRead => "see your keyboards and their state".into(),
            Permission::LightingWrite => "change your keyboards' and PC's lights".into(),
            Permission::BehaviorsInvoke => "run your keymap's actions (not risky ones)".into(),
            Permission::ScreensWrite => "draw on the keyboard's info screen".into(),
            Permission::ArtWrite => "change the art screen's art".into(),
            Permission::KeyEvents => "see every key you press".into(),
            Permission::HidRaw => "send raw messages to your keyboard".into(),
            Permission::ProfilesWrite => "switch your Foxglove profile".into(),
            Permission::FocusWrite => "choose which keyboard Foxglove shows".into(),
            Permission::Net => "use any network, without the sandbox".into(),
            Permission::NetConnect { host, port } => match port {
                Some(port) => format!("connect to {host} port {port}"),
                None => format!("connect to {host}"),
            },
            Permission::NetListenUdp(port) => {
                format!("receive network messages from this PC on port {port}")
            }
            Permission::NetListenTcp(port) => {
                format!("accept connections from this PC on port {port}")
            }
            Permission::Files => "read and write your files".into(),
            Permission::AudioInputsRead => "see whether your microphone is muted".into(),
            Permission::AudioInputsMute => "mute and unmute your microphone".into(),
            Permission::Secrets => {
                "keep its passwords or tokens in Windows' credential store".into()
            }
            Permission::AppsWatch(exe) => {
                format!("see whether {} is running", watched_name(exe, names))
            }
            Permission::AppWindows => "open and close Foxglove's tray panel and overlay".into(),
            Permission::WallpaperControl => "pause and resume your live wallpaper".into(),
        }
    }
}

impl Permission {
    /// [`Permission::describe_spelled`] for a host that tells a plugin when an app's windows
    /// open and close rather than whether it runs (Foxglove's plugin host, `app.window.appeared`
    /// and `app.window.closed`, #277): an app watch reads "see when ExampleApp opens or closes".
    /// Every other line is the same, and none names a file (#991).
    pub fn describe_windows_spelled(&self, names: &[&str]) -> String {
        match self {
            Permission::AppsWatch(exe) => {
                format!("see when {} opens or closes", watched_name(exe, names))
            }
            other => other.describe_spelled(names),
        }
    }
}

impl Permission {
    /// [`Permission::describe_spelled`], with an app watch's app called `program` when the hub
    /// found its friendly name (its exe's version info, #991: "see whether Example Studio is
    /// running"). `None`, or any other permission, reads as `describe_spelled(names)`. A name
    /// that ends in `.exe` is ignored, so no line names a file.
    pub fn describe_with_program(&self, names: &[&str], program: Option<&str>) -> String {
        match (self, program.map(str::trim)) {
            (Permission::AppsWatch(_), Some(n))
                if !n.is_empty() && !n.to_ascii_lowercase().contains(".exe") =>
            {
                format!("see whether {n} is running")
            }
            _ => self.describe_spelled(names),
        }
    }

    /// The app an app watch is about: its exe name without `.exe` (`obs64`), for a friendly
    /// name lookup. `None` for every other permission.
    pub fn watched_app(&self) -> Option<&str> {
        match self {
            Permission::AppsWatch(exe) => Some(exe.strip_suffix(".exe").unwrap_or(exe)),
            _ => None,
        }
    }
}

/// The fact patterns a program outside Hideout may read without asking (`capability::tool_class`).
/// Device or PC state that only the hub or a first-party app writes. An entry without `*` is one
/// exact key; an entry ending in `.*` covers that prefix; `<id>` stands for one app id (no `*`).
/// Anything not covered here is not free: a new fact is never free by default. Not here on
/// purpose: `module.<id>.version` (text the app itself sends) and the rest of `mic.*` (the
/// microphone's name can be personal).
pub const TOOL_FREE_FACTS: &[&str] = &[
    "keyboard.connected",
    "keyboard.link",
    "keyboard.right.battery",
    "keyboard.game_mode",
    "keyboard.profile",
    "keyboard.profile_connected",
    "keyboard.lighting.*",
    "sensor.*",
    "volume.*",
    "mic.state",
    "module.<id>.state",
];

/// Image file names (without `.exe`, lower case) of programs that run whatever script they are
/// handed. The hub never remembers a grant for one: the file says nothing about what runs in it.
/// One list, here; the router reads it through [`tool_runs_scripts`].
pub const TOOL_SCRIPT_RUNNERS: &[&str] = &[
    "python",
    "pythonw",
    "py",
    "pyw",
    "pypy",
    "pypyw",
    "node",
    "nodejs",
    "deno",
    "bun",
    "powershell",
    "powershell_ise",
    "pwsh",
    "pwsh-preview",
    "python_d",
    "pythonw_d",
    "cmd",
    "wscript",
    "cscript",
    "mshta",
    "java",
    "javaw",
    "jshell",
    "ruby",
    "rubyw",
    "perl",
    "wperl",
    "php",
    "php-cgi",
    "lua",
    "luajit",
    "bash",
    "sh",
    "wsl",
    "rscript",
    "julia",
    "tclsh",
    "wish",
    "csi",
    "fsi",
    "electron",
    "nw",
    "autoit3",
    "autohotkey",
    // Loaders: they run code from a file or an argument they are given.
    "rundll32",
    "regsvr32",
    "dotnet",
    "msbuild",
    "installutil",
    "regasm",
    "msiexec",
    "dllhost",
];

/// Names that run scripts under any suffix: the stem starts with one of these and what follows
/// is digits, dots, dashes or underscores (`autohotkey64`, `AutoHotkeyU64`).
pub const TOOL_SCRIPT_RUNNER_PREFIXES: &[&str] = &["autohotkey"];

/// Whether the image file name `file` (for example `Python.EXE`) is a script runner. A stem is a
/// runner when it is in [`TOOL_SCRIPT_RUNNERS`], or is one of them followed only by digits and
/// dots (`python3.12`, `lua54`, `pypy3`), or starts with one of
/// [`TOOL_SCRIPT_RUNNER_PREFIXES`]. A renamed copy of a runner is not caught by a name list; that is a known
/// limit, and the dialog says the program runs scripts only when the name says so.
pub fn tool_runs_scripts(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    let stem = lower.strip_suffix(".exe").unwrap_or(&lower);
    if TOOL_SCRIPT_RUNNERS.contains(&stem) {
        return true;
    }
    if TOOL_SCRIPT_RUNNER_PREFIXES
        .iter()
        .any(|p| stem.starts_with(p))
    {
        return true;
    }
    let versioned = |s: &str| {
        let base = s.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        base.len() < s.len() && !base.is_empty() && TOOL_SCRIPT_RUNNERS.contains(&base)
    };
    if versioned(stem) {
        return true;
    }
    // One letter after the version: `python3.13t` (free-threaded), `python3.12d` (debug).
    stem.strip_suffix(|c: char| c.is_ascii_lowercase())
        .is_some_and(|s| s.ends_with(|c: char| c.is_ascii_digit()) && versioned(s))
}

/// Whether the fact pattern `pattern` (as asked in `facts.read:<pattern>`) lies wholly inside
/// [`TOOL_FREE_FACTS`].
pub fn tool_free_fact(pattern: &str) -> bool {
    TOOL_FREE_FACTS.iter().any(|e| {
        if let Some((pre, post)) = e.split_once("<id>") {
            return pattern
                .strip_prefix(pre)
                .and_then(|r| r.strip_suffix(post))
                .is_some_and(|id| !id.is_empty() && !id.contains('*'));
        }
        match e.strip_suffix('*') {
            Some(prefix) => pattern.starts_with(prefix),
            None => pattern == *e,
        }
    })
}

/// Plain words for a fact pattern in a consent line: (pattern, what reading it is, what
/// publishing it is). Display only: a grant is always for the exact pattern string, whatever
/// these words say, and a lookup is an exact, case-sensitive match of the whole pattern, so a
/// key's text never widens or narrows anything. A pattern that ends in `*` must read wider than
/// any exact key under it ("all of ..."); a test keeps that so. No line shows a dotted key.
pub const FACT_WORDS: &[(&str, &str, &str)] = &[
    (
        "keyboard.game_mode",
        "see whether game mode is on",
        "share whether game mode is on",
    ),
    (
        "keyboard.connected",
        "see whether your keyboard is connected",
        "share whether your keyboard is connected",
    ),
    (
        "keyboard.layer",
        "see which layer your keyboard is on",
        "share which layer your keyboard is on",
    ),
    (
        "keyboard.layers",
        "see which of your keyboard's layers are on",
        "share which of your keyboard's layers are on",
    ),
    (
        "keyboard.right.battery",
        "see the battery level of your keyboard's right half",
        "share the battery level of your keyboard's right half",
    ),
    (
        "keyboard.link",
        "see whether your keyboard is on USB or Bluetooth",
        "share whether your keyboard is on USB or Bluetooth",
    ),
    (
        "keyboard.profile",
        "see which Bluetooth profile your keyboard uses",
        "share which Bluetooth profile your keyboard uses",
    ),
    (
        "keyboard.profile_connected",
        "see whether your keyboard's current Bluetooth profile is connected",
        "share whether your keyboard's current Bluetooth profile is connected",
    ),
    (
        "keyboard.lighting.on",
        "see whether your keyboard's lights are on",
        "share whether your keyboard's lights are on",
    ),
    (
        "keyboard.lighting.color",
        "see your keyboard's light colour",
        "share your keyboard's light colour",
    ),
    (
        "keyboard.lighting.brightness",
        "see your keyboard's light brightness",
        "share your keyboard's light brightness",
    ),
    (
        "keyboard.lighting.effect",
        "see your keyboard's light effect",
        "share your keyboard's light effect",
    ),
    (
        "keyboard.lighting.rainbow",
        "see whether your keyboard's light effect makes its own colours",
        "share whether your keyboard's light effect makes its own colours",
    ),
    (
        "keyboard.lighting.*",
        "see all of your keyboard's light settings",
        "share all of your keyboard's light settings",
    ),
    (
        "keyboard.*",
        "see all of your keyboard's status (layer, battery, lights and more)",
        "share all of your keyboard's status (layer, battery, lights and more)",
    ),
    (
        "module.foxglove.state",
        "see whether Foxglove is running",
        "share whether Foxglove is running",
    ),
    (
        "module.*",
        "see every app Hideout runs and whether it is running",
        "share the state of every app Hideout runs",
    ),
    (
        "sensor.cpu.load_pct",
        "see how busy your processor is",
        "share how busy your processor is",
    ),
    (
        "sensor.cpu.state",
        "see whether your processor's reading is available",
        "share whether your processor's reading is available",
    ),
    (
        "sensor.cpu.*",
        "see all of your processor's readings",
        "share all of your processor's readings",
    ),
    (
        "sensor.gpu.load_pct",
        "see how busy your graphics card is",
        "share how busy your graphics card is",
    ),
    (
        "sensor.gpu.temp_c",
        "see your graphics card's temperature",
        "share your graphics card's temperature",
    ),
    (
        "sensor.gpu.clock_mhz",
        "see your graphics card's speed",
        "share your graphics card's speed",
    ),
    (
        "sensor.gpu.power_w",
        "see how much power your graphics card uses",
        "share how much power your graphics card uses",
    ),
    (
        "sensor.gpu.fan_rpm",
        "see how fast your graphics card's fan spins",
        "share how fast your graphics card's fan spins",
    ),
    (
        "sensor.gpu.fan_pct",
        "see how hard your graphics card's fan works",
        "share how hard your graphics card's fan works",
    ),
    (
        "sensor.gpu.state",
        "see whether your graphics card's readings are available",
        "share whether your graphics card's readings are available",
    ),
    (
        "sensor.gpu.*",
        "see all of your graphics card's readings",
        "share all of your graphics card's readings",
    ),
    (
        "sensor.ram.used_pct",
        "see how much of your memory is in use",
        "share how much of your memory is in use",
    ),
    (
        "sensor.ram.state",
        "see whether your memory reading is available",
        "share whether your memory reading is available",
    ),
    (
        "sensor.ram.*",
        "see all of your memory's readings",
        "share all of your memory's readings",
    ),
    (
        "sensor.*",
        "see all of your PC's readings (processor, memory and graphics card: load, temperature, speed, power and fan)",
        "share all of your PC's readings (processor, memory and graphics card: load, temperature, speed, power and fan)",
    ),
    (
        "media.title",
        "see the title of what is playing",
        "share the title of what is playing",
    ),
    (
        "media.artist",
        "see the artist of what is playing",
        "share the artist of what is playing",
    ),
    (
        "media.playing",
        "see whether media is playing",
        "share whether media is playing",
    ),
    (
        "media.*",
        "see all of what is playing (title, artist and more)",
        "share all of what is playing (title, artist and more)",
    ),
    (
        "volume.level",
        "see your volume level",
        "share your volume level",
    ),
    (
        "volume.muted",
        "see whether your volume is muted",
        "share whether your volume is muted",
    ),
    (
        "volume.*",
        "see all of your volume settings",
        "share all of your volume settings",
    ),
    (
        "mic.state",
        "see whether your microphone is muted",
        "share whether your microphone is muted",
    ),
    (
        "mic.*",
        "see all of your microphone's state (muted, which one and more)",
        "share all of your microphone's state (muted, which one and more)",
    ),
];

/// A fact pattern in plain words, starting with the verb ("see ...", "share ..."). A known
/// pattern uses [`FACT_WORDS`]. Any other still names the key, in a sentence: an exact key reads
/// `see the status called “x.y”`, a prefix `see every status whose name starts with “x.”`, and
/// `*` `see every status any app shares`. A wildcard always says "every" or "all of". The key is
/// whatever its sender wrote, so it is shown cleaned: one line, no hidden characters, no quote
/// marks of its own, at most [`FACT_NAME_MAX`] characters; a key with nothing left to show gets
/// plain words instead of an empty pair of quotes.
pub fn fact_words(read: bool, pattern: &str) -> String {
    if let Some((_, r, w)) = FACT_WORDS.iter().find(|(p, _, _)| *p == pattern) {
        return (if read { *r } else { *w }).to_string();
    }
    let verb = if read { "see" } else { "share" };
    let shown = |s: &str| crate::module::quoted_line(s, FACT_NAME_MAX);
    match pattern.strip_suffix('*') {
        Some("") => format!("{verb} every status any app shares"),
        Some(prefix) => match shown(prefix) {
            Some(q) => format!("{verb} every status whose name starts with {q}"),
            None => format!("{verb} every status whose name Hideout can't show"),
        },
        None => match shown(pattern) {
            Some(q) => format!("{verb} the status called {q}"),
            None => format!("{verb} a status with no name"),
        },
    }
}

/// The most characters of a status key or pattern [`fact_words`] shows; a longer one is cut.
pub const FACT_NAME_MAX: usize = 60;

/// Whether no `facts.write` permission can cover a write to `key`: the device's own statuses
/// (§6.6) are set by the app registered for them and nobody else. [`Grant::can_write_fact`]
/// and the words for a refused write both read this one rule.
pub fn fact_write_never_granted(key: &str) -> bool {
    key.starts_with("device.")
}

/// Every line [`fact_words`] says for the table, plus one of each fallback shape, for the copy
/// tests.
pub fn fact_lines() -> Vec<String> {
    let mut v = Vec::new();
    for p in FACT_WORDS
        .iter()
        .map(|(p, _, _)| *p)
        .chain(["x.y", "x.*", "*"])
    {
        v.push(fact_words(true, p));
        v.push(fact_words(false, p));
    }
    v
}

/// An app watch's app for a permission line: `exe` without `.exe`, spelled as one of `names`
/// spells it when one matches it without regard to case.
fn watched_name(exe: &str, names: &[&str]) -> String {
    let bare = |n: &str| {
        let n = n.trim();
        let cut = n.len().saturating_sub(4);
        match n.get(cut..) {
            Some(tail) if tail.eq_ignore_ascii_case(".exe") => n[..cut].to_string(),
            _ => n.to_string(),
        }
    };
    let stored = bare(exe);
    names
        .iter()
        .map(|n| bare(n))
        .find(|n| n.eq_ignore_ascii_case(&stored))
        .unwrap_or(stored)
}

impl TryFrom<String> for Permission {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let pattern = |p: &str| -> Result<String, String> {
            let ok = p == "*"
                || (!p.is_empty()
                    && p.split('.').enumerate().all(|(i, part)| {
                        !part.is_empty()
                            && (part
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                                || (part == "*" && i == p.split('.').count() - 1))
                    }));
            ok.then(|| p.to_string())
                .ok_or_else(|| format!("bad fact pattern {p:?}"))
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
                let (host, port) = target
                    .rsplit_once(':')
                    .ok_or_else(|| format!("{text:?}: write net.connect:<host>:<port>"))?;
                let port = if port == "*" {
                    None
                } else {
                    Some(port.parse().map_err(|_| format!("{text:?}: bad port"))?)
                };
                let label = |l: &str| {
                    !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                };
                let ok = host
                    .split('.')
                    .enumerate()
                    .all(|(i, l)| label(l) || (i == 0 && l == "*" && host.contains('.')));
                if !ok || host == "*" {
                    return Err(format!("{text:?}: name one host"));
                }
                // A wildcard covers names, never addresses: `*.0.0.1` or `*.1` would cover
                // 127.0.0.1 and every x.x.x.1 (contract §6.1, §8.10, #1107 review).
                let numeric = |l: &str| l.chars().all(|c| c.is_ascii_digit());
                if let Some(rest) = host.strip_prefix("*.") {
                    if rest.rsplit('.').next().is_some_and(numeric) {
                        return Err(format!(
                            "{text:?}: a wildcard names a domain, not an address"
                        ));
                    }
                }
                Permission::NetConnect {
                    host: host.to_ascii_lowercase(),
                    port,
                }
            }
            Some(("net.listen", target)) => {
                let port = |prefix: &str| {
                    target
                        .strip_prefix(prefix)
                        .and_then(|p| p.parse::<u16>().ok())
                        .filter(|p| *p != 0)
                };
                match (port("udp:127.0.0.1:"), port("tcp:127.0.0.1:")) {
                    (Some(p), _) => Permission::NetListenUdp(p),
                    (_, Some(p)) => Permission::NetListenTcp(p),
                    _ => return Err(format!("{text:?}: write net.listen:udp:127.0.0.1:<port> or net.listen:tcp:127.0.0.1:<port>")),
                }
            }
            Some(("apps.watch", exe)) => {
                let exe = exe.to_ascii_lowercase();
                let name = exe.strip_suffix(".exe").unwrap_or_default();
                if name.is_empty()
                    || exe.len() > 64
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._- ".contains(c))
                {
                    return Err(format!("{text:?}: write apps.watch:<name>.exe"));
                }
                Permission::AppsWatch(exe)
            }
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
                "focus.write" => Permission::FocusWrite,
                "net" => Permission::Net,
                "files" => Permission::Files,
                "audio.inputs.read" => Permission::AudioInputsRead,
                "audio.inputs.mute" => Permission::AudioInputsMute,
                "secrets" => Permission::Secrets,
                "app.windows" => Permission::AppWindows,
                "wallpaper.control" => Permission::WallpaperControl,
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
            Permission::FocusWrite => "focus.write".into(),
            Permission::Net => "net".into(),
            Permission::NetConnect { host, port } => {
                format!(
                    "net.connect:{host}:{}",
                    port.map_or("*".to_string(), |p| p.to_string())
                )
            }
            Permission::NetListenUdp(port) => format!("net.listen:udp:127.0.0.1:{port}"),
            Permission::NetListenTcp(port) => format!("net.listen:tcp:127.0.0.1:{port}"),
            Permission::Files => "files".into(),
            Permission::AudioInputsRead => "audio.inputs.read".into(),
            Permission::AudioInputsMute => "audio.inputs.mute".into(),
            Permission::Secrets => "secrets".into(),
            Permission::AppsWatch(exe) => format!("apps.watch:{exe}"),
            Permission::AppWindows => "app.windows".into(),
            Permission::WallpaperControl => "wallpaper.control".into(),
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
            Permission::NetConnect {
                host: h,
                port: allowed,
            } => {
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

    pub fn can_listen_tcp(&self, port: u16) -> bool {
        self.has(&Permission::NetListenTcp(port))
    }

    /// Whether the plugin needs to run outside the sandbox (unrestricted network or files).
    pub fn unsandboxed(&self) -> bool {
        self.0
            .iter()
            .any(|p| matches!(p, Permission::Net | Permission::Files))
    }

    pub fn can_read_fact(&self, key: &str) -> bool {
        self.0.iter().any(|p| match p {
            Permission::FactsRead(pat) => key_matches(pat, key),
            Permission::AudioInputsRead => key_matches("mic.*", key),
            _ => false,
        })
    }

    pub fn can_write_fact(&self, key: &str) -> bool {
        !fact_write_never_granted(key)
            && self
                .0
                .iter()
                .any(|p| matches!(p, Permission::FactsWrite(pat) if key_matches(pat, key)))
    }

    /// Whether a subscription pattern stays inside what may be read: the pattern itself must be
    /// covered by one granted read pattern.
    pub fn can_subscribe(&self, pattern: &str) -> bool {
        self.0.iter().any(|p| match p {
            Permission::AudioInputsRead => {
                pattern == "mic.*" || (pattern.starts_with("mic.") && !pattern.contains('*'))
            }
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
    /// Versioned names, loaders and the other runners are script runners; a plain tool is not.
    #[test]
    fn script_runners_cover_versions_loaders_and_more() {
        for f in [
            "python3.12.exe",
            "Python313.EXE",
            "pyw.exe",
            "pypy3.exe",
            "luajit.exe",
            "lua54.exe",
            "rubyw.exe",
            "wperl.exe",
            "php-cgi.exe",
            "jshell.exe",
            "rundll32.exe",
            "regsvr32.exe",
            "dotnet.exe",
            "msbuild.exe",
            "installutil.exe",
            "regasm.exe",
            "msiexec.exe",
            "dllhost.exe",
            "AutoHotkey64.exe",
            "AutoHotkeyU32.exe",
            "AutoIt3.exe",
            "Rscript.exe",
            "julia.exe",
            "tclsh86.exe",
            "wish.exe",
            "csi.exe",
            "fsi.exe",
            "electron.exe",
            "nw.exe",
            "python3.13t.exe",
            "Python3.12d.exe",
            "python_d.exe",
            "pwsh-preview.exe",
        ] {
            assert!(tool_runs_scripts(f), "{f}");
        }
        for f in [
            "mytool.exe",
            "pythonista.exe",
            "shell.exe",
            "notes.exe",
            "spy.exe",
            "mytool3t.exe",
        ] {
            assert!(!tool_runs_scripts(f), "{f}");
        }
    }

    use super::*;

    fn p(s: &str) -> Result<Permission, String> {
        Permission::try_from(s.to_string())
    }

    fn dotted(line: &str) -> bool {
        line.split_whitespace().any(|w| {
            let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_');
            w.contains('.') && w.chars().any(|c| c.is_ascii_alphabetic()) && !w.ends_with('.')
        })
    }

    #[test]
    fn every_known_fact_has_plain_words_and_no_raw_key() {
        let mut seen = std::collections::HashSet::new();
        for (pat, read, write) in FACT_WORDS {
            assert!(seen.insert(*pat), "{pat} twice");
            assert!(p(&format!("facts.read:{pat}")).is_ok(), "{pat}");
            for (verb, line) in [("see", read), ("share", write)] {
                assert!(line.starts_with(verb), "{pat}: {line}");
                assert!(!line.contains(pat), "{pat} shown raw in {line}");
                assert!(!dotted(line), "{pat}: dotted key in {line}");
                assert!(!line.contains('_'), "{pat}: {line}");
                assert!(!line.to_lowercase().contains("module"), "{pat}: {line}");
            }
            // The lines are what describe() says.
            assert_eq!(p(&format!("facts.read:{pat}")).unwrap().describe(), *read);
            assert_eq!(p(&format!("facts.write:{pat}")).unwrap().describe(), *write);
        }
        // The keys the sensors module and Foxglove's link ask for.
        for pat in ["keyboard.game_mode", "sensor.*", "keyboard.*", "module.*"] {
            assert!(seen.contains(pat), "{pat} missing");
        }
        assert_eq!(
            p("facts.read:keyboard.game_mode").unwrap().describe(),
            "see whether game mode is on"
        );
    }

    #[test]
    fn a_wildcard_reads_wider_than_an_exact_key() {
        for (pat, read, write) in FACT_WORDS {
            for line in [read, write] {
                let wide = line.contains("all of") || line.contains("every");
                assert_eq!(wide, pat.ends_with('*'), "{pat}: {line}");
            }
        }
        // A wildcard line never equals the line of any exact key.
        let exact: Vec<&str> = FACT_WORDS
            .iter()
            .filter(|(p, _, _)| !p.ends_with('*'))
            .map(|(_, r, _)| *r)
            .collect();
        for (pat, read, _) in FACT_WORDS {
            if pat.ends_with('*') {
                assert!(!exact.contains(read), "{pat}");
            }
        }
        // The same holds for the fallback.
        for (exact, wild) in [("x.y", "x.*"), ("x.y", "*"), ("x.*", "*")] {
            for read in [true, false] {
                assert_ne!(fact_words(read, exact), fact_words(read, wild));
            }
        }
    }

    #[test]
    fn an_unknown_fact_names_its_key_in_a_sentence() {
        assert_eq!(
            fact_words(true, "weather.temp"),
            "see the status called \u{201c}weather.temp\u{201d}"
        );
        assert_eq!(
            fact_words(false, "weather.temp"),
            "share the status called \u{201c}weather.temp\u{201d}"
        );
        assert_eq!(
            fact_words(true, "weather.*"),
            "see every status whose name starts with \u{201c}weather.\u{201d}"
        );
        assert_eq!(
            fact_words(false, "weather.*"),
            "share every status whose name starts with \u{201c}weather.\u{201d}"
        );
        assert_eq!(fact_words(true, "*"), "see every status any app shares");
        assert_eq!(fact_words(false, "*"), "share every status any app shares");
        // A lookup is the whole pattern: a near miss is not a known key.
        assert!(fact_words(true, "Keyboard.game_mode").contains("Keyboard.game_mode"));
        assert!(fact_words(true, "keyboard.game_mode.x").contains("keyboard.game_mode.x"));
        assert!(fact_words(true, "keyboard.game_mod").contains("keyboard.game_mod"));
        // Read and write never say the same thing.
        for (pat, r, w) in FACT_WORDS {
            assert_ne!(r, w, "{pat}");
        }
        assert_ne!(fact_words(true, "a.b"), fact_words(false, "a.b"));
    }

    #[test]
    fn the_words_never_change_what_is_granted() {
        // The raw string is the grant: words are looked up from it and never parsed back.
        for s in [
            "facts.read:keyboard.*",
            "facts.write:sensor.*",
            "facts.read:x.y",
        ] {
            let perm = p(s).unwrap();
            let _ = perm.describe();
            assert_eq!(String::from(perm), s);
        }
        let narrow = p("facts.read:keyboard.game_mode").unwrap();
        let wide = p("facts.read:keyboard.*").unwrap();
        assert_ne!(narrow, wide);
        assert_ne!(narrow.describe(), wide.describe());
    }

    #[test]
    fn no_permission_line_names_a_file() {
        let all = [
            "facts.read:clock.*",
            "facts.write:weather.temp",
            "devices.read",
            "lighting.write",
            "behaviors.invoke",
            "screens.write",
            "art.write",
            "key.events",
            "hid.raw",
            "profiles.write",
            "focus.write",
            "net",
            "net.connect:example.com:443",
            "net.connect:*.example.com:*",
            "net.listen:udp:127.0.0.1:9000",
            "net.listen:tcp:127.0.0.1:9001",
            "files",
            "audio.inputs.read",
            "audio.inputs.mute",
            "wallpaper.control",
            "secrets",
            "app.windows",
            "apps.watch:ExampleApp.exe",
            "apps.watch:obs64.exe",
            "apps.watch:My Tool.EXE",
        ];
        for s in all {
            let perm = p(s).unwrap();
            for line in [
                perm.describe(),
                perm.describe_spelled(&["ExampleApp.exe", "My Tool"]),
                perm.describe_windows_spelled(&[]),
                perm.describe_windows_spelled(&["ExampleApp.exe", "My Tool"]),
            ] {
                assert!(!line.to_ascii_lowercase().contains(".exe"), "{s}: {line}");
            }
        }
        let app = p("apps.watch:ExampleApp.exe").unwrap();
        assert_eq!(app.describe(), "see whether exampleapp is running");
        assert_eq!(
            app.describe_spelled(&["Example Suite", "ExampleApp.exe"]),
            "see whether ExampleApp is running"
        );
        assert_eq!(
            p("apps.watch:obs64.exe")
                .unwrap()
                .describe_spelled(&["ExampleApp.exe"]),
            "see whether obs64 is running"
        );
        assert_eq!(
            app.describe_windows_spelled(&["ExampleApp"]),
            "see when ExampleApp opens or closes"
        );
        assert_eq!(
            p("files").unwrap().describe_windows_spelled(&[]),
            p("files").unwrap().describe()
        );
    }

    #[test]
    fn a_friendly_program_name_wins_and_never_names_a_file() {
        let app = p("apps.watch:obs64.exe").unwrap();
        assert_eq!(app.watched_app(), Some("obs64"));
        assert_eq!(
            app.describe_with_program(&[], Some("Example Studio")),
            "see whether Example Studio is running"
        );
        assert_eq!(
            app.describe_with_program(&[], None),
            "see whether obs64 is running"
        );
        assert_eq!(
            app.describe_with_program(&[], Some("obs64.EXE")),
            "see whether obs64 is running"
        );
        let files = p("files").unwrap();
        assert_eq!(files.watched_app(), None);
        assert_eq!(
            files.describe_with_program(&[], Some("Example Studio")),
            files.describe()
        );
    }

    #[test]
    fn parses_round_trips_and_refuses_bad_ones() {
        for s in [
            "facts.read:clock.*",
            "facts.write:weather.temp",
            "lighting.write",
            "key.events",
            "facts.read:*",
            "audio.inputs.read",
            "audio.inputs.mute",
            "apps.watch:obs64.exe",
            "wallpaper.control",
        ] {
            assert_eq!(String::from(p(s).unwrap()), s);
        }
        assert!(
            p("facts.write:device.x").is_err(),
            "device facts are the app's"
        );
        assert!(p("facts.write:*").is_err());
        assert!(p("facts.read:a.*.b").is_err());
        assert!(p("facts.read:").is_err());
        assert!(p("root").is_err());
        assert!(p("key.events").unwrap().sensitive());
    }

    #[test]
    fn scoped_network_permissions() {
        for s in [
            "net.connect:127.0.0.1:4455",
            "net.connect:*.example.com:443",
            "net.connect:api.example.com:*",
            "net.listen:udp:127.0.0.1:5568",
            "net.listen:tcp:127.0.0.1:47911",
        ] {
            assert_eq!(String::from(p(s).unwrap()), s);
        }
        for bad in [
            "net.connect:*:80",
            "net.connect:example.com",
            "net.connect:a b:1",
            "net.connect:*:*",
            "net.connect:*.0.0.1:*",
            "net.connect:*.1:80",
            "net.connect:*.example.1:443",
            "net.listen:udp:0.0.0.0:5568",
            "net.listen:tcp:0.0.0.0:1",
            "net.listen:tcp:127.0.0.1:0",
            "net.listen:udp:127.0.0.1:0",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
        let g = Grant(vec![
            p("net.connect:*.example.com:443").unwrap(),
            p("net.connect:127.0.0.1:*").unwrap(),
            p("net.listen:udp:127.0.0.1:5568").unwrap(),
        ]);
        assert!(g.can_connect("API.example.com", 443));
        assert!(
            !g.can_connect("example.com", 443),
            "a wildcard covers subdomains only"
        );
        assert!(!g.can_connect("evil-example.com", 443));
        assert!(!g.can_connect("api.example.com", 80));
        assert!(g.can_connect("127.0.0.1", 4455));
        assert!(g.can_listen_udp(5568) && !g.can_listen_udp(5569));
        let t = Grant(vec![Permission::try_from(
            "net.listen:tcp:127.0.0.1:47911".to_string(),
        )
        .unwrap()]);
        assert!(t.can_listen_tcp(47911) && !t.can_listen_udp(47911) && !t.unsandboxed());
        assert!(
            Permission::try_from("net.listen:tcp:0.0.0.0:47911".to_string()).is_err(),
            "loopback only"
        );
        assert!(!g.unsandboxed(), "scoped network keeps the sandbox");
        assert!(Grant(vec![Permission::Net]).unsandboxed() && Permission::Net.sensitive());
    }

    #[test]
    fn audio_inputs_read_covers_the_mic_facts_only() {
        let g = Grant(vec![p("audio.inputs.read").unwrap()]);
        assert!(
            g.can_read_fact("mic.state")
                && g.can_subscribe("mic.*")
                && g.can_subscribe("mic.state")
        );
        assert!(
            !g.can_read_fact("media.title")
                && !g.can_subscribe("*")
                && !g.can_subscribe("mic.state.*x")
        );
    }

    #[test]
    fn grants_match_keys_and_subscriptions() {
        let g = Grant(vec![
            p("facts.read:media.*").unwrap(),
            p("facts.write:weather.*").unwrap(),
        ]);
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

#[cfg(test)]
mod outside_words_tests {
    use super::*;

    /// What must hold (#1492 a): the words name a key or pattern cleaned (one line, no control
    /// or hidden characters, no quote mark of its own) and cut; a key with nothing to show
    /// gets plain words, never an empty pair of quotes.
    #[test]
    fn fact_words_show_a_key_cleaned_cut_and_never_in_empty_quotes() {
        let open = '\u{201c}';
        let close = '\u{201d}';
        let odd = format!(
            "notes.\u{201d} yet. Do this\n instead\u{7}\u{202e} \u{201c}{}",
            "k".repeat(200)
        );
        for read in [true, false] {
            for pattern in [odd.clone(), format!("{odd}*")] {
                let w = fact_words(read, &pattern);
                assert!(!w.chars().any(char::is_control), "{w:?}");
                assert!(!w.chars().any(crate::module::hidden_in_name), "{w:?}");
                assert_eq!(w.chars().filter(|c| *c == open).count(), 1, "{w}");
                assert_eq!(w.chars().filter(|c| *c == close).count(), 1, "{w}");
                assert!(w.ends_with("\u{2026}\u{201d}"), "{w}");
                let inside = w.split(open).nth(1).unwrap().trim_end_matches(close);
                assert_eq!(inside.chars().count(), FACT_NAME_MAX, "{w}");
                assert!(inside.starts_with("notes. yet. Do this instead k"), "{w}");
            }
            let verb = if read { "see" } else { "share" };
            for blank in ["", " ", "\u{200b}", "\n", "\u{201c}\u{201d}"] {
                let w = fact_words(read, blank);
                assert_eq!(w, format!("{verb} a status with no name"), "{blank:?}");
                let w = fact_words(read, &format!("{blank}*"));
                if blank.is_empty() {
                    assert_eq!(w, format!("{verb} every status any app shares"));
                } else {
                    assert_eq!(
                        w,
                        format!("{verb} every status whose name Hideout can't show"),
                        "{blank:?}"
                    );
                }
            }
        }
        // An ordinary key reads as before.
        assert_eq!(
            fact_words(true, "notes.secret"),
            "see the status called \u{201c}notes.secret\u{201d}"
        );
        assert_eq!(
            fact_words(false, "notes.*"),
            "share every status whose name starts with \u{201c}notes.\u{201d}"
        );
    }

    /// What must hold (#1492 b): "no permission can cover this write" is one rule, read by the
    /// grant check, the class an outside program's ask gets and the dialog's style.
    #[test]
    fn a_write_no_permission_covers_is_one_rule() {
        use crate::capability::{tool_ask_is_red, tool_class, Capability, ToolClass};
        let wide = Grant(vec![
            Permission::FactsWrite("*".into()),
            Permission::FactsWrite("device.*".into()),
            Permission::FactsWrite("device.kb.battery".into()),
            Permission::FactsWrite("notes.*".into()),
        ]);
        for key in ["device.kb.battery", "device.", "device.a.b.c"] {
            assert!(fact_write_never_granted(key), "{key}");
            assert!(!wide.can_write_fact(key), "{key}");
            let cap = Capability::Kept(Permission::FactsWrite(key.into()));
            assert_eq!(tool_class(&cap, None), ToolClass::Never, "{key}");
            assert!(!tool_ask_is_red(&cap, None), "{key}");
        }
        for key in ["notes.a", "devices.count", "device", "mydevice.x"] {
            assert!(!fact_write_never_granted(key), "{key}");
            assert!(wide.can_write_fact(key), "{key}");
            let cap = Capability::Kept(Permission::FactsWrite(key.into()));
            assert_eq!(tool_class(&cap, None), ToolClass::Ask, "{key}");
        }
    }
}
