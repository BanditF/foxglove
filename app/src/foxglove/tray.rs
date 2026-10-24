//! Foxglove's tray controls. Native menus are updated on their owning UI thread.
use super::{art_library, art_service, screens};
use std::sync::atomic::Ordering;
use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, Submenu};

pub struct Controls {
    status: MenuItem,
    menu: Menu,
    update_item: Option<MenuItem>,
    pause: CheckMenuItem,
    art: Submenu,
    signature: String,
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
        let _ = menu.append(&status);
        let _ = menu.append(&MenuItem::with_id("settings", "Settings...", true, None));
        let _ = menu.append(&pause);
        let _ = menu.append(&art);
        Self {
            status,
            menu: menu.clone(),
            update_item: None,
            pause,
            art,
            signature: String::new(),
        }
    }
    pub fn update(&mut self) {
        if let Some(version) = super::update::available_version() {
            let text = format!("Update available: v{version}");
            if let Some(item) = &self.update_item {
                item.set_text(text);
            } else {
                let item = MenuItem::with_id("fox-update", text, true, None);
                let _ = self.menu.append(&item);
                self.update_item = Some(item);
            }
        } else if let Some(item) = self.update_item.take() {
            let _ = self.menu.remove(&item);
        }
        self.pause.set_checked(screens::paused());
        let Some(h) = art_service::handle() else {
            return;
        };
        let s = h.snapshot();
        let library = art_library::snapshot();
        let name = super::link::connected_name().unwrap_or_default();
        self.status.set_text(format!(
            "{} {name} / right art: {}",
            if s.connected {
                "Connected"
            } else {
                "Disconnected"
            },
            s.status.map_or_else(|| "unknown".into(), |s| s.to_string())
        ));
        let signature = format!(
            "{:?}{:?}",
            s.built_ins(),
            library
                .entries
                .iter()
                .map(|e| (&e.path, e.preview.is_ok()))
                .collect::<Vec<_>>()
        );
        self.art.set_enabled(s.connected && !s.busy);
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
