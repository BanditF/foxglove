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

    fn update_hub(&mut self) {
        hideout_shell::open_update_page();
    }

    fn force_update(&mut self, module: &str) {
        self.0.force_update(module);
    }

    fn not_now(&mut self, module: &str) {
        self.0.not_now(module);
    }

    fn quarantine(&mut self, module: &str) {
        self.0.quarantine(module);
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
            note_needs_hub_names(&reg);
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
    // Added ids the registry didn't hold yet (a lab folder copied before its manifest): while
    // any waits, a change of only files inside `lab-modules\` is read too (#1076).
    let mut waiting = std::collections::BTreeSet::new();
    // Ids learnt that way: asked again at each change, so an install that enables the module
    // in a second write (`hideout.toml` after `inventory.json`) still starts it.
    let mut late = std::collections::BTreeSet::<String>::new();
    let watched = crate::inventory_watch::start(
        home,
        Box::new(move |change| {
            if change.files_only && waiting.is_empty() {
                // Only files inside a lab module's folder changed: nothing to read again.
                return;
            }
            // The router and the supervisor forget first, so whoever the shell's removal
            // message wakes already sees the module gone.
            for id in &change.removed {
                waiting.remove(id);
                late.remove(id);
                sup.forget(id);
                hub.act(|r| r.forget_module(id));
                side.removed(id);
            }
            // One read for the supervisor and the page, so the page never lists what the
            // supervisor hasn't seen (#1076). A failed read keeps the page's last list.
            let reg = match hideout_registry::Registry::open_from_env(&at) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "Modules page: the installed modules couldn't be read ({e}); the last list stays"
                    );
                    waiting.extend(change.added.iter().cloned());
                    return;
                }
            };
            // Joined after the start (#1076): the router and the supervisor learn it, so an
            // enabled module starts now; the panel host gets its manifest. Taken from this read,
            // not from the watch's ids: those came from a read a moment earlier, which a write
            // landing in between makes older than the page's list.
            for id in &late {
                let _ = sup.learn_from(&reg, id);
            }
            waiting.extend(change.added.iter().cloned());
            let new: Vec<String> = reg
                .inventory
                .modules
                .keys()
                .filter(|id| !late.contains(*id) && !hub.read(|r| r.knows_module(id)))
                .cloned()
                .collect();
            for id in new {
                waiting.remove(&id);
                if let Some(m) = sup.learn_from(&reg, &id) {
                    if let Some(m) = m {
                        side.manifest(m);
                    }
                    late.insert(id);
                }
            }
            note_needs_hub_names(&reg);
            // Added but not in the registry yet (a lab folder whose manifest isn't written):
            // kept, so a change of only files inside it is read again.
            waiting.retain(|id| !reg.inventory.modules.contains_key(id));
            if fresh.put(crate::modules_page::list(&reg)) {
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

/// The tray names a module whose manifest asks for a newer hub by its head's name (#1096): the
/// panel feed gets no manifest for it, so it would otherwise say the id.
fn note_needs_hub_names(reg: &hideout_registry::Registry) {
    for id in reg.inventory.modules.keys() {
        if let Some(h) = reg.manifest_head(id) {
            hideout_shell::marks::note_name(id, &h.name);
        }
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

    /// #1076, window-free, on a normal hub's real files (`inventory.json`, `hideout.toml`,
    /// `modules\<id>\<version>\module.toml`, installed by the registry): the Modules page lists
    /// what is installed; a module installed after the start is learnt by the router and the
    /// supervisor, which tries to start it (its exe is missing, so no process starts: its
    /// state fact comes); an unreadable `hideout.toml` keeps the page's list instead of
    /// emptying it. Start event: each write; stop event: the feed's wake or the state fact;
    /// 10 s is the fallback only.
    #[cfg(windows)]
    #[test]
    fn a_normal_hub_lists_and_starts_a_module_installed_after_the_start() {
        use hideout_shell::hideout_panels::PanelFeed;
        let base = std::env::temp_dir().join(format!("hideout-rs-late-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        let install = |id: &str, name: &str, on: bool| {
            let src = base.join("src").join(id);
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(
                src.join("module.toml"),
                format!(
                    "id = \"{id}\"\nname = \"{name}\"\nversion = \"0.1.0\"\nkind = \"service\"\n\
                     family = \"third-party\"\nhub = \"1.0\"\nvisibility = \"private\"\n\
                     run = [\"missing.exe\"]\n"
                ),
            )
            .unwrap();
            let mut reg = hideout_registry::Registry::open(&home, true).unwrap();
            reg.install(&src, "2026-11-10T00:00:00Z").unwrap();
            if on {
                reg.set_enabled(id, true).unwrap();
            }
        };
        install("example.side.art", "Art", true);
        assert!(home
            .join("modules")
            .join("example.side.art")
            .join("0.1.0")
            .join("module.toml")
            .is_file());
        let names = |l: &[hideout_shell::modules_list::Installed]| -> Vec<String> {
            l.iter().map(|m| m.name.clone()).collect()
        };
        assert_eq!(names(&crate::modules_page::installed(&home)), ["Art"]);

        let mut router = Router::new(Settings::default(), approver());
        router.load_registry(&hideout_registry::Registry::open_from_env(&home).unwrap());
        let _ = router.watch_facts(vec![String::new()]);
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
        watch_inventory(&home, side, hub.clone(), sup.clone(), fresh.clone());
        let late = "example.side.beat";
        assert!(!sup.knows(late) && !hub.read(|r| r.knows_module(late)));

        // The page's next list that `want` accepts (each list comes with a wake).
        let mut list_where = |want: &dyn Fn(&[hideout_shell::modules_list::Installed]) -> bool| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < until {
                let _ = wakes.recv_timeout(std::time::Duration::from_secs(1));
                let _ = feed.drain();
                if fresh.take().is_some_and(|l| want(&l)) {
                    return true;
                }
            }
            false
        };
        let state = |hub: &Hub| hub.read(|r| r.fact(&format!("module.{late}.state")).cloned());
        // Installed while the hub runs, as an installer does it: `inventory.json` first (the
        // watch reads it as off), then turned on in a second write to `hideout.toml`.
        install(late, "Beat", false);
        assert!(
            list_where(&|l| names(l) == ["Art", "Beat"]),
            "the page never got Beat"
        );
        assert!(
            sup.knows(late),
            "the supervisor didn't learn the late module"
        );
        assert!(
            hub.read(|r| r.knows_module(late)),
            "the router didn't learn it"
        );
        assert_eq!(state(&hub).as_ref().and_then(|v| v.as_str()), Some("off"));
        let mut reg = hideout_registry::Registry::open(&home, true).unwrap();
        reg.set_enabled(late, true).unwrap();
        assert!(
            list_where(&|l| l.iter().any(|m| m.id == late && m.enabled)),
            "the page never read Beat as on"
        );
        let s = state(&hub);
        assert!(
            s.as_ref().is_some_and(|s| s != "off"),
            "no start was tried after it was turned on: state {s:?}"
        );

        // An unreadable hideout.toml mid-run: the watch wakes (a new file), the page keeps Art
        // and Beat. Before #1076 it got an empty list.
        std::fs::write(home.join("hideout.toml.tmp"), "[modules\nbroken").unwrap();
        std::fs::rename(home.join("hideout.toml.tmp"), home.join("hideout.toml")).unwrap();
        std::fs::write(home.join("nudge.txt"), "x").unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            let _ = wakes.recv_timeout(std::time::Duration::from_millis(200));
            if let Some(l) = fresh.take() {
                panic!(
                    "the page got {:?} from an unreadable hideout.toml",
                    names(&l)
                );
            }
        }
        drop(sup);
        let _ = std::fs::remove_dir_all(&base);
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
