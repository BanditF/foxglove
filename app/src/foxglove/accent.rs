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
//! shared re-apply triggers (`reassert.rs`): a keyboard connecting, the right half's relay
//! coming up, the PC coming back, a display change and a taskbar restart. On those the colour
//! goes out again when the keyboard doesn't report it, and always after a push that failed
//! (#601: a push right after the left reboots can fail until `&rgb_ug` resolves, and used to
//! wait for the next colour change). Nothing polls.
//!
//! Only the hue and saturation follow; the board keeps its own brightness. The case's "Match
//! keyboard" follows the same colour (#423). A lighting lease wins (a running scene, a
//! notification): the colour then waits and comes back when it ends (`Arbiter::follow`).
//! Calibration applies in the drivers, as for every colour Foxglove sends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use foxglove_devices::Device as _;

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
    /// The keyboard may have lost it (`reassert.rs`): send it unless the keyboard reports it.
    Reassert(super::reassert::Trigger),
}

/// What the follower knows about the colour on the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Follower {
    /// The hue and saturation last sent (or found on the keyboard).
    applied: Option<(u16, u8)>,
    /// The last push failed: the next wake sends again, whatever it is (#601).
    failed: bool,
}

/// What the keyboard reports of its lights.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reported {
    hue: u16,
    sat: u8,
    brightness: u8,
    /// Both halves of a split keyboard agree (`LightingState::settled`).
    settled: bool,
}

/// How one wake ended.
#[derive(Debug, PartialEq)]
enum Step {
    /// Nothing to send: already applied, or the keyboard reports it.
    Skipped,
    /// Sent now (`true`), or kept for when a lighting lease ends (`false`).
    Sent(bool),
    Failed(String),
}

impl Follower {
    /// Whether a colour change can stop before any keyboard traffic: `want` went out already.
    fn applied_already(&self, why: Wake, want: (u16, u8)) -> bool {
        why == Wake::Changed && !self.failed && self.applied == Some(want)
    }

    /// Handles one wake for colour `want`. `read` reads the keyboard (asked at most once);
    /// `push` sends at a brightness and says whether it went out now (a lease may hold the
    /// lights).
    fn step(
        &mut self,
        why: Wake,
        want: (u16, u8),
        read: impl FnOnce() -> Option<Reported>,
        push: impl FnOnce(u8) -> Result<bool, String>,
    ) -> Step {
        if self.applied_already(why, want) {
            return Step::Skipped;
        }
        let reported = read();
        // Idempotent: a re-apply skips a colour the keyboard reports, unless the last push
        // failed (it may have reached one half only). When the right half just linked, its
        // half has to agree too (`settled`): it may have missed a push while it was away.
        let shows = reported.is_some_and(|r| {
            (r.hue, r.sat) == want
                && (r.settled || why != Wake::Reassert(super::reassert::Trigger::RightRelayReady))
        });
        if matches!(why, Wake::Reassert(_)) && !self.failed && shows {
            self.applied = Some(want);
            return Step::Skipped;
        }
        match push(reported.map_or(100, |r| r.brightness)) {
            Ok(now) => {
                *self = Follower {
                    applied: Some(want),
                    failed: false,
                };
                Step::Sent(now)
            }
            Err(e) => {
                self.failed = true;
                Step::Failed(e)
            }
        }
    }
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
        // Keyboard arrivals, the right half's relay coming up, coming back and the rest
        // (`reassert.rs`): the colour goes out again where the keyboard lost it.
        let reassert = Mutex::new(tx.clone());
        crate::foxglove::reassert::register("accent", move |t| {
            let _ = reassert
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .send(Wake::Reassert(t));
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
            let mut follower = Follower::default();
            for why in rx {
                // Paused (#404): nothing goes out; resuming wakes this again.
                if crate::foxglove::pause::paused() {
                    follower.applied = None;
                    continue;
                }
                let windows = || {
                    let c = ui.GetColorValue(UIColorType::Accent).ok()?;
                    Some([c.R, c.G, c.B])
                };
                let wallpaper = *ON_SCREEN.lock().unwrap_or_else(|e| e.into_inner());
                let Some(c) = pick(settings(), wallpaper, windows) else {
                    follower.applied = None;
                    continue;
                };
                // The case's "Match keyboard" follows it too (#423).
                crate::foxglove::pc_lights::send(crate::foxglove::pc_lights::Msg::Follow(c));
                let want = hue_sat(c[0], c[1], c[2]);
                if follower.applied_already(why, want) {
                    continue;
                }
                // device() may discover a keyboard; this thread can wait for it.
                let Some(d) = crate::foxglove::devices::device() else {
                    continue;
                };
                let Some(lighting) = d.lighting() else {
                    continue;
                };
                let device = d.info().id.to_string();
                // Read outside the arbiter's lock: it is a keyboard round trip.
                let read = || {
                    lighting.state().ok().map(|s| Reported {
                        hue: s.hue,
                        sat: s.sat,
                        brightness: s.brightness,
                        settled: s.settled,
                    })
                };
                let push = |brightness| {
                    // An effect a quit left running (#542) would hide the colour.
                    crate::foxglove::scene_run::release_left_effect(&d);
                    crate::foxglove::lighting_leases::with(|a| {
                        a.follow(&device, (want.0, want.1, brightness), lighting)
                    })
                    .map_err(|e| crate::foxglove::devices::describe(&e))
                };
                match (follower.step(why, want, read, push), why) {
                    (Step::Sent(true), Wake::Reassert(t)) => {
                        eprintln!(
                            "{}",
                            crate::foxglove::reassert::line("the accent colour", t)
                        );
                    }
                    (Step::Failed(e), _) => eprintln!("Foxglove accent: {e}"),
                    _ => {}
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

    use crate::foxglove::reassert::Trigger;
    use std::cell::{Cell, RefCell};

    /// A keyboard in memory: the hue, saturation and brightness it reports (None: unreadable),
    /// whether its right half agrees, the pushes it got, and whether the next push fails (a
    /// reconnect before `&rgb_ug` resolves, #601).
    struct FakeBoard {
        colour: RefCell<Option<(u16, u8, u8)>>,
        right_agrees: Cell<bool>,
        pushes: Cell<u32>,
        fail_next: Cell<bool>,
    }

    impl FakeBoard {
        fn new(colour: Option<(u16, u8, u8)>) -> FakeBoard {
            FakeBoard {
                colour: RefCell::new(colour),
                right_agrees: Cell::new(true),
                pushes: Cell::new(0),
                fail_next: Cell::new(false),
            }
        }

        fn wake(&self, f: &mut Follower, why: Wake, want: (u16, u8)) -> Step {
            let reported = self.colour.borrow().map(|(hue, sat, brightness)| Reported {
                hue,
                sat,
                brightness,
                settled: self.right_agrees.get(),
            });
            f.step(
                why,
                want,
                || reported,
                |brightness| {
                    self.pushes.set(self.pushes.get() + 1);
                    if self.fail_next.take() {
                        return Err("the keyboard doesn't know that behavior".into());
                    }
                    *self.colour.borrow_mut() = Some((want.0, want.1, brightness));
                    self.right_agrees.set(true);
                    Ok(true)
                },
            )
        }
    }

    /// #601: a push that fails at the left's reconnect goes out again at the next link event
    /// (the right half's relay coming up), once; more events while the keyboard shows it send
    /// nothing.
    #[test]
    fn a_failed_push_is_retried_once_at_the_next_link_event() {
        let blue = hue_sat(0, 120, 215);
        let board = FakeBoard::new(Some((0, 100, 60)));
        board.fail_next.set(true);
        let mut f = Follower::default();
        let first = board.wake(&mut f, Wake::Reassert(Trigger::KeyboardConnected), blue);
        assert!(matches!(first, Step::Failed(_)), "{first:?}");
        assert!(f.failed);
        assert_eq!(board.pushes.get(), 1);
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::RightRelayReady), blue),
            Step::Sent(true),
            "retried at the relay-ready event"
        );
        assert_eq!(board.pushes.get(), 2);
        assert_eq!(
            *board.colour.borrow(),
            Some((blue.0, blue.1, 60)),
            "the board keeps its own brightness"
        );
        for _ in 0..3 {
            for t in Trigger::ALL {
                assert_eq!(board.wake(&mut f, Wake::Reassert(t), blue), Step::Skipped);
            }
            assert_eq!(board.wake(&mut f, Wake::Changed, blue), Step::Skipped);
        }
        assert_eq!(board.pushes.get(), 2, "no spam once the keyboard shows it");
    }

    /// A push that keeps failing is tried once per event, not in a loop.
    #[test]
    fn a_failing_keyboard_gets_one_try_per_event() {
        let blue = hue_sat(0, 120, 215);
        let board = FakeBoard::new(None);
        let mut f = Follower::default();
        for n in 1..=3 {
            board.fail_next.set(true);
            let step = board.wake(&mut f, Wake::Reassert(Trigger::RightRelayReady), blue);
            assert!(matches!(step, Step::Failed(_)));
            assert_eq!(board.pushes.get(), n);
        }
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::KeyboardConnected), blue),
            Step::Sent(true)
        );
        assert!(!f.failed);
    }

    /// After a failure, a re-apply sends even when the keyboard reports the colour (the push
    /// may have reached one half only), and the same colour coming again sends too.
    #[test]
    fn a_failure_overrides_the_readback() {
        let blue = hue_sat(0, 120, 215);
        let board = FakeBoard::new(Some((blue.0, blue.1, 80)));
        let mut f = Follower {
            applied: Some(blue),
            failed: true,
        };
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::PcBack), blue),
            Step::Sent(true)
        );
        assert_eq!(board.pushes.get(), 1);
        let mut f = Follower {
            applied: Some(blue),
            failed: true,
        };
        assert_eq!(board.wake(&mut f, Wake::Changed, blue), Step::Sent(true));
    }

    /// A re-apply sends a colour the keyboard lost (it rebooted to its saved one) and skips one
    /// it shows; an unreadable keyboard gets it at full brightness, as before.
    #[test]
    fn a_reassert_sends_only_what_the_keyboard_lost() {
        let blue = hue_sat(0, 120, 215);
        let board = FakeBoard::new(Some((blue.0, blue.1, 50)));
        let mut f = Follower::default();
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::PcBack), blue),
            Step::Skipped
        );
        assert_eq!(f.applied, Some(blue), "found on the keyboard");
        *board.colour.borrow_mut() = Some((0, 0, 50));
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::DisplayChange), blue),
            Step::Sent(true)
        );
        assert_eq!(board.pushes.get(), 1);
        let unread = FakeBoard::new(None);
        let mut f = Follower::default();
        assert_eq!(
            unread.wake(&mut f, Wake::Reassert(Trigger::TaskbarRestart), blue),
            Step::Sent(true)
        );
        assert_eq!(*unread.colour.borrow(), Some((blue.0, blue.1, 100)));
    }

    /// The right half linking again re-sends a colour its half missed (the halves disagree),
    /// even when the left shows it; other triggers go by the left's report.
    #[test]
    fn a_right_half_that_missed_the_colour_gets_it_at_relay_ready() {
        let blue = hue_sat(0, 120, 215);
        let board = FakeBoard::new(Some((blue.0, blue.1, 50)));
        board.right_agrees.set(false);
        let mut f = Follower {
            applied: Some(blue),
            failed: false,
        };
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::PcBack), blue),
            Step::Skipped
        );
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::RightRelayReady), blue),
            Step::Sent(true)
        );
        assert_eq!(
            board.wake(&mut f, Wake::Reassert(Trigger::RightRelayReady), blue),
            Step::Skipped,
            "once the halves agree"
        );
        assert_eq!(board.pushes.get(), 1);
    }

    /// A colour change already applied touches no keyboard; a lease holding the lights still
    /// counts as applied (the colour comes back when it ends).
    #[test]
    fn a_change_sends_a_new_colour_only() {
        let (blue, red) = (hue_sat(0, 120, 215), hue_sat(255, 0, 0));
        let mut f = Follower {
            applied: Some(blue),
            failed: false,
        };
        assert_eq!(
            f.step(
                Wake::Changed,
                blue,
                || panic!("not read"),
                |_| panic!("not sent")
            ),
            Step::Skipped
        );
        assert_eq!(
            f.step(Wake::Changed, red, || None, |_| Ok(false)),
            Step::Sent(false)
        );
        assert_eq!(f.applied, Some(red));
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
