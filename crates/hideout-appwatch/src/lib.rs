// SPDX-License-Identifier: MIT
//! `hideout.appwatch` (contract §4.6, v1.7): tells modules whether an outside app runs, so a
//! module can tell "the app said no" from "the app isn't open". It watches only the apps some
//! manifest asks about ([`hideout_contract::module::Module::watched_apps`]); no app name lives
//! here. For each it publishes:
//!
//! - `apps.running.<name>`: `true` while a process `<name>.exe` runs, else `false`;
//! - `apps.seen.<name>`: `true` once the app has run on this PC, kept across restarts in
//!   [`SEEN_FILE`].
//!
//! Event-driven, never polling (Windows): one process snapshot at start, then a wait on each
//! watched process's handle for its exit, and an out-of-context `EVENT_OBJECT_CREATE` hook for
//! new top-level windows, whose process is looked up once per window. An app's start is seen
//! when it creates its first top-level window (a hidden one counts; apps in the notification
//! area make one). With no app to watch, no thread starts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use hideout_contract::module::app_label;
use serde_json::{json, Value};

#[cfg(windows)]
mod win;

/// The file in the hub's home that keeps the apps seen (`apps.seen.<name>`), one label a line.
pub const SEEN_FILE: &str = "apps-seen.txt";

/// Fact changes for the router to publish, as (key, value).
pub type Changes = Vec<(String, Value)>;

/// Where the facts go (the router's `publish`).
pub type Sink = Box<dyn Fn(Changes) + Send>;

/// The app facts' state, without I/O: which watched apps run (by process id) and which were
/// seen.
#[derive(Debug, Default)]
pub struct Watch {
    names: BTreeSet<String>,
    running: BTreeMap<u32, String>,
    seen: BTreeSet<String>,
    /// The facts as last published, so only changes go out.
    published: BTreeMap<String, Value>,
}

impl Watch {
    /// Watches `names` (app labels); `seen` are the apps seen before (from [`SEEN_FILE`]).
    pub fn new(names: BTreeSet<String>, seen: BTreeSet<String>) -> Watch {
        Watch {
            names,
            seen,
            ..Watch::default()
        }
    }

    /// The label a process's exe (a path or a file name) has, when it is a watched app.
    pub fn watched(&self, exe: &str) -> Option<String> {
        let file = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
        let lower = file.to_ascii_lowercase();
        lower.strip_suffix(".exe")?;
        app_label(&lower).filter(|l| self.names.contains(l))
    }

    /// A watched process `pid` runs (`label` from [`Watch::watched`]). Returns whether the app
    /// was seen for the first time.
    pub fn started(&mut self, pid: u32, label: &str) -> bool {
        self.running.insert(pid, label.to_string());
        self.seen.insert(label.to_string())
    }

    /// Process `pid` ended.
    pub fn ended(&mut self, pid: u32) {
        self.running.remove(&pid);
    }

    pub fn is_running(&self, pid: u32) -> bool {
        self.running.contains_key(&pid)
    }

    pub fn seen(&self) -> &BTreeSet<String> {
        &self.seen
    }

    /// The facts that changed since the last call: every watched app's `apps.running.<name>`,
    /// and `apps.seen.<name>` for each one seen.
    pub fn changes(&mut self) -> Changes {
        let mut now = BTreeMap::new();
        for n in &self.names {
            let up = self.running.values().any(|l| l == n);
            now.insert(format!("apps.running.{n}"), json!(up));
            if self.seen.contains(n) {
                now.insert(format!("apps.seen.{n}"), json!(true));
            }
        }
        let out: Changes = now
            .iter()
            .filter(|(k, v)| self.published.get(*k) != Some(*v))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.published = now;
        out
    }
}

/// The labels in a [`SEEN_FILE`] text.
pub fn parse_seen(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && app_label(l).as_deref() == Some(*l))
        .map(String::from)
        .collect()
}

fn read_seen(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .map(|t| parse_seen(&t))
        .unwrap_or_default()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn write_seen(path: &Path, seen: &BTreeSet<String>) {
    let mut text = String::new();
    for s in seen {
        text.push_str(s);
        text.push('\n');
    }
    if let Err(e) = std::fs::write(path, text) {
        log::warn!("appwatch: {} not saved: {e}", path.display());
    }
}

/// Starts the app watch for `names` (app labels) on its own thread, with the seen apps kept in
/// `home`'s [`SEEN_FILE`]. Every change goes to `sink`, the first ones (each app's state now)
/// before anything else. With no names, or off Windows, nothing starts and `None` comes back:
/// the facts stay unset, which `when` reads as false (§4.6).
pub fn start(
    names: BTreeSet<String>,
    home: &Path,
    sink: Sink,
) -> Option<std::thread::JoinHandle<()>> {
    if names.is_empty() {
        return None;
    }
    let file: PathBuf = home.join(SEEN_FILE);
    let watch = Watch::new(names, read_seen(&file));
    #[cfg(windows)]
    {
        win::spawn(watch, file, sink)
    }
    #[cfg(not(windows))]
    {
        let _ = (watch, file, sink);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(n: &[&str]) -> BTreeSet<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn only_watched_apps_match_by_label() {
        let w = Watch::new(names(&["example-app", "tool64"]), BTreeSet::new());
        assert_eq!(
            w.watched(r"C:\Program Files\Example\Example-App.exe")
                .as_deref(),
            Some("example-app")
        );
        assert_eq!(w.watched("TOOL64.EXE").as_deref(), Some("tool64"));
        assert_eq!(w.watched("notepad.exe"), None);
        assert_eq!(w.watched("example-app.exe.bak"), None);
    }

    #[test]
    fn running_and_seen_follow_processes_and_send_changes_only() {
        let mut w = Watch::new(names(&["example-app", "other"]), names(&["other"]));
        assert_eq!(
            w.changes(),
            vec![
                ("apps.running.example-app".into(), json!(false)),
                ("apps.running.other".into(), json!(false)),
                ("apps.seen.other".into(), json!(true)),
            ]
        );
        assert!(w.changes().is_empty());
        assert!(w.started(10, "example-app"));
        assert!(!w.started(11, "example-app"));
        assert_eq!(
            w.changes(),
            vec![
                ("apps.running.example-app".into(), json!(true)),
                ("apps.seen.example-app".into(), json!(true)),
            ]
        );
        // One of two processes ends: still running.
        w.ended(10);
        assert!(w.changes().is_empty());
        w.ended(11);
        assert_eq!(
            w.changes(),
            vec![("apps.running.example-app".into(), json!(false))]
        );
    }

    #[test]
    fn the_seen_file_keeps_labels_only() {
        assert_eq!(
            parse_seen("example-app\n\nTool64\ntool64\nbad label\n"),
            names(&["example-app", "tool64"])
        );
        let dir = std::env::temp_dir().join(format!("hideout-appwatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(SEEN_FILE);
        write_seen(&f, &names(&["b", "a"]));
        assert_eq!(read_seen(&f), names(&["a", "b"]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nothing_starts_without_names() {
        assert!(start(BTreeSet::new(), Path::new("."), Box::new(|_| {})).is_none());
    }
}
