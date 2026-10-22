//! The left screen's host panel, drawn by Foxglove: the clock, volume, media and per-game hints
//! from `foxglove_panel`, sent over the shared link. It runs on its own thread from start-up,
//! whether or not the overlay is connected, and only draws while it holds the panel lock that
//! the standalone hostpanel also takes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use digitalis::link::Link;
use digitalis::panel;
use foxglove_panel::panel_lock::PanelLock;
use foxglove_panel::render::{self, Panel, View};
use foxglove_panel::Sources;

use super::screens_settings::ScreensSettings;

static STATUS: Mutex<String> = Mutex::new(String::new());

/// What the panel service is doing, for the UI.
#[allow(dead_code)] // shown in the Screens settings (#13)
pub fn status() -> String {
    STATUS.lock().unwrap().clone()
}

fn set_status(s: impl Into<String>) {
    let s = s.into();
    let mut cur = STATUS.lock().unwrap();
    if *cur != s {
        eprintln!("Foxglove screens: {s}");
        *cur = s;
    }
}

pub fn start() {
    let _ = std::thread::Builder::new()
        .name("foxglove-screens".into())
        .spawn(run);
}

/// The panel's state while one keyboard is being drawn.
struct Drawing {
    link: Arc<Link>,
    generation: u64,
    panel: Panel,
    last: Option<Vec<u8>>,
    last_sent: Instant,
    hello_at: Instant,
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

    loop {
        let frame_start = Instant::now();
        if settings_at.elapsed() > Duration::from_secs(5) {
            settings = ScreensSettings::load();
            settings_at = Instant::now();
        }

        // Hands the panel back to the firmware and lets another app have it.
        let mut stop_driving = |drawing: &Option<Drawing>, was_driving: &mut bool| {
            if *was_driving {
                if let Some(d) = drawing {
                    let _ = panel::release(d.link.as_ref());
                }
            }
            lock.release();
            *was_driving = false;
        };

        if !settings.drive_left {
            stop_driving(&drawing, &mut was_driving);
            set_status("Left panel off");
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        // Find a keyboard with the host link, reusing the overlay's link when it has one.
        let state = drawing.as_ref().map(|d| d.link.state());
        match state.as_ref().map(|s| {
            (
                s.connected.as_ref().map(|c| c.hello.is_some()),
                s.generation,
            )
        }) {
            None | Some((Some(false), _)) => {
                stop_driving(&drawing, &mut was_driving);
                drawing = None;
                match super::link::host_link() {
                    Some(link) => {
                        drawing = Some(Drawing {
                            link,
                            generation: 0,
                            panel: Panel::new(64, 52),
                            last: None,
                            last_sent: Instant::now(),
                            hello_at: Instant::now(),
                        });
                    }
                    None => {
                        set_status("Waiting for a keyboard with the host link");
                        std::thread::sleep(Duration::from_secs(3));
                    }
                }
                continue;
            }
            Some((None, _)) => {
                // Reconnecting; the link does that by itself.
                stop_driving(&drawing, &mut was_driving);
                set_status("Waiting for the keyboard");
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            Some((Some(true), _)) => {}
        }
        let d = drawing.as_mut().unwrap();
        let state = state.unwrap();
        let conn = state.connected.unwrap();
        let hello = conn.hello.unwrap();
        if state.generation != d.generation {
            d.generation = state.generation;
            d.panel = Panel::new(hello.width, hello.height);
            d.last = None;
            d.hello_at = Instant::now();
        }

        sources.update(&mut view, |exe| settings.games.get(exe).cloned());

        let driving = lock.acquire();
        let mut result = Ok(());
        if driving {
            if !was_driving {
                set_status(format!("Drawing on {}", conn.product));
                // Logged only by the app holding the panel, so two apps don't both write it.
                battery_log(hello.right_battery, &mut last_battery);
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
}

/// Appends the right half's battery to `right-battery.csv` (next to the settings) when it changes.
fn battery_log(battery: Option<u8>, last: &mut Option<u8>) {
    use std::io::Write;

    let Some(percent) = battery.filter(|_| battery != *last) else {
        return;
    };
    *last = battery;
    let Some(path) = ScreensSettings::path().map(|p| p.with_file_name("right-battery.csv")) else {
        return;
    };
    let new = !path.exists();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        if new {
            let _ = writeln!(f, "time,right_battery_percent");
        }
        let _ = writeln!(f, "{},{percent}", foxglove_panel::local_timestamp());
    }
}
