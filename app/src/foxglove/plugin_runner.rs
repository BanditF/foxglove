//! Runs the enabled plugins (foxglove_plugin_host::runner): started at launch, started and stopped as the Plugins tab
//! changes, each with only the permissions the user approved. `FOXGLOVE_DISABLE=plugins` leaves
//! them all off.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use foxglove_plugin_host::runner::{Config, PluginSpec, PluginState, Runner};
use foxglove_plugin_host::{Client, Context, Limits};

static RUNNERS: Mutex<Option<HashMap<String, Runner>>> = Mutex::new(None);
static STATES: Mutex<Option<HashMap<String, PluginState>>> = Mutex::new(None);
/// The running plugins that were launched unsigned (#381), with their package hashes.
static UNSIGNED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// A Plugin's state for the Plugins tab, if it has been started this session.
pub fn state(id: &str) -> Option<PluginState> {
    STATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()?
        .get(id)
        .cloned()
}

/// Starts enabled plugins that aren't running and stops the rest. Call at launch and after every
/// change in the Plugins tab.
pub fn sync() {
    if super::repaint_trace::disabled("plugins") {
        return;
    }
    let Some(dir) = super::plugin_install::plugins_dir() else {
        return;
    };
    let enabled: Vec<_> = super::plugin_install::installed()
        .into_iter()
        .filter(|k| k.record.enabled)
        .collect();
    // Before every launch: still signed with its files unchanged (#159), or an unsigned build
    // the user allowed in developer mode (#381). A running plugin launched unsigned is checked
    // again against developer mode, so turning it off stops it; it stays installed.
    use super::plugin_trust::{self as trust, Trust};
    let running: Vec<String> = RUNNERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default();
    let mut unsigned = UNSIGNED.lock().unwrap_or_else(|e| e.into_inner());
    let unsigned = unsigned.get_or_insert_with(HashMap::new);
    unsigned.retain(|id, _| running.contains(id));
    let dev = trust::dev_mode();
    let mut catalog = None;
    let mut wanted = Vec::new();
    for plugin in enabled {
        let id = &plugin.manifest.id;
        let t = if running.contains(id) {
            unsigned
                .get(id)
                .map_or(Trust::Signed, |h| Trust::Unsigned(h.clone()))
        } else {
            let c = catalog.get_or_insert_with(|| trust::catalog().ok());
            let t = trust::trust_in(c.as_ref(), id, &plugin.manifest.version, &dir.join(id));
            match &t {
                Trust::Unsigned(h) => unsigned.insert(id.clone(), h.clone()),
                _ => unsigned.remove(id),
            };
            t
        };
        match trust::verdict(&t, &dev) {
            Ok(()) => wanted.push(plugin),
            Err(why) => {
                STATES
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(HashMap::new)
                    .insert(plugin.manifest.id.clone(), PluginState::Disabled(why));
            }
        }
    }
    let mut guard = RUNNERS.lock().unwrap_or_else(|e| e.into_inner());
    let runners = guard.get_or_insert_with(HashMap::new);
    runners.retain(|id, _| wanted.iter().any(|k| &k.manifest.id == id));
    for plugin in wanted {
        let id = plugin.manifest.id.clone();
        if runners.contains_key(&id) {
            continue;
        }
        use foxglove_plugin_api::permission::Permission;
        // Plugins that don't ask for the network or the user's files don't get either.
        let sandbox = !plugin
            .record
            .approved
            .iter()
            .any(|p| matches!(p, Permission::Net | Permission::Files));
        let spec = PluginSpec {
            id: id.clone(),
            dir: dir.join(&id),
            run: plugin.manifest.run.clone(),
            sandbox,
        };
        let approved = plugin.record.approved.clone();
        let status_id = id.clone();
        let runner = Runner::start(
            spec,
            Config::default(),
            move || Context {
                facts: super::devices::bus().facts().clone(),
                devices: Arc::new(super::api::AppDevices),
                client: Client::Plugin(approved.clone()),
                app_version: env!("CARGO_PKG_VERSION").into(),
                limits: Limits::default(),
            },
            move |s| {
                if let PluginState::Restarting { reason, .. } | PluginState::Disabled(reason) = &s {
                    eprintln!("Foxglove plugin {status_id}: {reason}");
                }
                STATES
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_insert_with(HashMap::new)
                    .insert(status_id.clone(), s);
            },
        );
        runners.insert(id, runner);
    }
}

/// Stops one plugin (before removing its files, which Windows keeps locked while it runs).
pub fn stop(id: &str) {
    let runner = RUNNERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|r| r.remove(id));
    drop(runner);
    // Give the job a moment to end so its files unlock.
    std::thread::sleep(std::time::Duration::from_millis(300));
}
