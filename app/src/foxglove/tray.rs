//! Foxglove's tray controls. Native menus are updated on their owning UI thread, and only when
//! something changed: this runs every frame, and touching a native menu can wake the window
//! again, which kept an idle app drawing (#100).
use super::{art_library, art_service, screens};
use std::sync::atomic::Ordering;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, Submenu};

pub struct Controls {
    status: MenuItem,
    menu: Menu,
    update_item: Option<MenuItem>,
    pause: CheckMenuItem,
    art: Submenu,
    profile: Submenu,
    /// The profile names and active one the submenu shows.
    profiles_shown: (Vec<String>, Option<String>),
    signature: String,
    /// What the items show now: update text, paused, status text, art menu enabled.
    shown: (Option<String>, bool, String, bool),
    /// Whether a keyboard is connected, for the tray icon's connected/disconnected glyph.
    connected: bool,
}
impl Controls {
    pub fn new(menu: &Menu) -> Self {
        let status = MenuItem::new("Disconnected", false, None);
        let pause = CheckMenuItem::with_id(
            "fox-pause",
            "Pause left panel",
            true,
            screens::paused(),
            None,
        );
        let art = Submenu::new("Right art", false);
        let profile = Submenu::new("Profile", true);
        let _ = menu.append(&status);
        let _ = menu.append(&MenuItem::with_id("settings", "Settings...", true, None));
        let _ = menu.append(&pause);
        let _ = menu.append(&art);
        let _ = menu.append(&profile);
        Self {
            status,
            menu: menu.clone(),
            update_item: None,
            pause,
            art,
            profile,
            profiles_shown: (vec!["-".into()], None),
            signature: String::new(),
            shown: (None, screens::paused(), "Disconnected".into(), false),
            connected: false,
        }
    }
    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn update(&mut self) {
        let profiles = super::store::cached_profiles();
        if profiles != self.profiles_shown {
            while self.profile.remove_at(0).is_some() {}
            let (names, active) = &profiles;
            let base = CheckMenuItem::with_id("fox-profile:", "Base settings", true, active.is_none(), None);
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
        let update = super::update::available_version().map(|v| format!("Update available: v{v}"));
        if update != self.shown.0 {
            match (&update, &self.update_item) {
                (Some(text), Some(item)) => item.set_text(text),
                (Some(text), None) => {
                    let item = MenuItem::with_id("fox-update", text, true, None);
                    let _ = self.menu.append(&item);
                    self.update_item = Some(item);
                }
                (None, _) => {
                    if let Some(item) = self.update_item.take() {
                        let _ = self.menu.remove(&item);
                    }
                }
            }
            self.shown.0 = update;
        }
        let paused = screens::paused();
        if paused != self.shown.1 {
            self.pause.set_checked(paused);
            self.shown.1 = paused;
        }
        let Some(h) = art_service::handle() else {
            return;
        };
        let s = h.snapshot();
        self.connected = s.connected;
        let library = art_library::snapshot();
        let name = super::link::connected_name().unwrap_or_default();
        let status = format!(
            "{} {name} / right art: {}",
            if s.connected {
                "Connected"
            } else {
                "Disconnected"
            },
            s.status.map_or_else(|| "unknown".into(), |s| s.to_string())
        );
        if status != self.shown.2 {
            self.status.set_text(&status);
            self.shown.2 = status;
        }
        let signature = format!(
            "{:?}{:?}",
            s.built_ins(),
            library
                .entries
                .iter()
                .map(|e| (&e.path, e.preview.is_ok()))
                .collect::<Vec<_>>()
        );
        let enabled = s.connected && !s.busy;
        if enabled != self.shown.3 {
            self.art.set_enabled(enabled);
            self.shown.3 = enabled;
        }
        if signature == self.signature {
            return;
        }
        self.signature = signature;
        while self.art.remove_at(0).is_some() {}
        let _ = self
            .art
            .append(&MenuItem::with_id("fox-set--1", "Random", true, None));
        for (n, name) in s.built_ins() {
            let _ = self.art.append(&MenuItem::with_id(
                format!("fox-set-{n}"),
                name,
                true,
                None,
            ));
        }
        for entry in library.entries {
            if entry.preview.is_ok() {
                let _ = self.art.append(&MenuItem::with_id(
                    format!("fox-file-{}", entry.path.display()),
                    entry.path.file_stem().unwrap_or_default().to_string_lossy(),
                    true,
                    None,
                ));
            }
        }
        let _ = self
            .art
            .append(&MenuItem::with_id("fox-push", "Push file...", true, None));
    }
}
pub fn event(id: &str, on_settings: &dyn Fn()) {
    if let Some(name) = id.strip_prefix("fox-profile:") {
        let name = (!name.is_empty()).then_some(name);
        if let Err(e) = super::store::set_active(name) {
            eprintln!("Foxglove: can't switch profile: {e}");
        }
    }
    if id == "fox-update" {
        on_settings();
    }
    if id == "fox-pause" {
        screens::set_paused(!screens::paused());
    }
    if id == "fox-push" {
        art_library::PICK_FILE.store(true, Ordering::Relaxed);
        on_settings();
    }
    if let Some(h) = art_service::handle() {
        if let Some(n) = id.strip_prefix("fox-set-").and_then(|n| n.parse().ok()) {
            h.select(n);
        }
        if let Some(path) = id.strip_prefix("fox-file-") {
            h.push(path.into());
        }
    }
}
