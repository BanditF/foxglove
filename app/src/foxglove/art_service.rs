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
        match (&self.pinned, self.status) {
            (Some(Choice::File(p)), Some(s)) if s.right_state == 2 => Some(p),
            _ => None,
        }
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
    Save {
        slot: u8,
        name: String,
    },
    Erase(u8),
    /// Show the hotlist's next entry (timer, lights wake, or the UI's "Next").
    Next,
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

    /// Saves the pushed set now playing on the right into flash slot `slot`.
    pub fn save_to_slot(&self, slot: u8, name: String) -> bool {
        self.submit(Command::Save { slot, name })
    }

    pub fn erase_slot(&self, slot: u8) -> bool {
        self.submit(Command::Erase(slot))
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

    /// Shows the hotlist's next entry now.
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
}

fn load_pinned() -> Option<Choice> {
    super::store::load_or_migrate::<ArtSettings>("art", || {
        let json = crate::settings::Settings::config_file_path()?.with_file_name("art.json");
        let pinned = serde_json::from_slice::<Choice>(&std::fs::read(&json).ok()?).ok()?;
        Some((
            ArtSettings {
                pinned: Some(pinned),
            },
            vec![json],
        ))
    })
    .pinned
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

fn choose(link: &Link, h: &Handle, choice: &Choice) -> Result<ArtStatus, String> {
    if h.cancelled() {
        return Err("cancelled".into());
    }
    match choice {
        Choice::BuiltIn(set) => art::select(link, *set).map_err(|e| e.to_string()),
        Choice::Slot(slot) => {
            let count = h
                .snapshot()
                .status
                .map(|s| s.right_set_count)
                .ok_or("No art status yet")?;
            art::select_slot(link, count, *slot).map_err(|e| e.to_string())
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
        }
    }
}

/// Reads every flash slot the right half has; none on firmware without them.
fn read_slots(link: &Link, h: &Handle, st: &ArtStatus) {
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
    let pinned = load_pinned();
    h.update(|s| {
        s.pinned = pinned;
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
                let woke = lights_on.replace(on) == Some(false) && on;
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
                let pinned = load_pinned();
                let changed = h.snapshot().pinned != pinned;
                h.update(|s| {
                    s.pinned = pinned.clone();
                    s.cycle = Cycle::load();
                });
                due = reschedule(current.is_some());
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
            Command::Next => {
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
                // The hotlist waits while layer or preset art is on top.
                if triggers.top(&snap.cycle).is_some() {
                    continue;
                }
                let mut cycle = snap.cycle;
                let Some(choice) = cycle
                    .advance(|item| art_cycle::resolve(item, st.right_set_count, st.flash_used))
                else {
                    continue;
                };
                let _ = cycle.save();
                h.update(|s| {
                    s.cycle = cycle;
                    s.busy = true;
                    s.error = None;
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
        let result = if let Some((id, _)) = current {
            let link = &links[id].0;
            if automatic {
                reported
                    .map(Ok)
                    .unwrap_or_else(|| fresh_status(link, h))
                    .and_then(|st| {
                        h.update(|s| s.status = Some(st));
                        if connected_now && !queued_command {
                            read_slots(link, h, &st);
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
                                .or(snap.pinned);
                            if let Some(choice) = restore(&wanted, st) {
                                return choose(link, h, &choice);
                            }
                        }
                        Ok(st)
                    })
            } else {
                match command {
                    Command::Status => fresh_status(link, h).inspect(|st| read_slots(link, h, st)),
                    Command::Save { slot, name } => art::save(link.as_ref(), slot, &name)
                        .map_err(|e| e.to_string())
                        .inspect(|st| read_slots(link, h, st)),
                    Command::Erase(slot) => art::erase(link.as_ref(), slot)
                        .map_err(|e| e.to_string())
                        .inspect(|st| read_slots(link, h, st)),
                    Command::Show(choice) => choose(link, h, &choice),
                    Command::Choose(choice) => choose(link, h, &choice).and_then(|st| {
                        h.update(|s| {
                            s.status = Some(st);
                            s.pinned = Some(choice.clone());
                        });
                        let art = ArtSettings {
                            pinned: Some(choice.clone()),
                        };
                        super::store::save("art", &art)
                            .map_err(|e| format!("saving pinned art: {e}"))?;
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
        h.update(|s| {
            s.busy = false;
            s.cancelled = h.cancelled();
            match result {
                Ok(st) => {
                    if s.push_progress.is_some() {
                        s.push_progress = Some((st.pushed, st.length));
                    }
                    s.status = Some(st);
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_the_ui_may_only_read() {
        assert!(allowed(&Command::Status, true));
        for c in [
            Command::Choose(Choice::BuiltIn(1)),
            Command::Save {
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
