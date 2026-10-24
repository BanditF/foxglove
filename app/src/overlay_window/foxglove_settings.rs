//! Foxglove's own sections in the settings window.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::OverlayApp;
use crate::foxglove::keyboard_actions::{self, Action};
use crate::foxglove::keyboard_settings::{self as ks, RgbSync};
use crate::foxglove::settings_ui::section as titled_group;
use digitalis::protocol::RgbHalf;

const EFFECTS: [&str; 4] = ["Solid", "Breathe", "Spectrum", "Swirl"];

/// The colour being picked (hue, saturation, brightness), seeded from the left half.
static DRAFT_COLOR: Mutex<Option<(u16, u8, u8)>> = Mutex::new(None);
/// A risky action clicked once, waiting for the confirming second click.
static CONFIRMING: Mutex<Option<(String, Instant)>> = Mutex::new(None);
const CONFIRM_WITHIN: Duration = Duration::from_secs(4);

fn swatch(ui: &mut egui::Ui, half: &RgbHalf) {
    let color: egui::Color32 = egui::ecolor::Hsva::new(
        f32::from(half.hue) / 360.0,
        f32::from(half.sat) / 100.0,
        if half.on {
            f32::from(half.brightness.max(15)) / 100.0
        } else {
            0.08
        },
        1.0,
    )
    .into();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 14.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, color);
}

fn half_row(ui: &mut egui::Ui, name: &str, half: Option<&RgbHalf>, age: Option<u8>) {
    ui.label(name);
    match half {
        Some(h) => {
            swatch(ui, h);
            let effect = EFFECTS.get(h.effect as usize).copied().unwrap_or("?");
            let mut text = if h.on {
                format!("on · {effect} · {}% · speed {}", h.brightness, h.speed)
            } else {
                "off".to_string()
            };
            if let Some(age) = age.filter(|a| *a > 3) {
                text.push_str(&format!(" · {age} s ago"));
            }
            ui.label(text);
        }
        None => {
            ui.label("");
            ui.weak("no data yet");
        }
    }
    ui.end_row();
}

/// A button for `action`; disruptive ones need a second click within a few seconds.
fn action_button(ui: &mut egui::Ui, action: &Action) {
    let ctx = ui.ctx().clone();
    let mut confirming = CONFIRMING.lock().unwrap();
    let armed = confirming
        .as_ref()
        .is_some_and(|(label, at)| *label == action.label && at.elapsed() < CONFIRM_WITHIN);
    let text = if armed {
        format!("Click again: {}", action.label)
    } else {
        action.label.clone()
    };
    let mut button = egui::Button::new(text);
    if armed {
        button = button.fill(ui.visuals().warn_fg_color.linear_multiply(0.35));
    }
    let response = ui.add_enabled(!ks::invoking(), button);
    if !response.clicked() {
        return;
    }
    if action.risky && !armed {
        *confirming = Some((action.label.clone(), Instant::now()));
        return;
    }
    *confirming = None;
    ks::invoke(action.label.clone(), action.binding, ctx);
}

impl OverlayApp {
    /// The keyboard itself: settings saved on it, both halves' lights, and everything its
    /// keys can do, all of which hold without Foxglove running.
    pub(super) fn foxglove_keyboard_group(ui: &mut egui::Ui) {
        crate::foxglove::update::group(ui);
        titled_group(ui, "Keyboard", |ui| {
            crate::foxglove::connection::row(ui);
            Self::firmware_row(ui);
            Self::rgb_sync_row(ui);
            ui.add_space(4.0);
            egui::CollapsingHeader::new("Lights")
                .default_open(true)
                .show(ui, Self::lights_section);
            egui::CollapsingHeader::new("Actions")
                .default_open(false)
                .show(ui, Self::actions_section);
            if let Some((label, result)) = ks::last_action() {
                ui.weak(format!("Last: {label} — {result}"));
            }
        });
    }

    /// Both halves' firmware version and build, with a warning when they differ (#132).
    fn firmware_row(ui: &mut egui::Ui) {
        let describe = |b: &digitalis::protocol::BuildInfo| format!("{} · {}", b.version, b.sha);
        match ks::firmware(ui.ctx()) {
            ks::Firmware::Known { left, right } => {
                let text = |r: &Result<digitalis::protocol::BuildInfo, String>| {
                    r.as_ref().map_or_else(|e| format!("unknown ({e})"), describe)
                };
                ui.horizontal_wrapped(|ui| {
                    ui.weak("Firmware");
                    ui.label(format!("left {} / right {}", text(&left), text(&right)));
                });
                if let (Ok(l), Ok(r)) = (&left, &right) {
                    if l.version != r.version || l.sha != r.sha {
                        ui.colored_label(
                            ui.visuals().warn_fg_color,
                            "The halves run different firmware; flash both from the same build.",
                        );
                    }
                }
            }
            ks::Firmware::Reading => {
                ui.weak("Firmware: reading…");
            }
            ks::Firmware::NotReported | ks::Firmware::NoKeyboard | ks::Firmware::Unknown => {}
        }
    }

    fn rgb_sync_row(ui: &mut egui::Ui) {
        let state = ks::rgb_sync();
        if state == RgbSync::Unknown {
            ks::ask_rgb_sync(None, ui.ctx().clone());
        }
        ui.horizontal(|ui| {
            match state {
                RgbSync::Known { on } => {
                    let mut follow = on;
                    if ui
                        .checkbox(&mut follow, "Right half's lights follow the left")
                        .on_hover_text(
                            "On: the left half decides when both halves' underglow is on, \
                             so they always match (it sees key presses on both halves). \
                             Off: each half turns its own lights off and on when idle. \
                             Saved on the keyboard.",
                        )
                        .changed()
                    {
                        ks::ask_rgb_sync(Some(follow), ui.ctx().clone());
                    }
                }
                RgbSync::Unknown | RgbSync::Asking => {
                    ui.spinner();
                    ui.weak("Asking the keyboard…");
                }
                RgbSync::NoKeyboard => {
                    ui.weak("Right half's lights follow the left: no keyboard with Foxglove's firmware found");
                }
                RgbSync::NotSupported => {
                    ui.weak("Right half's lights follow the left: needs newer keyboard firmware");
                }
            }
            if !matches!(state, RgbSync::Unknown | RgbSync::Asking)
                && ui.small_button("↻").on_hover_text("Ask the keyboard again").clicked()
            {
                ks::ask_rgb_sync(None, ui.ctx().clone());
            }
        });
    }

    fn lights_section(ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut follow = crate::foxglove::accent::enabled();
        if ui
            .checkbox(&mut follow, "Follow the Windows accent colour")
            .on_hover_text(
                "The underglow takes the accent colour's hue and saturation whenever it changes                  in Windows, and when the keyboard connects. The brightness stays the board's.",
            )
            .changed()
        {
            crate::foxglove::accent::set_enabled(follow);
            ks::refresh_rgb_state();
        }
        if ui.small_button("Refresh lights").clicked() {
            ks::refresh_rgb_state();
        }
        Self::presets_row(ui);
        let state = ks::rgb_state(&ctx);
        egui::Grid::new("foxglove_lights")
            .num_columns(3)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                half_row(ui, "Left", state.as_ref().map(|s| &s.left), None);
                let right = state.as_ref().and_then(|s| s.right);
                half_row(
                    ui,
                    "Right",
                    right.as_ref().map(|(h, _)| h),
                    right.map(|(_, a)| a),
                );
            });

        if keyboard_actions::underglow_behavior().is_none() {
            ui.weak("Controls need an RGB key (&rgb_ug) somewhere in the keymap.");
            return;
        }
        let busy = ks::invoking();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                if ui.button("On").clicked() {
                    ks::set_lights(true, ctx.clone());
                }
                if ui.button("Off").clicked() {
                    ks::set_lights(false, ctx.clone());
                }
                let current = state.map(|s| s.left.effect as usize).unwrap_or(0);
                egui::ComboBox::from_id_salt("foxglove_effect")
                    .selected_text(EFFECTS.get(current).copied().unwrap_or("Effect"))
                    .show_ui(ui, |ui| {
                        for (i, name) in EFFECTS.iter().enumerate() {
                            if ui.selectable_label(i == current, *name).clicked() {
                                ks::set_effect(i as u8, ctx.clone());
                            }
                        }
                    });
            });

            let mut draft = DRAFT_COLOR.lock().unwrap();
            if draft.is_none() {
                *draft = state.map(|s| (s.left.hue, s.left.sat, s.left.brightness));
            }
            let Some((hue, sat, brightness)) = draft.as_mut() else {
                ui.weak("Waiting for the keyboard's colour…");
                return;
            };
            ui.add(egui::Slider::new(hue, 0..=359).text("hue"));
            ui.add(egui::Slider::new(sat, 0..=100).text("saturation"));
            ui.add(egui::Slider::new(brightness, 0..=100).text("brightness"));
            ui.horizontal(|ui| {
                swatch(
                    ui,
                    &RgbHalf {
                        on: true,
                        hue: *hue,
                        sat: *sat,
                        brightness: *brightness,
                        ..Default::default()
                    },
                );
                if ui.button("Apply colour").clicked() {
                    ks::set_color(*hue, *sat, *brightness, ctx.clone());
                }
            });
        });
    }

    /// The curated presets (#265): a colour plus an effect, and matching right art (#273).
    fn presets_row(ui: &mut egui::Ui) {
        use crate::foxglove::{accent::LightingSettings, lighting_presets as lp};
        let settings = LightingSettings::load();
        let current = settings.preset.as_deref().and_then(lp::find);
        ui.horizontal_wrapped(|ui| {
            ui.label("Preset");
            egui::ComboBox::from_id_salt("foxglove-lighting-preset")
                .selected_text(current.map_or("none", |p| p.name.as_str()))
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "none").clicked() {
                        lp::choose(None);
                    }
                    for p in lp::presets() {
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 3.0, lp::swatch(p));
                            if ui
                                .selectable_label(current.is_some_and(|c| c.id == p.id), &p.name)
                                .on_hover_text(format!("{} ({})", p.description, p.effect))
                                .clicked()
                            {
                                lp::choose(Some(&p.id));
                            }
                        });
                    }
                });
            let mut art = settings.preset_art;
            if ui
                .checkbox(&mut art, "Change the right screen with the preset")
                .on_hover_text("Shows the preset's art on the right screen; your own art comes back when you clear the preset.")
                .changed()
            {
                lp::set_preset_art(art);
            }
        });
        if current.is_some() {
            ui.weak("Picking a preset turns off following the accent colour.");
        }
    }

    fn actions_section(ui: &mut egui::Ui) {
        let actions = keyboard_actions::actions();
        if actions.is_empty() {
            ui.weak("Connect to the keyboard to list what its keys can do.");
            return;
        }
        for chunk in actions.chunk_by(|a, b| a.group == b.group) {
            egui::CollapsingHeader::new(chunk[0].group.title())
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for action in chunk {
                            action_button(ui, action);
                        }
                    });
                });
            ui.add_space(4.0);
        }
        ui.weak("Connection, reset, bootloader and soft-off behaviors are available only on the keyboard.");
    }
}
