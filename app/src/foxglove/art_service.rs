//! Right art on the shared host link. File I/O and all keyboard requests stay on the worker.
#![allow(dead_code)] // Handle consumed by the art settings and tray in #15/#16.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::art_cycle::{self, Cycle};
use super::art_triggers::{Change, Triggers};

use digitalis::art;
use digitalis::link::{Event as LinkEvent, EventSubscription, Link};
use digitalis::protocol::{self, ArtStatus, SlotInfo};
use digitalis::Transport;
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
    /// The left's underglow is on or off (an RGB_STATE report, firmware #130).
    Lights {
        source: Weak<Link>,
        generation: u64,
        on: bool,
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
        // The live wallpaper feed names the piece on the right screen (#499).
        super::live_feed::art_changed();
        if let Some(wake) = WAKE.get() {
            wake.request_repaint();
        }
    }

    fn submit(&self, command: Command) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.busy || self.stop.load(Ordering::Relaxed) {
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
        state.busy = true;
        state.cancelled = false;
        state.error = None;
        state.push_progress = None;
        state.outcome = None;
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
        let name = digitalis::protocol::art_name_fit(&file_name(&file)).to_string();
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
    /// A lighting preset was applied with this art, or cleared (`None`). The preset code calls
    /// this only when "Change the right screen with the preset" is on (#273).
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

static SERVICE: OnceLock<Handle> = OnceLock::new();
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
        let worker = h.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("foxglove-art".into())
            .spawn(move || {
                while !worker.stop.load(Ordering::Relaxed) {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run(&worker, &receiver)
                    }))
                    .is_ok()
                    {
                        break;
                    }
                    worker.update(|s| {
                        s.busy = false;
                        s.connected = false;
                        s.error = Some("Art worker panicked; restarting".into());
                    });
                }
            })
        {
            h.update(|s| s.error = Some(e.to_string()));
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

fn restore(pinned: &Option<Choice>, status: ArtStatus) -> Option<Choice> {
    // Only link=2 has a ready relay; 1 is connected but still discovering it.
    pinned
        .clone()
        .filter(|_| status.link == 2 && status.right_state == 0)
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
            let mut blob = Vec::new();
            std::fs::File::open(path)
                .and_then(|f| f.take(65536).read_to_end(&mut blob))
                .map_err(|e| format!("reading {}: {e}", path.display()))?;
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
    let hash = std::fs::read(file)
        .map(|b| super::art_library::blob_hash(&b))
        .map_err(|e| format!("reading {}: {e}", file.display()))?;
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

fn run(h: &Handle, receiver: &mpsc::Receiver<Command>) {
    let (pinned, pinned_name) = load_pinned();
    h.update(|s| {
        s.pinned = pinned;
        s.pinned_name = pinned_name;
        s.cycle = Cycle::load();
    });
    let profile_tx = h.commands.clone();
    let profile = super::devices::bus().facts().subscribe("profile.active");
    super::services::spawn_named("art-profile", move || {
        for _ in profile {
            if profile_tx.send(Command::ProfileChanged).is_err() {
                break;
            }
        }
    });
    // Layer changes for layer art (#60), straight from the device events.
    let layers_tx = h.commands.clone();
    let events = super::devices::bus().subscribe(false);
    super::services::spawn_named("art-layers", move || {
        for event in events {
            if let foxglove_devices::DeviceEvent::LayerChanged { active } = event.event {
                if layers_tx.send(Command::Layers(active)).is_err() {
                    break;
                }
            }
        }
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

fn run_events(h: &Handle, receiver: &mpsc::Receiver<Command>) {
    let mut links: Vec<(Arc<Link>, EventSubscription)> = Vec::new();
    let mut light_taps: Vec<EventSubscription> = Vec::new();
    // The left's lights as last reported on the current connection; off then on = a wake.
    let mut lights_on: Option<bool> = None;
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
            Command::Choose(_) | Command::Keep { .. } | Command::Erase(_) | Command::Status
        );
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
            h.update(|s| s.busy = true);
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
            } => {
                let is_current = current.is_some_and(|(id, g)| {
                    g == generation && Weak::ptr_eq(&Arc::downgrade(&links[id].0), &source)
                });
                if !is_current {
                    continue;
                }
                let was = lights_on.replace(on);
                // Switched on the keyboard too: the tray panel and Lights page read them again.
                if was != Some(on) {
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
                    Some(command) => command,
                    None => continue,
                }
            }
            Command::LayerDue => {
                let change = triggers.due(&h.snapshot().cycle, Instant::now());
                if held(&change, super::pause::paused(), &mut missed) {
                    continue;
                }
                match shown(change, &triggers) {
                    Some(command) => command,
                    None => continue,
                }
            }
            Command::Preset(art) => {
                let change = triggers.preset(art);
                if held(&change, super::pause::paused(), &mut missed) {
                    continue;
                }
                match shown(change, &triggers) {
                    Some(command) => command,
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
                        h.update(|s| s.busy = true);
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
                        h.update(|s| s.busy = true);
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
                    Some(command) => command,
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
                h.update(|s| {
                    s.busy = true;
                    s.error = None;
                    s.outcome = None;
                });
                h.cancel.store(false, Ordering::Relaxed);
                Command::Choose(choice)
            }
            other => other,
        };
        let mut reported = None;
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
                        let on = protocol::RgbState::parse(r).left.on;
                        let _ = tx.send(Command::Lights {
                            source: source.clone(),
                            generation,
                            on,
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
                reported = Some(ArtStatus::parse(report));
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
        if automatic {
            h.update(|s| {
                queued_command = s.busy;
                if queued_command {
                    return;
                }
                h.cancel.store(false, Ordering::Relaxed);
                s.busy = true;
                s.cancelled = false;
                s.error = None;
                s.push_progress = None;
            });
        }
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
                            if let Some(choice) = restore(&wanted, st) {
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
                        keep(link, h, &file, slot, &name).and_then(|st| {
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
                    Command::Show(choice) => choose(link, h, &choice),
                    Command::Choose(choice) => choose(link, h, &choice).and_then(|st| {
                        h.update(|s| s.status = Some(st));
                        save_pin(h, &choice)?;
                        Ok(st)
                    }),
                    _ => unreachable!(),
                }
            }
        } else {
            Err("Waiting for a keyboard with the host link".into())
        };
        if queued_command {
            continue;
        }
        if let Err(e) = &result {
            // The UI shows words for it (art_words); the raw error is for the log.
            eprintln!("Foxglove: art: {e}");
        }
        h.update(|s| {
            s.busy = false;
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
    use digitalis::protocol::{art as cmd, left_state, right_state, REPORT};
    use std::cell::{Cell, RefCell};

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

    impl digitalis::Transport for FakeKeyboard {
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
