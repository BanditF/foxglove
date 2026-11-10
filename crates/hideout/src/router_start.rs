// SPDX-License-Identifier: MIT
//! Starts the router (H2.3, contract §8) before the shell: the registry's modules and grants, the
//! router pipe `\\.\pipe\hideout-<user>`, and the `foxglove-<user>` alias when `foxglove.exe`
//! doesn't own it. Then the supervisor (H2.5, §7.7): it takes back what the last hub left
//! running and starts the enabled modules. No window.

use std::path::Path;
use std::sync::mpsc;

use hideout_router::server::{alias_pipe_name, router_pipe_name, Hub};
use hideout_router::{Router, Settings};
use hideout_supervisor::driver::Supervisor;

use crate::panel_feed::{self, HubFeed};

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

/// What the tab strip asks of the supervisor.
pub struct Hooks(pub Supervisor);

impl hideout_shell::TabHooks for Hooks {
    fn start_again(&mut self, module: &str) {
        self.0.start_again(module);
    }
}

/// Starts the router and the supervisor; the panel feed is for the shell (§5): every module's
/// manifest is loaded into the router (`ui.register` checked against its panels and actions)
/// and handed to the panel host (tiles, `when`, Keys and Settings).
pub fn start(home: &Path) -> (Hub, Supervisor, HubFeed) {
    let mut router = Router::new(Settings::default(), approver());
    let mut manifests = Vec::new();
    match hideout_registry::Registry::open(home, hideout_registry::dev_mode_from_env()) {
        Ok(reg) => {
            router.load_registry(&reg);
            manifests.extend(
                reg.inventory
                    .modules
                    .keys()
                    .filter_map(|id| reg.manifest(id)),
            );
        }
        Err(e) => log::warn!("router: registry not read ({e:?}); no modules"),
    }
    log::info!("router: {} module manifest(s) loaded", manifests.len());
    // The panel host draws from every fact it may need: module states, `when` facts such as
    // `apps.seen.<name>`, and bindings. Facts that exist already come out first.
    let early = router.watch_facts(vec![String::new()]);
    let (events, rx) = mpsc::channel();
    let hub = Hub::new(router, events);
    let (side, feed) = panel_feed::feed(hub.clone());
    for m in manifests {
        side.manifest(m);
    }
    for o in &early {
        side.on_router(o);
    }
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
    // After the pipe is served, so modules it starts or takes back can connect. Router events
    // wait in the channel until then.
    let sup = Supervisor::start(home, hub.clone());
    let s = sup.clone();
    std::thread::Builder::new()
        .name("hideout-router-events".into())
        .spawn(move || {
            for event in rx {
                s.on_router(&event);
                side.on_router(&event);
                log::debug!("router: {event:?}");
            }
        })
        .expect("router events thread");
    (hub, sup, feed)
}
