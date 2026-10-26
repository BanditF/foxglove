//! Which plugins wait for which apps' windows (`apps.watch`, #277), and the windows of those
//! apps that are open. The app feeds it from a WinEvent hook (`shown`, `destroyed`) and lists
//! the windows already open when a watch starts (`set_lister`); plugins get
//! `app.window.appeared` and `app.window.closed`. Nothing here runs on a timer.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde_json::json;

use crate::notify;

/// Lists the visible top-level windows open now: (window, program file name).
pub type Lister = fn() -> Vec<(isize, String)>;

static LISTER: OnceLock<Lister> = OnceLock::new();

/// Told `true` when a watch starts and `false` when the last one ends, so the app keeps its
/// window hook only while a plugin waits for an app (#410). On `false`, check [`watching`]:
/// a watch may have started since.
pub type OnWatching = fn(bool);

static ON_WATCHING: OnceLock<OnWatching> = OnceLock::new();

#[derive(Default)]
struct State {
    /// Plugin id → the programs it waits for.
    watches: HashMap<String, HashSet<String>>,
    /// Window → program, for the watched programs' open windows.
    windows: HashMap<isize, String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(STATE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(State::default))
}

/// How the app lists the windows already open (once, at start).
pub fn set_lister(f: Lister) {
    let _ = LISTER.set(f);
}

/// How the app hears that watching starts or stops (once, at start).
pub fn set_on_watching(f: OnWatching) {
    let _ = ON_WATCHING.set(f);
}

/// Whether any plugin waits for an app, so the app can skip looking up new windows.
pub fn watching() -> bool {
    with(|s| any_watch(s))
}

fn any_watch(s: &State) -> bool {
    s.watches.values().any(|w| !w.is_empty())
}

fn watching_changed(now: bool) {
    if let Some(f) = ON_WATCHING.get() {
        f(now);
    }
}

fn watched(s: &State, exe: &str) -> bool {
    s.watches.values().any(|w| w.contains(exe))
}

fn open(s: &State, exe: &str) -> u32 {
    s.windows.values().filter(|e| *e == exe).count() as u32
}

/// A plugin starts waiting for `exe`: how many of its windows are open now.
pub(crate) fn watch(plugin: &str, exe: &str) -> u32 {
    // The hook goes on first, then the windows already open are listed, so a window shown in
    // between is reported by one or the other. Told again once the watch is in, in case the
    // last other watch ended meanwhile and turned the hook off.
    watching_changed(true);
    // Listed outside the lock: the hook thread may be reporting a window meanwhile, and the
    // window map takes each window once either way.
    let listed = LISTER.get().map(|l| l()).unwrap_or_default();
    let count = with(|s| {
        s.watches.entry(plugin.to_string()).or_default().insert(exe.to_string());
        for (window, e) in listed {
            if e.eq_ignore_ascii_case(exe) {
                s.windows.insert(window, exe.to_string());
            }
        }
        open(s, exe)
    });
    watching_changed(true);
    count
}

/// A plugin's connection closed: its watches end, and windows nobody waits for are forgotten.
pub(crate) fn unwatch(plugin: &str) {
    let ended = with(|s| {
        let before = any_watch(s);
        s.watches.remove(plugin);
        let watches = std::mem::take(&mut s.watches);
        s.windows.retain(|_, e| watches.values().any(|w| w.contains(e)));
        s.watches = watches;
        before && !any_watch(s)
    });
    if ended {
        watching_changed(false);
    }
}

/// Tells the plugins waiting for `exe`; how many were told.
fn tell(exe: &str, event: &str, open: u32) -> usize {
    let plugins: Vec<String> = with(|s| s.watches.iter().filter(|(_, w)| w.contains(exe)).map(|(p, _)| p.clone()).collect());
    plugins.iter().filter(|p| notify(p, event, json!({ "exe": exe, "open": open }))).count()
}

/// A visible top-level window of `exe` (a file name such as `obs64.exe`) appeared. How many
/// plugins were told; none if nobody waits for it or the window was known already.
pub fn shown(window: isize, exe: &str) -> usize {
    let exe = exe.to_ascii_lowercase();
    let open = with(|s| {
        if !watched(s, &exe) || s.windows.contains_key(&window) {
            return None;
        }
        s.windows.insert(window, exe.clone());
        Some(open(s, &exe))
    });
    open.map_or(0, |n| tell(&exe, "app.window.appeared", n))
}

/// A window was destroyed: if it was a watched app's, those plugins are told how many of its
/// windows are still open.
pub fn destroyed(window: isize) -> usize {
    let gone = with(|s| s.windows.remove(&window).map(|exe| (open(s, &exe), exe)));
    gone.map_or(0, |(n, exe)| tell(&exe, "app.window.closed", n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    static WATCHING_LOG: Mutex<Vec<bool>> = Mutex::new(Vec::new());

    #[test]
    fn windows_of_watched_apps_are_counted_and_told() {
        set_lister(|| vec![(1, "OBS64.exe".into()), (2, "notepad.exe".into())]);
        set_on_watching(|now| WATCHING_LOG.lock().unwrap().push(now));
        let (tx, rx) = mpsc::channel();
        crate::register("test.apps", Some(tx));
        assert_eq!(shown(7, "obs64.exe"), 0, "nobody waits yet");
        assert_eq!(watch("test.apps", "obs64.exe"), 1, "the window already open");
        assert!(watching());
        assert_eq!(shown(3, "notepad.exe"), 0);
        assert_eq!(shown(4, "obs64.exe"), 1);
        assert_eq!(shown(4, "obs64.exe"), 0, "once per window");
        assert_eq!(destroyed(1), 1);
        assert_eq!(destroyed(99), 0);
        assert_eq!(destroyed(4), 1);
        let got: Vec<serde_json::Value> = rx.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
        let seen: Vec<(String, u64)> = got.iter().map(|v| (v["method"].as_str().unwrap().to_string(), v["params"]["open"].as_u64().unwrap())).collect();
        assert_eq!(seen, [("app.window.appeared".into(), 2), ("app.window.closed".into(), 1), ("app.window.closed".into(), 0)]);
        assert!(WATCHING_LOG.lock().unwrap().contains(&true), "the first watch turns the hook on");
        unwatch("test.apps");
        if !watching() {
            assert_eq!(WATCHING_LOG.lock().unwrap().last(), Some(&false), "the last watch turns it off");
        }
        assert_eq!(shown(5, "obs64.exe"), 0);
        crate::register("test.apps", None);
    }
}
