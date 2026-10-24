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

/// `%APPDATA%\Foxglove\foxglove.toml` (the folder above KeyPeek's `config`).
pub fn path() -> Option<PathBuf> {
    let settings = crate::settings::Settings::config_file_path()?;
    Some(settings.parent()?.parent()?.join("foxglove.toml"))
}

fn read(path: &Path) -> Result<Table, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text.parse::<Table>().map_err(|e| format!("{}: {e}", path.display())),
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
    get(&table, section)
        .map(|v| v.clone().try_into().map_err(|e: toml::de::Error| format!("[{section}]: {e}")))
        .transpose()
}

fn save_at<T: Serialize>(path: &Path, section: &str, value: &T) -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let table = read(path).map_err(std::io::Error::other)?;
    let mut table = upgrade(table);
    set(&mut table, section, Value::try_from(value).map_err(std::io::Error::other)?);
    write(path, &table)
}

/// A section, or `None` when the store can't be read right now (keep what you have).
pub fn try_load<T: DeserializeOwned>(section: &str) -> Option<T> {
    try_load_at(&path()?, section).ok().flatten()
}

/// Saves one section, leaving the others as they are.
pub fn save<T: Serialize>(section: &str, value: &T) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings directory"))?;
    save_at(&path, section, value)
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
                match save_at(path, section, &v) {
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
        save_at(&path, "screens", &Demo { on: true, n: 4 }).unwrap();
        save_at(&path, "art.cycle", &Demo { on: false, n: 2 }).unwrap();
        assert_eq!(try_load_at::<Demo>(&path, "screens").unwrap(), Some(Demo { on: true, n: 4 }));
        assert_eq!(try_load_at::<Demo>(&path, "art.cycle").unwrap(), Some(Demo { on: false, n: 2 }));
        assert_eq!(try_load_at::<Demo>(&path, "updates").unwrap(), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("schema = 1"));
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
}
