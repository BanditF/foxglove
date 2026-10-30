//! Right art on the shared host link. File I/O and all keyboard requests stay on the worker.
#![allow(dead_code)] // Handle consumed by the art settings and tray in #15/#16.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::art_cycle::{self, Cycle};
use super::art_triggers::{Change, Triggers};

use foxglove_link::art;
use foxglove_link::link::{Event as LinkEvent, EventSubscription, Link};
use foxglove_link::protocol::{self, ArtStatus, SlotInfo};
use foxglove_link::Transport;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Choice {
    /// -1 chooses a random built-in.
    BuiltIn(i8),
    File(PathBuf),
    /// A flash slot on the right half (firmware #123); survives the keyboard's reboots.
    Slot(u8),
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub status: Option<ArtStatus>,
    pub pinned: Option<Choice>,
    pub busy: bool,
    pub cancelled: bool,
    pub push_progress: Option<(u16, u16)>,
    pub error: Option<String>,
    /// The right half's flash slots, read on connect and after a save or erase.
    pub slots: Vec<SlotInfo>,
    /// The hotlist and how it cycles (#60).
    pub cycle: Cycle,
    /// The keyboard reports its lights turning on and off (HELLO RGB_STATE_EVENT).
    pub wake_events: bool,
    /// The file Foxglove last sent and a hash of its bytes, while the right half plays it from
    /// memory (whoever asked: a click, a layer, a preset). Cleared by a select and a disconnect.
    pub sent: Option<(PathBuf, u64)>,
    /// The pinned built-in's manifest name, so the pin follows it across firmware updates.
    pub pinned_name: Option<String>,
    /// What changed on the keyboard since Foxglove last saw it (a firmware update), in words,
    /// until the user dismisses it (#521).
    pub notice: Option<String>,
    /// What runs now, named for the sending strip (#518).
    pub job: Option<Job>,
    /// How the last job ended; the strip says so until the next one starts.
    pub outcome: Option<Outcome>,
    /// The running job is Foxglove's own (a re-apply, the relay coming up, Rotation's timer,
    /// layer or preset art): a command from the user or an API caller takes it over instead of
    /// being refused (#618). False for a job the user or an API caller started.
    pub background: bool,
    /// Counts claims on `busy`, so a background job sees that a user's command took it over.
    pub claim: u64,
}

/// What a job does, for the strip's words (`art_words`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Shows a piece: a built-in, a kept place, or one sent from this PC.
    Show,
    /// Sends a piece if it isn't showing, then keeps it in a place (#517).
    Keep,
    Remove,
    /// Reads the keyboard's art and its kept places.
    Read,
}

/// How to run a job again (the strip's Try again).
#[derive(Clone, Debug, PartialEq)]
pub enum Retry {
    Choose(Choice),
    Keep { file: PathBuf, slot: u8 },
    Erase(u8),
    Status,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub kind: JobKind,
    /// The piece's name as the user knows it.
    pub name: String,
    /// Started by Foxglove itself (Rotation, a put-back after a restart): no success line.
    pub quiet: bool,
    pub again: Option<Retry>,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub job: Job,
    /// The error as the service has it; `art_words` turns it into words.
    pub result: Result<(), String>,
    pub at: Instant,
}

/// The piece `choice` names, in the user's words.
pub fn name_of(choice: &Choice, s: &Snapshot) -> String {
    match choice {
        Choice::BuiltIn(-1) => "a piece of the keyboard's own".into(),
        Choice::BuiltIn(n) => s
            .built_ins()
            .into_iter()
            .find(|(i, _)| i == n)
            .map_or_else(|| format!("piece {}", n + 1), |(_, name)| name),
        Choice::Slot(n) => s
            .slots
            .iter()
            .find(|i| i.slot == *n && i.used)
            .map_or_else(|| format!("kept piece {}", n + 1), |i| i.name.clone()),
        Choice::File(p) => file_name(p),
    }
}

/// A library file's name as the page shows it.
pub fn file_name(p: &std::path::Path) -> String {
    p.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// The job a command from the UI (or the service's own triggers, `quiet`) starts.
fn job_for(command: &Command, s: &Snapshot, quiet: bool) -> Option<Job> {
    let (kind, name, again) = match command {
        Command::Choose(c) | Command::Show(c) => {
            (JobKind::Show, name_of(c, s), Retry::Choose(c.clone()))
        }
        Command::Keep { file, slot, name } => (
            JobKind::Keep,
            name.clone(),
            Retry::Keep {
                file: file.clone(),
                slot: *slot,
            },
        ),
        Command::Erase(n) => (
            JobKind::Remove,
            name_of(&Choice::Slot(*n), s),
            Retry::Erase(*n),
        ),
        Command::Status => (JobKind::Read, String::new(), Retry::Status),
        _ => return None,
    };
    Some(Job {
        kind,
        name,
        quiet,
        again: Some(again),
    })
}

impl Snapshot {
    /// The built-in sets with their names from the art manifest, or "Set n" when the firmware
    /// isn't the one the manifest describes.
    pub fn built_ins(&self) -> Vec<(i8, String)> {
        let count = self.status.map_or(0, |s| s.right_set_count).min(128);
        let names = super::art_manifest::sets(count);
        (0..count)
            .map(|n| {
                let label =
                    names.map_or_else(|| format!("Set {n}"), |s| s[usize::from(n)].label.clone());
                (n as i8, label)
            })
            .collect()
    }

    /// The built-in set the right half shows now, if it's playing one.
    pub fn showing_built_in(&self) -> Option<i8> {
        self.status
            .filter(|s| s.right_state == 0)
            .map(|s| s.right_set)
    }

    /// The library file the right half plays now: the pinned file, while it plays a pushed set.
    pub fn showing_file(&self) -> Option<&PathBuf> {
        match (&self.sent, self.status) {
            (Some((p, _)), Some(s))
                if matches!(
                    s.right_state,
                    protocol::right_state::PLAYING | protocol::right_state::SAVED
                ) =>
            {
                Some(p)
            }
            _ => None,
        }
    }

    /// The pin as this keyboard has it (#521): a built-in pinned by name follows the name.
    pub fn pin_now(&self) -> Option<Choice> {
        let count = self.status.map_or(0, |s| s.right_set_count);
        pin_for(
            &self.pinned,
            self.pinned_name.as_deref(),
            manifest_index(count),
        )
    }

    pub fn progress(&self) -> Option<(u16, u16)> {
        self.push_progress
    }
}

enum Command {
    Stop,
    /// The profile changed: its pinned art may differ.
    ProfileChanged,
    /// Wallpaper Engine shows this wallpaper now.
    Wallpaper(super::wallpaper::Wallpaper),
    Available(Arc<Link>),
    Link(Weak<Link>, LinkEvent),
    Status,
    Choose(Choice),
    /// Keep `file` in place `slot` under `name`: send it unless it shows now, save, show it
    /// from the place (#517).
    Keep {
        file: PathBuf,
        slot: u8,
        name: String,
    },
    Erase(u8),
    /// Show Rotation's next piece: the keyboard's own art (timer, lights wake, or Next).
    Next,
    /// Show Rotation's piece before the one on screen (the UI's ‹).
    Previous,
    /// The hotlist or its mode changed: reschedule.
    CycleChanged,
    /// The left's underglow is on or off (an RGB_STATE report, firmware #130), with the right
    /// half's on/off as the relay last read it (None: unknown).
    Lights {
        source: Weak<Link>,
        generation: u64,
        on: bool,
        right: Option<bool>,
    },
    /// The keyboard's active layers changed (the device event bus).
    Layers(Vec<u8>),
    /// A layer's art delay ran out (art_triggers::LAYER_DELAY).
    LayerDue,
    /// A lighting preset's art (`None` = no preset, or the preset doesn't change the art).
    Preset(Option<art_cycle::Item>),
    /// Show a trigger's art without pinning it.
    Show(Choice),
    /// Foxglove resumed (#404): show the art skipped while paused.
    Resume,
    /// An event that can have lost the right screen's art (`reassert.rs`).
    Reassert(super::reassert::Trigger),
    /// The tray's Next › (`true`) or ‹ from the user or an agent (#618): Rotation's step as
    /// their job, which already holds `busy` ([`Handle::step_now`]).
    Step(bool),
    /// A user's job that can't run after all: it ends with this error.
    Refused(String),
}

/// Whether the UI may send `command` (#404): while Foxglove is paused only reads go out.
fn allowed(command: &Command, paused: bool) -> bool {
    !paused || matches!(command, Command::Status)
}

/// A trigger changed while Foxglove is paused (#404): nothing shows, and `missed` remembers
/// that something should once it resumes. True when the change is held back.
fn held(change: &Change, paused: bool, missed: &mut bool) -> bool {
    if paused {
        *missed |= !matches!(change, Change::Nothing);
    }
    paused
}

#[derive(Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    state: Arc<Mutex<Snapshot>>,
    cancel: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Handle {
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn update(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
        // An agent waiting for a job's end (`wait_job`) looks again.
        *CHANGES.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        CHANGED.notify_all();
        // The live wallpaper feed names the piece on the right screen (#499).
        super::live_feed::art_changed();
        if let Some(wake) = WAKE.get() {
            wake.request_repaint();
        }
    }

    /// Sends a command from the user or an API caller. Refused only while another job they
    /// started runs: Foxglove's own job (a re-apply, Rotation's timer, ...) is taken over
    /// (#618). It skips its keyboard write once it sees that, and this command runs right after
    /// it on the worker, so what the user chose is what stays on screen.
    fn submit(&self, command: Command) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if user_refused(&state) || self.stop.load(Ordering::Relaxed) {
            return false;
        }
        if !allowed(&command, super::pause::paused()) {
            state.error = Some(super::pause::REFUSED.into());
            return false;
        }
        self.cancel.store(false, Ordering::Relaxed);
        if self.commands.send(command).is_err() {
            return false;
        }
        claim_for_user(&mut state);
        true
    }

    pub fn refresh(&self) -> bool {
        self.submit(Command::Status)
    }

    pub fn select(&self, set: i8) -> bool {
        set >= -1 && self.submit(Command::Choose(Choice::BuiltIn(set)))
    }

    pub fn push(&self, path: PathBuf) -> bool {
        self.submit(Command::Choose(Choice::File(path)))
    }

    /// Plays flash slot `slot` and pins it.
    pub fn play_slot(&self, slot: u8) -> bool {
        self.submit(Command::Choose(Choice::Slot(slot)))
    }

    /// Keeps library file `file` on the keyboard in place `slot` (replacing what it holds),
    /// named after the file: one job that sends it if needed, saves it and shows it from there.
    pub fn keep(&self, file: PathBuf, slot: u8) -> bool {
        let name = foxglove_link::protocol::art_name_fit(&file_name(&file)).to_string();
        self.submit(Command::Keep { file, slot, name })
    }

    pub fn erase_slot(&self, slot: u8) -> bool {
        self.submit(Command::Erase(slot))
    }

    /// Hides the firmware-change line.
    pub fn dismiss_notice(&self) {
        self.update(|s| s.notice = None);
    }

    /// Runs a job again (the strip's Try again).
    pub fn again(&self, retry: Retry) -> bool {
        match retry {
            Retry::Choose(c) => self.submit(Command::Choose(c)),
            Retry::Keep { file, slot } => self.keep(file, slot),
            Retry::Erase(n) => self.erase_slot(n),
            Retry::Status => self.refresh(),
        }
    }

    /// Shows Rotation's piece before the one on screen.
    pub fn previous(&self) -> bool {
        let _ = self.commands.send(Command::Previous);
        true
    }

    /// Replaces the hotlist and its mode, saves them, and reschedules the cycling.
    pub fn set_cycle(&self, cycle: Cycle) -> Result<(), String> {
        let saved = cycle.save().map_err(|e| format!("saving the hotlist: {e}"));
        self.update(|s| s.cycle = cycle);
        let _ = self.commands.send(Command::CycleChanged);
        saved
    }

    /// Tells the service which wallpaper is on screen; used in the "match the wallpaper" mode.
    /// A preset's art shows, or is cleared (`None`): today a running scene's wallpaper art
    /// with "Art screen too" on (#488, `scene_run::set_art`).
    pub fn preset_art(&self, art: Option<art_cycle::Item>) {
        let _ = self.commands.send(Command::Preset(art));
    }

    pub fn wallpaper(&self, w: super::wallpaper::Wallpaper) {
        let _ = self.commands.send(Command::Wallpaper(w));
    }

    /// Foxglove resumed (#404): the art held back while paused shows now.
    pub fn resumed(&self) {
        let _ = self.commands.send(Command::Resume);
    }

    /// The tray panel's Next › (`forward`) or ‹ for an agent (#618, the local API's
    /// `art.select`): the same command, once it's known that it would show something. Err says
    /// why not. Wait for the piece with [`Self::wait_job`].
    pub fn step_now(&self, forward: bool) -> Result<(), String> {
        if super::pause::paused() {
            return Err(super::pause::REFUSED.into());
        }
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let st = s
                .status
                .filter(|_| s.connected)
                .ok_or("no keyboard with the art relay")?;
            // Only a job the user (or an agent) started refuses it; Foxglove's own re-apply or
            // timer job is taken over (#618: it answered "another art job is running").
            if user_refused(&s) || self.stop.load(Ordering::Relaxed) {
                return Err("another art job is running".into());
            }
            art_cycle::step(
                &art_cycle::rotation(&st),
                art_cycle::on_screen(&st),
                forward,
            )
            .ok_or("the keyboard has nothing else to show")?;
            self.cancel.store(false, Ordering::Relaxed);
            self.commands
                .send(Command::Step(forward))
                .map_err(|_| "the art service has stopped")?;
            // The outcome is cleared, so the wait sees this job's end and not the last one's.
            claim_for_user(&mut s);
        }
        self.update(|_| {});
        Ok(())
    }

    /// Sleeps until the job just asked for has ended (no job running, its outcome in), at most
    /// `timeout`; woken by each snapshot change, no polling. The snapshot then.
    pub fn wait_job(&self, timeout: Duration) -> Snapshot {
        let changes = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
        let _ = CHANGED
            .wait_timeout_while(changes, timeout, |_| {
                let s = self.snapshot();
                s.busy || s.outcome.is_none()
            })
            .unwrap_or_else(|e| e.into_inner());
        self.snapshot()
    }

    /// The snapshot once the keyboard's first art status is in, at most `timeout` (#618: a
    /// Foxglove just started answered `art.status` with "no art status from the keyboard yet"
    /// while its connect read was still on the way). Woken by each snapshot change, no polling;
    /// at once when a status is there.
    pub fn wait_status(&self, timeout: Duration) -> Snapshot {
        let changes = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
        let _ = CHANGED
            .wait_timeout_while(changes, timeout, |_| self.snapshot().status.is_none())
            .unwrap_or_else(|e| e.into_inner());
        self.snapshot()
    }

    /// Shows Rotation's next piece now.
    pub fn next(&self) -> bool {
        let _ = self.commands.send(Command::Next);
        true
    }

    /// Stops further writes/polls; a relay already committed may finish on the keyboard.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed)
    }
}

/// A command from the user or an API caller can't start: a job they started runs (#618).
fn user_refused(s: &Snapshot) -> bool {
    s.busy && !s.background
}

/// `busy` is the user's (or an API caller's) job now; a background job that held it sees its
/// claim gone and leaves the keyboard alone (#618).
fn claim_for_user(s: &mut Snapshot) {
    if s.busy {
        eprintln!("Foxglove: art: a user's choice takes over Foxglove's own art job");
    }
    s.busy = true;
    s.background = false;
    s.claim = s.claim.wrapping_add(1);
    s.cancelled = false;
    s.error = None;
    s.push_progress = None;
    s.outcome = None;
}

static SERVICE: OnceLock<Handle> = OnceLock::new();
/// Snapshot changes so far, and the condvar [`Handle::wait_job`] sleeps on.
static CHANGES: Mutex<u64> = Mutex::new(0);
static CHANGED: std::sync::Condvar = std::sync::Condvar::new();
static WAKE: OnceLock<crate::ui_wake::UiWake> = OnceLock::new();

/// Repaints the UI (settings, tray) whenever the snapshot changes, so it needn't poll.
pub fn set_wake(wake: crate::ui_wake::UiWake) {
    let _ = WAKE.set(wake);
}

pub fn handle() -> Option<Handle> {
    SERVICE.get().cloned()
}

pub fn start() {
    SERVICE.get_or_init(|| {
        let (commands, receiver) = mpsc::channel();
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        forward_events(&h);
        let reassert = Mutex::new(h.commands.clone());
        super::reassert::register("art", move |t| {
            let _ = reassert
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(Command::Reassert(t));
        });
        // Supervised (#587): a panic restarts the worker with backoff (it used to restart at
        // once, without a limit). The command queue outlives the attempt.
        let worker = h.clone();
        let receiver = Mutex::new(receiver);
        let restarted = AtomicBool::new(false);
        let started = super::services::spawn_supervised("art", move || {
            if worker.stop.load(Ordering::Relaxed) {
                return;
            }
            if restarted.swap(true, Ordering::Relaxed) {
                worker.update(|s| {
                    s.busy = false;
                    s.connected = false;
                    s.error = Some("Art worker panicked; restarted".into());
                });
            }
            run(&worker, &receiver.lock().unwrap_or_else(|e| e.into_inner()));
        });
        if started.is_none() {
            h.update(|s| s.error = Some("Can't start the art worker".into()));
        }
        h
    });
}

pub fn stop() {
    if let Some(h) = SERVICE.get() {
        h.stop.store(true, Ordering::Relaxed);
        h.cancel();
        let _ = h.commands.send(Command::Stop);
    }
}

/// The `[art]` section of the settings store: the pinned choice, re-applied after the right
/// half power-cycles. Imported once from `art.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ArtSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned: Option<Choice>,
    /// A pinned built-in's manifest name (#521): indexes change between firmware builds.
    #[serde(skip_serializing_if = "Option::is_none")]
    built_in: Option<String>,
}

/// A pinned built-in, followed by name to its index on this firmware (`None` when the
/// firmware's manifest no longer has it); anything else as pinned. `index_of` answers
/// `None` when no manifest knows the firmware: then the saved index stands.
pub fn pin_for(
    pinned: &Option<Choice>,
    name: Option<&str>,
    index_of: impl Fn(&str) -> Option<Option<u8>>,
) -> Option<Choice> {
    match (pinned, name) {
        (Some(Choice::BuiltIn(n)), Some(name)) if *n >= 0 => match index_of(name) {
            None => pinned.clone(),
            Some(found) => found
                .and_then(|i| i8::try_from(i).ok())
                .map(Choice::BuiltIn),
        },
        _ => pinned.clone(),
    }
}

/// A built-in's index by manifest name, on firmware with `set_count` built-ins; `None` when
/// no manifest matches that firmware.
fn manifest_index(set_count: u8) -> impl Fn(&str) -> Option<Option<u8>> {
    move |name| {
        let sets = super::art_manifest::sets(set_count)?;
        Some(sets.iter().find(|s| s.name == name).map(|s| s.index))
    }
}

/// Saves the pin, with a built-in's manifest name when the manifest knows it.
fn save_pin(h: &Handle, choice: &Choice) -> Result<(), String> {
    let count = h.snapshot().status.map_or(0, |s| s.right_set_count);
    let built_in = match choice {
        Choice::BuiltIn(n) if *n >= 0 => super::art_manifest::sets(count)
            .and_then(|sets| sets.get(usize::try_from(*n).ok()?))
            .map(|s| s.name.clone()),
        _ => None,
    };
    h.update(|s| {
        s.pinned = Some(choice.clone());
        s.pinned_name = built_in.clone();
    });
    super::store::save(
        "art",
        &ArtSettings {
            pinned: Some(choice.clone()),
            built_in,
        },
    )
    .map_err(|e| format!("saving pinned art: {e}"))
}

/// Remembers what the keyboard holds; on a connect, first says what changed since (#521).
fn note_seen(h: &Handle, announce: bool) {
    let snap = h.snapshot();
    let Some(now) = super::art_seen::Seen::of(&snap) else {
        return;
    };
    let before = super::art_seen::load();
    if before.as_ref() == Some(&now) {
        return;
    }
    if announce {
        if let Some(line) = before.and_then(|b| super::art_seen::change(&b, &now)) {
            h.update(|s| s.notice = Some(line));
        }
    }
    super::art_seen::save(&now);
}

fn load_pinned() -> (Option<Choice>, Option<String>) {
    let a = load_art_settings();
    (a.pinned, a.built_in)
}

fn load_art_settings() -> ArtSettings {
    super::store::load_or_migrate::<ArtSettings>("art", || {
        let json = crate::settings::Settings::config_file_path()?.with_file_name("art.json");
        let pinned = serde_json::from_slice::<Choice>(&std::fs::read(&json).ok()?).ok()?;
        Some((
            ArtSettings {
                pinned: Some(pinned),
                built_in: None,
            },
            vec![json],
        ))
    })
}

/// What a re-apply (#618) shows: as [`restore`], but nothing when the right half already shows
/// it (a SELECT restarts the animation, and a pushed file costs a send).
fn reapply(wanted: &Option<Choice>, status: ArtStatus) -> Option<Choice> {
    restore(wanted, status).filter(|c| art_cycle::on_screen(&status).as_ref() != Some(c))
}

/// An unasked STATUS `now` says the relay came up since the status before it (`was`; none
/// known yet counts as down): the firmware sends one when it does (art_relay_central.c).
fn came_up(was: Option<ArtStatus>, now: ArtStatus) -> bool {
    now.link == 2 && was.is_none_or(|w| w.link != 2)
}

fn restore(pinned: &Option<Choice>, status: ArtStatus) -> Option<Choice> {
    use protocol::right_state::{BUILT_IN, SAVING};
    // Only link=2 has a ready relay; 1 is connected but still discovering it. A pinned built-in
    // or kept piece wins over anything else the right shows, a set pushed behind Foxglove's back
    // included (#671: the right played that push after a display change). A pinned file goes out
    // again only once the right lost it (back on a built-in): a set playing may be that file.
    // Never while the right receives (1) or saves (SAVING).
    let settled = status.right_state != 1 && status.right_state != SAVING;
    pinned.clone().filter(|c| {
        status.link == 2
            && match c {
                Choice::File(_) => status.right_state == BUILT_IN,
                _ => settled,
            }
    })
}

fn fresh_status(link: &Link, h: &Handle) -> Result<ArtStatus, String> {
    art::status(link).map_err(|e| e.to_string())?;
    std::thread::sleep(Duration::from_millis(300));
    if h.cancelled() {
        return Err("cancelled".into());
    }
    art::status(link).map_err(|e| e.to_string())
}

/// How long after a SELECT the right half has switched sets (its display queue runs it).
const SELECT_SETTLE: Duration = Duration::from_millis(300);

/// How long an art write waits for the right half's relay to come back (#662): a re-link (the
/// relay's unstick dropping the split link, the right rebooting, a range blip) takes ~20 s from
/// the drop to discovery done.
#[cfg(not(test))]
const RELAY_WAIT: Duration = Duration::from_secs(30);
#[cfg(test)]
const RELAY_WAIT: Duration = Duration::from_millis(900);

/// The left's relay said the right half isn't there (art STATUS error 1, link below 2).
fn relay_down(e: &str) -> bool {
    e.contains(protocol::ERRORS[1])
}

/// Runs the art write `f` (a SELECT, a push, a keep) with the relay up (#662). While the right
/// half re-links, a write failed at once with "right half not connected" (the 5a50426 gate's art
/// Next › and ‹). Now, when the last status says the relay is down, or `f` fails that way, the
/// worker waits for the relay-ready event (D6 85 with link 2; `art::wait_relay_ready` asks STATUS
/// once and sleeps on the link's events, no polling) for at most `RELAY_WAIT`, then runs `f`
/// (again). Only when the relay doesn't come back in time does the error stand, with one log
/// line. Start: a write with the relay down. Stop: the relay-ready event, the deadline, the link
/// stopping.
fn with_relay(
    link: &Link,
    h: &Handle,
    what: &str,
    f: impl Fn() -> Result<ArtStatus, String>,
) -> Result<ArtStatus, String> {
    if h.snapshot().status.is_some_and(|s| s.link == 2) {
        match f() {
            Err(e) if relay_down(&e) => {}
            done => return done,
        }
    }
    if h.cancelled() {
        return Err("cancelled".into());
    }
    match art::wait_relay_ready(link, RELAY_WAIT).map_err(|e| e.to_string())? {
        art::RelayWait::Ready(st) => {
            // Known before the relay-up notice reaches the worker: that notice then re-applies
            // nothing over this write (came_up).
            h.update(|s| s.status = Some(st));
            if h.cancelled() {
                return Err("cancelled".into());
            }
            f()
        }
        art::RelayWait::TimedOut(last) => {
            eprintln!(
                "Foxglove: art {what}: the right half's relay didn't come back within {} s ({})",
                RELAY_WAIT.as_secs_f32(),
                last.map_or_else(|| "no status".to_string(), |st| st.to_string())
            );
            if let Some(st) = last {
                h.update(|s| s.status = Some(st));
            }
            f()
        }
    }
}

fn choose(link: &impl Transport, h: &Handle, choice: &Choice) -> Result<ArtStatus, String> {
    if h.cancelled() {
        return Err("cancelled".into());
    }
    match choice {
        // Settled: the select's own reply names the set from before it, which left the
        // tray panel and the Screens page showing the old art.
        Choice::BuiltIn(set) => art::select_settled(link, *set, SELECT_SETTLE)
            .map_err(|e| e.to_string())
            .inspect(|_| h.update(|s| s.sent = None)),
        Choice::Slot(slot) => {
            let count = h
                .snapshot()
                .status
                .map(|s| s.right_set_count)
                .ok_or("No art status yet")?;
            art::select_slot(link, count, *slot)
                .map_err(|e| e.to_string())
                .inspect(|_| h.update(|s| s.sent = None))
        }
        Choice::File(path) => {
            let blob = pushed_bytes(path)?;
            if !art::is_blob(&blob) {
                return Err(format!("{} is not an art blob", path.display()));
            }
            if blob.len() <= u16::MAX as usize {
                h.update(|s| s.push_progress = Some((0, blob.len() as u16)));
            }
            art::push_cancellable(
                link,
                &blob,
                |st| {
                    h.update(|s| {
                        s.status = Some(*st);
                        s.push_progress = Some((st.pushed, st.length));
                    })
                },
                || h.cancelled(),
            )
            .map_err(|e| e.to_string())
            .inspect(|_| {
                let hash = super::art_library::blob_hash(&blob);
                h.update(|s| s.sent = Some((path.clone(), hash)))
            })
        }
    }
}

/// A blob file as it is sent: its bytes normalised to the piece's polarity (#666; Auto unless
/// the art picker or a plugin chose otherwise), so the panel polarity alone decides its look.
fn pushed_bytes(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let mut blob = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(65536).read_to_end(&mut blob))
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    Ok(super::art_polarity::blob(path, &blob))
}

/// Keep on keyboard (#517): send `file` unless the right half plays these very bytes from
/// memory now, save it into `slot` (SAVE stores the set the right half has in memory), then
/// show it from the place, which also checks that the place reads back. The kept places are
/// read again at the end, and after a failed save too (the firmware empties a place before it
/// writes it).
fn keep(
    link: &impl Transport,
    h: &Handle,
    file: &std::path::Path,
    slot: u8,
    name: &str,
) -> Result<ArtStatus, String> {
    let snap = h.snapshot();
    let hash = super::art_library::blob_hash(&pushed_bytes(file)?);
    let in_memory = snap.showing_file().map(PathBuf::as_path) == Some(file)
        && snap.sent.as_ref().is_some_and(|(_, h)| *h == hash);
    if !in_memory {
        choose(link, h, &Choice::File(file.to_path_buf()))?;
    }
    // The saving step: no byte count, and no Cancel (the send is done; the save is short).
    h.update(|s| s.push_progress = None);
    let saved = match art::save(link, slot, name) {
        Ok(st) => st,
        Err(e) => {
            if let Ok(st) = art::status(link) {
                read_slots(link, h, &st);
            }
            return Err(e.to_string());
        }
    };
    h.update(|s| s.status = Some(saved));
    let st = art::select_slot(link, saved.right_set_count, slot).map_err(|e| e.to_string())?;
    read_slots(link, h, &st);
    Ok(st)
}

/// Reads every flash slot the right half has; none on firmware without them.
fn read_slots(link: &impl Transport, h: &Handle, st: &ArtStatus) {
    let mut slots = Vec::new();
    for n in 0..st.flash_slots {
        if h.cancelled() {
            return;
        }
        match art::slot(link, n) {
            Ok(info) => slots.push(info),
            Err(e) => {
                h.update(|s| s.error = Some(format!("reading flash slot {n}: {e}")));
                break;
            }
        }
    }
    h.update(|s| s.slots = slots);
}

/// Profile switches and layer changes (layer art, #60) into the worker's queue, straight from
/// the facts and the device events. Started once: the queue outlives a worker restart, so a
/// restarted worker doesn't add a second pair (it did, each time it panicked).
fn forward_events(h: &Handle) {
    let subscribe = || super::devices::bus().facts().subscribe("profile.active");
    let (profile, profile_tx) = (super::services::First::new(subscribe()), h.commands.clone());
    let _ = super::services::spawn_supervised("art-profile", move || {
        for _ in profile.take_or(subscribe) {
            if profile_tx.send(Command::ProfileChanged).is_err() {
                return;
            }
        }
    });
    let subscribe = || super::devices::bus().subscribe(false);
    let (events, layers_tx) = (super::services::First::new(subscribe()), h.commands.clone());
    let _ = super::services::spawn_supervised("art-layers", move || {
        for event in events.take_or(subscribe) {
            if let foxglove_devices::DeviceEvent::LayerChanged { active } = event.event {
                if layers_tx.send(Command::Layers(active)).is_err() {
                    return;
                }
            }
        }
    });
}

fn run(h: &Handle, receiver: &mpsc::Receiver<Command>) {
    let (pinned, pinned_name) = load_pinned();
    h.update(|s| {
        s.pinned = pinned;
        s.pinned_name = pinned_name;
        s.cycle = Cycle::load();
    });
    let notifications = h.commands.clone();
    let _subscription = super::link::subscribe_links(move |link| {
        let _ = notifications.send(Command::Available(link));
    });
    // Discovery runs once. New shared links and each link's reconnects arrive as events.
    let _ = super::link::discover_links();
    // Coming back (#410) restarts the hotlist's timer, as resuming from a pause does.
    let back = h.commands.clone();
    let _away = super::away::subscribe(move |away| {
        if !away && !super::pause::paused() {
            let _ = back.send(Command::Resume);
        }
    });
    run_events(h, receiver);
}

/// Whether an RGB_STATE report shows the lights switched since the last on this connection:
/// the left on or off, or the right half's on/off or whether it is known (which comes as an
/// event with HELLO bit 20, #543). The left's first report after a connect counts, as before.
fn lights_switched(
    was: Option<bool>,
    right_was: Option<Option<bool>>,
    on: bool,
    right: Option<bool>,
) -> bool {
    was != Some(on) || right_was.is_some_and(|r| r != right)
}

fn run_events(h: &Handle, receiver: &mpsc::Receiver<Command>) {
    let mut links: Vec<(Arc<Link>, EventSubscription)> = Vec::new();
    let mut light_taps: Vec<EventSubscription> = Vec::new();
    // The left's lights as last reported on the current connection; off then on = a wake.
    let mut lights_on: Option<bool> = None;
    // The right half's on/off from the same reports (None inside: unknown).
    let mut right_on: Option<Option<bool>> = None;
    let mut current: Option<(usize, u64)> = None;
    // When the timed hotlist mode next changes the art; only while a keyboard is connected.
    let mut due: Option<Instant> = None;
    // Layer and preset art above the pinned choice (#60).
    let mut triggers = Triggers::default();
    // Art held back while Foxglove was paused (#404), to show on resume.
    let mut missed = false;
    while !h.stop.load(Ordering::Relaxed) {
        let next_deadline = match (due, triggers.deadline()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let received = match next_deadline {
            Some(at) => receiver.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        let command = match received {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if triggers.deadline().is_some_and(|at| at <= Instant::now()) {
                    Command::LayerDue
                } else {
                    Command::Next
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Clicked in the UI: these say how they went; Foxglove's own changes stay quiet.
        let from_ui = matches!(
            command,
            Command::Choose(_)
                | Command::Keep { .. }
                | Command::Erase(_)
                | Command::Status
                | Command::Step(_)
        );
        // Foxglove's own job (#618): it claims `busy` when it starts, and a user's command
        // queued meanwhile, or sent while it runs, wins.
        let mut background = false;
        // What a trigger change means on screen: its art, or the next one down / the pin.
        let shown = |change: Change, triggers: &Triggers| -> Option<Command> {
            let snap = h.snapshot();
            let (Some(st), false, true) = (snap.status, snap.busy, current.is_some()) else {
                return None;
            };
            let resolve = |item: &art_cycle::Item| {
                art_cycle::resolve(item, st.right_set_count, st.flash_used)
            };
            let choice = match change {
                Change::Nothing => return None,
                Change::Show(item) => resolve(&item)?,
                Change::Restore => triggers
                    .top(&snap.cycle)
                    .and_then(|i| resolve(&i))
                    .or(snap.pinned)?,
            };
            Some(Command::Show(choice))
        };
        let reschedule = |connected: bool| {
            connected
                .then(|| h.snapshot().cycle.period())
                .flatten()
                .map(|p| Instant::now() + p)
        };
        let command = match command {
            Command::Lights {
                source,
                generation,
                on,
                right,
            } => {
                let is_current = current.is_some_and(|(id, g)| {
                    g == generation && Weak::ptr_eq(&Arc::downgrade(&links[id].0), &source)
                });
                if !is_current {
                    continue;
                }
                let was = lights_on.replace(on);
                let right_was = right_on.replace(right);
                // Switched on the keyboard too: the tray panel and Lights page read them again.
                if lights_switched(was, right_was, on, right) {
                    super::keyboard_settings::lights_changed();
                }
                let woke = was == Some(false) && on;
                if !(woke && h.snapshot().cycle.steps_on(art_cycle::Step::LightsWake)) {
                    continue;
                }
                Command::Next
            }
            Command::Layers(active) => {
                let change = triggers.layers(&active, &h.snapshot().cycle, Instant::now());
                if held(&change, super::pause::paused(), &mut missed) {
                    continue;
                }
                match shown(change, &triggers) {
                    Some(command) => {
                        background = true;
                        command
                    }
                    None => continue,
                }
            }
            Command::LayerDue => {
                let change = triggers.due(&h.snapshot().cycle, Instant::now());
                if held(&change, super::pause::paused(), &mut missed) {
                    continue;
                }
                match shown(change, &triggers) {
                    Some(command) => {
                        background = true;
                        command
                    }
                    None => continue,
                }
            }
            Command::Preset(art) => {
                let change = triggers.preset(art);
                if held(&change, super::pause::paused(), &mut missed) {
                    continue;
                }
                match shown(change, &triggers) {
                    Some(command) => {
                        background = true;
                        command
                    }
                    None => continue,
                }
            }
            Command::Wallpaper(w) => {
                let snap = h.snapshot();
                let (Some(st), false, true) = (snap.status, snap.busy, current.is_some()) else {
                    continue;
                };
                // Layer or preset art is on top; the wallpaper's art shows when it ends.
                if snap.cycle.follow != art_cycle::Follow::Wallpaper
                    || triggers.top(&snap.cycle).is_some()
                {
                    continue;
                }
                match super::wallpaper::choice(&w, st.right_set_count) {
                    Some(choice)
                        if snap.pinned.as_ref() != Some(&choice) && super::pause::paused() =>
                    {
                        missed = true;
                        continue;
                    }
                    Some(choice) if snap.pinned.as_ref() != Some(&choice) => {
                        background = true;
                        Command::Choose(choice)
                    }
                    _ => continue,
                }
            }
            Command::ProfileChanged => {
                // The profile may have its own pinned art and hotlist.
                let (pinned, pinned_name) = load_pinned();
                let changed = h.snapshot().pinned != pinned;
                h.update(|s| {
                    s.pinned = pinned.clone();
                    s.pinned_name = pinned_name;
                    s.cycle = Cycle::load();
                });
                due = reschedule(current.is_some());
                let pinned = h.snapshot().pin_now();
                match pinned.filter(|_| changed && current.is_some() && !h.snapshot().busy) {
                    Some(_) if super::pause::paused() => {
                        missed = true;
                        continue;
                    }
                    Some(choice) => {
                        background = true;
                        Command::Choose(choice)
                    }
                    None => continue,
                }
            }
            Command::Resume => {
                due = reschedule(current.is_some());
                if !std::mem::take(&mut missed) {
                    continue;
                }
                let snap = h.snapshot();
                // Following the wallpaper: its art, unless layer or preset art is on top.
                if snap.cycle.follow == art_cycle::Follow::Wallpaper
                    && triggers.top(&snap.cycle).is_none()
                {
                    if let Some(w) =
                        super::wallpaper::current(snap.cycle.wallpaper_monitor.as_deref())
                    {
                        let _ = h.commands.send(Command::Wallpaper(w));
                    }
                    continue;
                }
                // Otherwise layer or preset art, or the pin.
                match shown(Change::Restore, &triggers) {
                    Some(command) => {
                        background = true;
                        command
                    }
                    None => continue,
                }
            }
            other => other,
        };
        let command = match command {
            Command::CycleChanged => {
                due = reschedule(current.is_some());
                // Turning on "match the wallpaper" shows the current wallpaper's art right away.
                if h.snapshot().cycle.follow == art_cycle::Follow::Wallpaper {
                    if let Some(w) =
                        super::wallpaper::current(h.snapshot().cycle.wallpaper_monitor.as_deref())
                    {
                        let _ = h.commands.send(Command::Wallpaper(w));
                    }
                }
                continue;
            }
            Command::Next | Command::Previous => {
                let forward = matches!(command, Command::Next);
                // Paused, or away (#410): the timer stops instead of firing to no effect;
                // resuming or coming back (Command::Resume) starts it again.
                if super::pause::paused() || super::away::away() {
                    due = None;
                    continue;
                }
                due = reschedule(current.is_some());
                let snap = h.snapshot();
                let (Some(st), false, true) = (snap.status, snap.busy, current.is_some()) else {
                    continue;
                };
                // Rotation waits while layer or preset art is on top.
                if triggers.top(&snap.cycle).is_some() {
                    continue;
                }
                // Rotation is the keyboard's own art (#515): what it stores is what it steps.
                let Some(choice) = art_cycle::step(
                    &art_cycle::rotation(&st),
                    art_cycle::on_screen(&st),
                    forward,
                ) else {
                    continue;
                };
                background = true;
                Command::Choose(choice)
            }
            // The user's (or an agent's) Next › / ‹: it holds `busy` already, and shows even
            // over layer or preset art.
            Command::Step(forward) => {
                due = reschedule(current.is_some());
                let step = h
                    .snapshot()
                    .status
                    .filter(|_| current.is_some())
                    .and_then(|st| {
                        art_cycle::step(
                            &art_cycle::rotation(&st),
                            art_cycle::on_screen(&st),
                            forward,
                        )
                    });
                match step {
                    Some(choice) => Command::Choose(choice),
                    None => Command::Refused("the keyboard has nothing else to show".into()),
                }
            }
            other => other,
        };
        let mut reported = None;
        // Set for a re-apply (`reassert.rs`): what asked.
        let mut reassert = None;
        // An unasked STATUS that says the relay just came up (#618).
        let mut relay_came_up = false;
        // On a new connection, the flash slots are read once its status is in.
        let mut connected_now = false;
        let automatic = match &command {
            Command::Stop => break,
            Command::Available(link) => {
                if links.iter().any(|(l, _)| Arc::ptr_eq(l, link)) {
                    continue;
                }
                let source = Arc::downgrade(link);
                let tx = h.commands.clone();
                let subscription = link.subscribe_events(move |event| {
                    let _ = tx.send(Command::Link(source.clone(), event));
                });
                let source = Arc::downgrade(link);
                let tx = h.commands.clone();
                // Runs on the link's reader thread: only enqueue.
                light_taps.push(link.subscribe_reports(move |generation, r| {
                    if protocol::is_reply(r, protocol::SETTINGS, protocol::settings::RGB_STATE) {
                        let state = protocol::RgbState::parse(r);
                        let _ = tx.send(Command::Lights {
                            source: source.clone(),
                            generation,
                            on: state.left.on,
                            right: state.right.map(|(half, _)| half.on),
                        });
                    }
                }));
                links.push((link.clone(), subscription));
                continue;
            }
            Command::Link(source, LinkEvent::State(state)) => {
                let Some(id) = links
                    .iter()
                    .position(|(l, _)| Weak::ptr_eq(&Arc::downgrade(l), source))
                else {
                    continue;
                };
                if current.is_some_and(|(active, _)| active == id) && state.connected.is_none() {
                    current = None;
                    due = None;
                    lights_on = None;
                    right_on = None;
                    triggers.disconnected();
                    h.update(|s| {
                        s.connected = false;
                        s.status = None;
                        s.sent = None;
                        // Read again on the next connect; never taken for that keyboard's.
                        s.slots = Vec::new();
                    });
                }
                if state.connected.is_none() || current == Some((id, state.generation)) {
                    continue;
                }
                let link = &links[id].0;
                let live = link.state();
                if live.generation != state.generation
                    || live.connected.is_none()
                    || super::link::ask_hello(link).is_none()
                {
                    continue;
                }
                current = Some((id, state.generation));
                let wake_events = super::link::ask_hello(link).is_some_and(|hello| {
                    hello.capabilities & protocol::capabilities::RGB_STATE_EVENT != 0
                });
                h.update(|s| {
                    s.connected = true;
                    s.wake_events = wake_events;
                });
                connected_now = true;
                due = reschedule(true);
                true
            }
            Command::Link(source, LinkEvent::ArtStatus { generation, report }) => {
                let Some(id) = links
                    .iter()
                    .position(|(l, _)| Weak::ptr_eq(&Arc::downgrade(l), source))
                else {
                    continue;
                };
                let live = links[id].0.state();
                if current != Some((id, *generation))
                    || live.generation != *generation
                    || live.connected.is_none()
                {
                    continue;
                }
                // Unasked STATUS is the firmware's relay up/down notice, or another host's STATUS
                // reply on the same HID device (the lab, a second Foxglove): every handle gets
                // every report. Only the relay coming up puts the art back (#618); answering
                // each one with a SELECT fought the lab's own selects and a second app's.
                let st = ArtStatus::parse(report);
                relay_came_up = came_up(h.snapshot().status, st);
                reported = Some(st);
                true
            }
            // PC back, a display change or a taskbar restart (#618): read the status and put
            // the art back if the right half lost it. Link events already do that above (a
            // connect, and the relay-ready STATUS event), so their re-apply is theirs.
            Command::Reassert(trigger) => {
                if trigger.is_link() || trigger.lighting_only() || current.is_none() {
                    continue;
                }
                reassert = Some(*trigger);
                true
            }
            _ => false,
        };
        // Paused (#404): a new connection gets its art back on resume instead.
        let paused_now = super::pause::paused();
        if paused_now && connected_now {
            missed = true;
        }
        // A UI command already queued owns busy/cancel until that command completes.
        let mut queued_command = false;
        // This job's claim on `busy`, while it is Foxglove's own: a user's command sent while
        // it runs takes it over (#618), and the job then leaves the keyboard alone.
        let mut mine = None;
        if automatic || background {
            h.update(|s| {
                queued_command = s.busy;
                if queued_command {
                    return;
                }
                h.cancel.store(false, Ordering::Relaxed);
                s.busy = true;
                s.background = true;
                s.claim = s.claim.wrapping_add(1);
                mine = Some(s.claim);
                s.cancelled = false;
                s.error = None;
                s.push_progress = None;
            });
        }
        // Foxglove's own art while a user's command waits in the queue: theirs wins.
        if background && queued_command {
            continue;
        }
        let taken_over = || mine.is_some_and(|m| h.snapshot().claim != m);
        if !automatic {
            let job = job_for(&command, &h.snapshot(), !from_ui);
            h.update(|s| s.job = job);
        }
        let result = if let Some((id, _)) = current {
            let link: &Link = links[id].0.as_ref();
            if automatic {
                reported
                    .map(Ok)
                    .unwrap_or_else(|| fresh_status(link, h))
                    .and_then(|st| {
                        h.update(|s| s.status = Some(st));
                        if connected_now && !queued_command {
                            read_slots(link, h, &st);
                            note_seen(h, true);
                        }
                        if !queued_command && !paused_now {
                            // Preset art (layers are unknown until the keyboard reports them)
                            // comes back over the pin after a reconnect.
                            let snap = h.snapshot();
                            let wanted = triggers
                                .top(&snap.cycle)
                                .and_then(|i| {
                                    art_cycle::resolve(&i, st.right_set_count, st.flash_used)
                                })
                                .or(snap.pin_now());
                            // What asked: a re-apply trigger, a new connection, or the relay
                            // coming up. Any other STATUS changes nothing (#618).
                            let why = reassert
                                .or(connected_now
                                    .then_some(super::reassert::Trigger::KeyboardConnected))
                                .or(relay_came_up
                                    .then_some(super::reassert::Trigger::RightRelayReady));
                            // Each leaves art the right half shows alone (a SELECT restarts
                            // the animation and costs relay round trips).
                            let choice = why
                                .and_then(|t| reapply(&wanted, st).map(|c| (c, t)))
                                .filter(|_| !taken_over())
                                .map(|(c, t)| {
                                    eprintln!(
                                        "{}",
                                        super::reassert::line("the right screen's art", t)
                                    );
                                    c
                                });
                            if let Some(choice) = choice {
                                h.update(|s| {
                                    s.job = Some(Job {
                                        kind: JobKind::Show,
                                        name: name_of(&choice, s),
                                        quiet: true,
                                        again: Some(Retry::Choose(choice.clone())),
                                    })
                                });
                                return choose(link, h, &choice);
                            }
                        }
                        Ok(st)
                    })
            } else {
                match command {
                    Command::Status => fresh_status(link, h).inspect(|st| read_slots(link, h, st)),
                    Command::Keep { file, slot, name } => {
                        with_relay(link, h, &format!("keep {name}"), || {
                            keep(link, h, &file, slot, &name)
                        })
                        .and_then(|st| {
                            // Pinned to the place: after a restart it shows from there, with no
                            // send.
                            h.update(|s| s.status = Some(st));
                            note_seen(h, false);
                            save_pin(h, &Choice::Slot(slot))?;
                            Ok(st)
                        })
                    }
                    Command::Erase(slot) => art::erase(link, slot)
                        .map_err(|e| e.to_string())
                        .inspect(|st| {
                            read_slots(link, h, st);
                            note_seen(h, false);
                        }),
                    // Foxglove's own art a user's command took over: theirs runs next.
                    Command::Show(_) | Command::Choose(_) if taken_over() => {
                        Err("taken over".into())
                    }
                    Command::Show(choice) => {
                        let what = name_of(&choice, &h.snapshot());
                        with_relay(link, h, &what, || choose(link, h, &choice))
                    }
                    Command::Refused(e) => Err(e),
                    Command::Choose(choice) => {
                        let what = name_of(&choice, &h.snapshot());
                        with_relay(link, h, &what, || choose(link, h, &choice)).and_then(|st| {
                            h.update(|s| s.status = Some(st));
                            save_pin(h, &choice)?;
                            Ok(st)
                        })
                    }
                    _ => unreachable!(),
                }
            }
        } else {
            Err("Waiting for a keyboard with the host link".into())
        };
        if queued_command {
            continue;
        }
        if taken_over() {
            // The user's command holds `busy` now and ends it.
            if let Ok(st) = result {
                h.update(|s| s.status = Some(st));
            }
            continue;
        }
        if let Err(e) = &result {
            // The UI shows words for it (art_words); the raw error is for the log.
            eprintln!("Foxglove: art: {e}");
        }
        h.update(|s| {
            s.busy = false;
            s.background = false;
            s.cancelled = h.cancelled();
            let job = s.job.take();
            match result {
                Ok(st) => {
                    if s.push_progress.is_some() {
                        s.push_progress = Some((st.pushed, st.length));
                    }
                    s.status = Some(st);
                    s.error = None;
                    // A status the keyboard sent on its own leaves the last outcome standing.
                    if let Some(job) = job {
                        s.outcome = Some(Outcome {
                            job,
                            result: Ok(()),
                            at: Instant::now(),
                        });
                    }
                }
                Err(e) => {
                    s.error = Some(e.clone());
                    // A failed read on connect has no job of its own: name it one.
                    let job = job.unwrap_or(Job {
                        kind: JobKind::Read,
                        name: String::new(),
                        quiet: true,
                        again: Some(Retry::Status),
                    });
                    s.outcome = Some(Outcome {
                        job,
                        result: Err(e),
                        at: Instant::now(),
                    });
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_link::protocol::{art as cmd, left_state, right_state, REPORT};
    use std::cell::{Cell, RefCell};

    /// The right half's own switch (HELLO bit 20 sends it as an RGB_STATE event, #543) reads
    /// the lights again, as the left's always did; a report that changes nothing doesn't.
    #[test]
    fn a_right_half_switch_reads_the_lights_again() {
        assert!(
            lights_switched(None, None, true, Some(true)),
            "the first report"
        );
        assert!(!lights_switched(
            Some(true),
            Some(Some(true)),
            true,
            Some(true)
        ));
        assert!(lights_switched(
            Some(true),
            Some(Some(true)),
            false,
            Some(true)
        ));
        assert!(lights_switched(
            Some(true),
            Some(Some(false)),
            true,
            Some(true)
        ));
        assert!(
            lights_switched(Some(true), Some(Some(true)), true, None),
            "the split link went"
        );
    }

    /// The keyboard on the far side of the link, answering the art relay as the firmware does
    /// (art_relay_central.c / art_relay_peripheral.c), with a log of the commands it got.
    #[derive(Default)]
    struct FakeKeyboard {
        log: RefCell<Vec<u8>>,
        left: Cell<u8>,
        right: Cell<u8>,
        set: Cell<i8>,
        used: Cell<u8>,
        pushed: Cell<u16>,
        kept: RefCell<Vec<(u8, String, u16)>>,
        save_fails: bool,
    }

    impl FakeKeyboard {
        fn status(&self, command: u8) -> [u8; REPORT] {
            let mut r = [0u8; REPORT];
            r[0] = protocol::ART;
            r[1] = 0x80 | command;
            r[2] = self.left.get();
            r[8] = 2; // link: relay ready
            r[9] = 3; // right status version with places
            r[10] = self.right.get();
            r[13..15].copy_from_slice(&16384u16.to_le_bytes());
            r[15] = self.set.get() as u8;
            r[16] = 17;
            r[24] = 8;
            r[25] = self.used.get();
            r
        }
    }

    impl foxglove_link::Transport for FakeKeyboard {
        fn send(&self, _: &[u8]) -> anyhow::Result<()> {
            Ok(()) // DATA: nothing to answer
        }

        fn request(
            &self,
            report: &[u8],
            _: u8,
            command: u8,
            _: Duration,
        ) -> anyhow::Result<[u8; REPORT]> {
            self.log.borrow_mut().push(command);
            match command {
                cmd::BEGIN => {
                    self.pushed.set(u16::from_le_bytes([report[2], report[3]]));
                    self.left.set(left_state::RECEIVING);
                }
                cmd::COMMIT => {
                    self.left.set(left_state::DONE);
                    self.right.set(right_state::PLAYING);
                    self.set.set(-1);
                }
                cmd::SAVE => {
                    let slot = report[2];
                    if self.save_fails {
                        self.right.set(right_state::SAVE_FAILED);
                    } else {
                        let name: Vec<u8> = report[3..]
                            .iter()
                            .copied()
                            .take_while(|b| *b != 0)
                            .collect();
                        let name = String::from_utf8_lossy(&name).into_owned();
                        self.kept.borrow_mut().push((slot, name, self.pushed.get()));
                        self.used.set(self.used.get() | 1 << slot);
                        self.right.set(right_state::SAVED);
                    }
                }
                cmd::SELECT => {
                    self.set.set(report[2] as i8);
                    self.right.set(right_state::BUILT_IN);
                }
                cmd::SLOT => {
                    let n = report[2];
                    let mut r = [0u8; REPORT];
                    r[0] = protocol::ART;
                    r[1] = 0x80 | cmd::SLOT;
                    r[3] = n;
                    if let Some((_, name, len)) = self.kept.borrow().iter().find(|k| k.0 == n) {
                        r[4] = 1;
                        r[9..11].copy_from_slice(&len.to_le_bytes());
                        r[11..11 + name.len()].copy_from_slice(name.as_bytes());
                    }
                    return Ok(r);
                }
                _ => {}
            }
            Ok(self.status(command))
        }
    }

    fn idle_handle(s: Snapshot) -> Handle {
        let (commands, _) = mpsc::channel();
        Handle {
            commands,
            state: Arc::new(Mutex::new(s)),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn fury() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/lighting/art/fury.bin")
    }

    /// Keep on keyboard from this PC (#517): send, save into the place, show it from there,
    /// read the places again: one job, in that order.
    #[test]
    fn keep_sends_saves_shows_and_rereads() {
        let kb = FakeKeyboard::default();
        let h = idle_handle(Snapshot::default());
        let st = keep(&kb, &h, &fury(), 2, "fury").expect("kept");
        let log = kb.log.borrow().clone();
        let first = |c: u8| log.iter().position(|x| *x == c).unwrap();
        assert!(first(cmd::BEGIN) < first(cmd::COMMIT));
        assert!(first(cmd::COMMIT) < first(cmd::SAVE));
        assert!(first(cmd::SAVE) < first(cmd::SELECT));
        assert!(first(cmd::SELECT) < first(cmd::SLOT));
        assert_eq!(
            log.iter().filter(|c| **c == cmd::SLOT).count(),
            8,
            "all places re-read"
        );
        assert_eq!(
            crate::foxglove::art_words::showing(&st),
            crate::foxglove::art_words::Showing::Kept(2),
            "shown from the place, not the sent copy"
        );
        let s = h.snapshot();
        let place = s.slots.iter().find(|i| i.slot == 2).unwrap();
        assert!(place.used);
        assert_eq!(place.name, "fury");
        assert_eq!(usize::from(place.len), std::fs::read(fury()).unwrap().len());
    }

    /// The piece already on screen from this file is saved as it is: no second send.
    #[test]
    fn keep_of_the_piece_on_screen_skips_the_send() {
        let kb = FakeKeyboard::default();
        kb.right.set(right_state::PLAYING);
        let hash = super::super::art_library::blob_hash(&std::fs::read(fury()).unwrap());
        let playing = Snapshot {
            sent: Some((fury(), hash)),
            status: Some(ArtStatus {
                right_state: right_state::PLAYING,
                ..Default::default()
            }),
            ..Default::default()
        };
        let h = idle_handle(playing.clone());
        keep(&kb, &h, &fury(), 0, "fury").expect("kept");
        assert!(
            !kb.log.borrow().contains(&cmd::BEGIN),
            "{:?}",
            kb.log.borrow()
        );
        assert_eq!(kb.log.borrow()[0], cmd::SAVE);

        // The pin names fury, but a layer sent other bytes since: Keep sends fury first, so
        // the place never gets the wrong piece under fury's name.
        let kb = FakeKeyboard::default();
        let h = idle_handle(Snapshot {
            pinned: Some(Choice::File(fury())),
            sent: Some((PathBuf::from("art/radar.bin"), 1)),
            ..playing.clone()
        });
        keep(&kb, &h, &fury(), 0, "fury").expect("kept");
        assert_eq!(kb.log.borrow()[0], cmd::BEGIN);
        // Same path, edited since it was sent: sent again.
        let kb = FakeKeyboard::default();
        let h = idle_handle(Snapshot {
            sent: Some((fury(), hash ^ 1)),
            ..playing
        });
        keep(&kb, &h, &fury(), 0, "fury").expect("kept");
        assert_eq!(kb.log.borrow()[0], cmd::BEGIN);
    }

    /// A failed save says so in words, and leaves the places as they were.
    #[test]
    fn a_failed_keep_is_named_and_changes_no_place() {
        let kb = FakeKeyboard {
            save_fails: true,
            ..Default::default()
        };
        let h = idle_handle(Snapshot::default());
        let e = keep(&kb, &h, &fury(), 1, "fury").unwrap_err();
        let st = ArtStatus::parse(&kb.status(cmd::STATUS));
        assert_eq!(
            crate::foxglove::art_words::problem(&e, Some(&st)),
            crate::foxglove::art_words::Problem::KeepFailed
        );
        assert!(!kb.log.borrow().contains(&cmd::SELECT));
        assert!(kb.kept.borrow().is_empty());
        assert!(
            kb.log.borrow().contains(&cmd::SLOT),
            "the places are read again: the firmware empties one before it writes it"
        );
    }

    /// A pinned built-in follows its name across firmware builds (#521): two manifests, the
    /// set moved from index 3 to 5, and a firmware without it.
    #[test]
    fn a_pinned_built_in_follows_its_name() {
        let old: &'static [&str] = &["A", "B", "C", "Bandit_Gem"];
        let new: &'static [&str] = &["A", "B", "C", "D", "E", "Bandit_Gem"];
        let look = |sets: &'static [&'static str]| {
            move |name: &str| Some(sets.iter().position(|n| *n == name).map(|i| i as u8))
        };
        let pin = Some(Choice::BuiltIn(3));
        assert_eq!(pin_for(&pin, Some("Bandit_Gem"), look(old)), pin);
        assert_eq!(
            pin_for(&pin, Some("Bandit_Gem"), look(new)),
            Some(Choice::BuiltIn(5))
        );
        assert_eq!(
            pin_for(&pin, Some("Bandit_Gem"), look(&["A"])),
            None,
            "gone from a known firmware: no pin"
        );
        assert_eq!(
            pin_for(&pin, Some("Bandit_Gem"), |_| None),
            pin,
            "unknown firmware: the saved index stands"
        );
        assert_eq!(
            pin_for(&pin, None, look(new)),
            pin,
            "an old pin without a name"
        );
        let random = Some(Choice::BuiltIn(-1));
        assert_eq!(pin_for(&random, Some("x"), look(new)), random);
        let slot = Some(Choice::Slot(2));
        assert_eq!(pin_for(&slot, Some("x"), look(new)), slot);
    }

    #[test]
    fn jobs_are_named_for_the_strip() {
        let s = Snapshot {
            slots: vec![SlotInfo {
                slot: 3,
                used: true,
                name: "Signal".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let job = job_for(&Command::Erase(3), &s, false).unwrap();
        assert_eq!((job.kind, job.name.as_str()), (JobKind::Remove, "Signal"));
        assert_eq!(job.again, Some(Retry::Erase(3)));
        let orbit = Command::Choose(Choice::File("art/Orbit.bin".into()));
        let job = job_for(&orbit, &s, true).unwrap();
        assert_eq!(
            (job.kind, job.name.as_str(), job.quiet),
            (JobKind::Show, "Orbit", true)
        );
        assert_eq!(name_of(&Choice::Slot(5), &s), "kept piece 6");
        assert!(job_for(&Command::Next, &s, false).is_none());
    }

    #[test]
    fn paused_the_ui_may_only_read() {
        assert!(allowed(&Command::Status, true));
        for c in [
            Command::Choose(Choice::BuiltIn(1)),
            Command::Keep {
                file: "x.bin".into(),
                slot: 0,
                name: "x".into(),
            },
            Command::Erase(0),
            Command::Show(Choice::Slot(0)),
        ] {
            assert!(!allowed(&c, true));
            assert!(allowed(&c, false));
        }
    }

    #[test]
    fn triggers_while_paused_wait_for_the_resume() {
        let mut missed = false;
        assert!(!held(&Change::Restore, false, &mut missed));
        assert!(!missed, "not paused: shown now, nothing missed");
        assert!(held(&Change::Nothing, true, &mut missed));
        assert!(!missed, "no change, nothing to catch up");
        assert!(held(&Change::Restore, true, &mut missed));
        assert!(missed);
        assert!(held(&Change::Nothing, true, &mut missed));
        assert!(missed, "stays missed until the resume takes it");
    }

    #[test]
    fn pinned_choices_round_trip_through_toml() {
        for pinned in [
            Choice::BuiltIn(-1),
            Choice::BuiltIn(3),
            Choice::File(PathBuf::from(r"C:\art\fox.bin")),
        ] {
            let a = ArtSettings {
                pinned: Some(pinned),
                built_in: Some("Bandit_Gem".into()),
            };
            let back: ArtSettings = toml::from_str(&toml::to_string(&a).unwrap()).unwrap();
            assert_eq!(back, a);
        }
    }

    #[test]
    fn handle_rejects_overlapping_commands_and_cancel_is_immediate() {
        let (commands, receiver) = mpsc::channel();
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        assert!(!h.select(-2));
        assert!(h.select(-1));
        assert!(h.snapshot().busy);
        assert!(!h.refresh());
        h.cancel();
        assert!(h.cancelled());
        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::Choose(Choice::BuiltIn(-1)))
        ));
        h.update(|s| s.busy = false);
        assert!(h.refresh());
        assert!(!h.cancelled());
        h.stop.store(true, Ordering::Relaxed);
        h.update(|s| s.busy = false);
        assert!(!h.refresh());
    }

    #[test]
    fn idle_worker_handles_commands_and_stop_without_a_link() {
        let (commands, receiver) = mpsc::channel();
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let worker = h.clone();
        let (done, finished) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            run_events(&worker, &receiver);
            done.send(()).unwrap();
        });
        assert!(h.refresh());
        // Stop is ordered after the command; it wakes recv even with the sender still alive.
        h.commands.send(Command::Stop).unwrap();
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        thread.join().unwrap();
        let snapshot = h.snapshot();
        assert!(!snapshot.busy);
        assert!(!snapshot.connected);
        assert_eq!(
            snapshot.error.as_deref(),
            Some("Waiting for a keyboard with the host link")
        );
    }

    #[test]
    fn restores_only_a_live_right_half_on_a_builtin() {
        let pinned = Some(Choice::File("art.bin".into()));
        let mut st = ArtStatus::default();
        assert_eq!(restore(&pinned, st), None);
        st.link = 1;
        assert_eq!(restore(&pinned, st), None);
        st.link = 2;
        assert_eq!(restore(&pinned, st), pinned);
        st.right_state = 2;
        assert_eq!(restore(&pinned, st), None);
        st.right_state = 0;
        assert_eq!(restore(&None, st), None);
    }

    /// #671 (On-call's overlay_tray run): a set pushed behind Foxglove's back plays (state 2,
    /// set -1); the pinned built-in comes back on the next re-apply, not the push.
    #[test]
    fn a_pinned_built_in_wins_over_a_push_behind_the_apps_back() {
        let st = ArtStatus {
            link: 2,
            right_state: protocol::right_state::PLAYING,
            right_set_count: 1,
            right_set: -1,
            ..ArtStatus::default()
        };
        let pinned = Some(Choice::BuiltIn(0));
        assert_eq!(reapply(&pinned, st), pinned);
        let kept = Some(Choice::Slot(1));
        let with_slot = ArtStatus {
            flash_slots: 8,
            flash_used: 0b10,
            ..st
        };
        assert_eq!(reapply(&kept, with_slot), kept);
        // Not while the right receives or saves.
        for busy in [1, protocol::right_state::SAVING] {
            let st = ArtStatus {
                right_state: busy,
                ..st
            };
            assert_eq!(reapply(&pinned, st), None, "state {busy}");
        }
    }

    /// #618: an agent's Next sends the tray's Next as its own job once it would show something;
    /// the wait ends with that job's outcome (woken by the snapshot change, not a timeout).
    #[test]
    fn an_agent_step_sends_next_and_waits_for_the_job() {
        let (commands, receiver) = mpsc::channel();
        let st = ArtStatus {
            link: 2,
            right_state: 0,
            right_set_count: 4,
            right_set: 1,
            ..ArtStatus::default()
        };
        let job = Job {
            kind: JobKind::Show,
            name: "ZMK_2".into(),
            quiet: true,
            again: None,
        };
        let done = |job: &Job| Outcome {
            job: job.clone(),
            result: Ok(()),
            at: Instant::now(),
        };
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot {
                connected: true,
                status: Some(st),
                outcome: Some(done(&job)),
                ..Snapshot::default()
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        h.step_now(true).unwrap();
        assert!(matches!(receiver.try_recv(), Ok(Command::Step(true))));
        assert!(
            h.snapshot().outcome.is_none(),
            "the last job's outcome is cleared"
        );
        let worker = h.clone();
        let ended = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            worker.update(|s| s.busy = true);
            worker.update(|s| {
                s.busy = false;
                if let Some(st) = s.status.as_mut() {
                    st.right_set = 2;
                }
                s.outcome = Some(done(&job));
            });
        });
        let started = Instant::now();
        let s = h.wait_job(Duration::from_secs(10));
        ended.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "woken, not timed out"
        );
        assert_eq!(s.status.map(|st| st.right_set), Some(2));
        h.step_now(false).unwrap();
        assert!(matches!(receiver.try_recv(), Ok(Command::Step(false))));
        // Busy or without the keyboard: refused with a reason, nothing sent.
        h.update(|s| s.busy = true);
        assert_eq!(h.step_now(true), Err("another art job is running".into()));
        h.update(|s| s.connected = false);
        assert!(h.step_now(true).unwrap_err().contains("no keyboard"));
        assert!(receiver.try_recv().is_err());
    }

    /// A re-apply (#618) leaves the right half's art alone when it shows what's wanted, and
    /// puts it back when the right half fell back to a piece of its own.
    #[test]
    fn a_reapply_skips_art_that_already_shows() {
        let mut st = ArtStatus {
            link: 2,
            right_state: 0,
            right_set_count: 8,
            right_set: 3,
            ..ArtStatus::default()
        };
        let three = Some(Choice::BuiltIn(3));
        assert_eq!(reapply(&three, st), None, "showing it");
        st.right_set = 5;
        assert_eq!(reapply(&three, st), three, "the right half picked its own");
        // A sent file the right half lost (it shows a built-in) goes out again.
        let file = Some(Choice::File("art.bin".into()));
        assert_eq!(reapply(&file, st), file);
        // No relay, or nothing wanted: nothing.
        st.link = 1;
        assert_eq!(reapply(&three, st), None);
        st.link = 2;
        assert_eq!(reapply(&None, st), None);
        // A kept place that plays is left alone.
        let mut kept = ArtStatus {
            flash_slots: 4,
            flash_used: 0b10,
            right_set: 9,
            ..st
        };
        assert_eq!(reapply(&Some(Choice::Slot(1)), kept), None);
        kept.right_set = 2;
        assert_eq!(reapply(&Some(Choice::Slot(1)), kept), Some(Choice::Slot(1)));
    }

    #[test]
    fn choices_round_trip_and_builtin_list_uses_firmware_count() {
        for choice in [
            Choice::BuiltIn(-1),
            Choice::BuiltIn(4),
            Choice::File(PathBuf::from(r"C:\art\fox.bin")),
        ] {
            assert_eq!(
                serde_json::from_str::<Choice>(&serde_json::to_string(&choice).unwrap()).unwrap(),
                choice
            );
        }
        let s = Snapshot {
            status: Some(ArtStatus {
                right_set_count: 3,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            s.built_ins(),
            vec![
                (0, "Set 0".into()),
                (1, "Set 1".into()),
                (2, "Set 2".into())
            ]
        );
    }
}

/// #618: the art service against an in-memory keyboard on a real host link, window-free. The
/// board counts every request, so the tests say how much the service asks the keyboard on a
/// connect, on reports and while idle, and that a user's choice wins over a re-apply.
#[cfg(test)]
mod contention_tests {
    use super::*;
    use foxglove_link::link::{Bus, Connected, RawHid, Report};
    use foxglove_link::protocol::{left_state, Hello, REPORT};
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicU32;
    use std::sync::Condvar;

    const WAIT: Duration = Duration::from_secs(10);

    /// The left half and the right half's art, as the relay reports them.
    #[derive(Default)]
    struct Board {
        set: Mutex<i8>,
        link: Mutex<u8>,
        /// How long a STATUS takes to answer (a slow relay): keeps a re-apply running.
        status_delay: Mutex<Duration>,
        statuses: AtomicU32,
        selects: AtomicU32,
        others: AtomicU32,
        inbox: (Mutex<VecDeque<Report>>, Condvar),
        interrupted: AtomicBool,
    }

    impl Board {
        fn new(set: i8) -> Arc<Board> {
            let b = Board::default();
            *b.set.lock().unwrap() = set;
            *b.link.lock().unwrap() = 2;
            Arc::new(b)
        }

        fn status(&self, cmd: u8, seq: u16) -> Report {
            let mut r = [0u8; REPORT];
            r[0] = protocol::ART;
            r[1] = 0x80 | cmd;
            r[6..8].copy_from_slice(&seq.to_le_bytes());
            r[8] = *self.link.lock().unwrap();
            r[9] = 2;
            r[15] = *self.set.lock().unwrap() as u8;
            r[16] = 6;
            r
        }

        fn push(&self, r: Report) {
            self.inbox.0.lock().unwrap().push_back(r);
            self.inbox.1.notify_all();
        }

        /// An art STATUS nobody here asked for: the firmware's relay notice, or another host's
        /// STATUS reply on the same HID device. `seq` marks it (in `pushed`).
        fn unasked(&self, seq: u16) {
            self.push(self.status(protocol::art::STATUS, seq));
        }

        fn requests(&self) -> (u32, u32, u32) {
            (
                self.statuses.load(Ordering::SeqCst),
                self.selects.load(Ordering::SeqCst),
                self.others.load(Ordering::SeqCst),
            )
        }
    }

    struct Conn(Arc<Board>);

    impl RawHid for Conn {
        fn read(&self, buf: &mut Report) -> anyhow::Result<usize> {
            let b = &self.0;
            let mut inbox = b.inbox.0.lock().unwrap();
            loop {
                if b.interrupted.load(Ordering::Relaxed) {
                    return Ok(0);
                }
                if let Some(r) = inbox.pop_front() {
                    *buf = r;
                    return Ok(REPORT);
                }
                inbox = b.inbox.1.wait(inbox).unwrap();
            }
        }

        fn interrupt(&self) {
            let _inbox = self.0.inbox.0.lock().unwrap();
            self.0.interrupted.store(true, Ordering::Relaxed);
            self.0.inbox.1.notify_all();
        }

        fn write(&self, out: &[u8]) -> anyhow::Result<()> {
            let b = &self.0;
            match (out[0], out[1]) {
                (protocol::ART, protocol::art::STATUS) => {
                    b.statuses.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(*b.status_delay.lock().unwrap());
                    b.push(b.status(protocol::art::STATUS, 0));
                }
                (protocol::ART, protocol::art::SELECT) => {
                    b.selects.fetch_add(1, Ordering::SeqCst);
                    let mut r = b.status(protocol::art::SELECT, 0);
                    if *b.link.lock().unwrap() == 2 {
                        *b.set.lock().unwrap() = out[2] as i8;
                    } else {
                        // The firmware: no relay to the right, the SELECT fails at once.
                        r[2] = left_state::FAILED;
                        r[3] = 1;
                    }
                    b.push(r);
                }
                _ => {
                    b.others.fetch_add(1, Ordering::SeqCst);
                }
            }
            Ok(())
        }
    }

    /// The service on the board's link with `pinned`, its connect job done.
    struct Rig {
        board: Arc<Board>,
        h: Handle,
        worker: Option<std::thread::JoinHandle<()>>,
        _link: Arc<Link>,
    }

    impl Rig {
        fn new(showing: i8, pinned: i8) -> Rig {
            crate::foxglove::store::test_config_dir();
            let board = Board::new(showing);
            let opened = board.clone();
            let link = Arc::new(Link::start(Box::new(move || {
                let connected = Connected {
                    product: "Fake".into(),
                    hello: Some(Hello {
                        version: 1,
                        width: 68,
                        height: 160,
                        right_link: 2,
                        ..Hello::default()
                    }),
                    bus: Bus::Unknown,
                };
                Ok((Box::new(Conn(opened.clone())) as Box<dyn RawHid>, connected))
            })));
            link.wait_connected(WAIT).unwrap();
            let (commands, receiver) = mpsc::channel();
            let h = Handle {
                commands,
                state: Arc::new(Mutex::new(Snapshot {
                    pinned: Some(Choice::BuiltIn(pinned)),
                    ..Snapshot::default()
                })),
                cancel: Arc::new(AtomicBool::new(false)),
                stop: Arc::new(AtomicBool::new(false)),
            };
            let w = h.clone();
            let worker = std::thread::spawn(move || run_events(&w, &receiver));
            h.commands.send(Command::Available(link.clone())).unwrap();
            let rig = Rig {
                board,
                h,
                worker: Some(worker),
                _link: link,
            };
            rig.until("the connect job ended", |s| {
                s.connected && !s.busy && s.status.is_some_and(|st| st.right_set == pinned)
            });
            rig
        }

        /// Sleeps on the service's snapshot changes until `done`, at most `WAIT`.
        fn until(&self, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
            let end = Instant::now() + WAIT;
            let mut changes = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let s = self.h.snapshot();
                if done(&s) {
                    return s;
                }
                let left = end.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "{what}: timed out; {:?}", s.status);
                changes = CHANGED
                    .wait_timeout(changes, left.min(Duration::from_millis(200)))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }

        /// `n` unasked STATUS reports, then the service done with the last one.
        fn reports(&self, n: u16) {
            for seq in 1..=n {
                self.board.unasked(seq);
            }
            self.until("the reports were handled", |s| {
                !s.busy && s.status.is_some_and(|st| st.pushed == n)
            });
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = self.h.commands.send(Command::Stop);
            if let Some(w) = self.worker.take() {
                let _ = w.join();
            }
        }
    }

    /// #618: `art.status` right after a start waits for the connect's status read (woken by the
    /// snapshot change), and with no keyboard it gives up after its bound.
    #[test]
    fn art_status_waits_for_the_first_status_bounded() {
        let (commands, _receiver) = mpsc::channel();
        let h = Handle {
            commands,
            state: Arc::new(Mutex::new(Snapshot::default())),
            cancel: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let started = Instant::now();
        assert!(h.wait_status(Duration::from_millis(150)).status.is_none());
        assert!(started.elapsed() >= Duration::from_millis(150));

        let w = h.clone();
        let arrives = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            w.update(|s| s.status = Some(ArtStatus::default()));
        });
        let started = Instant::now();
        assert!(h.wait_status(WAIT).status.is_some());
        assert!(
            started.elapsed() < WAIT,
            "woken by the status, not the bound"
        );
        arrives.join().unwrap();
        // A status already there: at once.
        let started = Instant::now();
        assert!(h.wait_status(WAIT).status.is_some());
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    /// A connect asks a bounded, counted number of requests: one fresh status read (two
    /// STATUS), and the pin only when the right half doesn't show it (a SELECT and its settled
    /// STATUS). Then, with no events, nothing at all reaches the keyboard.
    #[test]
    fn a_connect_asks_a_counted_few_and_idle_asks_nothing() {
        let rig = Rig::new(2, 2);
        assert_eq!(rig.board.requests(), (2, 0, 0), "the pin shows: no SELECT");
        let rig = Rig::new(1, 4);
        assert_eq!(rig.board.requests(), (3, 1, 0), "the pin put back once");
        // Idle: no events, no traffic (the service sleeps on its queue).
        let before = rig.board.requests();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(rig.board.requests(), before, "idle asks nothing");
        assert!(!rig.h.snapshot().busy);
    }

    /// Unasked STATUS (another host reading the relay, the lab's own selects) doesn't fight
    /// the right half's art: N of them cause no re-apply. The relay coming up causes one,
    /// however many reports follow it.
    #[test]
    fn n_status_reports_cause_at_most_one_reapply() {
        let rig = Rig::new(2, 2);
        let start = rig.board.requests();
        // Changed behind the app's back (the lab's direct SELECT), then other hosts' reads.
        *rig.board.set.lock().unwrap() = 0;
        rig.reports(5);
        assert_eq!(rig.board.requests(), start, "no request for 5 reports");
        assert_eq!(rig.h.snapshot().status.map(|s| s.right_set), Some(0));
        // The relay drops and comes back: its notice puts the pin back, once.
        *rig.board.link.lock().unwrap() = 1;
        rig.board.unasked(100);
        rig.until("the relay is down", |s| {
            !s.busy && s.status.is_some_and(|st| st.pushed == 100)
        });
        *rig.board.link.lock().unwrap() = 2;
        rig.reports(5);
        let (statuses, selects, others) = rig.board.requests();
        assert_eq!(selects - start.1, 1, "one re-apply for the relay coming up");
        assert_eq!(statuses - start.0, 1, "its settled read only");
        assert_eq!(others, 0);
        assert_eq!(*rig.board.set.lock().unwrap(), 2, "the pin shows again");
    }

    /// A user's (or an API caller's) choice while Foxglove's own re-apply runs is taken, not
    /// refused (#618: art.select answered "another art job is running"); the re-apply leaves the
    /// keyboard alone, and later re-applies keep what was chosen.
    #[test]
    fn a_choice_during_a_reapply_wins_and_stays() {
        let rig = Rig::new(2, 2);
        *rig.board.status_delay.lock().unwrap() = Duration::from_millis(400);
        *rig.board.set.lock().unwrap() = 0;
        let start = rig.board.requests();
        rig.h
            .commands
            .send(Command::Reassert(
                super::super::reassert::Trigger::DisplayChange,
            ))
            .unwrap();
        rig.until("the re-apply runs", |s| s.busy && s.background);
        assert!(rig.h.select(3), "a select during a re-apply is taken");
        let after = rig.h.wait_job(WAIT);
        assert!(
            matches!(after.outcome.as_ref().map(|o| &o.result), Some(Ok(()))),
            "{:?}",
            after.outcome
        );
        assert_eq!(*rig.board.set.lock().unwrap(), 3);
        assert_eq!(
            after.pinned,
            Some(Choice::BuiltIn(3)),
            "the choice is the pin"
        );
        assert_eq!(
            rig.board.requests().1 - start.1,
            1,
            "only the user's SELECT"
        );
        // The next display change finds the choice on screen: nothing re-applied.
        *rig.board.status_delay.lock().unwrap() = Duration::ZERO;
        let reads = rig.board.requests().0;
        rig.h
            .commands
            .send(Command::Reassert(
                super::super::reassert::Trigger::DisplayChange,
            ))
            .unwrap();
        rig.until("the second re-apply read the status", |s| {
            !s.busy && rig.board.requests().0 >= reads + 2
        });
        assert_eq!(
            rig.board.requests().1 - start.1,
            1,
            "the choice isn't reverted"
        );
        assert_eq!(*rig.board.set.lock().unwrap(), 3);
    }

    /// The tray's Next › (and an agent's art.select next) during a re-apply: taken, and shown.
    #[test]
    fn a_step_during_a_reapply_is_taken() {
        let rig = Rig::new(2, 2);
        *rig.board.status_delay.lock().unwrap() = Duration::from_millis(400);
        rig.h
            .commands
            .send(Command::Reassert(super::super::reassert::Trigger::PcBack))
            .unwrap();
        rig.until("the re-apply runs", |s| s.busy && s.background);
        rig.h.step_now(true).expect("Next during a re-apply");
        let after = rig.h.wait_job(WAIT);
        assert!(
            matches!(after.outcome.as_ref().map(|o| &o.result), Some(Ok(()))),
            "{:?}",
            after.outcome
        );
        assert_eq!(*rig.board.set.lock().unwrap(), 3, "the next piece");
        // A job the user started still refuses a second one.
        rig.h.update(|s| {
            s.busy = true;
            s.background = false;
        });
        assert_eq!(
            rig.h.step_now(true),
            Err("another art job is running".into())
        );
        assert!(!rig.h.select(1));
        rig.h.update(|s| s.busy = false);
    }

    /// #662: a choice while the right half re-links (the art Next › of the 5a50426 gate, right
    /// after the relay's unstick dropped the split link) waits for the relay-ready event and
    /// then shows, instead of failing with "right half not connected".
    #[test]
    fn a_choice_while_the_relay_relinks_waits_for_it() {
        let rig = Rig::new(2, 2);
        *rig.board.link.lock().unwrap() = 1;
        rig.board.unasked(50);
        rig.until("the relay is down", |s| {
            !s.busy && s.status.is_some_and(|st| st.pushed == 50 && st.link == 1)
        });
        let board = rig.board.clone();
        let back = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            *board.link.lock().unwrap() = 2;
            board.unasked(51); // the firmware's relay-ready notice (D6 85, link 2)
        });
        let start = rig.board.requests();
        assert!(rig.h.select(3));
        let after = rig.h.wait_job(WAIT);
        back.join().unwrap();
        assert!(
            matches!(after.outcome.as_ref().map(|o| &o.result), Some(Ok(()))),
            "{:?}",
            after.outcome
        );
        assert_eq!(*rig.board.set.lock().unwrap(), 3);
        assert_eq!(
            rig.board.requests().1 - start.1,
            1,
            "one SELECT, sent once the relay was back"
        );
        assert_eq!(after.pinned, Some(Choice::BuiltIn(3)));
    }

    /// #662: the relay drops while the app still thinks it's up (no notice yet): the SELECT
    /// fails "not connected", the worker waits for the relay and sends it again, once.
    #[test]
    fn a_select_failing_not_connected_is_sent_again_after_the_relink() {
        let rig = Rig::new(2, 2);
        *rig.board.link.lock().unwrap() = 1; // the app's last status still says 2
        let board = rig.board.clone();
        let back = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            *board.link.lock().unwrap() = 2;
            board.unasked(60);
        });
        let start = rig.board.requests();
        assert!(rig.h.select(4));
        let after = rig.h.wait_job(WAIT);
        back.join().unwrap();
        assert!(
            matches!(after.outcome.as_ref().map(|o| &o.result), Some(Ok(()))),
            "{:?}",
            after.outcome
        );
        assert_eq!(*rig.board.set.lock().unwrap(), 4);
        assert_eq!(
            rig.board.requests().1 - start.1,
            2,
            "the failed SELECT and its one retry"
        );
    }

    /// #662: a relay that doesn't come back within the bound: the error stands, after the wait
    /// (bounded, not forever), and nothing on the right changed.
    #[test]
    fn a_relay_that_stays_down_fails_after_the_bound() {
        let rig = Rig::new(2, 2);
        *rig.board.link.lock().unwrap() = 1;
        rig.board.unasked(70);
        rig.until("the relay is down", |s| {
            !s.busy && s.status.is_some_and(|st| st.pushed == 70 && st.link == 1)
        });
        let started = Instant::now();
        assert!(rig.h.select(3));
        let after = rig.h.wait_job(WAIT);
        let took = started.elapsed();
        let err = match after.outcome.as_ref().map(|o| &o.result) {
            Some(Err(e)) => e.clone(),
            other => panic!("expected the error, got {other:?}"),
        };
        assert!(relay_down(&err), "{err}");
        assert!(took >= RELAY_WAIT, "waited {took:?}");
        assert!(took < WAIT, "bounded: {took:?}");
        assert_eq!(*rig.board.set.lock().unwrap(), 2);
    }
}
