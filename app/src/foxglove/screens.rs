//! The left screen's host panel, drawn by Foxglove: the clock, volume, media and per-game hints
//! from `foxglove_panel`, sent over the shared link. It runs on its own thread from start-up,
//! whether or not the overlay is connected, and only draws while it holds the panel lock that
//! the standalone hostpanel also takes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use digitalis::link::{EventSubscription, Link};
use digitalis::panel;
use digitalis::protocol::Hello;
use digitalis::{protocol, Transport};
use foxglove_panel::panel_lock::PanelLock;
use foxglove_panel::render::{self, Panel, View};
use foxglove_panel::Sources;

use super::screens_settings::ScreensSettings;

static STATUS: Mutex<String> = Mutex::new(String::new());
static PAUSED: AtomicBool = AtomicBool::new(false);

pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}
pub fn set_paused(value: bool) {
    PAUSED.store(value, Ordering::Relaxed);
    wake();
}

static STOP: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);

/// What the panel service is doing, for the UI.
pub fn status() -> String {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The last frame sent to the left panel, for the Screens settings' preview.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bits: Vec<u8>,
}

/// What the Screens settings show: the keyboard with the host link and what it said to HELLO,
/// the frame last sent, and the last program in front other than Foxglove.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub product: String,
    pub hello: Option<Hello>,
    pub frame: Option<Frame>,
    pub foreground: Option<String>,
}

static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot {
    product: String::new(),
    hello: None,
    frame: None,
    foreground: None,
});

pub fn snapshot() -> Snapshot {
    SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn update_snapshot(f: impl FnOnce(&mut Snapshot)) {
    f(&mut SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()));
}

/// Remembers the program in front unless it's Foxglove itself, so "Add foreground app" can
/// offer the game the user just left for the settings window.
fn note_foreground() {
    let Some(exe) = foxglove_panel::providers::foreground_exe().map(|e| e.to_lowercase()) else {
        return;
    };
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    if own.as_deref() != Some(exe.as_str()) {
        update_snapshot(|s| s.foreground = Some(exe));
    }
}

fn set_status(s: impl Into<String>) {
    let s = s.into();
    let mut cur = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if *cur != s {
        eprintln!("Foxglove screens: {s}");
        *cur = s;
    }
}

pub fn start() {
    let _ = super::services::spawn_supervised("screens", run);
}

/// Hands the panel back to the firmware before the app exits (waits up to half a second), so
/// the left screen doesn't keep Foxglove's last frame.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    wake();
    let end = Instant::now() + Duration::from_millis(500);
    while !STOPPED.load(Ordering::Relaxed) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The keyboard being drawn on and the panel's state there.
struct Drawing {
    link: Arc<Link>,
    hello: Hello,
    generation: u64,
    panel: Panel,
    last: Option<Vec<u8>>,
    last_sent: Instant,
}

impl Drawing {
    fn new(link: Arc<Link>, hello: Hello) -> Self {
        let generation = link.state().generation;
        Drawing {
            link,
            hello,
            generation,
            panel: Panel::new(hello.width, hello.height),
            last: None,
            last_sent: Instant::now(),
        }
    }
}

/// Hands the panel back to the firmware and lets another app have it.
fn stop_driving(lock: &mut PanelLock, drawing: &Option<Drawing>, was_driving: &mut bool) {
    if *was_driving {
        if let Some(d) = drawing {
            let _ = panel::release(d.link.as_ref());
        }
    }
    lock.release();
    *was_driving = false;
}

struct WatchedLink {
    link: Arc<Link>,
    _state: EventSubscription,
    _reports: EventSubscription,
    hello: Option<(u64, Hello)>,
    checked_generation: Option<u64>,
}

enum Event {
    Wake,
    Providers,
    PanelAvailable,
    Settings,
    Available(Arc<Link>),
    Report(usize, u64, digitalis::link::Report),
}
static EVENTS: OnceLock<Mutex<Option<mpsc::Sender<Event>>>> = OnceLock::new();

fn wake() {
    if let Some(tx) = EVENTS.get().and_then(|s| s.lock().ok()) {
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(Event::Wake);
        }
    }
}

/// Zero means firmware keeps the frame forever. Leave queue/USB scheduling headroom.
fn keep_alive(timeout_s: u16) -> Option<Duration> {
    (timeout_s > 0).then(|| {
        let timeout = Duration::from_secs(u64::from(timeout_s));
        timeout - Duration::from_secs(1).min(timeout / 10)
    })
}

fn changed(last: Option<&[u8]>, frame: &[u8]) -> bool {
    last != Some(frame)
}

fn deadline(
    clock: Duration,
    animation: Option<Duration>,
    keep_alive: Option<Duration>,
) -> Duration {
    [Some(clock), animation, keep_alive]
        .into_iter()
        .flatten()
        .min()
        .unwrap()
}

fn run() {
    let (tx, rx) = mpsc::channel();
    *EVENTS.get_or_init(Default::default).lock().unwrap() = Some(tx.clone());
    STOPPED.store(false, Ordering::Relaxed);
    let mut sources = Sources::new();
    let providers_tx = tx.clone();
    let mut changes = foxglove_panel::changes::Changes::new(Arc::new(move || {
        let _ = providers_tx.send(Event::Providers);
    }));
    let mut providers_dirty = true;
    let settings_tx = tx.clone();
    let _settings_watch = ScreensSettings::path().and_then(|p| {
        super::screens_watch::settings(
            &p,
            Arc::new(move || {
                let _ = settings_tx.send(Event::Settings);
            }),
        )
    });
    let mut settings = ScreensSettings::load();
    let available = tx.clone();
    let _subscription = super::link::subscribe_links(move |link| {
        let _ = available.send(Event::Available(link));
    });
    let _ = super::link::discover_links();
    let mut links: Vec<WatchedLink> = Vec::new();
    let mut lock = PanelLock::new();
    let mut lock_watch = None;
    let mut view = View::default();
    let mut drawing: Option<Drawing> = None;
    let mut was_driving = false;
    let mut last_battery = None;
    let mut tick: u32 = 0;
    let mut next_frame = Instant::now();
    let mut wait = None;

    while !STOP.load(Ordering::Relaxed) {
        let event = match wait {
            Some(delay) => rx.recv_timeout(delay).ok(),
            None => rx.recv().ok(),
        };
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        match event {
            Some(Event::Providers) => providers_dirty = true,
            Some(Event::PanelAvailable) => lock_watch = None,
            Some(Event::Settings) => {
                // Keep current settings while an external writer has the file locked.
                if let Some(s) = ScreensSettings::path().and_then(ScreensSettings::load_from_file) {
                    providers_dirty |= s != settings;
                    settings = s;
                }
            }
            Some(Event::Available(link)) => {
                if links.iter().any(|l| Arc::ptr_eq(&l.link, &link)) {
                    continue;
                }
                let id = links.len();
                let events = tx.clone();
                let subscription = link.subscribe_events(move |_| {
                    let _ = events.send(Event::Wake);
                });
                let reports = tx.clone();
                let layer = std::cell::Cell::new(None);
                let tap = link.subscribe_reports(move |generation, r| {
                    if r[0] == 0xff {
                        if layer.get() == Some((generation, *r)) {
                            return;
                        }
                        layer.set(Some((generation, *r)));
                    }
                    if r[0] == 0xff
                        || protocol::is_reply(r, protocol::PANEL, protocol::panel::HELLO)
                        || protocol::is_reply(r, protocol::ART, protocol::art::STATUS)
                    {
                        let _ = reports.send(Event::Report(id, generation, *r));
                    }
                });
                links.push(WatchedLink {
                    link,
                    _state: subscription,
                    _reports: tap,
                    hello: None,
                    checked_generation: None,
                });
            }
            Some(Event::Report(id, generation, report))
                if protocol::is_reply(&report, protocol::PANEL, protocol::panel::HELLO) =>
            {
                let hello = Hello::parse(&report);
                if let Some(watched) = links.get_mut(id) {
                    let state = watched.link.state();
                    if state.generation == generation && state.connected.is_some() {
                        watched.hello = Some((generation, hello));
                        if let Some(d) = drawing.as_mut() {
                            if Arc::ptr_eq(&watched.link, &d.link) && generation == d.generation {
                                d.hello = hello;
                                if was_driving {
                                    battery_log(hello.right_battery, &mut last_battery);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        wait = None;
        let shown = drawing
            .as_ref()
            .map(|d| (d.link.clone(), d.hello))
            .or_else(|| {
                links.iter().find_map(|w| {
                    let state = w.link.state();
                    let (generation, hello) = w.hello?;
                    (state.connected.is_some() && generation == state.generation)
                        .then(|| (w.link.clone(), hello))
                })
            });
        update_snapshot(|s| {
            s.product = shown
                .as_ref()
                .and_then(|(l, _)| l.state().connected.map(|c| c.product))
                .unwrap_or_default();
            s.hello = shown.map(|(_, h)| h);
        });
        if paused() || !settings.drive_left {
            stop_driving(&mut lock, &drawing, &mut was_driving);
            if let Some(d) = drawing.as_mut() {
                d.last = None;
            }
            lock_watch = None;
            set_status(if paused() {
                "Left panel paused"
            } else {
                "Left panel off"
            });
            continue;
        }
        if drawing.as_ref().is_some_and(|d| {
            let state = d.link.state();
            state.connected.is_none() || state.generation != d.generation
        }) {
            stop_driving(&mut lock, &drawing, &mut was_driving);
            drawing = None;
        }
        if drawing.is_none() {
            drawing = links.iter_mut().find_map(|watched| {
                let state = watched.link.state();
                state.connected.as_ref()?;
                let usable = |(generation, h): &(u64, Hello)| {
                    *generation == state.generation
                        && (1..=256).contains(&h.width)
                        && (1..=256).contains(&h.height)
                };
                if !watched.hello.as_ref().is_some_and(usable)
                    && watched.checked_generation != Some(state.generation)
                {
                    // Probe a missing capability once per connection, never on a retry timer.
                    watched.checked_generation = Some(state.generation);
                    watched.hello =
                        super::link::ask_hello(&watched.link).map(|h| (state.generation, h));
                }
                let (_, hello) = watched.hello.filter(usable)?;
                Some(Drawing::new(watched.link.clone(), hello))
            });
        }
        let Some(d) = drawing.as_mut() else {
            set_status("Waiting for a keyboard with the host link");
            continue;
        };
        if !lock.acquire() {
            set_status("Panel driven by another app");
            if lock_watch.is_none() {
                let available = tx.clone();
                lock_watch = super::screens_watch::panel(Arc::new(move || {
                    let _ = available.send(Event::PanelAvailable);
                }));
            }
            continue;
        }
        lock_watch = None;
        if !was_driving {
            d.last = None;
            set_status("Drawing the left panel");
            battery_log(d.hello.right_battery, &mut last_battery);
        }
        was_driving = true;
        let clock_at = if providers_dirty {
            changes.refresh();
            providers_dirty = false;
            note_foreground();
            sources.update(&mut view, |exe| settings.games.get(exe).cloned())
        } else {
            Sources::update_clock(&mut view)
        };
        let now = Instant::now();
        let period = Duration::from_millis(1000 / settings.fps.clamp(1, 10) as u64);
        let animated = render::animated(&view, d.panel.width);
        if animated && now >= next_frame {
            tick = tick.wrapping_add(1);
            next_frame = now + period;
        }
        render::draw(&mut d.panel, &view, tick);
        let interval = keep_alive(d.hello.panel_timeout_s);
        let result = if changed(d.last.as_deref(), &d.panel.bits) {
            panel::show(
                d.link.as_ref(),
                &d.panel.bits,
                d.last.as_deref(),
                d.panel.stride,
            )
            .map(|_| ())
        } else if interval.is_some_and(|i| d.last_sent.elapsed() >= i) {
            // PRESENT alone refreshes timeout_work without transferring any rows.
            d.link.send(&protocol::present())
        } else {
            wait = Some(deadline(
                clock_at.saturating_duration_since(Instant::now()),
                animated.then(|| next_frame.saturating_duration_since(Instant::now())),
                interval.map(|i| i.saturating_sub(d.last_sent.elapsed())),
            ));
            continue;
        };
        match result {
            Ok(()) => {
                d.last = Some(d.panel.bits.clone());
                d.last_sent = Instant::now();
                let frame = Frame {
                    width: d.panel.width,
                    height: d.panel.height,
                    stride: d.panel.stride,
                    bits: d.panel.bits.clone(),
                };
                update_snapshot(|s| s.frame = Some(frame));
            }
            Err(e) => {
                set_status(format!("Lost keyboard: {e}"));
                lock.release();
                was_driving = false;
                d.last = None;
                continue;
            }
        }
        wait = Some(deadline(
            clock_at.saturating_duration_since(Instant::now()),
            animated.then(|| next_frame.saturating_duration_since(Instant::now())),
            interval,
        ));
    }
    stop_driving(&mut lock, &drawing, &mut was_driving);
    // RELEASE is queued to the shared writer; allow it to leave before app shutdown.
    std::thread::sleep(Duration::from_millis(50));
    *EVENTS.get().unwrap().lock().unwrap() = None;
    STOPPED.store(true, Ordering::Relaxed);
}

/// Appends the right half's battery to `right-battery.csv` (next to the settings) when it changes.
fn battery_log(battery: Option<u8>, last: &mut Option<u8>) {
    use std::io::Write;

    let Some(percent) = battery.filter(|_| battery != *last) else {
        return;
    };
    let Some(path) = ScreensSettings::path().map(|p| p.with_file_name("right-battery.csv")) else {
        return;
    };
    let new = !path.exists();
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let header_ok = !new || writeln!(f, "time,right_battery_percent").is_ok();
    if header_ok && writeln!(f, "{},{percent}", foxglove_panel::local_timestamp()).is_ok() {
        *last = battery;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_change_only_after_pixels_change_or_ownership_resets() {
        assert!(changed(None, &[1, 2]));
        assert!(!changed(Some(&[1, 2]), &[1, 2]));
        assert!(changed(Some(&[1, 2]), &[1, 3]));
    }
    #[test]
    fn sleeps_until_the_first_visible_change_or_firmware_deadline() {
        let minute = Duration::from_secs(47);
        assert_eq!(keep_alive(0), None);
        assert_eq!(deadline(minute, None, None), minute);
        assert_eq!(
            deadline(minute, Some(Duration::from_millis(250)), None),
            Duration::from_millis(250)
        );
        assert_eq!(
            deadline(minute, None, keep_alive(10)),
            Duration::from_secs(9)
        );
        assert_eq!(keep_alive(1), Some(Duration::from_millis(900)));
        assert_eq!(keep_alive(3600), Some(Duration::from_secs(3599)));
        assert_eq!(
            deadline(Duration::ZERO, Some(Duration::from_secs(1)), None),
            Duration::ZERO
        );
    }
}
