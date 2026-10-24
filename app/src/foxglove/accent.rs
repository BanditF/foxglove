//! Follow the Windows accent colour on the keyboard's underglow (#93), as an option in Settings.
//!
//! Event-driven: Windows' UISettings.ColorValuesChanged (accent or theme changed) and the device layer's
//! Connected event (a keyboard came, so it gets the current colour). Only the hue and
//! saturation follow the accent; the board keeps its own brightness. Saved in the store's `[lighting]`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use foxglove_devices::{DeviceEvent, Device as _};

static ENABLED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();

/// The `[lighting]` section of the settings store.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct LightingSettings {
    follow_accent: bool,
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turns following on or off, saves it, and applies the colour now when turned on.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if let Err(e) = super::store::save("lighting", &LightingSettings { follow_accent: on }) {
        eprintln!("Foxglove: can't save the lighting settings: {e}");
    }
    if on {
        wake();
    }
}

fn wake() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(());
    }
}

/// An sRGB colour as the underglow's hue (0-359) and saturation (0-100).
pub fn hue_sat(r: u8, g: u8, b: u8) -> (u16, u8) {
    let (r, g, b) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0, 0);
    }
    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    ((hue.round() as u16) % 360, (delta / max * 100.0).round() as u8)
}

/// The effective setting (the active profile's, else the base).
fn load_enabled() -> bool {
    super::store::load_or_migrate::<LightingSettings>("lighting", || {
        // The old lighting.ini, from before the store.
        let ini = crate::settings::Settings::config_file_path()?.with_file_name("lighting.ini");
        let on = ini::Ini::load_from_file(&ini)
            .ok()?
            .section(Some("lighting"))?
            .get("follow_accent")?
            == "true";
        Some((LightingSettings { follow_accent: on }, vec![ini]))
    })
    .follow_accent
}

/// Starts following (Windows): loads the setting, watches the accent and keyboard arrivals.
pub fn start() {
    if super::repaint_trace::disabled("accent") {
        return;
    }
    let on = load_enabled();
    ENABLED.store(on, Ordering::Relaxed);
    #[cfg(windows)]
    windows_impl::start();
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::ViewManagement::{UIColorType, UISettings};

    pub fn start() {
        let (tx, rx) = mpsc::channel::<()>();
        let _ = WAKE.set(Mutex::new(tx.clone()));
        // A profile switch can change the setting: re-read it, then apply.
        let switched = tx.clone();
        let profile = crate::foxglove::devices::bus().facts().subscribe("profile.active");
        crate::foxglove::services::spawn_named("accent-profile", move || {
            for _ in profile {
                ENABLED.store(load_enabled(), Ordering::Relaxed);
                if switched.send(()).is_err() {
                    break;
                }
            }
        });
        // Keyboard arrivals: apply the current colour to a newly connected board.
        let arrivals = tx.clone();
        let events = crate::foxglove::devices::bus().subscribe(false);
        crate::foxglove::services::spawn_named("accent-arrivals", move || {
            for e in events {
                if e.event == DeviceEvent::Connected && arrivals.send(()).is_err() {
                    break;
                }
            }
        });
        crate::foxglove::services::spawn_named("accent", move || {
            let Ok(ui) = UISettings::new() else { return };
            let changed = tx.clone();
            // The handler runs on a thread-pool thread; it only wakes this one. Keeping `ui`
            // alive here keeps the subscription.
            let _token = ui.ColorValuesChanged(&TypedEventHandler::new(move |_, _| {
                let _ = changed.send(());
                Ok(())
            }));
            let mut applied: Option<(u16, u8)> = None;
            for () in rx {
                if !enabled() {
                    applied = None;
                    continue;
                }
                let Ok(c) = ui.GetColorValue(UIColorType::Accent) else { continue };
                let (hue, sat) = hue_sat(c.R, c.G, c.B);
                if applied == Some((hue, sat)) {
                    continue;
                }
                // device() may discover a keyboard; this thread can wait for it.
                let Some(d) = crate::foxglove::devices::device() else { continue };
                let Some(lighting) = d.lighting() else { continue };
                let brightness = lighting.state().map_or(100, |s| s.brightness);
                match lighting.set_color(hue, sat, brightness.max(1)) {
                    Ok(()) => applied = Some((hue, sat)),
                    Err(e) => eprintln!("Foxglove accent: {}", crate::foxglove::devices::describe(&e)),
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_accent_colours() {
        assert_eq!(hue_sat(255, 0, 0), (0, 100));
        assert_eq!(hue_sat(0, 120, 215), (207, 100)); // Windows' default blue
        assert_eq!(hue_sat(0, 255, 0), (120, 100));
        assert_eq!(hue_sat(255, 0, 255), (300, 100));
        assert_eq!(hue_sat(128, 128, 128), (0, 0));
        assert_eq!(hue_sat(255, 128, 128).1, 50);
    }
}
