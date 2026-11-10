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

#[cfg(windows)]
fn main() {
    use hideout_shell::paths::HubPaths;
    let Some(paths) = HubPaths::from_env() else {
        eprintln!("hideout: neither HIDEOUT_CONFIG_DIR nor LOCALAPPDATA is set");
        std::process::exit(2);
    };
    let _router = router_start::start(&paths.root);
    let opts = hideout_shell::ShellOptions {
        paths,
        pages: modules_page::pages(),
        version: env!("CARGO_PKG_VERSION"),
        // The router (H2.3) supplies the feed; until then the hub starts with no module panels.
        panels: None,
    };
    if let Err(e) = hideout_shell::run(opts) {
        log::error!("hub stopped: {e}");
        eprintln!("hideout: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("hideout: the hub runs on Windows only");
    std::process::exit(2);
}
