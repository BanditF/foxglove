use image::load_from_memory;
use std::process;
use std::sync::Arc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

/// Keeps the tray icon alive for the lifetime of the app.
///
/// On Linux the icon lives on a dedicated GTK thread: libappindicator
/// registers the StatusNotifierItem over DBus via the glib main loop, so the
/// tray only works if `gtk::main()` runs on the thread that created it.
pub struct Tray {
    /// Foxglove: the icon and its menu, or nothing until the taskbar exists (sign-in).
    #[cfg(not(target_os = "linux"))]
    icon: crate::foxglove::taskbar::Slot<(TrayIcon, crate::foxglove::tray::Controls)>,
    /// Foxglove: the glyph shown (#403).
    #[cfg(not(target_os = "linux"))]
    glyph: crate::foxglove::tray::Glyph,
    /// Foxglove: the tooltip shown, with the battery (#373).
    #[cfg(not(target_os = "linux"))]
    tooltip: String,
}

// Windows convention is a glyph matching the taskbar theme. Foxglove (#403): the fox drawn
// for the small-icon size at the system's scale, so the shell doesn't shrink a big one.
#[cfg(target_os = "windows")]
fn icon_bytes(glyph: crate::foxglove::tray::Glyph) -> &'static [u8] {
    let light_theme = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|value| value != 0)
        .unwrap_or(false);
    // SAFETY: GetSystemMetrics takes no pointers and only reads a system value; 0 on failure.
    let metric = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON,
        )
    };
    let size = crate::foxglove::tray::size_for(metric);
    crate::foxglove::tray::png(glyph, light_theme, size)
}

// Monochrome template image; macOS tints it to match the menu bar.
#[cfg(target_os = "macos")]
fn icon_bytes(glyph: crate::foxglove::tray::Glyph) -> &'static [u8] {
    crate::foxglove::tray::template_png(glyph)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn icon_bytes(_glyph: crate::foxglove::tray::Glyph) -> &'static [u8] {
    include_bytes!("../resources/icon.ico")
}

fn create_icon(glyph: crate::foxglove::tray::Glyph) -> Icon {
    let icon = load_from_memory(icon_bytes(glyph))
        .expect("Failed to load icon.")
        .into_rgba8();

    let (width, height) = icon.dimensions();
    Icon::from_rgba(icon.into_raw(), width, height).expect("Failed to create icon.")
}

// Foxglove: an error instead of a panic, so a missing taskbar can't end the app.
fn build_tray_icon() -> Result<(TrayIcon, crate::foxglove::tray::Controls), String> {
    let menu = Menu::new();
    let controls = crate::foxglove::tray::Controls::new(&menu);
    menu.append_items(&[
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("quit", "Quit", true, None),
    ])
    .map_err(|e| format!("tray menu: {e}"))?;

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(create_icon(crate::foxglove::tray::Glyph::Disconnected))
        .with_tooltip("Foxglove");

    // Left click opens the settings instead (see `create_tray_icon`).
    #[cfg(target_os = "windows")]
    let builder = builder.with_menu_on_left_click(false);

    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);

    let icon = builder.build().map_err(|e| format!("tray icon: {e}"))?;
    Ok((icon, controls))
}

pub fn create_tray_icon(on_settings: Arc<dyn Fn() + Send + Sync>) -> Tray {
    crate::foxglove::services::spawn_named("tray-menu", {
        let on_settings = on_settings.clone();
        move || {
            while let Ok(event) = MenuEvent::receiver().recv() {
                match event.id.0.as_str() {
                    "settings" => on_settings(),
                    "quit" => {
                        crate::foxglove::app_log::line("Foxglove: Quit from the tray menu");
                        // Foxglove: hand the left screen's panel back before going.
                        crate::foxglove::screens::stop();
                        crate::foxglove::art_service::stop();
                        crate::foxglove::calibrate_ui::cancel(); // Foxglove: mid-calibration
                        process::exit(0)
                    }
                    _ => crate::foxglove::tray::event(event.id.0.as_str(), on_settings.as_ref()),
                }
            }
        }
    });

    // Foxglove (#391): left-clicking the icon opens the tray panel next to it.
    #[cfg(target_os = "windows")]
    crate::foxglove::services::spawn_named("tray-click", move || {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        while let Ok(event) = TrayIconEvent::receiver().recv() {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let size = [f64::from(rect.size.width), f64::from(rect.size.height)];
                crate::foxglove::tray_panel::clicked([rect.position.x, rect.position.y], size);
            }
        }
    });

    #[cfg(target_os = "linux")]
    {
        std::thread::spawn(|| {
            gtk::init().expect("Failed to initialize GTK. Is a display available?");
            let (_icon, mut controls) = match build_tray_icon() {
                Ok(tray) => tray,
                Err(e) => return eprintln!("Foxglove: no tray icon ({e})"),
            };
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                controls.update();
                gtk::glib::ControlFlow::Continue
            });
            gtk::main();
        });
        Tray {}
    }

    #[cfg(not(target_os = "linux"))]
    {
        let icon = crate::foxglove::taskbar::Slot::new(build_tray_icon);
        Tray { icon, glyph: crate::foxglove::tray::Glyph::Disconnected, tooltip: "Foxglove".into() }
    }
}

impl Tray {
    pub fn update(&mut self) {
        #[cfg(not(target_os = "linux"))]
        {
            // Foxglove: the taskbar started (sign-in, Explorer restart): make a missing icon.
            if crate::foxglove::taskbar::take() && self.icon.taskbar_created(build_tray_icon) {
                self.glyph = crate::foxglove::tray::Glyph::Disconnected;
                self.tooltip = "Foxglove".into();
            }
            let Some((icon, controls)) = self.icon.get_mut() else { return };
            controls.update();
            // Foxglove: swap the glyph only when it changes.
            let glyph = controls.glyph();
            if glyph != self.glyph {
                self.glyph = glyph;
                let _ = icon.set_icon(Some(create_icon(glyph)));
            }
            // Foxglove (#373): the tooltip only when it changes.
            // Kept only once Windows took it, so a failed set is tried again next frame.
            let tooltip = controls.tooltip();
            if tooltip != self.tooltip && icon.set_tooltip(Some(&tooltip)).is_ok() {
                self.tooltip = tooltip;
            }
        }
    }
}
