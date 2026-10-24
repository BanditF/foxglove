//! Runs the enabled Kits (den::runner): started at launch, started and stopped as the Den
//! changes, each with only the permissions the user approved. `FOXGLOVE_DISABLE=kits` leaves
//! them all off.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use den::runner::{Config, KitSpec, KitState, Runner};
use den::{Client, Context, Limits};

static RUNNERS: Mutex<Option<HashMap<String, Runner>>> = Mutex::new(None);
static STATES: Mutex<Option<HashMap<String, KitState>>> = Mutex::new(None);

/// A Kit's state for the Den, if it has been started this session.
pub fn state(id: &str) -> Option<KitState> {
    STATES.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(id).cloned()
}

/// Starts enabled Kits that aren't running and stops the rest. Call at launch and after every
/// change in the Den.
pub fn sync() {
    if super::repaint_trace::disabled("kits") {
        return;
    }
    let Some(dir) = super::den::kits_dir() else { return };
    let wanted: Vec<_> = super::den::installed().into_iter().filter(|k| k.record.enabled).collect();
    let mut guard = RUNNERS.lock().unwrap_or_else(|e| e.into_inner());
    let runners = guard.get_or_insert_with(HashMap::new);
    runners.retain(|id, _| wanted.iter().any(|k| &k.manifest.id == id));
    for kit in wanted {
        let id = kit.manifest.id.clone();
        if runners.contains_key(&id) {
            continue;
        }
        use foxglove_kit_api::permission::Permission;
        // Kits that don't ask for the network or the user's files don't get either.
        let sandbox = !kit.record.approved.iter().any(|p| matches!(p, Permission::Net | Permission::Files));
        let spec = KitSpec { id: id.clone(), dir: dir.join(&id), run: kit.manifest.run.clone(), sandbox };
        let approved = kit.record.approved.clone();
        let status_id = id.clone();
        let runner = Runner::start(
            spec,
            Config::default(),
            move || Context {
                facts: super::palm_host::bus().facts().clone(),
                devices: Arc::new(super::api::AppDevices),
                client: Client::Kit(approved.clone()),
                app_version: env!("CARGO_PKG_VERSION").into(),
                limits: Limits::default(),
            },
            move |s| {
                if let KitState::Restarting { reason, .. } | KitState::Disabled(reason) = &s {
                    eprintln!("Foxglove Kit {status_id}: {reason}");
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

/// Stops one Kit (before removing its files, which Windows keeps locked while it runs).
pub fn stop(id: &str) {
    let runner = RUNNERS.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|r| r.remove(id));
    drop(runner);
    // Give the job a moment to end so its files unlock.
    std::thread::sleep(std::time::Duration::from_millis(300));
}
