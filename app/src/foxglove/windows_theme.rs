//! Windows' app mode (Settings > Personalisation > Colours > "Choose your app mode"): the one
//! source for everything in Foxglove that follows it.
//!
//! - The window theme's **System** choice (`theme.rs`): Foxglove picks Dark or Light from
//!   [`current`] itself, rather than egui's own reading of the system theme.
//! - The wallpapers' **Follow Windows** theme (`wallpaper_sync`, `foxglove_theme`): Wallpaper
//!   Engine's browser doesn't reliably show Windows' mode to a page (`prefers-color-scheme`), so
//!   Foxglove tells every wallpaper that supports `theme`, and two monitors never disagree.
//!
//! The value is `AppsUseLightTheme` under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize` (1 light, 0 dark), the
//! value Windows' own apps read. Missing or unreadable: unknown (`None`), and each follower keeps
//! its own fallback.
//!
//! Event-driven: Windows' `UISettings.ColorValuesChanged`, the event `accent.rs` already uses
//! for the accent colour, fires when the app mode changes too; the watcher reads the value again
//! then and tells the followers when it differs. Nothing polls.

use std::sync::atomic::{AtomicU8, Ordering};

/// Windows' app mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    Light,
    Dark,
}

impl AppMode {
    /// From `AppsUseLightTheme`: non-zero is light, zero dark, missing unknown.
    pub fn from_apps_use_light_theme(value: Option<u32>) -> Option<AppMode> {
        value.map(|v| {
            if v != 0 {
                AppMode::Light
            } else {
                AppMode::Dark
            }
        })
    }

    /// The `foxglove_theme` property's value (docs/wallpaper-manifest.md).
    pub fn property(self) -> &'static str {
        match self {
            AppMode::Light => "light",
            AppMode::Dark => "dark",
        }
    }
}

/// The last value read: 0 never read, 1 unknown, 2 light, 3 dark.
static LAST: AtomicU8 = AtomicU8::new(0);

fn encode(mode: Option<AppMode>) -> u8 {
    match mode {
        None => 1,
        Some(AppMode::Light) => 2,
        Some(AppMode::Dark) => 3,
    }
}

fn decode(v: u8) -> Option<AppMode> {
    match v {
        2 => Some(AppMode::Light),
        3 => Some(AppMode::Dark),
        _ => None,
    }
}

/// `AppsUseLightTheme` from the registry.
#[cfg(windows)]
fn read() -> Option<AppMode> {
    use winreg::enums::HKEY_CURRENT_USER;
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .ok()?;
    AppMode::from_apps_use_light_theme(key.get_value::<u32, _>("AppsUseLightTheme").ok())
}

#[cfg(not(windows))]
fn read() -> Option<AppMode> {
    None
}

/// Windows' app mode now: the last value read (read once on first use), `None` when Windows
/// doesn't say.
pub fn current() -> Option<AppMode> {
    match LAST.load(Ordering::Relaxed) {
        0 => refresh(),
        v => decode(v),
    }
}

/// Reads the value again; when it changed, tells the followers (the app's System theme and
/// the wallpapers).
fn refresh() -> Option<AppMode> {
    let now = read();
    let before = LAST.swap(encode(now), Ordering::Relaxed);
    if before != 0 && before != encode(now) {
        eprintln!(
            "Foxglove: Windows app mode is now {}",
            now.map_or("unknown", AppMode::property)
        );
        super::theme::windows_changed(now);
    }
    super::wallpaper_sync::theme(now);
    now
}

/// Starts following (Windows): the wallpapers get the mode now, and again on each change.
pub fn start() {
    refresh();
    #[cfg(windows)]
    watch();
}

#[cfg(windows)]
fn watch() {
    use std::sync::mpsc;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::ViewManagement::UISettings;

    super::services::spawn_named("windows-theme", || {
        let Ok(ui) = UISettings::new() else { return };
        let (tx, rx) = mpsc::channel::<()>();
        // The handler runs on a thread-pool thread; it only wakes this one. Keeping `ui` alive
        // here keeps the subscription.
        let _token = ui.ColorValuesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = tx.send(());
            Ok(())
        }));
        for () in rx {
            refresh();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_value_reads_as_the_app_mode() {
        assert_eq!(
            AppMode::from_apps_use_light_theme(Some(1)),
            Some(AppMode::Light)
        );
        assert_eq!(
            AppMode::from_apps_use_light_theme(Some(0)),
            Some(AppMode::Dark)
        );
        assert_eq!(AppMode::from_apps_use_light_theme(None), None);
    }

    #[test]
    fn the_cache_round_trips_every_mode() {
        for m in [None, Some(AppMode::Light), Some(AppMode::Dark)] {
            assert_eq!(decode(encode(m)), m);
        }
        assert_eq!(AppMode::Light.property(), "light");
        assert_eq!(AppMode::Dark.property(), "dark");
    }
}
