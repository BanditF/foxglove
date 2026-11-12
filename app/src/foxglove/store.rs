//! Foxglove's settings store (docs/profiles.md, phase 1): one versioned `foxglove.toml` in
//! `%APPDATA%\Foxglove`, one table per feature (`[screens]`, `[updates]`, ...).
//!
//! - Each feature reads and writes its own typed section; missing keys take their defaults.
//! - Writes replace the file atomically (temp file and rename) under a process-wide lock, so
//!   two features saving at once can't lose each other's changes.
//! - A feature moves in through [`load_or_migrate`]: the first time its section is missing, its
//!   old file is imported and renamed `*.migrated` (kept, so an older Foxglove can be pointed
//!   back at it by hand).
//! - KeyPeek's `settings.ini` stays where it is; upstream code owns that file.
//! - A read that fails (the file held by another program, broken, or no settings folder) never
//!   turns into defaults that get saved (#741): it's retried briefly, and while it fails the
//!   section is "unread" and a save of it is refused, until a read of that section succeeds.
//!   A missing file is a genuine first run: defaults, saved as usual.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use toml::{Table, Value};

/// The store's format version; each bump gets a migration in [`upgrade`].
pub const SCHEMA: i64 = 1;

static LOCK: Mutex<()> = Mutex::new(());

/// `%APPDATA%\Foxglove\foxglove.toml` (the folder above KeyPeek's `config`), or inside
/// `FOXGLOVE_CONFIG_DIR` when that's set, so a test run never shares or touches a store
/// outside its own folder. In a test build always inside [`test_sandbox`], set up or not.
pub fn path() -> Option<PathBuf> {
    if let Some(dir) = test_sandbox() {
        return Some(dir.join("foxglove.toml"));
    }
    path_for(
        std::env::var_os("FOXGLOVE_CONFIG_DIR").map(PathBuf::from),
        crate::settings::Settings::config_file_path(),
    )
}

fn path_for(config_dir: Option<PathBuf>, settings_ini: Option<PathBuf>) -> Option<PathBuf> {
    match config_dir {
        Some(dir) => Some(dir.join("foxglove.toml")),
        None => Some(settings_ini?.parent()?.parent()?.join("foxglove.toml")),
    }
}

/// Sections renamed since they shipped (#87): the old name is read as the new one, and the next
/// save writes the new one.
const RENAMED: &[(&str, &str)] = &[("kits", "plugins")];

fn renamed(mut table: Table) -> Table {
    for (old, new) in RENAMED {
        if !table.contains_key(*new) {
            if let Some(v) = table.remove(*old) {
                table.insert((*new).into(), v);
            }
        }
    }
    table
}

/// The waits before each retry of a read the file system refused (#741): a few tries over
/// about half a second, only when a read fails.
const READ_RETRIES: [Duration; 3] = [
    Duration::from_millis(50),
    Duration::from_millis(150),
    Duration::from_millis(300),
];

/// The store's table; a missing file is an empty one. A read the file system refuses (another
/// program holds the file) is tried again a few times; a broken file isn't.
fn read(path: &Path) -> Result<Table, String> {
    let mut waits = READ_RETRIES.iter();
    loop {
        match read_once(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                let Some(wait) = waits.next() else {
                    return Err(format!("{}: {e}", path.display()));
                };
                std::thread::sleep(*wait);
            }
            Err(_) => return Ok(Table::new()),
            Ok(text) => {
                return text
                    .parse::<Table>()
                    .map(renamed)
                    .map_err(|e| format!("{}: {e}", path.display()))
            }
        }
    }
}

fn read_once(path: &Path) -> std::io::Result<String> {
    assert_sandboxed(path);
    #[cfg(test)]
    if FAIL_READS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(path)
    {
        return Err(std::io::Error::other(
            "the process cannot access the file (test fake)",
        ));
    }
    std::fs::read_to_string(path)
}

/// Tests: reads of these files fail, as one another program holds would.
#[cfg(test)]
pub static FAIL_READS: Mutex<std::collections::BTreeSet<PathBuf>> =
    Mutex::new(std::collections::BTreeSet::new());

/// Sections whose last read failed, by store file: saving one would write over settings that
/// were never read, so it's refused until a read of it succeeds (#741).
/// The value is the failed read's error.
static UNREAD: Mutex<BTreeMap<(PathBuf, String), String>> = Mutex::new(BTreeMap::new());

fn mark_unread(path: &Path, section: &str, why: &str) {
    let fresh = UNREAD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert((path.to_path_buf(), section.to_string()), why.to_string())
        .is_none();
    if fresh {
        eprintln!(
            "Foxglove: settings: can't read [{section}] ({why}); it won't be saved until a read succeeds"
        );
    }
}

fn mark_read(path: &Path, section: &str) {
    UNREAD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(path.to_path_buf(), section.to_string()));
}

fn is_unread(path: &Path, section: &str) -> bool {
    UNREAD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&(path.to_path_buf(), section.to_string()))
}

/// The sections of the store at `path` whose read is failing, with the last error.
fn unread_at(path: &Path) -> Vec<(String, String)> {
    UNREAD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|((p, _), _)| p == path)
        .map(|((_, s), why)| (s.clone(), why.clone()))
        .collect()
}

/// The health view's row (registered as "settings", #741): a warning while any section's read
/// is failing, from the store's own state; nothing otherwise.
pub fn health_rows() -> Vec<super::health::HealthRow> {
    path().map(|p| rows_for(&unread_at(&p))).unwrap_or_default()
}

/// Tests: the health row for the store at `path`.
#[cfg(test)]
pub fn rows_for_test(path: &Path) -> Vec<super::health::HealthRow> {
    rows_for(&unread_at(path))
}

fn rows_for(unread: &[(String, String)]) -> Vec<super::health::HealthRow> {
    let Some((_, why)) = unread.last() else {
        return vec![];
    };
    let sections: Vec<String> = unread.iter().map(|(s, _)| format!("[{s}]")).collect();
    vec![super::health::HealthRow::new(
        "app",
        "Settings",
        super::health::Level::Warn,
        format!(
            "settings unreadable: {why} (not saved until read: {})",
            sections.join(", ")
        ),
    )]
}

fn write(path: &Path, table: &Table) -> std::io::Result<()> {
    assert_sandboxed(path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(table).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Brings an older table up to [`SCHEMA`]; schema 1 is the first, so nothing to do yet.
fn upgrade(mut table: Table) -> Table {
    table.insert("schema".into(), Value::Integer(SCHEMA));
    table
}

/// The value at a dotted section path (`art.cycle`), if any.
fn get<'a>(table: &'a Table, section: &str) -> Option<&'a Value> {
    let mut parts = section.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

fn set(table: &mut Table, section: &str, value: Value) {
    let mut parts: Vec<&str> = section.split('.').collect();
    let last = parts.pop().expect("a section name");
    let mut t = table;
    for part in parts {
        let entry = t.entry(part).or_insert_with(|| Value::Table(Table::new()));
        if !entry.is_table() {
            // A hand edit left a value where a section belongs (`scenes = 1`): the section
            // takes its place instead of every save panicking.
            eprintln!("Foxglove: settings: `{part}` wasn't a section; it is one again");
            *entry = Value::Table(Table::new());
        }
        let Value::Table(next) = entry else {
            unreachable!("made a table above")
        };
        t = next;
    }
    t.insert(last.into(), value);
}

/// A section from the store at `path`: `Err` when the file can't be read now (held by another
/// program, or broken), `Ok(None)` when the section isn't there. A failed read leaves the
/// section unread (no saves of it) until a read succeeds.
pub(crate) fn try_load_at<T: DeserializeOwned>(
    path: &Path,
    section: &str,
) -> Result<Option<T>, String> {
    let table = match read(path) {
        Ok(t) => {
            mark_read(path, section);
            t
        }
        Err(e) => {
            mark_unread(path, section, &e);
            return Err(e);
        }
    };
    effective(&table, section)
        .map(|v| {
            v.try_into()
                .map_err(|e: toml::de::Error| format!("[{section}]: {e}"))
        })
        .transpose()
}

/// Sections a profile may override (docs/profiles.md); the rest describe the machine.
pub const PROFILED: &[&str] = &[
    "screens",
    "lighting",
    "art",
    "art.cycle",
    "overlay",
    // The Scenes page's chosen look (#487); its saved presets (`scene_presets`) are shared.
    "scenes",
];

/// The profiles a new install starts with; easy to change once the user decides (#90 row 13).
pub const STARTERS: &[&str] = &["Work", "Game"];

/// The active profile's name, if one is chosen and still exists.
fn active_profile(table: &Table) -> Option<String> {
    let name = get(table, "profiles.active")?.as_str()?.to_string();
    get(table, &format!("profiles.list.{name}")).map(|_| name)
}

/// A section as the features see it: the base, with the active profile's overrides on top.
fn effective(table: &Table, section: &str) -> Option<Value> {
    let base = get(table, section).cloned();
    if !PROFILED.contains(&section) {
        return base;
    }
    let over = active_profile(table)
        .and_then(|name| get(table, &format!("profiles.list.{name}.{section}")).cloned());
    match (base, over) {
        (Some(Value::Table(mut b)), Some(Value::Table(o))) => {
            merge(&mut b, o);
            Some(Value::Table(b))
        }
        (b, None) => b,
        (_, o) => o,
    }
}

/// Deep merge: tables merge key by key, anything else is replaced.
fn merge(base: &mut Table, over: Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(Value::Table(b)), Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// What `new` changes compared with `base`: the keys a profile needs to store.
fn diff(base: &Table, new: &Table) -> Table {
    let mut out = Table::new();
    for (k, v) in new {
        match (base.get(k), v) {
            (Some(Value::Table(b)), Value::Table(n)) => {
                let d = diff(b, n);
                if !d.is_empty() {
                    out.insert(k.clone(), Value::Table(d));
                }
            }
            (Some(b), n) if b == n => {}
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// Saves a section. While a profile is active, a profiled section's changes go into that
/// profile (only what differs from the base), so editing settings edits the profile in use.
/// Migrations always write the base (`to_base`). A section whose last read failed isn't saved
/// (#741): the value would be defaults standing in for settings that were never read.
pub(crate) fn save_at<T: Serialize>(
    path: &Path,
    section: &str,
    value: &T,
    to_base: bool,
) -> std::io::Result<()> {
    if is_unread(path, section) {
        return Err(std::io::Error::other(format!(
            "[{section}] wasn't read from {}, so it isn't saved over",
            path.display()
        )));
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let table = read(path).map_err(std::io::Error::other)?;
    let mut table = upgrade(table);
    let value = Value::try_from(value).map_err(std::io::Error::other)?;
    match active_profile(&table).filter(|_| !to_base && PROFILED.contains(&section)) {
        Some(name) => {
            let base = get(&table, section)
                .and_then(Value::as_table)
                .cloned()
                .unwrap_or_default();
            let changes = value
                .as_table()
                .map_or_else(Table::new, |new| diff(&base, new));
            set(
                &mut table,
                &format!("profiles.list.{name}.{section}"),
                Value::Table(changes),
            );
        }
        None => set(&mut table, section, value),
    }
    write(path, &table)
}

/// The profiles there are and which one is active (`None` = the base settings).
pub fn profiles() -> (Vec<String>, Option<String>) {
    let Some(table) = path().and_then(|p| read(&p).ok()) else {
        return (vec![], None);
    };
    let names = get(&table, "profiles.list")
        .and_then(Value::as_table)
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    (names, active_profile(&table))
}

/// Switches profile (`None` = the base) and announces it as the `profile.active` fact, which
/// the features listen for. The settings file watcher sees the change too.
pub fn set_active(name: Option<&str>) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
    {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut table = upgrade(read(&path).map_err(std::io::Error::other)?);
        match name {
            Some(n) => set(&mut table, "profiles.active", Value::String(n.into())),
            None => {
                if let Some(Value::Table(p)) = table.get_mut("profiles") {
                    p.remove("active");
                }
            }
        }
        write(&path, &table)?;
    }
    announce(name);
    Ok(())
}

static PROFILES: Mutex<(Vec<String>, Option<String>)> = Mutex::new((Vec::new(), None));

/// The profiles and the active one as last read or set; for the tray and Settings, which draw
/// often and shouldn't read the file each time.
pub fn cached_profiles() -> (Vec<String>, Option<String>) {
    PROFILES.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn announce(name: Option<&str>) {
    *PROFILES.lock().unwrap_or_else(|e| e.into_inner()) = profiles();
    super::devices::bus().facts().set(
        "profile.active",
        foxglove_devices::facts::Value::Text(name.unwrap_or("").into()),
    );
    // A running scene takes the new profile's look (#487).
    super::scenes_ui::profile_changed();
}

/// One "switch to this profile while this program is in front" rule.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    /// Lower-case exe name, e.g. `eldenring.exe`.
    pub program: String,
    pub profile: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct RuleSettings {
    /// Switch by program at all.
    auto: bool,
    rules: Vec<Rule>,
}

impl Default for RuleSettings {
    fn default() -> Self {
        RuleSettings {
            auto: true,
            rules: vec![],
        }
    }
}

/// Whether switching by program is on, and the rules.
pub fn rules() -> (bool, Vec<Rule>) {
    match path().and_then(|p| read(&p).ok()) {
        Some(table) => rules_in(&table),
        None => (true, vec![]),
    }
}

fn rules_in(table: &Table) -> (bool, Vec<Rule>) {
    let mut t = Table::new();
    for key in ["auto", "rules"] {
        if let Some(v) = get(table, &format!("profiles.{key}")) {
            t.insert(key.into(), v.clone());
        }
    }
    let r: RuleSettings = Value::Table(t).try_into().unwrap_or_default();
    (r.auto, r.rules)
}

/// Saves the rules and the on/off switch, leaving the profiles themselves alone.
pub fn set_rules(auto: bool, rules: &[Rule]) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut table = upgrade(read(&path).map_err(std::io::Error::other)?);
    set(&mut table, "profiles.auto", Value::Boolean(auto));
    let list = Value::try_from(rules.to_vec()).map_err(std::io::Error::other)?;
    set(&mut table, "profiles.rules", list);
    write(&path, &table)
}

/// A file holding profiles to share: their overrides and the rules that switch to them, never
/// the machine-specific settings (`.foxprofile`, TOML).
pub fn export(names: &[String]) -> Result<String, String> {
    let table = read(&path().ok_or("no settings directory")?)?;
    export_from(&table, names)
}

fn export_from(table: &Table, names: &[String]) -> Result<String, String> {
    let mut out = Table::new();
    out.insert("format".into(), Value::String("foxglove-profile".into()));
    out.insert("version".into(), Value::Integer(1));
    for name in names {
        let p = get(table, &format!("profiles.list.{name}")).ok_or(format!("no profile {name}"))?;
        set(&mut out, &format!("profiles.{name}"), keep_profiled(p));
    }
    let rules: Vec<Rule> = rules_in(table)
        .1
        .into_iter()
        .filter(|r| names.contains(&r.profile))
        .collect();
    if !rules.is_empty() {
        out.insert(
            "rules".into(),
            Value::try_from(rules).map_err(|e| e.to_string())?,
        );
    }
    toml::to_string_pretty(&out).map_err(|e| e.to_string())
}

/// Only the profiled sections of a profile's table.
fn keep_profiled(profile: &Value) -> Value {
    let mut t = Table::new();
    if let Some(src) = profile.as_table() {
        for section in PROFILED {
            if let Some(v) = get(src, section) {
                set(&mut t, section, v.clone());
            }
        }
    }
    Value::Table(t)
}

/// The name an imported profile gets: its own, except one called "Default" (any case), which
/// would look like the built-in Default (#535) and becomes "Default (imported)".
pub fn imported_name(name: &str) -> String {
    if name.trim().eq_ignore_ascii_case("default") {
        format!("{} (imported)", name.trim())
    } else {
        name.to_string()
    }
}

/// Reads a `.foxprofile`: the profile names it holds (as they'll be imported), and which
/// already exist here.
pub fn import_names(text: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let (incoming, _) = parse_export(text)?;
    let existing = profiles().0;
    let names: Vec<String> = incoming.keys().map(|n| imported_name(n)).collect();
    let clashes = names
        .iter()
        .filter(|n| existing.contains(n))
        .cloned()
        .collect();
    Ok((names, clashes))
}

fn parse_export(text: &str) -> Result<(Table, Vec<Rule>), String> {
    let t: Table = text
        .parse()
        .map_err(|e| format!("not a profile file: {e}"))?;
    if get(&t, "format").and_then(Value::as_str) != Some("foxglove-profile") {
        return Err("not a Foxglove profile file".into());
    }
    if get(&t, "version").and_then(Value::as_integer) != Some(1) {
        return Err("made by a newer Foxglove".into());
    }
    let profiles = get(&t, "profiles")
        .and_then(Value::as_table)
        .cloned()
        .unwrap_or_default();
    let rules: Vec<Rule> = get(&t, "rules")
        .cloned()
        .map(Value::try_into)
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    Ok((profiles, rules))
}

/// Adds the profiles in a `.foxprofile`, replacing ones with the same name, and their rules.
pub fn import(text: &str) -> Result<Vec<String>, String> {
    let path = path().ok_or("no settings directory")?;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut table = upgrade(read(&path)?);
    let names = import_into(&mut table, text)?;
    write(&path, &table).map_err(|e| e.to_string())?;
    drop(_guard);
    announce(profiles().1.as_deref());
    Ok(names)
}

fn import_into(table: &mut Table, text: &str) -> Result<Vec<String>, String> {
    let (incoming, mut rules) = parse_export(text)?;
    let mut names = Vec::new();
    for (name, profile) in incoming {
        if name.is_empty() || name.contains('.') {
            return Err(format!("bad profile name {name:?}"));
        }
        let name = {
            let to = imported_name(&name);
            for r in rules.iter_mut().filter(|r| r.profile == name) {
                r.profile.clone_from(&to);
            }
            to
        };
        set(
            table,
            &format!("profiles.list.{name}"),
            keep_profiled(&profile),
        );
        names.push(name);
    }
    let (auto, mut existing) = rules_in(table);
    existing.retain(|r| !rules.iter().any(|n| n.program == r.program));
    existing.extend(rules);
    set(table, "profiles.auto", Value::Boolean(auto));
    set(
        table,
        "profiles.rules",
        Value::try_from(existing).map_err(|e| e.to_string())?,
    );
    Ok(names)
}

/// Creates the starter profiles (empty) the first time and announces the active one. Called
/// once at start-up.
pub fn init_profiles() {
    let Some(path) = path() else { return };
    {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(table) = read(&path) {
            if get(&table, "profiles").is_none() {
                let mut table = upgrade(table);
                for name in STARTERS {
                    set(
                        &mut table,
                        &format!("profiles.list.{name}"),
                        Value::Table(Table::new()),
                    );
                }
                if let Err(e) = write(&path, &table) {
                    eprintln!("Foxglove: can't create the starter profiles: {e}");
                }
            }
        }
    }
    announce(profiles().1.as_deref());
}

/// A section, or `None` when it isn't there or the store can't be read right now (keep what
/// you have). A caller that caches the result should use [`load`], so a failed read isn't kept.
///
/// Internal (#1143, P10): features reach their sections through [`section`]; this takes any
/// name, and is kept for the store itself and for tests.
#[cfg_attr(not(test), allow(dead_code))] // only tests use it now
pub fn try_load<T: DeserializeOwned>(section: &str) -> Option<T> {
    load(section).ok().flatten()
}

/// A section: `Ok(None)` when it isn't there (defaults are right), `Err` when the store can't be
/// read (don't keep defaults in its place; saves of the section are refused meanwhile).
///
/// Internal (#1143, P10): features reach their sections through [`section`]; this takes any
/// name, and is kept for the store itself and for tests.
pub fn load<T: DeserializeOwned>(section: &str) -> Result<Option<T>, String> {
    try_load_at(&path().ok_or("no settings folder")?, section)
}

/// Saves one section, leaving the others as they are.
///
/// Internal (#1143, P10): features reach their sections through [`section`]; this takes any
/// name, and is kept for the store itself and for tests.
pub fn save<T: Serialize>(section: &str, value: &T) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
    save_at(&path, section, value, false)
}

/// Read-modify-write of one machine section (not a [`PROFILED`] one) under the store's lock:
/// `change` gets the section as it is on disk now (its defaults when missing), and what it
/// leaves is saved, so two writers of different keys in the section can't lose each other's
/// change. A store that can't be read is left alone (`Err`): defaults never overwrite it.
///
/// Internal (#1143, P10): features reach their sections through [`section`]; this takes any
/// name, and is kept for the store itself and for tests.
pub fn update<T: DeserializeOwned + Serialize + Default>(
    section: &str,
    change: impl FnOnce(&mut T),
) -> std::io::Result<T> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
    update_at(&path, section, change)
}

fn update_at<T: DeserializeOwned + Serialize + Default>(
    path: &Path,
    section: &str,
    change: impl FnOnce(&mut T),
) -> std::io::Result<T> {
    if PROFILED.contains(&section) {
        return Err(std::io::Error::other(format!(
            "[{section}] is profiled: save it with `save`"
        )));
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut table = upgrade(read(path).map_err(std::io::Error::other)?);
    let mut value: T = match get(&table, section) {
        Some(v) => v
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| std::io::Error::other(format!("[{section}]: {e}")))?,
        None => T::default(),
    };
    change(&mut value);
    set(
        &mut table,
        section,
        Value::try_from(&value).map_err(std::io::Error::other)?,
    );
    write(path, &table)?;
    Ok(value)
}

/// A feature's section. The first time it's missing, `migrate` imports the feature's old
/// settings and names the files it read; they're saved here and renamed `*.migrated`. With
/// nothing to import, the defaults.
///
/// Internal (#1143, P10): features reach their sections through [`section`]; this takes any
/// name, and is kept for the store itself and for tests.
pub fn load_or_migrate<T: DeserializeOwned + Serialize + Default>(
    section: &str,
    migrate: impl FnOnce() -> Option<(T, Vec<PathBuf>)>,
) -> T {
    let Some(path) = path() else {
        return T::default();
    };
    load_or_migrate_at(&path, section, migrate)
}

fn load_or_migrate_at<T: DeserializeOwned + Serialize + Default>(
    path: &Path,
    section: &str,
    migrate: impl FnOnce() -> Option<(T, Vec<PathBuf>)>,
) -> T {
    match try_load_at(path, section) {
        Ok(Some(v)) => v,
        Ok(None) => match migrate() {
            Some((v, files)) => {
                match save_at(path, section, &v, true) {
                    Ok(()) => {
                        for f in files {
                            let mut done = f.clone().into_os_string();
                            done.push(".migrated");
                            let _ = std::fs::rename(&f, done);
                        }
                    }
                    Err(e) => eprintln!(
                        "Foxglove: can't save [{section}] to {}: {e}",
                        path.display()
                    ),
                }
                v
            }
            None => T::default(),
        },
        Err(e) => {
            // There but unreadable: run on defaults and leave the file for the user to fix.
            eprintln!("Foxglove: {e}; using default [{section}] settings");
            T::default()
        }
    }
}

/// One store section, opened by the feature that owns it (#1143, P10; H2.5).
///
/// [`section`] checks the feature table ([`super::features::FEATURES`]): a section has exactly
/// one owner, and only that feature gets a working handle. A handle for a section the feature
/// doesn't own is refused at open time (an error on the console, then every use of it fails
/// with the same message and the file is never touched), so one feature can't read or
/// overwrite another's settings. The file and its format are unchanged; a handle calls the
/// same code as the free functions below.
#[derive(Clone, Debug)]
pub struct Section {
    name: &'static str,
    /// `Err` with the reason when the owner check failed at open time.
    access: Result<(), String>,
}

/// Opens `name` for `feature_id`. Never panics: a refused section is reported (see [`Section`])
/// and its uses fail, so a wrong id can't take the app down, and a test of the feature table
/// (`every_section_opens_for_its_owner`) keeps the call sites honest.
pub fn section(feature_id: &str, name: &'static str) -> Section {
    let access = check_owner(feature_id, name);
    if let Err(e) = &access {
        eprintln!("Foxglove: {e}");
    }
    Section { name, access }
}

fn check_owner(feature_id: &str, name: &str) -> Result<(), String> {
    use super::features;
    if !features::is_feature(feature_id) {
        return Err(format!(
            "settings: [{name}] opened by `{feature_id}`, which isn't a feature in the table"
        ));
    }
    match features::section_owner(name) {
        Some(owner) if owner == feature_id => Ok(()),
        Some(owner) => Err(format!(
            "settings: [{name}] belongs to `{owner}`; `{feature_id}` can't open it"
        )),
        None => Err(format!(
            "settings: [{name}] has no owner in the feature table; add it to `{feature_id}`'s store_sections"
        )),
    }
}

/// A handle on a section of a test's own (tests share one settings folder, so a test that
/// saves keeps to a section of its own); skips the owner table but not the sandbox.
#[cfg(test)]
pub fn test_section(name: &'static str) -> Section {
    test_config_dir();
    Section {
        name,
        access: Ok(()),
    }
}

impl Section {
    /// The section's name, as it is in the file.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Whether this handle may be used (the open-time owner check).
    pub fn check(&self) -> Result<(), String> {
        self.access.clone()
    }

    /// Like [`try_load`]; `None` too when the handle isn't allowed.
    pub fn try_load<T: DeserializeOwned>(&self) -> Option<T> {
        self.load().ok().flatten()
    }

    /// Like [`load`]; `Err` when the handle isn't allowed.
    pub fn load<T: DeserializeOwned>(&self) -> Result<Option<T>, String> {
        self.check()?;
        load(self.name)
    }

    /// Like [`save`]; refused when the handle isn't allowed.
    pub fn save<T: Serialize>(&self, value: &T) -> std::io::Result<()> {
        self.check().map_err(std::io::Error::other)?;
        save(self.name, value)
    }

    /// Like [`update`]; refused when the handle isn't allowed.
    pub fn update<T: DeserializeOwned + Serialize + Default>(
        &self,
        change: impl FnOnce(&mut T),
    ) -> std::io::Result<T> {
        self.check().map_err(std::io::Error::other)?;
        update(self.name, change)
    }

    /// Like [`load_or_migrate`]; the defaults (and no import) when the handle isn't allowed.
    pub fn load_or_migrate<T: DeserializeOwned + Serialize + Default>(
        &self,
        migrate: impl FnOnce() -> Option<(T, Vec<PathBuf>)>,
    ) -> T {
        if self.check().is_err() {
            return T::default();
        }
        load_or_migrate(self.name, migrate)
    }
}

/// Test builds: the per-process test settings folder ([`test_config_dir`], set up on first
/// use); `None` in the app. Every path Foxglove resolves under its settings folder or
/// `%LOCALAPPDATA%\Foxglove` asks this first, so a test reaches only its own temp folder,
/// whether it called `test_config_dir()` or not. A test once wrote into the user's real
/// `foxglove.toml` because it saved before setting the folder up; that can't happen any more.
#[inline]
pub fn test_sandbox() -> Option<&'static Path> {
    #[cfg(test)]
    {
        Some(test_config_dir())
    }
    #[cfg(not(test))]
    {
        None
    }
}

/// Test builds: panics on a store read or write outside [`test_sandbox`], the backstop for a
/// path built some other way. Nothing in the app.
#[inline]
fn assert_sandboxed(path: &Path) {
    if let Some(dir) = test_sandbox() {
        assert!(
            path.starts_with(dir),
            "a test touched {}, outside the test settings folder {}; build test paths under \
             store::test_config_dir()",
            path.display(),
            dir.display()
        );
    }
}

/// One temp settings folder for every test in this process. FOXGLOVE_CONFIG_DIR is
/// process-wide and tests run in parallel, so they share one folder (each its own sections)
/// instead of racing to point it elsewhere. Never the user's real settings. Code that resolves
/// a settings path sets it up itself ([`test_sandbox`]); a test calls this only to get the
/// folder.
#[cfg(test)]
pub fn test_config_dir() -> &'static Path {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir()
            .join(format!("foxglove-tests-{}", std::process::id()))
            .join("config");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp settings folder");
        std::env::set_var("FOXGLOVE_CONFIG_DIR", &dir);
        dir
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Demo {
        on: bool,
        n: u32,
    }

    fn temp(name: &str) -> PathBuf {
        let dir = test_config_dir().join(format!("store-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sections_save_independently_and_carry_the_schema() {
        let dir = temp("sections");
        let path = dir.join("foxglove.toml");
        save_at(&path, "screens", &Demo { on: true, n: 4 }, false).unwrap();
        save_at(&path, "art.cycle", &Demo { on: false, n: 2 }, false).unwrap();
        assert_eq!(
            try_load_at::<Demo>(&path, "screens").unwrap(),
            Some(Demo { on: true, n: 4 })
        );
        assert_eq!(
            try_load_at::<Demo>(&path, "art.cycle").unwrap(),
            Some(Demo { on: false, n: 2 })
        );
        assert_eq!(try_load_at::<Demo>(&path, "updates").unwrap(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("schema = 1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_stays_inside_the_config_dir() {
        let tmp = std::env::temp_dir().join("foxglove-run-1");
        assert_eq!(
            path_for(Some(tmp.clone()), Some(tmp.join("settings.ini"))),
            Some(tmp.join("foxglove.toml"))
        );
        // Built with joins, so the paths are real ones on Linux too (CI).
        let foxglove = PathBuf::from(if cfg!(windows) {
            r"C:\Users\u\AppData\Roaming"
        } else {
            "/home/u/.config"
        })
        .join("Foxglove");
        let real = foxglove.join("config").join("settings.ini");
        assert_eq!(
            path_for(None, Some(real)),
            Some(foxglove.join("foxglove.toml"))
        );
        assert_eq!(path_for(None, None), None);
    }

    #[test]
    fn kits_section_reads_as_plugins() {
        let dir = temp("rename");
        let path = dir.join("foxglove.toml");
        std::fs::write(&path, "schema = 1\n[kits.installed.\"com.example.a\"]\nenabled = true\n[kits.trust]\nsequence = 3\n").unwrap();
        let v: Option<toml::Value> = try_load_at(&path, "plugins.trust").unwrap();
        assert_eq!(
            v.and_then(|v| v.get("sequence").and_then(toml::Value::as_integer)),
            Some(3)
        );
        save_at(&path, "screens", &toml::Table::new(), true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("[plugins.installed") && !text.contains("[kits"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_once_and_keeps_the_old_file() {
        let dir = temp("migrate");
        let path = dir.join("foxglove.toml");
        let old = dir.join("screens.ini");
        std::fs::write(&old, "[screens]\nfps=7\n").unwrap();
        let v: Demo = load_or_migrate_at(&path, "screens", || {
            Some((Demo { on: true, n: 7 }, vec![old.clone()]))
        });
        assert_eq!(v, Demo { on: true, n: 7 });
        assert!(!old.exists() && dir.join("screens.ini.migrated").exists());
        let again: Demo = load_or_migrate_at(&path, "screens", || unreachable!("already migrated"));
        assert_eq!(again, v);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// #741: a read that fails at start must not become defaults that a later save writes over
    /// the real section; a missing file still means defaults that save as usual.
    #[test]
    fn a_failed_read_never_becomes_saved_defaults() {
        let dir = temp("failed-read");
        let path = dir.join("foxglove.toml");
        std::fs::write(&path, "schema = 1\n[screens]\non = true\nn = 7\n").unwrap();
        FAIL_READS.lock().unwrap().insert(path.clone());
        assert!(try_load_at::<Demo>(&path, "screens").is_err());
        // What a caching feature would hold: the defaults.
        let held: Demo = load_or_migrate_at(&path, "screens", || unreachable!("never migrate"));
        assert_eq!(held, Demo::default());
        // Its save is refused, even once the file reads again, until the section is read.
        assert!(save_at(&path, "screens", &held, false).is_err());
        FAIL_READS.lock().unwrap().remove(&path);
        let err = save_at(&path, "screens", &held, false).unwrap_err();
        assert!(err.to_string().contains("[screens] wasn't read"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "schema = 1\n[screens]\non = true\nn = 7\n",
            "the real settings are untouched"
        );
        // Other sections were read fine and save as usual, keeping [screens].
        assert_eq!(try_load_at::<Demo>(&path, "art").unwrap(), None);
        save_at(&path, "art", &Demo { on: true, n: 1 }, false).unwrap();
        // A real read of the section lifts the hold.
        let real = try_load_at::<Demo>(&path, "screens").unwrap().unwrap();
        assert_eq!(real, Demo { on: true, n: 7 });
        save_at(&path, "screens", &Demo { n: 8, ..real }, false).unwrap();
        assert_eq!(
            try_load_at::<Demo>(&path, "screens").unwrap(),
            Some(Demo { on: true, n: 8 })
        );

        // A missing file is a first run: no hold.
        let fresh = dir.join("fresh.toml");
        assert_eq!(try_load_at::<Demo>(&fresh, "screens").unwrap(), None);
        save_at(&fresh, "screens", &Demo::default(), false).unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_keys_take_defaults_and_broken_files_are_left_alone() {
        let dir = temp("broken");
        let path = dir.join("foxglove.toml");
        std::fs::write(&path, "schema = 1\n[screens]\non = true\n").unwrap();
        assert_eq!(
            try_load_at::<Demo>(&path, "screens").unwrap(),
            Some(Demo { on: true, n: 0 })
        );
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert!(try_load_at::<Demo>(&path, "screens").is_err());
        let v: Demo = load_or_migrate_at(&path, "screens", || {
            unreachable!("never migrate over a broken store")
        });
        assert_eq!(v, Demo::default());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "this is = = not toml"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn profiles_override_and_capture_only_what_differs() {
        let dir = temp("profiles");
        let path = dir.join("foxglove.toml");
        save_at(&path, "screens", &Demo { on: true, n: 4 }, false).unwrap();
        // Switch to Game by hand (set_active uses the app's paths).
        let mut t = read(&path).unwrap();
        set(&mut t, "profiles.list.Game", Value::Table(Table::new()));
        set(&mut t, "profiles.active", Value::String("Game".into()));
        write(&path, &t).unwrap();
        save_at(&path, "screens", &Demo { on: true, n: 10 }, false).unwrap();
        let t = read(&path).unwrap();
        assert_eq!(
            get(&t, "screens.n").and_then(Value::as_integer),
            Some(4),
            "base kept"
        );
        let over = get(&t, "profiles.list.Game.screens")
            .and_then(Value::as_table)
            .unwrap();
        assert_eq!(over.len(), 1, "only the changed key: {over:?}");
        assert_eq!(
            try_load_at::<Demo>(&path, "screens").unwrap(),
            Some(Demo { on: true, n: 10 })
        );
        // Not profiled: always the base.
        save_at(&path, "connection", &Demo { on: true, n: 1 }, false).unwrap();
        assert!(get(&read(&path).unwrap(), "profiles.list.Game.connection").is_none());
        // Back to the base settings.
        let mut t = read(&path).unwrap();
        t.get_mut("profiles")
            .and_then(Value::as_table_mut)
            .unwrap()
            .remove("active");
        write(&path, &t).unwrap();
        assert_eq!(
            try_load_at::<Demo>(&path, "screens").unwrap(),
            Some(Demo { on: true, n: 4 })
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merge_and_diff_are_inverse_for_nested_tables() {
        let base: Table = toml::from_str("a = 1\n[t]\nx = 1\ny = 2\n").unwrap();
        let new: Table = toml::from_str("a = 1\n[t]\nx = 1\ny = 3\nz = 4\n").unwrap();
        let d = diff(&base, &new);
        assert_eq!(toml::to_string(&d).unwrap(), "[t]\ny = 3\nz = 4\n");
        let mut merged = base.clone();
        merge(&mut merged, d);
        assert_eq!(merged, new);
    }

    /// A hand-edited file with a value where a section belongs saves instead of panicking.
    #[test]
    fn a_value_where_a_section_belongs_is_replaced() {
        let mut t: Table = toml::from_str(
            "scenes = 1
keep = true
",
        )
        .unwrap();
        set(&mut t, "scenes.look", Value::Integer(5));
        assert_eq!(get(&t, "scenes.look").and_then(Value::as_integer), Some(5));
        assert_eq!(t.get("keep").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn rules_round_trip_and_default_to_on() {
        let mut t = Table::new();
        assert_eq!(rules_in(&t), (true, vec![]));
        let rules = vec![Rule {
            program: "eldenring.exe".into(),
            profile: "Game".into(),
        }];
        set(&mut t, "profiles.auto", Value::Boolean(false));
        set(
            &mut t,
            "profiles.rules",
            Value::try_from(rules.clone()).unwrap(),
        );
        let text = toml::to_string(&t).unwrap();
        assert_eq!(rules_in(&text.parse().unwrap()), (false, rules));
    }

    #[test]
    fn export_import_round_trip_without_machine_settings() {
        let mut t: Table = toml::from_str(
            "[connection.last]\nid = \"COM4\"\n[profiles.list.Game.screens]\nfps = 10\n[profiles.list.Game.connection]\nid = \"leak\"\n[[profiles.rules]]\nprogram = \"eldenring.exe\"\nprofile = \"Game\"\n",
        )
        .unwrap();
        let text = export_from(&t, &["Game".into()]).unwrap();
        assert!(text.contains("fps = 10") && text.contains("eldenring.exe"));
        assert!(
            !text.contains("COM4") && !text.contains("leak"),
            "machine settings stay home: {text}"
        );
        let mut fresh = Table::new();
        assert_eq!(
            import_into(&mut fresh, &text).unwrap(),
            vec!["Game".to_string()]
        );
        assert_eq!(
            get(&fresh, "profiles.list.Game.screens.fps").and_then(Value::as_integer),
            Some(10)
        );
        assert_eq!(rules_in(&fresh).1.len(), 1);
        assert!(import_into(&mut t, "format = \"other\"").is_err());
        assert!(import_into(&mut t, "format = \"foxglove-profile\"\nversion = 2").is_err());
    }

    /// An imported profile called "Default" can't pass for the built-in Default (#535): it
    /// comes in as "Default (imported)", and its program rules follow it.
    #[test]
    fn an_imported_default_gets_its_own_name() {
        assert_eq!(imported_name("Default"), "Default (imported)");
        assert_eq!(imported_name("default "), "default (imported)");
        assert_eq!(imported_name("Game"), "Game");
        let source: Table = toml::from_str(
            "[profiles.list.Default.screens]\nfps = 5\n[[profiles.rules]]\nprogram = \"code.exe\"\nprofile = \"Default\"\n",
        )
        .unwrap();
        let text = export_from(&source, &["Default".into()]).unwrap();
        let mut fresh = Table::new();
        assert_eq!(
            import_into(&mut fresh, &text).unwrap(),
            vec!["Default (imported)".to_string()]
        );
        assert!(get(&fresh, "profiles.list.Default").is_none());
        assert_eq!(rules_in(&fresh).1[0].profile, "Default (imported)");
    }

    /// `update` reads the section under the lock, so each writer's key survives the other's
    /// save; a profiled section is refused, and an unreadable store is never overwritten.
    #[test]
    fn update_is_read_modify_write_and_leaves_a_broken_store_alone() {
        let dir = temp("update");
        let path = dir.join("foxglove.toml");
        save_at(&path, "screens", &Demo { on: true, n: 4 }, false).unwrap();
        let a: Demo = update_at(&path, "machine", |d: &mut Demo| d.n = 7).unwrap();
        assert_eq!(a, Demo { on: false, n: 7 });
        let b: Demo = update_at(&path, "machine", |d: &mut Demo| d.on = true).unwrap();
        assert_eq!(b, Demo { on: true, n: 7 }, "the first writer's key is kept");
        assert_eq!(
            try_load_at::<Demo>(&path, "screens").unwrap(),
            Some(Demo { on: true, n: 4 }),
            "other sections untouched"
        );
        assert!(update_at(&path, "screens", |_: &mut Demo| {}).is_err());
        std::fs::write(&path, "not [valid").unwrap();
        assert!(update_at(&path, "machine", |d: &mut Demo| d.n = 1).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not [valid");
    }

    /// A real-looking settings folder: `%APPDATA%\Foxglove` as a fake, never the user's.
    fn fake_appdata() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\Users\u\AppData\Roaming"
        } else {
            "/home/u/.config"
        })
        .join("Foxglove")
    }

    /// A test that saves without setting anything up lands in the test folder: the guard in
    /// `path()` doesn't depend on FOXGLOVE_CONFIG_DIR having been set by someone first.
    /// Fails if the guard is removed: `test_sandbox()` must be the test folder in a test build.
    #[test]
    fn a_save_with_no_setup_lands_in_the_test_folder() {
        let sandbox = test_sandbox().expect("a test build always has its sandbox");
        let root = std::env::temp_dir().join(format!("foxglove-tests-{}", std::process::id()));
        assert!(sandbox.starts_with(&root), "{}", sandbox.display());
        let path = path().unwrap();
        assert_eq!(path, sandbox.join("foxglove.toml"));
        assert!(!path.starts_with(fake_appdata()));
        save("guard_probe", &Demo { on: true, n: 41 }).unwrap();
        assert_eq!(
            try_load_at::<Demo>(&path, "guard_probe").unwrap(),
            Some(Demo { on: true, n: 41 })
        );
    }

    /// Every path Foxglove builds under its settings folder or `%LOCALAPPDATA%\Foxglove`
    /// resolves inside the test folder in a test build (or to nothing).
    #[test]
    fn every_settings_path_stays_in_the_test_folder() {
        let sandbox = test_sandbox().unwrap();
        let inside = |what: &str, p: Option<PathBuf>| {
            let p = p.unwrap_or_else(|| panic!("{what}: no path"));
            assert!(p.starts_with(sandbox), "{what}: {}", p.display());
        };
        inside(
            "settings.ini",
            crate::settings::Settings::config_file_path(),
        );
        inside("foxglove.toml", path());
        inside("logs", crate::foxglove::app_log::dir());
        inside("plugins", crate::foxglove::plugin_install::plugins_dir());
        inside(
            "plugin catalog cache",
            crate::foxglove::plugin_trust::cache_dir(),
        );
        inside("art library", crate::foxglove::art_library::folder());
        assert_eq!(
            crate::foxglove::app_log::fallback_dir(),
            None,
            "log fallback"
        );
    }

    /// The backstop: a store read or write outside the test folder panics in a test build,
    /// even when the path was built by hand (here a fake `%APPDATA%`, never touched).
    #[test]
    #[should_panic(expected = "outside the test settings folder")]
    fn a_store_write_outside_the_test_folder_panics() {
        let _ = save_at(
            &fake_appdata().join("foxglove.toml"),
            "guard_probe",
            &Demo::default(),
            true,
        );
    }

    #[test]
    #[should_panic(expected = "outside the test settings folder")]
    fn a_store_read_outside_the_test_folder_panics() {
        let _ = try_load_at::<Demo>(&fake_appdata().join("foxglove.toml"), "guard_probe");
    }

    // --- Section handles (#1143, P10) ---

    #[test]
    fn a_feature_opens_its_own_section() {
        assert_eq!(section("foxglove.art", "art.cycle").check(), Ok(()));
        assert_eq!(section("hideout.pclights", "openrgb").name(), "openrgb");
    }

    #[test]
    fn a_feature_cannot_open_another_features_section() {
        let err = section("foxglove.screens", "art.cycle")
            .check()
            .unwrap_err();
        assert!(
            err.contains("[art.cycle] belongs to `foxglove.art`"),
            "{err}"
        );
        assert!(err.contains("`foxglove.screens` can't open it"), "{err}");
    }

    #[test]
    fn an_unowned_section_and_an_unknown_feature_are_refused() {
        let err = section("foxglove.art", "nobody_owns_this")
            .check()
            .unwrap_err();
        assert!(err.contains("no owner"), "{err}");
        let err = section("not.a.feature", "art").check().unwrap_err();
        assert!(err.contains("isn't a feature"), "{err}");
    }

    #[test]
    fn a_refused_handle_never_reads_or_writes_the_file() {
        test_config_dir();
        let file = path().unwrap();
        let before = std::fs::read(&file).ok();
        let wrong = section("foxglove.screens", "art.polarity");
        assert_eq!(wrong.try_load::<Demo>(), None);
        assert!(wrong.load::<Demo>().is_err());
        assert!(wrong.save(&Demo { on: true, n: 9 }).is_err());
        assert!(wrong.update(|d: &mut Demo| d.n = 9).is_err());
        let migrated: Demo = wrong.load_or_migrate(|| panic!("a refused handle must not import"));
        assert_eq!(migrated, Demo::default());
        assert_eq!(std::fs::read(&file).ok(), before, "the file was touched");
    }

    #[test]
    fn every_section_opens_for_its_owner_and_for_no_one_else() {
        for feat in super::super::features::FEATURES {
            for name in feat.store_sections {
                assert_eq!(section(feat.id, name).check(), Ok(()), "{name}");
                for other in super::super::features::FEATURES
                    .iter()
                    .filter(|o| o.id != feat.id)
                {
                    assert!(
                        section(other.id, name).check().is_err(),
                        "{name} / {}",
                        other.id
                    );
                }
            }
        }
    }

    #[test]
    fn a_handle_writes_the_same_file_an_older_build_reads() {
        // A file as the last release wrote it: `schema` and one table per section, dotted
        // sections nested. This is the read path an older build runs.
        let fixture = "schema = 1
[screens]
on = true
n = 3
[art.cycle]
on = false
n = 7
";
        let dir = temp("older-build");
        let old = dir.join("foxglove.toml");
        std::fs::write(&old, fixture).unwrap();
        assert_eq!(
            try_load_at::<Demo>(&old, "screens").unwrap(),
            Some(Demo { on: true, n: 3 })
        );
        assert_eq!(
            try_load_at::<Demo>(&old, "art.cycle").unwrap(),
            Some(Demo { on: false, n: 7 })
        );
        // A handle's save is the same bytes as the free save, so the format is unchanged.
        let by_handle = test_section("p10-fixture-handle");
        by_handle.save(&Demo { on: true, n: 5 }).unwrap();
        let text = std::fs::read_to_string(path().unwrap()).unwrap();
        let table: Table = text.parse().unwrap();
        assert_eq!(table.get("schema"), Some(&Value::Integer(SCHEMA)));
        let want = Value::try_from(&Demo { on: true, n: 5 }).unwrap();
        assert_eq!(table.get("p10-fixture-handle"), Some(&want));
        // And what the handle wrote reads back through the unchanged path.
        assert_eq!(
            try_load_at::<Demo>(&path().unwrap(), "p10-fixture-handle").unwrap(),
            Some(Demo { on: true, n: 5 })
        );
        assert_eq!(by_handle.try_load::<Demo>(), Some(Demo { on: true, n: 5 }));
        let _ = std::fs::remove_dir_all(dir);
    }
}
