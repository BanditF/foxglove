// SPDX-License-Identifier: MIT
//! Starts the router (H2.3, contract §8) before the shell: the registry's modules and grants, the
//! router pipe `\\.\pipe\hideout-<user><suffix>`, and the `foxglove-<user><suffix>` alias when
//! `foxglove.exe` doesn't own it; `<suffix>` names a hub folder other than the default (#1010). Then the supervisor (H2.5, §7.7): it takes back what the last hub left
//! running and starts the enabled modules. No window.

use std::path::Path;
use std::sync::mpsc;

use hideout_router::server::Hub;
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

    fn start_with_defaults(&mut self, module: &str) {
        self.0.start_with_defaults(module);
    }

    fn go_back(&mut self, module: &str) {
        self.0.go_back(module);
    }

    fn turn_off(&mut self, module: &str) {
        self.0.turn_off(module);
    }

    fn check_for_updates(&mut self, module: &str) {
        self.0.check_for_updates(module);
    }

    fn diagnostics(&mut self, module: &str) -> Option<String> {
        Some(self.0.diagnostics(module, env!("CARGO_PKG_VERSION")))
    }
}

/// What a notification button hands back: `start-again:<id>`, `open:<id>` (a click on the
/// toast itself opens the module too) or `open-log:<id>` (`logs\<id>.log`, as the crash bar's
/// Open log). OK only closes the toast.
pub fn on_toast(sup: &Supervisor, home: &Path, arg: &str) {
    match arg.split_once(':') {
        Some(("start-again", id)) => sup.start_again(id),
        // The withdrawn notification (#1026): the check runs, and the page shows its result.
        Some(("check-updates", id)) => {
            sup.check_for_updates(id);
            hideout_shell::open_module(id);
        }
        Some(("open", id)) => hideout_shell::open_module(id),
        Some(("open-log", id)) => hideout_shell::open_module_log(home, id),
        _ => {}
    }
}

/// The buttons of a supervisor notification with what each hands back: Start again, Open
/// <Module> (the stopped one), Open log (the rollback one); anything else (OK) closes it.
pub fn toast_buttons(id: &str, actions: &[String]) -> Vec<(String, String)> {
    use hideout_supervisor::words;
    let open = actions
        .first()
        .is_some_and(|a| a == words::START_AGAIN || a == words::CHECK_FOR_UPDATES);
    actions
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let arg = if label == words::START_AGAIN {
                format!("start-again:{id}")
            } else if label == words::CHECK_FOR_UPDATES {
                format!("check-updates:{id}")
            } else if label == words::OPEN_LOG {
                format!("open-log:{id}")
            } else if i == 1 && open {
                format!("open:{id}")
            } else {
                String::from("dismiss")
            };
            (label.clone(), arg)
        })
        .collect()
}

/// The supervisor's notifications as Windows toasts (§7.7: once per module per sign-in, never
/// for a restart; the policy decides).
fn notifier(sup: Supervisor, home: &Path) -> hideout_supervisor::driver::Notifier {
    hideout_shell::notify::set_home(home);
    let home = home.to_path_buf();
    Box::new(move |id, title, body, actions| {
        let (sup, home) = (sup.clone(), home.clone());
        hideout_shell::notify::toast(
            title.into(),
            body.into(),
            toast_buttons(id, actions),
            format!("open:{id}"),
            std::sync::Arc::new(move |arg| on_toast(&sup, &home, arg)),
        );
    })
}

/// Starts the router and the supervisor; the panel feed is for the shell (§5): every module's
/// manifest is loaded into the router (`ui.register` checked against its panels and actions)
/// and handed to the panel host (tiles, `when`, Keys and Settings). `suffix` is the folder's
/// (`HubPaths::instance_suffix`): the pipes and the modules' `HIDEOUT_PIPE` carry it. `fresh`
/// is the Modules page's list, which the inventory watch keeps current.
pub fn start(
    home: &Path,
    suffix: &str,
    fresh: crate::modules_page::Fresh,
) -> (Hub, Supervisor, HubFeed) {
    let mut router = Router::new(Settings::default(), approver());
    let mut manifests = Vec::new();
    // Lab mode (#1001, docs/hideout-lab.md): said once, refused with its reason.
    match hideout_registry::lab::lab_mode_from_env(home) {
        hideout_registry::lab::LabMode::On => log::warn!(
            "lab: lab mode on; loading lab modules from {}",
            hideout_registry::lab::LabMode::modules_dir(home).display()
        ),
        hideout_registry::lab::LabMode::Refused(why) => {
            log::warn!("lab: lab mode refused: {why}; no lab modules loaded")
        }
        hideout_registry::lab::LabMode::Off => {}
    }
    match hideout_registry::Registry::open_from_env(home) {
        Ok(reg) => {
            for line in &reg.lab_report {
                log::info!("lab: {line}");
            }
            // §7.10: the flag in hideout.toml; Start normally clears it.
            if reg.hub.safe_mode() {
                log::warn!("router: safe mode (hideout.toml); no module starts by itself");
                router.set_safe_mode(true);
            }
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
    hub.set_folder_suffix(suffix);
    let (side, feed) = panel_feed::feed(hub.clone());
    // `hideout.appwatch` (§4.6, v1.7): only the apps the manifests ask about, by name from the
    // manifests; with none it doesn't run. Its facts go through the router like any hub fact.
    let apps: std::collections::BTreeSet<String> =
        manifests.iter().flat_map(|m| m.watched_apps()).collect();
    if !apps.is_empty() {
        log::info!("appwatch: watching {} app(s)", apps.len());
    }
    let h = hub.clone();
    hideout_appwatch::start(
        apps,
        home,
        Box::new(move |changes| {
            h.act(|r| {
                changes
                    .into_iter()
                    .flat_map(|(k, v)| r.publish(&k, v))
                    .collect()
            })
        }),
    );
    for m in manifests {
        side.manifest(m);
    }
    // The inventory watch starts at the end, once the supervisor runs (#1054).
    let gone = side.clone();
    for o in &early {
        side.on_router(o);
    }
    #[cfg(windows)]
    {
        let name = hub.router_pipe();
        match hub.serve_pipe(&name) {
            Ok(_) => log::info!("router: serving {name}"),
            Err(e) => log::error!("router: {name}: {e}"),
        }
        // §8.2, §8.6: the alias only once foxglove.exe no longer owns the name.
        let alias = hub.alias_pipe();
        match hub.serve_pipe(&alias) {
            Ok(_) => log::info!("router: serving the alias {alias}"),
            Err(e) => log::info!("router: alias {alias} not served (owned elsewhere): {e}"),
        }
    }
    // After the pipe is served, so modules it starts or takes back can connect. Router events
    // wait in the channel until then.
    let sup = Supervisor::start(home, hub.clone());
    // Activation facts already set (§4.6, v1.10): once the supervisor knows its modules. The
    // Activate events wait in the channel for the events thread below.
    hub.act(|r| r.activation_due(None));
    sup.set_notifier(notifier(sup.clone(), home));
    // Check for updates (#1026) and the catalog fetch (#1049): a normal hub fetches each
    // source once now and the module's source on Check for updates (no source configured:
    // nothing is fetched and the check reads "Couldn't check"). A lab hub reads no trust; its
    // check re-reads lab-withdrawn.json (the registry reads it again on every open), the lab's
    // stand-in for a fresh catalog.
    if hideout_registry::lab::lab_mode_from_env(home).on() {
        sup.set_refresh(std::sync::Arc::new(|id: &str| {
            log::info!("lab: check for updates for {id}: lab-withdrawn.json read again");
            Ok(())
        }));
    } else {
        let fetch: crate::catalog_fetch::Fetcher =
            std::sync::Arc::new(crate::https::WinHttp::default());
        sup.set_refresh(crate::catalog_fetch::refresh(home, fetch.clone()));
        crate::catalog_fetch::fetch_at_start(home, sup.clone(), fetch);
    }
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
    watch_inventory(home, gone, hub.clone(), sup.clone(), fresh);
    (hub, sup, feed)
}

/// The inventory watch (§7.1, #1012.4, #1054): a module that leaves the inventory has its
/// tabs and pop-outs closed and is dropped by the supervisor and the router (the registry
/// refuses to remove a running module); on any change the Modules page gets the list again.
fn watch_inventory(
    home: &Path,
    side: panel_feed::RouterSide,
    hub: Hub,
    sup: Supervisor,
    fresh: crate::modules_page::Fresh,
) {
    let at = home.to_path_buf();
    let watched = crate::inventory_watch::start(
        home,
        Box::new(move |change| {
            // The router and the supervisor forget first, so whoever the shell's removal
            // message wakes already sees the module gone.
            for id in &change.removed {
                sup.forget(id);
                hub.act(|r| r.forget_module(id));
                side.removed(id);
            }
            if fresh.put(crate::modules_page::installed(&at)) {
                side.wake();
            }
        }),
    );
    if let Err(e) = watched {
        log::warn!(
            "inventory watch not started: {e}; installs and removals show at the next start"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_supervisor::words;

    /// #1054.1–2, window-free: the watch's chain on a temp hub folder with a real router,
    /// supervisor and feed. A removal tells the shell, and the router forgets the module; an
    /// install and the removal both give the Modules page a fresh list. Start event: the
    /// inventory write; stop event: the feed's wake; 10 s is the fallback only.
    #[cfg(windows)]
    #[test]
    fn a_removal_and_an_install_reach_the_router_and_the_modules_page() {
        use hideout_shell::hideout_panels::{Inbound, PanelFeed};
        let home = std::env::temp_dir().join(format!("hideout-rs-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let entry = |id: &str| {
            format!(
                r#""{id}": {{"version": "1.0.0", "kind": "service", "family": "foxglove",
                   "visibility": "public", "source": "sideload", "channel": "stable",
                   "installed": "2026-11-10T00:00:00Z", "files": {{}}}}"#
            )
        };
        let save = |ids: &[&str]| {
            let m: Vec<_> = ids.iter().map(|id| entry(id)).collect();
            let text = format!(
                r#"{{"format": "hideout-inventory-v1", "modules": {{{}}}}}"#,
                m.join(",")
            );
            std::fs::write(home.join("inventory.json.tmp"), text).unwrap();
            std::fs::rename(home.join("inventory.json.tmp"), home.join("inventory.json")).unwrap();
        };
        save(&["a.art", "b.beat"]);
        let reg = hideout_registry::Registry::open_from_env(&home).unwrap();
        let mut router = Router::new(Settings::default(), approver());
        router.load_registry(&reg);
        assert!(router.knows_module("b.beat"));
        let (events, _rx) = mpsc::channel();
        let hub = Hub::new(router, events);
        hub.set_folder_suffix(&hideout_router::server::folder_suffix(&home));
        let (side, mut feed) = panel_feed::feed(hub.clone());
        let (woke, wakes) = mpsc::channel::<()>();
        let woke = std::sync::Mutex::new(woke);
        feed.set_waker(Box::new(move || {
            let _ = woke.lock().unwrap().send(());
        }));
        let sup = Supervisor::start(&home, hub.clone());
        let fresh = crate::modules_page::Fresh::default();
        watch_inventory(&home, side, hub.clone(), sup, fresh.clone());
        let mut ids_after = |want: &[&str]| -> Vec<Inbound> {
            let mut seen = Vec::new();
            loop {
                wakes
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("no wake from the watch");
                seen.extend(feed.drain());
                if let Some(list) = fresh.take() {
                    let ids: Vec<_> = list.iter().map(|m| m.id.as_str()).collect();
                    if ids == want {
                        return seen;
                    }
                }
            }
        };
        save(&["a.art", "b.beat", "c.chat"]);
        ids_after(&["a.art", "b.beat", "c.chat"]);
        save(&["a.art", "c.chat"]);
        let mut seen = ids_after(&["a.art", "c.chat"]);
        // The removal reaches the feed on its own path; the list can land first (flaky on a
        // loaded gate, 11/11). Wait, bounded, for the removal too.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !seen.contains(&Inbound::ModuleRemoved {
            module: "b.beat".into(),
        }) && std::time::Instant::now() < until
        {
            let _ = wakes.recv_timeout(std::time::Duration::from_millis(200));
            seen.extend(feed.drain());
        }
        assert!(
            seen.contains(&Inbound::ModuleRemoved {
                module: "b.beat".into()
            }),
            "the shell wasn't told: {seen:?}"
        );
        assert!(!hub.read(|r| r.knows_module("b.beat")));
        assert!(hub.read(|r| r.knows_module("a.art")));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn every_toast_button_does_what_it_says() {
        let (_, _, stopped) = words::stopped_notification("Art");
        assert_eq!(
            toast_buttons("m.art", &stopped),
            vec![
                ("Start again".to_string(), "start-again:m.art".to_string()),
                ("Open Art".into(), "open:m.art".into()),
            ]
        );
        let (_, _, withdrawn) = words::withdrawn_notification("Art", "0.5.0");
        assert_eq!(
            toast_buttons("m.art", &withdrawn),
            vec![
                (
                    "Check for updates".to_string(),
                    "check-updates:m.art".to_string()
                ),
                ("Open Art".into(), "open:m.art".into()),
            ]
        );
        let rollback = vec![words::OK.to_string(), words::OPEN_LOG.to_string()];
        assert_eq!(
            toast_buttons("m.art", &rollback),
            vec![
                ("OK".to_string(), "dismiss".to_string()),
                ("Open log".into(), "open-log:m.art".into()),
            ]
        );
    }
}
