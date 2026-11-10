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

pub mod flyout;
pub mod icon;
pub mod logfile;
pub mod marks;
pub mod palette;
pub mod paths;
pub mod placement;
pub mod prefs;
pub mod status;
pub mod tabs;
pub mod theme;

#[cfg(windows)]
pub mod notify;
#[cfg(windows)]
mod platform;
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
pub use shell::{open_module, open_module_log, run, Page, ShellOptions, TabHooks};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_use_the_contract_form() {
        assert_eq!(window_title("Modules"), "Hideout \u{b7} Modules");
        assert_eq!(window_title(""), "Hideout");
    }
}
