//! The tray glyph (#403), shared by Foxglove and Hideout: the Bandit fox, solid while connected,
//! its outline while not, and greyed with an orange pause badge while paused; drawn for each of
//! Windows' small-icon sizes and for a light or dark taskbar.
/// What the tray icon shows (#403): the Bandit fox, solid while a keyboard is connected, its
/// outline while none is, and greyed with an orange pause badge while the app is paused.
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
