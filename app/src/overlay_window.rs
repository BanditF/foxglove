use crate::device_discovery::DiscoveredDevice;
use crate::platform::OverlayHost;
use crate::settings::Settings;
use crate::ui_wake::UiWake;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

mod connection_flow;
mod foxglove_connect;
mod foxglove_settings;
mod settings_sync;
mod state;
mod ui_overlay;
mod ui_settings;
use state::{
    AppConnectionState, ConnectDraftState, ConnectionDraft, SessionState, SettingsState, UiState,
};

pub struct OverlayApp {
    _tray: crate::tray::Tray,
    settings_requested: Arc<AtomicBool>,
    ui_wake: UiWake,
    pub(crate) ui: UiState,
    settings: SettingsState,
    session: SessionState,
    connect: ConnectDraftState,
    serial_watch: foxglove_connect::SerialWatch,
    pub(crate) editor: crate::keymap_editor::EditorState,
    /// Foxglove: the last profile switch the overlay applied.
    profile_seen: u64,
}

impl OverlayApp {
    pub fn new(
        tray: crate::tray::Tray,
        settings_requested: Arc<AtomicBool>,
        ui_wake: UiWake,
        base_settings: Settings,
        available_devices: Vec<DiscoveredDevice>,
    ) -> Self {
        Self {
            _tray: tray,
            settings_requested,
            ui_wake,
            ui: UiState {
                settings_visible: true,
                settings_error: None,
                settings_warning: None,
                mouse_passthrough: None,
                hit_region: None,
                file_dialog: egui_file_dialog::FileDialog::new(),
                settings_window: Default::default(),
                overlay_rect: None,
                overlay_hovered: false,
            },
            settings: SettingsState {
                active: base_settings.clone(),
                draft: base_settings,
            },
            session: SessionState {
                connection: AppConnectionState::Disconnected,
                ever_connected: false,
                last_spec: None,
                reopen: None,
                connected_definition: None,
                layout_names: Vec::new(),
                active_layout_name: String::new(),
                draft_layout_name: String::new(),
            },
            connect: ConnectDraftState {
                available_devices,
                selected_device_index: None,
                draft: ConnectionDraft::Via {
                    json_path: String::new(),
                },
                pending_connect: None,
            },
            editor: crate::keymap_editor::EditorState::new(),
            serial_watch: foxglove_connect::SerialWatch::default(),
            profile_seen: 0,
        }
    }

    /// The display the overlay should be shown on (empty: the main display).
    pub fn monitor(&self) -> &str {
        &self.settings.active.monitor
    }

    pub(crate) fn is_any_window_open(&self) -> bool {
        self.ui.settings_visible || self.editor.target.is_some()
    }

    fn sync_mouse_passthrough(&mut self, ctx: &egui::Context, host: &mut dyn OverlayHost) {
        // Foxglove (#391): the tray panel takes the mouse too, but only over itself (#405): a
        // click anywhere else goes straight to the window underneath, and the panel closes when
        // the overlay loses the focus to it. The Settings window is a window of its own; while
        // it's open the overlay takes the mouse only over the keys (to open the editor), and over
        // the whole display only while a key's tooltip shows or the editor is open.
        // A host without native windows (Wayland) draws Settings inside the overlay: the whole
        // overlay takes the mouse then, as the editor does.
        let editor = self.editor.target.is_some()
            || (self.ui.settings_visible && ctx.embed_viewports());
        let tray = crate::foxglove::tray_panel::is_open();
        let keys = self
            .ui
            .overlay_rect
            .filter(|_| self.ui.settings_visible && !self.ui.overlay_hovered);
        let region = if editor || (self.ui.settings_visible && self.ui.overlay_hovered) {
            None
        } else if tray {
            crate::foxglove::tray_panel::hit_region(ctx)
        } else {
            keys.map(|r| crate::foxglove::settings_window::keys_region(r, ctx.pixels_per_point()))
        };
        if region != self.ui.hit_region {
            host.set_hit_region(region);
            self.ui.hit_region = region;
        }
        let takes_mouse = editor
            || tray
            || (self.ui.settings_visible && self.ui.overlay_rect.is_some());
        let mouse_passthrough = !takes_mouse;
        if self.ui.mouse_passthrough == Some(mouse_passthrough) {
            return;
        }

        host.set_passthrough(mouse_passthrough);
        self.ui.mouse_passthrough = Some(mouse_passthrough);
    }

    /// Draw a centered modal with `message` and an OK button that clears `slot`.
    fn message_window(ctx: &egui::Context, title: &str, slot: &mut Option<String>) {
        let Some(message) = slot.clone() else {
            return;
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(message);
                ui.add_space(10.0);
                if ui.button("OK").clicked() {
                    *slot = None;
                }
            });
    }

    /// Requests the editor window to close, initiating a ZMK save first if changes
    /// are pending; otherwise closes immediately.
    pub(crate) fn request_close_editor(&mut self) {
        if self.editor.request_close() {
            if let AppConnectionState::Connected { keyboard } = &self.session.connection {
                keyboard.end_edit_session();
            }
        }
    }

    /// Closes the editor window immediately and ends any open ZMK write session on the
    /// connected keyboard, if one is present.
    pub(crate) fn close_editor(&mut self) {
        self.editor.reset();
        if let AppConnectionState::Connected { keyboard } = &self.session.connection {
            keyboard.end_edit_session();
        }
    }

    /// Wakes the UI up when the overlay is due to appear or disappear on its own.
    fn schedule_overlay_repaint(&self, ctx: &egui::Context) {
        // Foxglove (#404): paused, the overlay stays hidden, so nothing is due.
        if self.is_any_window_open() || crate::foxglove::pause::paused() {
            return;
        }

        let AppConnectionState::Connected { keyboard } = &self.session.connection else {
            return;
        };

        if let Some(delay) = keyboard.overlay_changes_in(Instant::now()) {
            ctx.request_repaint_after(delay);
        }
    }
}

impl OverlayApp {
    /// Backdrop color the host clears to before egui paints: dimmed while the keymap editor is
    /// open, otherwise transparent so only the overlay is visible. Foxglove: the Settings window
    /// is a window of its own and dims nothing.
    pub fn clear_color(&self) -> egui::Rgba {
        if self.editor.target.is_some() {
            egui::Rgba::from_black_alpha(0.65)
        } else {
            egui::Rgba::TRANSPARENT
        }
    }

    /// A [`KeyPaintStyle`] tuned for the given unit size (pixels per key-unit):
    /// `active.size`-scaled keys on the overlay, miniature ones in pickers.
    pub(crate) fn paint_style(&self, unit: f32) -> crate::key_paint::KeyPaintStyle {
        crate::key_paint::KeyPaintStyle::from_settings(&self.settings.active).with_unit(unit)
    }

    pub fn ui(&mut self, ctx: &egui::Context, host: &mut dyn OverlayHost) {
        if self.settings_requested.swap(false, Ordering::Relaxed) {
            if self.ui.settings_visible {
                // Foxglove: already open, maybe behind other windows or on another display.
                ctx.send_viewport_cmd_to(
                    crate::foxglove::settings_window::id(),
                    egui::ViewportCommand::Focus,
                );
            }
            self.ui.settings_visible = true;
        }

        self.poll_connect_result();
        self.auto_connect(); // Foxglove
        self.maintain_connection(ctx);
        self.apply_live_visual_settings();
        self.apply_live_layout_settings();
        if !self.ui.settings_visible {
            // Foxglove: with Settings open these draw in its window (ui_settings.rs).
            self.ui.file_dialog.update(ctx);
            crate::foxglove::art_ui::update(ctx);
        }
        // Foxglove: a profile switch reloads the overlay's settings.
        if crate::foxglove::overlay_profile::switched(ctx, &mut self.profile_seen) {
            let s = crate::foxglove::overlay_profile::effective(crate::settings::Settings::load().unwrap_or_default());
            self.settings.active = s.clone();
            self.settings.draft = s;
        }
        self._tray.update();
        // Foxglove (#391): the tray panel; its "Open Foxglove" opens the settings.
        if crate::foxglove::tray_panel::show(ctx) {
            self.ui.settings_visible = true;
        }

        if let Some(path) = self.ui.file_dialog.take_picked() {
            if let ConnectionDraft::Via { json_path } = &mut self.connect.draft {
                *json_path = path.to_string_lossy().to_string();
            }
            self.connect_from_ui();
        }

        self.sync_mouse_passthrough(ctx, host);
        let drawn = (self.ui.overlay_rect, self.ui.overlay_hovered);
        self.ui.overlay_rect = None;
        self.ui.overlay_hovered = false;
        if let AppConnectionState::Connected { keyboard } = &self.session.connection {
            // Clone the shared keyboard so drawing can mutate app state (the
            // editor) without holding a borrow on `self.session`.
            let keyboard = Arc::clone(keyboard);
            self.draw_overlay_window(ctx, &keyboard, self.overlay_visible());
            if self.editor.target.is_some() {
                let style = self.paint_style(crate::keymap_editor::KEY_UNIT);
                self.editor.draw_window(ctx, &keyboard, &style);
            }
        } else if self.editor.target.is_some() {
            // The connection dropped; close the editor. Unsaved ZMK changes
            // died with the connection, so the dirty flag goes too.
            self.close_editor();
        }

        if (self.ui.overlay_rect, self.ui.overlay_hovered) != drawn {
            // The overlay's hit region follows it on the next frame.
            ctx.request_repaint();
        }

        if self.ui.settings_visible {
            self.draw_settings_window(ctx, host);
        }

        if !self.ui.settings_visible {
            Self::message_window(ctx, "Error", &mut self.ui.settings_error);
            Self::message_window(ctx, "Notice", &mut self.ui.settings_warning);
        }

        self.schedule_overlay_repaint(ctx);
    }
}
