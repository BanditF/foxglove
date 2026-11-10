//! `module.toml` (spec §4), the legacy mapping (§4.10) and reading a module folder.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::capability::{pattern_ok, Capability};
use crate::error::{fail, Error, ErrorId};
use crate::legacy::{ActionDecl, SecretDecl, SettingDecl, SettingKind};
use crate::{legacy, Version, HUB_VERSION};

/// The `hub` a legacy manifest reads as (§4.10).
pub const LEGACY_HUB: &str = "1.0";

/// The `hub` minor that brought `kind = "file"` settings (spec v1.4).
const HUB_1_1: Version = Version { major: 1, minor: 1 };

/// The manifest file of a module.
pub const FILE: &str = "module.toml";
/// Manifest files a reader looks for, in order (§4.10).
pub const FILES: &[&str] = &["module.toml", "plugin.toml", "kit.toml"];

/// Reserved first labels and the family each one needs (§4.3). `arete` is `vulpes`.
const RESERVED: &[(&str, Family)] = &[
    ("hideout", Family::Hideout),
    ("foxglove", Family::Foxglove),
    ("vulpes", Family::Vulpes),
    ("arete", Family::Vulpes),
    ("digitalis", Family::Digitalis),
];

const CONTENT_TYPES: &[&str] = &[
    "board-profile",
    "art-pack",
    "lighting-preset",
    "lighting-catalog",
    "wallpaper-manifest",
    "agent-pack",
    "style-pack",
    "eval-pack",
    "template-pack",
    "edition",
];

/// Longest hold an MCP write tool may declare (§9).
pub const MCP_TTL_MAX_S: u64 = 600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Service,
    Runtime,
    Content,
    Firmware,
    Adopted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Hideout,
    Foxglove,
    Vulpes,
    Digitalis,
    ThirdParty,
}

impl Family {
    /// Reads a family name, taking `arete` as `vulpes` (§4.3).
    pub fn parse(s: &str) -> Option<Family> {
        Some(match s {
            "hideout" => Family::Hideout,
            "foxglove" => Family::Foxglove,
            "vulpes" | "arete" => Family::Vulpes,
            "digitalis" => Family::Digitalis,
            "third-party" => Family::ThirdParty,
            _ => return None,
        })
    }

    pub fn first_party(self) -> bool {
        self != Family::ThirdParty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Pipe,
    Stdio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Swap {
    Overlap,
    Handover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DataPolicy {
    Keep,
    Remove,
    Ask,
}

/// A panel slot (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    Page,
    Tile,
    Status,
    Settings,
    Action,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PanelDecl {
    pub id: String,
    pub slot: Slot,
    pub title: String,
    pub icon: String,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowDecl {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub when: Option<String>,
}

/// A `[[settings]]` kind: plugin.toml's three, plus `file` (spec v1.4, `hub = "1.1"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModuleSettingKind {
    Bool,
    Int,
    Text,
    /// A path the user picks with the hub's Choose file button (a native picker); the value is
    /// the path as a string, `""` for none.
    File,
}

/// `[[settings]]`: as plugin.toml, plus `when`, and (spec v1.4) `kind = "file"` with `filter`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModuleSetting {
    /// What the module reads (`focus-minutes`).
    pub name: String,
    /// For the settings form (`Focus minutes`).
    pub label: String,
    pub kind: ModuleSettingKind,
    /// The value until the user changes it; its type must match `kind` (a string for `file`).
    pub default: serde_json::Value,
    /// For `int`: the range the form allows.
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    /// For `file` only: the file names the picker offers, as `*`/`?` patterns joined by `;`
    /// (`ExampleSDK*.dll`). No path separators (spec v1.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default)]
    pub when: Option<String>,
}

impl ModuleSetting {
    /// A legacy setting, as a module setting with no `when`.
    pub fn from_decl(decl: SettingDecl) -> ModuleSetting {
        ModuleSetting {
            kind: match decl.kind {
                SettingKind::Bool => ModuleSettingKind::Bool,
                SettingKind::Int => ModuleSettingKind::Int,
                SettingKind::Text => ModuleSettingKind::Text,
            },
            name: decl.name,
            label: decl.label,
            default: decl.default,
            min: decl.min,
            max: decl.max,
            filter: None,
            when: None,
        }
    }

    /// The plugin.toml shape of this setting: a `file` setting is a `text` one there (its value
    /// is the path).
    pub fn decl(&self) -> SettingDecl {
        SettingDecl {
            name: self.name.clone(),
            label: self.label.clone(),
            kind: match self.kind {
                ModuleSettingKind::Bool => SettingKind::Bool,
                ModuleSettingKind::Int => SettingKind::Int,
                ModuleSettingKind::Text | ModuleSettingKind::File => SettingKind::Text,
            },
            default: self.default.clone(),
            min: self.min,
            max: self.max,
        }
    }

    /// `value` as this setting takes it (an int clamped to its range), or `None` if it's the
    /// wrong type.
    pub fn accept(&self, value: &serde_json::Value) -> Option<serde_json::Value> {
        self.decl().accept(value)
    }
}

/// A `file` setting's `filter` (spec v1.4): 1 to 128 characters, patterns joined by `;`, each a
/// file name with `*` and `?` wildcards and no path separators, drive colons, quotes or `<>|`.
pub fn filter_ok(filter: &str) -> bool {
    !filter.is_empty()
        && filter.len() <= 128
        && filter.split(';').all(|p| {
            !p.trim().is_empty() && !p.chars().any(|c| c.is_control() || "/\\:<>|\"".contains(c))
        })
}

/// `[[actions]]`: as plugin.toml, plus `when`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModuleAction {
    #[serde(flatten)]
    pub decl: ActionDecl,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum McpAccess {
    Read,
    Write,
}

/// `[[mcp.tools]]` (§9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub method: String,
    pub access: McpAccess,
    #[serde(default)]
    pub ttl_max_s: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Unpack {
    Zip,
    None,
}

/// `[[pin]]` (§4.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default = "Pin::default_unpack")]
    pub unpack: Unpack,
    #[serde(default = "Pin::default_strip_top")]
    pub strip_top: bool,
}

impl Pin {
    fn default_unpack() -> Unpack {
        Unpack::Zip
    }
    fn default_strip_top() -> bool {
        true
    }
}

/// `[identity]` (§4.8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub application: String,
}

/// `[edition]` (§4.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Edition {
    pub family: String,
    pub modules: Vec<String>,
}

/// A read manifest, after defaults, the `arete` alias and the legacy mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: Kind,
    pub family: Family,
    pub hub: String,
    pub visibility: Visibility,
    pub description: String,
    /// Method family to `MAJOR.MINOR`, with `arete` written as `vulpes`.
    pub api: BTreeMap<String, String>,
    pub requires: Vec<String>,
    pub enriches: Vec<String>,
    pub run: Option<Vec<String>>,
    pub transport: Transport,
    pub swap: Swap,
    pub provides: Vec<String>,
    pub permissions: Vec<Capability>,
    pub activation_on: Vec<String>,
    pub lifecycle_data: DataPolicy,
    pub identity: Option<Identity>,
    pub panels: Vec<PanelDecl>,
    pub windows: Vec<WindowDecl>,
    pub settings: Vec<ModuleSetting>,
    pub actions: Vec<ModuleAction>,
    pub secrets: Vec<SecretDecl>,
    pub mcp_tools: Vec<McpTool>,
    pub pin: Vec<Pin>,
    pub edition: Option<Edition>,
    /// The file it came from was a legacy `plugin.toml` or `kit.toml`.
    pub legacy: bool,
}

/// What a reader is told besides the file.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Ids known to be private modules (for `requires`, §13.2).
    pub private_ids: Vec<String>,
}

/// Keys allowed at each table path; arrays of tables use their element's path.
const SCHEMA: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "id",
            "name",
            "version",
            "kind",
            "family",
            "hub",
            "visibility",
            "description",
            "api",
            "requires",
            "enriches",
            "run",
            "transport",
            "swap",
            "provides",
            "permissions",
            "activation",
            "lifecycle",
            "identity",
            "ui",
            "settings",
            "actions",
            "secrets",
            "mcp",
            "pin",
            "edition",
        ],
    ),
    ("activation", &["on"]),
    ("lifecycle", &["data"]),
    ("identity", &["application"]),
    ("ui", &["panels", "windows"]),
    ("ui.panels", &["id", "slot", "title", "icon", "when"]),
    ("ui.windows", &["id", "title", "when"]),
    (
        "settings",
        &[
            "name", "label", "kind", "default", "min", "max", "filter", "when",
        ],
    ),
    ("actions", &["id", "name", "when"]),
    ("secrets", &["name", "label"]),
    ("mcp", &["tools"]),
    (
        "mcp.tools",
        &["name", "description", "method", "access", "ttl_max_s"],
    ),
    ("pin", &["url", "sha256", "size", "unpack", "strip_top"]),
    ("edition", &["family", "modules"]),
];

const LEGACY_SCHEMA: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "id",
            "name",
            "version",
            "api",
            "run",
            "permissions",
            "provides",
            "description",
            "actions",
            "secrets",
            "settings",
        ],
    ),
    ("actions", &["id", "name"]),
    ("secrets", &["name", "label"]),
    (
        "settings",
        &["name", "label", "kind", "default", "min", "max"],
    ),
];

fn check_keys(table: &toml::Table, path: &str, schema: &[(&str, &[&str])]) -> Result<(), Error> {
    let Some((_, allowed)) = schema.iter().find(|(p, _)| *p == path) else {
        return Ok(());
    };
    for (key, value) in table {
        let full = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if !allowed.contains(&key.as_str()) {
            return fail(ErrorId::ManifestUnknownKey, format!("unknown key {full}"));
        }
        match value {
            toml::Value::Table(t) => check_keys(t, &full, schema)?,
            toml::Value::Array(items) => {
                for item in items {
                    if let toml::Value::Table(t) = item {
                        check_keys(t, &full, schema)?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_table(text: &str) -> Result<toml::Table, Error> {
    crate::check_toml_nesting(text)?;
    text.parse::<toml::Table>()
        .map_err(|e| Error::new(ErrorId::ManifestSyntax, e.to_string()))
}

/// Deserializes a part of the file; a failure is reported as `id`.
fn part<T: DeserializeOwned>(value: toml::Value, id: ErrorId) -> Result<T, Error> {
    value
        .try_into()
        .map_err(|e: toml::de::Error| Error::new(id, e.to_string()))
}

#[derive(Deserialize)]
struct Raw {
    id: String,
    name: String,
    version: String,
    kind: String,
    family: String,
    hub: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    api: Option<toml::Value>,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    enriches: Vec<String>,
    #[serde(default)]
    run: Option<Vec<String>>,
    #[serde(default)]
    transport: Option<Transport>,
    #[serde(default)]
    swap: Option<Swap>,
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    activation: Option<RawActivation>,
    #[serde(default)]
    lifecycle: Option<RawLifecycle>,
    #[serde(default)]
    identity: Option<Identity>,
    #[serde(default)]
    ui: Option<RawUi>,
    #[serde(default)]
    settings: Vec<toml::Value>,
    #[serde(default)]
    actions: Vec<toml::Value>,
    #[serde(default)]
    secrets: Vec<SecretDecl>,
    #[serde(default)]
    mcp: Option<RawMcp>,
    #[serde(default)]
    pin: Vec<toml::Value>,
    #[serde(default)]
    edition: Option<Edition>,
}

#[derive(Deserialize)]
struct RawActivation {
    #[serde(default)]
    on: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawLifecycle {
    #[serde(default)]
    data: Option<DataPolicy>,
}

#[derive(Deserialize)]
struct RawUi {
    #[serde(default)]
    panels: Vec<toml::Value>,
    #[serde(default)]
    windows: Vec<toml::Value>,
}

#[derive(Deserialize)]
struct RawMcp {
    #[serde(default)]
    tools: Vec<toml::Value>,
}

/// `[a-z0-9][a-z0-9_-]*`
fn label_ok(l: &str) -> bool {
    let mut chars = l.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// An id's grammar (§4.3): labels joined by `.`, at most 128 characters.
pub fn id_grammar_ok(id: &str) -> bool {
    id.len() <= 128 && id.split('.').all(label_ok)
}

/// Readers accept `arete` as an alias of `vulpes` in ids and write `vulpes` (§4.3).
pub fn write_vulpes(id: &str) -> String {
    match id.strip_prefix("arete") {
        Some(rest) if rest.is_empty() || rest.starts_with('.') => format!("vulpes{rest}"),
        _ => id.to_string(),
    }
}

/// `a-z`, `0-9`, `-`: ids of contributions and names of settings.
pub(crate) fn item_id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn reserved_family(id: &str) -> Option<Family> {
    let first = id.split('.').next().unwrap_or_default();
    RESERVED.iter().find(|(l, _)| *l == first).map(|(_, f)| *f)
}

/// SemVer `MAJOR.MINOR.PATCH`, optional `-prerelease`, no `+build` (§4.2).
pub fn semver_ok(v: &str) -> bool {
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let num = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'))
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| num(p))
        && pre.is_none_or(|p| {
            p.split('.')
                .all(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        })
}

/// The fact prefixes of the app watch (§4.6): `apps.seen.<name>` (spec v1.4) and
/// `apps.running.<name>` (spec v1.7). Each names one app by its [`app_label`].
pub const APP_FACTS: [&str; 2] = ["apps.seen.", "apps.running."];

/// An app's label in the app facts (§4.6): the exe name without `.exe`, lowercased, with any
/// character outside `[a-z0-9_-]` turned into `-` (`Example-App.exe` is `example-app`,
/// `tool64.exe` is `tool64`). `None` for an empty name.
pub fn app_label(exe: &str) -> Option<String> {
    let lower = exe.trim().to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    let label: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    (!label.is_empty()).then_some(label)
}

/// The app a fact key or `when` clause names, when it is an app fact (`apps.seen.example-app`,
/// `!apps.running.tool64`): its label.
pub fn app_fact_label(key: &str) -> Option<&str> {
    let key = key.strip_prefix('!').unwrap_or(key);
    APP_FACTS
        .iter()
        .find_map(|p| key.strip_prefix(p))
        .filter(|l| !l.is_empty() && !l.contains('.'))
}

impl Module {
    /// The apps this manifest asks the app watch about (§4.6, v1.7), by [`app_label`]: those
    /// its `when` clauses name, its `enriches`, its `app:<name>.exe` activation events and its
    /// `apps.watch:<name>.exe` permissions. The hub watches only these.
    pub fn watched_apps(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let whens = self
            .panels
            .iter()
            .map(|p| p.when.as_ref())
            .chain(self.windows.iter().map(|w| w.when.as_ref()))
            .chain(self.settings.iter().map(|s| s.when.as_ref()))
            .chain(self.actions.iter().map(|a| a.when.as_ref()))
            .flatten();
        out.extend(whens.filter_map(|w| app_fact_label(w)).map(String::from));
        let exes = self
            .enriches
            .iter()
            .map(String::as_str)
            .chain(
                self.activation_on
                    .iter()
                    .filter_map(|e| e.strip_prefix("app:")),
            )
            .chain(self.permissions.iter().filter_map(|c| match c {
                Capability::Kept(crate::permission::Permission::AppsWatch(exe)) => {
                    Some(exe.as_str())
                }
                _ => None,
            }));
        out.extend(exes.filter_map(app_label));
        out
    }

    /// A permission's line for the permission dialog (§6.4): [`Permission::describe`], with an
    /// app watch's app spelled as this manifest spells it in `name`, `enriches` or an `app:`
    /// activation event (`apps.watch:exampleapp.exe` with `enriches = ["ExampleApp.exe"]` gives
    /// "see whether ExampleApp is running", #991).
    ///
    /// [`Permission::describe`]: crate::permission::Permission::describe
    pub fn describe_permission(&self, p: &crate::permission::Permission) -> String {
        let names: Vec<&str> = self
            .enriches
            .iter()
            .map(String::as_str)
            .chain(
                self.activation_on
                    .iter()
                    .filter_map(|e| e.strip_prefix("app:")),
            )
            .chain(self.name.split_whitespace())
            .collect();
        p.describe_spelled(&names)
    }
}

/// One `when` clause: a fact name, optionally prefixed by `!` (§4.6). An `apps.seen.<name>`
/// fact (spec v1.4) or an `apps.running.<name>` fact (v1.7) names one app: a single lowercase
/// label, the exe name without `.exe`.
pub fn when_ok(w: &str) -> bool {
    let name = w.strip_prefix('!').unwrap_or(w);
    let grammar = !name.is_empty()
        && name.split('.').all(|l| {
            !l.is_empty()
                && l.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    grammar
        && APP_FACTS
            .iter()
            .find_map(|p| name.strip_prefix(p))
            .is_none_or(|app| {
                !app.contains('.')
                    && app.chars().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
                    })
            })
}

/// `<name>.exe`
fn exe_ok(s: &str) -> bool {
    s.strip_suffix(".exe").is_some_and(|n| {
        !n.is_empty()
            && n.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._- ".contains(c))
    })
}

/// One `activation.on` entry (§4.6).
pub fn activation_ok(e: &str) -> bool {
    match e.split_once(':') {
        None => matches!(e, "enable" | "open" | "call"),
        Some(("fact", p)) => pattern_ok(p),
        Some(("app", exe)) => exe_ok(exe),
        Some(("device", class)) => !class.is_empty() && class.split('.').all(label_ok),
        _ => false,
    }
}

/// One `requires` entry (§4.4); returns the required id.
pub fn requires_id(entry: &str) -> Option<&str> {
    let (id, version) = match entry.split_once(">=") {
        Some((id, v)) => (id, Some(v)),
        None => (entry, None),
    };
    let version_ok = version.is_none_or(|v| {
        let parts: Vec<&str> = v.split('.').collect();
        (1..=3).contains(&parts.len())
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    (id_grammar_ok(id) && version_ok).then_some(id)
}

/// A `run[0]` that stays inside the folder: only normal names or `.`.
fn stays_inside(program: &str) -> bool {
    !program.is_empty()
        && !program.starts_with(['/', '\\'])
        && !program.contains(':')
        && program
            .split(['/', '\\'])
            .all(|c| c != ".." && !c.is_empty())
}

/// An absolute Windows path (`C:\...`, `C:/...` or `\\server\...`), however the reader runs.
fn absolute(program: &str) -> bool {
    let b = program.as_bytes();
    (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'/' | b'\\'))
        || program.starts_with("\\\\")
        || Path::new(program).is_absolute()
}

fn unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    id: ErrorId,
    what: &str,
) -> Result<(), Error> {
    let mut seen = HashSet::new();
    for i in ids {
        if !item_id_ok(i) || !seen.insert(i) {
            return fail(id, format!("{what} {i:?} needs a unique id (a-z, 0-9, -)"));
        }
    }
    Ok(())
}

fn whens<'a>(items: impl Iterator<Item = Option<&'a String>>) -> Result<(), Error> {
    for w in items.flatten() {
        if !when_ok(w) {
            return fail(ErrorId::ManifestActivation, format!("bad when {w:?}"));
        }
    }
    Ok(())
}

impl Module {
    /// Reads `module.toml` text and makes every §4 check.
    pub fn parse(text: &str, ctx: &Context) -> Result<Module, Error> {
        let table = parse_table(text)?;
        check_keys(&table, "", SCHEMA)?;
        for key in ["id", "name", "version", "kind", "family", "hub"] {
            if !table.contains_key(key) {
                return fail(ErrorId::ManifestMissingKey, format!("missing key {key}"));
            }
        }
        let raw: Raw = part(toml::Value::Table(table), ErrorId::ManifestSyntax)?;
        Module::check(raw, ctx)
    }

    fn check(raw: Raw, ctx: &Context) -> Result<Module, Error> {
        use ErrorId::*;
        let id = raw.id;
        if !id_grammar_ok(&id) {
            return fail(ManifestId, format!("id {id:?} breaks §4.3"));
        }
        let family = Family::parse(&raw.family).ok_or_else(|| {
            Error::new(ManifestFamily, format!("unknown family {:?}", raw.family))
        })?;
        if !id.contains('.') && !family.first_party() {
            return fail(ManifestId, "one-label ids are for first-party modules only");
        }
        match reserved_family(&id) {
            Some(f) if f != family => {
                return fail(ManifestFamily, format!("id {id:?} needs family {f:?}"))
            }
            None if id.starts_with("org.foxglove.") && family != Family::Foxglove => {
                return fail(ManifestFamily, "org.foxglove.* is family foxglove")
            }
            _ => {}
        }
        let kind = match raw.kind.as_str() {
            "service" => Kind::Service,
            "runtime" => Kind::Runtime,
            "content" => Kind::Content,
            "firmware" => Kind::Firmware,
            "adopted" => Kind::Adopted,
            k => return fail(ManifestKind, format!("unknown kind {k:?}")),
        };
        if !semver_ok(&raw.version) {
            return fail(
                ManifestVersion,
                format!("version {:?} isn't SemVer", raw.version),
            );
        }
        match Version::parse(&raw.hub) {
            Some(v) if v.served_by(HUB_VERSION) => {}
            _ => {
                return fail(
                    ManifestHub,
                    format!("hub {:?}: this reader serves {HUB_VERSION}", raw.hub),
                )
            }
        }
        let visibility = match raw.visibility.as_deref() {
            None | Some("public") => Visibility::Public,
            Some("private") => Visibility::Private,
            Some(v) => return fail(ManifestVisibility, format!("unknown visibility {v:?}")),
        };
        if visibility == Visibility::Private && reserved_family(&id).is_some() {
            return fail(
                ManifestVisibility,
                "a private module can't use a reserved first label",
            );
        }
        let mut api = BTreeMap::new();
        match raw.api {
            None => {}
            Some(toml::Value::String(s)) => {
                api.insert("foxglove".to_string(), s);
            }
            Some(toml::Value::Table(t)) => {
                for (k, v) in t {
                    let toml::Value::String(v) = v else {
                        return fail(ManifestApi, format!("api.{k} must be a string"));
                    };
                    let k = if k == "arete" {
                        "vulpes".to_string()
                    } else {
                        k
                    };
                    api.insert(k, v);
                }
            }
            Some(_) => return fail(ManifestSyntax, "api is a table or a string"),
        }
        if let Some((k, v)) = api.iter().find(|(_, v)| Version::parse(v).is_none()) {
            return fail(ManifestApi, format!("api.{k} = {v:?} isn't MAJOR.MINOR"));
        }
        match (kind, &raw.run) {
            (Kind::Service | Kind::Adopted, None) => return fail(ManifestRun, "run is required"),
            (Kind::Service | Kind::Adopted, Some(r)) if r.is_empty() => {
                return fail(ManifestRun, "run is empty")
            }
            (Kind::Service, Some(r)) if !stays_inside(&r[0]) => {
                return fail(
                    ManifestRun,
                    format!("run {:?} must stay inside the folder", r[0]),
                )
            }
            (Kind::Adopted, Some(r)) if !absolute(&r[0]) => {
                return fail(ManifestRun, "an adopted module's run[0] is absolute")
            }
            (Kind::Runtime | Kind::Content | Kind::Firmware, Some(_)) => {
                return fail(ManifestRun, "only service and adopted modules have run")
            }
            _ => {}
        }
        for r in &raw.requires {
            let Some(rid) = requires_id(r) else {
                return fail(ManifestRequires, format!("bad requires entry {r:?}"));
            };
            if visibility == Visibility::Public && ctx.private_ids.iter().any(|p| p == rid) {
                return fail(
                    ManifestRequires,
                    format!("a public module can't require private {rid}"),
                );
            }
        }
        if let Some(e) = raw.enriches.iter().find(|e| !exe_ok(e)) {
            return fail(ManifestEnriches, format!("enriches {e:?} isn't <name>.exe"));
        }
        for p in &raw.provides {
            let ok = match p.split_once(':') {
                Some(("method" | "facts", pat)) => {
                    pattern_ok(pat) && matches!(kind, Kind::Service | Kind::Adopted)
                }
                Some(("channel", c)) => {
                    item_id_ok(c) && matches!(kind, Kind::Service | Kind::Adopted)
                }
                Some(("content", t)) => CONTENT_TYPES.contains(&t) && kind == Kind::Content,
                Some(("firmware", b)) => label_ok(b) && kind == Kind::Firmware,
                _ => false,
            };
            if !ok {
                return fail(ManifestProvides, format!("bad provides entry {p:?}"));
            }
        }
        let has = |prefix: &str| raw.provides.iter().any(|p| p.starts_with(prefix));
        if kind == Kind::Content && !has("content:") {
            return fail(ManifestProvides, "a content module provides content:");
        }
        if kind == Kind::Firmware && !has("firmware:") {
            return fail(ManifestProvides, "a firmware module provides firmware:");
        }
        let permissions = raw
            .permissions
            .iter()
            .map(|p| Capability::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        let activation_on = raw
            .activation
            .and_then(|a| a.on)
            .unwrap_or_else(|| vec!["enable".to_string()]);
        if let Some(e) = activation_on.iter().find(|e| !activation_ok(e)) {
            return fail(ManifestActivation, format!("bad activation event {e:?}"));
        }
        let ui = raw.ui.unwrap_or(RawUi {
            panels: vec![],
            windows: vec![],
        });
        let panels: Vec<PanelDecl> = ui
            .panels
            .into_iter()
            .map(|v| part(v, ManifestUi))
            .collect::<Result<_, _>>()?;
        let windows: Vec<WindowDecl> = ui
            .windows
            .into_iter()
            .map(|v| part(v, ManifestUi))
            .collect::<Result<_, _>>()?;
        unique_ids(panels.iter().map(|p| p.id.as_str()), ManifestUi, "panel")?;
        unique_ids(windows.iter().map(|w| w.id.as_str()), ManifestUi, "window")?;
        if !windows.is_empty() && !permissions.contains(&Capability::UiWindow) {
            return fail(
                ManifestWindowMissing,
                "[[ui.windows]] needs the ui.window permission",
            );
        }
        let settings: Vec<ModuleSetting> = raw
            .settings
            .into_iter()
            .map(|v| part(v, ManifestSettings))
            .collect::<Result<_, _>>()?;
        let actions: Vec<ModuleAction> = raw
            .actions
            .into_iter()
            .map(|v| part(v, ManifestActions))
            .collect::<Result<_, _>>()?;
        unique_ids(
            actions.iter().map(|a| a.decl.id.as_str()),
            ManifestActions,
            "action",
        )?;
        if actions.iter().any(|a| a.decl.name.trim().is_empty()) {
            return fail(ManifestActions, "an action needs a name");
        }
        unique_ids(
            settings.iter().map(|s| s.name.as_str()),
            ManifestSettings,
            "setting",
        )?;
        if settings
            .iter()
            .any(|s| s.label.trim().is_empty() || s.accept(&s.default).is_none())
        {
            return fail(
                ManifestSettings,
                "a setting needs a label and a default of its kind",
            );
        }
        for s in &settings {
            match (&s.filter, s.kind) {
                (Some(f), ModuleSettingKind::File) if !filter_ok(f) => {
                    return fail(ManifestSettings, format!("bad filter {f:?}"))
                }
                (Some(_), kind) if kind != ModuleSettingKind::File => {
                    return fail(ManifestSettings, "filter is for kind = \"file\" only")
                }
                _ => {}
            }
        }
        // A key or value new in hub 1.1 (spec v1.4) needs the manifest to say so (§4.1), so a
        // 1.0 reader refuses it with manifest.hub instead of misreading it.
        let hub_1_1 = Version::parse(&raw.hub).is_some_and(|v| v >= HUB_1_1);
        if !hub_1_1 && settings.iter().any(|s| s.kind == ModuleSettingKind::File) {
            return fail(ManifestHub, "kind = \"file\" needs hub = \"1.1\"");
        }
        whens(
            panels
                .iter()
                .map(|p| p.when.as_ref())
                .chain(windows.iter().map(|w| w.when.as_ref()))
                .chain(settings.iter().map(|s| s.when.as_ref()))
                .chain(actions.iter().map(|a| a.when.as_ref())),
        )?;
        if !raw.secrets.is_empty()
            && !permissions.contains(&Capability::Kept(crate::permission::Permission::Secrets))
        {
            return fail(ManifestSecrets, "[[secrets]] needs the secrets permission");
        }
        let mcp_tools: Vec<McpTool> = raw
            .mcp
            .map(|m| m.tools)
            .unwrap_or_default()
            .into_iter()
            .map(|v| part(v, ManifestMcp))
            .collect::<Result<_, _>>()?;
        unique_ids(
            mcp_tools.iter().map(|t| t.name.as_str()),
            ManifestMcp,
            "MCP tool",
        )?;
        for t in &mcp_tools {
            if t.ttl_max_s.is_some_and(|s| s > MCP_TTL_MAX_S) {
                return fail(
                    ManifestMcp,
                    format!("{}: ttl_max_s is at most {MCP_TTL_MAX_S}", t.name),
                );
            }
            let provided = raw
                .provides
                .iter()
                .filter_map(|p| p.strip_prefix("method:"))
                .any(|pat| crate::permission::key_matches(pat, &t.method));
            if !provided {
                return fail(
                    ManifestMcpMethod,
                    format!("{}: the module doesn't provide {}", t.name, t.method),
                );
            }
        }
        let pin: Vec<Pin> = raw
            .pin
            .into_iter()
            .map(|v| part(v, ManifestPin))
            .collect::<Result<_, _>>()?;
        match (kind, pin.is_empty()) {
            (Kind::Runtime, true) => return fail(ManifestPin, "a runtime module needs [[pin]]"),
            (k, false) if k != Kind::Runtime => {
                return fail(ManifestPin, "[[pin]] is for runtime modules only")
            }
            _ => {}
        }
        for p in &pin {
            let sha_ok = p.sha256.len() == 64
                && p.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            if !p.url.starts_with("https://") || !sha_ok {
                return fail(
                    ManifestPin,
                    "a pin needs an https:// url and a lowercase sha256",
                );
            }
        }
        if raw.edition.is_some() != raw.provides.iter().any(|p| p == "content:edition") {
            return fail(
                ManifestEdition,
                "[edition] goes with provides content:edition",
            );
        }
        if let Some(e) = &raw.edition {
            if Family::parse(&e.family).is_none() || e.modules.iter().any(|m| !id_grammar_ok(m)) {
                return fail(ManifestEdition, "bad [edition]");
            }
        }
        if raw.identity.is_some() && (!family.first_party() || visibility != Visibility::Public) {
            return fail(
                ManifestIdentity,
                "[identity] is for public first-party modules only",
            );
        }
        Ok(Module {
            id: write_vulpes(&id),
            name: raw.name,
            version: raw.version,
            kind,
            family,
            hub: raw.hub,
            visibility,
            description: raw.description,
            api,
            requires: raw.requires,
            enriches: raw.enriches,
            run: raw.run,
            transport: raw.transport.unwrap_or(Transport::Pipe),
            swap: raw.swap.unwrap_or(Swap::Overlap),
            provides: raw.provides,
            permissions,
            activation_on,
            lifecycle_data: raw
                .lifecycle
                .and_then(|l| l.data)
                .unwrap_or(DataPolicy::Ask),
            identity: raw.identity,
            panels,
            windows,
            settings,
            actions,
            secrets: raw.secrets,
            mcp_tools,
            pin,
            edition: raw.edition,
            legacy: false,
        })
    }

    /// Reads a legacy `plugin.toml` or `kit.toml` (§4.10) as `Manifest::parse` does, then maps it.
    /// `app` is the Foxglove API version served.
    pub fn parse_legacy(text: &str, app: Version) -> Result<Module, Error> {
        let table = parse_table(text)?;
        check_keys(&table, "", LEGACY_SCHEMA)?;
        for key in ["id", "name", "version", "api", "run"] {
            if !table.contains_key(key) {
                return fail(ErrorId::ManifestMissingKey, format!("missing key {key}"));
            }
        }
        let m = legacy::Manifest::check(text, app)?;
        Ok(Module::from_legacy(m))
    }

    /// The §4.10 mapping of a checked legacy manifest.
    pub fn from_legacy(m: legacy::Manifest) -> Module {
        let family = if m.id.starts_with("org.foxglove.") || m.id.starts_with("foxglove.") {
            Family::Foxglove
        } else {
            Family::ThirdParty
        };
        Module {
            family,
            kind: Kind::Service,
            hub: LEGACY_HUB.to_string(),
            visibility: Visibility::Public,
            description: m.description,
            api: BTreeMap::from([("foxglove".to_string(), m.api)]),
            requires: vec![],
            enriches: vec![],
            run: Some(m.run),
            transport: Transport::Stdio,
            swap: Swap::Overlap,
            provides: vec![],
            permissions: m.permissions.into_iter().map(Capability::Kept).collect(),
            activation_on: vec!["enable".to_string()],
            lifecycle_data: DataPolicy::Remove,
            identity: None,
            panels: vec![],
            windows: vec![],
            settings: m
                .settings
                .into_iter()
                .map(ModuleSetting::from_decl)
                .collect(),
            actions: m
                .actions
                .into_iter()
                .map(|decl| ModuleAction { decl, when: None })
                .collect(),
            secrets: m.secrets,
            mcp_tools: vec![],
            pin: vec![],
            edition: None,
            legacy: true,
            id: m.id,
            name: m.name,
            version: m.version,
        }
    }

    /// The fields a reader reports for a module (`normalized` in the vectors).
    pub fn normalized(&self) -> serde_json::Value {
        fn s<T: Serialize>(v: &T) -> serde_json::Value {
            serde_json::to_value(v).unwrap_or_default()
        }
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "kind": s(&self.kind),
            "family": s(&self.family),
            "hub": self.hub,
            "visibility": s(&self.visibility),
            "api": self.api,
            "transport": s(&self.transport),
            "swap": s(&self.swap),
            "activation_on": self.activation_on,
            "lifecycle_data": s(&self.lifecycle_data),
            "requires": self.requires,
            "run": self.run,
            "provides": self.provides,
            "permissions": self.permissions.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            "counts": {
                "panels": self.panels.len(),
                "windows": self.windows.len(),
                "settings": self.settings.len(),
                "actions": self.actions.len(),
                "secrets": self.secrets.len(),
                "mcp_tools": self.mcp_tools.len(),
            },
        })
    }
}

/// Which file a folder's manifest came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestFile {
    Module,
    Plugin,
    Kit,
}

impl ManifestFile {
    pub fn name(self) -> &'static str {
        match self {
            ManifestFile::Module => "module.toml",
            ManifestFile::Plugin => "plugin.toml",
            ManifestFile::Kit => "kit.toml",
        }
    }
}

/// Reads a module folder: `module.toml`, else `plugin.toml`, else `kit.toml` (§4.10).
pub fn read_dir(
    dir: &Path,
    ctx: &Context,
    app: Version,
) -> std::io::Result<(ManifestFile, Result<Module, Error>)> {
    for file in [
        ManifestFile::Module,
        ManifestFile::Plugin,
        ManifestFile::Kit,
    ] {
        match std::fs::read_to_string(dir.join(file.name())) {
            Ok(text) => {
                let module = match file {
                    ManifestFile::Module => Module::parse(&text, ctx),
                    _ => Module::parse_legacy(&text, app),
                };
                return Ok((file, module));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::ErrorKind::NotFound.into())
}

#[cfg(test)]
mod app_watch_tests {
    use super::*;

    #[test]
    fn app_labels_and_the_apps_a_manifest_asks_about() {
        assert_eq!(app_label("Example-App.exe").as_deref(), Some("example-app"));
        assert_eq!(app_label("My Tool.EXE").as_deref(), Some("my-tool"));
        assert_eq!(app_label(".exe"), None);
        assert_eq!(app_fact_label("!apps.running.tool64"), Some("tool64"));
        assert_eq!(app_fact_label("apps.seen.a.b"), None);
        assert!(when_ok("apps.running.example-app"));
        assert!(!when_ok("apps.running.example-app.exe"));
        let m = Module::parse(
            r#"
id = "foxglove.example-art"
name = "Example Art"
version = "0.4.0"
kind = "service"
family = "foxglove"
hub = "1.0"
run = ["example-art.exe"]
enriches = ["Example-App.exe"]
permissions = ["apps.watch:other-app.exe"]

[activation]
on = ["enable", "app:third.exe"]

[[ui.panels]]
id = "tile"
slot = "tile"
title = "Example"
icon = "mic"
when = "!apps.seen.fourth"
"#,
            &Context::default(),
        )
        .unwrap();
        let want: BTreeSet<String> = ["example-app", "fourth", "other-app", "third"]
            .map(String::from)
            .into();
        assert_eq!(m.watched_apps(), want);
        let watch = |exe: &str| crate::permission::Permission::AppsWatch(exe.into());
        assert_eq!(
            m.describe_permission(&watch("example-app.exe")),
            "see whether Example-App is running"
        );
        assert_eq!(
            m.describe_permission(&watch("other-app.exe")),
            "see whether other-app is running"
        );
        assert_eq!(
            m.describe_permission(&watch("art.exe")),
            "see whether Art is running"
        );
    }
}
