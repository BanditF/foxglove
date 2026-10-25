use super::state::AppConnectionState;
use super::OverlayApp;
use crate::settings::{LayerMask, LegendMode, Settings, ThemeColor, ThemeSettings, WindowPosition};
use crate::foxglove::settings_ui::section as titled_group;
use egui::Window;

impl OverlayApp {
    /// The color button that ends every theme row, right-aligned in the row.
    fn theme_color_button(ui: &mut egui::Ui, color: &mut ThemeColor) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut display_color = crate::key_paint::to_egui_color(*color);
            if ui.color_edit_button_srgba(&mut display_color).changed() {
                *color = crate::key_paint::from_egui_color(display_color);
            }
        });
    }

    /// A theme row: a label and the color it stands for.
    fn theme_color_entry(ui: &mut egui::Ui, label: &str, color: &mut ThemeColor) {
        ui.horizontal(|ui| {
            ui.label(label);
            Self::theme_color_button(ui, color);
        });
        ui.add_space(4.0);
    }

    /// A theme table row for the layers in the `layers` bitmask: their Show and Hold
    /// checkboxes, the label, and the color they are painted in.
    fn theme_layer_row(
        ui: &mut egui::Ui,
        label: &str,
        color: &mut ThemeColor,
        visible_layers: &mut LayerMask,
        held_layers: &mut LayerMask,
        layers: u32,
    ) {
        let mut shown = visible_layers.contains_any(layers);
        if ui
            .add(egui::Checkbox::without_text(&mut shown))
            .on_hover_text("Show the overlay while these layers are active")
            .changed()
        {
            visible_layers.set(layers, shown);
        }
        let mut held = held_layers.contains_any(layers);
        if ui
            .add_enabled(shown, egui::Checkbox::without_text(&mut held))
            .on_hover_text(
                "Keep the overlay up for as long as these layers are active. Untick for                  toggled layers (like a game layer): the overlay then shows when they turn on                  and fades after the display duration.",
            )
            .changed()
        {
            held_layers.set(layers, held);
        }
        crate::foxglove::layer_access::label(ui, label, layers);
        Self::theme_color_button(ui, color);
        ui.end_row();
    }

    fn timeout_to_ui_value(timeout: i64) -> u32 {
        if timeout < 0 {
            15_000
        } else {
            (timeout as u32).min(14_999)
        }
    }

    fn ui_value_to_timeout(value: u32) -> i64 {
        if value >= 15_000 {
            -1
        } else {
            value as i64
        }
    }

    pub(super) fn draw_settings_window(
        &mut self,
        ctx: &egui::Context,
        host: &mut dyn crate::platform::OverlayHost,
    ) {
        let mut open = self.ui.settings_visible;
        let reconnecting = matches!(
            self.session.connection,
            AppConnectionState::Reconnecting { .. }
        );
        // Keep the device/protocol pickers locked while connected or reconnecting.
        let connection_locked =
            !matches!(self.session.connection, AppConnectionState::Disconnected);
        let selected_device = self
            .connect
            .selected_device_index
            .and_then(|i| self.connect.available_devices.get(i))
            .cloned();
        let selected_device_text = selected_device
            .as_ref()
            .map(|d| d.display_name())
            .unwrap_or_else(|| "Select device...".to_string());

        let settings_window_size = egui::vec2(1040.0, 660.0); // Foxglove: room for the sidebar (#385)
        let settings_window_pos = ctx.viewport_rect().center() - settings_window_size * 0.5;

        Window::new("Foxglove Settings")
            .open(&mut open)
            .default_size(settings_window_size)
            .default_pos(settings_window_pos)
            .collapsible(false)
            .resizable(true)
            .min_size(egui::vec2(720.0, 420.0))
            .frame(crate::foxglove::shell::window_frame(ctx))
            // Foxglove (#385): the sidebar, status strip and pages; each page scrolls itself.
            .show(ctx, |ui| crate::foxglove::shell::frame(ui, |ui| {
                titled_group(ui, "Connection", |ui| {
                    let control_spacing = ui.spacing().item_spacing.x;
                    const RIGHT_COLUMN_WIDTH: f32 = 100.0;

                    egui::Grid::new("connection_grid")
                        .num_columns(2)
                        .striped(true)
                        .spacing([20.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Device");
                            ui.add_enabled_ui(!connection_locked, |ui| {
                                ui.horizontal(|ui| {
                                    let combo_width = (ui.available_width()
                                        - RIGHT_COLUMN_WIDTH
                                        - control_spacing)
                                        .max(120.0);
                                    egui::ComboBox::from_id_salt("device_combo")
                                        .width(combo_width)
                                        .selected_text(selected_device_text.clone())
                                        .show_ui(ui, |ui| {
                                            for idx in 0..self.connect.available_devices.len() {
                                                let device = &self.connect.available_devices[idx];
                                                let selected =
                                                    self.connect.selected_device_index == Some(idx);
                                                if ui
                                                    .selectable_label(
                                                        selected,
                                                        device.display_name(),
                                                    )
                                                    .clicked()
                                                {
                                                    self.select_device(idx);
                                                }
                                            }
                                            if self.connect.available_devices.is_empty() {
                                                ui.weak("No devices found");
                                            }
                                        });

                                    ui.allocate_ui_with_layout(
                                        egui::vec2(RIGHT_COLUMN_WIDTH, 20.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let connect_in_progress =
                                                self.connect.pending_connect.is_some();
                                            let can_connect = !connection_locked
                                                && !connect_in_progress
                                                && self.connect.selected_device_index.is_some();
                                            let button_label = if reconnecting {
                                                "Reconnecting..."
                                            } else if connect_in_progress {
                                                "Connecting..."
                                            } else {
                                                "Connect"
                                            };
                                            ui.add_enabled_ui(can_connect, |ui| {
                                                if ui
                                                    .add_sized(
                                                        [RIGHT_COLUMN_WIDTH, 20.0],
                                                        egui::Button::new(button_label),
                                                    )
                                                    .clicked()
                                                {
                                                    self.connect_from_ui();
                                                }
                                            });
                                        },
                                    );
                                });
                            });
                            ui.end_row();

                            ui.label("Layout");
                            ui.horizontal(|ui| {
                                let layout_enabled = !self.session.layout_names.is_empty();
                                let layout_width =
                                    (ui.available_width() - RIGHT_COLUMN_WIDTH - control_spacing)
                                        .max(120.0);
                                ui.add_enabled_ui(layout_enabled, |ui| {
                                    let selected_text = if self.session.layout_names.is_empty() {
                                        "Connect to device first".to_string()
                                    } else {
                                        self.session.draft_layout_name.clone()
                                    };
                                    egui::ComboBox::from_id_salt("layout_combo")
                                        .width(layout_width)
                                        .selected_text(selected_text)
                                        .show_ui(ui, |ui| {
                                            for name in &self.session.layout_names {
                                                ui.selectable_value(
                                                    &mut self.session.draft_layout_name,
                                                    name.clone(),
                                                    name,
                                                );
                                            }
                                        });
                                });
                                ui.allocate_space(egui::vec2(RIGHT_COLUMN_WIDTH, 20.0));
                            });
                            ui.end_row();
                        });
                });

                titled_group(ui, "Overlay Appearance", |ui| {
                    egui::Grid::new("appearance_grid")
                        .num_columns(2)
                        .striped(true)
                        .spacing([20.0, 10.0])
                        .show(ui, |ui| {
                            let monitors = crate::monitors::list();
                            if monitors.len() > 1 {
                                let current =
                                    crate::monitors::find(&monitors, &self.settings.draft.monitor);
                                ui.label("Display");
                                egui::ComboBox::from_id_salt("monitor_combo")
                                    .width(ui.available_width())
                                    .selected_text(current.map(|m| m.label).unwrap_or_default())
                                    .show_ui(ui, |ui| {
                                        for monitor in &monitors {
                                            ui.selectable_value(
                                                &mut self.settings.draft.monitor,
                                                monitor.id.clone(),
                                                &monitor.label,
                                            );
                                        }
                                    });
                                ui.end_row();
                            }

                            ui.label("Alignment");
                            egui::ComboBox::from_id_salt("position_combo")
                                .width(ui.available_width())
                                .selected_text(self.settings.draft.position.to_string())
                                .show_ui(ui, |ui| {
                                    for pos in [
                                        WindowPosition::TopLeft,
                                        WindowPosition::TopRight,
                                        WindowPosition::BottomLeft,
                                        WindowPosition::BottomRight,
                                        WindowPosition::Top,
                                        WindowPosition::Bottom,
                                    ] {
                                        ui.selectable_value(
                                            &mut self.settings.draft.position,
                                            pos,
                                            pos.to_string(),
                                        );
                                    }
                                });
                            ui.end_row();

                            ui.label("Display duration");
                            let mut timeout_ui =
                                Self::timeout_to_ui_value(self.settings.draft.timeout);
                            ui.add_sized(
                                ui.available_size(),
                                egui::DragValue::new(&mut timeout_ui)
                                    .speed(50)
                                    .range(0..=15_000)
                                    .custom_formatter(|value, _range| {
                                        if value >= 15_000.0 {
                                            "∞".to_string()
                                        } else {
                                            format!("{} ms", value as i64)
                                        }
                                    }),
                            );
                            self.settings.draft.timeout = Self::ui_value_to_timeout(timeout_ui);
                            ui.end_row();

                            ui.label("Activation delay");
                            ui.add_sized(
                                ui.available_size(),
                                egui::DragValue::new(&mut self.settings.draft.activation_delay)
                                    .speed(10)
                                    .range(0..=Settings::MAX_ACTIVATION_DELAY_MS)
                                    .suffix(" ms"),
                            )
                            .on_hover_text(
                                "How long a layer has to be held before the overlay appears",
                            );
                            ui.end_row();

                            ui.label("Distance from screen edge");
                            ui.add_sized(
                                ui.available_size(),
                                egui::DragValue::new(&mut self.settings.draft.margin)
                                    .speed(1)
                                    .suffix(" px"),
                            );
                            ui.end_row();

                            ui.label("Key unit size");
                            ui.add_sized(
                                ui.available_size(),
                                egui::DragValue::new(&mut self.settings.draft.size)
                                    .speed(1)
                                    .range(20..=1000)
                                    .suffix(" px"),
                            );
                            ui.end_row();

                            ui.label("Key label font scale");
                            ui.add_sized(
                                ui.available_size(),
                                egui::DragValue::new(&mut self.settings.draft.font_size_multiplier)
                                    .speed(0.01)
                                    .range(0.5..=1.5)
                                    .suffix(" x"),
                            );
                            ui.end_row();

                            ui.label("Auto-fit long labels");
                            ui.checkbox(
                                &mut self.settings.draft.auto_fit_before_ellipsis,
                                "Fit long labels to available space",
                            );
                            ui.end_row();

                            ui.label("Legend mode");
                            egui::ComboBox::from_id_salt("legend_mode_combo")
                                .width(ui.available_width())
                                .selected_text(self.settings.draft.legend_mode.to_string())
                                .show_ui(ui, |ui| {
                                    for mode in [
                                        LegendMode::Stacked,
                                        LegendMode::Single,
                                        LegendMode::SingleLive,
                                    ] {
                                        ui.selectable_value(
                                            &mut self.settings.draft.legend_mode,
                                            mode,
                                            mode.to_string(),
                                        );
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "Stacked: Base+Shifted shown together (default). Single: \
                                     only what a plain tap produces. Single + live preview: \
                                     also swaps to the Shift/RAlt result while that modifier \
                                     is physically held.",
                                );
                            ui.end_row();
                        });
                });

                titled_group(ui, "Theme", |ui| {
                    const LAYERS: usize = ThemeSettings::OTHER_LAYERS as usize;
                    const HALF: usize = LAYERS / 2;

                    // The keyboard's layer names (ZMK), remembered from the last connection,
                    // else "Layer n".
                    let names: Vec<Option<String>> = match &self.session.connection {
                        AppConnectionState::Connected { keyboard } => {
                            let names: Vec<_> =
                                keyboard.layer_infos().into_iter().map(|l| l.name).collect();
                            crate::foxglove::layer_names::remember(&names);
                            names
                        }
                        _ => crate::foxglove::layer_names::remembered(),
                    };
                    let layer_label = |layer: usize| {
                        names
                            .get(layer)
                            .cloned()
                            .flatten()
                            .filter(|n| !n.trim().is_empty())
                            .unwrap_or_else(|| format!("Layer {layer}"))
                    };

                    // Foxglove (#390): the window theme is in "Theme and updates".
                    let draft = &mut self.settings.draft;
                    Self::theme_color_entry(ui, "Font color", &mut draft.theme.font_color);

                    let mut layer_table = |ui: &mut egui::Ui, id: &str, layers: &[usize]| {
                        egui::Grid::new(id)
                            .num_columns(4)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.weak("Show");
                                ui.weak("Hold");
                                ui.weak("Layer");
                                ui.label("");
                                ui.end_row();
                                for &layer in layers {
                                    let (label, mask) = if layer == LAYERS {
                                        ("Other layers".to_string(), !0 << layer)
                                    } else {
                                        (layer_label(layer), 1 << layer)
                                    };
                                    Self::theme_layer_row(
                                        ui,
                                        &label,
                                        &mut draft.theme.layer_colors[layer],
                                        &mut draft.visible_layers,
                                        &mut draft.held_layers,
                                        mask,
                                    );
                                }
                            });
                    };
                    // Foxglove (#390): one table when the card is too narrow for two.
                    if ui.available_width() < 560.0 {
                        let all: Vec<usize> = (0..=LAYERS).collect();
                        layer_table(ui, "theme_layers_all", &all);
                        return;
                    }
                    ui.columns(2, |columns| {
                        let left: Vec<usize> = (0..HALF).collect();
                        let right: Vec<usize> = (HALF..=LAYERS).collect();
                        layer_table(&mut columns[0], "theme_layers_left", &left);
                        layer_table(&mut columns[1], "theme_layers_right", &right);
                    });
                });

                crate::foxglove::profiles_ui::picker(ui); // Foxglove
                Self::foxglove_keyboard_group(ui); // Foxglove
                // Foxglove (#388): the Screens page draws at its page's top, from the shell.
                crate::foxglove::via_ui::group(ui);
                crate::foxglove::plugins_ui::group(ui);

                // Foxglove (#384): links, KeyPeek credit and font credits, stacked.
                crate::foxglove::settings_ui::footer(ui);
            }));

        if self.ui.settings_visible && !open {
            self.ui.settings_visible = false;
            self.persist_settings();
            if !self.session.ever_connected {
                self.request_close_editor();
                host.request_close();
            }
        }
    }
}
