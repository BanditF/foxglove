// SPDX-License-Identifier: MIT
//! Finding the module that breaks Hideout (contract §7.10, #1033, docs/hideout-safe-mode-ux.md),
//! with no I/O but the progress file: from safe mode the hub starts half of the modules
//! **Start normally** would start, and halves again by what happened.
//!
//! - **Entries** are those modules, a module that needs another one (`requires`) together with
//!   it, so they are on or off together and count as one ("Lights with Art").
//! - **A round** starts the first half of the suspects (all of them when one is left). It ends
//!   **closed** (the hub closed without a clean exit, or the user says It closed: the culprit is
//!   in the half) or **fine** (5 minutes open, or It's fine: it is in the other half). A clean
//!   Quit pauses the round; an end the hub can't read asks.
//! - **The result:** a suspect on alone in a round that closed is found; when the last suspect
//!   was never on alone in a round that closed, one more round has it alone. Every round fine,
//!   or the last one alone fine: none found. A close while nothing was on: Hideout itself.
//! - "Round k of n": n is the most rounds it can take (⌈log₂ suspects⌉ plus that last round), and
//!   only ever drops.
//!
//! The saved on/off list is never touched here: progress lives in `run\bisect.json`, cleared by
//! the result's choice or Stop finding. The driver (`crate::finder`) starts and stops the
//! rounds' modules and runs the timer.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a round lasts with Hideout open and nothing happening.
pub const ROUND: Duration = Duration::from_secs(5 * 60);

/// A module the search may start: what **Start normally** would start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// The ids its `requires` names.
    pub needs: Vec<String>,
}

/// One entry of the search: a module, or modules that need each other, on or off together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub ids: Vec<String>,
    pub names: Vec<String>,
}

impl Entry {
    /// "Art", "Lights with Art": the modules that need another first.
    pub fn label(&self) -> String {
        match self.names.split_first() {
            None => String::new(),
            Some((first, [])) => first.clone(),
            Some((first, rest)) => format!("{first} with {}", words::and_list(rest)),
        }
    }
}

/// The entries for `cands`: grouped by their provider links among them, A–Z.
pub fn entries(cands: &[Candidate]) -> Vec<Entry> {
    let idx: BTreeMap<&str, usize> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let mut root: Vec<usize> = (0..cands.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for (i, c) in cands.iter().enumerate() {
        for n in &c.needs {
            if let Some(&j) = idx.get(n.as_str()) {
                let (a, b) = (find(&mut root, i), find(&mut root, j));
                root[a] = b;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..cands.len() {
        let r = find(&mut root, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Entry> = groups
        .into_values()
        .map(|mut g| {
            // The modules that need another one in the group first, then A–Z.
            let needy = |i: usize| cands[i].needs.iter().any(|n| idx.contains_key(n.as_str()));
            g.sort_by_key(|&i| (!needy(i), cands[i].name.to_lowercase()));
            Entry {
                ids: g.iter().map(|&i| cands[i].id.clone()).collect(),
                names: g.iter().map(|&i| cands[i].name.clone()).collect(),
            }
        })
        .collect();
    out.sort_by_key(|e| e.label().to_lowercase());
    out
}

/// How the last run ended, as the next start saw it (`run\hub-run.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Quit Hideout, Windows ending the session, Ctrl+C: not counted.
    Clean,
    /// The running mark was still there: a crash, a kill, a restart by itself.
    Unclean,
    /// Can't tell: the file was missing or broken, or Windows started after the run did (the
    /// PC went down with it).
    Unknown,
}

/// Why a round ended (Copy diagnostics only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Why {
    /// The hub closed without a clean exit.
    UncleanExit,
    /// The user said It closed.
    SaidClosed,
    /// 5 minutes with Hideout open.
    FiveMinutes,
    /// The user said It's fine.
    SaidFine,
}

impl Why {
    fn words(self) -> &'static str {
        match self {
            Why::UncleanExit => "the hub closed without a clean exit",
            Why::SaidClosed => "the user said it closed",
            Why::FiveMinutes => "5 minutes open",
            Why::SaidFine => "the user said it was fine",
        }
    }
}

/// One round that ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Round {
    pub k: u32,
    /// The entries that were on (indexes into [`Search::entries`]).
    pub on: Vec<usize>,
    pub closed: bool,
    pub why: Why,
}

/// Where the search is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "phase")]
pub enum Phase {
    /// Round `round` runs with `on` started.
    Running,
    /// Round `round` closed; nothing is on until **Start round k+1**.
    Closed,
    /// Round `round` was paused by a clean Quit; nothing is on until **Go on**.
    Paused,
    /// Hideout closed during round `round` in a way it can't read: did it close by itself?
    Asked,
    /// The result: this entry seems to make Hideout close.
    Found { entry: usize },
    /// The result: Hideout stayed open in every round.
    NotFound,
    /// The result: Hideout closed with no modules on.
    HubItself,
}

/// The search's progress (`run\bisect.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Search {
    pub entries: Vec<Entry>,
    /// The entries that may still be the culprit, in order.
    pub suspects: Vec<usize>,
    /// The current (or last) round's number.
    pub round: u32,
    /// The current (or last) round's entries.
    pub on: Vec<usize>,
    /// The most rounds the search can take; only drops.
    pub bound: u32,
    pub phase: Phase,
    /// The rounds that ended, oldest first.
    pub rounds: Vec<Round>,
    /// The round before this one was fine (the bar says so).
    #[serde(default)]
    pub fine_before: Option<u32>,
    /// Events for Copy diagnostics (closes between rounds, pauses).
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Rounds a search with `s` suspects can still take (halving, then the last one alone).
pub fn rounds_left(s: usize) -> u32 {
    if s <= 1 {
        return 1;
    }
    (usize::BITS - (s - 1).leading_zeros()) + 1
}

impl Search {
    /// A search over `entries`, round 1 running. `None` without entries.
    pub fn new(entries: Vec<Entry>) -> Option<Search> {
        if entries.is_empty() {
            return None;
        }
        let mut s = Search {
            suspects: (0..entries.len()).collect(),
            entries,
            round: 0,
            on: Vec::new(),
            bound: u32::MAX,
            phase: Phase::Running,
            rounds: Vec::new(),
            fine_before: None,
            notes: Vec::new(),
        };
        s.plan();
        Some(s)
    }

    /// The next round: the first half of the suspects, or the last one alone.
    fn plan(&mut self) {
        let k = self.rounds.len() as u32 + 1;
        let n = self.suspects.len();
        self.bound = self.bound.min(k - 1 + rounds_left(n));
        self.round = k;
        self.on = if n == 1 {
            self.suspects.clone()
        } else {
            self.suspects[..n.div_ceil(2)].to_vec()
        };
        self.phase = Phase::Running;
    }

    /// The search still runs (no result yet).
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Running | Phase::Closed | Phase::Paused | Phase::Asked
        )
    }

    /// The running round ended.
    fn ended(&mut self, closed: bool, why: Why) {
        self.rounds.push(Round {
            k: self.round,
            on: self.on.clone(),
            closed,
            why,
        });
        if closed {
            self.suspects = self.on.clone();
            self.fine_before = None;
            self.phase = match self.suspects.as_slice() {
                [only] if self.on.len() == 1 => Phase::Found { entry: *only },
                _ => Phase::Closed,
            };
        } else {
            let on = self.on.clone();
            self.suspects.retain(|e| !on.contains(e));
            if self.suspects.is_empty() {
                self.phase = Phase::NotFound;
            } else {
                self.fine_before = Some(self.round);
                self.plan();
            }
        }
    }

    /// The hub started again: how the last run ended counts for the round it ran.
    pub fn after_exit(&mut self, exit: Exit) {
        match (self.phase, exit) {
            (Phase::Running, Exit::Clean) => {
                self.phase = Phase::Paused;
                self.notes
                    .push(format!("round {}: paused by a clean exit", self.round));
            }
            (Phase::Running, Exit::Unclean) => self.ended(true, Why::UncleanExit),
            (Phase::Running, Exit::Unknown) => {
                self.phase = Phase::Asked;
                self.notes.push(format!(
                    "round {}: the exit couldn't be read; asked",
                    self.round
                ));
            }
            (Phase::Closed | Phase::Paused | Phase::Asked, Exit::Unclean) => {
                self.notes.push(format!(
                    "after round {}: the hub closed without a clean exit with no module on",
                    self.round
                ));
                self.phase = Phase::HubItself;
            }
            _ => {}
        }
    }

    /// **It's fine** (`by_timer`: the 5 minutes passed).
    pub fn fine(&mut self, by_timer: bool) -> bool {
        if self.phase != Phase::Running {
            return false;
        }
        let why = if by_timer {
            Why::FiveMinutes
        } else {
            Why::SaidFine
        };
        self.ended(false, why);
        true
    }

    /// **It closed** (in a round, or answering the question).
    pub fn closed(&mut self) -> bool {
        if !matches!(self.phase, Phase::Running | Phase::Asked) {
            return false;
        }
        self.ended(true, Why::SaidClosed);
        true
    }

    /// **Start round k+1** after a close.
    pub fn start_next(&mut self) -> bool {
        if self.phase != Phase::Closed {
            return false;
        }
        self.fine_before = None;
        self.plan();
        true
    }

    /// **Go on with round k** (paused) or **I closed it** (asked): the same round again.
    pub fn go_on(&mut self) -> bool {
        if !matches!(self.phase, Phase::Paused | Phase::Asked) {
            return false;
        }
        self.phase = Phase::Running;
        true
    }

    /// The module ids on now: the running round's.
    pub fn on_ids(&self) -> BTreeSet<String> {
        if self.phase != Phase::Running {
            return BTreeSet::new();
        }
        self.ids(&self.on)
    }

    /// Every module id in the search.
    pub fn all_ids(&self) -> BTreeSet<String> {
        self.entries.iter().flat_map(|e| e.ids.clone()).collect()
    }

    fn ids(&self, entries: &[usize]) -> BTreeSet<String> {
        entries
            .iter()
            .flat_map(|&e| self.entries[e].ids.clone())
            .collect()
    }

    fn labels(&self, entries: &[usize]) -> Vec<String> {
        entries.iter().map(|&e| self.entries[e].label()).collect()
    }

    /// The rounds that closed with `entry` on.
    pub fn closed_with(&self, entry: usize) -> Vec<u32> {
        self.rounds
            .iter()
            .filter(|r| r.closed && r.on.contains(&entry))
            .map(|r| r.k)
            .collect()
    }

    pub fn path(home: &Path) -> PathBuf {
        home.join("run").join("bisect.json")
    }

    /// The search in progress, if any (a missing or broken file is none).
    pub fn load(home: &Path) -> Option<Search> {
        let text = std::fs::read_to_string(Self::path(home)).ok()?;
        let s: Search = serde_json::from_str(&text).ok()?;
        // A file that doesn't add up is no search.
        let n = s.entries.len();
        let ok = n > 0
            && s.suspects.iter().chain(&s.on).all(|&e| e < n)
            && !matches!(s.phase, Phase::Found { entry } if entry >= n);
        ok.then_some(s)
    }

    /// Writes the file atomically (temp file, then rename).
    pub fn save(&self, home: &Path) -> io::Result<()> {
        let path = Self::path(home);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    /// Ends the search: the file goes.
    pub fn clear(home: &Path) -> io::Result<()> {
        match std::fs::remove_file(Self::path(home)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// A button of the bar, the cards and the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// It's fine.
    Fine,
    /// It closed.
    Closed,
    /// Stop finding.
    Stop,
    /// Start round k+1.
    StartNext,
    /// Go on with round k, I closed it.
    GoOn,
    /// Turn off <Module>.
    TurnOff,
    /// Go back to <Module> <version>.
    GoBack,
    /// Copy diagnostics.
    CopyDiagnostics,
    /// Start normally, Start normally anyway.
    StartNormally,
    /// Try again.
    TryAgain,
}

/// Which card the bar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Round,
    Between,
    Paused,
    Asked,
    Found,
    NotFound,
    HubItself,
}

impl Kind {
    pub fn is_result(self) -> bool {
        matches!(self, Kind::Found | Kind::NotFound | Kind::HubItself)
    }
}

/// What the bar draws, as words (docs/hideout-safe-mode-ux.md, "Wording").
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub kind: Kind,
    /// The bold first line.
    pub title: String,
    pub lines: Vec<String>,
    /// The buttons in order; the first is the primary one.
    pub buttons: Vec<(Act, String)>,
    /// What the polite live region says once when this view appears.
    pub said: String,
    /// Changes with every change of the search (the driver's count): a new `said`, the focus.
    pub seq: u64,
    /// The card takes the focus on its primary button when it appears (not a round's bar).
    pub takes_focus: bool,
    /// A running round: time left of its 5 minutes.
    pub ends_in: Option<Duration>,
    /// The modules in the search that are off now: (id, name).
    pub off_now: Vec<(String, String)>,
}

/// The bar's view of `s`. `ends_in`: the round's time left; `previous`: the version kept of
/// the found module (one-module entry), for Go back.
pub fn view(s: &Search, ends_in: Option<Duration>, previous: Option<&str>) -> View {
    use words as w;
    let total: usize = s.entries.iter().map(|e| e.ids.len()).sum();
    let on_ids = s.on_ids();
    // Off for this round: only while the search runs (not at the result).
    let off_now = s
        .entries
        .iter()
        .flat_map(|e| e.ids.iter().zip(&e.names))
        .filter(|(id, _)| s.active() && !on_ids.contains(*id))
        .map(|(id, n)| (id.clone(), n.clone()))
        .collect();
    let (k, n) = (s.round, s.bound);
    let mut v = View {
        kind: Kind::Round,
        title: String::new(),
        lines: Vec::new(),
        buttons: Vec::new(),
        said: String::new(),
        seq: 0,
        takes_focus: true,
        ends_in: None,
        off_now,
    };
    match s.phase {
        Phase::Running => {
            let labels = s.labels(&s.on);
            v.title = w::title(k, n);
            if let Some(j) = s.fine_before {
                v.lines.push(w::round_fine(j));
            }
            if s.entries.len() == 1 {
                v.lines.push(w::only_on(&labels[0]));
            } else {
                v.lines.push(w::on_line(on_ids.len(), total, &labels));
            }
            v.lines.push(w::USE_AS_USUAL.into());
            if let Some(left) = ends_in {
                v.lines.push(w::ends_in(left));
            }
            v.buttons = vec![
                (Act::Fine, w::ITS_FINE.into()),
                (Act::Closed, w::IT_CLOSED.into()),
                (Act::Stop, w::STOP.into()),
            ];
            let started = w::round_started(k, n, &labels);
            v.said = match s.fine_before {
                Some(j) => format!("{} {started}", w::round_fine(j)),
                None => started,
            };
            v.takes_focus = false;
            v.ends_in = ends_in;
        }
        Phase::Closed => {
            v.kind = Kind::Between;
            v.title = w::closed_title(k);
            v.lines.push(w::narrows(&s.labels(&s.suspects)));
            v.buttons = vec![
                (Act::StartNext, w::start_round(k + 1)),
                (Act::Stop, w::STOP.into()),
            ];
        }
        Phase::Paused => {
            v.kind = Kind::Paused;
            v.title = w::paused(k);
            v.buttons = vec![(Act::GoOn, w::go_on(k)), (Act::Stop, w::STOP.into())];
        }
        Phase::Asked => {
            v.kind = Kind::Asked;
            v.title = w::asked(k);
            v.buttons = vec![
                (Act::Closed, w::IT_CLOSED.into()),
                (Act::GoOn, w::I_CLOSED_IT.into()),
                (Act::Stop, w::STOP.into()),
            ];
        }
        Phase::Found { entry } => {
            let e = &s.entries[entry];
            let m = e.label();
            v.kind = Kind::Found;
            v.title = w::found(&m);
            v.lines.push(w::found_line(&s.closed_with(entry), &m));
            let back = previous.filter(|_| e.ids.len() == 1);
            if let Some(p) = back {
                v.lines.push(w::worked_before(&m, p));
            }
            v.buttons = vec![
                (Act::TurnOff, w::turn_off(&m)),
                (Act::CopyDiagnostics, w::COPY.into()),
                (Act::StartNormally, w::START_ANYWAY.into()),
            ];
            if let Some(p) = back {
                v.buttons.push((Act::GoBack, w::go_back(&m, p)));
            }
        }
        Phase::NotFound => {
            v.kind = Kind::NotFound;
            v.title = w::NOT_FOUND.into();
            v.lines.push(w::NOT_FOUND_LINE.into());
            v.buttons = vec![
                (Act::StartNormally, w::START_NORMALLY.into()),
                (Act::CopyDiagnostics, w::COPY.into()),
                (Act::TryAgain, w::TRY_AGAIN.into()),
            ];
        }
        Phase::HubItself => {
            v.kind = Kind::HubItself;
            v.title = w::HUB_ITSELF.into();
            v.lines.push(w::HUB_ITSELF_LINE.into());
            v.buttons = vec![
                (Act::CopyDiagnostics, w::COPY.into()),
                (Act::StartNormally, w::START_ANYWAY.into()),
            ];
        }
    }
    if v.said.is_empty() {
        v.said = std::iter::once(v.title.clone())
            .chain(v.lines.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
    }
    v
}

/// The tray during the search: the status line (also the tooltip) and the menu's
/// **Start round k+1** (between rounds only) and **Stop finding**.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tray {
    pub line: String,
    pub start_next: Option<String>,
    pub stop: bool,
}

/// The tray's view of `s`; `None` for a result other than found (the safe-mode look).
pub fn tray(s: &Search) -> Option<Tray> {
    use words as w;
    let stop = s.active();
    let (line, start_next) = match s.phase {
        Phase::Running => (w::tray_round(s.round, s.bound), None),
        Phase::Paused | Phase::Asked => (w::tray_paused(s.round), None),
        Phase::Closed => (w::tray_closed(s.round), Some(w::start_round(s.round + 1))),
        Phase::Found { .. } => (w::TRAY_FOUND.into(), None),
        Phase::NotFound | Phase::HubItself => return None,
    };
    Some(Tray {
        line,
        start_next,
        stop,
    })
}

/// The notification when the result is ready and the window is closed: title, body.
pub fn notification(s: &Search) -> Option<(String, String)> {
    use words as w;
    Some(match s.phase {
        Phase::Found { entry } => (w::NOTIFY_FOUND.into(), w::found(&s.entries[entry].label())),
        Phase::NotFound => (w::NOT_FOUND.into(), w::NOT_FOUND_LINE.into()),
        Phase::HubItself => (w::HUB_ITSELF.into(), w::HUB_ITSELF_LINE.into()),
        _ => return None,
    })
}

/// Copy diagnostics' text for the search: entries (ids and names), every round (which entries
/// were on, closed or fine, why), the result. No paths, no user name: ids and words only.
pub fn diagnostics(s: Option<&Search>, hub_version: &str) -> String {
    let mut t = format!("Hideout {hub_version}: finding the module that breaks Hideout\n");
    let Some(s) = s else {
        t.push_str("no search: no module would start\n");
        return t;
    };
    for (i, e) in s.entries.iter().enumerate() {
        t.push_str(&format!(
            "entry {}: {} ({})\n",
            i + 1,
            e.ids.join(", "),
            e.label()
        ));
    }
    let list = |v: &[usize]| {
        v.iter()
            .map(|e| (e + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for r in &s.rounds {
        t.push_str(&format!(
            "round {}: entries {} on; {} ({})\n",
            r.k,
            list(&r.on),
            if r.closed { "closed" } else { "fine" },
            r.why.words()
        ));
    }
    for n in &s.notes {
        t.push_str(&format!("note: {n}\n"));
    }
    let now = match s.phase {
        Phase::Running => format!("round {} running, entries {} on", s.round, list(&s.on)),
        Phase::Closed => format!("round {} closed; waiting for the next round", s.round),
        Phase::Paused => format!("round {} paused", s.round),
        Phase::Asked => format!("round {}: asked whether it closed by itself", s.round),
        Phase::Found { entry } => format!(
            "result: entry {} ({})",
            entry + 1,
            s.entries[entry].ids.join(", ")
        ),
        Phase::NotFound => "result: none found (open in every round)".into(),
        Phase::HubItself => "result: the hub closed with no module on".into(),
    };
    t.push_str(&format!(
        "{now}; suspects {}; at most {} rounds\n",
        list(&s.suspects),
        s.bound
    ));
    t
}

/// Every string of the search a person reads (docs/hideout-safe-mode-ux.md, "Wording"). Never
/// an exit code, a marker file, a state id or "bisect".
pub mod words {
    use std::time::Duration;

    pub use crate::words::{COPY_DIAGNOSTICS as COPY, FIND_CULPRIT as FIND, START_NORMALLY};

    /// The bar's landmark name.
    pub const REGION: &str = "Finding the module that breaks Hideout";
    pub const USE_AS_USUAL: &str = "Use Hideout as usual. If it closes, it'll pick up here.";
    pub const ITS_FINE: &str = "It's fine";
    pub const IT_CLOSED: &str = "It closed";
    pub const STOP: &str = "Stop finding";
    pub const I_CLOSED_IT: &str = "I closed it";
    /// The Modules page's rows during the search.
    pub const ON_IN_ROUND: &str = "On in this round";
    pub const OFF_FOR_ROUND: &str = "Off for this round";
    pub const NOT_IN_SEARCH: &str = "Off \u{b7} not part of the search";
    /// Under the list, and the locked buttons' description.
    pub const LOCKED: &str = "Locked while finding the module. Stop finding to change it.";
    pub const START_ANYWAY: &str = "Start normally anyway";
    pub const TRY_AGAIN: &str = "Try again";
    pub const NOT_FOUND: &str = "Hideout stayed open in every round.";
    pub const NOT_FOUND_LINE: &str =
        "It may need two modules on together, or the problem has passed.";
    pub const HUB_ITSELF: &str = "Hideout closed with no modules on.";
    pub const HUB_ITSELF_LINE: &str = "The problem is in Hideout itself, not a module.";
    /// The safe-mode banner's line after Stop finding.
    pub const STOPPED: &str = "Stopped finding. Nothing was changed.";
    /// The banner's line when no module would start (no link then).
    pub const NONE_ON: &str = "No modules are on, so the problem is in Hideout itself.";
    pub const TRAY_FOUND: &str = "Hideout \u{b7} found the module";
    pub const NOTIFY_FOUND: &str = "Found the module";
    pub const OPEN_HIDEOUT: &str = "Open Hideout";

    pub fn title(k: u32, n: u32) -> String {
        format!("Finding the module that breaks Hideout \u{b7} Round {k} of {n}")
    }

    /// "Lights, Art, Chat", then "… and 2 more".
    pub fn comma_list(names: &[String]) -> String {
        if names.len() <= 3 {
            return names.join(", ");
        }
        format!("{} and {} more", names[..3].join(", "), names.len() - 3)
    }

    fn joined(names: &[String], word: &str) -> String {
        match names {
            [] => String::new(),
            [one] => one.clone(),
            _ if names.len() > 3 => {
                format!("{} {word} {} more", names[..3].join(", "), names.len() - 3)
            }
            [rest @ .., last] => format!("{} {word} {last}", rest.join(", ")),
        }
    }

    /// "Lights, Art and Chat".
    pub fn and_list(names: &[String]) -> String {
        joined(names, "and")
    }

    /// "Lights, Art or Chat".
    pub fn or_list(names: &[String]) -> String {
        joined(names, "or")
    }

    pub fn on_line(a: usize, b: usize, names: &[String]) -> String {
        format!("{a} of {b} modules are on: {}.", comma_list(names))
    }

    /// The one-entry search's line.
    pub fn only_on(name: &str) -> String {
        format!("Only {name} is on.")
    }

    /// "This round ends in 4 min …": whole minutes, rounded up, at least 1.
    pub fn ends_in(left: Duration) -> String {
        let m = left.as_secs().div_ceil(60).max(1);
        format!("This round ends in {m} min if Hideout stays open.")
    }

    pub fn round_fine(k: u32) -> String {
        format!("Round {k} was fine.")
    }

    /// The live region at a round's start.
    pub fn round_started(k: u32, n: u32, names: &[String]) -> String {
        let verb = if names.len() == 1 { "is" } else { "are" };
        format!("Round {k} of {n} started. {} {verb} on.", and_list(names))
    }

    pub fn closed_title(k: u32) -> String {
        format!("Round {k}: Hideout closed.")
    }

    pub fn narrows(names: &[String]) -> String {
        format!("That narrows it to {}.", or_list(names))
    }

    pub fn start_round(k: u32) -> String {
        format!("Start round {k}")
    }

    pub fn paused(k: u32) -> String {
        format!("Round {k} was paused.")
    }

    pub fn go_on(k: u32) -> String {
        format!("Go on with round {k}")
    }

    pub fn asked(k: u32) -> String {
        format!("Hideout closed during round {k}. Did it close by itself?")
    }

    pub fn found(module: &str) -> String {
        format!("{module} seems to make Hideout close.")
    }

    /// "Hideout closed in rounds 1 and 3 with Art on, and stayed open without it."
    pub fn found_line(rounds: &[u32], module: &str) -> String {
        let r: Vec<String> = rounds.iter().map(u32::to_string).collect();
        let which = match r.len() {
            1 => format!("round {}", r[0]),
            _ => format!("rounds {}", and_list(&r)),
        };
        format!("Hideout closed in {which} with {module} on, and stayed open without it.")
    }

    pub fn turn_off(module: &str) -> String {
        format!("Turn off {module}")
    }

    pub fn worked_before(module: &str, version: &str) -> String {
        format!("{module} {version} worked before.")
    }

    pub fn go_back(module: &str, version: &str) -> String {
        format!("Go back to {module} {version}")
    }

    /// The banner's link with one entry in the search.
    pub fn check_one(module: &str) -> String {
        format!("Check whether {module} breaks Hideout")
    }

    pub fn tray_round(k: u32, n: u32) -> String {
        format!("Hideout \u{b7} finding the module (round {k} of {n})")
    }

    /// Nothing is on while paused or asked, so the tray doesn't say "finding" (designer, #1050).
    pub fn tray_paused(k: u32) -> String {
        format!("Hideout \u{b7} round {k} paused")
    }

    pub fn tray_closed(k: u32) -> String {
        format!("Hideout \u{b7} round {k} closed, start round {}", k + 1)
    }

    /// A sidebar entry's accessible name and tooltip for a module off this round.
    pub fn off_for_round(module: &str) -> String {
        format!("{module}, off for this round")
    }

    /// A sample of every string, for the "never shown" checks.
    pub fn every_string() -> Vec<String> {
        let names: Vec<String> = ["Lights", "Art", "Chat", "Dock", "Echo"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut v: Vec<String> = [
            REGION,
            USE_AS_USUAL,
            ITS_FINE,
            IT_CLOSED,
            STOP,
            I_CLOSED_IT,
            ON_IN_ROUND,
            OFF_FOR_ROUND,
            NOT_IN_SEARCH,
            LOCKED,
            START_ANYWAY,
            TRY_AGAIN,
            NOT_FOUND,
            NOT_FOUND_LINE,
            HUB_ITSELF,
            HUB_ITSELF_LINE,
            STOPPED,
            NONE_ON,
            TRAY_FOUND,
            NOTIFY_FOUND,
            OPEN_HIDEOUT,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        v.extend([
            title(2, 4),
            on_line(3, 6, &names[..3]),
            on_line(5, 6, &names),
            only_on("Art"),
            ends_in(Duration::from_secs(239)),
            round_fine(2),
            round_started(2, 4, &names[..3]),
            closed_title(2),
            narrows(&names[..3]),
            start_round(3),
            paused(2),
            go_on(2),
            asked(2),
            found("Art"),
            found_line(&[1, 3], "Art"),
            turn_off("Art"),
            worked_before("Art", "0.4.2"),
            go_back("Art", "0.4.2"),
            check_one("Art"),
            tray_round(2, 4),
            tray_paused(2),
            tray_closed(2),
            off_for_round("Art"),
        ]);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, name: &str, needs: &[&str]) -> Candidate {
        Candidate {
            id: id.into(),
            name: name.into(),
            needs: needs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn six() -> Vec<Entry> {
        let c: Vec<Candidate> = ["Art", "Beat", "Chat", "Dock", "Echo", "Fern"]
            .iter()
            .map(|n| cand(&format!("m.{}", n.to_lowercase()), n, &[]))
            .collect();
        entries(&c)
    }

    /// Runs a search where entry `culprit` (if any) closes the hub whenever it is on.
    fn run(entries: Vec<Entry>, culprit: Option<usize>) -> Search {
        let mut s = Search::new(entries).unwrap();
        let mut guard = 0;
        while s.active() {
            guard += 1;
            assert!(guard < 20, "no end: {s:?}");
            match s.phase {
                Phase::Running => {
                    if culprit.is_some_and(|c| s.on.contains(&c)) {
                        s.after_exit(Exit::Unclean);
                    } else {
                        assert!(s.fine(true));
                    }
                }
                Phase::Closed => assert!(s.start_next()),
                p => panic!("{p:?}"),
            }
        }
        s
    }

    /// #1033 acceptance: with 6 entries and one culprit, every culprit is named within 4
    /// rounds, and "Round k of n" never promised fewer than it took.
    #[test]
    fn six_entries_name_the_culprit_in_at_most_four_rounds() {
        assert_eq!(rounds_left(6), 4);
        for c in 0..6 {
            let s = run(six(), Some(c));
            assert_eq!(s.phase, Phase::Found { entry: c }, "culprit {c}");
            assert!(s.rounds.len() <= 4, "culprit {c}: {:?}", s.rounds);
            assert!(s.bound >= s.rounds.len() as u32 && s.bound <= 4);
            assert!(!s.closed_with(c).is_empty());
        }
    }

    #[test]
    fn every_round_fine_finds_none() {
        let s = run(six(), None);
        assert_eq!(s.phase, Phase::NotFound);
        assert!(s.rounds.iter().all(|r| !r.closed));
        assert!(s.rounds.len() <= 4);
    }

    /// The bound only drops: 6 entries read "of 4", and a lucky first close can make it less.
    #[test]
    fn the_round_count_is_an_upper_bound_that_only_drops() {
        let mut s = Search::new(six()).unwrap();
        assert_eq!((s.round, s.bound), (1, 4));
        assert_eq!(s.on, vec![0, 1, 2]);
        let mut seen = vec![s.bound];
        s.fine(false);
        seen.push(s.bound);
        s.closed();
        s.start_next();
        seen.push(s.bound);
        assert!(seen.windows(2).all(|w| w[1] <= w[0]), "{seen:?}");
        assert_eq!(s.round, 3);
    }

    #[test]
    fn one_entry_has_one_round() {
        let e = entries(&[cand("m.art", "Art", &[])]);
        let s = Search::new(e).unwrap();
        assert_eq!((s.round, s.bound), (1, 1));
        let v = view(&s, Some(ROUND), None);
        assert_eq!(v.title, words::title(1, 1));
        assert_eq!(v.lines[0], "Only Art is on.");
        let mut found = s.clone();
        found.after_exit(Exit::Unclean);
        assert_eq!(found.phase, Phase::Found { entry: 0 });
        let mut fine = s;
        fine.fine(true);
        assert_eq!(fine.phase, Phase::NotFound);
    }

    #[test]
    fn modules_that_need_another_go_together() {
        let e = entries(&[
            cand("m.art", "Art", &[]),
            cand("m.lights", "Lights", &["m.art"]),
            cand("m.chat", "Chat", &["m.gone"]),
        ]);
        let labels: Vec<String> = e.iter().map(Entry::label).collect();
        assert_eq!(labels, ["Chat", "Lights with Art"]);
        assert_eq!(e[1].ids, ["m.lights", "m.art"]);
    }

    /// A clean Quit pauses (Go on runs the same round), an unreadable end asks, a close
    /// between rounds is Hideout itself.
    #[test]
    fn exits_pause_ask_or_blame_the_hub() {
        let mut s = Search::new(six()).unwrap();
        s.after_exit(Exit::Clean);
        assert_eq!(s.phase, Phase::Paused);
        assert!(s.on_ids().is_empty(), "nothing on while paused");
        assert_eq!(tray(&s).unwrap().line, "Hideout \u{b7} round 1 paused");
        assert!(s.go_on());
        assert_eq!(
            (s.phase, s.round, s.on.clone()),
            (Phase::Running, 1, vec![0, 1, 2])
        );
        s.after_exit(Exit::Unknown);
        assert_eq!(s.phase, Phase::Asked);
        let mut said_mine = s.clone();
        assert!(said_mine.go_on(), "I closed it");
        assert_eq!(said_mine.phase, Phase::Running);
        assert!(said_mine.rounds.is_empty(), "not counted");
        assert!(s.closed(), "It closed");
        assert_eq!(s.phase, Phase::Closed);
        assert_eq!(s.suspects, vec![0, 1, 2]);
        s.after_exit(Exit::Clean);
        assert_eq!(
            s.phase,
            Phase::Closed,
            "a quit between rounds changes nothing"
        );
        s.after_exit(Exit::Unclean);
        assert_eq!(s.phase, Phase::HubItself);
        assert!(!s.active());
        assert!(!s.start_next() && !s.fine(false) && !s.closed());
    }

    #[test]
    fn the_file_round_trips_and_a_broken_one_is_no_search() {
        let home = std::env::temp_dir().join(format!("hideout-bisect-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(Search::load(&home), None);
        let mut s = Search::new(six()).unwrap();
        s.fine(false);
        s.save(&home).unwrap();
        assert_eq!(Search::load(&home), Some(s));
        std::fs::write(Search::path(&home), "{\"entries\": []}").unwrap();
        assert_eq!(Search::load(&home), None);
        Search::clear(&home).unwrap();
        Search::clear(&home).unwrap();
        assert!(!Search::path(&home).exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The wording table, word for word.
    #[test]
    fn the_bar_says_what_the_ux_plan_says() {
        let mut s = Search::new(six()).unwrap();
        s.fine(true);
        let v = view(&s, Some(Duration::from_secs(239)), None);
        assert_eq!(
            v.title,
            "Finding the module that breaks Hideout \u{b7} Round 2 of 4"
        );
        assert_eq!(
            v.lines,
            [
                "Round 1 was fine.",
                "2 of 6 modules are on: Dock, Echo.",
                "Use Hideout as usual. If it closes, it'll pick up here.",
                "This round ends in 4 min if Hideout stays open.",
            ]
        );
        let b: Vec<&str> = v.buttons.iter().map(|b| b.1.as_str()).collect();
        assert_eq!(b, ["It's fine", "It closed", "Stop finding"]);
        assert_eq!(
            v.said,
            "Round 1 was fine. Round 2 of 4 started. Dock and Echo are on."
        );
        assert!(!v.takes_focus);
        let off: Vec<&str> = v.off_now.iter().map(|o| o.1.as_str()).collect();
        assert_eq!(off, ["Art", "Beat", "Chat", "Fern"]);
        assert_eq!(
            words::on_line(
                5,
                6,
                &["Lights", "Art", "Chat", "Dock", "Echo"].map(String::from)
            ),
            "5 of 6 modules are on: Lights, Art, Chat and 2 more."
        );
        s.closed();
        let v = view(&s, None, None);
        assert_eq!(v.title, "Round 2: Hideout closed.");
        assert_eq!(v.lines, ["That narrows it to Dock or Echo."]);
        assert_eq!(v.buttons[0], (Act::StartNext, "Start round 3".to_string()));
        assert!(v.takes_focus);
        assert_eq!(
            tray(&s).unwrap(),
            Tray {
                line: "Hideout \u{b7} round 2 closed, start round 3".into(),
                start_next: Some("Start round 3".into()),
                stop: true
            }
        );
        s.start_next();
        assert_eq!(
            tray(&s).unwrap().line,
            "Hideout \u{b7} finding the module (round 3 of 4)"
        );
        s.after_exit(Exit::Unclean);
        let v = view(&s, None, Some("0.4.2"));
        assert_eq!(v.kind, Kind::Found);
        assert!(
            v.off_now.is_empty(),
            "nothing is off for a round at the result"
        );
        assert_eq!(v.title, "Dock seems to make Hideout close.");
        assert_eq!(
            v.lines,
            [
                "Hideout closed in rounds 2 and 3 with Dock on, and stayed open without it.",
                "Dock 0.4.2 worked before."
            ]
        );
        let b: Vec<&str> = v.buttons.iter().map(|b| b.1.as_str()).collect();
        assert_eq!(
            b,
            [
                "Turn off Dock",
                "Copy diagnostics",
                "Start normally anyway",
                "Go back to Dock 0.4.2"
            ]
        );
        assert_eq!(tray(&s).unwrap().line, "Hideout \u{b7} found the module");
        assert_eq!(
            notification(&s).unwrap(),
            (
                "Found the module".to_string(),
                "Dock seems to make Hideout close.".to_string()
            )
        );
        let mut p = Search::new(six()).unwrap();
        p.after_exit(Exit::Clean);
        let v = view(&p, None, None);
        assert_eq!(v.title, "Round 1 was paused.");
        assert_eq!(v.buttons[0].1, "Go on with round 1");
        p.after_exit(Exit::Unknown);
        p.go_on();
        p.after_exit(Exit::Unknown);
        let v = view(&p, None, None);
        assert_eq!(
            v.title,
            "Hideout closed during round 1. Did it close by itself?"
        );
        let b: Vec<&str> = v.buttons.iter().map(|b| b.1.as_str()).collect();
        assert_eq!(b, ["It closed", "I closed it", "Stop finding"]);
        let v = view(&run(six(), None), None, None);
        assert_eq!(
            (v.title.as_str(), v.lines[0].as_str()),
            (
                "Hideout stayed open in every round.",
                "It may need two modules on together, or the problem has passed."
            )
        );
        let b: Vec<&str> = v.buttons.iter().map(|b| b.1.as_str()).collect();
        assert_eq!(b, ["Start normally", "Copy diagnostics", "Try again"]);
        assert_eq!(
            words::found_line(&[3], "Art"),
            "Hideout closed in round 3 with Art on, and stayed open without it."
        );
    }

    /// No exit code, path, state id or "bisect" in anything the bar, the tray or the result
    /// shows (#1033 acceptance).
    #[test]
    fn no_codes_paths_or_ids_are_shown() {
        let mut drawn = words::every_string();
        let mut s = Search::new(six()).unwrap();
        for step in 0..6 {
            let v = view(&s, Some(ROUND), Some("0.4.2"));
            drawn.push(v.title.clone());
            drawn.extend(v.lines.clone());
            drawn.extend(v.buttons.iter().map(|b| b.1.clone()));
            drawn.push(v.said.clone());
            drawn.extend(
                tray(&s)
                    .into_iter()
                    .flat_map(|t| [Some(t.line), t.start_next])
                    .flatten(),
            );
            match step {
                0 => s.after_exit(Exit::Clean),
                1 => {
                    s.go_on();
                    s.after_exit(Exit::Unclean);
                }
                2 => {
                    s.start_next();
                }
                3 => s.after_exit(Exit::Unknown),
                _ => {
                    s.closed();
                }
            }
        }
        for s in drawn {
            let low = s.to_lowercase();
            for never in crate::words::NEVER_SHOWN
                .iter()
                .copied()
                .chain(["bisect", "\\", ".json", "m.", "0x"])
            {
                assert!(!low.contains(&never.to_lowercase()), "{never:?} in {s:?}");
            }
        }
    }

    /// The #906 transcript `transcripts/hub/bisect.json` (prose steps) against the model: the
    /// rounds' halves, the close, the 5 minutes, the culprit and the rounds named.
    #[test]
    fn the_bisect_transcript_holds() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/hideout-vectors/transcripts/hub/bisect.json"
        );
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let expects: Vec<&serde_json::Value> = doc["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s.get("expect"))
            .collect();
        let ids = |v: &serde_json::Value| -> Vec<usize> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| (x.as_str().unwrap().as_bytes()[0] - b'a') as usize)
                .collect()
        };
        let c: Vec<Candidate> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| cand(n, n, &[]))
            .collect();
        let mut s = Search::new(entries(&c)).unwrap();
        assert_eq!(
            (s.round, s.bound),
            (1, expects[0]["of"].as_u64().unwrap() as u32)
        );
        assert_eq!(s.on, ids(&expects[0]["on"]));
        s.after_exit(Exit::Unclean);
        assert_eq!(s.phase, Phase::Closed);
        assert_eq!(s.suspects, ids(&expects[1]["suspects"]));
        assert!(s.on_ids().is_empty());
        s.start_next();
        assert_eq!(s.on, ids(&expects[2]["on"]));
        s.fine(true);
        assert_eq!(s.round, 3);
        assert_eq!(s.on, ids(&expects[3]["on"]));
        s.after_exit(Exit::Unclean);
        let culprit = expects[4]["culprit"].as_str().unwrap();
        assert_eq!(
            s.phase,
            Phase::Found {
                entry: (culprit.as_bytes()[0] - b'a') as usize
            }
        );
        let rounds: Vec<u32> = expects[4]["closed_in_rounds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_u64().unwrap() as u32)
            .collect();
        assert_eq!(s.closed_with(2), rounds);
    }

    #[test]
    fn diagnostics_list_rounds_and_hold_no_paths() {
        let s = run(six(), Some(4));
        let d = diagnostics(Some(&s), "0.1.0");
        assert!(
            d.contains("round 1: entries 1, 2, 3 on; fine (5 minutes open)"),
            "{d}"
        );
        assert!(d.contains("result: entry 5 (m.echo)"), "{d}");
        assert!(!d.contains('\\') && !d.contains(":/"), "{d}");
        assert!(diagnostics(None, "0.1.0").contains("no search"));
    }
}
