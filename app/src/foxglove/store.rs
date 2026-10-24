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

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::Serialize;
use toml::{Table, Value};

/// The store's format version; each bump gets a migration in [`upgrade`].
pub const SCHEMA: i64 = 1;

static LOCK: Mutex<()> = Mutex::new(());

/// `%APPDATA%\Foxglove\foxglove.toml` (the folder above KeyPeek's `config`), or inside
/// `FOXGLOVE_CONFIG_DIR` when that's set, so a test run never shares or touches a store
/// outside its own folder.
pub fn path() -> Option<PathBuf> {
    path_for(std::env::var_os("FOXGLOVE_CONFIG_DIR").map(PathBuf::from), crate::settings::Settings::config_file_path())
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

fn read(path: &Path) -> Result<Table, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text.parse::<Table>().map(renamed).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Table::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write(path: &Path, table: &Table) -> std::io::Result<()> {
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
        t = t
            .entry(part)
            .or_insert_with(|| Value::Table(Table::new()))
            .as_table_mut()
            .expect("sections are tables");
    }
    t.insert(last.into(), value);
}

/// A section from the store at `path`: `Err` when the file can't be read now (locked by a
/// writer, or broken), `Ok(None)` when the section isn't there.
fn try_load_at<T: DeserializeOwned>(path: &Path, section: &str) -> Result<Option<T>, String> {
    let table = read(path)?;
    effective(&table, section)
        .map(|v| v.try_into().map_err(|e: toml::de::Error| format!("[{section}]: {e}")))
        .transpose()
}

/// Sections a profile may override (docs/profiles.md); the rest describe the machine.
pub const PROFILED: &[&str] = &["screens", "lighting", "art", "art.cycle", "overlay"];

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
/// Migrations always write the base (`to_base`).
fn save_at<T: Serialize>(path: &Path, section: &str, value: &T, to_base: bool) -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let table = read(path).map_err(std::io::Error::other)?;
    let mut table = upgrade(table);
    let value = Value::try_from(value).map_err(std::io::Error::other)?;
    match active_profile(&table).filter(|_| !to_base && PROFILED.contains(&section)) {
        Some(name) => {
            let base = get(&table, section).and_then(Value::as_table).cloned().unwrap_or_default();
            let changes = value.as_table().map_or_else(Table::new, |new| diff(&base, new));
            set(&mut table, &format!("profiles.list.{name}.{section}"), Value::Table(changes));
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
    super::devices::bus()
        .facts()
        .set("profile.active", foxglove_devices::facts::Value::Text(name.unwrap_or("").into()));
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
        RuleSettings { auto: true, rules: vec![] }
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
    let rules: Vec<Rule> = rules_in(table).1.into_iter().filter(|r| names.contains(&r.profile)).collect();
    if !rules.is_empty() {
        out.insert("rules".into(), Value::try_from(rules).map_err(|e| e.to_string())?);
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

/// Reads a `.foxprofile`: the profile names it holds, and which already exist here.
pub fn import_names(text: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let (incoming, _) = parse_export(text)?;
    let existing = profiles().0;
    let names: Vec<String> = incoming.keys().cloned().collect();
    let clashes = names.iter().filter(|n| existing.contains(n)).cloned().collect();
    Ok((names, clashes))
}

fn parse_export(text: &str) -> Result<(Table, Vec<Rule>), String> {
    let t: Table = text.parse().map_err(|e| format!("not a profile file: {e}"))?;
    if get(&t, "format").and_then(Value::as_str) != Some("foxglove-profile") {
        return Err("not a Foxglove profile file".into());
    }
    if get(&t, "version").and_then(Value::as_integer) != Some(1) {
        return Err("made by a newer Foxglove".into());
    }
    let profiles = get(&t, "profiles").and_then(Value::as_table).cloned().unwrap_or_default();
    let rules: Vec<Rule> = get(&t, "rules").cloned().map(Value::try_into).transpose().map_err(|e| e.to_string())?.unwrap_or_default();
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
    let (incoming, rules) = parse_export(text)?;
    let mut names = Vec::new();
    for (name, profile) in incoming {
        if name.is_empty() || name.contains('.') {
            return Err(format!("bad profile name {name:?}"));
        }
        set(table, &format!("profiles.list.{name}"), keep_profiled(&profile));
        names.push(name);
    }
    let (auto, mut existing) = rules_in(table);
    existing.retain(|r| !rules.iter().any(|n| n.program == r.program));
    existing.extend(rules);
    set(table, "profiles.auto", Value::Boolean(auto));
    set(table, "profiles.rules", Value::try_from(existing).map_err(|e| e.to_string())?);
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
                    set(&mut table, &format!("profiles.list.{name}"), Value::Table(Table::new()));
                }
                if let Err(e) = write(&path, &table) {
                    eprintln!("Foxglove: can't create the starter profiles: {e}");
                }
            }
        }
    }
    announce(profiles().1.as_deref());
}

/// A section, or `None` when the store can't be read right now (keep what you have).
pub fn try_load<T: DeserializeOwned>(section: &str) -> Option<T> {
    try_load_at(&path()?, section).ok().flatten()
}

/// Saves one section, leaving the others as they are.
pub fn save<T: Serialize>(section: &str, value: &T) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
    save_at(&path, section, value, false)
}

/// A feature's section. The first time it's missing, `migrate` imports the feature's old
/// settings and names the files it read; they're saved here and renamed `*.migrated`. With
/// nothing to import, the defaults.
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
                    Err(e) => eprintln!("Foxglove: can't save [{section}] to {}: {e}", path.display()),
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
        let dir = std::env::temp_dir().join(format!("foxglove-store-{name}-{}", std::process::id()));
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
        assert_eq!(try_load_at::<Demo>(&path, "screens").unwrap(), Some(Demo { on: true, n: 4 }));
        assert_eq!(try_load_at::<Demo>(&path, "art.cycle").unwrap(), Some(Demo { on: false, n: 2 }));
        assert_eq!(try_load_at::<Demo>(&path, "updates").unwrap(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("schema = 1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_stays_inside_the_config_dir() {
        let tmp = std::env::temp_dir().join("foxglove-run-1");
        assert_eq!(path_for(Some(tmp.clone()), Some(tmp.join("settings.ini"))), Some(tmp.join("foxglove.toml")));
        let real = PathBuf::from(r"C:\Users\u\AppData\Roaming\Foxglove\config\settings.ini");
        assert_eq!(
            path_for(None, Some(real)),
            Some(PathBuf::from(r"C:\Users\u\AppData\Roaming\Foxglove\foxglove.toml"))
        );
        assert_eq!(path_for(None, None), None);
    }

    #[test]
    fn kits_section_reads_as_plugins() {
        let dir = std::env::temp_dir().join(format!("foxglove-store-rename-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("foxglove.toml");
        std::fs::write(&path, "schema = 1\n[kits.installed.\"com.example.a\"]\nenabled = true\n[kits.trust]\nsequence = 3\n").unwrap();
        let v: Option<toml::Value> = try_load_at(&path, "plugins.trust").unwrap();
        assert_eq!(v.and_then(|v| v.get("sequence").and_then(toml::Value::as_integer)), Some(3));
        save_at(&path, "screens", &toml::Table::new(), true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[plugins.installed") && !text.contains("[kits"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn migrates_once_and_keeps_the_old_file() {
        let dir = temp("migrate");
        let path = dir.join("foxglove.toml");
        let old = dir.join("screens.ini");
        std::fs::write(&old, "[screens]\nfps=7\n").unwrap();
        let v: Demo = load_or_migrate_at(&path, "screens", || Some((Demo { on: true, n: 7 }, vec![old.clone()])));
        assert_eq!(v, Demo { on: true, n: 7 });
        assert!(!old.exists() && dir.join("screens.ini.migrated").exists());
        let again: Demo = load_or_migrate_at(&path, "screens", || unreachable!("already migrated"));
        assert_eq!(again, v);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_keys_take_defaults_and_broken_files_are_left_alone() {
        let dir = temp("broken");
        let path = dir.join("foxglove.toml");
        std::fs::write(&path, "schema = 1\n[screens]\non = true\n").unwrap();
        assert_eq!(try_load_at::<Demo>(&path, "screens").unwrap(), Some(Demo { on: true, n: 0 }));
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert!(try_load_at::<Demo>(&path, "screens").is_err());
        let v: Demo = load_or_migrate_at(&path, "screens", || unreachable!("never migrate over a broken store"));
        assert_eq!(v, Demo::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "this is = = not toml");
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
        assert_eq!(get(&t, "screens.n").and_then(Value::as_integer), Some(4), "base kept");
        let over = get(&t, "profiles.list.Game.screens").and_then(Value::as_table).unwrap();
        assert_eq!(over.len(), 1, "only the changed key: {over:?}");
        assert_eq!(try_load_at::<Demo>(&path, "screens").unwrap(), Some(Demo { on: true, n: 10 }));
        // Not profiled: always the base.
        save_at(&path, "connection", &Demo { on: true, n: 1 }, false).unwrap();
        assert!(get(&read(&path).unwrap(), "profiles.list.Game.connection").is_none());
        // Back to the base settings.
        let mut t = read(&path).unwrap();
        t.get_mut("profiles").and_then(Value::as_table_mut).unwrap().remove("active");
        write(&path, &t).unwrap();
        assert_eq!(try_load_at::<Demo>(&path, "screens").unwrap(), Some(Demo { on: true, n: 4 }));
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

    #[test]
    fn rules_round_trip_and_default_to_on() {
        let mut t = Table::new();
        assert_eq!(rules_in(&t), (true, vec![]));
        let rules = vec![Rule { program: "eldenring.exe".into(), profile: "Game".into() }];
        set(&mut t, "profiles.auto", Value::Boolean(false));
        set(&mut t, "profiles.rules", Value::try_from(rules.clone()).unwrap());
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
        assert!(!text.contains("COM4") && !text.contains("leak"), "machine settings stay home: {text}");
        let mut fresh = Table::new();
        assert_eq!(import_into(&mut fresh, &text).unwrap(), vec!["Game".to_string()]);
        assert_eq!(get(&fresh, "profiles.list.Game.screens.fps").and_then(Value::as_integer), Some(10));
        assert_eq!(rules_in(&fresh).1.len(), 1);
        assert!(import_into(&mut t, "format = \"other\"").is_err());
        assert!(import_into(&mut t, "format = \"foxglove-profile\"\nversion = 2").is_err());
    }
}
