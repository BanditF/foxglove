//! Other keyboards over VIA (#74), read-only, off by default: QMK keyboards such as the
//! Keychron K5 Pro, found by the VIA JSON definitions the user loads. They join the device layer (facts,
//! the local API, plugins) next to the host-link keyboard. Nothing is written to them: the VIA
//! backend opens devices read-only, and reset/bootloader/factory commands never go out.
//!
//! Settings in the store's `[via]`; scanning happens on demand (Rescan, turning it on, adding a
//! definition), never on a timer.

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
    /// Keyboards Foxglove leaves alone (`vvvv:pppp`), so VIA or the maker's app can use them
    /// (#74): their handles are closed and they aren't opened again until resumed.
    pub paused: Vec<String>,
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
    /// `vvvv:pppp`, for pausing it; none for a definition or scan error.
    pub key: Option<String>,
    pub paused: bool,
}

/// How a keyboard is named for pausing: its USB vendor and product ids.
pub fn key(id: &foxglove_devices::DeviceId) -> String {
    format!("{:04x}:{:04x}", id.vid, id.pid)
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
            .map(|e| Found { name: "definition".into(), opened: Err(e), key: None, paused: false })
            .collect();
        found.extend(report.errors.iter().map(|(_, e)| Found { name: "scan".into(), opened: Err(e.to_string()), key: None, paused: false }));
        for entry in report.devices {
            let key = key(&entry.info.id);
            // Paused: listed, never opened, so another app has it to itself.
            if s.paused.contains(&key) {
                found.push(Found { name: entry.info.name.clone(), opened: Err("paused".into()), key: Some(key), paused: true });
                continue;
            }
            let opened = registry
                .open(&entry.info.id)
                .and_then(|d| d.keymap().ok_or(foxglove_devices::DeviceError::Unsupported)?.layers().map(|l| l.len()))
                .map_err(|e| super::devices::describe(&e));
            found.push(Found { name: entry.info.name.clone(), opened, key: Some(key), paused: false });
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
