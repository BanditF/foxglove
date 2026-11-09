// SPDX-License-Identifier: MIT
//! The Modules page, the hub's one page of its own (contract §2.1 job 2). With no registry yet
//! (H2.2) it shows the empty state. Generic only: it names no module (§2.2).
#![cfg_attr(not(windows), allow(dead_code))]

/// The page's id, title and empty-state text, kept apart from egui so they test window-free.
pub const ID: &str = "hub.modules";
pub const TITLE: &str = "Modules";
pub const EMPTY_HEADING: &str = "No modules yet";
pub const EMPTY_BODY: &str =
    "Modules add pages, tray tiles, status items and commands to Hideout. \
Installed modules will be listed here, with their state and version.";

#[cfg(windows)]
pub struct ModulesPage;

#[cfg(windows)]
impl hideout_shell::Page for ModulesPage {
    fn id(&self) -> &str {
        ID
    }

    fn title(&self) -> &str {
        TITLE
    }

    fn ui(&mut self, ui: &mut hideout_shell::egui::Ui, t: &hideout_shell::theme::Tokens) {
        use hideout_shell::egui::{Color32, RichText};
        let col = |c: hideout_shell::theme::Rgb| Color32::from_rgb(c.0, c.1, c.2);
        ui.label(RichText::new(TITLE).size(24.0).strong().color(col(t.text)));
        ui.add_space(20.0);
        ui.label(RichText::new(EMPTY_HEADING).size(17.0).color(col(t.text)));
        ui.add_space(4.0);
        ui.label(RichText::new(EMPTY_BODY).color(col(t.text_muted)));
    }
}

/// The hub's pages, in sidebar order: only Modules in H2.1.
#[cfg(windows)]
pub fn pages() -> Vec<Box<dyn hideout_shell::Page>> {
    vec![Box::new(ModulesPage)]
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_hub_starts_with_only_the_modules_page() {
        let pages = pages();
        assert_eq!(pages.len(), 1);
        assert_eq!((pages[0].id(), pages[0].title()), (ID, TITLE));
        assert_eq!(
            hideout_shell::window_title(pages[0].title()),
            "Hideout \u{b7} Modules"
        );
    }
}
