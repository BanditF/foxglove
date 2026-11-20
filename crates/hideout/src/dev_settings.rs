// SPDX-License-Identifier: MIT
//! R5 of the 0-click rebuild plan: the registry side of **Settings > Developer**. It implements
//! the shell's [`DevControl`] over the hub's folder.
//!
//! Fails closed: **Add a folder...** opens the picker here and sends what it chose to
//! [`Registry::try_add_dev_folder`], the one place the folder checks run. No other call writes
//! the list, and nothing on the page can hand this a path to add.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hideout_registry::dev_folder::{path_within, DevFolderRefusal, FolderProblem, RowProblem};
use hideout_registry::inventory::{IdentityClass, ADOPTED};
use hideout_registry::keyboard::same_path;
use hideout_registry::Registry;
use hideout_shell::dev_settings::{
    self as words, AppAction, AppState, DevApp, DevClick, DevControl, DevRow, DevView, Note,
};
use words::Problem;

/// Picks a folder; `None` when the user cancelled.
pub type Pick = Box<dyn Fn() -> Option<PathBuf>>;

/// The page was last drawn more than this ago: it was closed and opened again, so the list is
/// read afresh (there is no on-show hook, and no timer).
const REOPENED: Duration = Duration::from_secs(1);

/// A note older than this is gone when the page is opened again.
const NOTE_LIFE: Duration = Duration::from_secs(30);

pub struct RegistryDev {
    home: PathBuf,
    pick: Pick,
    /// Tests: open the registry with dev mode forced to this instead of reading the environment.
    mode: Option<bool>,
    /// The line after the last click, and when it was said.
    note: Option<(Note, Instant)>,
    shown: Option<(DevView, Instant)>,
    /// The apps kept over rebuilds in `shown`, each with whether it is turned on.
    kept: BTreeMap<String, bool>,
    /// Hub facts (the run state); none without a hub.
    facts: Option<Facts>,
    /// Review... and Set up...: asks for an app's permission dialog.
    opener: Option<Open>,
}

impl RegistryDev {
    pub fn new(home: PathBuf, pick: Pick) -> Self {
        Self {
            home,
            pick,
            mode: None,
            note: None,
            shown: None,
            kept: BTreeMap::new(),
            facts: None,
            opener: None,
        }
    }

    /// Where the apps' run state comes from.
    pub fn with_facts(mut self, f: Facts) -> Self {
        self.facts = Some(f);
        self
    }

    /// What Review... and Set up... do.
    pub fn with_open(mut self, o: Open) -> Self {
        self.opener = Some(o);
        self
    }

    #[cfg(test)]
    fn with_mode(mut self, on: bool) -> Self {
        self.mode = Some(on);
        self
    }

    fn open(&self) -> Option<Registry> {
        let r = match self.mode {
            Some(on) => Registry::open(&self.home, on),
            None => Registry::open_from_env(&self.home),
        };
        match r {
            Ok(r) => Some(r),
            Err(e) => {
                log::warn!("dev folders: registry not read ({e:?})");
                None
            }
        }
    }

    /// The listed folders with their apps; `kept` is filled with the apps kept over rebuilds.
    fn rows(reg: &Registry, kept: &mut BTreeMap<String, bool>) -> Vec<DevRow> {
        reg.dev_folder_rows()
            .into_iter()
            .map(|r| {
                let apps = apps_in(reg, &r.path, r.problem.is_none(), kept);
                DevRow {
                    path: r.path,
                    apps,
                    problem: r.problem.map(row_problem),
                }
            })
            .collect()
    }

    fn read(&self) -> (DevView, BTreeMap<String, bool>) {
        let mut kept = BTreeMap::new();
        // An unreadable registry shows dev mode off: nothing here can be changed.
        let Some(reg) = self.open() else {
            return (DevView::default(), kept);
        };
        if !reg.dev_folders_on() {
            // Off: the page shows nothing unless folders are still listed, and then only their
            // paths.
            let v = DevView {
                folders: reg
                    .dev_folders()
                    .into_iter()
                    .map(|path| DevRow {
                        path,
                        apps: Vec::new(),
                        problem: None,
                    })
                    .collect(),
                ..DevView::default()
            };
            return (v, kept);
        }
        let v = DevView {
            on: true,
            folders: Self::rows(&reg, &mut kept),
            restart: reg.restart_after_rebuild(),
            note: None,
        };
        (v, kept)
    }

    fn add(&mut self) {
        // Cancelled: nothing changes and nothing is said.
        let Some(picked) = (self.pick)() else {
            return;
        };
        let Some(mut reg) = self.open() else {
            self.say(words::FAILED, true);
            return;
        };
        let Some(path) = picked.to_str() else {
            self.say(Problem::Missing.line(), true);
            return;
        };
        match reg.try_add_dev_folder(path) {
            Ok(a) if a.was_listed => self.say(words::ALREADY, false),
            Ok(a) => {
                log::info!("dev folder added by the user: {}", a.path);
                // Apps already in it that still need the user's Review: named, never allowed here.
                let mut kept = BTreeMap::new();
                let names: Vec<String> = Self::rows(&reg, &mut kept)
                    .into_iter()
                    .filter(|r| same_path(&r.path, &a.path))
                    .flat_map(|r| r.apps)
                    .filter(|x| matches!(x.state, AppState::NotKept | AppState::NeedsOk))
                    .map(|x| x.name)
                    .collect();
                self.say(&words::added_line(&names), false);
            }
            Err(e) => self.refused(e),
        }
    }

    fn refused(&mut self, e: DevFolderRefusal) {
        match e {
            DevFolderRefusal::ModeOff => self.say(words::MODE_OFF, true),
            DevFolderRefusal::NotAFolder => self.say(Problem::Missing.line(), true),
            DevFolderRefusal::NotLocal => self.say(Problem::NotLocal.line(), true),
            DevFolderRefusal::Problem(p) => self.say(folder_problem(p).line(), true),
            DevFolderRefusal::Failed(f) => {
                log::warn!("dev folders: not added: {f:?}");
                self.say(words::FAILED, true);
            }
        }
    }

    fn remove(&mut self, path: &str) {
        let Some(mut reg) = self.open() else {
            self.say(words::FAILED, true);
            return;
        };
        // Only a listed folder can be taken off, by the spelling the list shows.
        let mut kept = BTreeMap::new();
        let apps = Self::rows(&reg, &mut kept)
            .into_iter()
            .find(|r| r.path == path)
            .map(|r| words::kept_names(&r.apps));
        let Some(apps) = apps else {
            // Not on the list: nothing was removed, so nothing is claimed.
            self.note = None;
            return;
        };
        match reg.remove_dev_folder(path) {
            Ok(_) => {
                log::info!("dev folder removed by the user: {path}");
                self.say(&words::removed_line(&apps), false);
            }
            Err(e) => {
                log::warn!("dev folders: not removed: {e:?}");
                // The folder list may have been written before the inventory write failed:
                // then the folder is gone, and "Nothing changed" would be untrue.
                let still_listed = self
                    .open()
                    .is_none_or(|r| r.dev_folder_rows().iter().any(|x| x.path == path));
                if still_listed {
                    self.say(words::FAILED, true);
                } else {
                    self.say(words::REMOVED_NOT_TIDY, true);
                }
            }
        }
    }

    fn restart(&mut self, on: bool) {
        let done = self.open().map(|mut r| r.set_restart_after_rebuild(on));
        match done {
            Some(Ok(())) => {
                log::info!("restart after a rebuild turned {} by the user", onoff(on));
                self.note = None;
            }
            Some(Err(e)) => {
                log::warn!("dev folders: restart switch not saved: {e:?}");
                self.say(words::FAILED, true);
            }
            None => self.say(words::FAILED, true),
        }
    }

    /// Review... / Set up... on an app line: only for an app the page shows with that link.
    fn ask(&self, id: &str, what: AppAction) {
        let shown = self.shown.as_ref().is_some_and(|(v, _)| {
            v.folders
                .iter()
                .flat_map(|r| &r.apps)
                .any(|a| a.id == id && a.state.action() == Some(what))
        });
        if !shown {
            log::warn!("dev folders: {what:?} for an app the page does not show that way: ignored");
            return;
        }
        if let Some(open) = &self.opener {
            open(id, what == AppAction::SetUp);
        }
    }

    fn say(&mut self, text: &str, error: bool) {
        self.note = Some((
            Note {
                text: text.to_string(),
                error,
            },
            Instant::now(),
        ));
    }
}

/// Reads a fact of the hub (`module.<id>.state`); none when there is no hub or no such fact.
pub type Facts = Box<dyn Fn(&str) -> Option<serde_json::Value>>;

/// Shows an app's permission dialog: (the hub's id, true for Set up... and false for Review...).
pub type Open = Box<dyn Fn(&str, bool)>;

/// How an app that is kept over rebuilds is doing, from whether it is on and its `state` fact.
/// A missing or unknown fact is never "running".
fn run_state(enabled: bool, fact: Option<&serde_json::Value>) -> AppState {
    if !enabled {
        return AppState::Off;
    }
    match fact.and_then(|v| v.as_str()) {
        Some("running") => AppState::Running,
        Some("starting" | "restarting" | "update-busy") => AppState::Restarting,
        Some("crash-stopped" | "crash-looped" | "install-failed" | "quarantined" | "revoked") => {
            AppState::Stopped
        }
        _ => AppState::Off,
    }
}

/// The apps in one listed folder, and which of them are kept (id to whether it is turned on).
///
/// A row that qualifies lists every adopted app whose program is inside the folder. A row with a
/// problem lists only what was stored for it, as ignored. Read only.
fn apps_in(
    reg: &Registry,
    folder: &str,
    qualifies: bool,
    kept: &mut BTreeMap<String, bool>,
) -> Vec<DevApp> {
    let mut out: Vec<DevApp> = Vec::new();
    for (id, m) in &reg.inventory.modules {
        if m.source != ADOPTED {
            continue;
        }
        let name = |v: Option<&hideout_registry::permission::View>| {
            v.map(|v| v.listing.name.clone())
                .filter(|n| !n.is_empty())
                .or_else(|| reg.manifest(id).map(|m| m.name))
                .unwrap_or_else(|| id.clone())
        };
        let view = reg.permission_view(id).ok();
        let state = if !qualifies {
            // Only what was stored for this folder.
            let stored = matches!(
                m.identity.as_ref().map(|i| &i.class),
                Some(IdentityClass::DevFolder { folder: Some(f) }) if same_path(f, folder)
            );
            if !stored {
                continue;
            }
            AppState::Ignored
        } else {
            let Some(v) = view.as_ref() else { continue };
            if !path_within(folder, &v.listing.exe_path) {
                continue;
            }
            if v.bound.is_none() && v.granted.is_empty() {
                AppState::NotAllowed
            } else if v.void.is_some() || (v.bound.is_some() && !v.pending.is_empty()) {
                AppState::NeedsOk
            } else if !reg.dev_folder_of(id).is_some_and(|f| same_path(&f, folder)) {
                AppState::NotKept
            } else {
                // The run state is read from the facts on every frame; this is its placeholder.
                AppState::Off
            }
        };
        if state.kept() {
            kept.insert(id.clone(), reg.hub.enabled(id));
        }
        out.push(DevApp {
            id: id.clone(),
            name: name(view.as_ref()),
            state,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out
}

/// Gives each kept app its run state from the facts as they are now.
fn restate(v: &mut DevView, kept: &BTreeMap<String, bool>, facts: Option<&Facts>) {
    for row in &mut v.folders {
        for app in &mut row.apps {
            if matches!(app.state, AppState::Ignored) {
                continue;
            }
            if let Some(&enabled) = kept.get(&app.id) {
                let f = facts.and_then(|f| f(&format!("module.{}.state", app.id)));
                app.state = run_state(enabled, f.as_ref());
            }
        }
    }
}

fn onoff(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn folder_problem(p: FolderProblem) -> Problem {
    match p {
        FolderProblem::Drive => Problem::Drive,
        FolderProblem::Profile => Problem::Profile,
        FolderProblem::DropZone => Problem::DropZone,
        FolderProblem::System => Problem::System,
        FolderProblem::WhoUnknown => Problem::WhoUnknown,
        FolderProblem::FolderOpen => Problem::FolderOpen,
        FolderProblem::ParentOpen => Problem::ParentOpen,
    }
}

fn row_problem(p: RowProblem) -> Problem {
    match p {
        RowProblem::Missing => Problem::Missing,
        RowProblem::NotLocal => Problem::NotLocal,
        RowProblem::Folder(f) => folder_problem(f),
    }
}

impl DevControl for RegistryDev {
    fn view(&mut self) -> DevView {
        // Read afresh when there is nothing cached (start, after a click) or the page was
        // closed and opened again; the frames of one visit reuse the read.
        let mut v = match &self.shown {
            Some((v, at)) if at.elapsed() <= REOPENED => v.clone(),
            _ => {
                // Opened again: an old note belongs to an earlier visit.
                if self
                    .note
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() > NOTE_LIFE)
                {
                    self.note = None;
                }
                let (v, kept) = self.read();
                self.kept = kept;
                v
            }
        };
        self.shown = Some((v.clone(), Instant::now()));
        // The run state is read from the facts on every frame, so a change shows without a
        // fresh registry read and without a timer.
        restate(&mut v, &self.kept, self.facts.as_ref());
        v.note = self.note.as_ref().map(|(n, _)| n.clone());
        v
    }

    fn click(&mut self, c: DevClick) {
        match c {
            DevClick::Add => self.add(),
            DevClick::Remove(p) => self.remove(&p),
            DevClick::Restart(on) => self.restart(on),
            DevClick::Review(id) => self.ask(&id, AppAction::Review),
            DevClick::SetUp(id) => self.ask(&id, AppAction::SetUp),
        }
        // The next frame reads the list again.
        self.shown = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    fn temp(name: &str) -> PathBuf {
        // Under the build's target folder, not %TEMP%: a dev folder must not be writable by
        // other accounts, and a PC's temp folder often is.
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join("hideout-dev-settings-tests")
            .join(format!("{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn picking(p: Rc<RefCell<Option<PathBuf>>>) -> Pick {
        Box::new(move || p.borrow().clone())
    }

    fn ctl(name: &str, mode: bool) -> (RegistryDev, Rc<RefCell<Option<PathBuf>>>, PathBuf) {
        let home = temp(&format!("{name}-home"));
        let next = Rc::new(RefCell::new(None));
        let c = RegistryDev::new(home.clone(), picking(next.clone())).with_mode(mode);
        (c, next, home)
    }

    fn listed(home: &Path) -> Vec<String> {
        Registry::open(home, true).unwrap().dev_folders()
    }

    #[test]
    fn dev_mode_off_shows_only_the_off_line_and_adds_nothing() {
        let (mut c, next, home) = ctl("off", false);
        let v = c.view();
        assert!(!v.on && v.folders.is_empty());
        let dir = temp("off-dir");
        *next.borrow_mut() = Some(dir);
        c.click(DevClick::Add);
        let v = c.view();
        assert!(!v.on);
        assert_eq!(v.note.unwrap().text, words::MODE_OFF);
        assert!(listed(&home).is_empty());
    }

    #[test]
    fn add_and_remove_go_through_the_registry() {
        let (mut c, next, home) = ctl("addrm", true);
        assert!(c.view().folders.is_empty());
        let dir = temp("addrm-dir");
        *next.borrow_mut() = Some(dir.clone());
        c.click(DevClick::Add);
        let v = c.view();
        assert_eq!(v.note.as_ref().unwrap().text, words::ADDED);
        assert!(!v.note.as_ref().unwrap().error);
        assert_eq!(v.folders.len(), 1);
        assert_eq!(v.folders[0].problem, None);
        assert_eq!(listed(&home), vec![v.folders[0].path.clone()]);
        // Again: already there, still one.
        c.click(DevClick::Add);
        assert_eq!(c.view().note.unwrap().text, words::ALREADY);
        assert_eq!(listed(&home).len(), 1);
        // Remove.
        let path = c.view().folders[0].path.clone();
        c.click(DevClick::Remove(path));
        let v = c.view();
        assert_eq!(v.note.unwrap().text, words::REMOVED);
        assert!(v.folders.is_empty() && listed(&home).is_empty());
    }

    #[test]
    fn a_cancelled_picker_changes_and_says_nothing() {
        let (mut c, _next, home) = ctl("cancel", true);
        c.click(DevClick::Add);
        let v = c.view();
        assert!(v.note.is_none() && v.folders.is_empty());
        assert!(listed(&home).is_empty());
    }

    /// The picker is a path in, never a way round the checks: a drive root, a missing folder
    /// and the profile folder are each refused with their own line and nothing is written.
    #[test]
    fn the_picker_cannot_bypass_the_folder_checks() {
        let (mut c, next, home) = ctl("refuse", true);
        let mut cases: Vec<(PathBuf, Problem)> = vec![
            (temp("refuse-gone").join("nope"), Problem::Missing),
            (PathBuf::from(r"C:\"), Problem::Drive),
        ];
        if let Some(p) = std::env::var_os("USERPROFILE") {
            cases.push((PathBuf::from(p), Problem::Profile));
        }
        if let Some(p) = std::env::var_os("SystemRoot") {
            cases.push((PathBuf::from(p), Problem::System));
        }
        for (path, want) in cases {
            *next.borrow_mut() = Some(path.clone());
            c.click(DevClick::Add);
            let v = c.view();
            let note = v.note.expect("a refusal says why");
            assert!(note.error, "{path:?}");
            assert_eq!(note.text, want.line(), "{path:?}");
            assert!(listed(&home).is_empty(), "{path:?} was written");
        }
    }

    #[test]
    fn a_listed_folder_that_stops_qualifying_shows_its_problem() {
        let (mut c, next, home) = ctl("gone", true);
        let dir = temp("gone-dir");
        *next.borrow_mut() = Some(dir.clone());
        c.click(DevClick::Add);
        std::fs::remove_dir_all(&dir).unwrap();
        let v = c.view();
        assert_eq!(v.folders.len(), 1);
        assert_eq!(v.folders[0].problem, Some(Problem::Missing));
        let _ = home;
    }

    #[test]
    fn the_restart_switch_is_on_by_default_and_saved() {
        let (mut c, _next, home) = ctl("switch", true);
        assert!(c.view().restart);
        c.click(DevClick::Restart(false));
        assert!(!c.view().restart);
        assert!(!Registry::open(&home, true).unwrap().restart_after_rebuild());
        c.click(DevClick::Restart(true));
        assert!(c.view().restart);
    }

    #[test]
    fn remove_only_takes_a_listed_folder() {
        let (mut c, next, home) = ctl("rmstray", true);
        // An earlier note must not be left standing as if this had worked either.
        *next.borrow_mut() = Some(temp("rmstray-dir"));
        c.click(DevClick::Add);
        let kept = listed(&home);
        c.click(DevClick::Remove(r"C:\not\listed".into()));
        assert_eq!(listed(&home), kept);
        assert!(c.view().note.is_none(), "a no-op must not say Removed.");
    }

    #[test]
    fn a_note_does_not_outlive_the_visit() {
        let (mut c, next, _home) = ctl("oldnote", true);
        *next.borrow_mut() = Some(temp("oldnote-dir"));
        c.click(DevClick::Add);
        assert!(c.view().note.is_some());
        // Same visit, frames close together: still there even when the note is old.
        c.note.as_mut().unwrap().1 = Instant::now() - NOTE_LIFE * 2;
        assert!(c.view().note.is_some());
        // The page is opened again (last frame long ago): the old note is gone.
        c.shown.as_mut().unwrap().1 = Instant::now() - REOPENED * 5;
        assert!(c.view().note.is_none());
        // A fresh click sets a fresh note.
        c.click(DevClick::Add);
        assert!(c.view().note.is_some());
    }

    #[test]
    fn a_missing_or_unknown_state_is_never_running() {
        use serde_json::json;
        assert_eq!(run_state(true, Some(&json!("running"))), AppState::Running);
        assert_eq!(run_state(true, None), AppState::Off);
        assert_eq!(run_state(true, Some(&json!("whatever"))), AppState::Off);
        assert_eq!(run_state(true, Some(&json!(7))), AppState::Off);
        assert_eq!(run_state(false, Some(&json!("running"))), AppState::Off);
        for s in ["starting", "restarting", "update-busy"] {
            assert_eq!(run_state(true, Some(&json!(s))), AppState::Restarting);
        }
        for s in [
            "crash-stopped",
            "crash-looped",
            "install-failed",
            "quarantined",
            "revoked",
        ] {
            assert_eq!(run_state(true, Some(&json!(s))), AppState::Stopped);
        }
        for s in ["waiting", "update-waiting", "off"] {
            assert_eq!(run_state(true, Some(&json!(s))), AppState::Off);
        }
    }

    fn app(id: &str, state: AppState) -> DevApp {
        DevApp {
            id: id.into(),
            name: id.to_uppercase(),
            state,
        }
    }

    #[test]
    fn the_run_state_follows_the_facts_every_frame() {
        use serde_json::json;
        let fact = Rc::new(RefCell::new(None::<serde_json::Value>));
        let f2 = fact.clone();
        let (mut c, _n, _h) = ctl("restate", true);
        c = c.with_facts(Box::new(move |k| {
            (k == "module.vulpes.state")
                .then(|| f2.borrow().clone())
                .flatten()
        }));
        c.kept.insert("vulpes".into(), true);
        c.kept.insert("idle".into(), false);
        let v = DevView {
            on: true,
            folders: vec![DevRow {
                path: "C:\\d".into(),
                apps: vec![app("vulpes", AppState::Off), app("idle", AppState::Off)],
                problem: None,
            }],
            ..DevView::default()
        };
        c.shown = Some((v, Instant::now()));
        let state = |c: &mut RegistryDev| c.view().folders[0].apps[0].state;
        assert_eq!(state(&mut c), AppState::Off);
        *fact.borrow_mut() = Some(json!("running"));
        assert_eq!(state(&mut c), AppState::Running);
        *fact.borrow_mut() = Some(json!("restarting"));
        assert_eq!(state(&mut c), AppState::Restarting);
        *fact.borrow_mut() = None;
        assert_eq!(state(&mut c), AppState::Off, "no fact is never running");
        // The app that is turned off stays off whatever the fact says.
        *fact.borrow_mut() = Some(json!("running"));
        assert_eq!(c.view().folders[0].apps[1].state, AppState::Off);
    }

    #[test]
    fn review_and_set_up_open_only_what_the_page_shows_that_way() {
        let asked = Rc::new(RefCell::new(Vec::<(String, bool)>::new()));
        let a2 = asked.clone();
        let (mut c, _n, _h) = ctl("ask", true);
        c = c.with_open(Box::new(move |id, up| {
            a2.borrow_mut().push((id.into(), up))
        }));
        let view = || DevView {
            on: true,
            folders: vec![DevRow {
                path: "C:\\d".into(),
                apps: vec![
                    app("ok", AppState::NeedsOk),
                    app("new", AppState::NotAllowed),
                    app("run", AppState::Running),
                ],
                problem: None,
            }],
            ..DevView::default()
        };
        // Cached and shown as such: acts.
        c.shown = Some((view(), Instant::now()));
        c.click(DevClick::Review("ok".into()));
        c.shown = Some((view(), Instant::now()));
        c.click(DevClick::SetUp("new".into()));
        assert_eq!(
            *asked.borrow(),
            vec![("ok".to_string(), false), ("new".to_string(), true)]
        );
        // Wrong link for the app, an app that is running, an unknown id: nothing opens.
        for click in [
            DevClick::SetUp("ok".into()),
            DevClick::Review("new".into()),
            DevClick::Review("run".into()),
            DevClick::Review("nope".into()),
        ] {
            c.shown = Some((view(), Instant::now()));
            c.click(click);
        }
        // Nothing was shown yet: nothing opens.
        c.click(DevClick::Review("ok".into()));
        assert_eq!(asked.borrow().len(), 2);
    }

    #[test]
    fn dev_mode_off_with_folders_listed_shows_only_their_paths() {
        let (mut c, _next, home) = ctl("offlisted", true);
        let dir = temp("offlisted-dir");
        let mut reg = Registry::open(&home, true).unwrap();
        reg.add_dev_folder(&dir.display().to_string()).unwrap();
        c.mode = Some(false);
        let v = c.view();
        assert!(!v.on);
        assert_eq!(v.folders.len(), 1);
        assert!(v.folders[0].problem.is_none());
    }
}
