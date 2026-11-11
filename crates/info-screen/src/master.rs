//! Software master volume (#1064, UX plan `docs/knob-exclude-ux.md`, "Master volume on a device
//! that ignores Windows volume"). Some output devices take Windows' master volume into their own
//! hardware (`QueryHardwareSupport` has the VOLUME flag) and sometimes don't apply it: the number
//! moves, the sound stays. Windows has no supported way to attenuate such a device itself, so
//! while this is on for a device Foxglove:
//!
//! - **holds the device's own level one key step below the top** ([`hold_for`]), so the device
//!   never turns the sound down twice whether it honours its volume or not (a step costs at most
//!   the device's smallest step, and only on a device that honours it);
//! - keeps the real master as a level of its own, and applies it as a factor ([`amp`], Windows'
//!   endpoint taper) on every app on that device, new ones as they appear;
//! - stores each app's own level and writes it back exactly ([`Record`]), and writes a
//!   [`Journal`] before every change, in an order that fails quieter, never louder;
//! - takes a change from outside (Windows' flyout, the volume keys) as the new master and holds
//!   the device again in the handling of that same notification. Whole key steps from the level
//!   Windows last reported are key presses ([`Soft::endpoint_event`]): that is why the hold is
//!   one step below the top (at the top, "volume up" sends no event at all). They are applied
//!   once no other change follows for [`KEY_SETTLE`] (a drag's first sample can look like one).
//!   A device whose keys move less than the step it reports is stepped by what its keys move.
//!   Anything else is a drag on Windows' slider, applied once it settles, and a drag down never
//!   raises master;
//! - mutes master by muting the device and the apps, marked, and unmutes only the marked ones.
//!   Stopped while muted, the marked apps stay muted until the device is unmuted (a device that
//!   ignores its volume may ignore its mute too, so unmuting them then would be louder);
//! - takes an app's own write (a game's slider) as that app's own level and scales it again, at
//!   most every [`COALESCE`]; an app that puts its own level back after Foxglove scaled it more
//!   than [`FIGHT_WRITES`] times in [`FIGHT_WINDOW`] is left alone.
//!
//! The pure core ([`Soft`]) runs over [`Audio`] (Windows' endpoint and sessions, or a test's) and
//! a [`Host`] (where the journal and the per-device choice live). Polling: none. Every call is
//! one event's handling; the only waits are one-shot deadlines ([`Soft::next_due`]): an app's
//! coalesced re-apply (started by its write), a key press's settle and a slider drag's settle
//! (started by the device's notification), each ended by the deadline or the next event.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// `QueryHardwareSupport`'s VOLUME flag (ENDPOINT_HARDWARE_SUPPORT_VOLUME).
pub const HARDWARE_VOLUME: u32 = 0x1;
/// Two levels this close are one (Windows keeps them as f32; a write reads back within this).
pub const SAME: f32 = 0.005;
/// An app's own writes are scaled again at most this often.
pub const COALESCE: Duration = Duration::from_millis(250);
/// More than this many of an app's own writes in [`FIGHT_WINDOW`]: the app sets its own volume,
/// and Foxglove leaves it alone.
pub const FIGHT_WRITES: usize = 10;
pub const FIGHT_WINDOW: Duration = Duration::from_secs(10);
/// A closed app's record is kept this long (Windows starts it at its saved, scaled level).
pub const KEEP_DAYS: i64 = 30;
/// Windows' volume key step when the device doesn't say (2 %, 51 steps).
pub const DEFAULT_STEP: f32 = 0.02;
/// The key step is kept within these (a 2-step device would hold at 0; a 1000-step one would
/// have steps too small to tell from a drag).
pub const MIN_STEP: f32 = 0.01;
pub const MAX_STEP: f32 = 0.10;
/// A drag on Windows' slider is applied once it has been still this long.
pub const DRAG_SETTLE: Duration = Duration::from_millis(500);
/// A key press is applied once no other change of the device has followed it for this long: a
/// drag on Windows' slider sends its next sample sooner, and a key's own repeat starts later
/// (Windows' shortest repeat delay is 250 ms).
pub const KEY_SETTLE: Duration = Duration::from_millis(120);
/// Presses in a row of one smaller size before it becomes a coarse device's key step.
pub const LEARN_PRESSES: u32 = 2;
/// Most key steps one notification can carry (a key repeat that outran Foxglove's hold).
pub const MAX_KEY_STEPS: f32 = 3.0;
/// An app's write counts towards [`FIGHT_WRITES`] only when it puts back its own level this soon
/// after Foxglove scaled it (a fight); a Volume-mixer drag of its slider never does.
pub const FIGHT_GAP: Duration = Duration::from_secs(2);

/// Windows' endpoint taper, in dB for a slider level (0.0-1.0): -10 dB per halving of the
/// slider. Measured on the dongle: 35 % is -15.42 dB (this gives -15.15). `-inf` at 0.
pub fn db(level: f32) -> f32 {
    if level <= 0.0 {
        f32::NEG_INFINITY
    } else {
        10.0 * level.min(1.0).log2()
    }
}

/// The amplitude factor for a master level (0.0-1.0) on Windows' taper: 0 is silent, 1 is 1.
/// App levels (ISimpleAudioVolume) are linear amplitude, so `own * amp(master)` sounds like
/// master at that level on a device that applies it.
pub fn amp(level: f32) -> f32 {
    if level <= 0.0 {
        0.0
    } else if level >= 1.0 {
        1.0
    } else {
        10f32.powf(db(level) / 20.0)
    }
}

/// The key step for a device with `count` volume steps (`GetVolumeStepInfo`).
pub fn step_for(count: u32) -> f32 {
    if count > 1 {
        (1.0 / (count - 1) as f32).clamp(MIN_STEP, MAX_STEP)
    } else {
        DEFAULT_STEP
    }
}

/// Where the device's own level is held: one key step below the top.
pub fn hold_for(step: f32) -> f32 {
    (1.0 - step).clamp(0.0, 1.0)
}

/// Whether Windows hands this device's volume to the hardware (`QueryHardwareSupport`).
pub fn eligible(hardware_support: u32) -> bool {
    hardware_support & HARDWARE_VOLUME != 0
}

/// Whether one of the device's hardware ids (its own and its parents') starts with one of the
/// data list's prefixes, in any case.
pub fn listed(ids: &[String], prefixes: &[String]) -> bool {
    ids.iter().any(|id| {
        prefixes.iter().any(|p| {
            !p.is_empty()
                && id
                    .get(..p.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(p))
        })
    })
}

/// Whether Foxglove sets the device's volume: only when Windows hands it to the hardware; then
/// the user's choice, else on for a listed device.
pub fn turned_on(eligible: bool, listed: bool, choice: Option<bool>) -> bool {
    eligible && choice.unwrap_or(listed)
}

/// One session on the device, as Windows says it is now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    /// The session instance id (unique while it lives).
    pub id: String,
    /// The session identifier: one per app and device, the same after the app restarts (the key
    /// Windows saves the app's level by).
    pub sid: String,
    pub level: f32,
    pub muted: bool,
}

/// Windows' device and sessions, as the core needs them. Every write is marked as Foxglove's
/// own, so its notification is ignored.
pub trait Audio {
    /// The device's own level (0.0-1.0) and mute.
    fn endpoint(&self) -> Result<(f32, bool), String>;
    fn set_endpoint(&mut self, level: f32) -> Result<(), String>;
    fn set_endpoint_mute(&mut self, muted: bool) -> Result<(), String>;
    /// Every session on the device (any state but expired).
    fn sessions(&self) -> Vec<Live>;
    fn set_level(&mut self, id: &str, level: f32) -> Result<(), String>;
    fn set_mute(&mut self, id: &str, muted: bool) -> Result<(), String>;
}

/// The device Foxglove is setting the volume of, as of the last write (crash recovery).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Active {
    /// The endpoint id.
    pub endpoint: String,
    /// Master's level and mute.
    pub level: f32,
    pub muted: bool,
    /// The level the device is held at.
    pub hold: f32,
}

/// One app's levels (by its session identifier): its own, and what Foxglove last wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub own: f32,
    pub applied: f32,
    /// Muted by master's mute (unmuted by master's unmute).
    #[serde(default)]
    pub by_master: bool,
    /// Days since 1970 it was last written (kept [`KEEP_DAYS`]).
    #[serde(default)]
    pub day: i64,
}

/// The write-ahead journal: written before every change, so a crash can be undone exactly.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    /// The devices Foxglove set the volume of, by endpoint id: one record each, so a second
    /// device never overwrites the first one's crash record.
    #[serde(default)]
    pub devices: BTreeMap<String, Active>,
    /// App records by session identifier: the live apps' and closed apps' (Windows starts a
    /// closed app at the scaled level it saved).
    #[serde(default)]
    pub apps: BTreeMap<String, Record>,
}

impl Journal {
    /// Nothing to keep: the file goes.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty() && self.apps.is_empty()
    }
}

/// Where the journal and the per-device facts live (the app's config folder; a test's memory).
pub trait Host: Send {
    /// The journal as last written (`Ok(None)`: none). `Err`: there is one, but it can't be read
    /// (it is kept, and never written over while so).
    fn journal(&self) -> Result<Option<Journal>, String>;
    /// Writes it (an empty one removes the file).
    fn write_journal(&mut self, journal: &Journal) -> Result<(), String>;
    /// The user's choice for a device (`None`: the default).
    fn choice(&self, endpoint: &str) -> Option<bool>;
    /// Whether a device with these hardware ids is in the data list.
    fn listed(&self, ids: &[String]) -> bool;
    /// Master's level and mute for a device, as last remembered.
    fn saved(&self, endpoint: &str) -> Option<(f32, bool)>;
    /// Remembers them (on engage and disengage) with the device's Windows name.
    fn remember(&mut self, endpoint: &str, name: &str, level: f32, muted: bool);
}

/// The default output device, as the Knob card's MASTER VOLUME part shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    /// The endpoint id (the per-device setting's key).
    pub endpoint: String,
    /// Windows' name for it ("Headphones (Xbox Controller)").
    pub name: String,
    /// Its device tree's instance and hardware ids (the data list's match).
    pub ids: Vec<String>,
    /// Windows hands its volume to the hardware.
    pub eligible: bool,
    /// In the data list.
    pub listed: bool,
    /// The user's choice (`None`: the default).
    pub choice: Option<bool>,
    /// Foxglove sets its volume now.
    pub on: bool,
    /// On, but not started: the journal on disk can't be read (#1070). The setting says so.
    pub paused: bool,
    /// Windows lets apps take exclusive control of it (`None`: can't tell).
    pub exclusive: Option<bool>,
    /// Its volume key step.
    pub step: f32,
}

static DEVICE: std::sync::Mutex<Option<Device>> = std::sync::Mutex::new(None);
static MASTER: std::sync::Mutex<Option<(u8, bool)>> = std::sync::Mutex::new(None);
static RESTORED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Apps put back from a crashed run's journal during this run (the health view counts them).
pub fn restored() -> u32 {
    RESTORED.load(std::sync::atomic::Ordering::Relaxed)
}

fn note_restored(n: usize) {
    RESTORED.fetch_add(n as u32, std::sync::atomic::Ordering::Relaxed);
}

/// The default output device as last read (the sessions' thread publishes it).
pub fn device() -> Option<Device> {
    DEVICE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn publish_device(d: Option<Device>) {
    *DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = d;
}

/// Master's level and mute while Foxglove sets the device's volume (the left screen's volume
/// bar shows it rather than the held device level); `None` otherwise.
pub fn handled_master() -> Option<(u8, bool)> {
    *MASTER.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn publish_master(m: Option<(u8, bool)>) {
    *MASTER.lock().unwrap_or_else(|e| e.into_inner()) = m;
}

/// What the knob's tray row says of an app on a handled device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppVolume {
    /// Its own level, what Windows' Volume mixer shows, and master's level (percent).
    Scaled { own: u8, applied: u8, master: u8 },
    /// It sets its own volume: master doesn't change it.
    SelfManaged,
}

#[derive(Clone, Debug)]
struct App {
    sid: String,
    own: f32,
    applied: f32,
    by_master: bool,
    self_managed: bool,
    /// Its own writes in the last [`FIGHT_WINDOW`].
    writes: VecDeque<Instant>,
    /// When Foxglove last scaled it, and a re-apply waiting for [`COALESCE`].
    last: Option<Instant>,
    due: Option<Instant>,
}

/// The device Foxglove sets the volume of now.
#[derive(Clone, Debug)]
struct On {
    endpoint: String,
    name: String,
    level: f32,
    muted: bool,
    step: f32,
    hold: f32,
    /// The device's level as Windows last said it (its events, Foxglove's own included): key
    /// steps are counted from here.
    seen: f32,
    /// Key presses waiting for [`KEY_SETTLE`]: applied at their deadline.
    press: Option<Press>,
    /// A key step smaller than the device reports, and how many presses in a row moved it: it
    /// becomes the device's step at [`LEARN_PRESSES`] (one slider click never does).
    candidate: Option<(f32, u32)>,
    /// A drag on Windows' slider in progress: applied at its deadline.
    drag: Option<Drag>,
    apps: HashMap<String, App>,
}

/// Key presses not applied yet: where they started, how many steps of `step` they moved, and
/// when they are applied.
#[derive(Clone, Copy, Debug)]
struct Press {
    start: f32,
    steps: f32,
    step: f32,
    due: Instant,
}

/// A drag on Windows' slider: where it started and where it is now.
#[derive(Clone, Copy, Debug)]
struct Drag {
    start: f32,
    to: f32,
    due: Instant,
}

impl On {
    /// How close two device levels must be to be one, for this device's step.
    fn tolerance(&self) -> f32 {
        tolerance(self.step)
    }

    fn factor(&self) -> f32 {
        amp(self.level)
    }
}

/// How close two device levels must be to be one, for a key step of `step`.
fn tolerance(step: f32) -> f32 {
    (step / 4.0).min(SAME)
}

/// The whole number of key steps of `step` a change of `off` is (up to [`MAX_KEY_STEPS`]).
fn key_steps(off: f32, step: f32) -> Option<f32> {
    let n = (off / step).round();
    (n != 0.0 && n.abs() <= MAX_KEY_STEPS && (off - n * step).abs() <= tolerance(step)).then_some(n)
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= SAME
}

fn percent(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// `level` moved by `steps` of 2 %, on Windows' grid, within 0-100 % (as `sessions::step_level`).
fn step_level(level: f32, steps: i32) -> f32 {
    let p = (level * 100.0).round() as i32 + steps * 2;
    p.clamp(0, 100) as f32 / 100.0
}

/// The software master (see the module's docs): the journal, and the device it is on.
pub struct Soft {
    host: Box<dyn Host>,
    journal: Journal,
    on: Option<On>,
    /// Today, in days since 1970 (records' age).
    today: i64,
    /// The journal's file is there but can't be read (why): it is kept, nothing writes over it,
    /// and nothing starts until a later read works.
    unread: Option<String>,
}

impl Soft {
    /// Reads the journal; drops closed apps' records older than [`KEEP_DAYS`]. A journal that
    /// can't be read is not "no journal": it is kept, and read again before anything is written.
    pub fn new(host: Box<dyn Host>, today: i64) -> Soft {
        let mut s = Soft {
            host,
            journal: Journal::default(),
            on: None,
            today,
            unread: Some(String::new()),
        };
        s.readable();
        s
    }

    /// Whether the journal has been read (reads it again if it couldn't be before).
    fn readable(&mut self) -> bool {
        let Some(before) = self.unread.take() else {
            return true;
        };
        match self.host.journal() {
            Ok(j) => {
                let mut j = j.unwrap_or_default();
                let today = self.today;
                j.apps.retain(|_, r| today - r.day <= KEEP_DAYS);
                self.journal = j;
                if !before.is_empty() {
                    eprintln!("Foxglove: master volume: the journal can be read again");
                }
                true
            }
            Err(e) => {
                if before.is_empty() {
                    eprintln!(
                        "Foxglove: master volume: the journal can't be read ({e}); it is kept \
                         and not written over, and master volume waits until it can be read"
                    );
                }
                self.unread = Some(e);
                false
            }
        }
    }

    /// Whether the journal on disk couldn't be read (tests, the log).
    pub fn journal_unread(&self) -> bool {
        self.unread.is_some()
    }

    pub fn set_today(&mut self, today: i64) {
        self.today = today;
    }

    pub fn host(&self) -> &dyn Host {
        &*self.host
    }

    /// The journal as kept now (tests, the log).
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// The endpoint Foxglove sets the volume of now.
    pub fn engaged(&self) -> Option<&str> {
        self.on.as_ref().map(|o| o.endpoint.as_str())
    }

    /// Master's level and mute (percent) while engaged.
    pub fn master(&self) -> Option<(u8, bool)> {
        self.on.as_ref().map(|o| (percent(o.level), o.muted))
    }

    /// Master's level (0.0-1.0) while engaged.
    pub fn level(&self) -> Option<f32> {
        self.on.as_ref().map(|o| o.level)
    }

    /// An app's own level (0.0-1.0) while engaged.
    pub fn own(&self, id: &str) -> Option<f32> {
        self.on.as_ref()?.apps.get(id).map(|a| a.own)
    }

    /// What the tray row says of an app while engaged.
    pub fn app_volume(&self, id: &str) -> Option<AppVolume> {
        let on = self.on.as_ref()?;
        let a = on.apps.get(id)?;
        Some(if a.self_managed {
            AppVolume::SelfManaged
        } else {
            AppVolume::Scaled {
                own: percent(a.own),
                applied: percent(a.applied),
                master: percent(on.level),
            }
        })
    }

    fn save_journal(&mut self) -> Result<(), String> {
        if !self.readable() {
            return Err("the journal on disk can't be read, so it isn't written over".into());
        }
        let r = self.host.write_journal(&self.journal);
        if let Err(e) = &r {
            eprintln!("Foxglove: master volume: the journal can't be written: {e}");
        }
        r
    }

    /// Notes `on`'s state in the journal (master's and each live app's), and writes it.
    fn journal_on(&mut self) -> Result<(), String> {
        if let Some(on) = &self.on {
            self.journal.devices.insert(
                on.endpoint.clone(),
                Active {
                    endpoint: on.endpoint.clone(),
                    level: on.level,
                    muted: on.muted,
                    hold: on.hold,
                },
            );
            for a in on.apps.values() {
                self.journal.apps.insert(
                    a.sid.clone(),
                    Record {
                        own: a.own,
                        applied: a.applied,
                        by_master: a.by_master,
                        day: self.today,
                    },
                );
            }
        }
        self.save_journal()
    }

    /// An app's own level from its record: the record's when Windows still has what Foxglove
    /// wrote (it is ours), else the level it has now (someone set it since).
    fn own_of(&self, s: &Live) -> (f32, Option<&Record>) {
        match self.journal.apps.get(&s.sid) {
            Some(r) if near(s.level, r.applied) => (r.own, Some(r)),
            _ => (s.level, None),
        }
    }

    /// Starts setting the device's volume (the setting is on and Windows hands the device's
    /// volume to the hardware). Journal first, then every app scaled, then the device held: a
    /// failure before the hold leaves the apps quieter (and puts them back), never louder.
    /// After a crash the journal gives master's level and the apps' own levels back.
    pub fn engage(
        &mut self,
        audio: &mut dyn Audio,
        endpoint: &str,
        name: &str,
        step: f32,
    ) -> Result<(), String> {
        if self.on.as_ref().is_some_and(|o| o.endpoint == endpoint) {
            return Ok(());
        }
        if !self.readable() {
            return Err("the journal can't be read, so not started (it is kept)".into());
        }
        let (scalar, muted) = audio.endpoint()?;
        let hold = hold_for(step);
        let crash = self.journal.devices.get(endpoint).cloned();
        // At the hold, the level is Foxglove's own (from the crash's journal, else remembered);
        // anywhere else someone set it meanwhile, and that is master now.
        let at_hold = near(scalar, hold) || crash.as_ref().is_some_and(|a| near(scalar, a.hold));
        let level = match (&crash, at_hold) {
            (Some(a), true) => a.level,
            (None, true) => self.host.saved(endpoint).map_or(scalar, |s| s.0),
            (_, false) => scalar,
        };
        let mut on = On {
            endpoint: endpoint.to_string(),
            name: name.to_string(),
            level,
            muted,
            step,
            hold,
            seen: scalar,
            press: None,
            candidate: None,
            drag: None,
            apps: HashMap::new(),
        };
        let factor = on.factor();
        let live = audio.sessions();
        let mut restored = 0usize;
        // The apps master muted before the crash, and still muted: unmuted now if master isn't.
        let mut unmute: Vec<String> = Vec::new();
        for s in &live {
            let (own, rec) = self.own_of(s);
            if rec.is_some() {
                restored += 1;
            }
            let was_by_master = rec.is_some_and(|r| r.by_master) && s.muted;
            if was_by_master && !muted {
                unmute.push(s.id.clone());
            }
            on.apps.insert(
                s.id.clone(),
                App {
                    sid: s.sid.clone(),
                    own,
                    applied: own * factor,
                    by_master: was_by_master && muted,
                    self_managed: false,
                    writes: VecDeque::new(),
                    last: None,
                    due: None,
                },
            );
        }
        self.on = Some(on);
        if let Err(e) = self.journal_on() {
            self.on = None;
            return Err(format!("no journal, so not started: {e}"));
        }
        // The apps, then the device.
        let mut written: Vec<(String, f32)> = Vec::new();
        let mut failed: Option<String> = None;
        for s in &live {
            let applied = self
                .on
                .as_ref()
                .and_then(|o| o.apps.get(&s.id))
                .map(|a| a.applied);
            let Some(applied) = applied else { continue };
            match audio.set_level(&s.id, applied) {
                Ok(()) => written.push((s.id.clone(), s.level)),
                // An app that ended meanwhile is left out; the others go on.
                Err(_) if !audio.sessions().iter().any(|x| x.id == s.id) => {
                    if let Some(on) = self.on.as_mut() {
                        on.apps.remove(&s.id);
                    }
                }
                // One still playing that can't be scaled: holding the device would make it
                // louder on a device that honours its volume, so nothing starts.
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        if failed.is_none() {
            if let Err(e) = audio.set_endpoint(hold) {
                failed = Some(e);
            }
        }
        if let Some(e) = failed {
            // Back as it was: each app at its level before (a failed write leaves it quieter).
            for (id, before) in written {
                let _ = audio.set_level(&id, before);
            }
            self.on = None;
            self.journal.devices.remove(endpoint);
            for s in &live {
                if let Some(r) = self.journal.apps.get_mut(&s.sid) {
                    r.applied = s.level;
                }
            }
            let _ = self.save_journal();
            return Err(format!("not started: {e}"));
        }
        // Master's mute over the apps, after the levels.
        if muted {
            self.mute_apps(audio, true);
        }
        for id in unmute {
            let _ = audio.set_mute(&id, false);
        }
        if crash.is_some() {
            note_restored(restored);
            eprintln!(
                "Foxglove: master volume: restored {restored} apps from the last run's journal"
            );
        }
        eprintln!(
            "Foxglove: master volume: on for {name}: master {} %{}, the device held at {} %",
            percent(level),
            if muted { " (muted)" } else { "" },
            percent(hold)
        );
        self.host.remember(endpoint, name, level, muted);
        Ok(())
    }

    /// Stops (the setting off, the default device changed away, Foxglove quits): the device back
    /// to master's level first, then every app to its own level, then the journal. A failure
    /// leaves the apps quieter, never louder; if the device can't be set, nothing changes.
    ///
    /// While master is muted, the apps its mute muted stay muted (and marked in the journal):
    /// the device stays muted, but one that ignores its volume may ignore that too, so unmuting
    /// them would be louder. They are unmuted when the device is ([`Soft::unmute_left`]).
    pub fn disengage(&mut self, audio: &mut dyn Audio) -> Result<(), String> {
        let Some(on) = self.on.take() else {
            return Ok(());
        };
        if let Err(e) = audio.set_endpoint(on.level) {
            self.on = Some(on);
            return Err(format!("the device's level can't be set back: {e}"));
        }
        let mut failed = 0;
        let mut kept = 0;
        for (id, a) in &on.apps {
            if audio.set_level(id, a.own).is_err() {
                failed += 1;
                continue;
            }
            if a.by_master && on.muted {
                self.journal.apps.insert(
                    a.sid.clone(),
                    Record {
                        own: a.own,
                        applied: a.own,
                        by_master: true,
                        day: self.today,
                    },
                );
                kept += 1;
                continue;
            }
            if a.by_master {
                let _ = audio.set_mute(id, false);
            }
            self.journal.apps.remove(&a.sid);
        }
        self.journal.devices.remove(&on.endpoint);
        let _ = self.save_journal();
        self.host
            .remember(&on.endpoint, &on.name, on.level, on.muted);
        eprintln!(
            "Foxglove: master volume: off for {}: the device back at {} %, {} apps at their own level{}",
            on.name,
            percent(on.level),
            on.apps.len() - failed,
            if failed > 0 {
                format!(", {failed} couldn't be set")
            } else {
                String::new()
            }
        );
        if kept > 0 {
            eprintln!(
                "Foxglove: master volume: master was muted: {kept} apps stay muted until the device is unmuted"
            );
        }
        Ok(())
    }

    /// The device isn't muted (now, or as Foxglove starts) while Foxglove doesn't set its volume:
    /// the apps master's mute left muted when it stopped are unmuted, as master's unmute would
    /// have. Nothing while the device is muted or can't be read. The number unmuted.
    pub fn unmute_left(&mut self, audio: &mut dyn Audio) -> usize {
        if self.on.is_some() || !matches!(audio.endpoint(), Ok((_, false))) || !self.readable() {
            return 0;
        }
        let mut n = 0;
        let mut changed = false;
        for s in audio.sessions() {
            let left = self
                .journal
                .apps
                .get(&s.sid)
                .is_some_and(|r| r.by_master && near(r.applied, r.own));
            if !left {
                continue;
            }
            if s.muted {
                if audio.set_mute(&s.id, false).is_err() {
                    continue;
                }
                n += 1;
            }
            self.journal.apps.remove(&s.sid);
            changed = true;
        }
        if changed {
            let _ = self.save_journal();
        }
        if n > 0 {
            eprintln!("Foxglove: master volume: the device was unmuted: {n} apps unmuted");
        }
        n
    }

    /// The device went before it could be put back (unplugged): forgotten here, kept in the
    /// journal, so it is put back when it is the default again.
    pub fn abandon(&mut self) {
        self.on = None;
    }

    /// After a crash with the setting now off (or the device no longer eligible): the device and
    /// the apps back as the journal had them, and the journal's device record goes. The number
    /// of apps put back.
    pub fn recover_off(&mut self, audio: &mut dyn Audio, endpoint: &str) -> usize {
        if self.on.is_some() || !self.readable() {
            return 0;
        }
        let Some(a) = self.journal.devices.get(endpoint).cloned() else {
            return 0;
        };
        let read = audio.endpoint();
        // Muted (or unreadable): the apps master muted stay muted, as on a clean stop.
        let device_muted = read.as_ref().map_or(true, |(_, m)| *m);
        if let Ok((scalar, _)) = read {
            if near(scalar, a.hold) && audio.set_endpoint(a.level).is_err() {
                return 0;
            }
        }
        let mut n = 0;
        for s in audio.sessions() {
            let Some(r) = self.journal.apps.get(&s.sid).cloned() else {
                continue;
            };
            let at_own = if near(s.level, r.applied) {
                let ok = audio.set_level(&s.id, r.own).is_ok();
                n += usize::from(ok);
                ok
            } else {
                true
            };
            if r.by_master && s.muted && device_muted && at_own {
                self.journal.apps.insert(
                    s.sid.clone(),
                    Record {
                        applied: r.own,
                        day: self.today,
                        ..r
                    },
                );
                continue;
            }
            if r.by_master && s.muted {
                let _ = audio.set_mute(&s.id, false);
            }
            self.journal.apps.remove(&s.sid);
        }
        self.journal.devices.remove(endpoint);
        let _ = self.save_journal();
        note_restored(n);
        eprintln!("Foxglove: master volume: restored {n} apps from the last run's journal");
        n
    }

    /// A new session on the device. On: it joins at its own level times master (muted when
    /// master is). Off: a closed app Windows started at the scaled level it saved gets its own
    /// level back.
    pub fn created(&mut self, audio: &mut dyn Audio, s: &Live) {
        // Master muted it before it closed, and Windows started it muted again.
        let stale = self.journal.apps.get(&s.sid).is_some_and(|r| r.by_master) && s.muted;
        let Some(on) = &self.on else {
            if !self.readable() {
                return;
            }
            let Some(r) = self.journal.apps.get(&s.sid).cloned() else {
                return;
            };
            let at_own = if near(s.level, r.applied) && !near(r.applied, r.own) {
                audio.set_level(&s.id, r.own).is_ok()
            } else {
                true
            };
            // The device is muted (or can't be read): master's mark stays until it is unmuted.
            if stale && at_own && audio.endpoint().map_or(true, |(_, m)| m) {
                self.journal.apps.insert(
                    s.sid.clone(),
                    Record {
                        applied: r.own,
                        day: self.today,
                        ..r
                    },
                );
                let _ = self.save_journal();
                return;
            }
            if stale {
                let _ = audio.set_mute(&s.id, false);
            }
            self.journal.apps.remove(&s.sid);
            let _ = self.save_journal();
            return;
        };
        if on.apps.contains_key(&s.id) {
            return;
        }
        let factor = on.factor();
        let master_muted = on.muted;
        let (own, _) = self.own_of(s);
        // Muted by master now: it is master's (a stale mark included); else a stale mark is
        // undone.
        let by_master = master_muted && (!s.muted || stale);
        if stale && !master_muted {
            let _ = audio.set_mute(&s.id, false);
        }
        if let Some(on) = self.on.as_mut() {
            on.apps.insert(
                s.id.clone(),
                App {
                    sid: s.sid.clone(),
                    own,
                    applied: own * factor,
                    by_master,
                    self_managed: false,
                    writes: VecDeque::new(),
                    last: None,
                    due: None,
                },
            );
        }
        let _ = self.journal_on();
        if let Err(e) = audio.set_level(&s.id, own * factor) {
            eprintln!("Foxglove: master volume: a new app's level can't be set: {e}");
        }
        if by_master && !s.muted {
            let _ = audio.set_mute(&s.id, true);
        }
    }

    /// A session ended. Its record stays (Windows saved its level; the next start of the app is
    /// put back from it).
    pub fn gone(&mut self, id: &str) {
        if let Some(on) = self.on.as_mut() {
            on.apps.remove(id);
        }
    }

    /// An app's level or mute changed from outside (the app itself, Windows' Volume mixer):
    /// a new level is the app's own and is scaled again (now, or at the returned deadline when
    /// Foxglove scaled it less than [`COALESCE`] ago). An app that writes more than
    /// [`FIGHT_WRITES`] times in [`FIGHT_WINDOW`] sets its own volume: left alone from then on.
    /// An unmute while master is muted clears the app's mark.
    pub fn volume_event(
        &mut self,
        audio: &mut dyn Audio,
        id: &str,
        level: f32,
        muted: bool,
        now: Instant,
    ) -> Option<Instant> {
        let on = self.on.as_mut()?;
        let factor = on.factor();
        let app = on.apps.get_mut(id)?;
        let mut journal = false;
        if !muted && app.by_master {
            app.by_master = false;
            journal = true;
        }
        if near(level, app.applied) {
            if journal {
                let _ = self.journal_on();
            }
            return None;
        }
        // A fight: the app puts its own level back soon after Foxglove scaled it. A drag of its
        // slider in Volume mixer moves the level each time and never counts.
        let fight = near(level, app.own)
            && app
                .last
                .is_some_and(|t| now.saturating_duration_since(t) <= FIGHT_GAP);
        app.own = level;
        app.applied = level;
        if app.self_managed {
            let _ = self.journal_on();
            return None;
        }
        if fight {
            app.writes.push_back(now);
        }
        while app
            .writes
            .front()
            .is_some_and(|t| now.duration_since(*t) > FIGHT_WINDOW)
        {
            app.writes.pop_front();
        }
        if app.writes.len() > FIGHT_WRITES {
            app.self_managed = true;
            app.due = None;
            eprintln!(
                "Foxglove: master volume: an app put its own volume back {} times in {} s; master leaves it alone",
                app.writes.len(),
                FIGHT_WINDOW.as_secs()
            );
            // Its own level is what it just wrote, and what it has.
            let _ = self.journal_on();
            return None;
        }
        if let Some(last) = app.last.filter(|t| now < *t + COALESCE) {
            app.due = Some(last + COALESCE);
            let due = app.due;
            let _ = self.journal_on();
            return due;
        }
        app.applied = level * factor;
        app.last = Some(now);
        app.due = None;
        let applied = app.applied;
        let _ = self.journal_on();
        let _ = audio.set_level(id, applied);
        None
    }

    /// The earliest deadline waiting: an app's coalesced re-apply, or a key press or a drag
    /// that settles.
    pub fn next_due(&self) -> Option<Instant> {
        let on = self.on.as_ref()?;
        on.apps
            .values()
            .filter_map(|a| a.due)
            .chain(on.press.map(|p| p.due))
            .chain(on.drag.map(|d| d.due))
            .min()
    }

    /// Runs what is due by `now`: settled key presses or a settled drag on Windows' slider
    /// (whether master was taken from them: Foxglove shows its card), then the apps' coalesced
    /// re-applies.
    pub fn run_due(&mut self, audio: &mut dyn Audio, now: Instant) -> bool {
        let adopted = self.settle_press(audio, now) | self.settle_drag(audio, now);
        let Some(on) = self.on.as_mut() else {
            return adopted;
        };
        let factor = on.factor();
        let mut writes = Vec::new();
        for (id, a) in on.apps.iter_mut() {
            if a.due.is_some_and(|d| d <= now) {
                a.due = None;
                a.last = Some(now);
                a.applied = a.own * factor;
                writes.push((id.clone(), a.applied));
            }
        }
        if !writes.is_empty() {
            let _ = self.journal_on();
            for (id, level) in writes {
                let _ = audio.set_level(&id, level);
            }
        }
        adopted
    }

    /// Key presses no other change followed for [`KEY_SETTLE`]: master moves by their steps
    /// (down never raises it), and the device is held again. Steps of another size than the
    /// device's are what its keys move: master steps by that from now on.
    fn settle_press(&mut self, audio: &mut dyn Audio, now: Instant) -> bool {
        let Some(on) = self.on.as_mut() else {
            return false;
        };
        let Some(p) = on.press.filter(|p| p.due <= now) else {
            return false;
        };
        on.press = None;
        if (p.step - on.step).abs() > f32::EPSILON {
            // Applied by the size it moved; the device's step only after presses in a row agree.
            let n = match on.candidate {
                Some((size, n)) if (size - p.step).abs() <= tolerance(p.step) => n + 1,
                _ => 1,
            };
            on.candidate = Some((p.step, n));
            if n >= LEARN_PRESSES {
                eprintln!(
                    "Foxglove: master volume: {}'s keys move {} %, not the {} % it reports; master steps by {} %",
                    on.name,
                    percent(p.step),
                    percent(on.step),
                    percent(p.step)
                );
                on.step = p.step;
                on.candidate = None;
            }
        } else {
            on.candidate = None;
        }
        let new = (on.level + p.steps * p.step).clamp(0.0, 1.0);
        // Snap to Windows' grid, as its own keys leave it; a step down never ends higher.
        let new = (new * 100.0).round() / 100.0;
        on.level = if p.steps < 0.0 {
            new.min(on.level)
        } else {
            new
        };
        self.hold_again(audio);
        true
    }

    /// A drag that has been still for [`DRAG_SETTLE`]: down, master goes to where it was
    /// dropped, but only ever lower (the slider starts near the top, so a drag down from there
    /// never raises master); up (to the top), master goes up by the whole steps it moved. The
    /// device is held again.
    fn settle_drag(&mut self, audio: &mut dyn Audio, now: Instant) -> bool {
        let Some(on) = self.on.as_mut() else {
            return false;
        };
        let Some(d) = on.drag.filter(|d| d.due <= now) else {
            return false;
        };
        on.drag = None;
        let tol = on.tolerance();
        let level = if d.to < d.start - tol {
            on.level.min(d.to)
        } else if d.to > d.start + tol {
            let n = ((d.to - d.start) / on.step).round().max(1.0);
            (on.level + n * on.step).min(1.0)
        } else {
            on.level
        };
        on.level = (level * 100.0).round() / 100.0;
        self.hold_again(audio);
        true
    }

    /// Every app scaled to master now, then the device back at its hold (unless it is there).
    fn hold_again(&mut self, audio: &mut dyn Audio) {
        self.rescale(audio);
        let Some(on) = self.on.as_ref() else { return };
        if !near(on.seen, on.hold) {
            if let Err(e) = audio.set_endpoint(on.hold) {
                eprintln!("Foxglove: master volume: the device can't be held: {e}");
            }
        }
    }

    /// Every app at its own level times master now (journal first).
    fn rescale(&mut self, audio: &mut dyn Audio) {
        let Some(on) = self.on.as_mut() else { return };
        let factor = on.factor();
        let mut writes = Vec::new();
        for (id, a) in on.apps.iter_mut() {
            if a.self_managed {
                continue;
            }
            let new = a.own * factor;
            if new.to_bits() != a.applied.to_bits() {
                a.applied = new;
                writes.push((id.clone(), new));
            }
        }
        let _ = self.journal_on();
        for (id, level) in writes {
            if let Err(e) = audio.set_level(&id, level) {
                eprintln!("Foxglove: master volume: an app's level can't be set: {e}");
            }
        }
    }

    /// Mutes the apps not muted already (marked), or unmutes the marked ones.
    fn mute_apps(&mut self, audio: &mut dyn Audio, muted: bool) {
        let live: HashMap<String, bool> = audio
            .sessions()
            .into_iter()
            .map(|s| (s.id, s.muted))
            .collect();
        let Some(on) = self.on.as_mut() else { return };
        on.muted = muted;
        let mut writes = Vec::new();
        for (id, a) in on.apps.iter_mut() {
            if muted && !a.by_master && live.get(id) == Some(&false) {
                a.by_master = true;
                writes.push((id.clone(), true));
            } else if !muted && a.by_master {
                a.by_master = false;
                writes.push((id.clone(), false));
            }
        }
        let _ = self.journal_on();
        for (id, m) in writes {
            let _ = audio.set_mute(&id, m);
        }
    }

    /// Windows says the device's level or mute changed. `own`: Foxglove's own write landing
    /// (only the level it left is noted). Otherwise it becomes master, and the device is held
    /// again here:
    ///
    /// - a change of a whole number of key steps (up to [`MAX_KEY_STEPS`]) from the level last
    ///   seen is that many key presses: master moves that many steps from where it is, once no
    ///   other change has followed for [`KEY_SETTLE`] ([`Soft::run_due`]). Counting from the
    ///   level last seen, not the hold, reads a key repeat that outran the hold right. Further
    ///   whole steps the same way before then add to them; any other change makes them the
    ///   start of a drag (a drag's first sample can be one whole step);
    /// - from the hold, on a device that reports a step coarser than [`DEFAULT_STEP`], a change
    ///   smaller than that step (at least [`MIN_STEP`], whole percent) is one key press on a
    ///   device whose keys move less than it reports: master
    ///   steps by that, then and from then on, so up and down stay alike;
    /// - anything else is a drag on Windows' slider: applied once it has been still for
    ///   [`DRAG_SETTLE`] ([`Soft::run_due`]), never raising master on the way down.
    ///
    /// Whether master changed now (a mute; a level change shows when it settles).
    pub fn endpoint_event(
        &mut self,
        audio: &mut dyn Audio,
        level: f32,
        muted: bool,
        own: bool,
        now: Instant,
    ) -> bool {
        let Some(on) = self.on.as_mut() else {
            return false;
        };
        if own {
            on.seen = level;
            return false;
        }
        let mut changed = false;
        if muted != on.muted {
            self.mute_apps(audio, muted);
            changed = true;
        }
        let Some(on) = self.on.as_mut() else {
            return changed;
        };
        let from = on.seen;
        on.seen = level;
        let tol = on.tolerance();
        if let Some(d) = on.drag.as_mut() {
            d.to = level;
            d.due = now + DRAG_SETTLE;
            return changed;
        }
        let off = level - from;
        if off.abs() <= tol {
            return changed;
        }
        if let Some(p) = on.press.as_mut() {
            match key_steps(off, p.step) {
                Some(n) if n.signum() == p.steps.signum() => p.steps += n,
                // Not a key: the presses were a drag's first samples.
                _ => {
                    on.drag = Some(Drag {
                        start: p.start,
                        to: level,
                        due: now + DRAG_SETTLE,
                    });
                    on.press = None;
                }
            }
            return changed;
        }
        let key = key_steps(off, on.step).map(|n| (n, on.step)).or_else(|| {
            // A device whose keys move less than the step it reports (one press, from the hold).
            let size = (off.abs() * 100.0).round() / 100.0;
            (on.step > DEFAULT_STEP + tol
                && near(from, on.hold)
                && size >= MIN_STEP
                && size < on.step - tol
                && (off.abs() - size).abs() <= tolerance(size))
            .then(|| (off.signum(), size))
        });
        if let Some((steps, step)) = key {
            on.press = Some(Press {
                start: from,
                steps,
                step,
                due: now + KEY_SETTLE,
            });
            return changed;
        }
        on.drag = Some(Drag {
            start: from,
            to: level,
            due: now + DRAG_SETTLE,
        });
        changed
    }

    /// The key step master counts presses by now (the device's, or what its keys were seen to
    /// move); `None` when not on.
    pub fn key_step(&self) -> Option<f32> {
        self.on.as_ref().map(|o| o.step)
    }

    /// The knob turns master while Foxglove doesn't set the device's volume (#855): the device's
    /// own level by `steps` detents, unmuted as Windows' keys do; its new level in percent. The
    /// writes are Foxglove's own, so no notification follows them: an unmute frees the apps a
    /// muted master left muted here ([`Soft::unmute_left`]).
    pub fn plain_step(&mut self, audio: &mut dyn Audio, steps: i32) -> Result<u8, String> {
        let (level, muted) = audio.endpoint()?;
        let new = step_level(level, steps);
        audio.set_endpoint(new)?;
        if muted {
            audio.set_endpoint_mute(false)?;
            self.unmute_left(audio);
        }
        Ok(percent(new))
    }

    /// The knob's double click on master while Foxglove doesn't set the device's volume: the
    /// device muted or unmuted (an unmute frees the apps a muted master left muted); whether it
    /// is muted now.
    pub fn plain_mute(&mut self, audio: &mut dyn Audio) -> Result<bool, String> {
        let (_, muted) = audio.endpoint()?;
        audio.set_endpoint_mute(!muted)?;
        if muted {
            self.unmute_left(audio);
        }
        Ok(!muted)
    }

    /// The knob turns master by `steps` detents (2 % each) and unmutes it, as Windows' keys do;
    /// master's new level in percent.
    pub fn step_master(&mut self, audio: &mut dyn Audio, steps: i32) -> Result<u8, String> {
        let on = self.on.as_mut().ok_or("not on")?;
        on.level = step_level(on.level, steps);
        let level = on.level;
        if on.muted {
            audio.set_endpoint_mute(false)?;
            self.mute_apps(audio, false);
        }
        self.rescale(audio);
        Ok(percent(level))
    }

    /// The knob's double click on master: mutes or unmutes master (the device and the apps);
    /// whether it is muted now.
    pub fn toggle_mute(&mut self, audio: &mut dyn Audio) -> Result<bool, String> {
        let muted = !self.on.as_ref().ok_or("not on")?.muted;
        audio.set_endpoint_mute(muted)?;
        self.mute_apps(audio, muted);
        Ok(muted)
    }

    /// The knob turns an app: its own level by `steps` detents, then scaled; its own level in
    /// percent. `None`: not an app Foxglove scales (the plain path applies).
    pub fn step_app(
        &mut self,
        audio: &mut dyn Audio,
        id: &str,
        steps: i32,
        now: Instant,
    ) -> Option<Result<u8, String>> {
        let on = self.on.as_mut()?;
        let factor = on.factor();
        let app = on.apps.get_mut(id)?;
        app.own = step_level(app.own, steps);
        app.applied = if app.self_managed {
            app.own
        } else {
            app.own * factor
        };
        app.last = Some(now);
        app.due = None;
        let (own, applied) = (app.own, app.applied);
        let _ = self.journal_on();
        Some(audio.set_level(id, applied).map(|()| percent(own)))
    }

    /// The knob mutes or unmutes an app: an unmute clears master's mark on it.
    pub fn app_muted(&mut self, id: &str, muted: bool) {
        let Some(app) = self.on.as_mut().and_then(|o| o.apps.get_mut(id)) else {
            return;
        };
        if !muted && app.by_master {
            app.by_master = false;
            let _ = self.journal_on();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// The device's and apps' state as Windows has it, shared with the test.
    #[derive(Clone, Debug, Default)]
    struct World {
        endpoint: f32,
        endpoint_muted: bool,
        sessions: Vec<Live>,
        /// Writes left before one fails (`None`: none fails).
        fail_after: Option<usize>,
        /// An app that ends when Foxglove writes its level (its sound stopped meanwhile).
        vanish: Option<String>,
        /// Every write, in order.
        log: Vec<String>,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<World>>);

    impl Fake {
        fn new(endpoint: f32, apps: &[(&str, f32)]) -> Fake {
            Fake(Arc::new(Mutex::new(World {
                endpoint,
                sessions: apps
                    .iter()
                    .map(|(id, l)| Live {
                        id: id.to_string(),
                        sid: format!("sid-{id}"),
                        level: *l,
                        muted: false,
                    })
                    .collect(),
                ..World::default()
            })))
        }
        fn w(&self) -> std::sync::MutexGuard<'_, World> {
            self.0.lock().unwrap()
        }
        fn level(&self, id: &str) -> f32 {
            self.w().sessions.iter().find(|s| s.id == id).unwrap().level
        }
        fn muted(&self, id: &str) -> bool {
            self.w().sessions.iter().find(|s| s.id == id).unwrap().muted
        }
        fn write(&self, what: String) -> Result<(), String> {
            let mut w = self.w();
            if let Some(n) = w.fail_after.as_mut() {
                if *n == 0 {
                    return Err(format!("failed: {what}"));
                }
                *n -= 1;
            }
            w.log.push(what);
            Ok(())
        }
        /// What one app sounds like: its level, through the device's level when the device
        /// honours it, silent when muted.
        fn out(&self, id: &str, honours: bool) -> f32 {
            let w = self.w();
            let s = w.sessions.iter().find(|s| s.id == id).unwrap();
            if s.muted || w.endpoint_muted && honours {
                return 0.0;
            }
            s.level * if honours { amp(w.endpoint) } else { 1.0 }
        }
        fn add(&self, id: &str, sid: &str, level: f32) -> Live {
            let s = Live {
                id: id.into(),
                sid: sid.into(),
                level,
                muted: false,
            };
            self.w().sessions.push(s.clone());
            s
        }
    }

    impl Audio for Fake {
        fn endpoint(&self) -> Result<(f32, bool), String> {
            let w = self.w();
            Ok((w.endpoint, w.endpoint_muted))
        }
        fn set_endpoint(&mut self, level: f32) -> Result<(), String> {
            self.write(format!("endpoint {level:.2}"))?;
            self.w().endpoint = level;
            Ok(())
        }
        fn set_endpoint_mute(&mut self, muted: bool) -> Result<(), String> {
            self.write(format!("endpoint mute {muted}"))?;
            self.w().endpoint_muted = muted;
            Ok(())
        }
        fn sessions(&self) -> Vec<Live> {
            self.w().sessions.clone()
        }
        fn set_level(&mut self, id: &str, level: f32) -> Result<(), String> {
            if self.w().vanish.as_deref() == Some(id) {
                self.w().sessions.retain(|s| s.id != id);
                return Err("the app's sound ended".into());
            }
            self.write(format!("{id} {level:.4}"))?;
            let mut w = self.w();
            w.sessions.iter_mut().find(|s| s.id == id).unwrap().level = level;
            Ok(())
        }
        fn set_mute(&mut self, id: &str, muted: bool) -> Result<(), String> {
            self.write(format!("{id} mute {muted}"))?;
            let mut w = self.w();
            w.sessions.iter_mut().find(|s| s.id == id).unwrap().muted = muted;
            Ok(())
        }
    }

    #[derive(Default)]
    struct Disk {
        journal: Option<Journal>,
        /// The file is there but can't be read (a sharing violation, its rights).
        unreadable: bool,
        writes: usize,
        choices: HashMap<String, bool>,
        saved: HashMap<String, (f32, bool)>,
    }

    #[derive(Clone, Default)]
    struct MemHost(Arc<Mutex<Disk>>);

    impl Host for MemHost {
        fn journal(&self) -> Result<Option<Journal>, String> {
            let d = self.0.lock().unwrap();
            if d.unreadable {
                return Err("access denied".into());
            }
            Ok(d.journal.clone())
        }
        fn write_journal(&mut self, j: &Journal) -> Result<(), String> {
            let mut d = self.0.lock().unwrap();
            d.writes += 1;
            d.journal = (!j.is_empty()).then(|| j.clone());
            Ok(())
        }
        fn choice(&self, endpoint: &str) -> Option<bool> {
            self.0.lock().unwrap().choices.get(endpoint).copied()
        }
        fn listed(&self, ids: &[String]) -> bool {
            listed(ids, &["USB\\VID_0ECB&PID_2148".to_string()])
        }
        fn saved(&self, endpoint: &str) -> Option<(f32, bool)> {
            self.0.lock().unwrap().saved.get(endpoint).copied()
        }
        fn remember(&mut self, endpoint: &str, _name: &str, level: f32, muted: bool) {
            self.0
                .lock()
                .unwrap()
                .saved
                .insert(endpoint.into(), (level, muted));
        }
    }

    const DEV: &str = "{0.0.0.00000000}.{dongle}";
    const STEP: f32 = 0.02;

    fn soft(host: &MemHost) -> Soft {
        Soft::new(Box::new(host.clone()), 20_000)
    }

    fn the_three() -> Fake {
        Fake::new(0.35, &[("vivaldi", 1.0), ("tarkov", 0.84), ("chat", 0.50)])
    }

    #[test]
    fn t1_scale_then_restore_exactly() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        let before: Vec<u32> = a.sessions().iter().map(|s| s.level.to_bits()).collect();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let f = amp(0.35);
        assert_eq!(a.level("vivaldi"), f);
        assert_eq!(a.level("tarkov"), 0.84 * f);
        assert_eq!(a.level("chat"), 0.50 * f);
        assert_eq!(
            a.w().endpoint,
            hold_for(STEP),
            "held one key step below the top"
        );
        assert_eq!(s.master(), Some((35, false)));
        assert_eq!(
            s.own("tarkov"),
            Some(0.84),
            "the knob's row shows the app's own level"
        );
        assert_eq!(
            s.app_volume("tarkov"),
            Some(AppVolume::Scaled {
                own: 84,
                applied: 15,
                master: 35
            })
        );
        s.disengage(&mut a).unwrap();
        let after: Vec<u32> = a.sessions().iter().map(|s| s.level.to_bits()).collect();
        assert_eq!(before, after, "bit-exact");
        assert_eq!(a.w().endpoint, 0.35);
        assert!(
            host.journal().unwrap().is_none(),
            "the journal goes after a clean restore"
        );
    }

    #[test]
    fn t2_a_new_session_joins_scaled() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let new = a.add("game", "sid-game", 0.7);
        s.created(&mut a, &new);
        assert_eq!(a.level("game"), 0.7 * amp(0.35));
        assert_eq!(
            s.own("game"),
            Some(0.7),
            "its own is the level Windows gave it"
        );
    }

    #[test]
    fn t3_the_order_never_gets_louder_whatever_fails() {
        // The write order: apps before the device on engage; device before the apps on
        // disengage.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let log = a.w().log.clone();
        assert_eq!(log.last().unwrap(), "endpoint 0.98", "{log:?}");
        a.w().log.clear();
        s.disengage(&mut a).unwrap();
        assert_eq!(a.w().log[0], "endpoint 0.35");

        // Every failure point, on a device that honours its volume and on one that doesn't.
        let ids = ["vivaldi", "tarkov", "chat"];
        for honours in [true, false] {
            for fail in 0..8 {
                let host = MemHost::default();
                let mut s = soft(&host);
                let mut a = the_three();
                let before: Vec<f32> = ids.iter().map(|i| a.out(i, honours)).collect();
                a.w().fail_after = Some(fail);
                let _ = s.engage(&mut a, DEV, "Headphones", STEP);
                for (i, id) in ids.iter().enumerate() {
                    assert!(
                        a.out(id, honours) <= before[i] + 1e-6,
                        "engage, failure at write {fail}, honours {honours}: {id}"
                    );
                }
                // And from on, a failing disengage.
                let host = MemHost::default();
                let mut s = soft(&host);
                let mut a = the_three();
                s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
                a.w().fail_after = Some(fail);
                let _ = s.disengage(&mut a);
                for (i, id) in ids.iter().enumerate() {
                    assert!(
                        a.out(id, honours) <= before[i] + 1e-6,
                        "disengage, failure at write {fail}, honours {honours}: {id}"
                    );
                }
            }
        }
    }

    #[test]
    fn t4_eligibility() {
        let dongle = vec![r"USB\VID_0ECB&PID_2148&IGA_00\7&1a2b".to_string()];
        let other = vec![r"USB\VID_046D&PID_0A87&MI_00\6&9".to_string()];
        let host = MemHost::default();
        // No hardware flag: never, even listed and chosen.
        assert!(!turned_on(eligible(0x2), host.listed(&dongle), None));
        assert!(!turned_on(eligible(0x0), host.listed(&dongle), Some(true)));
        // Listed with the flag: on by default; the user's choice wins.
        assert!(turned_on(eligible(0x3), host.listed(&dongle), None));
        assert!(!turned_on(eligible(0x3), host.listed(&dongle), Some(false)));
        // Unlisted with the flag: off until turned on.
        assert!(!turned_on(eligible(0x1), host.listed(&other), None));
        assert!(turned_on(eligible(0x1), host.listed(&other), Some(true)));
        // The match is a prefix, in any case.
        assert!(listed(
            &[r"usb\vid_0ecb&pid_2148".to_string()],
            &[r"USB\VID_0ECB&PID_2148".to_string()]
        ));
        assert!(!listed(
            &[r"USB\VID_0ECB&PID_2149".to_string()],
            &[r"USB\VID_0ECB&PID_2148".to_string()]
        ));
    }

    /// Windows reports the device at `level`, not by Foxglove (a key press, a drag's step).
    fn outside(s: &mut Soft, a: &mut Fake, level: f32, now: Instant) -> bool {
        a.w().endpoint = level;
        s.endpoint_event(a, level, false, false, now)
    }

    /// Foxglove's own hold lands: Windows reports it with Foxglove's context.
    fn echo(s: &mut Soft, a: &mut Fake, now: Instant) {
        let level = a.w().endpoint;
        assert!(!s.endpoint_event(a, level, false, true, now));
    }

    /// One key press, as Windows sends it: the change, nothing else for [`KEY_SETTLE`], then
    /// Foxglove's hold lands. Whether master changed (its card shows).
    fn key(s: &mut Soft, a: &mut Fake, level: f32, t: &mut Instant) -> bool {
        assert!(!outside(s, a, level, *t), "nothing before it settles");
        *t += KEY_SETTLE;
        let changed = s.run_due(a, *t);
        echo(s, a, *t);
        *t += Duration::from_millis(300);
        changed
    }

    #[test]
    fn t5_an_outside_change_is_master_and_the_device_is_held_again() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let mut t = Instant::now();
        echo(&mut s, &mut a, t);
        // Volume up from the hold: one step once nothing else follows; the device held again.
        assert!(!outside(&mut s, &mut a, 1.0, t));
        assert_eq!(s.master(), Some((35, false)), "not before it settles");
        assert_eq!(s.next_due(), Some(t + KEY_SETTLE));
        t += KEY_SETTLE;
        assert!(s.run_due(&mut a, t));
        assert_eq!(s.master(), Some((37, false)));
        assert_eq!(a.level("tarkov"), 0.84 * amp(0.37));
        assert_eq!(a.w().endpoint, 0.98, "held again");
        echo(&mut s, &mut a, t);
        assert!(key(&mut s, &mut a, 0.96, &mut t));
        assert_eq!(s.master(), Some((35, false)));
        // From the top and from the bottom: keys work both ways.
        for _ in 0..40 {
            key(&mut s, &mut a, 1.0, &mut t);
        }
        assert_eq!(s.master(), Some((100, false)));
        key(&mut s, &mut a, 0.96, &mut t);
        assert_eq!(s.master(), Some((98, false)));
        for _ in 0..60 {
            key(&mut s, &mut a, 0.96, &mut t);
        }
        assert_eq!(s.master(), Some((0, false)));
        key(&mut s, &mut a, 1.0, &mut t);
        assert_eq!(s.master(), Some((2, false)));
        // The mute key: the apps muted and marked at once; no level change.
        assert!(s.endpoint_event(&mut a, 0.98, true, false, t));
        assert!(a.muted("tarkov"));
        assert_eq!(s.master(), Some((2, true)));
    }

    #[test]
    fn a_key_repeat_that_outruns_the_hold_counts_every_step() {
        // Review #1069 (2): Windows reaches 96 %, 94 %, 92 % before Foxglove's hold lands.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let mut t = Instant::now();
        echo(&mut s, &mut a, t);
        let before = a.level("tarkov");
        for level in [0.96, 0.94, 0.92] {
            assert!(!s.endpoint_event(&mut a, level, false, false, t));
            assert_eq!(a.level("tarkov"), before, "down never louder");
            t += Duration::from_millis(30);
        }
        assert!(s.run_due(&mut a, t - Duration::from_millis(90) + KEY_SETTLE));
        assert_eq!(
            s.master(),
            Some((29, false)),
            "three steps down, not a drag to 92 %"
        );
        // The hold lands afterwards: nothing more.
        echo(&mut s, &mut a, t);
        assert_eq!(s.master(), Some((29, false)));
        // A repeat whose middle notifications Windows folded: 98 % to 92 % at once is 3 steps.
        t += Duration::from_secs(1);
        assert!(key(&mut s, &mut a, 0.92, &mut t));
        assert_eq!(s.master(), Some((23, false)));
        assert_eq!(a.level("tarkov"), 0.84 * amp(0.23));
    }

    #[test]
    fn a_drag_whose_first_sample_is_one_whole_step_is_a_drag() {
        // #1070 (5): from the hold, a drag's first sample can be exactly one key step.
        for first in [1.0, 0.96] {
            let host = MemHost::default();
            let mut s = soft(&host);
            let mut a = the_three();
            s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
            let mut t = Instant::now();
            echo(&mut s, &mut a, t);
            a.w().log.clear();
            let before = a.level("tarkov");
            for p in [first, 0.90, 0.70, 0.60] {
                assert!(!outside(&mut s, &mut a, p, t));
                assert!(!s.run_due(&mut a, t), "not settled ({p})");
                assert_eq!(s.master(), Some((35, false)), "no key step ({p})");
                t += Duration::from_millis(30);
            }
            assert!(a.w().log.is_empty(), "nothing written while it moves");
            assert!(!s.run_due(&mut a, t + KEY_SETTLE), "still a drag");
            assert!(s.run_due(&mut a, t + DRAG_SETTLE));
            assert_eq!(
                s.master(),
                Some((35, false)),
                "dropped above master: a drag down never raises it ({first})"
            );
            assert!(a.level("tarkov") <= before);
            assert_eq!(a.w().endpoint, 0.98);
        }
    }

    #[test]
    fn a_device_whose_keys_move_less_than_its_step_steps_both_ways_alike() {
        // #1070 (3): the device reports 10 % steps; Windows' keys move it 2 %.
        let coarse = step_for(11);
        assert_eq!(coarse, 0.10);
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", coarse).unwrap();
        assert_eq!(a.w().endpoint, hold_for(coarse));
        let mut t = Instant::now();
        echo(&mut s, &mut a, t);
        assert!(key(&mut s, &mut a, 0.92, &mut t));
        assert_eq!(s.master(), Some((37, false)), "up by what the key moved");
        assert_eq!(a.w().endpoint, 0.9, "held where it was");
        assert_eq!(s.key_step(), Some(coarse), "one press is not learnt yet");
        let mut loudest = a.level("tarkov");
        assert!(key(&mut s, &mut a, 0.88, &mut t));
        assert_eq!(s.master(), Some((35, false)), "and down by the same");
        assert!(a.level("tarkov") <= loudest);
        assert_eq!(
            s.key_step(),
            Some(0.02),
            "two presses of 2 % in a row: learnt"
        );
        for _ in 0..5 {
            key(&mut s, &mut a, 0.92, &mut t);
        }
        assert_eq!(s.master(), Some((45, false)));
        for _ in 0..5 {
            key(&mut s, &mut a, 0.88, &mut t);
        }
        assert_eq!(s.master(), Some((35, false)), "symmetric");
        // Down never louder, every press.
        for _ in 0..20 {
            loudest = a.level("tarkov");
            key(&mut s, &mut a, 0.88, &mut t);
            assert!(a.level("tarkov") <= loudest, "down never louder");
        }
        assert_eq!(s.master(), Some((0, false)));
        // A device whose keys do move its own step still steps by it.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", coarse).unwrap();
        let mut t = Instant::now();
        echo(&mut s, &mut a, t);
        assert!(key(&mut s, &mut a, 1.0, &mut t));
        assert_eq!(s.master(), Some((45, false)));
        assert!(key(&mut s, &mut a, 0.8, &mut t));
        assert_eq!(s.master(), Some((35, false)));
        // One slider click of 3 %, then keys of the device's own step: nothing learnt.
        assert!(key(&mut s, &mut a, 0.87, &mut t));
        assert_eq!(s.master(), Some((32, false)));
        assert!(key(&mut s, &mut a, 1.0, &mut t));
        assert!(key(&mut s, &mut a, 0.93, &mut t));
        assert_eq!(s.key_step(), Some(coarse), "3 % then 3 %, but not in a row");
        // Two different sizes in a row don't agree either.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", coarse).unwrap();
        let mut t = Instant::now();
        echo(&mut s, &mut a, t);
        key(&mut s, &mut a, 0.93, &mut t);
        key(&mut s, &mut a, 0.88, &mut t);
        assert_eq!(s.key_step(), Some(coarse));
    }

    #[test]
    fn a_slider_drag_is_applied_once_it_settles_and_never_raises_master_on_the_way_down() {
        // Review #1069 (1): from the hold, a drag down passes 97 %, 95 % ... first.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = Fake::new(0.30, &[("vivaldi", 1.0), ("tarkov", 0.84)]);
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let mut t = Instant::now();
        echo(&mut s, &mut a, t);
        let before = a.level("tarkov");
        for p in (20..=95).rev().step_by(3) {
            outside(&mut s, &mut a, p as f32 / 100.0, t);
            assert_eq!(
                s.master(),
                Some((30, false)),
                "nothing while it moves ({p} %)"
            );
            assert_eq!(a.level("tarkov"), before, "never louder ({p} %)");
            assert!(!s.run_due(&mut a, t), "not settled");
            t += Duration::from_millis(40);
        }
        assert_eq!(
            s.next_due(),
            Some(t - Duration::from_millis(40) + DRAG_SETTLE)
        );
        assert!(s.run_due(&mut a, t + DRAG_SETTLE), "settled: master's card");
        assert_eq!(s.master(), Some((20, false)), "where it was dropped");
        assert_eq!(a.level("tarkov"), 0.84 * amp(0.20));
        assert_eq!(a.w().endpoint, 0.98, "held again after the drag");
        echo(&mut s, &mut a, t);
        // Dropped above master: master stays (the drag went down, so nothing gets louder).
        t += Duration::from_secs(2);
        for p in [0.97, 0.90, 0.75, 0.60] {
            outside(&mut s, &mut a, p, t);
            t += Duration::from_millis(40);
        }
        assert!(s.run_due(&mut a, t + DRAG_SETTLE));
        assert_eq!(s.master(), Some((20, false)));
        assert_eq!(a.level("tarkov"), 0.84 * amp(0.20));
        echo(&mut s, &mut a, t);
        // A small drag up to the top: up by the steps it moved.
        t += Duration::from_secs(2);
        outside(&mut s, &mut a, 0.99, t);
        assert!(s.run_due(&mut a, t + DRAG_SETTLE));
        assert_eq!(s.master(), Some((22, false)));
    }

    #[test]
    fn t6_an_app_that_sets_its_own_volume() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let t0 = Instant::now();
        let f = amp(0.35);
        // The game writes 1.0: its own; scaled at once.
        a.w().sessions[1].level = 1.0;
        assert_eq!(s.volume_event(&mut a, "tarkov", 1.0, false, t0), None);
        assert_eq!(s.own("tarkov"), Some(1.0));
        assert_eq!(a.level("tarkov"), f);
        // Again 100 ms later: waits for the 250 ms.
        let t1 = t0 + Duration::from_millis(100);
        a.w().sessions[1].level = 0.9;
        let due = s.volume_event(&mut a, "tarkov", 0.9, false, t1);
        assert_eq!(due, Some(t0 + COALESCE));
        assert_eq!(s.next_due(), due);
        assert_eq!(a.level("tarkov"), 0.9, "not yet");
        assert!(!s.run_due(&mut a, t0 + COALESCE));
        assert_eq!(s.next_due(), None);
        assert_eq!(a.level("tarkov"), 0.9 * f);
        // The game puts its 0.9 back after each scale, twelve times in 10 s: a fight. Master
        // leaves it alone; its own level stays and nothing more is written to it.
        let mut t = t0 + Duration::from_secs(1);
        for _ in 0..12 {
            a.w().sessions[1].level = 0.9;
            s.volume_event(&mut a, "tarkov", 0.9, false, t);
            t += Duration::from_millis(700);
        }
        a.w().log.clear();
        assert_eq!(s.app_volume("tarkov"), Some(AppVolume::SelfManaged));
        assert_eq!(s.next_due(), None);
        assert_eq!(a.level("tarkov"), 0.9, "its own level, unscaled");
        s.step_master(&mut a, -5).unwrap();
        assert!(!a.w().log.iter().any(|l| l.starts_with("tarkov")));
    }

    #[test]
    fn a_volume_mixer_drag_is_not_a_fight() {
        // Review #1069 (3): dragging one app's slider sends dozens of writes a second.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let mut t = Instant::now();
        let mut level = 0.60;
        for _ in 0..40 {
            a.w().sessions[1].level = level;
            s.volume_event(&mut a, "tarkov", level, false, t);
            level -= 0.01;
            t += Duration::from_millis(25);
        }
        s.run_due(&mut a, t + COALESCE);
        let own = s.own("tarkov").unwrap();
        assert!(matches!(
            s.app_volume("tarkov"),
            Some(AppVolume::Scaled { .. })
        ));
        assert_eq!(a.level("tarkov"), own * amp(0.35), "still follows master");
        assert!(
            (own - (level + 0.01)).abs() < 1e-6,
            "the last level is its own"
        );
    }

    #[test]
    fn an_app_master_muted_before_it_closed_comes_back_unmuted() {
        // Review #1069 (4): mute master, close the app, unmute, start the app again.
        for engaged_at_restart in [true, false] {
            let host = MemHost::default();
            let mut s = soft(&host);
            let mut a = the_three();
            s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
            s.toggle_mute(&mut a).unwrap();
            a.w().sessions.retain(|x| x.id != "tarkov");
            s.gone("tarkov");
            s.toggle_mute(&mut a).unwrap();
            if !engaged_at_restart {
                s.disengage(&mut a).unwrap();
            }
            // Windows starts it muted, at the level it saved.
            let f = amp(0.35);
            let mut back = a.add("tarkov-2", "sid-tarkov", 0.84 * f);
            a.w().sessions.last_mut().unwrap().muted = true;
            back.muted = true;
            s.created(&mut a, &back);
            assert!(
                !a.muted("tarkov-2"),
                "engaged at restart: {engaged_at_restart}"
            );
        }
        // Master muted again when it comes back: it stays muted, and master's unmute frees it.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        s.toggle_mute(&mut a).unwrap();
        a.w().sessions.retain(|x| x.id != "tarkov");
        s.gone("tarkov");
        let mut back = a.add("tarkov-2", "sid-tarkov", 0.84 * amp(0.35));
        a.w().sessions.last_mut().unwrap().muted = true;
        back.muted = true;
        s.created(&mut a, &back);
        assert!(a.muted("tarkov-2"));
        s.toggle_mute(&mut a).unwrap();
        assert!(!a.muted("tarkov-2"));
    }

    #[test]
    fn two_devices_keep_their_own_crash_records() {
        // Review #1069 (5): a crash on A, then B engaged, then A with the setting off.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        drop(s);
        let mut s = soft(&host);
        let mut b = Fake::new(0.5, &[("speakers-app", 1.0)]);
        s.engage(&mut b, "{speakers}", "Speakers", STEP).unwrap();
        let j = host.journal().unwrap().unwrap();
        assert!(j.devices.contains_key(DEV) && j.devices.contains_key("{speakers}"));
        s.disengage(&mut b).unwrap();
        assert_eq!(s.recover_off(&mut a, DEV), 3);
        assert_eq!(a.w().endpoint, 0.35, "A back at its level, not left held");
        assert_eq!(a.level("tarkov"), 0.84);
        assert!(host.journal().unwrap().is_none());
    }

    #[test]
    fn an_app_that_ends_while_foxglove_starts_is_left_out() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        a.w().vanish = Some("chat".into());
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        assert_eq!(s.engaged(), Some(DEV));
        assert_eq!(s.own("chat"), None);
        assert_eq!(a.level("tarkov"), 0.84 * amp(0.35));
        assert_eq!(a.w().endpoint, 0.98);
    }

    #[test]
    fn t7_the_journal_after_a_crash() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        // Crash: the Soft is gone, Windows keeps the scaled levels and the held device.
        drop(s);
        let f = amp(0.35);
        // Meanwhile the user set chat to 0.3 in Windows' Volume mixer.
        a.w().sessions[2].level = 0.3;
        let mut s = soft(&host);
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        assert_eq!(
            s.master(),
            Some((35, false)),
            "master's level from the journal"
        );
        assert_eq!(s.own("vivaldi"), Some(1.0));
        assert_eq!(
            s.own("tarkov"),
            Some(0.84),
            "at Foxglove's level: the journal's own"
        );
        assert_eq!(s.own("chat"), Some(0.3), "set meanwhile: that is its own");
        assert_eq!(a.level("tarkov"), 0.84 * f);
        s.disengage(&mut a).unwrap();
        assert!(
            host.journal().unwrap().is_none(),
            "deleted after the restore"
        );
        assert_eq!(a.level("tarkov"), 0.84);

        // Crash, then the setting is off at the next start: restored, not engaged.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        drop(s);
        let mut s = soft(&host);
        assert_eq!(s.recover_off(&mut a, DEV), 3);
        assert_eq!(a.level("tarkov"), 0.84);
        assert_eq!(a.w().endpoint, 0.35);
        assert!(host.journal().unwrap().is_none());

        // An app that closed while scaled starts at Windows' saved, scaled level: its own back.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        a.w().sessions.retain(|x| x.id != "tarkov");
        s.gone("tarkov");
        s.disengage(&mut a).unwrap();
        assert!(host
            .journal()
            .unwrap()
            .unwrap()
            .apps
            .contains_key("sid-tarkov"));
        let back = a.add("tarkov-2", "sid-tarkov", 0.84 * f);
        s.created(&mut a, &back);
        assert_eq!(a.level("tarkov-2"), 0.84);
        assert!(host.journal().unwrap().is_none());
    }

    #[test]
    fn t8_master_mute_unmutes_only_its_own() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        a.w().sessions[2].muted = true; // the user muted chat
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        assert_eq!(s.toggle_mute(&mut a), Ok(true));
        assert!(a.w().endpoint_muted);
        assert!(a.muted("vivaldi") && a.muted("tarkov") && a.muted("chat"));
        // A new app during master mute joins muted.
        let new = a.add("game", "sid-game", 0.7);
        s.created(&mut a, &new);
        assert!(a.muted("game"));
        // The user unmutes vivaldi by itself: its mark goes.
        a.w().sessions[0].muted = false;
        s.app_muted("vivaldi", false);
        s.app_muted("nothing", false);
        assert_eq!(s.toggle_mute(&mut a), Ok(false));
        assert!(!a.muted("tarkov") && !a.muted("game"));
        assert!(a.muted("chat"), "the user's mute stays");
        // A master step unmutes too.
        s.toggle_mute(&mut a).unwrap();
        s.step_master(&mut a, 1).unwrap();
        assert_eq!(s.master(), Some((37, false)));
        assert!(!a.muted("tarkov") && a.muted("chat"));
    }

    #[test]
    fn stopping_while_master_is_muted_keeps_its_apps_muted_until_the_device_is_unmuted() {
        // #1070 (2): a device that ignores its volume may ignore its mute: unmuting the apps on
        // quit or switch-off would be louder. They stay muted until the device is unmuted.
        for restart in [false, true] {
            let host = MemHost::default();
            let mut s = soft(&host);
            let mut a = the_three();
            a.w().sessions[2].muted = true; // the user muted chat
            s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
            s.toggle_mute(&mut a).unwrap();
            s.disengage(&mut a).unwrap();
            assert!(a.w().endpoint_muted, "the device stays muted");
            for id in ["vivaldi", "tarkov", "chat"] {
                assert!(a.muted(id), "{id} stays muted");
                assert_eq!(
                    a.out(id, false),
                    0.0,
                    "silent where the mute is ignored: {id}"
                );
            }
            assert_eq!(a.level("tarkov"), 0.84, "at its own level");
            let j = host.journal().unwrap().unwrap();
            assert!(j.devices.is_empty(), "nothing to recover");
            assert!(j.apps["sid-tarkov"].by_master && !j.apps.contains_key("sid-chat"));
            if restart {
                s = soft(&host);
            }
            // Still muted: nothing changes, also not for an app that starts again.
            assert_eq!(s.unmute_left(&mut a), 0);
            assert_eq!(s.recover_off(&mut a, DEV), 0);
            a.w().sessions.retain(|x| x.id != "vivaldi");
            let mut back = a.add("vivaldi-2", "sid-vivaldi", 1.0);
            a.w().sessions.last_mut().unwrap().muted = true;
            back.muted = true;
            s.created(&mut a, &back);
            assert!(a.muted("vivaldi-2") && a.muted("tarkov"));
            // The user unmutes the device: the apps master muted follow; the user's mute stays.
            a.w().endpoint_muted = false;
            assert_eq!(s.unmute_left(&mut a), 2, "restart: {restart}");
            assert!(!a.muted("vivaldi-2") && !a.muted("tarkov"));
            assert!(a.muted("chat"), "the user's own mute stays");
            assert!(host.journal().unwrap().is_none(), "nothing left to do");
            assert_eq!(s.unmute_left(&mut a), 0);
        }
        // Switched on again while still muted: master is muted, and its unmute frees them.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        s.toggle_mute(&mut a).unwrap();
        s.disengage(&mut a).unwrap();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        assert_eq!(s.master(), Some((35, true)));
        assert!(a.muted("tarkov"));
        s.toggle_mute(&mut a).unwrap();
        assert!(!a.muted("tarkov") && !a.w().endpoint_muted);
    }

    #[test]
    fn the_knobs_unmute_while_off_frees_the_apps_a_muted_master_left_muted() {
        // Review #1072: the knob's own writes send no outside notification, so its unmute must
        // free them itself, by the step and by the double click.
        for by_step in [true, false] {
            let host = MemHost::default();
            let mut s = soft(&host);
            let mut a = the_three();
            s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
            s.toggle_mute(&mut a).unwrap();
            s.disengage(&mut a).unwrap();
            assert!(a.muted("tarkov") && a.w().endpoint_muted);
            if by_step {
                assert_eq!(s.plain_step(&mut a, 1), Ok(37));
            } else {
                assert_eq!(s.plain_mute(&mut a), Ok(false));
            }
            assert!(!a.w().endpoint_muted);
            assert!(
                !a.muted("tarkov") && !a.muted("vivaldi"),
                "by step: {by_step}"
            );
            assert!(host.journal().unwrap().is_none());
            // Muting again with the knob leaves the apps be: the device's mute is enough.
            assert_eq!(s.plain_mute(&mut a), Ok(true));
            assert!(!a.muted("tarkov"));
        }
    }

    #[test]
    fn a_journal_that_cant_be_read_is_kept_and_not_written_over() {
        // #1070 (4): a read error is not "no journal".
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        drop(s); // a crash: the apps scaled, the device held
        let crashed = host.journal().unwrap().unwrap();
        host.0.lock().unwrap().unreadable = true;
        let writes = host.0.lock().unwrap().writes;
        let mut s = soft(&host);
        assert!(s.journal_unread());
        a.w().log.clear();
        assert!(s.engage(&mut a, DEV, "Headphones", STEP).is_err());
        assert_eq!(s.engaged(), None);
        assert_eq!(s.recover_off(&mut a, DEV), 0);
        assert_eq!(s.unmute_left(&mut a), 0);
        let new = a.add("game", "sid-game", 0.7);
        s.created(&mut a, &new);
        assert!(a.w().log.is_empty(), "nothing changed: {:?}", a.w().log);
        let d = host.0.lock().unwrap();
        assert_eq!(d.writes, writes, "never written over");
        assert_eq!(d.journal.as_ref(), Some(&crashed));
        drop(d);
        // Readable again: the next start reads it and puts the crash right.
        host.0.lock().unwrap().unreadable = false;
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        assert!(!s.journal_unread());
        assert_eq!(
            s.own("tarkov"),
            Some(0.84),
            "the crash's own level, from the journal"
        );
        s.disengage(&mut a).unwrap();
        assert_eq!(a.level("tarkov"), 0.84);
    }

    #[test]
    fn t9_a_default_device_change_puts_the_old_one_back() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut old = the_three();
        s.engage(&mut old, DEV, "Headphones", STEP).unwrap();
        // The worker disengages the old device with its own objects, then looks at the new.
        s.disengage(&mut old).unwrap();
        assert_eq!(old.w().endpoint, 0.35);
        assert_eq!(old.level("tarkov"), 0.84);
        let mut new = Fake::new(0.5, &[("vivaldi", 1.0)]);
        assert!(
            !turned_on(eligible(0x0), false, None),
            "speakers: software volume"
        );
        assert_eq!(s.engaged(), None);
        assert_eq!(new.level("vivaldi"), 1.0);
        // And a second eligible device is set up on its own.
        s.engage(&mut new, "{other}", "Speakers", 0.04).unwrap();
        assert_eq!(new.w().endpoint, 0.96);
        assert_eq!(s.master(), Some((50, false)));
    }

    /// Lab-measured pairs (slider level, dB): the dongle's 35 % (11/10 research). The lab pass
    /// on a software-volume device adds more here.
    const MEASURED: &[(f32, f32)] = &[(0.35, -15.42)];

    #[test]
    fn t10_the_taper() {
        assert_eq!(amp(0.0), 0.0, "0 % is silent");
        assert_eq!(amp(1.0), 1.0);
        let mut last = 0.0;
        for p in 1..=100 {
            let a = amp(p as f32 / 100.0);
            assert!(a > last, "monotonic at {p} %");
            last = a;
        }
        for &(level, measured) in MEASURED {
            assert!(
                (db(level) - measured).abs() <= 0.5,
                "{level}: {} dB vs {measured} dB",
                db(level)
            );
        }
        assert_eq!(step_for(51), 0.02);
        assert_eq!(step_for(0), DEFAULT_STEP);
        assert_eq!(hold_for(0.02), 0.98);
        // A 2-step device doesn't hold at 0; a 1000-step one keeps steps it can tell apart.
        assert_eq!(step_for(2), MAX_STEP);
        assert_eq!(hold_for(step_for(2)), 0.9);
        assert_eq!(step_for(1000), MIN_STEP);
    }

    #[test]
    fn the_knob_steps_an_apps_own_level() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let r = s.step_app(&mut a, "tarkov", 2, Instant::now());
        assert_eq!(r, Some(Ok(88)));
        assert_eq!(a.level("tarkov"), 0.88 * amp(0.35));
        assert_eq!(s.step_app(&mut a, "nobody", 1, Instant::now()), None);
        // Master steps scale every app; Foxglove's own write is never read as the app's.
        s.step_master(&mut a, 5).unwrap();
        assert_eq!(a.level("tarkov"), 0.88 * amp(0.45));
        let echo = a.level("tarkov");
        assert_eq!(
            s.volume_event(&mut a, "tarkov", echo, false, Instant::now()),
            None
        );
        assert_eq!(s.own("tarkov"), Some(0.88));
    }

    #[test]
    fn a_journal_is_written_before_every_change() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        let j = host.journal().unwrap().unwrap();
        assert_eq!(j.devices[DEV].endpoint, DEV);
        assert_eq!(j.apps["sid-tarkov"].own, 0.84);
        let n = host.0.lock().unwrap().writes;
        s.step_master(&mut a, 1).unwrap();
        assert!(host.0.lock().unwrap().writes > n);
        assert_eq!(host.journal().unwrap().unwrap().devices[DEV].level, 0.37);
        // Old closed apps' records go after 30 days.
        let mut j = host.journal().unwrap().unwrap();
        j.apps.insert(
            "old".into(),
            Record {
                own: 1.0,
                applied: 0.1,
                by_master: false,
                day: 20_000 - 31,
            },
        );
        host.0.lock().unwrap().journal = Some(j);
        let s2 = soft(&host);
        assert!(!s2.journal().apps.contains_key("old"));
    }
}
