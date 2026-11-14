// SPDX-License-Identifier: MIT
//! The Hideout hub's shell (contract §2.1 job 2, and the tray of job 1): the window frame with
//! its sidebar, status strip and command palette; the tray icon and its fixed menu frame; theme
//! tokens that follow Windows; per-monitor placement; single instance; the AUMID; the log.
//!
//! Clean-room: written from `docs/hideout-contract.md`, the Hideout design doc and public crate
//! docs. It holds no module ids and no device code (§2.2): pages come from the caller.
//!
//! The pure parts (paths, preferences, placement, palette, status strip, theme tokens, the tray
//! glyph, instance names) build and test on every platform; the window, the tray and the Win32
//! calls are Windows-only.

pub mod drag;
pub mod flyout;
pub mod icon;
pub mod keyboard_grant;
#[cfg(windows)]
pub mod keyboard_grant_ui;
pub mod logfile;
pub mod marks;
pub mod palette;
pub mod paths;
pub mod permission_dialog;
#[cfg(windows)]
pub mod permission_dialog_ui;
pub mod placement;
pub mod popout;
pub mod prefs;
pub mod status;
pub mod tabs;
pub mod theme;

#[cfg(windows)]
pub mod bisect_bar;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod exit_watch;
#[cfg(windows)]
pub mod hotkeys;
#[cfg(windows)]
pub mod lab_control;
#[cfg(windows)]
pub mod modules_list;
#[cfg(windows)]
pub mod notify;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
pub mod settings_view;
#[cfg(windows)]
mod shell;
#[cfg(windows)]
pub mod strip;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod wake;

#[cfg(windows)]
pub use eframe::egui;
#[cfg(windows)]
pub use hideout_panels;
#[cfg(windows)]
pub use shell::{
    hub_on_screen, open_module, open_module_log, open_update_page, open_update_page_at, run,
    show_hub, Finder, FinderHost, Page, ShellOptions, StartNormally, TabHooks,
};

/// The hub's AppUserModelID: the hub window, its pop-outs and module windows group under it
/// (contract §2.5).
pub const AUMID: &str = "BanditF.Hideout";

/// The product name in titles, the tray tooltip and the log.
pub const PRODUCT: &str = "Hideout";

/// A window title in the contract's form, `Hideout · <page or window name>` (§2.5).
pub fn window_title(page: &str) -> String {
    if page.is_empty() {
        PRODUCT.to_string()
    } else {
        format!("{PRODUCT} \u{b7} {page}")
    }
}

/// The page **Update Hideout** opens for a release link from a catalog (#991, contract §7.8
/// v1.11): `url` when it is on the releases page's allow-list, else the releases page. Nothing
/// else from a catalog is ever opened.
pub fn update_page(url: &str) -> &str {
    if hideout_contract::release::link_allowed(url) {
        url
    } else {
        hideout_contract::release::RELEASES_URL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_use_the_contract_form() {
        assert_eq!(window_title("Modules"), "Hideout \u{b7} Modules");
        assert_eq!(window_title(""), "Hideout");
    }

    /// Update Hideout opens a release page only on the allow-list (#991).
    #[test]
    fn update_hideout_opens_only_allowed_release_pages() {
        let tag = "https://github.com/BanditF/foxglove/releases/tag/hideout-v0.4.0";
        assert_eq!(update_page(tag), tag);
        let releases = "https://github.com/BanditF/foxglove/releases";
        for bad in [
            "https://example.invalid/hideout-v0.4.0",
            "http://github.com/BanditF/foxglove/releases",
            "https://github.com@example.invalid/BanditF/foxglove/releases",
            "file:///C:/Windows/System32/calc.exe",
            "",
        ] {
            assert_eq!(update_page(bad), releases, "{bad:?}");
        }
    }
}
