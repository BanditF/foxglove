// SPDX-License-Identifier: MIT
//! hideout.exe, the Hideout hub (contract §2). This first cut (H2.1) is the shell only: it starts
//! empty, with the Modules page and nothing else. The router (H2.3) starts before the shell; the
//! panel host (H2.4) fills it in later.
//!
//! Its files live under `HIDEOUT_CONFIG_DIR` when set (lab and tests), else `HIDEOUT_HOME`, else
//! `%LOCALAPPDATA%\Hideout`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod modules_page;
#[cfg(windows)]
mod router_start;

/// What runs beside the shell: kept alive for the life of the process.
#[cfg(windows)]
struct Started {
    /// The one-supervisor-per-folder lock; `None` when another hub holds it.
    _lock: Option<hideout_supervisor::win::Handle>,
    _router: Option<hideout_router::server::Hub>,
    hooks: Option<Box<dyn hideout_shell::TabHooks>>,
}

/// Opens `logs\hideout.log` first, so the router's and the supervisor's start-up lines land in it
/// (#986); the shell finds the logger set and keeps it. Then the router and the supervisor, only
/// in the hub that holds the folder's supervisor lock: a second hub on the same folder only
/// brings the first one's window up, and must never start or take back modules.
#[cfg(windows)]
fn start(paths: &hideout_shell::paths::HubPaths) -> Started {
    use hideout_shell::logfile::FileLog;
    match FileLog::open(&paths.logs_dir(), log::LevelFilter::Info) {
        Ok(l) => {
            let _ = l.install();
        }
        Err(e) => eprintln!("hideout: log {}: {e}", paths.logs_dir().display()),
    }
    let lock_name = format!(
        r"Local\BanditF.Hideout.Supervisor{}",
        paths.instance_suffix()
    );
    let lock = match hideout_supervisor::win::claim(&lock_name) {
        Ok(Some(l)) => Some(l),
        Ok(None) => {
            log::info!(
                "another hub runs on {}; it keeps the router and the supervisor",
                paths.root.display()
            );
            return Started {
                _lock: None,
                _router: None,
                hooks: None,
            };
        }
        Err(e) => {
            log::warn!("supervisor lock {lock_name}: {e}; starting anyway");
            None
        }
    };
    let (router, supervisor) = router_start::start(&paths.root);
    Started {
        _lock: lock,
        _router: Some(router),
        hooks: Some(Box::new(router_start::Hooks(supervisor))),
    }
}

#[cfg(windows)]
fn main() {
    use hideout_shell::paths::HubPaths;
    let Some(paths) = HubPaths::from_env() else {
        eprintln!("hideout: neither HIDEOUT_CONFIG_DIR nor LOCALAPPDATA is set");
        std::process::exit(2);
    };
    let mut started = start(&paths);
    let opts = hideout_shell::ShellOptions {
        paths,
        pages: modules_page::pages(),
        version: env!("CARGO_PKG_VERSION"),
        // The router (H2.3) supplies the feed; until then the hub starts with no module panels.
        panels: None,
        // The supervisor's Start again; pop-out windows (H2.11) come later.
        hooks: started.hooks.take(),
    };
    if let Err(e) = hideout_shell::run(opts) {
        log::error!("hub stopped: {e}");
        eprintln!("hideout: {e}");
        std::process::exit(1);
    }
    drop(started);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("hideout: the hub runs on Windows only");
    std::process::exit(2);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The supervisor's first line reaches `hideout.log` (#986). Window-free: no shell, a temp
    /// `HIDEOUT_CONFIG_DIR`, and a user name of its own so the router pipe and lock are its own.
    #[test]
    fn the_first_supervisor_line_lands_in_the_log() {
        let dir = std::env::temp_dir().join(format!("hideout-main-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HIDEOUT_CONFIG_DIR", &dir);
        std::env::set_var("USERNAME", format!("hideouttest{}", std::process::id()));
        let paths = hideout_shell::paths::HubPaths::from_env().unwrap();
        let started = start(&paths);
        assert!(started.hooks.is_some(), "the supervisor didn't start");
        log::logger().flush();
        let text = std::fs::read_to_string(paths.logs_dir().join("hideout.log")).unwrap();
        assert!(
            text.contains("supervisor: starting; 0 instance(s) recorded"),
            "no supervisor line in hideout.log:\n{text}"
        );
        // A second hub on the same folder starts neither router nor supervisor.
        let second = start(&paths);
        assert!(second.hooks.is_none() && second._router.is_none());
        drop(started);
    }
}
