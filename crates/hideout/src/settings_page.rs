// SPDX-License-Identifier: MIT
//! The Settings page (#991, the #1087 design review): **Privacy**, with what the app watch
//! keeps (`hideout.appwatch`, contract §4.6) and **Forget seen apps**, then **About** with the
//! hub's version. Forget seen apps clears `apps-seen.txt` and the running watch's list
//! (`hideout_appwatch::forget_seen`), so the tiles that wait for a seen app hide again until it
//! runs. The words and drawing are `hideout_shell::settings_view`'s; this page owns the action.
//! Generic only: it names no module or app.
#![cfg_attr(not(windows), allow(dead_code))]

pub struct SettingsPage {
    /// The hub's folder, where `apps-seen.txt` lives.
    home: std::path::PathBuf,
    version: &'static str,
    /// Forget seen apps was pressed since the page was made (its line shows).
    forgotten: bool,
}

impl SettingsPage {
    pub fn new(home: std::path::PathBuf, version: &'static str) -> Self {
        Self {
            home,
            version,
            forgotten: false,
        }
    }

    /// Forget seen apps: the file goes, and a running watch forgets too (no wait).
    pub fn forget(&mut self) {
        hideout_appwatch::forget_seen(&self.home);
        self.forgotten = true;
    }
}

#[cfg(windows)]
impl hideout_shell::Page for SettingsPage {
    fn id(&self) -> &str {
        hideout_shell::settings_view::ID
    }

    fn title(&self) -> &str {
        hideout_shell::settings_view::TITLE
    }

    fn icon(&self) -> &str {
        hideout_shell::settings_view::ICON
    }

    /// At the bottom of the sidebar, after the modules' pages.
    fn at_bottom(&self) -> bool {
        true
    }

    fn ui(&mut self, ui: &mut hideout_shell::egui::Ui, t: &hideout_shell::theme::Tokens) {
        if hideout_shell::settings_view::settings_ui(ui, t, self.version, self.forgotten) {
            self.forget();
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use hideout_shell::Page;

    #[test]
    fn the_page_is_settings_at_the_bottom() {
        let p = SettingsPage::new(std::env::temp_dir(), "0.0.0");
        assert_eq!(
            (p.id(), p.title(), p.icon()),
            ("hub.settings", "Settings", "gear")
        );
        assert!(p.at_bottom());
        assert_eq!(
            hideout_shell::window_title(p.title()),
            "Hideout \u{b7} Settings"
        );
    }

    #[test]
    fn forget_seen_apps_clears_the_list_file() {
        let home = std::env::temp_dir().join(format!("hideout-settings-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let file = home.join(hideout_appwatch::SEEN_FILE);
        std::fs::write(&file, "example-app\n").unwrap();
        let mut page = SettingsPage::new(home.clone(), "0.0.0");
        page.forget();
        assert!(!file.exists());
        assert!(page.forgotten);
        let _ = std::fs::remove_dir_all(&home);
    }
}
