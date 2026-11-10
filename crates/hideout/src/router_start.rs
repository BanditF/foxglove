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
        Some(("open", id)) => hideout_shell::open_module(id),
        Some(("open-log", id)) => hideout_shell::open_module_log(home, id),
        _ => {}
    }
}

/// The buttons of a supervisor notification with what each hands back: Start again, Open
/// <Module> (the stopped one), Open log (the rollback one); anything else (OK) closes it.
pub fn toast_buttons(id: &str, actions: &[String]) -> Vec<(String, String)> {
    use hideout_supervisor::words;
    let open = actions.first().map(|a| a == words::START_AGAIN) == Some(true);
    actions
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let arg = if label == words::START_AGAIN {
                format!("start-again:{id}")
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
/// (`HubPaths::instance_suffix`): the pipes and the modules' `HIDEOUT_PIPE` carry it.
pub fn start(home: &Path, suffix: &str) -> (Hub, Supervisor, HubFeed) {
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
    sup.set_notifier(notifier(sup.clone(), home));
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

#[cfg(test)]
mod tests {
    use super::*;
    use hideout_supervisor::words;

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
