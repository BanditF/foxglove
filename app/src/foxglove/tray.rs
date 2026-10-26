//! Foxglove's tray controls: the right-click menu, the tooltip and the glyph. Native menus are updated on their owning UI thread, and only when
//! something changed: this runs every frame, and touching a native menu can wake the window
//! again, which kept an idle app drawing (#100).
use super::art_service;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

/// What the tray icon shows (#403): the Bandit fox, solid while a keyboard is connected, its
/// outline while none is, and greyed with an orange pause badge while Foxglove is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Connected,
    Disconnected,
    Paused,
}

impl Glyph {
    pub fn of(connected: bool, paused: bool) -> Self {
        match (paused, connected) {
            (true, _) => Glyph::Paused,
            (false, true) => Glyph::Connected,
            (false, false) => Glyph::Disconnected,
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
    const DARK: [[&[u8]; 4]; 3] = [sized!("white"), sized!("white-off"), sized!("white-paused")];
    const LIGHT: [[&[u8]; 4]; 3] = [sized!("black"), sized!("black-off"), sized!("black-paused")];
    let set = if light_taskbar { &LIGHT } else { &DARK };
    let row = match glyph {
        Glyph::Connected => 0,
        Glyph::Disconnected => 1,
        Glyph::Paused => 2,
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
}

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
        let pause = MenuItem::with_id(
            "fox-pause-all",
            super::tray_panel::pause_button(paused).0,
            true,
            None,
        );
        let profile = Submenu::new(PROFILE, true);
        let _ = menu.append(&MenuItem::with_id("settings", OPEN, true, None));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&pause);
        let _ = menu.append(&profile);
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id("fox-logs", LOGS, true, None));
        Self {
            pause,
            paused_shown: paused,
            profile,
            profiles_shown: (vec!["-".into()], None),
            status: status_line(false, "", paused),
            connected: false,
        }
    }

    /// The tray icon's tooltip: the status line and each half's battery (#373).
    pub fn tooltip(&self) -> String {
        super::battery::tooltip(&self.status, &super::battery::levels())
    }

    /// The tray icon's glyph for what Foxglove knows now.
    pub fn glyph(&self) -> Glyph {
        Glyph::of(self.connected, super::pause::paused())
    }

    pub fn update(&mut self) {
        let profiles = super::store::cached_profiles();
        if profiles != self.profiles_shown {
            while self.profile.remove_at(0).is_some() {}
            let (names, active) = &profiles;
            let base = CheckMenuItem::with_id(
                "fox-profile:",
                "Base settings",
                true,
                active.is_none(),
                None,
            );
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
        let name = super::link::connected_name().unwrap_or_default();
        self.status = status_line(self.connected, &name, paused);
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
        let glyphs = [Glyph::Connected, Glyph::Disconnected, Glyph::Paused];
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
}
