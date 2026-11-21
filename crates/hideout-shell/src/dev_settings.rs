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
/// The second line when dev mode is off but folders are still listed (a developer): what
/// happens to their apps, and the way back.
#[cfg(feature = "lab-switches")]
pub const MODE_OFF_MORE: &str = "Apps from them ask for your OK at their next build. To use dev folders, start Hideout with HIDEOUT_DEV_MODE=1.";
/// The installed build has no developer mode: say where it is instead of naming a switch it ignores.
#[cfg(not(feature = "lab-switches"))]
pub const MODE_OFF_MORE: &str = "Apps from them ask for your OK at their next build. Dev folders need a developer build of Hideout.";

pub const ADDED: &str = "Added. Apps you allow from here keep what you allowed over rebuilds.";
pub const ALREADY: &str = "That folder is already on the list.";
pub const REMOVED: &str = "Removed.";
pub const FAILED: &str = "Hideout couldn't save that. Nothing changed. Try again.";

/// The folder is off the list but the clean-up behind it failed: "Nothing changed" would be untrue.
pub const REMOVED_NOT_TIDY: &str =
    "Hideout removed the folder but couldn't finish tidying up. It will finish next time it starts.";

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

// App lines (DF2): one line per app in a folder, "{name} \u{b7} {state}", the state in words.
pub const REVIEW: &str = "Review\u{2026}";
pub const SET_UP: &str = "Set up\u{2026}";

/// What one app in a dev folder is doing, as a person would say it. Never colour alone: each
/// state is its own words. A state Hideout could not read is never [`AppState::Running`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    /// Kept over rebuilds and running.
    Running,
    /// Kept over rebuilds, off or idle (and any state that is not known).
    Off,
    /// Kept, starting or restarting.
    Restarting,
    /// Kept, stopped after crashes or could not start.
    Stopped,
    /// Its OK is void or it asks for more.
    NeedsOk,
    /// Inside the folder and allowed, but its rebuilds are not kept yet.
    NotKept,
    /// Inside the folder and never allowed.
    NotAllowed,
    /// On a row Hideout ignores: shown from what was stored.
    Ignored,
}

/// What the link beside an app line does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAction {
    Review,
    SetUp,
}

impl AppState {
    pub fn words(self) -> &'static str {
        match self {
            AppState::Running => "Running",
            AppState::Off => "Off",
            AppState::Restarting => "Restarting\u{2026}",
            AppState::Stopped => "Stopped",
            AppState::NeedsOk => "Needs your OK",
            AppState::NotKept => "Not kept over rebuilds yet",
            AppState::NotAllowed => "Not allowed yet",
            AppState::Ignored => "Not kept while Hideout ignores this folder",
        }
    }

    pub fn action(self) -> Option<AppAction> {
        match self {
            AppState::NeedsOk | AppState::NotKept => Some(AppAction::Review),
            AppState::NotAllowed => Some(AppAction::SetUp),
            _ => None,
        }
    }

    /// Its rebuilds are kept (so Remove makes it ask again).
    pub fn kept(self) -> bool {
        matches!(
            self,
            AppState::Running
                | AppState::Off
                | AppState::Restarting
                | AppState::Stopped
                | AppState::Ignored
        )
    }
}

impl AppAction {
    pub fn label(self) -> &'static str {
        match self {
            AppAction::Review => REVIEW,
            AppAction::SetUp => SET_UP,
        }
    }
}

/// One app inside a dev folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevApp {
    /// The hub's id for it (what a click hands back; never shown).
    pub id: String,
    pub name: String,
    pub state: AppState,
}

impl DevApp {
    /// The visible line: "Vulpes \u{b7} Needs your OK".
    pub fn line(&self) -> String {
        format!("{} \u{b7} {}", self.name, self.state.words())
    }

    /// The screen reader's one label: "Vulpes, needs your OK".
    pub fn label(&self) -> String {
        let w = self.state.words().trim_end_matches('\u{2026}');
        let mut ch = w.chars();
        let low: String = ch
            .next()
            .map(|f| f.to_lowercase().chain(ch).collect())
            .unwrap_or_default();
        format!("{}, {low}", self.name)
    }

    /// The link's accessible name: "Review Vulpes" / "Set up Vulpes".
    pub fn action_name(&self) -> Option<String> {
        self.state.action().map(|a| match a {
            AppAction::Review => format!("Review {}", self.name),
            AppAction::SetUp => format!("Set up {}", self.name),
        })
    }
}

/// The names of the apps whose rebuilds are kept: who asks again when the folder is removed.
pub fn kept_names(apps: &[DevApp]) -> Vec<String> {
    apps.iter()
        .filter(|a| a.state.kept())
        .map(|a| a.name.clone())
        .collect()
}

fn join_and(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// The note after adding a folder that has apps already allowed in it: points to Review... and
/// grants nothing.
pub fn added_line(names: &[String]) -> String {
    match names {
        [] => ADDED.to_string(),
        [one] => format!(
            "Added. {one} is in this folder. Review it once so its rebuilds keep what you allowed."
        ),
        many => format!(
            "Added. {} are in this folder. Review each once so their rebuilds keep what you allowed.",
            join_and(many)
        ),
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
    /// A USB, SD or other plug-in drive.
    PlugIn,
    /// A virtual disk, or a drive Hideout can't tell the kind of.
    VirtualDrive,
    /// A network share or mapped drive.
    NetworkDrive,
    /// Syncs with the cloud (OneDrive and others).
    Cloud,
    /// Hideout couldn't tell whether it syncs with the cloud.
    CloudUnknown,
    /// The folder's name isn't valid text (refusal only).
    NameUnreadable,
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
            Problem::Missing => {
                "Hideout can't find that folder. Check that it still exists, or choose another."
            }
            Problem::PlugIn => {
                "Folders on a USB or other plug-in drive can't be dev folders. Choose one on this PC's own drive."
            }
            Problem::VirtualDrive => {
                "Folders on a virtual or unknown drive can't be dev folders. Choose one on this PC's own drive."
            }
            Problem::NetworkDrive => {
                "Network folders can't be dev folders. Choose one on this PC's own drive."
            }
            Problem::Cloud => {
                "Folders that sync with the cloud can't be dev folders. Choose one that stays on this PC."
            }
            Problem::CloudUnknown => {
                "Hideout couldn't check whether that folder syncs with the cloud. Try again."
            }
            Problem::NameUnreadable => {
                "Hideout can't read that folder's name. Rename it or choose another."
            }
            Problem::Drive => "A whole drive can't be a dev folder. Choose a folder inside it.",
            Problem::Profile => {
                "Your whole profile folder can't be a dev folder. Choose a project folder inside it."
            }
            Problem::DropZone => {
                "Folders in AppData, Temp or Downloads can't be dev folders. Choose a project folder outside them."
            }
            Problem::System => {
                "System and shared folders can't be dev folders. Choose a project folder of your own."
            }
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
            Problem::PlugIn => "This folder is on a plug-in drive, so Hideout ignores it.",
            Problem::VirtualDrive => {
                "This folder is on a virtual or unknown drive, so Hideout ignores it."
            }
            Problem::NetworkDrive => "This folder is on a network drive, so Hideout ignores it.",
            Problem::Cloud => "This folder syncs with the cloud, so Hideout ignores it.",
            Problem::CloudUnknown => {
                "Hideout couldn't check whether this folder syncs with the cloud, so Hideout ignores it for now."
            }
            Problem::NameUnreadable => "Hideout can't read this folder's name, so it ignores it.",
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
    /// The apps inside it.
    pub apps: Vec<DevApp>,
    /// Set when the folder no longer qualifies (its apps are not carried).
    pub problem: Option<Problem>,
    /// Another listed folder that holds this one ("Also inside {parent}.").
    pub also_inside: Option<String>,
}

/// The note when a picked folder sits inside one that is already a dev folder.
pub fn inside_line(parent: &str) -> String {
    format!("That folder is inside {parent}, which is already a dev folder.")
}

/// The muted line on a row whose folder is held by another listed folder.
pub fn also_inside_line(parent: &str) -> String {
    format!("Also inside {parent}.")
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
    /// An app's **Review...**: the hub opens its permission dialog (by the hub's id).
    Review(String),
    /// An app's **Set up...**: the same dialog for an app never allowed.
    SetUp(String),
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
        assert_eq!(NO_APPS, "No apps from here yet");
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

    /// A refusal says what to do next: a second sentence that starts with a verb the person
    /// can act on.
    #[test]
    fn every_refusal_says_what_to_do_next() {
        let next = ["Choose ", "Try again.", "Check ", "Rename "];
        for l in ALL.iter().map(|p| p.line()).chain([FAILED]) {
            let last = l.trim_end_matches('.').rsplit(". ").next().unwrap();
            assert!(l.contains(". "), "{l}");
            assert!(
                next.iter().any(|n| format!("{last}.").starts_with(n)),
                "{l}"
            );
        }
        assert_eq!(
            Problem::Missing.line(),
            "Hideout can't find that folder. Check that it still exists, or choose another."
        );
        assert_eq!(
            Problem::DropZone.line(),
            "Folders in AppData, Temp or Downloads can't be dev folders. Choose a project folder outside them."
        );
        assert_eq!(
            Problem::System.line(),
            "System and shared folders can't be dev folders. Choose a project folder of your own."
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
        assert_eq!(
            FAILED,
            "Hideout couldn't save that. Nothing changed. Try again."
        );
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
            REMOVED_NOT_TIDY.to_string(),
            REMOVED.to_string(),
            MODE_OFF.to_string(),
        ];
        for n in 0..4 {
            let apps: Vec<String> = (0..n).map(|i| format!("App{i}")).collect();
            all.push(confirm_body(&apps));
            all.push(removed_line(&apps));
            all.push(added_line(&apps));
        }
        for l in all {
            let l = l.to_lowercase();
            for bad in ["module", "hash", "enrol", "grant", "program"] {
                assert!(!l.contains(bad), "{l}");
            }
        }
    }

    /// DF5 (Q3): the dev-off second line, word for word, in plain words.
    #[test]
    fn the_dev_off_second_line_names_the_way_back() {
        assert_eq!(
            MODE_OFF_MORE,
            "Apps from them ask for your OK at their next build. To use dev folders, start Hideout with HIDEOUT_DEV_MODE=1."
        );
        let l = MODE_OFF_MORE.to_lowercase();
        for bad in ["module", "hash", "enrol", "grant", "program"] {
            assert!(!l.contains(bad), "{l}");
        }
    }

    fn app(name: &str, state: AppState) -> DevApp {
        DevApp {
            id: name.to_lowercase(),
            name: name.into(),
            state,
        }
    }

    #[test]
    fn each_app_state_has_its_own_words_and_the_right_link() {
        let all = [
            (AppState::Running, "Running", None),
            (AppState::Off, "Off", None),
            (AppState::Restarting, "Restarting\u{2026}", None),
            (AppState::Stopped, "Stopped", None),
            (AppState::NeedsOk, "Needs your OK", Some(AppAction::Review)),
            (
                AppState::NotKept,
                "Not kept over rebuilds yet",
                Some(AppAction::Review),
            ),
            (
                AppState::NotAllowed,
                "Not allowed yet",
                Some(AppAction::SetUp),
            ),
            (
                AppState::Ignored,
                "Not kept while Hideout ignores this folder",
                None,
            ),
        ];
        for (s, words, act) in all {
            assert_eq!(s.words(), words);
            assert_eq!(s.action(), act);
            let l = app("Vulpes", s).line();
            assert_eq!(l, format!("Vulpes \u{b7} {words}"));
            // Internal words never reach the user.
            for bad in ["module", "hash", "enrol", "grant", "program", "identity"] {
                assert!(!l.to_lowercase().contains(bad), "{l}");
            }
        }
        assert_eq!((REVIEW, SET_UP), ("Review\u{2026}", "Set up\u{2026}"));
    }

    #[test]
    fn an_app_reads_as_one_label_and_its_link_names_it() {
        assert_eq!(
            app("Foxglove", AppState::Running).label(),
            "Foxglove, running"
        );
        assert_eq!(
            app("Vulpes", AppState::NeedsOk).label(),
            "Vulpes, needs your OK"
        );
        assert_eq!(
            app("Gizmo Keys", AppState::NotKept).label(),
            "Gizmo Keys, not kept over rebuilds yet"
        );
        assert_eq!(app("A", AppState::Restarting).label(), "A, restarting");
        assert_eq!(
            app("Vulpes", AppState::NeedsOk).action_name().as_deref(),
            Some("Review Vulpes")
        );
        assert_eq!(
            app("Gizmo", AppState::NotAllowed).action_name().as_deref(),
            Some("Set up Gizmo")
        );
        assert_eq!(app("Foxglove", AppState::Running).action_name(), None);
    }

    #[test]
    fn only_kept_apps_ask_again_on_remove() {
        let apps = [
            app("A", AppState::Running),
            app("B", AppState::NeedsOk),
            app("C", AppState::NotKept),
            app("D", AppState::NotAllowed),
            app("E", AppState::Ignored),
            app("F", AppState::Off),
        ];
        assert_eq!(kept_names(&apps), vec!["A", "E", "F"]);
    }

    #[test]
    fn the_added_line_points_to_review_and_grants_nothing() {
        assert_eq!(added_line(&[]), ADDED);
        assert_eq!(
            added_line(&["Vulpes".into()]),
            "Added. Vulpes is in this folder. Review it once so its rebuilds keep what you allowed."
        );
        assert_eq!(
            added_line(&["A".into(), "B".into()]),
            "Added. A and B are in this folder. Review each once so their rebuilds keep what you allowed."
        );
        assert_eq!(
            added_line(&["A".into(), "B".into(), "C".into()]),
            "Added. A, B and C are in this folder. Review each once so their rebuilds keep what you allowed."
        );
        for n in 0..4 {
            let names: Vec<String> = (0..n).map(|i| format!("App{i}")).collect();
            let l = added_line(&names).to_lowercase();
            for bad in ["module", "hash", "enrol", "grant", "program"] {
                assert!(!l.contains(bad), "{l}");
            }
        }
    }

    const ALL: [Problem; 14] = [
        Problem::Missing,
        Problem::PlugIn,
        Problem::VirtualDrive,
        Problem::NetworkDrive,
        Problem::Cloud,
        Problem::CloudUnknown,
        Problem::NameUnreadable,
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
        let all = ALL;
        let mut lines: Vec<_> = all.iter().map(|p| p.line()).collect();
        lines.sort();
        lines.dedup();
        assert_eq!(lines.len(), all.len());
        for l in lines {
            assert!(l.ends_with('.'), "{l}");
        }
    }

    #[test]
    fn the_new_refusal_words_are_exact_and_plain() {
        assert_eq!(
            inside_line(r"C:\Dev"),
            r"That folder is inside C:\Dev, which is already a dev folder."
        );
        assert_eq!(also_inside_line(r"C:\Dev"), r"Also inside C:\Dev.");
        let mut all: Vec<String> = ALL
            .iter()
            .flat_map(|p| [p.line().to_string(), p.row_line().to_string()])
            .collect();
        all.push(inside_line("X"));
        all.push(also_inside_line("X"));
        for l in all {
            let l = l.to_lowercase();
            // One noun "app"; no internal words and no vendor names in the words.
            for bad in [
                "module", "hash", "enrol", "grant", "program", "onedrive", "dropbox", "icloud",
                "hresult", "cfapi",
            ] {
                assert!(!l.contains(bad), "{l}");
            }
        }
    }
}
