// SPDX-License-Identifier: MIT
//! hideout.exe, the Hideout hub (contract §2). This first cut (H2.1) is the shell only: it starts
//! empty, with the Modules page and nothing else. The router (H2.3) starts before the shell; the
//! panel host (H2.4) fills it in later.
//!
//! Its files live under `HIDEOUT_CONFIG_DIR` when set (lab and tests), else `HIDEOUT_HOME`, else
//! `%LOCALAPPDATA%\Hideout`.
//!
//! `--lab` (or `HIDEOUT_LAB=1`) asks for lab mode: with a `HIDEOUT_CONFIG_DIR` outside the
//! user's hub folder, the modules in `<config>\lab-modules\` load (#1001, docs/hideout-lab.md),
//! and the lab control pipe is served (#1003). `hideout.exe --lab-do <verb> [args]` is its client.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod modules_page;
#[cfg(windows)]
mod panel_feed;
#[cfg(windows)]
mod router_start;

/// What runs beside the shell: kept alive for the life of the process.
#[cfg(windows)]
struct Started {
    /// The one-supervisor-per-folder lock; `None` when another hub holds it.
    _lock: Option<hideout_supervisor::win::Handle>,
    _router: Option<hideout_router::server::Hub>,
    hooks: Option<Box<dyn hideout_shell::TabHooks>>,
    /// The Modules page's fact reader (the router's cache).
    facts: Option<modules_page::Facts>,
    panels: Option<Box<dyn hideout_shell::hideout_panels::PanelFeed>>,
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
                facts: None,
                panels: None,
            };
        }
        Err(e) => {
            log::warn!("supervisor lock {lock_name}: {e}; starting anyway");
            None
        }
    };
    let (router, supervisor, feed) = router_start::start(&paths.root, &paths.instance_suffix());
    let hub = router.clone();
    let facts: modules_page::Facts = Box::new(move |k| hub.read(|r| r.fact(k).cloned()));
    Started {
        _lock: lock,
        facts: Some(facts),
        _router: Some(router),
        hooks: Some(Box::new(router_start::Hooks(supervisor))),
        panels: Some(Box::new(feed)),
    }
}

#[cfg(windows)]
fn main() {
    use hideout_shell::paths::HubPaths;
    if std::env::args().skip(1).any(|a| a == "--lab") {
        // Before any thread starts; the registry checks the folder (lab::lab_mode).
        std::env::set_var(hideout_registry::lab::LAB_ENV, "1");
    }
    let Some(paths) = HubPaths::from_env() else {
        eprintln!("hideout: neither HIDEOUT_CONFIG_DIR nor LOCALAPPDATA is set");
        std::process::exit(2);
    };
    if let Some(code) = capture_client(&paths, std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    if let Some(code) = lab_do_client(&paths, std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    // Lab control only where the registry allows lab mode (never on the user's folder).
    hideout_shell::lab_control::allow(hideout_registry::lab::lab_mode_from_env(&paths.root).on());
    let mut started = start(&paths);
    let pages = modules_page::pages(modules_page::installed(&paths.root), started.facts.take());
    let opts = hideout_shell::ShellOptions {
        paths,
        pages,
        version: env!("CARGO_PKG_VERSION"),
        // Module panels from the router (§5, #991): pages, Keys and Settings.
        panels: started.panels.take(),
        // The supervisor's Start again and its ladder.
        hooks: started.hooks.take(),
        // `--tray` (sign-in): saved pop-outs wait for the first open of Hideout (§2.5).
        start_in_tray: std::env::args().any(|a| a == "--tray"),
    };
    if let Err(e) = hideout_shell::run(opts) {
        log::error!("hub stopped: {e}");
        eprintln!("hideout: {e}");
        std::process::exit(1);
    }
    drop(started);
}

/// `hideout.exe --capture <out.png>` (#1002): asks the lab hub on the same folder for a PNG of
/// its window (and `<out>-flyout.png` of an open flyout), prints its answer and exits; it opens
/// no window and starts nothing. `None` for any other command line: the hub starts as usual.
/// Exit 0 when every window was written, 1 otherwise.
#[cfg(windows)]
fn capture_client(
    paths: &hideout_shell::paths::HubPaths,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    use hideout_shell::capture;
    if args.next()? != "--capture" {
        return None;
    }
    let Some(out) = args.next().map(std::path::PathBuf::from) else {
        eprintln!("hideout: --capture needs an output path (.png)");
        return Some(2);
    };
    let out = std::path::absolute(&out).unwrap_or(out);
    let pipe = capture::pipe_name(paths);
    match capture::request(&pipe, &out) {
        Ok(reply) => {
            print!("{reply}");
            Some(if capture::answer_ok(&reply) { 0 } else { 1 })
        }
        Err(e) => {
            println!(
                "error no lab hub with capture on for {} ({pipe}: {e}); start it with HIDEOUT_CONFIG_DIR and {}=1",
                paths.root.display(),
                capture::ENV
            );
            Some(1)
        }
    }
}

/// `hideout.exe --lab-do <verb> [args]` (#1003): sends one lab control request to the lab hub on
/// the same folder, prints its JSON answer and exits; it opens no window and starts nothing.
/// `None` for any other command line. Exit 0 when the answer says ok, 1 otherwise.
#[cfg(windows)]
fn lab_do_client(
    paths: &hideout_shell::paths::HubPaths,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    use hideout_shell::lab_control;
    if args.next()? != "--lab-do" {
        return None;
    }
    let line: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
    if line.is_empty() {
        eprintln!("hideout: --lab-do needs a verb; {}", lab_control::HELP);
        return Some(2);
    }
    let pipe = lab_control::pipe_name(paths);
    match lab_control::request(&pipe, &line.join(" ")) {
        Ok(reply) => {
            print!("{reply}");
            Some(if lab_control::answer_ok(&reply) { 0 } else { 1 })
        }
        Err(e) => {
            let msg = format!(
                "no lab hub with lab control for {} ({pipe}: {e}); start it with HIDEOUT_CONFIG_DIR and HIDEOUT_LAB=1",
                paths.root.display()
            );
            println!("{}", serde_json::json!({"ok": false, "error": msg}));
            Some(1)
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("hideout: the hub runs on Windows only");
    std::process::exit(2);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// `--capture` never starts a hub: with no lab hub it answers an error and exits 1; any
    /// other command line falls through to the hub.
    #[test]
    fn capture_without_a_lab_hub_fails_fast() {
        let dir = std::env::temp_dir().join(format!("hideout-main-cap-{}", std::process::id()));
        let root = dir.as_os_str().to_owned();
        let paths = hideout_shell::paths::HubPaths::resolve(|k| {
            (k == "HIDEOUT_CONFIG_DIR").then(|| root.clone())
        })
        .unwrap();
        let args = |a: &[&str]| a.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let out = dir.join("x.png");
        let cap = args(&["--capture", out.to_str().unwrap()]);
        assert_eq!(capture_client(&paths, cap.into_iter()), Some(1));
        assert_eq!(
            capture_client(&paths, args(&["--capture"]).into_iter()),
            Some(2)
        );
        assert_eq!(capture_client(&paths, args(&[]).into_iter()), None);
        assert_eq!(capture_client(&paths, args(&["--other"]).into_iter()), None);
        // `--lab-do` the same: an error and exit 1 with no lab hub, 2 without a verb.
        let lab = args(&["--lab-do", "state"]);
        assert_eq!(lab_do_client(&paths, lab.into_iter()), Some(1));
        assert_eq!(
            lab_do_client(&paths, args(&["--lab-do"]).into_iter()),
            Some(2)
        );
        assert_eq!(
            lab_do_client(&paths, args(&["--capture"]).into_iter()),
            None
        );
        assert!(!dir.exists(), "the client made the hub folder");
    }

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
