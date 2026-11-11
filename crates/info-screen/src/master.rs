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
//!   the device again in the handling of that same notification. A change of one key step from
//!   the held level is a key press ([`Soft::endpoint_event`]): that is why the hold is one step
//!   below the top (at the top, "volume up" sends no event at all);
//! - mutes master by muting the device and the apps, marked, and unmutes only the marked ones;
//! - takes an app's own write (a game's slider) as that app's own level and scales it again, at
//!   most every [`COALESCE`]; after [`FIGHT_WRITES`] writes in [`FIGHT_WINDOW`] it leaves that
//!   app alone.
//!
//! The pure core ([`Soft`]) runs over [`Audio`] (Windows' endpoint and sessions, or a test's) and
//! a [`Host`] (where the journal and the per-device choice live). Polling: none. Every call is
//! one event's handling; the only wait is a coalesced re-apply's deadline ([`Soft::next_due`]),
//! started by the app's write and ended by the deadline or the next event.

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
        1.0 / (count - 1) as f32
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
    #[serde(default)]
    pub active: Option<Active>,
    /// App records by session identifier: the live apps' and closed apps' (Windows starts a
    /// closed app at the scaled level it saved).
    #[serde(default)]
    pub apps: BTreeMap<String, Record>,
}

impl Journal {
    /// Nothing to keep: the file goes.
    pub fn is_empty(&self) -> bool {
        self.active.is_none() && self.apps.is_empty()
    }
}

/// Where the journal and the per-device facts live (the app's config folder; a test's memory).
pub trait Host: Send {
    /// The journal as last written (`None`: none).
    fn journal(&self) -> Option<Journal>;
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
    apps: HashMap<String, App>,
}

impl On {
    fn factor(&self) -> f32 {
        amp(self.level)
    }
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
}

impl Soft {
    /// Reads the journal; drops closed apps' records older than [`KEEP_DAYS`].
    pub fn new(host: Box<dyn Host>, today: i64) -> Soft {
        let mut journal = host.journal().unwrap_or_default();
        journal.apps.retain(|_, r| today - r.day <= KEEP_DAYS);
        Soft {
            host,
            journal,
            on: None,
            today,
        }
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
        let r = self.host.write_journal(&self.journal);
        if let Err(e) = &r {
            eprintln!("Foxglove: master volume: the journal can't be written: {e}");
        }
        r
    }

    /// Notes `on`'s state in the journal (master's and each live app's), and writes it.
    fn journal_on(&mut self) -> Result<(), String> {
        if let Some(on) = &self.on {
            self.journal.active = Some(Active {
                endpoint: on.endpoint.clone(),
                level: on.level,
                muted: on.muted,
                hold: on.hold,
            });
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
        let (scalar, muted) = audio.endpoint()?;
        let hold = hold_for(step);
        let crash = self
            .journal
            .active
            .clone()
            .filter(|a| a.endpoint == endpoint);
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
            self.journal.active = None;
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
    pub fn disengage(&mut self, audio: &mut dyn Audio) -> Result<(), String> {
        let Some(on) = self.on.take() else {
            return Ok(());
        };
        if let Err(e) = audio.set_endpoint(on.level) {
            self.on = Some(on);
            return Err(format!("the device's level can't be set back: {e}"));
        }
        let mut failed = 0;
        for (id, a) in &on.apps {
            if audio.set_level(id, a.own).is_err() {
                failed += 1;
                continue;
            }
            if a.by_master {
                let _ = audio.set_mute(id, false);
            }
            self.journal.apps.remove(&a.sid);
        }
        self.journal.active = None;
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
        Ok(())
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
        let Some(a) = self
            .journal
            .active
            .clone()
            .filter(|a| a.endpoint == endpoint)
        else {
            return 0;
        };
        if self.on.is_some() {
            return 0;
        }
        if let Ok((scalar, _)) = audio.endpoint() {
            if near(scalar, a.hold) && audio.set_endpoint(a.level).is_err() {
                return 0;
            }
        }
        let mut n = 0;
        for s in audio.sessions() {
            let Some(r) = self.journal.apps.get(&s.sid).cloned() else {
                continue;
            };
            if near(s.level, r.applied) && audio.set_level(&s.id, r.own).is_ok() {
                n += 1;
            }
            if r.by_master && s.muted {
                let _ = audio.set_mute(&s.id, false);
            }
            self.journal.apps.remove(&s.sid);
        }
        self.journal.active = None;
        let _ = self.save_journal();
        note_restored(n);
        eprintln!("Foxglove: master volume: restored {n} apps from the last run's journal");
        n
    }

    /// A new session on the device. On: it joins at its own level times master (muted when
    /// master is). Off: a closed app Windows started at the scaled level it saved gets its own
    /// level back.
    pub fn created(&mut self, audio: &mut dyn Audio, s: &Live) {
        let Some(on) = &self.on else {
            let Some(r) = self.journal.apps.get(&s.sid).cloned() else {
                return;
            };
            if near(s.level, r.applied) && !near(r.applied, r.own) {
                let _ = audio.set_level(&s.id, r.own);
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
        let by_master = master_muted && !s.muted;
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
        if by_master {
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
        app.own = level;
        app.applied = level;
        if app.self_managed {
            let _ = self.journal_on();
            return None;
        }
        app.writes.push_back(now);
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
                "Foxglove: master volume: an app set its own volume {} times in {} s; master leaves it alone",
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

    /// The earliest coalesced re-apply waiting.
    pub fn next_due(&self) -> Option<Instant> {
        self.on.as_ref()?.apps.values().filter_map(|a| a.due).min()
    }

    /// Re-applies the coalesced writes due by `now`; the next deadline.
    pub fn run_due(&mut self, audio: &mut dyn Audio, now: Instant) -> Option<Instant> {
        let on = self.on.as_mut()?;
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
        self.next_due()
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

    /// Windows says the device's level or mute changed, not by Foxglove (the flyout, the volume
    /// keys): it becomes master, and the device is held again here. One key step from the held
    /// level is a key press (master moves one step from where it is); anything else is a drag
    /// to that level. Whether master changed (Foxglove shows its card).
    pub fn endpoint_event(&mut self, audio: &mut dyn Audio, level: f32, muted: bool) -> bool {
        let Some(on) = self.on.as_mut() else {
            return false;
        };
        let mut changed = false;
        if muted != on.muted {
            self.mute_apps(audio, muted);
            changed = true;
        }
        let Some(on) = self.on.as_mut() else {
            return changed;
        };
        let off = level - on.hold;
        if off.abs() > SAME {
            let half = on.step / 2.0;
            on.level = if (off - on.step).abs() < half {
                (on.level + on.step).min(1.0)
            } else if (off + on.step).abs() < half {
                (on.level - on.step).max(0.0)
            } else {
                level
            };
            // Snap to Windows' grid, as its own keys leave it.
            on.level = (on.level * 100.0).round() / 100.0;
            let hold = on.hold;
            self.rescale(audio);
            if let Err(e) = audio.set_endpoint(hold) {
                eprintln!("Foxglove: master volume: the device can't be held: {e}");
            }
            changed = true;
        }
        changed
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
        writes: usize,
        choices: HashMap<String, bool>,
        saved: HashMap<String, (f32, bool)>,
    }

    #[derive(Clone, Default)]
    struct MemHost(Arc<Mutex<Disk>>);

    impl Host for MemHost {
        fn journal(&self) -> Option<Journal> {
            self.0.lock().unwrap().journal.clone()
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
            host.journal().is_none(),
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

    #[test]
    fn t5_an_outside_change_is_master_and_the_device_is_held_again() {
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        // A flyout drag to 60 %.
        a.w().endpoint = 0.60;
        assert!(s.endpoint_event(&mut a, 0.60, false));
        assert_eq!(s.master(), Some((60, false)));
        assert_eq!(a.level("tarkov"), 0.84 * amp(0.60));
        assert_eq!(a.w().endpoint, 0.98, "held again");
        // Volume keys: one step up from the hold, then one down, then down from 0 % stays 0.
        a.w().endpoint = 1.0;
        assert!(s.endpoint_event(&mut a, 1.0, false));
        assert_eq!(s.master(), Some((62, false)));
        assert_eq!(a.w().endpoint, 0.98);
        assert!(s.endpoint_event(&mut a, 0.96, false));
        assert_eq!(s.master(), Some((60, false)));
        // From the top and from the bottom: keys still work both ways.
        assert!(s.endpoint_event(&mut a, 0.10, false));
        for _ in 0..50 {
            s.endpoint_event(&mut a, 1.0, false);
        }
        assert_eq!(s.master(), Some((100, false)));
        s.endpoint_event(&mut a, 0.96, false);
        assert_eq!(s.master(), Some((98, false)));
        // The hold itself (an echo of Foxglove's own) changes nothing.
        assert!(!s.endpoint_event(&mut a, 0.98, false));
        // The mute key: the apps muted and marked.
        assert!(s.endpoint_event(&mut a, 0.98, true));
        assert!(a.muted("tarkov"));
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
        assert_eq!(s.run_due(&mut a, t0 + COALESCE), None);
        assert_eq!(a.level("tarkov"), 0.9 * f);
        // Twelve writes in 10 s: it sets its own volume; no more writes, its own level stays.
        let mut t = t0 + Duration::from_secs(1);
        for i in 0..12 {
            let l = 0.5 + i as f32 * 0.03;
            a.w().sessions[1].level = l;
            s.volume_event(&mut a, "tarkov", l, false, t);
            t += Duration::from_millis(700);
        }
        a.w().log.clear();
        assert_eq!(s.app_volume("tarkov"), Some(AppVolume::SelfManaged));
        assert_eq!(s.next_due(), None);
        let own = s.own("tarkov").unwrap();
        assert_eq!(a.level("tarkov"), own, "its own level, unscaled");
        s.step_master(&mut a, -5).unwrap();
        assert!(!a.w().log.iter().any(|l| l.starts_with("tarkov")));
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
        assert!(host.journal().is_none(), "deleted after the restore");
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
        assert!(host.journal().is_none());

        // An app that closed while scaled starts at Windows' saved, scaled level: its own back.
        let host = MemHost::default();
        let mut s = soft(&host);
        let mut a = the_three();
        s.engage(&mut a, DEV, "Headphones", STEP).unwrap();
        a.w().sessions.retain(|x| x.id != "tarkov");
        s.gone("tarkov");
        s.disengage(&mut a).unwrap();
        assert!(host.journal().unwrap().apps.contains_key("sid-tarkov"));
        let back = a.add("tarkov-2", "sid-tarkov", 0.84 * f);
        s.created(&mut a, &back);
        assert_eq!(a.level("tarkov-2"), 0.84);
        assert!(host.journal().is_none());
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
        let j = host.journal().unwrap();
        assert_eq!(j.active.as_ref().unwrap().endpoint, DEV);
        assert_eq!(j.apps["sid-tarkov"].own, 0.84);
        let n = host.0.lock().unwrap().writes;
        s.step_master(&mut a, 1).unwrap();
        assert!(host.0.lock().unwrap().writes > n);
        assert_eq!(host.journal().unwrap().active.unwrap().level, 0.37);
        // Old closed apps' records go after 30 days.
        let mut j = host.journal().unwrap();
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
