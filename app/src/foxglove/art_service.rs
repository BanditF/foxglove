//! Right art on the shared host link. File I/O and all keyboard requests stay on the worker.
#![allow(dead_code)] // Handle consumed by the art settings and tray in #15/#16.

use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::art_cycle::{self, Cycle};

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
                let label = names.map_or_else(|| format!("Set {n}"), |s| s[usize::from(n)].label.clone());
                (n as i8, label)
            })
            .collect()
    }

    /// The built-in set the right half shows now, if it's playing one.
    pub fn showing_built_in(&self) -> Option<i8> {
        self.status.filter(|s| s.right_state == 0).map(|s| s.right_set)
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
    Save { slot: u8, name: String },
    Erase(u8),
    /// Show the hotlist's next entry (timer, lights wake, or the UI's "Next").
    Next,
    /// The hotlist or its mode changed: reschedule.
    CycleChanged,
    /// The left's underglow is on or off (an RGB_STATE report, firmware #130).
    Lights { source: Weak<Link>, generation: u64, on: bool },
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
    pub fn wallpaper(&self, w: super::wallpaper::Wallpaper) {
        let _ = self.commands.send(Command::Wallpaper(w));
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
    let notifications = h.commands.clone();
    let _subscription = super::link::subscribe_links(move |link| {
        let _ = notifications.send(Command::Available(link));
    });
    // Discovery runs once. New shared links and each link's reconnects arrive as events.
    let _ = super::link::discover_links();
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
    while !h.stop.load(Ordering::Relaxed) {
        let received = match due {
            Some(at) => receiver.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => receiver.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        let command = match received {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => Command::Next,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let reschedule = |connected: bool| {
            connected
                .then(|| h.snapshot().cycle.period())
                .flatten()
                .map(|p| Instant::now() + p)
        };
        let command = match command {
            Command::Lights { source, generation, on } => {
                let is_current = current.is_some_and(|(id, g)| {
                    g == generation && Weak::ptr_eq(&Arc::downgrade(&links[id].0), &source)
                });
                if !is_current {
                    continue;
                }
                let woke = lights_on.replace(on) == Some(false) && on;
                if !(woke && h.snapshot().cycle.mode == art_cycle::Mode::LightsWake) {
                    continue;
                }
                Command::Next
            }
            Command::Wallpaper(w) => {
                let snap = h.snapshot();
                let (Some(st), false, true) = (snap.status, snap.busy, current.is_some()) else {
                    continue;
                };
                if snap.cycle.mode != art_cycle::Mode::Wallpaper {
                    continue;
                }
                match super::wallpaper::choice(&w, st.right_set_count) {
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
                    Some(choice) => {
                        h.update(|s| s.busy = true);
                        Command::Choose(choice)
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
                if h.snapshot().cycle.mode == art_cycle::Mode::Wallpaper {
                    if let Some(w) = super::wallpaper::current(h.snapshot().cycle.wallpaper_monitor.as_deref()) {
                        let _ = h.commands.send(Command::Wallpaper(w));
                    }
                }
                continue;
            }
            Command::Next => {
                due = reschedule(current.is_some());
                let snap = h.snapshot();
                let (Some(st), false, true) = (snap.status, snap.busy, current.is_some()) else {
                    continue;
                };
                let mut cycle = snap.cycle;
                let Some(choice) = cycle.advance(|item| {
                    art_cycle::resolve(item, st.right_set_count, st.flash_used)
                }) else {
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
                        let _ = tx.send(Command::Lights { source: source.clone(), generation, on });
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
                let wake_events = super::link::ask_hello(link)
                    .is_some_and(|hello| hello.capabilities & protocol::capabilities::RGB_STATE_EVENT != 0);
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
                        if !queued_command {
                            if let Some(choice) = restore(&h.snapshot().pinned, st) {
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
