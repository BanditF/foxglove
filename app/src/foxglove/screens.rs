//! The left screen's host panel, drawn by Foxglove: the clock, volume, media and per-game hints
//! from `foxglove_panel`, sent over the shared link. It runs on its own thread from start-up,
//! whether or not the overlay is connected, and only draws while it holds the panel lock that
//! the standalone hostpanel also takes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use digitalis::link::Link;
use digitalis::panel;
use digitalis::protocol::Hello;
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
}

static STOP: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);

/// What the panel service is doing, for the UI.
#[allow(dead_code)] // shown in the Screens settings (#13)
pub fn status() -> String {
    STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    hello_at: Instant,
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
            hello_at: Instant::now(),
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

fn run() {
    let mut sources = Sources::new();
    let mut lock = PanelLock::new();
    let mut settings = ScreensSettings::load();
    let mut settings_at = Instant::now();
    let mut view = View::default();
    let mut drawing: Option<Drawing> = None;
    let mut was_driving = false;
    let mut last_battery = None;
    let mut tick: u32 = 0;

    while !STOP.load(Ordering::Relaxed) {
        let frame_start = Instant::now();
        if settings_at.elapsed() > Duration::from_secs(5) {
            settings = ScreensSettings::load();
            settings_at = Instant::now();
        }

        // Paused (tray) or off: the firmware gets its own panel back.
        if paused() || !settings.drive_left {
            stop_driving(&mut lock, &drawing, &mut was_driving);
            set_status(if paused() { "Left panel paused" } else { "Left panel off" });
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        // The keyboard: the one we're on while it's connected, else find one with the host link.
        match &drawing {
            None => {
                match super::link::host_link() {
                    Some((link, hello)) => drawing = Some(Drawing::new(link, hello)),
                    None => {
                        set_status("Waiting for a keyboard with the host link");
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
                continue;
            }
            Some(d) => {
                let state = d.link.state();
                if state.connected.is_none() {
                    // Reconnecting; the link does that by itself.
                    stop_driving(&mut lock, &drawing, &mut was_driving);
                    set_status("Waiting for the keyboard");
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                if state.generation != d.generation {
                    // Reconnected, perhaps to different firmware: ask again.
                    stop_driving(&mut lock, &drawing, &mut was_driving);
                    let link = d.link.clone();
                    drawing = super::link::ask_hello(&link).map(|h| Drawing::new(link, h));
                    continue;
                }
            }
        }
        let d = drawing.as_mut().unwrap();

        sources.update(&mut view, |exe| settings.games.get(exe).cloned());

        let driving = lock.acquire();
        let mut result = Ok(());
        if driving {
            if !was_driving {
                set_status("Drawing the left panel");
                // Logged only by the app holding the panel, so two apps don't both write it.
                battery_log(d.hello.right_battery, &mut last_battery);
            }
            render::draw(&mut d.panel, &view, tick);
            // Resend now and then even when nothing changed, in case the firmware lets the panel go.
            let stale = d.last_sent.elapsed() > Duration::from_secs(20);
            if d.last.as_deref() != Some(&d.panel.bits[..]) || stale {
                result = panel::show(
                    d.link.as_ref(),
                    &d.panel.bits,
                    d.last.as_deref(),
                    d.panel.stride,
                )
                .map(|_| ());
                d.last = Some(d.panel.bits.clone());
                d.last_sent = Instant::now();
            }
            // HELLO turns the panel back on in older firmware, so only the app drawing it asks.
            if result.is_ok() && d.hello_at.elapsed() > Duration::from_secs(60) {
                d.hello_at = Instant::now();
                result = panel::hello(d.link.as_ref())
                    .map(|h| battery_log(h.right_battery, &mut last_battery));
            }
        } else {
            set_status("Panel driven by another app");
        }
        was_driving = driving;

        if let Err(e) = result {
            set_status(format!("Lost keyboard: {e}"));
            lock.release();
            was_driving = false;
            d.last = None;
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }

        tick = tick.wrapping_add(1);
        let period = Duration::from_millis(1000 / settings.fps.clamp(1, 10) as u64);
        std::thread::sleep(period.saturating_sub(frame_start.elapsed()));
    }

    stop_driving(&mut lock, &drawing, &mut was_driving);
    // Let the link thread write the RELEASE before the process goes.
    std::thread::sleep(Duration::from_millis(50));
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
