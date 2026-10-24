//! Other keyboards over VIA (#74), read-only, off by default: QMK keyboards such as the
//! Keychron K5 Pro, found by the VIA JSON definitions the user loads. They join the device layer (facts,
//! the local API, plugins) next to the host-link keyboard. Nothing is written to them: the VIA
//! backend opens devices read-only, and reset/bootloader/factory commands never go out.
//!
//! Settings in the store's `[via]`; scanning happens on demand (Rescan, turning it on, adding a
//! definition), never on a timer.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use foxglove_via::hid::ViaBackend;
use foxglove_via::Definition;
use foxglove_devices::{Device, Registry};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViaSettings {
    pub enabled: bool,
    /// VIA JSON definitions (Keychron publishes them per keyboard).
    pub definitions: Vec<PathBuf>,
    /// Keyboards Foxglove leaves alone (by [`Known::id`]; `vvvv:pppp` from before #292 still
    /// counts), so VIA or the maker's app can use them (#74): their handles are closed and they
    /// aren't opened again until resumed.
    pub paused: Vec<String>,
    /// Keyboards seen before, so one that comes back on another port can be recognised (#292).
    pub known: Vec<Known>,
    /// A keyboard's id on a new port to the id it was first known by, after the user said
    /// "This is the same keyboard": its saved state (pause) follows it.
    pub aliases: BTreeMap<String, String>,
}

/// A keyboard Foxglove has seen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// Its device id when first seen.
    pub id: String,
    pub name: String,
    /// `vvvv:pppp`: only a keyboard of the same model can be the same keyboard.
    pub model: String,
}

/// What a scan makes of one keyboard.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    /// The id its saved state is under: its own, or the one it was confirmed to be.
    pub id: String,
    /// Not known, but a known keyboard of the same model is missing: maybe it moved. The user
    /// is asked; until then it's treated as new but not remembered.
    pub maybe: Option<Known>,
    /// Known now for the first time (remembered by the scan).
    pub new: bool,
}

/// Recognises the keyboards present (`(id, model)`) against the saved ones. Pure, for the scan
/// and the tests.
pub fn recognise(present: &[(String, String)], known: &[Known], aliases: &BTreeMap<String, String>) -> Vec<Seen> {
    let canonical = |id: &str| aliases.get(id).cloned().unwrap_or_else(|| id.to_string());
    let present_ids: Vec<String> = present.iter().map(|(id, _)| canonical(id)).collect();
    present
        .iter()
        .map(|(id, model)| {
            let id = canonical(id);
            if known.iter().any(|k| k.id == id) {
                return Seen { id, maybe: None, new: false };
            }
            let missing = known.iter().find(|k| &k.model == model && !present_ids.contains(&k.id)).cloned();
            Seen { new: missing.is_none(), maybe: missing, id }
        })
        .collect()
}

pub fn settings() -> ViaSettings {
    super::store::try_load("via").unwrap_or_default()
}

pub fn save(s: &ViaSettings) -> Result<(), String> {
    super::store::save("via", s).map_err(|e| e.to_string())?;
    rescan();
    Ok(())
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);
static STATUS: Mutex<Vec<Found>> = Mutex::new(Vec::new());

/// A keyboard found on the last scan, for the settings.
#[derive(Clone, Debug)]
pub struct Found {
    pub name: String,
    /// `Ok(layers)` when it answered VIA, else why not.
    pub opened: Result<usize, String>,
    /// The id its saved state is under, for pausing it; none for a definition or scan error.
    pub key: Option<String>,
    pub paused: bool,
    /// Its id on this port, and the known keyboard it might be (asked in the settings).
    pub maybe: Option<(String, Known)>,
}

/// A keyboard's model: its USB vendor and product ids.
pub fn model(id: &foxglove_devices::DeviceId) -> String {
    format!("{:04x}:{:04x}", id.vid, id.pid)
}

/// The user's answer: the keyboard on this port is `known` (its saved state follows it), or
/// a different keyboard (remembered as itself).
pub fn same_keyboard(id: &str, name: &str, model: &str, same_as: Option<&Known>) -> Result<(), String> {
    let mut s = settings();
    match same_as {
        Some(k) => {
            s.aliases.insert(id.to_string(), k.id.clone());
        }
        None => s.known.push(Known { id: id.to_string(), name: name.to_string(), model: model.to_string() }),
    }
    save(&s)
}

/// Pauses or resumes Foxglove for one keyboard; a rescan closes or reopens its handle.
pub fn set_paused(key: &str, paused: bool) -> Result<(), String> {
    let mut s = settings();
    s.paused.retain(|k| k != key);
    if paused {
        s.paused.push(key.to_string());
    }
    save(&s)
}

pub fn found() -> Vec<Found> {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Loads the definitions; returns the ones that parse and the errors for the rest.
fn definitions(s: &ViaSettings) -> (Vec<Definition>, Vec<String>) {
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    for path in &s.definitions {
        match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| Definition::parse(&t)) {
            Ok(d) => ok.push(d),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    (ok, errors)
}

/// Finds and opens (read-only) the VIA keyboards matching the definitions; closes them all
/// when turned off. Runs on a background thread.
pub fn rescan() {
    super::services::spawn_named("via-scan", || {
        let s = settings();
        let mut guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None; // closes what was open
        if !s.enabled {
            STATUS.lock().unwrap_or_else(|e| e.into_inner()).clear();
            return;
        }
        let (defs, errors) = definitions(&s);
        let mut registry = Registry::new(super::devices::bus().clone());
        let _ = registry.add_backend(Box::new(ViaBackend::new(defs)));
        let report = registry.discover();
        let mut found: Vec<Found> = errors
            .into_iter()
            .map(|e| Found { name: "definition".into(), opened: Err(e), key: None, paused: false, maybe: None })
            .collect();
        found.extend(report.errors.iter().map(|(_, e)| Found { name: "scan".into(), opened: Err(e.to_string()), key: None, paused: false, maybe: None }));
        let present: Vec<(String, String)> = report.devices.iter().map(|e| (e.info.id.to_string(), model(&e.info.id))).collect();
        let seen = recognise(&present, &s.known, &s.aliases);
        let mut learned = Vec::new();
        for (entry, seen) in report.devices.iter().zip(seen) {
            let (own, model) = (entry.info.id.to_string(), model(&entry.info.id));
            if seen.new {
                learned.push(Known { id: own.clone(), name: entry.info.name.clone(), model: model.clone() });
            }
            let maybe = seen.maybe.map(|k| (own, k));
            let key = seen.id;
            // Paused: listed, never opened, so another app has it to itself.
            if s.paused.contains(&key) || s.paused.contains(&model) {
                found.push(Found { name: entry.info.name.clone(), opened: Err("paused".into()), key: Some(key), paused: true, maybe });
                continue;
            }
            let opened = registry
                .open(&entry.info.id)
                .and_then(|d| d.keymap().ok_or(foxglove_devices::DeviceError::Unsupported)?.layers().map(|l| l.len()))
                .map_err(|e| super::devices::describe(&e));
            found.push(Found { name: entry.info.name.clone(), opened, key: Some(key), paused: false, maybe });
        }
        if !learned.is_empty() {
            let mut s = settings();
            s.known.extend(learned);
            if let Err(e) = super::store::save("via", &s) {
                eprintln!("Foxglove: remembering VIA keyboards: {e}");
            }
        }
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = found;
        *guard = Some(registry);
    });
}

/// Runs `f` on an open VIA keyboard by its id.
pub fn with_device<R>(id: &str, f: impl FnOnce(&dyn Device) -> R) -> Option<R> {
    let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let registry = guard.as_ref()?;
    let entry = registry.list().into_iter().find(|e| e.info.id.to_string() == id)?;
    registry.device(&entry.info.id).map(f)
}

/// The open VIA keyboards, for `devices.list`.
pub fn devices() -> Vec<(foxglove_devices::DeviceInfo, foxglove_devices::Caps)> {
    let guard = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(registry) = guard.as_ref() else { return vec![] };
    registry
        .list()
        .into_iter()
        .filter_map(|e| registry.device(&e.info.id).map(|d| (e.info.clone(), d.caps())))
        .collect()
}

/// Opens the configured VIA keyboards at start-up, if turned on.
pub fn start() {
    if settings().enabled && !super::repaint_trace::disabled("via") {
        rescan();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(id: &str) -> Known {
        Known { id: id.into(), name: "K5 Pro".into(), model: "3434:0250".into() }
    }

    #[test]
    fn a_keyboard_on_a_new_port_is_asked_about_and_an_answer_sticks() {
        let model = "3434:0250".to_string();
        let mut aliases = BTreeMap::new();
        // First time: new, remembered.
        let seen = recognise(&[("port-a".into(), model.clone())], &[], &aliases);
        assert_eq!(seen, [Seen { id: "port-a".into(), maybe: None, new: true }]);
        let k = vec![known("port-a")];
        // Back on its port: known.
        assert_eq!(recognise(&[("port-a".into(), model.clone())], &k, &aliases)[0], Seen { id: "port-a".into(), maybe: None, new: false });
        // On another port while port-a is away: maybe the same one.
        let moved = recognise(&[("port-b".into(), model.clone())], &k, &aliases);
        assert_eq!(moved[0].maybe.as_ref().map(|k| k.id.as_str()), Some("port-a"));
        assert!(!moved[0].new, "not remembered until the user answers");
        // "This is the same keyboard": its state is under port-a's id from now on.
        aliases.insert("port-b".into(), "port-a".into());
        assert_eq!(recognise(&[("port-b".into(), model.clone())], &k, &aliases)[0], Seen { id: "port-a".into(), maybe: None, new: false });
        // Two of the same model at once: the one on a new port is a different keyboard.
        let both = recognise(&[("port-a".into(), model.clone()), ("port-c".into(), model.clone())], &k, &BTreeMap::new());
        assert_eq!(both[1], Seen { id: "port-c".into(), maybe: None, new: true });
        // Another model is never asked about.
        assert!(recognise(&[("x".into(), "1234:0001".into())], &k, &BTreeMap::new())[0].new);
    }

    #[test]
    fn the_saved_state_round_trips() {
        let mut s = ViaSettings { enabled: true, paused: vec!["port-a".into(), "3434:0250".into()], known: vec![known("port-a")], ..Default::default() };
        s.aliases.insert("port-b".into(), "port-a".into());
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<ViaSettings>(&text).unwrap(), s);
        // A store from before #292 (no known, no aliases) still loads.
        let old: ViaSettings = toml::from_str("enabled = true\npaused = [\"3434:0250\"]\n").unwrap();
        assert!(old.known.is_empty() && old.aliases.is_empty());
    }
}
