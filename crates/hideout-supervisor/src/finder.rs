// SPDX-License-Identifier: MIT
//! Carries out a search for the module that breaks Hideout ([`crate::bisect`], §7.10, #1033):
//! starts and stops each round's modules through the supervisor, keeps `run\bisect.json`, runs
//! the round's 5 minutes and carries out the result's choice.
//!
//! The 5 minutes count only while Hideout runs and the PC is awake (`QueryUnbiasedInterruptTime`,
//! which stops in sleep and hibernation). One timer thread waits for the round's end or a change
//! (a channel): no polling. A wake before the end (a change, a timeout that counted sleep) works
//! out what is left and waits again.
//!
//! Nothing here edits the saved on/off list but the result's **Turn off <Module>** (the
//! supervisor's Turn off, as the Modules page saves it).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use hideout_registry::Registry;

use crate::bisect::{self, Act, Candidate, Entry, Exit, Phase, Search, View};
use crate::closes;
use crate::driver::Supervisor;

/// Runs after every change of the search (any thread): the window's waker, the tray.
pub type Observer = Box<dyn Fn(&Finder) + Send + Sync>;

/// Milliseconds the PC has been awake since Windows started (sleep and hibernation don't count).
pub fn awake_ms() -> u64 {
    let mut t = 0u64;
    // 100 ns units.
    unsafe { windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(&mut t) };
    t / 10_000
}

/// When Windows started, ms since 1970, from `now_ms`.
pub fn boot_ms(now_ms: u64) -> u64 {
    now_ms
        .saturating_sub(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() })
}

/// How the last run ended, for the search: `readable` is [`closes::readable`] before the start
/// counted, `start` what [`closes::on_start`] found, `boot_ms` when Windows started. A run that
/// started before Windows did went down with the PC: that end can't be read.
pub fn exit_kind(readable: bool, start: &closes::Start, boot_ms: u64) -> Exit {
    if !readable {
        return Exit::Unknown;
    }
    if !start.unclean {
        return Exit::Clean;
    }
    match start.last_since_ms {
        // A little slack for the boot time's rounding.
        Some(t) if boot_ms > t + 2_000 => Exit::Unknown,
        _ => Exit::Unclean,
    }
}

/// What the Modules page needs during a search: the modules on now and every one in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rows {
    pub on: BTreeSet<String>,
    pub in_search: BTreeSet<String>,
}

/// The safe-mode banner's search parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Banner {
    /// The link's words: "Find the module that breaks Hideout", "Check whether Art breaks
    /// Hideout"; `None` when no module would start (the banner says it's Hideout itself).
    pub find: Option<String>,
    /// Stop finding was the last thing done: "Stopped finding. Nothing was changed."
    pub stopped: bool,
}

struct St {
    search: Option<Search>,
    /// Counts the changes (the view's `seq`).
    seq: u64,
    /// The awake clock when the running round started.
    since: Option<u64>,
    stopped: bool,
}

struct Inner {
    home: PathBuf,
    sup: Supervisor,
    st: Mutex<St>,
    /// Each module's `requires` ids (the provider links that keep entries together).
    links: BTreeMap<String, Vec<String>>,
    /// The version kept of each module (Go back).
    previous: BTreeMap<String, String>,
    observers: Mutex<Vec<Observer>>,
    nudge: Mutex<Sender<()>>,
    hub_version: String,
}

/// The search's driver. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Finder(Arc<Inner>);

impl Finder {
    /// Picks up a search in progress (`run\bisect.json`) after the hub started: `exit` is how
    /// the last run ended, which counts for its round. Starts the timer thread.
    pub fn start(home: &Path, sup: Supervisor, exit: Exit, hub_version: &str) -> Finder {
        let (mut links, mut previous) = (BTreeMap::new(), BTreeMap::new());
        if let Ok(reg) = Registry::open_from_env(home) {
            for (id, m) in &reg.inventory.modules {
                if let Some(man) = reg.manifest(id) {
                    let needs: Vec<String> = man
                        .requires
                        .iter()
                        .filter_map(|r| hideout_contract::module::requires_id(r))
                        .map(String::from)
                        .collect();
                    links.insert(id.clone(), needs);
                }
                if let Some(p) = &m.previous {
                    previous.insert(id.clone(), p.clone());
                }
            }
        }
        let mut search = Search::load(home);
        if let Some(s) = search.as_mut() {
            let before = s.phase;
            s.after_exit(exit);
            log::info!(
                "finder: search in progress, round {} ({before:?}); last exit {exit:?}; now {:?}",
                s.round,
                s.phase
            );
            if let Err(e) = s.save(home) {
                log::warn!("finder: run\\bisect.json not saved: {e}");
            }
        }
        let (nudge, wake) = mpsc::channel();
        let f = Finder(Arc::new(Inner {
            home: home.to_path_buf(),
            sup,
            st: Mutex::new(St {
                search,
                seq: 1,
                since: None,
                stopped: false,
            }),
            links,
            previous,
            observers: Mutex::new(Vec::new()),
            nudge: Mutex::new(nudge),
            hub_version: hub_version.into(),
        }));
        let weak = Arc::downgrade(&f.0);
        if let Err(e) = std::thread::Builder::new()
            .name("hideout-finder-timer".into())
            .spawn(move || timer(weak, wake))
        {
            log::warn!("finder: timer thread not started: {e}");
        }
        f
    }

    /// Runs `f` after every change (and once now).
    pub fn on_change(&self, f: Observer) {
        f(self);
        self.0.observers.lock().unwrap().push(f);
    }

    /// The entries a new search would have.
    pub fn entries(&self) -> Vec<Entry> {
        let cands: Vec<Candidate> = self
            .0
            .sup
            .startable()
            .into_iter()
            .map(|(id, name)| Candidate {
                needs: self.0.links.get(&id).cloned().unwrap_or_default(),
                id,
                name,
            })
            .collect();
        bisect::entries(&cands)
    }

    /// A search is on (a round, between rounds, a question or the result).
    pub fn searching(&self) -> bool {
        self.0.st.lock().unwrap().search.is_some()
    }

    /// A search runs (no result yet): the Modules page's toggles are locked.
    pub fn active(&self) -> bool {
        self.0
            .st
            .lock()
            .unwrap()
            .search
            .as_ref()
            .is_some_and(Search::active)
    }

    /// The safe-mode banner's link and line.
    pub fn banner(&self) -> Banner {
        let stopped = self.0.st.lock().unwrap().stopped;
        let e = self.entries();
        let find = match e.as_slice() {
            [] => None,
            [one] => Some(bisect::words::check_one(&one.label())),
            _ => Some(bisect::words::FIND.into()),
        };
        Banner { find, stopped }
    }

    /// The Modules page's rows while a search runs (at the result they read as in safe mode).
    pub fn rows(&self) -> Option<Rows> {
        let st = self.0.st.lock().unwrap();
        let s = st.search.as_ref().filter(|s| s.active())?;
        Some(Rows {
            on: s.on_ids(),
            in_search: s.all_ids(),
        })
    }

    /// The bar's view, `None` without a search.
    pub fn view(&self) -> Option<View> {
        let st = self.0.st.lock().unwrap();
        let s = st.search.as_ref()?;
        let previous = match s.phase {
            Phase::Found { entry } => match s.entries[entry].ids.as_slice() {
                [id] => self.0.previous.get(id).map(String::as_str),
                _ => None,
            },
            _ => None,
        };
        let mut v = bisect::view(s, left(&st), previous);
        v.seq = st.seq;
        Some(v)
    }

    /// The tray's status line and menu items, `None` for the safe-mode look.
    pub fn tray(&self) -> Option<bisect::Tray> {
        self.0
            .st
            .lock()
            .unwrap()
            .search
            .as_ref()
            .and_then(bisect::tray)
    }

    /// The notification for a result: title, body.
    pub fn notification(&self) -> Option<(String, String)> {
        self.0
            .st
            .lock()
            .unwrap()
            .search
            .as_ref()
            .and_then(bisect::notification)
    }

    /// Copy diagnostics: the rounds and the result, ids and words only.
    pub fn diagnostics(&self) -> String {
        let st = self.0.st.lock().unwrap();
        bisect::diagnostics(st.search.as_ref(), &self.0.hub_version)
    }

    /// The running round's time left.
    pub fn remaining(&self) -> Option<Duration> {
        left(&self.0.st.lock().unwrap())
    }

    /// **Find the module that breaks Hideout**: round 1 starts at once.
    pub fn begin(&self) {
        let entries = self.entries();
        let n = entries.len();
        let Some(s) = Search::new(entries) else {
            log::info!("finder: nothing to search: no module would start");
            return;
        };
        log::info!(
            "finder: search started: {n} entries, at most {} rounds",
            s.bound
        );
        self.set(Some(s), false);
    }

    /// A button of the bar, a card, the result or the tray.
    pub fn act(&self, a: Act) {
        let mut st = self.0.st.lock().unwrap();
        let Some(mut s) = st.search.clone() else {
            drop(st);
            if a == Act::TryAgain {
                self.begin();
            }
            return;
        };
        let changed = match a {
            Act::Fine => s.fine(false),
            Act::Closed => s.closed(),
            Act::StartNext => s.start_next(),
            Act::GoOn => s.go_on(),
            Act::CopyDiagnostics => false,
            Act::Stop => {
                drop(st);
                log::info!("finder: Stop finding; nothing was changed");
                self.set(None, true);
                return;
            }
            Act::TryAgain => {
                drop(st);
                self.set(None, false);
                self.begin();
                return;
            }
            Act::TurnOff | Act::GoBack | Act::StartNormally => {
                let found = match s.phase {
                    Phase::Found { entry } => Some(s.entries[entry].ids.clone()),
                    _ => None,
                };
                st.stopped = false;
                drop(st);
                self.set(None, false);
                match (a, found) {
                    (Act::TurnOff, Some(ids)) => {
                        for id in &ids {
                            log::info!("finder: result: Turn off {id}");
                            self.0.sup.turn_off(id);
                        }
                    }
                    (Act::GoBack, Some(ids)) if ids.len() == 1 => {
                        log::info!("finder: result: Go back {}", ids[0]);
                        self.0.sup.roll_back(&ids[0]);
                    }
                    _ => log::info!("finder: result: Start normally"),
                }
                self.0.sup.start_normally();
                return;
            }
        };
        if !changed {
            return;
        }
        log::info!("finder: {a:?}: round {} now {:?}", s.round, s.phase);
        drop(st);
        self.set(Some(s), false);
    }

    /// The round's 5 minutes passed.
    fn timer_fine(&self) {
        let mut st = self.0.st.lock().unwrap();
        let Some(mut s) = st.search.clone() else {
            return;
        };
        if left(&st) != Some(Duration::ZERO) || !s.fine(true) {
            return;
        }
        st.since = None;
        log::info!(
            "finder: 5 minutes open: round {} fine",
            s.round.saturating_sub(1)
        );
        drop(st);
        self.set(Some(s), false);
    }

    /// Takes the new state: saves or clears the file, starts the round's modules (or none),
    /// starts the round's clock, then tells the observers and the timer.
    fn set(&self, search: Option<Search>, stopped: bool) {
        let on = {
            let mut st = self.0.st.lock().unwrap();
            let was_running = st
                .search
                .as_ref()
                .map(|s| (s.phase == Phase::Running, s.round));
            let saved = match &search {
                Some(s) => s.save(&self.0.home),
                None => Search::clear(&self.0.home),
            };
            if let Err(e) = saved {
                log::warn!("finder: run\\bisect.json not saved: {e}");
            }
            let now_running = search
                .as_ref()
                .map(|s| (s.phase == Phase::Running, s.round));
            match now_running {
                Some((true, _)) if now_running != was_running => st.since = Some(awake_ms()),
                Some((true, _)) => {}
                _ => st.since = None,
            }
            st.stopped = stopped;
            st.seq += 1;
            let on = search.as_ref().map(Search::on_ids);
            st.search = search;
            on
        };
        // The round's modules; between rounds and at the result nothing is on; without a
        // search the rounds are over (safe mode holds them all).
        self.0.sup.set_round(on);
        self.changed();
    }

    fn changed(&self) {
        for o in self.0.observers.lock().unwrap().iter() {
            o(self);
        }
        let _ = self.0.nudge.lock().unwrap().send(());
    }
}

/// The running round's time left: 5 minutes less the awake time since it started.
fn left(st: &St) -> Option<Duration> {
    let running = st
        .search
        .as_ref()
        .is_some_and(|s| s.phase == Phase::Running);
    let since = st.since.filter(|_| running)?;
    let gone = Duration::from_millis(awake_ms().saturating_sub(since));
    Some(bisect::ROUND.saturating_sub(gone))
}

/// Sleeps until the running round's end or a change; no round, no timeout.
fn timer(me: Weak<Inner>, wake: Receiver<()>) {
    loop {
        let wait = match me.upgrade() {
            Some(i) => Finder(i).remaining(),
            None => return,
        };
        let got = match wait {
            None => wake.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(d) => wake.recv_timeout(d),
        };
        match got {
            Ok(()) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => match me.upgrade() {
                Some(i) => Finder(i).timer_fine(),
                None => return,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_read_as_the_search_needs() {
        let start = |unclean: bool, since: Option<u64>| closes::Start {
            unclean,
            last_since_ms: since,
            ..Default::default()
        };
        let boot = 10_000_000;
        assert_eq!(exit_kind(true, &start(false, None), boot), Exit::Clean);
        assert_eq!(
            exit_kind(true, &start(true, Some(boot + 60_000)), boot),
            Exit::Unclean,
            "a crash after Windows started"
        );
        assert_eq!(
            exit_kind(true, &start(true, Some(boot - 3_600_000)), boot),
            Exit::Unknown,
            "the PC went down with it"
        );
        assert_eq!(exit_kind(false, &start(false, None), boot), Exit::Unknown);
        // The awake clock moves and Windows started in the past.
        let a = awake_ms();
        assert!(awake_ms() >= a);
        let now = 2_000_000_000_000;
        assert!(boot_ms(now) < now);
    }
}
