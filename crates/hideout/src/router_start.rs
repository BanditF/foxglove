// SPDX-License-Identifier: MIT
//! Starts the router (H2.3, contract §8) before the shell: the registry's modules and grants, the
//! router pipe `\\.\pipe\hideout-<user>`, and the `foxglove-<user>` alias when `foxglove.exe`
//! doesn't own it. No window, no GUI change.

use std::path::Path;
use std::sync::mpsc;

use hideout_router::server::{alias_pipe_name, router_pipe_name, Hub};
use hideout_router::{Out, Router, Settings};
use serde_json::json;

/// Outside tools until the permission dialog exists (lab only, Phase 2): what isn't sensitive is
/// approved once per tool; sensitive asks (red in the dialog, §6.1) are left out.
fn approver() -> hideout_router::Approver {
    Box::new(|tool, asked| {
        let (yes, no): (Vec<_>, Vec<_>) = asked.iter().cloned().partition(|c| !c.sensitive());
        if !no.is_empty() {
            log::warn!("router: {tool} asked for sensitive {no:?}; needs the permission dialog");
        }
        yes
    })
}

pub fn start(home: &Path) -> Hub {
    let mut router = Router::new(Settings::default(), approver());
    match hideout_registry::Registry::open(home, hideout_registry::dev_mode_from_env()) {
        Ok(reg) => router.load_registry(&reg),
        Err(e) => log::warn!("router: registry not read ({e:?}); no modules"),
    }
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let h = hub.clone();
    std::thread::Builder::new()
        .name("hideout-router-events".into())
        .spawn(move || {
            for event in rx {
                match event {
                    // No supervisor yet (H2.4): a module whose connection ends is off.
                    Out::ModuleGone(id) => {
                        h.act(|r| r.publish(&format!("module.{id}.state"), json!("off")))
                    }
                    other => log::info!("router: {other:?}"),
                }
            }
        })
        .expect("router events thread");
    #[cfg(windows)]
    {
        let name = router_pipe_name();
        match hub.serve_pipe(&name) {
            Ok(_) => log::info!("router: serving {name}"),
            Err(e) => log::error!("router: {name}: {e}"),
        }
        // §8.2, §8.6: the alias only once foxglove.exe no longer owns the name.
        let alias = alias_pipe_name();
        match hub.serve_pipe(&alias) {
            Ok(_) => log::info!("router: serving the alias {alias}"),
            Err(e) => log::info!("router: alias {alias} not served (owned elsewhere): {e}"),
        }
    }
    hub
}
