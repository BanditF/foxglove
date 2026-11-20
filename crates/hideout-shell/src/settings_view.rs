// SPDX-License-Identifier: MIT
//! The hub's built-in **Settings** page (#991, the #1087 design review): the page the plans
//! name beside Modules (`hideout-tabs-ux.md`), shown in safe mode too. Sections in order:
//! **Privacy** (what the app watch keeps, and **Forget seen apps**), then **Start with Windows**
//! (H5.2, only when the hub gives it a row), then **Developer** (R5, only when the hub gives it
//! a control), then **About** (the version). The hub binary owns the page and its action; this draws it.

use crate::dev_settings::{self, DevClick, DevView};
use crate::startup_switch::{self, Click, Row};
use crate::theme::{Rgb, Tokens};
use eframe::egui::{self, RichText};

pub const ID: &str = "hub.settings";
pub const TITLE: &str = "Settings";
/// A published icon name (the gear).
pub const ICON: &str = "gear";

pub const PRIVACY: &str = "Privacy";
pub const ABOUT: &str = "About";
/// The privacy line (the #1040 review, without examples: the hub names no app, #1087).
pub const PRIVACY_LINE: &str = "Hideout notices when apps that your modules work with start or \
quit, so their tiles show up. It keeps a list of those apps on this PC only.";
pub const OPEN_LOGS: &str = "Open log folder";
pub const FORGET: &str = "Forget seen apps";
pub const FORGOTTEN: &str =
    "Hideout forgot the apps it has seen. Their tiles hide until the apps start again.";

/// "Hideout 0.3.0".
pub fn version_line(version: &str) -> String {
    format!("{} {version}", crate::PRODUCT)
}

fn c(x: Rgb) -> egui::Color32 {
    egui::Color32::from_rgb(x.0, x.1, x.2)
}

fn heading(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong().color(c(t.text)));
    ui.add_space(4.0);
}

/// Draws the page without a Start with Windows section.
pub fn settings_ui(ui: &mut egui::Ui, t: &Tokens, version: &str, forgotten: bool) -> bool {
    settings_ui_with(ui, t, version, forgotten, None).forget
}

/// Draws the page with a Start with Windows row and no Developer section.
pub fn settings_ui_with(
    ui: &mut egui::Ui,
    t: &Tokens,
    version: &str,
    forgotten: bool,
    startup: Option<&Row>,
) -> Actions {
    settings_ui_full(ui, t, version, forgotten, startup, None)
}

/// What a frame of the page did.
#[derive(Default)]
pub struct Actions {
    pub forget: bool,
    /// Open log folder was pressed.
    pub logs: bool,
    pub startup: Option<Click>,
    /// A Developer section click (R5).
    pub dev: Option<DevClick>,
}

fn startup_section(ui: &mut egui::Ui, t: &Tokens, row: &Row) -> Option<Click> {
    let mut click = None;
    heading(ui, t, startup_switch::HEADING);
    ui.add(egui::Label::new(RichText::new(&row.status).color(c(t.text))).wrap());
    ui.add_space(8.0);
    if let Some(replaces) = row.dialog {
        // The confirmation: the only way to Turn on. Not now is the first button, so Enter on
        // the opening focus never turns it on by itself.
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, c(t.text_muted)))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(startup_switch::DIALOG_TITLE)
                        .strong()
                        .color(c(t.text)),
                );
                ui.add(
                    egui::Label::new(RichText::new(startup_switch::DIALOG_BODY).color(c(t.text)))
                        .wrap(),
                );
                if row.apps {
                    ui.add(
                        egui::Label::new(
                            RichText::new(startup_switch::dialog_apps(
                                &startup_switch::join_names(&row.app_names),
                                row.app_names.len() <= 1,
                            ))
                            .color(c(t.text)),
                        )
                        .wrap(),
                    );
                }
                if replaces {
                    ui.label(RichText::new(startup_switch::DIALOG_REPLACES).color(c(t.text)));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // Not now is first and takes the focus when nothing else has it.
                    let not_now = ui.button(startup_switch::DIALOG_CANCEL);
                    if ui.memory(|m| m.focused().is_none()) {
                        not_now.request_focus();
                    }
                    if not_now.clicked() {
                        click = Some(Click::Cancel);
                    }
                    if ui.button(startup_switch::DIALOG_CONFIRM).clicked() {
                        click = Some(Click::Confirm);
                    }
                });
            });
        ui.add_space(8.0);
    }
    ui.horizontal_wrapped(|ui| {
        if row.turn_on && ui.button(startup_switch::TURN_ON).clicked() {
            click = Some(Click::Ask);
        }
        if row.turn_off && ui.button(startup_switch::TURN_OFF).clicked() {
            click = Some(Click::TurnOff);
        }
        if row.remove && ui.button(startup_switch::REMOVE).clicked() {
            click = Some(Click::Remove);
        }
        if let Some(m) = &row.message {
            ui.label(RichText::new(m).color(c(t.text_muted)));
        }
    });
    ui.add_space(20.0);
    click
}

/// Where focus goes on the frame after the Remove confirm closes: the row's own **Remove...**
/// first (Cancel, Esc, or a save that failed and left the row), then the fallback row (the next
/// one, else the previous one), else **Add a folder...**.
#[derive(Clone, Default)]
struct DevFocus {
    primary: String,
    fallback: Option<String>,
}

/// What the Developer section keeps between frames (egui temp memory, so the hub holds none).
#[derive(Clone, Default)]
struct DevUi {
    /// The folder the Remove confirm is open for.
    ask: Option<String>,
    /// The confirm was just opened: Cancel takes the focus once.
    fresh: bool,
    focus: Option<DevFocus>,
}

/// The Remove confirm (DF1). Cancel is first, filled and focused; Esc and a click outside are
/// Cancel. Returns `Some(true)` for Remove folder, `Some(false)` for Cancel.
fn remove_confirm(
    ctx: &egui::Context,
    t: &Tokens,
    path: &str,
    apps: &[String],
    fresh: bool,
) -> Option<bool> {
    let body = dev_settings::confirm_body(apps);
    let mut answer = None;
    let mut ids: Vec<egui::Id> = Vec::new();
    let modal = egui::Modal::new(egui::Id::new("hideout.dev.remove-confirm")).show(ctx, |ui| {
        ui.set_width(440.0_f32.min(ui.available_width()));
        ui.label(
            RichText::new(dev_settings::CONFIRM_TITLE)
                .heading()
                .color(c(t.text)),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(path).strong().color(c(t.text))).wrap());
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(&body).color(c(t.text))).wrap());
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            let cancel = ui.add(
                egui::Button::new(
                    RichText::new(dev_settings::CONFIRM_CANCEL).color(c(t.on_accent)),
                )
                .fill(c(t.accent)),
            );
            if fresh {
                cancel.request_focus();
            }
            if cancel.clicked() {
                answer = Some(false);
            }
            let go = ui.add(
                egui::Button::new(RichText::new(dev_settings::CONFIRM_OK).color(c(t.danger)))
                    .stroke(egui::Stroke::new(1.0_f32, c(t.danger))),
            );
            if go.clicked() {
                answer = Some(true);
            }
            ids.push(cancel.id);
            ids.push(go.id);
        });
    });
    // Tab stays inside the dialog.
    let (tab, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.modifiers.shift));
    if tab
        && !ctx
            .memory(|m| m.focused())
            .is_some_and(|f| ids.contains(&f))
    {
        let to = if shift { ids.last() } else { ids.first() };
        if let Some(to) = to {
            ctx.memory_mut(|m| m.request_focus(*to));
        }
    }
    ctx.accesskit_node_builder(modal.response.id, |n| {
        n.set_role(egui::accesskit::Role::Dialog);
        n.set_label(dev_settings::CONFIRM_TITLE);
        n.set_description(format!("{path}. {body}"));
    });
    if modal.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = answer.or(Some(false));
    }
    answer
}

/// Tests: opens the Remove confirm for `path` the way a click on its **Remove...** does.
#[cfg(test)]
pub(crate) fn open_remove_confirm(ctx: &egui::Context, path: &str) {
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new("hub.dev.ui"),
            DevUi {
                ask: Some(path.into()),
                fresh: true,
                focus: None,
            },
        )
    });
}

/// The Developer section (R5). Dev mode off: one line and nothing to click.
fn dev_section(ui: &mut egui::Ui, t: &Tokens, v: &DevView) -> Option<DevClick> {
    let mut click = None;
    if !v.on && v.folders.is_empty() {
        // Developer mode is off and nothing is listed: the page stays as it ships.
        return None;
    }
    heading(ui, t, dev_settings::HEADING);
    if !v.on {
        ui.add(egui::Label::new(RichText::new(dev_settings::MODE_OFF).color(c(t.text))).wrap());
        ui.add_space(6.0);
        for row in &v.folders {
            ui.add(egui::Label::new(RichText::new(&row.path).color(c(t.text_muted))).wrap());
        }
        ui.add_space(20.0);
        return None;
    }
    let sid = egui::Id::new("hub.dev.ui");
    let mut st: DevUi = ui.data(|d| d.get_temp(sid)).unwrap_or_default();
    // Taken now, applied at the end of this frame: it was set by a click on the frame before.
    let pending_focus = st.focus.take();
    // The folder went (removed elsewhere, or the view changed): its confirm goes with it.
    if st
        .ask
        .as_ref()
        .is_some_and(|p| !v.folders.iter().any(|r| &r.path == p))
    {
        st.ask = None;
    }
    let mut remove_ids: Vec<(String, egui::Id)> = Vec::new();
    ui.label(
        RichText::new(dev_settings::FOLDERS_HEADING)
            .strong()
            .color(c(t.text)),
    );
    ui.add(egui::Label::new(RichText::new(dev_settings::BODY).color(c(t.text))).wrap());
    ui.add_space(8.0);
    if v.folders.is_empty() {
        ui.label(RichText::new(dev_settings::EMPTY).color(c(t.text_muted)));
    }
    for row in &v.folders {
        egui::Frame::new()
            .fill(c(t.surface))
            .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(&row.path).strong().color(c(t.text))).wrap());
                match row.problem {
                    Some(p) => {
                        ui.add(
                            egui::Label::new(RichText::new(p.row_line()).color(c(t.danger))).wrap(),
                        );
                    }
                    None => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(dev_settings::apps_line(&row.apps))
                                    .color(c(t.text_muted)),
                            )
                            .wrap(),
                        );
                    }
                }
                ui.add_space(4.0);
                let b = ui.button(dev_settings::REMOVE_BUTTON);
                let name = dev_settings::remove_name(&row.path);
                ui.ctx().accesskit_node_builder(b.id, |n| {
                    n.set_label(name);
                });
                remove_ids.push((row.path.clone(), b.id));
                // Only asks: nothing is sent until Remove folder.
                if b.clicked() {
                    st.ask = Some(row.path.clone());
                    st.fresh = true;
                }
            });
        ui.add_space(6.0);
    }
    ui.add_space(2.0);
    let mut add_id = None;
    ui.horizontal_wrapped(|ui| {
        let add = ui.button(dev_settings::ADD);
        add_id = Some(add.id);
        if add.clicked() {
            click = Some(DevClick::Add);
        }
        if let Some(n) = &v.note {
            let colour = if n.error { c(t.danger) } else { c(t.text) };
            let l = ui.add(egui::Label::new(RichText::new(&n.text).color(colour)).wrap());
            // Announced politely, like the other pages' notes.
            ui.ctx().accesskit_node_builder(l.id, |x| {
                x.set_live(egui::accesskit::Live::Polite);
            });
        }
    });
    ui.add_space(10.0);
    let mut restart = v.restart;
    // The empty box must read against the page (3:1): the border at full strength.
    let changed = ui
        .scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke =
                egui::Stroke::new(1.5_f32, c(t.text_muted));
            ui.checkbox(&mut restart, dev_settings::RESTART_SWITCH)
                .changed()
        })
        .inner;
    if changed {
        click = Some(DevClick::Restart(restart));
    }
    ui.label(RichText::new(dev_settings::RESTART_SUB).color(c(t.text_muted)));
    ui.add_space(8.0);
    ui.add(egui::Label::new(RichText::new(dev_settings::FOOTNOTE).color(c(t.text_muted))).wrap());
    ui.add_space(20.0);
    if let Some(path) = st.ask.clone() {
        let apps = v
            .folders
            .iter()
            .find(|r| r.path == path)
            .map(|r| r.apps.clone())
            .unwrap_or_default();
        let fresh = std::mem::take(&mut st.fresh);
        match remove_confirm(ui.ctx(), t, &path, &apps, fresh) {
            Some(true) => {
                let at = v.folders.iter().position(|r| r.path == path);
                let fallback = at.and_then(|i| {
                    v.folders
                        .get(i + 1)
                        .or_else(|| i.checked_sub(1).and_then(|j| v.folders.get(j)))
                        .map(|r| r.path.clone())
                });
                st.focus = Some(DevFocus {
                    primary: path.clone(),
                    fallback,
                });
                st.ask = None;
                click = Some(DevClick::Remove(path));
                ui.ctx().request_repaint();
            }
            Some(false) => {
                st.focus = Some(DevFocus {
                    primary: path,
                    fallback: None,
                });
                st.ask = None;
                ui.ctx().request_repaint();
            }
            None => {}
        }
    }
    if let Some(f) = pending_focus {
        let row = |p: &str| remove_ids.iter().find(|(x, _)| x == p).map(|(_, id)| *id);
        let to = row(&f.primary)
            .or_else(|| f.fallback.as_deref().and_then(row))
            .or(add_id);
        if let Some(to) = to {
            ui.ctx().memory_mut(|m| m.request_focus(to));
        }
    }
    ui.data_mut(|d| d.insert_temp(sid, st));
    click
}

/// Draws the page. `forgotten`: Forget seen apps was pressed (its line shows beside the button,
/// which stays, so the focus stays on it). `startup`: the Start with Windows row, when the hub
/// has one. `dev`: the Developer section, when the hub has one.
pub fn settings_ui_full(
    ui: &mut egui::Ui,
    t: &Tokens,
    version: &str,
    forgotten: bool,
    startup: Option<&Row>,
    dev: Option<&DevView>,
) -> Actions {
    heading(ui, t, PRIVACY);
    ui.add(egui::Label::new(RichText::new(PRIVACY_LINE).color(c(t.text))).wrap());
    ui.add_space(8.0);
    let pressed = ui
        .horizontal_wrapped(|ui| {
            let pressed = ui.button(FORGET).clicked();
            if forgotten || pressed {
                ui.label(RichText::new(FORGOTTEN).color(c(t.text_muted)));
            }
            pressed
        })
        .inner;
    ui.add_space(20.0);
    let startup = startup.and_then(|row| startup_section(ui, t, row));
    let dev = dev.and_then(|v| dev_section(ui, t, v));
    heading(ui, t, ABOUT);
    ui.label(RichText::new(version_line(version)).color(c(t.text)));
    ui.add_space(8.0);
    let logs = ui.button(OPEN_LOGS).clicked();
    Actions {
        forget: pressed,
        logs,
        startup,
        dev,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_reads_as_the_review_says() {
        assert_eq!((ID, TITLE, ICON), ("hub.settings", "Settings", "gear"));
        assert!(hideout_contract::panel::ICONS.contains(&ICON));
        assert_eq!(version_line("0.3.0"), "Hideout 0.3.0");
        assert_eq!(
            PRIVACY_LINE,
            "Hideout notices when apps that your modules work with start or quit, so their \
             tiles show up. It keeps a list of those apps on this PC only."
        );
    }
}
