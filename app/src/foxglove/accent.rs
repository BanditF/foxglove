//! The underglow follows a colour, as options on the Lights page, saved in the store's
//! `[lighting]` (a profile may change them):
//! - **Follow the Windows accent colour** (#93);
//! - **Lights follow the wallpaper** (#70): the accent of the wallpaper on screen, from its
//!   `foxglove.json` palette (`wallpaper_manifest`, docs/wallpaper-manifest.md). A wallpaper
//!   without a palette gives the Windows accent instead. (Screen-edge sampling, decided opt-in
//!   on #39, isn't built yet; once it is, it comes before the Windows accent here, when on.)
//!
//! Event-driven: Windows' UISettings.ColorValuesChanged (accent or theme changed), the
//! wallpaper service's pass over a change (Wallpaper Engine's config or the desktop wallpaper
//! rewritten, a manifest found; `wallpaper.rs`), a profile switch, resuming (#404), and the
//! device layer's Connected event (a keyboard came, so it gets the current colour). Nothing
//! polls.
//!
//! Only the hue and saturation follow; the board keeps its own brightness. The case's "Match
//! keyboard" follows the same colour (#423). A lighting lease wins (a running scene, a
//! notification): the colour then waits and comes back when it ends (`Arbiter::follow`).
//! Calibration applies in the drivers, as for every colour Foxglove sends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use foxglove_devices::{Device as _, DeviceEvent};

static ACCENT: AtomicBool = AtomicBool::new(false);
static WALLPAPER: AtomicBool = AtomicBool::new(false);
/// The accent of the wallpaper on screen, as the wallpaper service last saw it.
static ON_SCREEN: Mutex<Option<[u8; 3]>> = Mutex::new(None);
static WAKE: OnceLock<Mutex<mpsc::Sender<Wake>>> = OnceLock::new();

/// Why the follower runs again.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wake {
    /// The colour may have changed (or the settings): send it if it did.
    Changed,
    /// A keyboard connected: send it the colour, changed or not.
    Arrived,
}

/// The `[lighting]` section of the settings store.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct LightingSettings {
    follow_accent: bool,
    /// Lights follow the wallpaper (#70).
    follow_wallpaper: bool,
}

/// "Follow the Windows accent colour".
pub fn enabled() -> bool {
    ACCENT.load(Ordering::Relaxed)
}

/// "Lights follow the wallpaper".
pub fn wallpaper_enabled() -> bool {
    WALLPAPER.load(Ordering::Relaxed)
}

fn settings() -> LightingSettings {
    LightingSettings {
        follow_accent: enabled(),
        follow_wallpaper: wallpaper_enabled(),
    }
}

fn store(s: LightingSettings) {
    ACCENT.store(s.follow_accent, Ordering::Relaxed);
    WALLPAPER.store(s.follow_wallpaper, Ordering::Relaxed);
}

/// Saves the settings and applies what the lights follow now (turning the wallpaper off can
/// leave the Windows accent to follow).
fn change(s: LightingSettings) {
    store(s);
    if let Err(e) = super::store::save("lighting", &s) {
        eprintln!("Foxglove: can't save the lighting settings: {e}");
    }
    wake();
}

/// Turns following the Windows accent on or off.
pub fn set_enabled(on: bool) {
    change(LightingSettings {
        follow_accent: on,
        ..settings()
    });
}

/// Turns "Lights follow the wallpaper" on or off.
pub fn set_wallpaper(on: bool) {
    change(LightingSettings {
        follow_wallpaper: on,
        ..settings()
    });
}

/// The wallpaper service saw the wallpapers on screen: `accent` is the one to follow (none
/// without a palette). Wakes the follower when it changed and the lights follow it.
pub fn wallpaper_changed(accent: Option<[u8; 3]>) {
    let before = std::mem::replace(
        &mut *ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner()),
        accent,
    );
    if wakes(before, accent, wallpaper_enabled()) {
        wake();
    }
}

/// Whether a new accent on screen needs the follower: it changed, and the lights follow it.
fn wakes(before: Option<[u8; 3]>, now: Option<[u8; 3]>, following: bool) -> bool {
    following && before != now
}

/// Applies the colour again (turned on, the wallpaper changed, or Foxglove resumed, #404).
pub(crate) fn wake() {
    send(Wake::Changed);
}

fn send(why: Wake) {
    if let Some(tx) = WAKE.get() {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(why);
    }
}

/// The colour the lights follow: the wallpaper's accent when they follow it and it has one,
/// else the Windows accent (`windows`, asked only then) when they follow either. None when
/// they follow nothing.
fn pick(
    s: LightingSettings,
    wallpaper: Option<[u8; 3]>,
    windows: impl FnOnce() -> Option<[u8; 3]>,
) -> Option<[u8; 3]> {
    match wallpaper {
        Some(c) if s.follow_wallpaper => Some(c),
        _ if s.follow_wallpaper || s.follow_accent => windows(),
        _ => None,
    }
}

/// The colour the lights follow now, if they follow one ("Match keyboard" starts from it).
pub fn followed() -> Option<[u8; 3]> {
    let wallpaper = *ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
    pick(settings(), wallpaper, current)
}

/// An sRGB colour as the underglow's hue (0-359) and saturation (0-100).
pub fn hue_sat(r: u8, g: u8, b: u8) -> (u16, u8) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
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
    (
        (hue.round() as u16) % 360,
        (delta / max * 100.0).round() as u8,
    )
}

/// The effective settings (the active profile's, else the base).
fn load() -> LightingSettings {
    super::store::load_or_migrate::<LightingSettings>("lighting", || {
        // The old lighting.ini, from before the store.
        let ini = crate::settings::Settings::config_file_path()?.with_file_name("lighting.ini");
        let on = ini::Ini::load_from_file(&ini)
            .ok()?
            .section(Some("lighting"))?
            .get("follow_accent")?
            == "true";
        Some((
            LightingSettings {
                follow_accent: on,
                ..LightingSettings::default()
            },
            vec![ini],
        ))
    })
}

/// Starts following (Windows): loads the settings, watches the accent and keyboard arrivals.
pub fn start() {
    if super::repaint_trace::disabled("accent") {
        return;
    }
    store(load());
    #[cfg(windows)]
    windows_impl::start();
}

/// The Windows accent colour now, as sRGB.
#[cfg(windows)]
pub fn current() -> Option<[u8; 3]> {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let c = UISettings::new()
        .ok()?
        .GetColorValue(UIColorType::Accent)
        .ok()?;
    Some([c.R, c.G, c.B])
}

#[cfg(not(windows))]
pub fn current() -> Option<[u8; 3]> {
    None
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::ViewManagement::{UIColorType, UISettings};

    pub fn start() {
        let (tx, rx) = mpsc::channel::<Wake>();
        let _ = WAKE.set(Mutex::new(tx.clone()));
        // A profile switch can change the settings: re-read them, then apply.
        let switched = tx.clone();
        let profile = crate::foxglove::devices::bus()
            .facts()
            .subscribe("profile.active");
        crate::foxglove::services::spawn_named("accent-profile", move || {
            for _ in profile {
                store(load());
                if switched.send(Wake::Changed).is_err() {
                    break;
                }
            }
        });
        // Keyboard arrivals: apply the current colour to a newly connected board.
        let arrivals = tx.clone();
        let events = crate::foxglove::devices::bus().subscribe(false);
        crate::foxglove::services::spawn_named("accent-arrivals", move || {
            for e in events {
                if e.event == DeviceEvent::Connected && arrivals.send(Wake::Arrived).is_err() {
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
                let _ = changed.send(Wake::Changed);
                Ok(())
            }));
            let mut applied: Option<(u16, u8)> = None;
            for why in rx {
                if why == Wake::Arrived {
                    applied = None;
                }
                // Paused (#404): nothing goes out; resuming wakes this again.
                if crate::foxglove::pause::paused() {
                    applied = None;
                    continue;
                }
                let windows = || {
                    let c = ui.GetColorValue(UIColorType::Accent).ok()?;
                    Some([c.R, c.G, c.B])
                };
                let wallpaper = *ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
                let Some(c) = pick(settings(), wallpaper, windows) else {
                    applied = None;
                    continue;
                };
                // The case's "Match keyboard" follows it too (#423).
                crate::foxglove::pc_lights::send(crate::foxglove::pc_lights::Msg::Follow(c));
                let (hue, sat) = hue_sat(c[0], c[1], c[2]);
                if applied == Some((hue, sat)) {
                    continue;
                }
                // device() may discover a keyboard; this thread can wait for it.
                let Some(d) = crate::foxglove::devices::device() else {
                    continue;
                };
                let Some(lighting) = d.lighting() else {
                    continue;
                };
                // An effect a quit left running (#542) would hide the colour.
                crate::foxglove::scene_run::release_left_effect(&d);
                let device = d.info().id.to_string();
                // Read outside the arbiter's lock: it is a keyboard round trip.
                let brightness = lighting.state().map_or(100, |s| s.brightness);
                match crate::foxglove::lighting_leases::with(|a| {
                    a.follow(&device, (hue, sat, brightness), lighting)
                }) {
                    Ok(_) => applied = Some((hue, sat)),
                    Err(e) => eprintln!(
                        "Foxglove accent: {}",
                        crate::foxglove::devices::describe(&e)
                    ),
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

    /// #70: the wallpaper's accent when the lights follow it and it has one, else the Windows
    /// accent (a wallpaper without a palette); nothing when they follow nothing.
    #[test]
    fn the_wallpaper_accent_comes_first_then_windows() {
        let (teal, blue) = ([0x0f, 0xb5, 0xae], [0, 120, 215]);
        let s = |follow_accent, follow_wallpaper| LightingSettings {
            follow_accent,
            follow_wallpaper,
        };
        let windows = || Some(blue);
        assert_eq!(pick(s(false, true), Some(teal), windows), Some(teal));
        assert_eq!(pick(s(true, true), Some(teal), windows), Some(teal));
        assert_eq!(pick(s(false, true), None, windows), Some(blue));
        assert_eq!(pick(s(true, false), Some(teal), windows), Some(blue));
        assert_eq!(pick(s(false, false), Some(teal), windows), None);
        assert_eq!(
            pick(s(false, true), Some(teal), || panic!("not asked")),
            Some(teal)
        );
    }

    #[test]
    fn only_a_new_accent_wakes_the_follower() {
        let teal = Some([0x0f, 0xb5, 0xae]);
        assert!(wakes(None, teal, true));
        assert!(wakes(teal, None, true), "back to the Windows accent");
        assert!(!wakes(teal, teal, true), "unchanged");
        assert!(!wakes(None, teal, false), "not following the wallpaper");
    }

    /// The old store keeps working: a section without the new key follows no wallpaper.
    #[test]
    fn an_old_lighting_section_reads() {
        let old: LightingSettings = toml::from_str("follow_accent = true").unwrap();
        assert_eq!(
            old,
            LightingSettings {
                follow_accent: true,
                follow_wallpaper: false,
            }
        );
    }
}
