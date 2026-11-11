// SPDX-License-Identifier: MIT
//! The About page (#991): the hub's version and its **Privacy** section, which says what the app
//! watch keeps (`hideout.appwatch`, contract §4.6) and lets the user forget it. Forget seen apps
//! clears `apps-seen.txt` and the running watch's list (`hideout_appwatch::forget_seen`), so the
//! tiles that wait for a seen app hide again until it runs. Generic only: it names no module.
#![cfg_attr(not(windows), allow(dead_code))]

/// The page's id and title, kept apart from egui so they test window-free.
pub const ID: &str = "hub.about";
pub const TITLE: &str = "About";

/// The privacy line (the #1040 designer review). Its examples (the review's "like OBS or ...")
/// are the names of installed modules that watch an app, so the hub names no product (§2.2):
/// two at most; with none the brackets are left out.
pub fn privacy_line(examples: &[String]) -> String {
    let like = match examples {
        [] => String::new(),
        [a] => format!(" (like {a})"),
        [a, b, ..] => format!(" (like {a} or {b})"),
    };
    format!(
        "Hideout notices when apps that your modules work with{like} start or quit, so their \
         tiles show up. It keeps a list of those apps on this PC only."
    )
}

/// The names of the installed modules that watch an app (`apps.watch`, `enriches`, an app
/// `when` or activation), sorted, for [`privacy_line`].
#[cfg(windows)]
pub fn watching_names(home: &std::path::Path) -> Vec<String> {
    let Ok(reg) = hideout_registry::Registry::open_from_env(home) else {
        return Vec::new();
    };
    let mut out: Vec<String> = reg
        .inventory
        .modules
        .keys()
        .filter_map(|id| reg.manifest(id))
        .filter(|m| !m.watched_apps().is_empty())
        .map(|m| m.name)
        .collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

/// Proposed (designer to confirm): the button that clears the list, and what shows after.
pub const FORGET: &str = "Forget seen apps";
pub const FORGOTTEN: &str = "Hideout forgot the apps it has seen.";

/// The version line: "Hideout 0.3.0".
pub fn version_line(version: &str) -> String {
    format!("{} {version}", hideout_shell::PRODUCT)
}

pub struct AboutPage {
    /// The hub's folder, where `apps-seen.txt` lives.
    home: std::path::PathBuf,
    version: &'static str,
    /// Installed modules that watch an app, for the privacy line's examples.
    examples: Vec<String>,
    /// Forget seen apps was pressed since the page was made (the confirmation line shows).
    forgotten: bool,
}

impl AboutPage {
    pub fn new(home: std::path::PathBuf, version: &'static str, examples: Vec<String>) -> Self {
        Self {
            home,
            version,
            examples,
            forgotten: false,
        }
    }

    /// Forget seen apps: the file goes, and a running watch forgets too (no wait).
    pub fn forget(&mut self) {
        hideout_appwatch::forget_seen(&self.home);
        self.forgotten = true;
    }
}

impl hideout_shell::Page for AboutPage {
    fn id(&self) -> &str {
        ID
    }

    fn title(&self) -> &str {
        TITLE
    }

    fn icon(&self) -> &str {
        "info"
    }

    fn ui(&mut self, ui: &mut hideout_shell::egui::Ui, t: &hideout_shell::theme::Tokens) {
        use hideout_shell::egui::{self, RichText};
        let c = |x: hideout_shell::theme::Rgb| egui::Color32::from_rgb(x.0, x.1, x.2);
        ui.label(
            RichText::new(version_line(self.version))
                .size(18.0)
                .strong()
                .color(c(t.text)),
        );
        ui.add_space(16.0);
        ui.label(
            RichText::new("Privacy")
                .size(15.0)
                .strong()
                .color(c(t.text)),
        );
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(RichText::new(privacy_line(&self.examples)).color(c(t.text))).wrap(),
        );
        ui.add_space(8.0);
        if self.forgotten {
            ui.label(RichText::new(FORGOTTEN).color(c(t.text_muted)));
        } else if ui.button(FORGET).clicked() {
            self.forget();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_says_the_privacy_line_and_the_version() {
        assert_eq!((ID, TITLE), ("hub.about", "About"));
        assert_eq!(version_line("0.3.0"), "Hideout 0.3.0");
        let two = privacy_line(&["Alpha".into(), "Beta".into(), "Gamma".into()]);
        assert_eq!(
            two,
            "Hideout notices when apps that your modules work with (like Alpha or Beta) start \
             or quit, so their tiles show up. It keeps a list of those apps on this PC only."
        );
        assert!(privacy_line(&["Alpha".into()]).contains("work with (like Alpha) start"));
        assert!(privacy_line(&[]).contains("work with start or quit"));
        assert!(!privacy_line(&[]).contains("  "));
    }

    #[test]
    fn forget_seen_apps_clears_the_list_file() {
        let home = std::env::temp_dir().join(format!("hideout-about-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let file = home.join(hideout_appwatch::SEEN_FILE);
        std::fs::write(&file, "example-app\n").unwrap();
        let mut page = AboutPage::new(home.clone(), "0.0.0", Vec::new());
        page.forget();
        assert!(!file.exists());
        assert!(page.forgotten);
        let _ = std::fs::remove_dir_all(&home);
    }
}
