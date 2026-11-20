// SPDX-License-Identifier: MIT
//! R5 of the 0-click rebuild plan: the registry side of **Settings > Developer**. It implements
//! the shell's [`DevControl`] over the hub's folder.
//!
//! Fails closed: **Add a folder...** opens the picker here and sends what it chose to
//! [`Registry::try_add_dev_folder`], the one place the folder checks run. No other call writes
//! the list, and nothing on the page can hand this a path to add.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hideout_registry::dev_folder::{DevFolderRefusal, FolderProblem, RowProblem};
use hideout_registry::Registry;
use hideout_shell::dev_settings::{self as words, DevClick, DevControl, DevRow, DevView, Note};
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
}

impl RegistryDev {
    pub fn new(home: PathBuf, pick: Pick) -> Self {
        Self {
            home,
            pick,
            mode: None,
            note: None,
            shown: None,
        }
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

    fn read(&self) -> DevView {
        // An unreadable registry shows dev mode off: nothing here can be changed.
        let Some(reg) = self.open() else {
            return DevView::default();
        };
        if !reg.dev_folders_on() {
            // Off: the page shows nothing unless folders are still listed, and then only their
            // paths.
            return DevView {
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
        }
        DevView {
            on: true,
            folders: reg
                .dev_folder_rows()
                .into_iter()
                .map(|r| DevRow {
                    path: r.path,
                    apps: r.apps,
                    problem: r.problem.map(row_problem),
                })
                .collect(),
            restart: reg.restart_after_rebuild(),
            note: None,
        }
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
                self.say(words::ADDED, false);
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
        let apps = reg
            .dev_folder_rows()
            .into_iter()
            .find(|r| r.path == path)
            .map(|r| r.apps);
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
                self.say(words::FAILED, true);
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
                self.read()
            }
        };
        self.shown = Some((v.clone(), Instant::now()));
        v.note = self.note.as_ref().map(|(n, _)| n.clone());
        v
    }

    fn click(&mut self, c: DevClick) {
        match c {
            DevClick::Add => self.add(),
            DevClick::Remove(p) => self.remove(&p),
            DevClick::Restart(on) => self.restart(on),
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
