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

#[cfg(windows)]
mod adopt;
#[cfg(windows)]
mod catalog_fetch;
#[cfg(windows)]
mod control;
#[cfg(windows)]
mod dev_settings;
#[cfg(windows)]
mod discover;
mod find_installed;
#[cfg(windows)]
mod grant_queue;
mod handover;
#[cfg(windows)]
mod https;
#[cfg(windows)]
mod inventory_watch;
#[cfg(windows)]
mod keyboard_actions;
#[cfg(windows)]
mod lab_verbs;
mod modules_page;
#[cfg(windows)]
mod panel_feed;
#[cfg(windows)]
mod permission_actions;
#[cfg(windows)]
mod rebuild_start;
// Off Windows only the state machine exists, and only its tests use it.
#[cfg_attr(not(windows), allow(dead_code))]
mod rebuild_watch;
#[cfg(windows)]
mod router_start;
mod settings_page;
#[cfg(windows)]
mod signin_fact;
#[cfg(windows)]
mod signin_signal;
#[cfg(windows)]
mod signin_watch;

/// What runs beside the shell: kept alive for the life of the process.
#[cfg(windows)]
struct Started {
    /// The one-supervisor-per-folder lock; `None` when another hub holds it.
    _lock: Option<hideout_supervisor::win::Handle>,
    _router: Option<hideout_router::server::Hub>,
    hooks: Option<Box<dyn hideout_shell::TabHooks>>,
    /// The Modules page's fact reader (the router's cache).
    facts: Option<modules_page::Facts>,
    /// The Modules page's buttons (the supervisor).
    actions: Option<Box<dyn modules_page::Actions>>,
    /// The tray menu's Start normally (the supervisor).
    start_normally: Option<hideout_shell::StartNormally>,
    panels: Option<Box<dyn hideout_shell::hideout_panels::PanelFeed>>,
    /// This hub runs the folder's supervisor: its clean end is marked in `run\hub-run.json`.
    owner: bool,
    /// Safe mode is on because the hub closed 3 times in 5 minutes (the banner's line).
    safe_by_closes: bool,
    /// Finding the module that breaks Hideout (#1033): the search's driver.
    finder: Option<hideout_supervisor::finder::Finder>,
    /// The Modules page's list as the inventory watch reads it (#1054).
    fresh: Option<modules_page::Fresh>,
}

/// How the last run ended (§7.10, #1033), in the hub that holds the supervisor lock, before the
/// router reads `hideout.toml`: the third unclean close in 5 minutes sets `safe_mode` there and
/// logs why. Answers whether safe mode is on because of the closes. Safe mode off at a start
/// forgets that cause, so a flag set by hand later reads neutral.
#[cfg(all(windows, test))]
fn closes_at_start(home: &std::path::Path, now_ms: u64) -> bool {
    closes_and_exit(home, now_ms).0
}

/// [`closes_at_start`], and how the last run ended for a search in progress (#1033): clean,
/// unclean, or unreadable (no readable file, or Windows started after that run did). While a
/// search runs the 3-in-5-minutes rule is off: one close counts for its round, and safe mode
/// is on already.
#[cfg(windows)]
fn closes_and_exit(
    home: &std::path::Path,
    now_ms: u64,
) -> (bool, hideout_supervisor::bisect::Exit) {
    use hideout_supervisor::{bisect, closes, finder};
    let searching = bisect::Search::load(home).is_some_and(|s| s.active());
    let readable = closes::readable(home);
    let cause_before = closes::safe_mode_by_closes(home);
    let (s, saved) = closes::on_start(home, std::process::id(), now_ms);
    if searching && s.enter_safe_mode && !cause_before {
        // Off during a search: the closes don't become safe mode's cause either.
        if let Err(e) = closes::clear_safe_mode_cause(home) {
            log::warn!("run\\hub-run.json not saved: {e}");
        }
    }
    let exit = finder::exit_kind(readable, &s, finder::boot_ms(now_ms));
    (closes_after(home, s, saved, searching), exit)
}

/// The 3-in-5 rule on what the start found; answers whether the closes turned safe mode on.
#[cfg(windows)]
fn closes_after(
    home: &std::path::Path,
    s: hideout_supervisor::closes::Start,
    saved: std::io::Result<()>,
    searching: bool,
) -> bool {
    use hideout_supervisor::closes;
    if let Err(e) = saved {
        log::warn!("run\\hub-run.json not saved: {e}; this run's end can't be told apart");
    }
    if s.unclean && !s.enter_safe_mode {
        log::info!(
            "the last run ended without a clean exit ({} in 5 minutes; safe mode at {})",
            s.recent,
            closes::KEEP
        );
    }
    let mut reg = match hideout_registry::Registry::open_from_env(home) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("registry not read ({e:?}); safe mode not checked");
            return false;
        }
    };
    if s.enter_safe_mode && searching {
        log::info!(
            "Hideout closed {} times in 5 minutes during a search for the module that breaks it; the search counts it",
            s.recent
        );
    } else if s.enter_safe_mode {
        log::warn!(
            "safe mode on: Hideout closed {} times in 5 minutes without a clean exit; no module starts until Start normally",
            s.recent
        );
        if let Err(e) = reg.set_safe_mode(true) {
            log::warn!("safe mode not saved in hideout.toml: {e:?}");
        }
        return true;
    }
    if !reg.hub.safe_mode() {
        if let Err(e) = closes::clear_safe_mode_cause(home) {
            log::warn!("run\\hub-run.json not saved: {e}");
        }
        return false;
    }
    closes::safe_mode_by_closes(home)
}

/// The search for the shell's bar and tray items (#1033).
#[cfg(windows)]
struct FinderUi(hideout_supervisor::finder::Finder);

#[cfg(windows)]
impl hideout_shell::Finder for FinderUi {
    fn view(&self) -> Option<hideout_supervisor::bisect::View> {
        self.0.view()
    }
    fn act(&self, act: hideout_supervisor::bisect::Act) {
        self.0.act(act)
    }
    fn diagnostics(&self) -> String {
        self.0.diagnostics()
    }
    fn set_waker(&self, wake: Box<dyn Fn() + Send + Sync>) {
        self.0.on_change(Box::new(move |_| wake()));
    }
}

/// The tray follows the search; its one notification comes when a result is ready and the
/// window is closed ("Found the module" / "<Module> seems to make Hideout close." / Open
/// Hideout). At start, "closed" is a start in the tray.
#[cfg(windows)]
fn watch_search(f: &hideout_supervisor::finder::Finder, start_in_tray: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    f.on_change(Box::new(|f| hideout_shell::marks::note_search(f.tray())));
    let first = AtomicBool::new(true);
    let last: Mutex<Option<(String, String)>> = Mutex::new(None);
    f.on_change(Box::new(move |f| {
        let note = f.notification();
        let was = std::mem::replace(&mut *last.lock().unwrap(), note.clone());
        let hidden = if first.swap(false, Ordering::AcqRel) {
            start_in_tray
        } else {
            !hideout_shell::hub_on_screen()
        };
        let Some((title, body)) = note.filter(|n| was.as_ref() != Some(n)) else {
            return;
        };
        if !hidden {
            return;
        }
        log::info!("finder: notify: {title} / {body}");
        hideout_shell::notify::toast(
            title,
            body,
            vec![(
                hideout_supervisor::bisect::words::OPEN_HIDEOUT.into(),
                "open-hub".into(),
            )],
            "open-hub".into(),
            std::sync::Arc::new(|arg: &str| {
                if arg == "open-hub" {
                    hideout_shell::show_hub();
                }
            }),
        );
    }));
}

#[cfg(windows)]
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
                actions: None,
                start_normally: None,
                panels: None,
                owner: false,
                safe_by_closes: false,
                finder: None,
                fresh: None,
            };
        }
        Err(e) => {
            log::warn!("supervisor lock {lock_name}: {e}; starting anyway");
            None
        }
    };
    let (safe_by_closes, exit) = closes_and_exit(&paths.root, unix_ms());
    let fresh = modules_page::Fresh::default();
    let (router, supervisor, feed) =
        router_start::start(&paths.root, &paths.instance_suffix(), fresh.clone());
    // A search in progress picks up after the supervisor started nothing (safe mode, #1033).
    let finder = hideout_supervisor::finder::Finder::start(
        &paths.root,
        supervisor.clone(),
        exit,
        env!("CARGO_PKG_VERSION"),
    );
    // The supervisor's update and quarantine verbs for lab control (#1096): a lab hub only.
    if hideout_shell::lab_control::enabled(paths) {
        lab_verbs::register(supervisor.clone());
    }
    if let Err(e) = control::serve(
        &control::pipe_name(paths),
        paths.root.clone(),
        router.clone(),
        supervisor.clone(),
        grant_queue::shared().clone(),
        // A lab hub (own folder, lab mode allowed) applies a CLI grant at once, so lab runs
        // stay headless; the user's hub always queues it (#1183).
        hideout_shell::lab_control::enabled(paths),
    ) {
        log::warn!("control pipe not served: {e}");
    }
    let hub = router.clone();
    let facts: modules_page::Facts = Box::new(move |k| hub.read(|r| r.fact(k).cloned()));
    let sup = supervisor.clone();
    let start_normally: hideout_shell::StartNormally = std::sync::Arc::new(move || {
        sup.start_normally();
    });
    Started {
        _lock: lock,
        facts: Some(facts),
        actions: Some(Box::new(supervisor.clone())),
        start_normally: Some(start_normally),
        _router: Some(router),
        hooks: Some(Box::new(router_start::Hooks(supervisor))),
        panels: Some(Box::new(feed)),
        owner: true,
        safe_by_closes,
        finder: Some(finder),
        fresh: Some(fresh),
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
    if let Some(code) = control::client(&paths, std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    if let Some(code) = adopt_client(&paths, std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    if let Some(code) = autostart_client(&paths, std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    // Lab control only where the registry allows lab mode (never on the user's folder).
    hideout_shell::lab_control::allow(hideout_registry::lab::lab_mode_from_env(&paths.root).on());
    // A sign-in start opens the hand-over event before the router's first fact (slice 3).
    #[cfg(windows)]
    signin_signal::install(std::env::args().any(|a| a == "--tray"), paths.overridden);
    let mut started = start(&paths);
    if started.owner {
        // Quit, the session's end and Ctrl+C mark the run clean (#1033).
        let home = paths.root.clone();
        hideout_shell::exit_watch::install(Box::new(move || {
            match hideout_supervisor::closes::mark_clean(&home) {
                Ok(()) => log::info!("clean exit marked"),
                Err(e) => log::warn!("clean exit not marked in run\\hub-run.json: {e}"),
            }
        }));
    }
    let start_in_tray = std::env::args().any(|a| a == "--tray");
    if let Some(f) = &started.finder {
        watch_search(f, start_in_tray);
    }
    let mut pages = modules_page::pages(
        modules_page::installed(&paths.root),
        started.facts.take(),
        started.actions.take(),
        started.safe_by_closes,
        started.finder.clone(),
        started.fresh.take(),
        // The keyboard grant's dialog and row: only the hub that holds the router.
        started
            ._router
            .clone()
            .map(|h| keyboard_actions::GrantUi::new(&paths.root, h)),
        // The module permission dialog, header lines and sections (#1178): same condition.
        started._router.clone().map(|h| {
            permission_actions::PermUi::new(&paths.root, h.clone())
                .with_queue(grant_queue::shared().clone())
                .with_standalone(handover::foxglove(&paths.root, keyboard_actions::MODULE, h))
        }),
        // The Found card and Add an app…: the same hub only (adding goes through its dialog).
        started
            ._router
            .as_ref()
            .map(|_| discover::DiscoverUi::new(paths.root.clone())),
    );
    // Settings: Privacy with Forget seen apps, and About (#991), at the sidebar's bottom.
    let mut settings =
        settings_page::SettingsPage::new(paths.root.clone(), env!("CARGO_PKG_VERSION"));
    // Start with Windows (H5.2): OFF until the user's click in its dialog; nothing here turns it
    // on, and a hub that is not the installed one (or a test run) cannot write it.
    if let Some(mut sw) = startup_switch(&paths) {
        // What the status and the dialog say about apps comes from the router: apps running now,
        // and apps told that Hideout starts at sign-in (they remember it).
        if let Some(hub) = started._router.clone() {
            sw = sw.with_apps_source(Box::new(move || {
                let (running, told) = hub.read(|r| r.apps_at_sign_in());
                hideout_shell::startup_switch::AppsNow { running, told }
            }));
        }
        // A sign-in start says what the switch is, in the log (reads the shell; writes nothing).
        if start_in_tray {
            log::info!("{}", sw.sign_in_line());
        }
        // The startup pass's one repair: --tray on an enabled Hideout.lnk of this exe.
        match sw.repair_tray_arg() {
            Ok(hideout_shell::autostart::Outcome::Changed) => {
                log::info!("Start with Windows: added --tray to the enabled Hideout.lnk")
            }
            Ok(_) => {}
            Err(e) => log::warn!("start with Windows: repair not done: {e}"),
        }
        // The router hears whether Hideout starts at sign-in and tells running modules
        // (`module.<id>.starts_at_sign_in`, read-only): only the hub that holds the router.
        // The fact also follows a change made outside this page (Windows Settings > Apps >
        // Startup, a deleted shortcut): a change notification, not a timer; read only.
        if let Some(hub) = started._router.clone() {
            watch_sign_in_fact(&paths, hub);
        }
        let sw: Box<dyn hideout_shell::startup_switch::StartupControl> =
            match started._router.clone() {
                Some(hub) => Box::new(signin_fact::Announcing::new(
                    sw,
                    Box::new(move |on| hub.act(|r| r.set_starts_at_sign_in(on))),
                )),
                None => Box::new(sw),
            };
        settings = settings.with_startup(sw);
    }
    // Developer (R5): dev folders, Add a folder..., the restart switch; the hub that holds the
    // router only, like Add an app....
    if started._router.is_some() {
        settings = settings.with_dev(Box::new(dev_settings::RegistryDev::new(
            paths.root.clone(),
            Box::new(hideout_shell::pick_folder),
        )));
    }
    pages.push(Box::new(settings));
    let opts = hideout_shell::ShellOptions {
        paths,
        pages,
        version: env!("CARGO_PKG_VERSION"),
        // Module panels from the router (§5, #991): pages, Keys and Settings.
        panels: started.panels.take(),
        // The supervisor's Start again and its ladder.
        hooks: started.hooks.take(),
        // `--tray` (sign-in): saved pop-outs wait for the first open of Hideout (§2.5).
        start_in_tray,
        // Safe mode's Start normally in the tray menu (§7.10).
        start_normally: started.start_normally.take(),
        // Finding the module that breaks Hideout: the bar and the tray's items (#1033).
        finder: started
            .finder
            .clone()
            .map(|f| std::sync::Arc::new(FinderUi(f)) as hideout_shell::FinderHost),
    };
    if let Err(e) = hideout_shell::run(opts) {
        log::error!("hub stopped: {e}");
        eprintln!("hideout: {e}");
        std::process::exit(1);
    }
    // The shell returned: an end on purpose (a no-op in a second hub, which installed nothing).
    hideout_shell::exit_watch::clean_exit();
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

/// Re-reads Start with Windows when the Startup folder or the StartupApproved key changes, and
/// tells the router when "Hideout starts at sign-in" changed. Nothing is written.
#[cfg(windows)]
fn watch_sign_in_fact(paths: &hideout_shell::paths::HubPaths, hub: hideout_router::server::Hub) {
    let Ok(folder) = hideout_shell::startup_win::RealStartup::new() else {
        return;
    };
    let dir = folder.dir().to_path_buf();
    let own = paths.clone();
    let mut refresh = signin_watch::Refresh::new(
        move || {
            use hideout_shell::startup_switch::StartupControl;
            // A fresh switch: its own snapshot cache is two seconds old at worst otherwise.
            startup_switch(&own).map(|mut s| s.row().turn_off)
        },
        move |on| hub.act(|r| r.set_starts_at_sign_in(on)),
        None,
    );
    let wake = Box::new(move || {
        refresh.wake();
    });
    if let Err(e) = signin_watch::start(&dir, hideout_shell::startup_win::APPROVED_PARENT, wake) {
        log::warn!("sign-in watch not started: {e}");
    }
}

#[cfg(not(windows))]
fn watch_sign_in_fact(_: &hideout_shell::paths::HubPaths, _: hideout_router::server::Hub) {}

/// The Start with Windows switch over the real Startup folder, for this exe.
#[cfg(windows)]
fn startup_switch(
    paths: &hideout_shell::paths::HubPaths,
) -> Option<
    hideout_shell::startup_switch::Switch<
        hideout_shell::startup_win::RealStartup,
        hideout_shell::startup_win::RealApproval,
    >,
> {
    let exe = std::env::current_exe().ok()?;
    match hideout_shell::startup_win::real(exe, paths.overridden) {
        Ok(s) => Some(s),
        Err(e) => {
            log::info!("start with Windows: switch not shown: {e}");
            None
        }
    }
}

/// `hideout.exe --autostart-status` / `--autostart-remove`: the rollback without a window.
/// Status prints one of missing, on, off, foreign and writes nothing. Remove deletes
/// `Hideout.lnk` and its StartupApproved value only when the shortcut starts this exe, prints
/// one line and exits; it never turns anything on and never touches `Foxglove.lnk`. `None` for
/// any other command line. Exit 0 done, 1 failed, 2 refused (a test run).
#[cfg(windows)]
fn autostart_client(
    paths: &hideout_shell::paths::HubPaths,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    use hideout_shell::autostart::{self, Outcome, State};
    let which = args.next()?;
    let remove = match which.to_str()? {
        "--autostart-remove" => true,
        "--autostart-status" => false,
        _ => return None,
    };
    let exe = std::env::current_exe().ok()?;
    let ctx = autostart::Context {
        exe,
        overridden: paths.overridden,
    };
    let (mut folder, mut approval) = match (
        hideout_shell::startup_win::RealStartup::new(),
        hideout_shell::startup_win::RealApproval::new(),
    ) {
        (Ok(f), Ok(a)) => (f, a),
        (Err(e), _) | (_, Err(e)) => {
            println!("hideout: {e}");
            return Some(2);
        }
    };
    if !remove {
        let s = match autostart::state(&ctx, &folder, &approval) {
            State::Missing => "missing",
            State::On => "on",
            State::Off => "off",
            State::Foreign => "foreign",
        };
        println!("{s}");
        return Some(0);
    }
    match autostart::remove(&ctx, &mut folder, &mut approval) {
        Ok(Outcome::Changed) => {
            println!("removed the Hideout startup shortcut");
            Some(0)
        }
        Ok(Outcome::Unchanged) => {
            println!("nothing to remove (no Hideout startup shortcut for this exe)");
            Some(0)
        }
        Ok(Outcome::SkippedLabRun) => {
            println!("hideout: a test run writes nothing");
            Some(2)
        }
        Err(e) => {
            println!("hideout: {e}");
            Some(1)
        }
    }
}

/// `hideout.exe --adopt <folder>` (#1117): registers the adopted module whose `module.toml` is in
/// `<folder>` (see `adopt`), prints one line and exits; no window, nothing started. `None` for
/// any other command line. Exit 0 when registered, 1 when refused, 2 for a missing folder.
#[cfg(windows)]
fn adopt_client(
    paths: &hideout_shell::paths::HubPaths,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Option<i32> {
    if args.next()? != "--adopt" {
        return None;
    }
    let Some(folder) = args.next().map(std::path::PathBuf::from) else {
        eprintln!("hideout: --adopt needs the folder that holds the module's module.toml");
        return Some(2);
    };
    let folder = std::path::absolute(&folder).unwrap_or(folder);
    match adopt::run(&paths.root, &folder, &hideout_registry::catalogs::now_utc()) {
        Ok(line) => {
            println!("{line}");
            Some(0)
        }
        Err(e) => {
            println!("hideout: {e}");
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
        // A second hub on the same folder starts neither router nor supervisor, and doesn't
        // count as a close of the first (#1033).
        let run = hideout_supervisor::closes::HubRun::load(&paths.root);
        let second = start(&paths);
        assert!(second.hooks.is_none() && second._router.is_none() && !second.owner);
        assert_eq!(hideout_supervisor::closes::HubRun::load(&paths.root), run);
        assert!(started.owner && run.running.is_some());
        drop(started);
    }

    /// #1033, window-free: three starts after unclean closes within 5 minutes set `safe_mode`
    /// in hideout.toml with the closes as the cause; clean quits don't count; a start long
    /// after doesn't; a flag set by hand reads neutral.
    #[test]
    fn three_unclean_closes_in_five_minutes_start_in_safe_mode() {
        use hideout_supervisor::closes;
        let dir = std::env::temp_dir().join(format!("hideout-main-safe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let toml_safe = || {
            hideout_registry::Registry::open_from_env(&dir)
                .unwrap()
                .hub
                .safe_mode()
        };
        const MIN: u64 = 60_000;
        let t0 = 2_000_000_000_000;
        // Clean quits: a start, then Quit, five times in a minute.
        for k in 0..5 {
            assert!(!closes_at_start(&dir, t0 + k * 1000));
            closes::mark_clean(&dir).unwrap();
        }
        assert!(!toml_safe());
        // Killed three times: the starts at 2, 3 and 4 minutes find the last run's mark.
        assert!(!closes_at_start(&dir, t0 + MIN)); // after the last quit: clean
        assert!(!closes_at_start(&dir, t0 + 2 * MIN));
        assert!(!closes_at_start(&dir, t0 + 3 * MIN));
        assert!(!toml_safe());
        assert!(closes_at_start(&dir, t0 + 4 * MIN));
        assert!(toml_safe(), "the third unclean close didn't set safe_mode");
        // Closed cleanly in safe mode: the next start still names the closes.
        closes::mark_clean(&dir).unwrap();
        assert!(closes_at_start(&dir, t0 + 5 * MIN));
        // Start normally clears the flag (the supervisor does it), then Quit; the next start
        // forgets the cause.
        let mut reg = hideout_registry::Registry::open_from_env(&dir).unwrap();
        reg.set_safe_mode(false).unwrap();
        closes::clear_safe_mode_cause(&dir).unwrap();
        closes::mark_clean(&dir).unwrap();
        assert!(!closes_at_start(&dir, t0 + 11 * MIN));
        assert!(!toml_safe());
        // Spread out: unclean closes noticed at 12, 14 and 18 minutes are not 3 in 5 minutes,
        // so the third doesn't turn safe mode on.
        for m in [12, 14, 18] {
            assert!(!closes_at_start(&dir, t0 + m * MIN));
        }
        assert!(!toml_safe());
        // Set by hand: safe mode, without the closes as its cause.
        closes::mark_clean(&dir).unwrap();
        let mut reg = hideout_registry::Registry::open_from_env(&dir).unwrap();
        reg.set_safe_mode(true).unwrap();
        assert!(!closes_at_start(&dir, t0 + 30 * MIN));
        assert!(toml_safe());
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// #1033, window-free: with a search on disk, a start tells how the last run ended (no file
    /// yet: can't tell; Quit: clean; killed: unclean, a start of its own every time), the
    /// 3-in-5-minutes rule stays off, and the round the kill ended counts as closed.
    #[test]
    fn a_search_reads_each_exit_and_the_3_in_5_rule_is_off() {
        use hideout_supervisor::bisect::{entries, Candidate, Exit, Phase, Search};
        use hideout_supervisor::closes;
        let dir = std::env::temp_dir().join(format!("hideout-main-bisect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut reg = hideout_registry::Registry::open_from_env(&dir).unwrap();
        reg.set_safe_mode(true).unwrap();
        let c: Vec<Candidate> = ["Art", "Beat", "Chat"]
            .iter()
            .map(|n| Candidate {
                id: format!("m.{}", n.to_lowercase()),
                name: n.to_string(),
                needs: vec![],
            })
            .collect();
        Search::new(entries(&c)).unwrap().save(&dir).unwrap();
        const MIN: u64 = 60_000;
        // Ahead of Windows' start, so no run here "started before Windows did".
        let t0 = hideout_supervisor::finder::boot_ms(unix_ms()) + 10 * MIN;
        // No run\hub-run.json yet: the end can't be read.
        assert_eq!(closes_and_exit(&dir, t0).1, Exit::Unknown);
        closes::mark_clean(&dir).unwrap();
        assert_eq!(closes_and_exit(&dir, t0 + 1000).1, Exit::Clean);
        // Killed four times in 4 minutes: unclean every time, and no new safe-mode cause.
        for k in 1..=4 {
            let (by_closes, exit) = closes_and_exit(&dir, t0 + k * MIN);
            assert_eq!(exit, Exit::Unclean, "start {k}");
            assert!(!by_closes, "start {k}: the 3-in-5 rule ran during a search");
        }
        let toml_safe = hideout_registry::Registry::open_from_env(&dir)
            .unwrap()
            .hub
            .safe_mode();
        assert!(toml_safe, "safe mode stays on during the search");
        // What the finder does with it: round 1 closed.
        let mut s = Search::load(&dir).unwrap();
        s.after_exit(Exit::Unclean);
        assert_eq!(s.phase, Phase::Closed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
