//! Hideout's tray icon (spec §2.1, job 1): the shared fox glyph, a left click that shows the
//! window, and a menu of the hub's own items. Modules' declared actions join the menu with the
//! registry (H2.2). The icon waits for the taskbar like Foxglove's (`hub_shell::taskbar`).

use std::sync::Arc;

use hub_shell::taskbar::Slot;
use hub_shell::tray_glyph::{self, Glyph};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const OPEN: &str = "open";
const QUIT: &str = "quit";

/// The hub's own menu items, in order (id, label).
pub fn items() -> [(&'static str, &'static str); 2] {
    [(OPEN, "Open Hideout"), (QUIT, "Quit")]
}

fn light_taskbar() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|value| value != 0)
        .unwrap_or(false)
}

fn icon() -> Result<Icon, String> {
    // The hub has no keyboard: the solid fox means "running".
    let png = tray_glyph::png(Glyph::Connected, light_taskbar(), 32);
    let rgba = image::load_from_memory(png)
        .map_err(|e| format!("tray glyph: {e}"))?
        .into_rgba8();
    let (w, h) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), w, h).map_err(|e| format!("tray glyph: {e}"))
}

fn build() -> Result<TrayIcon, String> {
    let menu = Menu::new();
    let [(open_id, open), (quit_id, quit)] = items();
    menu.append_items(&[
        &MenuItem::with_id(open_id, open, true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id(quit_id, quit, true, None),
    ])
    .map_err(|e| format!("tray menu: {e}"))?;
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon()?)
        .with_tooltip(crate::APP)
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| format!("tray icon: {e}"))
}

/// The icon, kept alive with the app.
pub struct Tray(Slot<TrayIcon>);

impl Tray {
    /// Makes the icon (or waits for the taskbar) and starts the menu and click threads, which
    /// block on their channels (no timer). `show` brings the window up; Quit ends the process.
    pub fn new(show: Arc<dyn Fn() + Send + Sync>) -> Self {
        let on_menu = show.clone();
        let _ = std::thread::Builder::new()
            .name("tray-menu".into())
            .spawn(move || {
                while let Ok(event) = MenuEvent::receiver().recv() {
                    match event.id.0.as_str() {
                        OPEN => on_menu(),
                        QUIT => {
                            eprintln!("Hideout: Quit from the tray menu");
                            std::process::exit(0)
                        }
                        _ => {}
                    }
                }
            });
        let _ = std::thread::Builder::new()
            .name("tray-click".into())
            .spawn(move || {
                while let Ok(event) = TrayIconEvent::receiver().recv() {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show();
                    }
                }
            });
        Self(Slot::new(crate::APP, build))
    }

    /// Each frame: makes a missing icon once the taskbar has started.
    pub fn frame(&mut self) {
        if hub_shell::taskbar::take() {
            self.0.taskbar_created(build);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_menu_is_open_and_quit_only() {
        let labels: Vec<_> = super::items().iter().map(|(_, l)| *l).collect();
        assert_eq!(labels, ["Open Hideout", "Quit"]);
    }
}
