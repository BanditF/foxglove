// SPDX-License-Identifier: MIT
//! R5 of the 0-click rebuild plan: the words and the shapes of **Settings > Developer** (the dev
//! folders list, **Add a folder...**, **Remove**, the restart switch and the dev-off line). The
//! drawing is `settings_view`'s; the hub binary owns the [`DevControl`] that reads and writes the
//! registry. This crate holds no registry code: the control hands over plain [`DevView`]s.
//!
//! Fails closed by shape: [`DevClick::Add`] carries no path. The control opens the folder
//! picker itself and sends what it chose through the registry's own checks, so nothing in the
//! page can name a folder that skipped them.

// ---- The words (user-visible; UX review needed) --------------------------------------------

pub const HEADING: &str = "Developer";
pub const FOLDERS_HEADING: &str = "Dev folders";
pub const BODY: &str = "Apps you allowed from these folders keep what you allowed when you \
rebuild them, and Hideout restarts them with the new build.";
pub const EMPTY: &str = "No dev folders yet.";
pub const ADD: &str = "Add a folder\u{2026}";
pub const NO_APPS: &str = "No apps from here yet";
pub const RESTART_SWITCH: &str = "Restart apps after a rebuild";
pub const RESTART_SUB: &str = "When off, the new build starts the next time you start the app.";
pub const FOOTNOTE: &str = "Developer mode is on for this PC. Hideout as shipped keeps it off.";
pub const MODE_OFF: &str = "Developer mode is off on this PC, so dev folders do nothing.";

pub const ADDED: &str = "Added. Apps you allow from here keep what you allowed over rebuilds.";
pub const ALREADY: &str = "That folder is already on the list.";
pub const REMOVED: &str = "Removed.";
pub const FAILED: &str = "Hideout couldn't save that. Nothing changed.";

// The Remove confirm (DF1): one click to ask, one to remove. Cancel is the default.
pub const REMOVE_BUTTON: &str = "Remove\u{2026}";
pub const CONFIRM_TITLE: &str = "Remove this dev folder?";
pub const CONFIRM_CANCEL: &str = "Cancel";
pub const CONFIRM_OK: &str = "Remove folder";
pub const CONFIRM_NO_APPS: &str = "No apps from here. Nothing else changes.";

/// The button's accessible name: "Remove dev folder C:\Dev\app".
pub fn remove_name(path: &str) -> String {
    format!("Remove dev folder {path}")
}

/// The sub-line under a folder: "Foxglove, Vulpes" or [`NO_APPS`].
pub fn apps_line(apps: &[String]) -> String {
    if apps.is_empty() {
        NO_APPS.to_string()
    } else {
        apps.join(", ")
    }
}

/// The confirm's second line: the apps stay as they are (a running copy is not stopped) and
/// their next build asks again.
pub fn confirm_body(apps: &[String]) -> String {
    let (who, stays, its) = match apps {
        [] => return CONFIRM_NO_APPS.to_string(),
        [one] => (one.clone(), "stays", "Its"),
        [init @ .., last] if init.len() == 1 => {
            (format!("{} and {last}", init[0]), "stay", "Their")
        }
        [init @ .., last] => (format!("{} and {last}", init.join(", ")), "stay", "Their"),
    };
    let run = if apps.len() == 1 {
        "a copy that is running keeps running"
    } else {
        "copies that are running keep running"
    };
    format!(
        "{who} {stays} as {} now, and {run}. {its} next build asks for your OK again.",
        if apps.len() == 1 { "it is" } else { "they are" }
    )
}

/// The line after a removal: names the apps that will ask again.
pub fn removed_line(apps: &[String]) -> String {
    match apps {
        [] => REMOVED.to_string(),
        [one] => format!("Removed. The next rebuild of {one} asks for your OK again."),
        [init @ .., last] => format!(
            "Removed. The next rebuild of {} and {last} asks for your OK again.",
            init.join(", ")
        ),
    }
}

/// Why a folder is not a dev folder: shown when adding is refused and on a listed folder that
/// has stopped qualifying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Not a folder that exists.
    Missing,
    /// Not on this PC's own drive.
    NotLocal,
    /// A whole drive.
    Drive,
    /// The whole profile folder.
    Profile,
    /// AppData, Temp or Downloads.
    DropZone,
    /// A system or shared folder.
    System,
    /// Hideout could not tell who can change the folder.
    WhoUnknown,
    /// Other accounts can change files in it.
    FolderOpen,
    /// Other accounts can change a folder above it.
    ParentOpen,
}

impl Problem {
    pub fn line(self) -> &'static str {
        match self {
            Problem::Missing => "Hideout can't find that folder.",
            Problem::NotLocal => "A dev folder must be on this PC's own drive.",
            Problem::Drive => "A whole drive can't be a dev folder. Choose a folder inside it.",
            Problem::Profile => {
                "Your whole profile folder can't be a dev folder. Choose a project folder inside it."
            }
            Problem::DropZone => "Folders in AppData, Temp or Downloads can't be dev folders.",
            Problem::System => "System and shared folders can't be dev folders.",
            Problem::WhoUnknown => "Hideout couldn't check who can change that folder. Try again.",
            Problem::FolderOpen => {
                "Other accounts on this PC can change files in that folder. Choose one only you can change."
            }
            Problem::ParentOpen => {
                "Other accounts on this PC can change a folder above it. Choose another folder."
            }
        }
    }

    /// The same problem on a folder already listed: says what Hideout does about it, not what
    /// to choose.
    pub fn row_line(self) -> &'static str {
        match self {
            Problem::Missing => "Hideout can't find this folder, so it ignores it.",
            Problem::NotLocal => "This folder is not on this PC's own drive, so Hideout ignores it.",
            Problem::Drive | Problem::Profile | Problem::DropZone | Problem::System => {
                "This folder can't be a dev folder any more, so Hideout ignores it."
            }
            Problem::WhoUnknown => {
                "Hideout couldn't check who can change this folder, so it ignores it for now."
            }
            Problem::FolderOpen => {
                "Other accounts on this PC can change files in this folder, so Hideout ignores it."
            }
            Problem::ParentOpen => {
                "Other accounts on this PC can change a folder above this one, so Hideout ignores it."
            }
        }
    }
}

// ---- What the page draws -------------------------------------------------------------------

/// One listed folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevRow {
    pub path: String,
    /// Names of the apps allowed from it.
    pub apps: Vec<String>,
    /// Set when the folder no longer qualifies (its apps are not carried).
    pub problem: Option<Problem>,
}

/// The line after an action: a refusal in the problem colour, news in the plain one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DevView {
    /// Developer mode is on for this PC. Off with no folders listed: nothing is drawn. Off
    /// with folders listed: [`MODE_OFF`] and the paths, with nothing to act on.
    pub on: bool,
    pub folders: Vec<DevRow>,
    /// The restart switch.
    pub restart: bool,
    pub note: Option<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevClick {
    /// Add a folder...: no path; the control opens the picker.
    Add,
    /// Remove, by the listed path.
    Remove(String),
    /// The restart switch, to this state.
    Restart(bool),
}

/// What the Settings page holds for this section (object safe, like `StartupControl`).
pub trait DevControl {
    fn view(&mut self) -> DevView;
    fn click(&mut self, c: DevClick);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_is_exact() {
        assert_eq!(
            BODY,
            "Apps you allowed from these folders keep what you allowed when you rebuild them, and Hideout restarts them with the new build."
        );
        assert_eq!(ADD, "Add a folder\u{2026}");
        assert_eq!(remove_name(r"C:\Dev\app"), r"Remove dev folder C:\Dev\app");
        assert_eq!(apps_line(&[]), "No apps from here yet");
        assert_eq!(
            apps_line(&["Foxglove".into(), "Vulpes".into()]),
            "Foxglove, Vulpes"
        );
        assert_eq!(removed_line(&[]), "Removed.");
        assert_eq!(
            removed_line(&["Foxglove".into()]),
            "Removed. The next rebuild of Foxglove asks for your OK again."
        );
        assert_eq!(
            removed_line(&["A".into(), "B".into(), "C".into()]),
            "Removed. The next rebuild of A, B and C asks for your OK again."
        );
        assert_eq!(
            RESTART_SUB,
            "When off, the new build starts the next time you start the app."
        );
        assert_eq!(
            Problem::Drive.line(),
            "A whole drive can't be a dev folder. Choose a folder inside it."
        );
    }

    #[test]
    fn a_listed_row_says_what_hideout_does_not_what_to_choose() {
        for p in ALL {
            let l = p.row_line();
            assert!(l.ends_with("ignores it.") || l.ends_with("for now."), "{l}");
            assert_ne!(l, p.line());
            assert!(!l.contains("Choose"), "{l}");
        }
    }

    #[test]
    fn the_remove_confirm_says_the_running_copy_stays_and_the_next_build_asks() {
        assert_eq!(CONFIRM_TITLE, "Remove this dev folder?");
        assert_eq!((CONFIRM_CANCEL, CONFIRM_OK), ("Cancel", "Remove folder"));
        assert_eq!(REMOVE_BUTTON, "Remove\u{2026}");
        assert_eq!(FAILED, "Hideout couldn't save that. Nothing changed.");
        assert_eq!(
            confirm_body(&[]),
            "No apps from here. Nothing else changes."
        );
        assert_eq!(
            confirm_body(&["Foxglove".into()]),
            "Foxglove stays as it is now, and a copy that is running keeps running. Its next build asks for your OK again."
        );
        assert_eq!(
            confirm_body(&["A".into(), "B".into()]),
            "A and B stay as they are now, and copies that are running keep running. Their next build asks for your OK again."
        );
        assert_eq!(
            confirm_body(&["A".into(), "B".into(), "C".into()]),
            "A, B and C stay as they are now, and copies that are running keep running. Their next build asks for your OK again."
        );
        // No internal words in anything the dialog or the failure says.
        let mut all = vec![
            CONFIRM_TITLE.to_string(),
            CONFIRM_NO_APPS.to_string(),
            FAILED.to_string(),
            REMOVED.to_string(),
        ];
        for n in 0..4 {
            let apps: Vec<String> = (0..n).map(|i| format!("App{i}")).collect();
            all.push(confirm_body(&apps));
            all.push(removed_line(&apps));
        }
        for l in all {
            let l = l.to_lowercase();
            for bad in ["module", "hash", "enrol", "grant", "program"] {
                assert!(!l.contains(bad), "{l}");
            }
        }
    }

    const ALL: [Problem; 9] = [
        Problem::Missing,
        Problem::NotLocal,
        Problem::Drive,
        Problem::Profile,
        Problem::DropZone,
        Problem::System,
        Problem::WhoUnknown,
        Problem::FolderOpen,
        Problem::ParentOpen,
    ];

    #[test]
    fn every_problem_has_its_own_plain_line() {
        let all = [
            Problem::Missing,
            Problem::NotLocal,
            Problem::Drive,
            Problem::Profile,
            Problem::DropZone,
            Problem::System,
            Problem::WhoUnknown,
            Problem::FolderOpen,
            Problem::ParentOpen,
        ];
        let mut lines: Vec<_> = all.iter().map(|p| p.line()).collect();
        lines.sort();
        lines.dedup();
        assert_eq!(lines.len(), all.len());
        for l in lines {
            assert!(l.ends_with('.'), "{l}");
        }
    }
}
