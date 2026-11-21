//! Foxglove's tray controls: the right-click menu, the tooltip and the glyph. Native menus are updated on their owning UI thread, and only when
//! something changed: this runs every frame, and touching a native menu can wake the window
//! again, which kept an idle app drawing (#100).
use super::art_service;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

/// What the tray icon shows (#403): the Bandit fox, solid while a keyboard is connected, its
/// outline while none is, and greyed with an orange pause badge while Foxglove is paused. While
/// Screen glow reads the screen (#496) the fox carries an eye badge instead; paused wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Connected,
    Disconnected,
    Paused,
    Screen,
}

impl Glyph {
    pub fn of(connected: bool, paused: bool) -> Self {
        match (paused, connected) {
            (true, _) => Glyph::Paused,
            (false, true) => Glyph::Connected,
            (false, false) => Glyph::Disconnected,
        }
    }

    /// The eye while Screen glow reads the screen (only then); paused wins.
    pub fn with_screen(self, reading: bool) -> Self {
        if reading && self != Glyph::Paused {
            Glyph::Screen
        } else {
            self
        }
    }
}

/// Windows' small-icon sizes the glyphs are drawn for (100, 125, 150 and 200 % scale).
pub const SIZES: [u32; 4] = [16, 20, 24, 32];

/// The drawn size for Windows' small-icon size `metric`: the smallest at least that big (the
/// shell shrinks it, which stays sharp), or the largest above 32 px.
pub fn size_for(metric: i32) -> u32 {
    let metric = u32::try_from(metric).unwrap_or(0);
    SIZES
        .into_iter()
        .find(|&s| s >= metric)
        .unwrap_or(SIZES[SIZES.len() - 1])
}

macro_rules! sized {
    ($name:literal) => {
        [
            include_bytes!(concat!("../../resources/tray-icon-", $name, "-16.png")).as_slice(),
            include_bytes!(concat!("../../resources/tray-icon-", $name, "-20.png")).as_slice(),
            include_bytes!(concat!("../../resources/tray-icon-", $name, "-24.png")).as_slice(),
            include_bytes!(concat!("../../resources/tray-icon-", $name, ".png")).as_slice(),
        ]
    };
}

/// The PNG for a glyph on a light or dark taskbar at one of [`SIZES`] (others get 32 px).
/// Drawn by `scripts/app_icons.py --tray`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn png(glyph: Glyph, light_taskbar: bool, size: u32) -> &'static [u8] {
    const DARK: [[&[u8]; 4]; 4] = [
        sized!("white"),
        sized!("white-off"),
        sized!("white-paused"),
        sized!("white-screen"),
    ];
    const LIGHT: [[&[u8]; 4]; 4] = [
        sized!("black"),
        sized!("black-off"),
        sized!("black-paused"),
        sized!("black-screen"),
    ];
    let set = if light_taskbar { &LIGHT } else { &DARK };
    let row = match glyph {
        Glyph::Connected => 0,
        Glyph::Disconnected => 1,
        Glyph::Paused => 2,
        Glyph::Screen => 3,
    };
    let col = SIZES
        .iter()
        .position(|&s| s == size)
        .unwrap_or(SIZES.len() - 1);
    set[row][col]
}

/// The macOS template for a glyph (black and alpha; the menu bar tints it).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn template_png(glyph: Glyph) -> &'static [u8] {
    match glyph {
        Glyph::Connected => include_bytes!("../../resources/tray-icon-template.png"),
        Glyph::Disconnected => include_bytes!("../../resources/tray-icon-template-off.png"),
        Glyph::Paused => include_bytes!("../../resources/tray-icon-template-paused.png"),
        Glyph::Screen => include_bytes!("../../resources/tray-icon-template-screen.png"),
    }
}

/// The right-click menu: only what's wanted without opening anything. Everything else (the
/// right art, the left screen's pause, updates) lives in the tray panel or the window.
///
/// Open Foxglove · Pause Foxglove / Resume Foxglove · Profile ▸ · Open log folder · Quit
pub struct Controls {
    /// Pause Foxglove (#404); reads Resume Foxglove while paused, as the tray panel's button.
    pause: MenuItem,
    paused_shown: bool,
    profile: Submenu,
    /// The profile names and active one the submenu shows.
    profiles_shown: (Vec<String>, Option<String>),
    /// The tooltip's status line.
    status: String,
    /// Whether a keyboard is connected, for the tray icon's connected/disconnected glyph.
    connected: bool,
    /// The menu, to add and drop the standalone Resume item (#1142).
    menu: Menu,
    /// "Resume wallpapers, paused by Arete" while something holds the wallpapers paused and no
    /// hub is there to show it; its text is the menu item's accessible name.
    we_resume: Option<(String, MenuItem)>,
    /// The line after a Resume or a stop, for the tooltip (the hub page's words).
    we_after: super::we_after::Tracker,
    /// Whether something held the wallpapers at the last update (for the stop line).
    we_held: bool,
    /// The last read of "Wallpaper Engine runs" and when (a process snapshot is not taken
    /// every frame).
    we_running: Option<(bool, std::time::Instant)>,
}

/// The standalone Resume item's id.
const WE_RESUME: &str = "fox-resume-we";
/// Pause Foxglove's place in [`labels`]; the Resume item goes right after it.
const PAUSE_AT: usize = 2;
const WE_RESUME_AT: usize = PAUSE_AT + 1;
/// How long a read of "Wallpaper Engine runs" is kept while something holds the wallpapers.
const RUNNING_KEPT: std::time::Duration = std::time::Duration::from_secs(5);

/// Wakes the overlay's frame (which runs the tray's update) when the lease facts change and
/// when an after-line ends: the tray draws on demand, so nothing else would.
static WAKE: std::sync::OnceLock<crate::ui_wake::UiWake> = std::sync::OnceLock::new();

/// The overlay's frame registers itself, once; a change of the `wallpaper_engine.*` facts
/// (who holds the pause, its reasons, a stale pause) then wakes it. Event-driven, no poll.
pub fn watch(ctx: &egui::Context) {
    if WAKE.set(crate::ui_wake::UiWake::from_ctx(ctx)).is_err() {
        return;
    }
    let changes = super::devices::bus()
        .facts()
        .subscribe("wallpaper_engine.*");
    super::services::spawn_named("tray-we-facts", move || {
        while changes.recv().is_ok() {
            if let Some(w) = WAKE.get() {
                w.request_repaint();
            }
        }
    });
}

/// The standalone tray's Resume text, from who holds the wallpapers paused: `None` when nothing
/// does (or the pause is stale: Wallpaper Engine restarted and plays). The words are the hub
/// panel's, so the item's text (its accessible name) says whose pause it ends. While a play
/// failed and is owed (#1399) the item stays, with no holder to name.
pub fn we_resume_label(
    names: &[String],
    reasons: &[String],
    stale: bool,
    owed: bool,
) -> Option<String> {
    if stale {
        return None;
    }
    if names.is_empty() && reasons.is_empty() {
        return owed.then(|| super::hub_panels::RESUME.to_string());
    }
    Some(super::hub_panels::resume_name(
        &super::hub_panels::holder_words(names),
    ))
}

/// A label as a native menu item's text: `&` marks a mnemonic there, so it is doubled.
fn menu_text(label: &str) -> String {
    label.replace('&', "&&")
}

/// The holders at the moment of a click on Resume, for the "Resumed. ..." line.
static WE_PRESSED: std::sync::Mutex<Option<(Vec<String>, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// The menu's labels, in order (`None` = a separator); Quit is added by `tray.rs`.
pub fn labels(paused: bool) -> [Option<&'static str>; 6] {
    [
        Some(OPEN),
        None,
        Some(super::tray_panel::pause_button(paused).0),
        Some(PROFILE),
        None,
        Some(LOGS),
    ]
}

const OPEN: &str = "Open Foxglove";
const PROFILE: &str = "Profile";
const LOGS: &str = "Open log folder";

/// The tooltip's status line: paused, or whether (and which) keyboard is connected. Short, so
/// the battery line under it always fits Windows' 127 characters (#373).
pub fn status_line(connected: bool, name: &str, paused: bool) -> String {
    match (paused, connected, name.is_empty()) {
        (true, _, _) => "Paused".into(),
        (false, true, true) => "Connected".into(),
        (false, true, false) => format!("Connected: {name}"),
        (false, false, _) => "Disconnected".into(),
    }
}

impl Controls {
    pub fn new(menu: &Menu) -> Self {
        let paused = super::pause::paused();
        // Built from `labels`, so the menu is the one its test pins.
        let mut pause = None;
        let mut profile = None;
        for (i, label) in labels(paused).into_iter().enumerate() {
            let _ = match (i, label) {
                (_, None) => menu.append(&PredefinedMenuItem::separator()),
                (0, Some(text)) => menu.append(&MenuItem::with_id("settings", text, true, None)),
                (PAUSE_AT, Some(text)) => {
                    let item = MenuItem::with_id("fox-pause-all", text, true, None);
                    let added = menu.append(&item);
                    pause = Some(item);
                    added
                }
                (3, Some(text)) => {
                    let item = Submenu::new(text, true);
                    let added = menu.append(&item);
                    profile = Some(item);
                    added
                }
                (_, Some(text)) => menu.append(&MenuItem::with_id("fox-logs", text, true, None)),
            };
        }
        let pause = pause.unwrap_or_else(|| MenuItem::new("", false, None));
        let profile = profile.unwrap_or_else(|| Submenu::new(PROFILE, false));
        Self {
            pause,
            paused_shown: paused,
            profile,
            profiles_shown: (vec!["-".into()], None),
            status: status_line(false, "", paused),
            connected: false,
            menu: menu.clone(),
            we_resume: None,
            we_after: Default::default(),
            we_held: false,
            we_running: None,
        }
    }

    /// Standalone only (a hub shows this on its page): the Resume item while something holds
    /// the wallpapers paused, and the line after a Resume or a stop. Nothing else is read here.
    fn we_update(&mut self) -> Option<String> {
        if super::hub_link::module_mode() {
            return None;
        }
        let stale = super::wallpaper_sync::pause_stale_now();
        let names = if stale {
            Vec::new()
        } else {
            super::wallpaper_sync::local_holders_now()
        };
        let reasons = if stale {
            Vec::new()
        } else {
            super::wallpaper_sync::reasons_now()
        };
        let owed = super::wallpaper_sync::owed_shown_now();
        let label = we_resume_label(&names, &reasons, stale, owed);
        let held = label.is_some();
        let shown = self.we_resume.as_ref().map(|(t, _)| t.as_str());
        if shown != label.as_deref() {
            if let Some((_, old)) = self.we_resume.take() {
                let _ = self.menu.remove(&old);
            }
            if let Some(text) = label {
                // `&` marks a mnemonic in a native menu; a name is the caller's word, so it
                // is shown as given.
                let item = MenuItem::with_id(WE_RESUME, menu_text(&text), true, None);
                let _ = self.menu.insert(&item, WE_RESUME_AT);
                self.we_resume = Some((text, item));
            }
        }
        let now = std::time::Instant::now();
        if let Some((who, at)) = WE_PRESSED.lock().unwrap_or_else(|e| e.into_inner()).take() {
            self.we_after.resume_pressed(&who, at);
        }
        // Only read the process list while the answer can matter.
        let busy = held || self.we_held || self.we_after.showing();
        let running = if busy {
            match self.we_running {
                Some((r, at)) if now.saturating_duration_since(at) < RUNNING_KEPT => r,
                _ => {
                    let r = super::wallpaper_engine::running_now();
                    self.we_running = Some((r, now));
                    r
                }
            }
        } else {
            self.we_running = None;
            true
        };
        if self.we_after.shrank(&names, held) {
            super::we_after::clear_stop();
        }
        let stop = !held && super::we_after::take_stop(now);
        let user_paused = busy && super::live_feed::wallpaper_paused();
        self.we_held = held;
        let line = self
            .we_after
            .update(&names, held, running, user_paused, stop, now);
        if let Some(end) = self.we_after.take_deadline() {
            // One wake-up when the line runs out, so the tooltip drops it on time.
            super::services::spawn_named("tray-we-after-line", move || {
                std::thread::sleep(end.saturating_duration_since(std::time::Instant::now()));
                if let Some(w) = WAKE.get() {
                    w.request_repaint();
                }
            });
        }
        match line {
            // A play owed shows for as long as it lasts, not only for `SHOW` (#1399).
            _ if owed && names.is_empty() && reasons.is_empty() => {
                Some(super::hub_panels::RESUME_FAILED_TIP.to_string())
            }
            line => line.map(|a| super::hub_panels::after_words(&a)),
        }
    }

    /// The tray icon's tooltip: the status line and each half's battery (#373).
    pub fn tooltip(&self) -> String {
        super::battery::tooltip(&self.status, &super::battery::levels())
    }

    /// The tray icon's glyph for what Foxglove knows now.
    pub fn glyph(&self) -> Glyph {
        Glyph::of(self.connected, super::pause::paused())
            .with_screen(super::screen_glow::status().reading)
    }

    pub fn update(&mut self) {
        let profiles = super::store::cached_profiles();
        if profiles != self.profiles_shown {
            while self.profile.remove_at(0).is_some() {}
            let (names, active) = &profiles;
            let base =
                CheckMenuItem::with_id("fox-profile:", "Default", true, active.is_none(), None);
            let _ = self.profile.append(&base);
            for name in names {
                let item = CheckMenuItem::with_id(
                    format!("fox-profile:{name}"),
                    name,
                    true,
                    active.as_deref() == Some(name.as_str()),
                    None,
                );
                let _ = self.profile.append(&item);
            }
            self.profiles_shown = profiles;
        }
        let paused = super::pause::paused();
        if paused != self.paused_shown {
            self.pause
                .set_text(super::tray_panel::pause_button(paused).0);
            self.paused_shown = paused;
        }
        self.connected = art_service::handle().is_some_and(|h| h.snapshot().connected);
        // #801: the tooltip names the focused keyboard while it's connected.
        let focus = super::keyboard_focus::current().filter(|k| k.connected);
        let name = focus
            .as_ref()
            .map(|k| k.name.clone())
            .or_else(super::link::connected_name)
            .unwrap_or_default();
        let connected = self.connected || focus.is_some();
        let we_line = self.we_update();
        self.status = if let Some(line) = we_line {
            line
        } else if super::screen_glow::status().reading && !paused {
            // #496: always visible while the screen is read.
            super::screen_glow::tooltip_status(connected).to_string()
        } else {
            status_line(connected, &name, paused)
        };
    }
}

pub fn event(id: &str) {
    if let Some(name) = id.strip_prefix("fox-profile:") {
        let name = (!name.is_empty()).then_some(name);
        if let Err(e) = super::store::set_active(name) {
            eprintln!("Foxglove: can't switch profile: {e}");
        }
    }
    if id == "fox-logs" {
        super::app_log::open_folder();
    }
    if id == WE_RESUME {
        // The user's word (#1142): every hold ends, as the hub page's Resume does.
        let who = super::wallpaper_sync::local_holders_now();
        *WE_PRESSED.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((who, std::time::Instant::now()));
        super::wallpaper_sync::resume_all();
    }
    if id == "fox-pause-all" {
        super::pause::set(!super::pause::paused());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The right-click menu holds only Open Foxglove, Pause/Resume, Profile and the log
    /// folder (Quit follows), and its pause item reads Resume while paused, as the panel's does.
    #[test]
    fn the_menu_is_short_and_its_pause_flips_to_resume() {
        let running = labels(false);
        assert_eq!(
            running,
            [
                Some("Open Foxglove"),
                None,
                Some("Pause Foxglove"),
                Some("Profile"),
                None,
                Some("Open log folder"),
            ]
        );
        let paused = labels(true);
        assert_eq!(paused[2], Some("Resume Foxglove"));
        assert_eq!(
            paused[2],
            Some(crate::foxglove::tray_panel::pause_button(true).0)
        );
    }

    /// The tooltip's status line is short: the battery line under it always fits.
    #[test]
    fn the_status_line_is_short() {
        assert_eq!(status_line(false, "", false), "Disconnected");
        assert_eq!(status_line(true, "", false), "Connected");
        assert_eq!(
            status_line(true, "Eyelash_Corne", false),
            "Connected: Eyelash_Corne"
        );
        assert_eq!(status_line(true, "Eyelash_Corne", true), "Paused");
    }

    #[test]
    fn paused_wins_over_the_connection() {
        assert_eq!(Glyph::of(true, false), Glyph::Connected);
        assert_eq!(Glyph::of(false, false), Glyph::Disconnected);
        assert_eq!(Glyph::of(true, true), Glyph::Paused);
        assert_eq!(Glyph::of(false, true), Glyph::Paused);
    }

    /// #496: the eye only while frames are read, and paused wins over it.
    #[test]
    fn the_eye_shows_only_while_the_screen_is_read() {
        assert_eq!(Glyph::of(true, false).with_screen(true), Glyph::Screen);
        assert_eq!(Glyph::of(false, false).with_screen(true), Glyph::Screen);
        assert_eq!(Glyph::of(true, false).with_screen(false), Glyph::Connected);
        assert_eq!(Glyph::of(true, true).with_screen(true), Glyph::Paused);
    }

    #[test]
    fn the_drawn_size_covers_the_small_icon_size() {
        assert_eq!(size_for(16), 16);
        assert_eq!(size_for(18), 20);
        assert_eq!(size_for(20), 20);
        assert_eq!(size_for(24), 24);
        assert_eq!(size_for(28), 32);
        assert_eq!(size_for(32), 32);
        // Above 200 % the shell scales the largest; a failed metric gets the smallest.
        assert_eq!(size_for(40), 32);
        assert_eq!(size_for(0), 16);
    }

    /// Every glyph decodes at the size it's filed under, draws something, and the three states
    /// differ on each taskbar.
    #[test]
    fn every_glyph_is_drawn_at_its_size() {
        let glyphs = [
            Glyph::Connected,
            Glyph::Disconnected,
            Glyph::Paused,
            Glyph::Screen,
        ];
        for light in [false, true] {
            for size in SIZES {
                let mut seen = Vec::new();
                for g in glyphs {
                    let img = image::load_from_memory(png(g, light, size))
                        .expect("decodes")
                        .into_rgba8();
                    assert_eq!(
                        img.dimensions(),
                        (size, size),
                        "{g:?} light={light} {size}px"
                    );
                    let drawn = img.pixels().filter(|p| p[3] > 128).count();
                    assert!(
                        drawn > (size * size / 8) as usize,
                        "{g:?} {size}px is nearly empty"
                    );
                    assert!(
                        !seen.contains(&img),
                        "{g:?} {size}px looks like another state"
                    );
                    seen.push(img);
                }
            }
            assert_eq!(
                png(Glyph::Connected, light, 40),
                png(Glyph::Connected, light, 32)
            );
        }
        for g in glyphs {
            let img = image::load_from_memory(template_png(g))
                .expect("decodes")
                .into_rgba8();
            assert_eq!(img.dimensions(), (44, 44));
        }
    }

    fn luminance(c: [u8; 3]) -> f64 {
        let lin = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// #1085: at 16 px the eye is a one-pixel rim, a clear gap and a pupil, all fully opaque and in
    /// the taskbar's orange; the round pause badge is solid where the eye is clear.
    #[test]
    fn the_eye_reads_on_both_taskbars_at_16_px() {
        for (light, taskbar, orange) in [
            (false, [0x20, 0x20, 0x20], [0xf0, 0x8a, 0x3c]),
            (true, [0xf3, 0xf3, 0xf3], [0xb4, 0x54, 0x0f]),
        ] {
            let load = |g| {
                image::load_from_memory(png(g, light, 16))
                    .expect("decodes")
                    .into_rgba8()
            };
            let eye = load(Glyph::Screen);
            // The badge box is 9 x 6 in the bottom-right corner; its middle row is y = 13.
            for (x, name) in [(7, "rim"), (11, "pupil")] {
                let p = eye.get_pixel(x, 13);
                assert_eq!(p[3], 255, "{name} is opaque (light={light})");
                assert_eq!([p[0], p[1], p[2]], orange, "{name} is the taskbar's orange");
                assert!(
                    contrast(orange, taskbar) >= 3.0,
                    "{name} reads (light={light})"
                );
            }
            assert_eq!(eye.get_pixel(8, 13)[3], 0, "clear between rim and pupil");
            let pause = load(Glyph::Paused);
            assert_eq!(
                pause.get_pixel(12, 12)[3],
                255,
                "the pause badge is solid here"
            );
        }
    }

    /// The fox reads on its taskbar, connected (white on dark, black on light) and paused (grey).
    #[test]
    fn the_fox_reads_on_both_taskbars() {
        for (light, taskbar) in [(false, [0x20, 0x20, 0x20]), (true, [0xf3, 0xf3, 0xf3])] {
            for g in [Glyph::Connected, Glyph::Paused] {
                let img = image::load_from_memory(png(g, light, 32))
                    .expect("decodes")
                    .into_rgba8();
                let face = img.get_pixel(16, 16);
                assert_eq!(face[3], 255, "{g:?} light={light}");
                let c = contrast([face[0], face[1], face[2]], taskbar);
                assert!(c >= 3.0, "{g:?} light={light}: contrast {c:.2}");
            }
        }
    }

    /// The paused fox's badge is the theme's orange (the darker one on a light taskbar), and the
    /// fox is not: only the paused glyph carries colour.
    #[test]
    fn only_the_paused_fox_has_the_orange_badge() {
        for (light, orange) in [(false, [0xf0, 0x8a, 0x3c]), (true, [0xb4, 0x54, 0x0f])] {
            let badge = |g| {
                let img = image::load_from_memory(png(g, light, 32))
                    .expect("decodes")
                    .into_rgba8();
                *img.get_pixel(20, 24)
            };
            let p = badge(Glyph::Paused);
            assert_eq!(
                [p[0], p[1], p[2], p[3]],
                [orange[0], orange[1], orange[2], 255],
                "light={light}"
            );
            assert!(
                contrast(
                    [p[0], p[1], p[2]],
                    if light { [0xf3; 3] } else { [0x20; 3] }
                ) >= 3.0
            );
            for g in [Glyph::Connected, Glyph::Disconnected] {
                let img = image::load_from_memory(png(g, light, 32))
                    .expect("decodes")
                    .into_rgba8();
                assert!(
                    img.pixels()
                        .all(|q| q[3] == 0 || q[0] == q[1] && q[1] == q[2]),
                    "{g:?} is grey"
                );
            }
        }
    }

    /// Standalone Resume (#1142): the hub panel's words, the holder in the item's text, none
    /// when nothing holds the wallpapers or the pause is stale.
    #[test]
    fn the_resume_item_goes_between_pause_and_profile() {
        let l = labels(false);
        assert_eq!(l[PAUSE_AT], Some("Pause Foxglove"));
        assert_eq!(l[WE_RESUME_AT], Some(PROFILE));
    }

    /// A name with `&` keeps its words in the label; the menu text doubles it for the mnemonic.
    #[test]
    fn an_ampersand_in_a_holder_name_stays_in_the_words() {
        let who = vec!["R&D tools".to_string()];
        let label = we_resume_label(&who, &[], false, false).unwrap();
        assert_eq!(label, "Resume wallpapers, paused by R&D tools");
        assert_eq!(menu_text(&label), "Resume wallpapers, paused by R&&D tools");
    }

    /// A caller that gave no name reads "an app" mid-sentence.
    #[test]
    fn a_nameless_caller_reads_an_app() {
        let who = vec!["An app".to_string()];
        assert_eq!(
            we_resume_label(&who, &[], false, false).as_deref(),
            Some("Resume wallpapers, paused by an app")
        );
    }

    /// #1399: the item stays while a play is owed, with no holder in its words; the tooltip line
    /// fits Windows' 127 characters next to the battery line.
    #[test]
    fn the_resume_item_stays_while_a_play_is_owed() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            we_resume_label(&none, &none, false, true).as_deref(),
            Some("Resume wallpapers")
        );
        assert_eq!(
            we_resume_label(&none, &none, true, true),
            None,
            "stale: gone"
        );
        let tip = crate::foxglove::hub_panels::RESUME_FAILED_TIP;
        assert_eq!(tip, "Couldn't resume wallpapers. Try again.");
        let levels: std::collections::BTreeMap<String, u8> =
            [("left".to_string(), 80u8), ("right".to_string(), 80u8)].into();
        let full = crate::foxglove::battery::tooltip(tip, &levels);
        assert!(full.contains(tip), "not cut: {full}");
        assert!(full.encode_utf16().count() <= 127);
    }

    #[test]
    fn the_standalone_resume_names_whose_pause_it_ends() {
        let none: Vec<String> = Vec::new();
        assert_eq!(we_resume_label(&none, &none, false, false), None);
        let arete = vec!["Arete".to_string()];
        assert_eq!(
            we_resume_label(&arete, &none, false, false).as_deref(),
            Some("Resume wallpapers, paused by Arete")
        );
        assert_eq!(
            we_resume_label(&arete, &none, false, false),
            Some(crate::foxglove::hub_panels::resume_name("Arete"))
        );
        // A hold with no name still says what it ends.
        assert_eq!(
            we_resume_label(&none, &["render".to_string()], false, false).as_deref(),
            Some("Resume wallpapers, paused by an app")
        );
        assert_eq!(we_resume_label(&arete, &none, true, false), None);
    }
}
