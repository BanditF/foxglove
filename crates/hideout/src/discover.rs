// SPDX-License-Identifier: MIT
//! Product slice 5: data-driven discovery. The Modules page's **Found** card and **Add an app…**.
//!
//! Where apps come from, in this order:
//! 1. **Self-registration.** An app's installer writes `HKCU\Software\Hideout\Modules\<id>`; the
//!    key's default value (or a value named `Path`) is the folder that holds the app's
//!    `module.toml`, or the file itself. The manifest names the app, its version and its program;
//!    nothing here knows any app.
//! 2. **The fallback list** (`products.json`, read at run time by `find_installed`), for apps whose
//!    installers do not register themselves yet.
//! 3. **Browse**: the user picks an app's folder.
//!
//! Adding registers the app through the existing `--adopt` code ([`crate::adopt::run`]). It
//! writes no grant and starts nothing: the app then waits for the normal first-run Allow dialog,
//! which the page opens. Reads only; the machine (registry, files) and the folder picker are the
//! [`Machine`] and [`Picker`] traits, so every test runs on fakes and never touches the real
//! registry.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use hideout_contract::module::{Context, Kind, Module};

use crate::find_installed::{self, Found, Machine};

/// The registry key apps register under (below `HKCU`).
pub const MODULES_KEY: &str = r"Software\Hideout\Modules";

/// Every string the user sees from this feature.
pub mod words {
    pub const FOUND_TITLE: &str = "Found on this PC";
    pub const FOUND_BODY: &str =
        "Hideout can run these. Before one starts, you choose what it may do.";
    pub const ADD: &str = "Add";
    pub const ADD_AN_APP: &str = "Add an app\u{2026}";
    pub const ADD_TITLE: &str = "Add an app";
    pub const ADD_BODY: &str =
        "Choose the folder the app was installed in. Hideout looks there for the file that describes the app.";
    pub const NOT_NOW: &str = "Not now";
    pub const LIST_NOT_READ: &str =
        "The app was added, but the list could not be refreshed. Close Hideout and open it again.";
    pub const BROWSE: &str = "Browse\u{2026}";
    pub const CANCEL: &str = "Cancel";
    pub const NO_FILE: &str =
        "Hideout can't find the app here. Choose the folder the app was installed in.";
    // Each refusal ends with a next step that Browse can do from where it is shown.
    pub const NOT_READ: &str =
        "That app's file could not be read. Reinstall the app, then try again.";
    pub const NOT_ADDABLE: &str =
        "That app can't be added to Hideout. Choose another app's folder.";
    /// Also shown when the folder is on this PC but the app it names is on another drive.
    pub const NOT_THIS_PC: &str =
        "Hideout adds apps only from this PC's own drive. Choose a folder there.";
    /// The folder is one Hideout keeps for its own files (the hub's folder, or a program inside it).
    pub const INSIDE_HUB: &str =
        "Hideout keeps that folder for its own files. Choose the folder the app was installed in.";
    pub fn already(name: &str) -> String {
        format!("{name} is already in Hideout.")
    }
    /// The program the app's file names is not there.
    pub fn program_missing(name: &str) -> String {
        format!("The app file for {name} is missing. Reinstall it, then try again.")
    }
    /// Anything else that went wrong while adding (the raw reason goes to the log).
    pub fn not_saved(name: &str) -> String {
        format!("Hideout could not save {name}. Try again.")
    }
    /// The app's file changed between the look and the add.
    pub fn changed(name: &str) -> String {
        format!("{name} changed while Hideout was adding it. Try again.")
    }
    pub fn added(name: &str) -> String {
        format!("Added {name}. Choose what it may do.")
    }
    pub fn add_label(name: &str) -> String {
        format!("Add {name}")
    }
    /// The line for one found app.
    pub fn found_line(name: &str, version: &str) -> String {
        format!("{name} {version}")
    }
    pub fn add_hover(name: &str, folder: &str) -> String {
        format!("Add {name} to Hideout. Found in {folder}")
    }
}

/// Where a found app's registration comes from.
#[derive(Clone, Debug)]
pub enum Origin {
    /// A folder with the app's own `module.toml` (self-registered or browsed to).
    Folder(PathBuf),
    /// A fallback-list product, with a manifest made from its template.
    Product(Found),
}

/// An app Hideout could add.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub version: String,
    pub origin: Origin,
    /// The program the app's file names (the card is not offered when it is gone).
    pub program: Option<PathBuf>,
    /// The manifest text that was read and validated (folder origin). It is what gets
    /// registered: the add stages exactly this text, never a second read of the folder.
    pub text: Option<String>,
    /// Set when the app is already in Hideout (added from a file) and the file now names another
    /// version or program: the version Hideout holds. The card then offers the update.
    pub update_of: Option<String>,
}

impl Candidate {
    /// The card's button: **Add**, or **Update in Hideout** for a changed app Hideout holds.
    pub fn button(&self) -> String {
        if self.update_of.is_some() {
            find_installed::Row::NeedsRefresh
                .button(&self.name)
                .unwrap_or_else(|| words::ADD.to_string())
        } else {
            words::ADD.to_string()
        }
    }

    /// The Apps page's row line for a changed app Hideout holds (none for a new one). The model
    /// carries it; the page's row draws the same [`find_installed::Row`] words.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn row_line(&self) -> Option<String> {
        self.update_of
            .is_some()
            .then(|| find_installed::Row::NeedsRefresh.line(&self.name))
    }

    /// The folder the app was found in, for the card's detail line.
    pub fn folder(&self) -> String {
        let p = match &self.origin {
            Origin::Folder(f) => Some(f.clone()),
            Origin::Product(f) => f
                .folder
                .clone()
                .or_else(|| f.exe.parent().map(Path::to_path_buf)),
        };
        p.map(|p| p.display().to_string()).unwrap_or_default()
    }
}

/// Text a registrant controls, as the card shows it: no control characters, at most 64
/// characters.
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(64)
        .collect()
}

/// The folder picker (a trait so tests choose without a window).
pub trait Picker {
    /// The folder the user chose, or `None` when cancelled.
    fn pick_folder(&self) -> Option<PathBuf>;
}

/// An absolute, local path: never relative, never UNC.
fn is_local(p: &Path) -> bool {
    let s = p.to_string_lossy();
    p.is_absolute() && !s.starts_with(r"\\") && !s.starts_with("//")
}

fn is_manifest_file(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("module.toml"))
}

/// The app described by the manifest in `at`: the folder, the manifest file, or a folder with
/// the manifest one level down in `hideout\`. Plain-words error when it is not an addable app.
pub fn from_folder(m: &dyn Machine, home: &Path, at: &Path) -> Result<Candidate, String> {
    // Local fixed drives only. Checked on the text first (no disk access, so a dead network
    // drive can't stall the page), then on the canonical path, which is what is read and added.
    if !is_local(at) || !m.is_local_fixed(at) {
        return Err(words::NOT_THIS_PC.into());
    }
    let at = m.canonical(at).ok_or_else(|| words::NO_FILE.to_string())?;
    if !is_local(&at) || !m.is_local_fixed(&at) {
        return Err(words::NOT_THIS_PC.into());
    }
    let folder = if is_manifest_file(&at) {
        at.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        at
    };
    let (folder, text) = [folder.clone(), folder.join("hideout")]
        .into_iter()
        .find_map(|f| m.read_text(&f.join("module.toml")).map(|t| (f, t)))
        .ok_or_else(|| words::NO_FILE.to_string())?;
    let module =
        Module::parse(&text, &Context::default()).map_err(|_| words::NOT_READ.to_string())?;
    let program = match module.kind {
        Kind::Adopted => {
            let program = module
                .run
                .as_ref()
                .and_then(|r| r.first())
                .map(|r| folder.join(r));
            // The program is on a local fixed drive too (text and drive type, then resolved).
            match program {
                Some(p) => Some(
                    find_installed::local_path(m, &p)
                        .ok_or_else(|| words::NOT_THIS_PC.to_string())?,
                ),
                None => None,
            }
        }
        Kind::Service => Some(service_program(m, home, &folder, &module)?),
        _ => return Err(words::NOT_ADDABLE.into()),
    };
    Ok(Candidate {
        text: Some(text),
        id: module.id,
        name: clean(&module.name),
        version: clean(&module.version),
        origin: Origin::Folder(folder),
        program,
        update_of: None,
    })
}

/// Whether `p` is `base` or below it (component by component; ASCII case only, so a look-alike
/// letter in a folder name never counts as the same folder).
fn inside(p: &Path, base: &Path) -> bool {
    let parts = |x: &Path| -> Vec<String> {
        x.components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect()
    };
    let (p, base) = (parts(p), parts(base));
    p.len() >= base.len() && p[..base.len()] == base[..]
}

/// The program of a `service` app file in `folder`, as an absolute canonical path. In this
/// order: the folder is not Hideout's own (`<home>` or below); `run[0]` is a plain relative name
/// (the contract's stays-inside rule, enforced by the parse and checked again here: no `..`,
/// root or drive); it is joined to the folder, resolved, and must be on a local fixed drive;
/// the resolved program must still be below the folder (a link that leads out is refused).
fn service_program(
    m: &dyn Machine,
    home: &Path,
    folder: &Path,
    module: &Module,
) -> Result<PathBuf, String> {
    // The whole hub folder, resolved: its installed apps, records and settings are its own.
    let hub = m.canonical(home).unwrap_or_else(|| home.to_path_buf());
    if inside(folder, &hub) {
        return Err(words::INSIDE_HUB.into());
    }
    let run0 = module
        .run
        .as_ref()
        .and_then(|r| r.first())
        .ok_or_else(|| words::NOT_READ.to_string())?;
    let plain = Path::new(run0)
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !plain {
        return Err(words::NOT_ADDABLE.into());
    }
    let exe = find_installed::local_path(m, &folder.join(run0))
        .ok_or_else(|| words::NOT_THIS_PC.to_string())?;
    if exe.as_path() == folder || !inside(&exe, folder) {
        return Err(words::NOT_ADDABLE.into());
    }
    // A folder above the hub's must not name a program the hub installed (its hash would look
    // genuine under an id and permissions the file made up).
    if inside(&exe, &hub) {
        return Err(words::INSIDE_HUB.into());
    }
    Ok(exe)
}

/// A `service` app file as an `adopted` one: the same text with only `kind` and `run[0]`
/// rewritten (`run[0]` = `exe`). A table edit, so every other byte, comment and key stays.
fn adopted_text(text: &str, exe: &Path) -> Result<String, String> {
    let exe = exe
        .to_str()
        .ok_or_else(|| "the program's path is not text".to_string())?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("module.toml: {e}"))?;
    let kind = doc
        .get_mut("kind")
        .and_then(toml_edit::Item::as_value_mut)
        .ok_or("module.toml: no kind")?;
    let decor = kind.decor().clone();
    *kind = toml_edit::Value::from("adopted");
    *kind.decor_mut() = decor;
    let run = doc
        .get_mut("run")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or("module.toml: no run")?;
    if run.is_empty() {
        return Err("module.toml: run is empty".into());
    }
    run.replace(0, exe);
    Ok(doc.to_string())
}

/// Whether `staged` parses to `was` with only `kind = adopted` and `run[0] = exe` changed.
fn same_but_adopted(was: &Module, staged: &str, exe: Option<&Path>) -> bool {
    let (Some(exe), Some(run)) = (exe.and_then(Path::to_str), was.run.as_ref()) else {
        return false;
    };
    let mut want = was.clone();
    want.kind = Kind::Adopted;
    let mut run = run.clone();
    if run.is_empty() {
        return false;
    }
    run[0] = exe.to_string();
    want.run = Some(run);
    Module::parse(staged, &Context::default()).is_ok_and(|got| got == want)
}

/// The most apps read from the folder next to Hideout.
const BESIDE_CAP: usize = 32;

/// Apps shipped next to Hideout: `<folder of hideout.exe>\apps\<x>\module.toml`, one level,
/// read-only, the first [`BESIDE_CAP`] folders by name. Each goes through [`from_folder`]; then
/// the canonical folder and the canonical program must both be below the canonical `apps`
/// folder (a link that leads out is skipped), the id must be new (not in `installed`, not in
/// `found`, not twice here), and the program must exist. Anything else is skipped silently.
/// An id Hideout already holds (added from a file) is offered again as an update when the file's
/// version or program differs from the record ([`registered_adopted`]).
fn beside_hideout(
    m: &dyn Machine,
    home: &Path,
    installed: &[String],
    found: &[Candidate],
) -> Vec<Candidate> {
    let Some(exe_dir) = m.hub_exe_dir() else {
        return Vec::new();
    };
    let Some(root) = find_installed::local_path(m, &exe_dir.join("apps")) else {
        return Vec::new();
    };
    let mut subs = m.sub_folders(&root);
    subs.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    let mut out: Vec<Candidate> = Vec::new();
    for sub in subs.into_iter().take(BESIDE_CAP) {
        let mut c = match from_folder(m, home, &sub.join("module.toml")) {
            Ok(c) => c,
            Err(why) => {
                log::debug!("discover: {}: {why}", sub.display());
                continue;
            }
        };
        let (Origin::Folder(folder), Some(program)) = (&c.origin.clone(), &c.program.clone())
        else {
            continue;
        };
        // The program must sit in this entry's own folder: one entry cannot name another's.
        if !inside(folder, &root) || !inside(program, folder) {
            log::debug!("discover: {}: leads out of the apps folder; skipped", c.id);
            continue;
        }
        let again = found.iter().any(|f| f.id == c.id) || out.iter().any(|f| f.id == c.id);
        if again || !m.is_file(program) {
            continue;
        }
        if installed.contains(&c.id) {
            // Held already: only a changed file added by hand (never a catalog install) is
            // offered, as an update.
            let Some(held) = registered_adopted(home, &c.id) else {
                continue;
            };
            let same_program = held
                .run0
                .as_deref()
                .and_then(|r| find_installed::local_path(m, Path::new(r)))
                .is_some_and(|r| inside(&r, program) && inside(program, &r));
            if held.version == c.version && same_program {
                continue;
            }
            c.update_of = Some(held.version);
        }
        out.push(c);
    }
    out
}

/// What Hideout holds for an app added from a file.
struct Held {
    /// The version, as shown (the same cleaning as the file's).
    version: String,
    /// The first word of `run` in the record's own copy of the file.
    run0: Option<String>,
}

/// The record of `id` when it is an adopted one (a catalog install is not touched), else none.
fn registered_adopted(home: &Path, id: &str) -> Option<Held> {
    let reg = hideout_registry::Registry::open_from_env(home).ok()?;
    let e = reg.inventory.modules.get(id)?;
    if e.source != hideout_registry::inventory::ADOPTED {
        return None;
    }
    Some(Held {
        version: clean(&e.version),
        run0: reg
            .manifest(id)
            .and_then(|m| m.run)
            .and_then(|r| r.first().cloned()),
    })
}

/// The apps found on this PC that are not in Hideout yet (`installed` = the ids it already has):
/// self-registered ones first, then apps next to Hideout (an app Hideout holds comes again when
/// its file changed), then fallback-list products they did not already cover.
pub fn discover(m: &dyn Machine, home: &Path, installed: &[String]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for sub in m.registry_subkeys(MODULES_KEY) {
        let key = format!(r"{MODULES_KEY}\{sub}");
        let Some(path) = m
            .registry_string(&key, "")
            .or_else(|| m.registry_string(&key, "Path"))
        else {
            continue;
        };
        match from_folder(m, home, Path::new(&path)) {
            // The key names the app; a manifest for another id is not that key's app.
            // An app whose program is gone is not offered: there is nothing to add.
            Ok(c) if c.id == sub && c.program.as_deref().is_some_and(|p| m.is_file(p)) => {
                out.push(c)
            }
            Ok(c) if c.id == sub => log::info!("discover: {}: its program is gone", c.id),
            Ok(c) => log::warn!(
                "discover: key {sub} points at an app file for {}; skipped",
                c.id
            ),
            Err(why) => log::warn!("discover: key {sub}: {why}"),
        }
    }
    for c in beside_hideout(m, home, installed, &out) {
        out.push(c);
    }
    for p in find_installed::products() {
        if out.iter().any(|c| c.id == p.id) {
            continue;
        }
        if let Some(f) = find_installed::find(m, p) {
            out.push(Candidate {
                id: p.id.clone(),
                name: p.name.clone(),
                version: clean(&f.version),
                program: Some(f.exe.clone()),
                text: None,
                update_of: None,
                origin: Origin::Product(f),
            });
        }
    }
    out.retain(|c| c.update_of.is_some() || !installed.contains(&c.id));
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// Registers `c` in the hub folder `home` (the `--adopt` code; no grant, nothing started).
/// Returns the app's id. What the user saw is what is registered: the folder is read again
/// first and must still name the same app, and the id the adopt code registered must be the
/// card's. A failure is one plain sentence; the raw reason goes to the log.
pub fn add(
    m: &dyn Machine,
    home: &Path,
    staging: &Path,
    c: &Candidate,
    now: &str,
) -> Result<String, String> {
    // The ids that were registered before this call: a record of another id that the adopt code
    // makes is removed again, one that was there is never touched.
    let before: Option<Vec<String>> = hideout_registry::Registry::open_from_env(home)
        .ok()
        .map(|r| r.inventory.modules.keys().cloned().collect());
    let done = match (&c.origin, &c.text) {
        (Origin::Folder(f), Some(text)) => {
            // Still the same file, byte for byte (name, program and permissions too)...
            match from_folder(m, home, f) {
                Ok(again)
                    if again.text.as_deref() == Some(text.as_str())
                        && again.program == c.program => {}
                _ => return Err(words::changed(&c.name)),
            }
            // A service app file is registered as adopted: only `kind` and `run[0]` change.
            let text = match Module::parse(text, &Context::default()) {
                Ok(module) if module.kind == Kind::Service => {
                    match c.program.as_deref().map(|p| adopted_text(text, p)) {
                        // The rewrite must read back as the same app: only the two keys moved.
                        Some(Ok(t)) if same_but_adopted(&module, &t, c.program.as_deref()) => t,
                        other => {
                            log::warn!("discover: {} not staged: {other:?}", c.id);
                            return Err(words::not_saved(&c.name));
                        }
                    }
                }
                _ => text.clone(),
            };
            // The id the user was shown is the id that gets registered.
            if !Module::parse(&text, &Context::default()).is_ok_and(|module| module.id == c.id) {
                log::warn!("discover: {}: the staged text names another id", c.id);
                return Err(words::changed(&c.name));
            }
            // ...and what is registered is the text the user was shown, staged and adopted
            // from there, so a swap after this point cannot reach the inventory.
            let dir = staging.join(format!("add-{}", c.id));
            // An earlier record of this id (and its grants) is not ours to drop on a failed check.
            let fresh = !hideout_registry::Registry::open_from_env(home)
                .is_ok_and(|r| r.inventory.modules.contains_key(&c.id));
            let staged = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(dir.join("module.toml"), &text))
                .map_err(|e| format!("{}: {e}", dir.display()))
                .and_then(|()| crate::adopt::run(home, &dir, now));
            let _ = std::fs::remove_dir_all(&dir);
            // An update re-registered a record that had grants: check them now, whatever the
            // read-back below says, so no earlier Allow stays on a program it was not given for.
            if staged.is_ok() && c.update_of.is_some() {
                void_grants_after_update(home, &c.id, now);
            }
            // What the registry now holds must be the staged text, byte for byte.
            if staged.is_ok() && !registered_matches(home, &c.id, &text, fresh) {
                // The adopt code may have registered another id than the card's: that stray
                // record made by this call does not stay either.
                if let Ok(line) = &staged {
                    drop_stray(home, line, before.as_deref());
                }
                return Err(words::changed(&c.name));
            }
            staged
        }
        (Origin::Folder(_), None) => return Err(words::changed(&c.name)),
        (Origin::Product(found), _) => {
            match find_installed::adopt(m, home, staging, found, now) {
                // Registered, but not the text that was staged: the same words as a changed file.
                Err(why) if why.ends_with(find_installed::SWAPPED) => {
                    log::warn!("discover: {} not added: {why}", c.id);
                    return Err(words::changed(&c.name));
                }
                other => other,
            }
        }
    };
    match done {
        Ok(line) if line.starts_with(&format!("registered {} ", c.id)) => Ok(c.id.clone()),
        Ok(line) => {
            log::warn!(
                "discover: asked to add {} but the adopt code said: {line}",
                c.id
            );
            drop_stray(home, &line, before.as_deref());
            Err(words::changed(&c.name))
        }
        Err(why) => {
            log::warn!("discover: {} not added: {why}", c.id);
            let gone = c.program.as_deref().is_some_and(|p| !m.is_file(p));
            Err(if gone {
                words::program_missing(&c.name)
            } else {
                words::not_saved(&c.name)
            })
        }
    }
}

/// The adopt code answered with another id than the card's: that record is removed again when
/// it was not there before the call (`before`; unknown = nothing is removed). A record that
/// existed is never dropped.
pub(crate) fn drop_stray(home: &Path, line: &str, before: Option<&[String]>) {
    let Some(other) = line
        .strip_prefix("registered ")
        .and_then(|r| r.split(' ').next())
    else {
        return;
    };
    if before.is_none_or(|b| b.iter().any(|i| i == other)) {
        return;
    }
    if let Ok(mut reg) = hideout_registry::Registry::open_from_env(home) {
        let _ = reg.remove(other);
    }
}

/// After an update the user's earlier Allow does not carry: the registry keeps the record's
/// grants over a re-register but binds them to the program's path and hash, so this runs the
/// same check the hub runs at every turn on and launch ([`void_if_changed`]) now. A changed
/// program sends the grants back to pending (the Allow is asked again); only a follow-folder
/// choice the user made themselves keeps one. A failure is logged and leaves the launch-time
/// check as the guard.
///
/// [`void_if_changed`]: hideout_registry::Registry::void_if_changed
fn void_grants_after_update(home: &Path, id: &str, now: &str) {
    match hideout_registry::Registry::open_from_env(home) {
        Ok(mut reg) => {
            if let Err(e) = reg.void_if_changed(id, now) {
                log::warn!("discover: {id}: grants not checked after the update: {e}");
            }
        }
        Err(e) => log::warn!("discover: {id}: grants not checked after the update: {e}"),
    }
}

/// Reads the registry's own copy of `id`'s manifest and requires it to equal `staged`. When it
/// does not (or can't be read) this is `false`, and when the record is `fresh` (this add made
/// it) it is removed: unregistered, with any grants; the app's own files are never touched. A
/// record that was there before the add is left alone.
pub(crate) fn registered_matches(home: &Path, id: &str, staged: &str, fresh: bool) -> bool {
    let Ok(mut reg) = hideout_registry::Registry::open_from_env(home) else {
        return false;
    };
    let same = reg
        .inventory
        .modules
        .get(id)
        .map(|e| reg.module_dir(id, &e.version).join("module.toml"))
        .and_then(|p| std::fs::read(p).ok())
        .is_some_and(|b| b == staged.as_bytes());
    if !same {
        log::warn!("discover: {id}: the registered file is not the staged one (fresh: {fresh})");
        if fresh {
            let _ = reg.remove(id);
        }
    }
    same
}

/// What the user is told after a click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    /// A refusal (shown as a problem); otherwise information.
    pub error: bool,
}

impl Note {
    pub fn error(text: impl Into<String>) -> Note {
        Note {
            text: text.into(),
            error: true,
        }
    }
    pub fn info(text: impl Into<String>) -> Note {
        Note {
            text: text.into(),
            error: false,
        }
    }
}

/// What the Found card and Add an app… show, and the steps behind their buttons.
#[derive(Default)]
pub struct AddApp {
    pub found: Vec<Candidate>,
    /// The Add an app… box is open.
    pub adding: bool,
    /// The last thing to tell the user (a refusal, "already in Hideout", "Added ...").
    pub note: Option<Note>,
    /// Apps the user chose Not now on, until the next start.
    pub dismissed: Vec<String>,
    scanned_for: Option<Vec<String>>,
}

impl AddApp {
    /// Looks again when what Hideout already holds changed since the last look (or `force`).
    pub fn scan(&mut self, m: &dyn Machine, home: &Path, installed: &[String], force: bool) {
        if !force && self.scanned_for.as_deref() == Some(installed) {
            return;
        }
        self.found = discover(m, home, installed);
        self.scanned_for = Some(installed.to_vec());
    }

    /// **Add an app…**: opens the box (and looks again, in case an app was installed since).
    pub fn open_box(&mut self, m: &dyn Machine, home: &Path, installed: &[String]) {
        self.adding = true;
        self.note = None;
        self.scan(m, home, installed, true);
    }

    pub fn cancel(&mut self) {
        self.adding = false;
        self.note = None;
    }

    /// **Not now** on the Found card: hides these apps until the next start.
    pub fn dismiss(&mut self) {
        self.dismissed = self.found.iter().map(|c| c.id.clone()).collect();
        self.note = None;
    }

    /// The apps the card lists (the ones not dismissed).
    pub fn shown(&self) -> Vec<&Candidate> {
        self.found
            .iter()
            .filter(|c| !self.dismissed.contains(&c.id))
            .collect()
    }

    /// **Browse…**: the user picks a folder; an addable app there is registered. `Some(id)` when
    /// it was added (the page then opens its Allow dialog).
    pub fn browse(
        &mut self,
        m: &dyn Machine,
        picker: &dyn Picker,
        home: &Path,
        staging: &Path,
        installed: &[String],
        now: &str,
    ) -> Option<String> {
        let folder = picker.pick_folder()?;
        match from_folder(m, home, &folder) {
            Ok(c) if installed.contains(&c.id) => {
                self.note = Some(Note::info(words::already(&c.name)));
                None
            }
            Ok(c) => self.register(m, home, staging, &c, now),
            Err(why) => {
                self.note = Some(Note::error(why));
                None
            }
        }
    }

    /// **Add** on a found app (by its id).
    pub fn add_found(
        &mut self,
        m: &dyn Machine,
        id: &str,
        home: &Path,
        staging: &Path,
        now: &str,
    ) -> Option<String> {
        let c = self.found.iter().find(|c| c.id == id)?.clone();
        self.register(m, home, staging, &c, now)
    }

    fn register(
        &mut self,
        m: &dyn Machine,
        home: &Path,
        staging: &Path,
        c: &Candidate,
        now: &str,
    ) -> Option<String> {
        match add(m, home, staging, c, now) {
            Ok(id) => {
                self.adding = false;
                self.found.retain(|f| f.id != id);
                self.note = Some(Note::info(words::added(&c.name)));
                Some(id)
            }
            Err(why) => {
                self.note = Some(Note::error(why));
                None
            }
        }
    }
}

#[cfg(windows)]
pub use ui::DiscoverUi;

#[cfg(windows)]
mod ui {
    use super::*;
    use hideout_shell::egui::{self, RichText};
    use hideout_shell::theme::Tokens;

    /// The Windows folder picker, modal to the hub window.
    pub struct ShellPicker;

    impl Picker for ShellPicker {
        fn pick_folder(&self) -> Option<PathBuf> {
            hideout_shell::pick_folder()
        }
    }

    /// The page's discovery part: the machine, the picker, the hub folder and the card state.
    pub struct DiscoverUi {
        pub machine: Box<dyn Machine>,
        pub picker: Box<dyn Picker>,
        pub home: PathBuf,
        pub state: AddApp,
        /// The Done card lists the found apps itself, so the Found card waits.
        pub quiet: bool,
    }

    enum Click {
        Add(String),
        OpenBox,
        Browse,
        Cancel,
        NotNow,
    }

    fn c(x: hideout_shell::theme::Rgb) -> egui::Color32 {
        egui::Color32::from_rgb(x.0, x.1, x.2)
    }

    impl DiscoverUi {
        pub fn new(home: PathBuf) -> DiscoverUi {
            DiscoverUi {
                machine: find_installed::machine_for(&home, Box::new(find_installed::RealMachine)),
                picker: Box::new(ShellPicker),
                home,
                state: AddApp::default(),
                quiet: false,
            }
        }

        /// **Set up** on a found app from the Done card: the same step as the Found card's.
        pub fn set_up(&mut self, id: &str, _installed: &[String]) -> Option<String> {
            self.act(Click::Add(id.to_string()), _installed)
        }

        fn staging(&self) -> PathBuf {
            std::env::temp_dir().join("hideout-adopt")
        }

        /// Carries out a click; `Some(id)` when an app was added.
        fn act(&mut self, click: Click, installed: &[String]) -> Option<String> {
            let now = hideout_registry::catalogs::now_utc();
            let staging = self.staging();
            match click {
                Click::Add(id) => {
                    self.state
                        .add_found(self.machine.as_ref(), &id, &self.home, &staging, &now)
                }
                Click::OpenBox => {
                    self.state
                        .open_box(self.machine.as_ref(), &self.home, installed);
                    None
                }
                Click::Browse => self.state.browse(
                    self.machine.as_ref(),
                    self.picker.as_ref(),
                    &self.home,
                    &staging,
                    installed,
                    &now,
                ),
                Click::Cancel => {
                    self.state.cancel();
                    None
                }
                Click::NotNow => {
                    self.state.dismiss();
                    None
                }
            }
        }

        /// Draws the Found card and the Add an app… line above the rows. `Some(id)` when an app
        /// was added this frame: the page then opens its Allow dialog.
        pub fn ui(
            &mut self,
            ui: &mut egui::Ui,
            t: &Tokens,
            installed: &[String],
        ) -> Option<String> {
            self.state
                .scan(self.machine.as_ref(), &self.home, installed, false);
            let mut click = None;
            let shown: Vec<Candidate> = self.state.shown().into_iter().cloned().collect();
            if self.state.adding
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                click = Some(Click::Cancel);
            }
            if !shown.is_empty() && !self.quiet {
                frame(ui, t, |ui| {
                    let head =
                        ui.label(RichText::new(words::FOUND_TITLE).strong().color(c(t.text)));
                    ui.ctx().accesskit_node_builder(head.id, |n| {
                        n.set_live(egui::accesskit::Live::Polite);
                    });
                    ui.label(RichText::new(words::FOUND_BODY).color(c(t.text_muted)));
                    ui.add_space(6.0);
                    for f in &shown {
                        ui.push_id(("hub.found", &f.id), |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(
                                        RichText::new(words::found_line(&f.name, &f.version))
                                            .color(c(t.text)),
                                    );
                                    ui.label(
                                        RichText::new(f.folder()).small().color(c(t.text_muted)),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let b = ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new(f.button()).color(c(t.on_accent)),
                                                )
                                                .fill(c(t.accent)),
                                            )
                                            .on_hover_text(words::add_hover(&f.name, &f.folder()));
                                        let label = if f.update_of.is_some() {
                                            f.button()
                                        } else {
                                            words::add_label(&f.name)
                                        };
                                        ui.ctx().accesskit_node_builder(b.id, |n| {
                                            n.set_label(label);
                                        });
                                        if b.clicked() {
                                            click = Some(Click::Add(f.id.clone()));
                                        }
                                    },
                                );
                            });
                        });
                    }
                    if !self.state.adding {
                        if let Some(note) = &self.state.note {
                            note_label(ui, t, note);
                        }
                    }
                    if ui.button(words::NOT_NOW).clicked() {
                        click = Some(Click::NotNow);
                    }
                });
                ui.add_space(8.0);
            } else if !self.state.adding {
                if let Some(note) = &self.state.note {
                    note_label(ui, t, note);
                    ui.add_space(6.0);
                }
            }
            if self.state.adding {
                frame(ui, t, |ui| {
                    ui.label(RichText::new(words::ADD_TITLE).strong().color(c(t.text)));
                    ui.label(RichText::new(words::ADD_BODY).color(c(t.text_muted)));
                    if let Some(note) = &self.state.note {
                        note_label(ui, t, note);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(words::BROWSE).color(c(t.on_accent)),
                                )
                                .fill(c(t.accent)),
                            )
                            .clicked()
                        {
                            click = Some(Click::Browse);
                        }
                        if ui.button(words::CANCEL).clicked() {
                            click = Some(Click::Cancel);
                        }
                    });
                });
                ui.add_space(8.0);
            } else if ui.button(words::ADD_AN_APP).clicked() {
                click = Some(Click::OpenBox);
            }
            ui.add_space(8.0);
            click.and_then(|k| self.act(k, installed))
        }
    }

    /// A note: a refusal in the problem colour, information in the plain one. Announced politely.
    fn note_label(ui: &mut egui::Ui, t: &Tokens, note: &Note) {
        let colour = if note.error { c(t.danger) } else { c(t.text) };
        let n = ui.label(RichText::new(&note.text).color(colour));
        ui.ctx().accesskit_node_builder(n.id, |x| {
            x.set_live(egui::accesskit::Live::Polite);
        });
    }

    fn frame(ui: &mut egui::Ui, t: &Tokens, body: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(c(t.surface))
            .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                body(ui);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        reg: BTreeMap<(String, String), String>,
        subkeys: Vec<String>,
        env: BTreeMap<String, String>,
        /// path -> (file version, text)
        files: BTreeMap<PathBuf, (String, Option<String>)>,
        /// Folders (and everything below) that are not on a local fixed drive.
        remote: Vec<PathBuf>,
        /// path -> the path it resolves to
        links: BTreeMap<PathBuf, PathBuf>,
        /// The folder of the running hub's exe.
        exe_dir: Option<PathBuf>,
        /// folder -> its sub-folders, as listed (a link's own path is listed, not its target)
        subs: BTreeMap<PathBuf, Vec<PathBuf>>,
    }

    fn norm(p: &Path) -> String {
        p.to_string_lossy().replace('/', "\\").to_lowercase()
    }

    impl Machine for Fake {
        fn registry_string(&self, key: &str, value: &str) -> Option<String> {
            self.reg.get(&(key.into(), value.into())).cloned()
        }
        fn env(&self, n: &str) -> Option<String> {
            self.env.get(n).cloned()
        }
        fn is_file(&self, p: &Path) -> bool {
            self.files.keys().any(|k| norm(k) == norm(p))
        }
        fn exe_version(&self, p: &Path) -> Option<String> {
            self.files.get(p).map(|f| f.0.clone())
        }
        fn read_text(&self, p: &Path) -> Option<String> {
            self.files.get(p).and_then(|f| f.1.clone())
        }
        fn registry_subkeys(&self, key: &str) -> Vec<String> {
            if key == MODULES_KEY {
                self.subkeys.clone()
            } else {
                Vec::new()
            }
        }
        fn canonical(&self, p: &Path) -> Option<PathBuf> {
            Some(
                self.links
                    .get(p)
                    .cloned()
                    .unwrap_or_else(|| p.to_path_buf()),
            )
        }
        fn is_local_fixed(&self, p: &Path) -> bool {
            !self.remote.iter().any(|r| p.starts_with(r))
        }
        fn hub_exe_dir(&self) -> Option<PathBuf> {
            self.exe_dir.clone()
        }
        fn sub_folders(&self, dir: &Path) -> Vec<PathBuf> {
            self.subs.get(dir).cloned().unwrap_or_default()
        }
    }

    struct Pick(Option<PathBuf>);
    impl Picker for Pick {
        fn pick_folder(&self) -> Option<PathBuf> {
            self.0.clone()
        }
    }

    fn manifest(id: &str, name: &str, version: &str, kind: &str, exe: &str) -> String {
        format!(
            "id = \"{id}\"\nname = \"{name}\"\nversion = \"{version}\"\nkind = \"{kind}\"\n\
             family = \"vulpes\"\nhub = \"1.1\"\nrun = ['{exe}', 'hub']\n\
             api = {{ vulpes = \"1.0\" }}\n[activation]\non = [\"open\", \"call\"]\n"
        )
    }

    /// A hub folder for tests that never touch it.
    fn hh() -> PathBuf {
        PathBuf::from(r"C:\HubHome")
    }

    fn dir(name: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\Apps\{name}"))
    }

    /// Registers `id` under the Modules key, pointing at a folder with its manifest.
    fn register(f: &mut Fake, id: &str, name: &str, version: &str, as_file: bool) {
        let folder = dir(id);
        let toml = folder.join("module.toml");
        f.files.insert(
            toml.clone(),
            (
                String::new(),
                Some(manifest(id, name, version, "adopted", "C:/Apps/x.exe")),
            ),
        );
        f.files
            .insert(PathBuf::from("C:/Apps/x.exe"), ("1.0".into(), None));
        f.subkeys.push(id.to_string());
        let at = if as_file { toml } else { folder };
        f.reg.insert(
            (format!(r"{MODULES_KEY}\{id}"), String::new()),
            at.display().to_string(),
        );
    }

    #[test]
    fn self_registered_apps_are_found_by_id_from_a_folder_or_a_manifest_path() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "2.0.1", false);
        register(&mut f, "widget", "Widget", "0.3.0", true);
        let found = discover(&f, &hh(), &[]);
        let names: Vec<_> = found
            .iter()
            .map(|c| (c.id.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(names, [("gizmo", "Gizmo"), ("widget", "Widget")]);
        assert_eq!(found[1].version, "0.3.0");
        assert!(matches!(&found[0].origin, Origin::Folder(p) if *p == dir("gizmo")));
    }

    #[test]
    fn the_value_can_be_named_path_and_a_key_for_another_id_is_not_that_app() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        let k = format!(r"{MODULES_KEY}\gizmo");
        let v = f.reg.remove(&(k.clone(), String::new())).unwrap();
        f.reg.insert((k, "Path".into()), v);
        // A key "other" whose manifest says "gizmo".
        f.subkeys.push("other".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\other"), String::new()),
            dir("gizmo").display().to_string(),
        );
        let found = discover(&f, &hh(), &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "gizmo");
    }

    #[test]
    fn a_bad_registration_is_skipped_and_the_rest_still_show() {
        let mut f = Fake::default();
        register(&mut f, "good", "Good", "1.0.0", false);
        // Points nowhere, points at a share, points at a non-adopted manifest, unreadable toml.
        for (id, at) in [("gone", r"C:\Nowhere"), ("share", r"\\srv\share\app")] {
            f.subkeys.push(id.into());
            f.reg
                .insert((format!(r"{MODULES_KEY}\{id}"), String::new()), at.into());
        }
        f.subkeys.push("svc".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\svc"), String::new()),
            dir("svc").display().to_string(),
        );
        f.files.insert(
            dir("svc").join("module.toml"),
            (
                String::new(),
                Some(manifest("svc", "Svc", "1.0.0", "service", "C:/x.exe")),
            ),
        );
        f.subkeys.push("junk".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\junk"), String::new()),
            dir("junk").display().to_string(),
        );
        f.files.insert(
            dir("junk").join("module.toml"),
            (String::new(), Some("not toml {".into())),
        );
        let found = discover(&f, &hh(), &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "good");
    }

    #[test]
    fn the_fallback_list_still_finds_the_two_known_apps_and_a_registration_wins() {
        let mut f = Fake::default();
        // Foxglove from its known folder; Vulpes self-registers (and has an installer record).
        f.env
            .insert("LOCALAPPDATA".into(), r"C:\Users\t\AppData\Local".into());
        f.files.insert(
            PathBuf::from(r"C:\Users\t\AppData\Local\Programs\Foxglove\foxglove.exe"),
            ("0.3.0".into(), None),
        );
        register(&mut f, "vulpes", "Vulpes", "0.1.0", false);
        f.reg.insert(
            (r"Software\Vulpes\Installer".into(), "App".into()),
            r"C:\Apps\vulpes".into(),
        );
        f.files.insert(
            PathBuf::from(r"C:\Apps\vulpes\venv\Scripts\arete.exe"),
            ("0.1.0".into(), None),
        );
        let found = discover(&f, &hh(), &[]);
        let ids: Vec<_> = found.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["foxglove", "vulpes"]);
        let vulpes = found.iter().find(|c| c.id == "vulpes").unwrap();
        assert!(
            matches!(vulpes.origin, Origin::Folder(_)),
            "the registration wins"
        );
        let fox = found.iter().find(|c| c.id == "foxglove").unwrap();
        assert!(matches!(fox.origin, Origin::Product(_)));
    }

    #[test]
    fn apps_hideout_already_has_are_not_offered() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        register(&mut f, "widget", "Widget", "1.0.0", false);
        let found = discover(&f, &hh(), &["gizmo".to_string()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "widget");
    }

    fn temp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("hideout-discover-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A real folder with a real exe and manifest (the add path writes to a temp hub folder),
    /// with a fake `Machine` that reads that manifest.
    fn app_on_disk(name: &str, id: &str) -> (PathBuf, Fake) {
        let install = temp(name);
        let exe = install.join("tool.exe");
        std::fs::write(&exe, b"x").unwrap();
        let text = manifest(
            id,
            "Gizmo",
            "1.4.0",
            "adopted",
            &exe.display().to_string().replace('\\', "/"),
        );
        std::fs::write(install.join("module.toml"), &text).unwrap();
        let mut f = Fake::default();
        f.files
            .insert(install.join("module.toml"), (String::new(), Some(text)));
        f.files.insert(exe, ("1.0".into(), None));
        (install, f)
    }

    const NOW: &str = "2026-11-19T08:00:00Z";

    #[test]
    fn browse_adds_the_app_in_the_folder_and_writes_no_grant() {
        let (install, f) = app_on_disk("browse", "gizmo");
        let home = temp("browse-home");
        let mut s = AddApp::default();
        s.open_box(&f, &hh(), &[]);
        assert!(s.adding);
        let id = s.browse(&f, &Pick(Some(install)), &home, &temp("stg"), &[], NOW);
        assert_eq!(id.as_deref(), Some("gizmo"));
        assert!(!s.adding);
        assert_eq!(
            s.note,
            Some(Note::info("Added Gizmo. Choose what it may do."))
        );
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.contains_key("gizmo"));
        // Registered, never allowed: the first-run Allow dialog is the user's.
        assert!(!reg.hub.enabled("gizmo"));
    }

    #[test]
    fn browse_says_why_in_plain_words_and_cancel_adds_nothing() {
        let (install, f) = app_on_disk("browse2", "gizmo");
        let home = temp("browse2-home");
        let stg = temp("stg2");
        let mut s = AddApp::default();
        // Cancelled picker: nothing, no note.
        assert!(s.browse(&f, &Pick(None), &home, &stg, &[], NOW).is_none());
        assert!(s.note.is_none());
        // An empty folder.
        let _ = s.browse(
            &f,
            &Pick(Some(PathBuf::from(r"C:\Empty"))),
            &home,
            &stg,
            &[],
            NOW,
        );
        assert_eq!(s.note, Some(Note::error(words::NO_FILE)));
        // A share.
        let _ = s.browse(
            &f,
            &Pick(Some(PathBuf::from(r"\\srv\x"))),
            &home,
            &stg,
            &[],
            NOW,
        );
        assert_eq!(s.note, Some(Note::error(words::NOT_THIS_PC)));
        // Already added.
        let _ = s.browse(
            &f,
            &Pick(Some(install)),
            &home,
            &stg,
            &["gizmo".into()],
            NOW,
        );
        assert_eq!(s.note, Some(Note::info("Gizmo is already in Hideout.")));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
    }

    #[test]
    fn adding_a_found_app_registers_it_and_leaves_the_card_and_a_refusal_keeps_it() {
        let (install, mut f) = app_on_disk("found", "gizmo");
        f.subkeys.push("gizmo".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            install.display().to_string(),
        );
        let home = temp("found-home");
        let stg = temp("stg3");
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        assert_eq!(s.found.len(), 1);
        assert_eq!(
            s.add_found(&f, "gizmo", &home, &stg, NOW).as_deref(),
            Some("gizmo")
        );
        assert!(s.found.is_empty());
        // The program is gone: the card is not offered at all.
        let (gone, mut g) = app_on_disk("found2", "gizmo");
        g.subkeys.push("gizmo".into());
        g.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            gone.display().to_string(),
        );
        g.files.remove(&gone.join("tool.exe"));
        let mut s2 = AddApp::default();
        s2.scan(&g, &hh(), &[], false);
        assert!(s2.found.is_empty());
    }

    #[test]
    fn a_program_that_vanishes_after_the_look_is_refused_in_plain_words_and_the_card_stays() {
        let (install, mut f) = app_on_disk("vanish", "gizmo");
        f.subkeys.push("gizmo".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            install.display().to_string(),
        );
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        assert_eq!(s.found.len(), 1);
        f.files.remove(&install.join("tool.exe"));
        std::fs::remove_file(install.join("tool.exe")).unwrap();
        assert!(s
            .add_found(&f, "gizmo", &temp("vanish-home"), &temp("stg5"), NOW)
            .is_none());
        let n = s.note.clone().unwrap();
        assert!(n.error);
        assert_eq!(n.text, words::program_missing("Gizmo"));
        assert!(!n.text.contains("os error") && !n.text.contains('\\'));
        assert_eq!(s.found.len(), 1);
        // The card shows the note even with the Add an app box closed (the UI draws it there).
        assert!(!s.adding);
    }

    #[test]
    fn a_network_or_mapped_drive_is_not_this_pc_and_a_junction_is_followed() {
        let (install, mut f) = app_on_disk("net", "gizmo");
        f.remote.push(install.clone());
        assert_eq!(
            from_folder(&f, &hh(), &install).unwrap_err(),
            words::NOT_THIS_PC.to_string()
        );
        // A local-looking path that resolves onto a remote drive is refused too.
        let mut g = Fake::default();
        g.remote.push(PathBuf::from(r"Z:\share"));
        g.links.insert(
            PathBuf::from(r"C:\Apps\link"),
            PathBuf::from(r"Z:\share\app"),
        );
        assert_eq!(
            from_folder(&g, &hh(), Path::new(r"C:\Apps\link")).unwrap_err(),
            words::NOT_THIS_PC.to_string()
        );
        // And it never reaches discovery.
        let mut h = Fake::default();
        register(&mut h, "gizmo", "Gizmo", "1.0.0", false);
        h.remote.push(dir("gizmo"));
        assert!(discover(&h, &hh(), &[]).is_empty());
    }

    #[test]
    fn a_manifest_that_changed_after_the_look_is_refused_and_nothing_is_registered() {
        let (install, mut f) = app_on_disk("swap", "gizmo");
        f.subkeys.push("gizmo".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            install.display().to_string(),
        );
        let home = temp("swap-home");
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        // Another app's manifest lands in the folder (the fake and the disk agree).
        let exe = install
            .join("tool.exe")
            .display()
            .to_string()
            .replace('\\', "/");
        let text = manifest("intruder", "Gizmo", "1.4.0", "adopted", &exe);
        std::fs::write(install.join("module.toml"), &text).unwrap();
        f.files
            .insert(install.join("module.toml"), (String::new(), Some(text)));
        assert!(s
            .add_found(&f, "gizmo", &home, &temp("stg6"), NOW)
            .is_none());
        assert_eq!(s.note, Some(Note::error(words::changed("Gizmo"))));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
    }

    #[test]
    fn any_change_to_the_manifest_after_the_look_is_refused_not_only_the_id_and_version() {
        let (install, mut f) = app_on_disk("swap2", "gizmo");
        f.subkeys.push("gizmo".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            install.display().to_string(),
        );
        let home = temp("swap2-home");
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        // Same id and version, another program.
        let other = install
            .join("other.exe")
            .display()
            .to_string()
            .replace('\\', "/");
        let text = manifest("gizmo", "Gizmo", "1.4.0", "adopted", &other);
        f.files
            .insert(install.join("module.toml"), (String::new(), Some(text)));
        assert!(s
            .add_found(&f, "gizmo", &home, &temp("stg7"), NOW)
            .is_none());
        assert_eq!(s.note, Some(Note::error(words::changed("Gizmo"))));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
    }

    #[test]
    fn what_is_registered_is_the_text_that_was_shown() {
        let (install, mut f) = app_on_disk("same", "gizmo");
        f.subkeys.push("gizmo".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\gizmo"), String::new()),
            install.display().to_string(),
        );
        let home = temp("same-home");
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        let shown = s.found[0].text.clone().unwrap();
        // The disk file changes after the card; the fake still shows the old text (a swap
        // that lands after the re-read). The registered record is still the shown text.
        std::fs::write(install.join("module.toml"), "id = \"gizmo\"\n").unwrap();
        assert!(s
            .add_found(&f, "gizmo", &home, &temp("stg8"), NOW)
            .is_some());
        let rec = hideout_registry::Registry::open_from_env(&home).unwrap();
        let dir = rec.module_dir("gizmo", "1.4.0");
        assert_eq!(
            std::fs::read_to_string(dir.join("module.toml")).unwrap(),
            shown
        );
    }

    #[test]
    fn a_program_on_a_remote_drive_is_not_this_pc() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        f.remote.push(PathBuf::from("C:/Apps/x.exe"));
        assert_eq!(
            from_folder(&f, &hh(), &dir("gizmo")).unwrap_err(),
            words::NOT_THIS_PC.to_string()
        );
    }

    #[test]
    fn names_and_versions_are_cleaned_and_the_card_gives_the_folder() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        f.files.insert(
            dir("gizmo").join("module.toml"),
            (
                String::new(),
                Some(manifest(
                    "gizmo",
                    "Gi\u{202e}zmo",
                    "1.0.0",
                    "adopted",
                    "C:/Apps/x.exe",
                )),
            ),
        );
        let found = discover(&f, &hh(), &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Gizmo");
        assert_eq!(found[0].folder(), dir("gizmo").display().to_string());
    }

    #[test]
    fn not_now_hides_the_card_for_the_session() {
        let mut f = Fake::default();
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        assert_eq!(s.shown().len(), 1);
        s.dismiss();
        assert!(s.shown().is_empty());
        // Still there for Add an app.
        assert_eq!(s.found.len(), 1);
    }

    #[test]
    fn scan_only_looks_again_when_the_installed_list_changes() {
        let mut f = Fake::default();
        let mut s = AddApp::default();
        s.scan(&f, &hh(), &[], false);
        assert!(s.found.is_empty());
        register(&mut f, "gizmo", "Gizmo", "1.0.0", false);
        s.scan(&f, &hh(), &[], false);
        assert!(s.found.is_empty(), "same list: no new registry read");
        s.scan(&f, &hh(), &["other".into()], false);
        assert_eq!(s.found.len(), 1);
    }

    // ---- service app files (Add an app accepts them and registers them as adopted) ----

    /// A service app file as an app ships it: relative `run[0]`, two permissions, a comment.
    fn service_text(id: &str, run0: &str) -> String {
        format!(
            "# A service app.\nid = \"{id}\"\nname = \"Gadget\"\nversion = \"0.1.0\"\n\
             kind = \"service\" # how it runs\nfamily = \"vulpes\"\nhub = \"1.5\"\n\
             run = [\"{run0}\", \"--quiet\"]\n\
             permissions = [\"facts.write:gadget.*\", \"facts.read:keyboard.game_mode\"]\n\
             provides = [\"facts:gadget.*\"]\n\n[[ui.panels]]\nid = \"page\"\nslot = \"page\"\n\
             title = \"Gadget\"\nicon = \"chart\"\n"
        )
    }

    /// A real folder with `svc.exe` and a service app file, with a fake `Machine` that reads it.
    fn service_on_disk(name: &str, id: &str, run0: &str) -> (PathBuf, Fake, String) {
        let install = temp(name);
        std::fs::write(install.join("svc.exe"), b"x").unwrap();
        let text = service_text(id, run0);
        std::fs::write(install.join("module.toml"), &text).unwrap();
        let mut f = Fake::default();
        f.files.insert(
            install.join("module.toml"),
            (String::new(), Some(text.clone())),
        );
        f.files
            .insert(install.join("svc.exe"), ("0.1.0".into(), None));
        (install, f, text)
    }

    #[test]
    fn a_service_app_file_is_added_as_adopted_with_its_program_resolved_next_to_it() {
        let (install, f, _) = service_on_disk("svc-add", "vulpes.gadget", "svc.exe");
        let home = temp("svc-add-home");
        let mut s = AddApp::default();
        let id = s.browse(
            &f,
            &Pick(Some(install.clone())),
            &home,
            &temp("svc-stg"),
            &[],
            NOW,
        );
        assert_eq!(id.as_deref(), Some("vulpes.gadget"));
        assert_eq!(
            s.note,
            Some(Note::info("Added Gadget. Choose what it may do."))
        );
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        let rec = &reg.inventory.modules["vulpes.gadget"];
        assert_eq!(rec.source, "adopted");
        let m = hideout_contract::module::Module::parse(
            &std::fs::read_to_string(reg.module_dir("vulpes.gadget", "0.1.0").join("module.toml"))
                .unwrap(),
            &Context::default(),
        )
        .unwrap();
        assert_eq!(m.kind, Kind::Adopted);
        let run = m.run.unwrap();
        assert_eq!(Path::new(&run[0]), install.join("svc.exe"));
        assert!(Path::new(&run[0]).is_absolute());
        assert_eq!(run[1], "--quiet");
        // Nothing granted: every permission is pending, nothing is allowed or on.
        let view = reg.permission_view("vulpes.gadget").unwrap();
        assert!(view.granted.is_empty());
        assert!(view.adopted);
        let mut pending = view.pending.clone();
        pending.sort();
        assert_eq!(
            pending,
            ["facts.read:keyboard.game_mode", "facts.write:gadget.*"]
        );
        assert!(!reg.hub.enabled("vulpes.gadget"));
    }

    #[test]
    fn the_staged_text_is_the_original_except_kind_and_run_0() {
        let (install, f, original) = service_on_disk("svc-text", "vulpes.gadget", "svc.exe");
        let home = temp("svc-text-home");
        let mut s = AddApp::default();
        assert!(s
            .browse(
                &f,
                &Pick(Some(install.clone())),
                &home,
                &temp("svc-stg2"),
                &[],
                NOW
            )
            .is_some());
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        let staged =
            std::fs::read_to_string(reg.module_dir("vulpes.gadget", "0.1.0").join("module.toml"))
                .unwrap();
        let (a, b): (Vec<_>, Vec<_>) = (original.lines().collect(), staged.lines().collect());
        assert_eq!(a.len(), b.len());
        let mut changed = Vec::new();
        for (x, y) in a.iter().zip(&b) {
            if x != y {
                changed.push((*x, *y));
            }
        }
        assert_eq!(changed.len(), 2, "{changed:?}");
        // The comment on the kind line stays.
        assert_eq!(changed[0].0, "kind = \"service\" # how it runs");
        assert_eq!(changed[0].1, "kind = \"adopted\" # how it runs");
        assert!(changed[1].0.starts_with("run = [\"svc.exe\""));
        assert!(changed[1].1.starts_with("run = ["));
        assert!(changed[1].1.ends_with(", \"--quiet\"]"));
        // Same keys and values otherwise (id, permissions, panels).
        let before: toml_edit::DocumentMut = original.parse().unwrap();
        let after: toml_edit::DocumentMut = staged.parse().unwrap();
        for k in [
            "id",
            "name",
            "version",
            "family",
            "hub",
            "permissions",
            "provides",
            "ui",
        ] {
            assert_eq!(before[k].to_string(), after[k].to_string(), "{k}");
        }
    }

    #[test]
    fn a_run_that_leaves_the_folder_is_refused() {
        let home = temp("svc-esc-home");
        for run0 in [
            "../svc.exe",
            "sub/../../svc.exe",
            "C:/Windows/x.exe",
            "/x.exe",
            "\\\\srv\\s\\x.exe",
        ] {
            let (install, f, _) = service_on_disk("svc-esc", "vulpes.gadget", run0);
            let mut s = AddApp::default();
            assert!(s
                .browse(&f, &Pick(Some(install)), &home, &temp("svc-stg3"), &[], NOW)
                .is_none());
            let n = s.note.clone().unwrap();
            assert!(n.error, "{run0}");
            assert!(
                n.text == words::NOT_READ || n.text == words::NOT_ADDABLE,
                "{run0}: {}",
                n.text
            );
        }
        // A link inside the folder that leads out is refused after it is resolved.
        let (install, mut f, _) = service_on_disk("svc-link", "vulpes.gadget", "svc.exe");
        f.links.insert(
            install.join("svc.exe"),
            PathBuf::from(r"C:\Elsewhere\other.exe"),
        );
        assert_eq!(
            from_folder(&f, &home, &install).unwrap_err(),
            words::NOT_ADDABLE
        );
        // ...and onto another drive it is not this PC's.
        f.remote.push(PathBuf::from(r"C:\Elsewhere"));
        assert_eq!(
            from_folder(&f, &home, &install).unwrap_err(),
            words::NOT_THIS_PC
        );
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
    }

    #[test]
    fn other_kinds_are_still_not_addable() {
        let home = temp("kinds-home");
        for kind in ["content", "runtime", "firmware"] {
            let install = temp(&format!("kind-{kind}"));
            let text = format!(
                "id = \"vulpes.thing\"\nname = \"Thing\"\nversion = \"1.0.0\"\nkind = \"{kind}\"\n\
                 family = \"vulpes\"\nhub = \"1.1\"\n"
            );
            let mut f = Fake::default();
            f.files
                .insert(install.join("module.toml"), (String::new(), Some(text)));
            let got = from_folder(&f, &home, &install).unwrap_err();
            assert!(
                got == words::NOT_ADDABLE || got == words::NOT_READ,
                "{kind}: {got}"
            );
        }
        // A content file the parser accepts is refused by word, not by accident.
        let install = temp("kind-content-ok");
        let text =
            "id = \"vulpes.pack\"\nname = \"Pack\"\nversion = \"1.0.0\"\nkind = \"content\"\n\
                    family = \"vulpes\"\nhub = \"1.1\"\nprovides = [\"content:art\"]\n";
        let mut f = Fake::default();
        f.files.insert(
            install.join("module.toml"),
            (String::new(), Some(text.into())),
        );
        match from_folder(&f, &home, &install) {
            Err(w) => assert!(w == words::NOT_ADDABLE || w == words::NOT_READ, "{w}"),
            Ok(_) => panic!("a content app must not be addable"),
        }
    }

    #[test]
    fn a_folder_inside_the_hubs_own_modules_folder_is_refused() {
        let home = temp("svc-hub-home");
        let inside_hub = home.join("modules").join("vulpes.gadget").join("0.1.0");
        std::fs::create_dir_all(&inside_hub).unwrap();
        let text = service_text("vulpes.gadget", "svc.exe");
        let mut f = Fake::default();
        f.files
            .insert(inside_hub.join("module.toml"), (String::new(), Some(text)));
        f.files
            .insert(inside_hub.join("svc.exe"), ("0.1.0".into(), None));
        assert_eq!(
            from_folder(&f, &home, &inside_hub).unwrap_err(),
            words::INSIDE_HUB
        );
        // The modules folder itself, and a different case of the path, too.
        f.files.insert(
            home.join("modules").join("module.toml"),
            (
                String::new(),
                Some(service_text("vulpes.gadget", "svc.exe")),
            ),
        );
        assert_eq!(
            from_folder(&f, &home, &home.join("modules")).unwrap_err(),
            words::INSIDE_HUB
        );
        // The hub folder itself, naming a program the hub installed, and a folder above it.
        f.files.insert(
            home.join("module.toml"),
            (
                String::new(),
                Some(service_text(
                    "vulpes.gadget",
                    "modules/vulpes.gadget/0.1.0/svc.exe",
                )),
            ),
        );
        assert_eq!(
            from_folder(&f, &home, &home).unwrap_err(),
            words::INSIDE_HUB
        );
        let above = home.parent().unwrap().to_path_buf();
        let down = format!(
            "{}/modules/vulpes.gadget/0.1.0/svc.exe",
            home.file_name().unwrap().to_string_lossy()
        );
        f.files.insert(
            above.join("module.toml"),
            (String::new(), Some(service_text("vulpes.gadget", &down))),
        );
        assert_eq!(
            from_folder(&f, &home, &above).unwrap_err(),
            words::INSIDE_HUB
        );
        // A folder beside it is fine.
        let (install, g, _) = service_on_disk("svc-hub-ok", "vulpes.gadget", "svc.exe");
        assert!(from_folder(&g, &home, &install).is_ok());
        let mut s = AddApp::default();
        assert!(s
            .browse(
                &f,
                &Pick(Some(inside_hub)),
                &home,
                &temp("svc-stg4"),
                &[],
                NOW
            )
            .is_none());
        assert_eq!(s.note, Some(Note::error(words::INSIDE_HUB)));
        assert_eq!(
            words::INSIDE_HUB,
            "Hideout keeps that folder for its own files. Choose the folder the app was installed in."
        );
    }

    #[test]
    fn a_self_registered_service_folder_is_found_and_added() {
        let (install, mut f, _) = service_on_disk("svc-reg", "vulpes.gadget", "svc.exe");
        f.subkeys.push("vulpes.gadget".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\vulpes.gadget"), String::new()),
            install.display().to_string(),
        );
        let home = temp("svc-reg-home");
        let mut s = AddApp::default();
        s.scan(&f, &home, &[], false);
        assert_eq!(s.found.len(), 1);
        assert_eq!(s.found[0].id, "vulpes.gadget");
        assert_eq!(
            s.found[0].program.as_deref(),
            Some(install.join("svc.exe").as_path())
        );
        assert_eq!(
            s.add_found(&f, "vulpes.gadget", &home, &temp("svc-stg5"), NOW)
                .as_deref(),
            Some("vulpes.gadget")
        );
        // The program is gone: the card is not offered.
        f.files.remove(&install.join("svc.exe"));
        let mut t = AddApp::default();
        t.scan(&f, &home, &[], false);
        assert!(t.found.is_empty());
    }

    #[test]
    fn a_service_file_that_changed_after_the_look_is_refused() {
        let (install, mut f, _) = service_on_disk("svc-swap", "vulpes.gadget", "svc.exe");
        f.subkeys.push("vulpes.gadget".into());
        f.reg.insert(
            (format!(r"{MODULES_KEY}\vulpes.gadget"), String::new()),
            install.display().to_string(),
        );
        let home = temp("svc-swap-home");
        let mut s = AddApp::default();
        s.scan(&f, &home, &[], false);
        // The program now resolves somewhere else inside the folder.
        std::fs::write(install.join("other.exe"), b"y").unwrap();
        f.files
            .insert(install.join("other.exe"), ("0.1.0".into(), None));
        f.links
            .insert(install.join("svc.exe"), install.join("other.exe"));
        assert!(s
            .add_found(&f, "vulpes.gadget", &home, &temp("svc-stg6"), NOW)
            .is_none());
        assert_eq!(s.note, Some(Note::error(words::changed("Gadget"))));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
    }

    // ---- apps next to Hideout ----

    const BESIDE: &str = r"C:\Prog\Hideout";

    /// A service app `name` under `<BESIDE>\apps`, with its program, listed as a sub-folder.
    fn beside(f: &mut Fake, name: &str, id: &str) -> PathBuf {
        let folder = Path::new(BESIDE).join("apps").join(name);
        let text = service_text(id, "svc.exe");
        f.files
            .insert(folder.join("module.toml"), (String::new(), Some(text)));
        f.files
            .insert(folder.join("svc.exe"), ("0.1.0".into(), None));
        f.exe_dir = Some(PathBuf::from(BESIDE));
        f.subs
            .entry(Path::new(BESIDE).join("apps"))
            .or_default()
            .push(folder.clone());
        folder
    }

    #[test]
    fn a_service_app_next_to_hideout_is_found_and_offered() {
        let mut f = Fake::default();
        let folder = beside(&mut f, "gadget", "vulpes.gadget");
        let found = discover(&f, &hh(), &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "vulpes.gadget");
        assert_eq!(
            found[0].program.as_deref(),
            Some(folder.join("svc.exe").as_path())
        );
        assert!(matches!(&found[0].origin, Origin::Folder(x) if *x == folder));
    }

    #[test]
    fn no_exe_folder_or_no_apps_folder_finds_nothing() {
        let mut f = Fake::default();
        beside(&mut f, "gadget", "vulpes.gadget");
        f.exe_dir = None;
        assert!(discover(&f, &hh(), &[]).is_empty());
        f.exe_dir = Some(PathBuf::from(BESIDE));
        f.subs.clear();
        assert!(discover(&f, &hh(), &[]).is_empty());
    }

    #[test]
    fn a_sibling_app_is_listed_once_per_id_and_not_again_when_known() {
        let mut f = Fake::default();
        beside(&mut f, "a", "vulpes.gadget");
        beside(&mut f, "b", "vulpes.gadget");
        assert_eq!(discover(&f, &hh(), &[]).len(), 1);
        // Already in Hideout: not offered.
        assert!(discover(&f, &hh(), &["vulpes.gadget".into()]).is_empty());
        // Already found by its self-registration: the registered one stands, not a second card.
        let (install, mut g, _) = service_on_disk("beside-reg", "vulpes.gadget", "svc.exe");
        g.subkeys.push("vulpes.gadget".into());
        g.reg.insert(
            (format!(r"{MODULES_KEY}\vulpes.gadget"), String::new()),
            install.display().to_string(),
        );
        beside(&mut g, "gadget", "vulpes.gadget");
        let found = discover(&g, &hh(), &[]);
        assert_eq!(found.len(), 1);
        assert!(
            matches!(&found[0].origin, Origin::Folder(x) if *x == install),
            "{:?} vs {install:?}",
            found[0].origin
        );
    }

    #[test]
    fn a_link_that_leads_out_of_the_apps_folder_is_skipped() {
        let mut f = Fake::default();
        let out = PathBuf::from(r"C:\Elsewhere\gadget");
        f.files.insert(
            out.join("module.toml"),
            (String::new(), Some(service_text("vulpes.evil", "svc.exe"))),
        );
        f.files.insert(out.join("svc.exe"), ("0.1.0".into(), None));
        let link = Path::new(BESIDE).join("apps").join("evil");
        f.links.insert(link.clone(), out.clone());
        f.exe_dir = Some(PathBuf::from(BESIDE));
        f.subs.insert(Path::new(BESIDE).join("apps"), vec![link]);
        assert!(discover(&f, &hh(), &[]).is_empty());
        // The program's own link leading out (the folder stays inside) is skipped too.
        let mut g = Fake::default();
        let folder = beside(&mut g, "gadget", "vulpes.gadget");
        g.files.insert(out.join("svc.exe"), ("0.1.0".into(), None));
        g.links.insert(folder.join("svc.exe"), out.join("svc.exe"));
        assert!(discover(&g, &hh(), &[]).is_empty());
    }

    #[test]
    fn only_the_first_thirty_two_folders_by_name_are_read() {
        let mut f = Fake::default();
        for i in 0..40 {
            beside(&mut f, &format!("app{i:02}"), &format!("vulpes.g{i:02}"));
        }
        // List them out of order: the result is by folder name, not by listing order.
        f.subs
            .get_mut(&Path::new(BESIDE).join("apps"))
            .unwrap()
            .reverse();
        let found = discover(&f, &hh(), &[]);
        assert_eq!(found.len(), 32);
        assert!(found.iter().any(|c| c.id == "vulpes.g31"));
        assert!(!found.iter().any(|c| c.id == "vulpes.g32"));
    }

    #[test]
    fn a_sibling_app_whose_program_is_missing_is_not_offered() {
        let mut f = Fake::default();
        let folder = beside(&mut f, "gadget", "vulpes.gadget");
        f.files.remove(&folder.join("svc.exe"));
        assert!(discover(&f, &hh(), &[]).is_empty());
    }

    // ---- a changed app next to Hideout is offered again ----

    /// A sibling app on real disk (`<exe dir>/apps/gadget`) with a fake `Machine` that reads it.
    struct Sib {
        f: Fake,
        folder: PathBuf,
        home: PathBuf,
    }

    fn sib_text(version: &str, run0: &str) -> String {
        service_text("vulpes.gadget", run0)
            .replace("version = \"0.1.0\"", &format!("version = \"{version}\""))
            .replace(
                "permissions = [\"facts.write:gadget.*\", \"facts.read:keyboard.game_mode\"]",
                "permissions = [\"hub.notify\"]",
            )
            .replace("provides = [\"facts:gadget.*\"]\n", "")
    }

    /// Writes the sibling's file and program (`body`) to disk and to the fake.
    fn sib_write(s: &mut Sib, version: &str, run0: &str, body: &[u8]) {
        let text = sib_text(version, run0);
        std::fs::write(s.folder.join(run0), body).unwrap();
        std::fs::write(s.folder.join("module.toml"), &text).unwrap();
        s.f.files
            .insert(s.folder.join("module.toml"), (String::new(), Some(text)));
        s.f.files
            .insert(s.folder.join(run0), ("0.1.0".into(), None));
    }

    fn sib(name: &str) -> Sib {
        let root = temp(name);
        let folder = root.join("apps").join("gadget");
        std::fs::create_dir_all(&folder).unwrap();
        let mut f = Fake {
            exe_dir: Some(root.clone()),
            ..Fake::default()
        };
        f.subs.insert(root.join("apps"), vec![folder.clone()]);
        let mut s = Sib {
            f,
            folder,
            home: temp(&format!("{name}-home")),
        };
        sib_write(&mut s, "0.1.0", "svc.exe", b"program v1");
        s
    }

    const GADGET: &str = "vulpes.gadget";

    /// Adds the sibling (as the card's Add does) and allows `hub.notify` for it.
    fn sib_added_and_allowed(s: &Sib) {
        let mut a = AddApp::default();
        a.scan(&s.f, &s.home, &[], true);
        assert_eq!(a.found[0].button(), "Add");
        assert!(a.found[0].row_line().is_none());
        assert_eq!(
            a.add_found(&s.f, GADGET, &s.home, &s.home.join("stg"), NOW)
                .as_deref(),
            Some(GADGET)
        );
        let mut reg = hideout_registry::Registry::open_from_env(&s.home).unwrap();
        let list = reg.permission_list(GADGET).unwrap();
        reg.approve_listed(
            GADGET,
            list.code(),
            &["hub.notify".to_string()],
            &[],
            NOW,
            "dialog",
            false,
        )
        .unwrap();
        assert_eq!(reg.inventory.modules[GADGET].granted, ["hub.notify"]);
    }

    #[test]
    fn a_changed_sibling_app_is_offered_as_an_update_and_the_same_one_is_not() {
        let mut s = sib("upd-offer");
        sib_added_and_allowed(&s);
        let held = [GADGET.to_string()];
        // The same version and the same program: nothing to offer.
        assert!(discover(&s.f, &s.home, &held).is_empty());
        // A new version of the file.
        sib_write(&mut s, "0.2.0", "svc.exe", b"program v2");
        let found = discover(&s.f, &s.home, &held);
        assert_eq!(found.len(), 1);
        let c = &found[0];
        assert_eq!(
            (c.version.as_str(), c.update_of.as_deref()),
            ("0.2.0", Some("0.1.0"))
        );
        assert_eq!(words::found_line(&c.name, &c.version), "Gadget 0.2.0");
        assert_eq!(c.button(), "Update in Hideout");
        assert_eq!(
            c.row_line().as_deref(),
            Some("Gadget \u{b7} Changed since you added it")
        );
        // The same version but another program: offered too.
        let mut t = sib("upd-offer-prog");
        sib_added_and_allowed(&t);
        sib_write(&mut t, "0.1.0", "svc2.exe", b"program v1");
        let found = discover(&t.f, &t.home, &[GADGET.to_string()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].update_of.as_deref(), Some("0.1.0"));
        // A changed file with a missing program is not offered.
        let mut u = sib("upd-offer-gone");
        sib_added_and_allowed(&u);
        sib_write(&mut u, "0.2.0", "svc.exe", b"program v2");
        u.f.files.remove(&u.folder.join("svc.exe"));
        assert!(discover(&u.f, &u.home, &[GADGET.to_string()]).is_empty());
    }

    #[test]
    fn pressing_update_registers_the_new_version_and_program_and_the_allow_is_asked_again() {
        let mut s = sib("upd-press");
        sib_added_and_allowed(&s);
        let before = hideout_registry::Registry::open_from_env(&s.home).unwrap();
        assert!(before.manifest(GADGET).unwrap().run.unwrap()[0].ends_with("svc.exe"));
        drop(before);
        // A new build under another file name: new version, new run[0], new hash.
        sib_write(&mut s, "0.2.0", "svc2.exe", b"program v2");
        let held = [GADGET.to_string()];
        let mut a = AddApp::default();
        a.scan(&s.f, &s.home, &held, true);
        assert_eq!(a.shown().len(), 1);
        assert_eq!(
            a.add_found(&s.f, GADGET, &s.home, &temp("upd-press-stg"), NOW)
                .as_deref(),
            Some(GADGET)
        );
        assert_eq!(
            a.note,
            Some(Note::info("Added Gadget. Choose what it may do."))
        );
        let reg = hideout_registry::Registry::open_from_env(&s.home).unwrap();
        let rec = &reg.inventory.modules[GADGET];
        assert_eq!(
            (rec.version.as_str(), rec.source.as_str()),
            ("0.2.0", "adopted")
        );
        let run0 = reg.manifest(GADGET).unwrap().run.unwrap()[0].clone();
        assert!(run0.ends_with("svc2.exe"), "{run0}");
        // Nothing stays allowed: back to pending, the old choice kept only as the suggestion.
        assert!(rec.granted.is_empty(), "{:?}", rec.granted);
        assert_eq!(rec.pending_permissions, ["hub.notify"]);
        assert_eq!(rec.last_choice, ["hub.notify"]);
        assert!(rec.bound.as_ref().is_some_and(|b| b.void));
        assert!(!reg.hub.enabled(GADGET));
        // And what Hideout holds is now the file's: no second offer.
        assert!(discover(&s.f, &s.home, &held).is_empty());
    }

    #[test]
    fn a_record_that_is_not_an_adopted_one_is_not_offered_an_update() {
        let mut s = sib("upd-other");
        sib_added_and_allowed(&s);
        let mut reg = hideout_registry::Registry::open_from_env(&s.home).unwrap();
        reg.inventory.modules.get_mut(GADGET).unwrap().source = "sideload".into();
        reg.inventory.save(&s.home.join("inventory.json")).unwrap();
        drop(reg);
        sib_write(&mut s, "0.2.0", "svc.exe", b"program v2");
        assert!(discover(&s.f, &s.home, &[GADGET.to_string()]).is_empty());
    }

    #[test]
    fn a_registered_copy_that_is_not_the_staged_text_is_unregistered() {
        let (install, f, _) = service_on_disk("svc-rb", "vulpes.gadget", "svc.exe");
        let home = temp("svc-rb-home");
        let c = from_folder(&f, &home, &install).unwrap();
        let id = add(&f, &home, &temp("svc-rb-stg"), &c, NOW).unwrap();
        // Registered, and the copy is the staged text: the check passes and keeps the record.
        let staged =
            adopted_text(c.text.as_deref().unwrap(), c.program.as_deref().unwrap()).unwrap();
        assert!(registered_matches(&home, &id, &staged, true));
        assert!(hideout_registry::Registry::open_from_env(&home)
            .unwrap()
            .inventory
            .modules
            .contains_key(&id));
        let other = format!("{staged}# changed");
        // A different text, on a record that was there before this add: refused, record kept.
        assert!(!registered_matches(&home, &id, &other, false));
        assert!(hideout_registry::Registry::open_from_env(&home)
            .unwrap()
            .inventory
            .modules
            .contains_key(&id));
        // The same on a record this add made: refused, and the record goes.
        assert!(!registered_matches(&home, &id, &other, true));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(!reg.inventory.modules.contains_key(&id));
    }

    /// A card whose file names another id than the card does is refused before anything is
    /// staged: nothing is registered, and a record of the id the file names is not touched.
    #[test]
    fn a_candidate_whose_text_names_another_id_is_refused_and_registers_nothing() {
        let (install, f, _) = service_on_disk("svc-otherid", "vulpes.gadget", "svc.exe");
        let home = temp("svc-otherid-home");
        let mut c = from_folder(&f, &home, &install).unwrap();
        c.id = "vulpes.other".into();
        let stg = temp("svc-otherid-stg");
        assert_eq!(add(&f, &home, &stg, &c, NOW), Err(words::changed(&c.name)));
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert!(reg.inventory.modules.is_empty());
        drop(reg);
        // A record of the id the file names was there already: refused, and it is left as it was.
        c.id = "vulpes.gadget".into();
        add(&f, &home, &stg, &c, NOW).unwrap();
        let held = hideout_registry::Registry::open_from_env(&home)
            .unwrap()
            .inventory
            .modules["vulpes.gadget"]
            .clone();
        c.id = "vulpes.other".into();
        assert!(add(&f, &home, &stg, &c, NOW).is_err());
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        assert_eq!(reg.inventory.modules.len(), 1);
        assert_eq!(reg.inventory.modules["vulpes.gadget"], held);
    }

    /// When the adopt code still reports another id, that record goes only if the call made it.
    #[test]
    fn a_stray_record_is_dropped_only_when_the_call_made_it() {
        let (install, f, _) = service_on_disk("svc-stray", "vulpes.gadget", "svc.exe");
        let home = temp("svc-stray-home");
        let c = from_folder(&f, &home, &install).unwrap();
        add(&f, &home, &temp("svc-stray-stg"), &c, NOW).unwrap();
        let line = "registered vulpes.gadget 0.1.0 (installed separately)";
        let has = || {
            hideout_registry::Registry::open_from_env(&home)
                .unwrap()
                .inventory
                .modules
                .contains_key("vulpes.gadget")
        };
        drop_stray(&home, line, Some(&["vulpes.gadget".to_string()]));
        assert!(has());
        drop_stray(&home, line, None);
        assert!(has());
        drop_stray(&home, line, Some(&[]));
        assert!(!has());
    }

    /// A refused pick says what to do next, in exact words.
    #[test]
    fn a_refused_pick_says_what_to_do_next() {
        assert_eq!(
            words::NOT_READ,
            "That app's file could not be read. Reinstall the app, then try again."
        );
        assert_eq!(
            words::NOT_ADDABLE,
            "That app can't be added to Hideout. Choose another app's folder."
        );
        assert_eq!(
            words::NOT_THIS_PC,
            "Hideout adds apps only from this PC's own drive. Choose a folder there."
        );
        for w in [
            words::NO_FILE,
            words::NOT_READ,
            words::NOT_ADDABLE,
            words::NOT_THIS_PC,
            words::INSIDE_HUB,
        ] {
            let last = w.trim_end_matches('.').rsplit(". ").next().unwrap();
            assert!(w.contains(". "), "{w}");
            assert!(
                last.starts_with("Choose ") || last.ends_with("try again"),
                "{w}"
            );
        }
    }

    /// No UI string names a product: the words module is generic.
    #[test]
    fn the_words_name_no_product() {
        let all = [
            words::FOUND_TITLE,
            words::FOUND_BODY,
            words::ADD,
            words::ADD_AN_APP,
            words::ADD_TITLE,
            words::ADD_BODY,
            words::BROWSE,
            words::CANCEL,
            words::NO_FILE,
            words::NOT_READ,
            words::NOT_ADDABLE,
            words::NOT_THIS_PC,
            words::INSIDE_HUB,
            words::NOT_NOW,
            words::LIST_NOT_READ,
        ];
        let sentences = [
            words::added("Gizmo"),
            words::program_missing("Gizmo"),
            words::not_saved("Gizmo"),
            words::changed("Gizmo"),
            words::add_label("Gizmo"),
        ];
        for w in sentences.iter().map(String::as_str).chain(all) {
            let l = w.to_lowercase();
            for p in find_installed::products() {
                assert!(!l.contains(&p.name.to_lowercase()), "{w}");
            }
            assert!(!l.contains("arete") && !l.contains("module.toml"), "{w}");
            assert!(!l.contains("program"), "{w}");
        }
    }
}
