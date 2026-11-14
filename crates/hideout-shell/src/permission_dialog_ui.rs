// SPDX-License-Identifier: MIT
//! Drawing for the module permission dialog (#1178), from the words and rules of
//! `permission_dialog`: the dialog, the header lines and the Permissions section. Drawn by the
//! hub, never by a module panel. Same frame as the keyboard grant (`keyboard_grant_ui`): a modal,
//! Not now focused and carrying the accent fill, Esc / close / Enter on the focused Not now
//! answer Not now, Tab stays inside.

use crate::egui;
use crate::keyboard_grant::{DIALOG_FOOTER, DIALOG_ITEMS};
use crate::keyboard_grant_ui::rgb as c;
use crate::permission_dialog::{
    keyboard_line, All, Group, ItemWord, PermAnswer, PermDialog, PermHeader, Section, CHANGE,
    NOT_NOW, TURN_ALL_OFF, TURN_OFF, UNDERSTAND,
};
use crate::theme::Tokens;

fn group_colour(t: &Tokens, g: Group) -> egui::Color32 {
    c(match g {
        Group::Sensitive | Group::CantBeAllowed => t.danger,
        Group::MakeChanges => t.attention,
        Group::AnswerQuestions | Group::ShareStatus => t.text_muted,
    })
}

/// The dialog. `first`: the first frame, when Not now takes the focus.
pub fn perm_dialog_ui(
    ctx: &egui::Context,
    t: &Tokens,
    d: &mut PermDialog,
    first: bool,
) -> Option<PermAnswer> {
    perm_dialog_ids(ctx, t, d, first).0
}

/// [`perm_dialog_ui`] and the focusable widgets in Tab order (for the focus-trap test); the last
/// two are Allow and Not now.
pub(crate) fn perm_dialog_ids(
    ctx: &egui::Context,
    t: &Tokens,
    d: &mut PermDialog,
    first: bool,
) -> (Option<PermAnswer>, Vec<egui::Id>) {
    let mut answer = None;
    let mut ids: Vec<egui::Id> = Vec::new();
    let screen_h = ctx.content_rect().height();
    let title = d.title();
    let modal = egui::Modal::new(egui::Id::new("hideout.permission-dialog")).show(ctx, |ui| {
        ui.set_width(620.0);
        ui.label(egui::RichText::new(&title).heading().color(c(t.text)));
        ui.add_space(6.0);
        for (i, line) in d.reason_lines().iter().enumerate() {
            let rt = egui::RichText::new(line).color(c(t.attention));
            ui.label(if i == 0 { rt.strong() } else { rt });
        }
        if d.reason.is_some() {
            ui.add_space(6.0);
        }
        ui.label(d.intro());
        ui.add_space(6.0);
        // All N: tri-state.
        let state = d.all_state();
        let mut all_on = state == All::All;
        let all = ui.add(
            egui::Checkbox::new(
                &mut all_on,
                egui::RichText::new(d.all_label()).strong().color(c(t.text)),
            )
            .indeterminate(state == All::Some),
        );
        if all.changed() {
            d.toggle_all();
        }
        ids.push(all.id);
        ui.add_space(4.0);
        // Scrolls inside when the list is taller than the window allows; the buttons stay.
        egui::ScrollArea::vertical()
            .max_height((screen_h * 0.8 - 250.0).max(140.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for g in Group::ALL {
                    let n = d.group(g).count();
                    if n == 0 {
                        continue;
                    }
                    let said =
                        format!("{}, {n} item{}", g.heading(), if n == 1 { "" } else { "s" });
                    let grp = ui.vertical(|ui| {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(g.heading())
                                .strong()
                                .small()
                                .color(group_colour(t, g)),
                        );
                        let name = d.name.clone();
                        for item in d.items.iter_mut().filter(|i| i.group == g) {
                            if !item.allowable {
                                ui.label(egui::RichText::new(&item.line).color(c(t.danger)));
                                if let Some(r) = &item.reason {
                                    ui.label(
                                        egui::RichText::new(r.replace("this module", &name))
                                            .small()
                                            .color(c(t.text_muted)),
                                    );
                                }
                                continue;
                            }
                            let r = ui.checkbox(&mut item.checked, &item.line);
                            let said = item.said();
                            ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(said));
                            if r.changed() && !item.checked {
                                item.confirmed = false;
                            }
                            ids.push(r.id);
                            if let Some(s) = item.sub_line() {
                                ui.horizontal(|ui| {
                                    ui.add_space(24.0);
                                    ui.label(egui::RichText::new(s).small().color(c(t.text_muted)));
                                });
                            }
                            if item.sensitive && item.checked {
                                ui.horizontal(|ui| {
                                    ui.add_space(24.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new(item.confirm_line(&name))
                                                .color(c(t.attention)),
                                        );
                                        let u = ui.checkbox(&mut item.confirmed, UNDERSTAND);
                                        ids.push(u.id);
                                    });
                                });
                            }
                        }
                    });
                    ui.ctx().accesskit_node_builder(grp.response.id, |n| {
                        n.set_role(egui::accesskit::Role::Group);
                        n.set_label(said);
                    });
                }
                if let Some(k) = d.keyboard.as_mut() {
                    ui.add_space(10.0);
                    let word = keyboard_line(&d.name, k.on);
                    let sign = if k.expanded { "\u{2212}" } else { "+" };
                    let b = ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!("{sign}  {word}")).color(c(t.accent)),
                        )
                        .frame(false),
                    );
                    if b.clicked() {
                        k.expanded = !k.expanded;
                    }
                    ids.push(b.id);
                    if k.expanded {
                        ui.horizontal(|ui| {
                            ui.add_space(24.0);
                            ui.vertical(|ui| {
                                for i in DIALOG_ITEMS {
                                    ui.label(format!("\u{2022} {i}"));
                                }
                                for f in DIALOG_FOOTER {
                                    ui.label(egui::RichText::new(f).color(c(t.text_muted)));
                                }
                                let on = ui.checkbox(&mut k.on, "Allow this too");
                                ids.push(on.id);
                            });
                        });
                    }
                }
            });
        ui.add_space(8.0);
        ui.separator();
        let [b0, b1, b2] = d.binding_lines();
        ui.label(b0);
        if !d.path.is_empty() {
            ui.add(
                egui::Label::new(egui::RichText::new(&d.path).monospace())
                    .wrap()
                    .selectable(true),
            );
        }
        ui.label(egui::RichText::new(b1).small().color(c(t.text_muted)));
        ui.label(egui::RichText::new(b2).small().color(c(t.text_muted)));
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("List code {}", d.code))
                    .small()
                    .color(c(t.text_muted)),
            )
            .selectable(true),
        );
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            let l = ui.label(egui::RichText::new(e).color(c(t.attention)));
            ui.ctx().accesskit_node_builder(l.id, |n| {
                n.set_live(egui::accesskit::Live::Polite);
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let allow = ui.add(
                egui::Button::new(egui::RichText::new(d.allow_label()).color(c(t.accent)))
                    .stroke(egui::Stroke::new(1.0_f32, c(t.accent))),
            );
            if allow.clicked() {
                answer = Some(PermAnswer::Allow(d.allowed()));
            }
            let not_now = ui.add(
                egui::Button::new(egui::RichText::new(NOT_NOW).color(c(t.on_accent)))
                    .fill(c(t.accent)),
            );
            if first {
                not_now.request_focus();
            }
            if not_now.clicked() {
                answer = Some(PermAnswer::NotNow);
            }
            ids.push(allow.id);
            ids.push(not_now.id);
        });
    });
    // Tab wraps inside: out of the dialog it goes to the first widget (the last with Shift).
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
    let said = d.said();
    ctx.accesskit_node_builder(modal.response.id, |n| {
        n.set_role(egui::accesskit::Role::Dialog);
        n.set_label(title);
        n.set_description(said);
    });
    if modal.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = answer.or(Some(PermAnswer::NotNow));
    }
    (answer, ids)
}

/// A header line over the page. `Some((true, id))`: Review…; `Some((false, id))`: the dismiss
/// button (Not now, or OK for the command-line line). A polite live region.
pub fn perm_header_ui(ui: &mut egui::Ui, t: &Tokens, h: &PermHeader) -> Option<(bool, egui::Id)> {
    let mut out = None;
    egui::Frame::new()
        .fill(c(t.surface))
        .stroke(egui::Stroke::new(1.0_f32, c(t.border)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let head = ui.label(egui::RichText::new(&h.title).strong().color(c(t.text)));
            let said = format!("{}. {}", h.title, h.body);
            ui.ctx().accesskit_node_builder(head.id, |n| {
                n.set_live(egui::accesskit::Live::Polite);
                n.set_label(said);
            });
            ui.label(egui::RichText::new(&h.body).color(c(t.text_muted)));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let review = ui.add(
                    egui::Button::new(egui::RichText::new(h.review).color(c(t.on_accent)))
                        .fill(c(t.accent)),
                );
                if review.clicked() {
                    out = Some((true, review.id));
                }
                let dismiss = ui.button(h.dismiss);
                if dismiss.clicked() {
                    out = Some((false, dismiss.id));
                }
            });
        });
    out
}

/// What a button in the Permissions section asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionAsk {
    /// Turn off one item (its permission string).
    TurnOff(String),
    TurnAllOff,
    Change,
}

fn word_colour(t: &Tokens, w: ItemWord) -> egui::Color32 {
    c(match w {
        ItemWord::Allowed => t.ok,
        ItemWord::Off => t.text_muted,
        ItemWord::CantBeAllowed => t.danger,
    })
}

/// The Permissions section of one module: the list of items in groups, each with its state in
/// words (never colour alone), Turn off per allowed item, Turn all off and Change…. `open`: the
/// list starts open.
pub fn section_ui(
    ui: &mut egui::Ui,
    t: &Tokens,
    s: &Section,
    open: bool,
) -> Option<(SectionAsk, egui::Id)> {
    let mut out = None;
    let id = ui.make_persistent_id(("hideout.permissions", &s.id));
    let group = ui.push_id(id, |ui| {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(s.heading())
                    .small()
                    .color(c(t.text_muted)),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} allowed", s.allowed()))
                        .strong()
                        .color(c(t.ok)),
                );
                if let Some(a) = &s.allowed_on {
                    ui.label(egui::RichText::new(a).color(c(t.text_muted)));
                }
            });
            for h in &s.history {
                ui.label(egui::RichText::new(h).small().color(c(t.text_muted)));
            }
            egui::CollapsingHeader::new(
                egui::RichText::new("Show each permission").color(c(t.text)),
            )
            .id_salt(("hideout.permissions.list", &s.id))
            .default_open(open)
            .show(ui, |ui| {
                for g in Group::ALL {
                    let items: Vec<_> = s.items.iter().filter(|i| i.group == g).collect();
                    if items.is_empty() {
                        continue;
                    }
                    let said = s.group_said(g);
                    let grp = ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(g.heading())
                                .strong()
                                .small()
                                .color(group_colour(t, g)),
                        );
                        for i in items {
                            ui.horizontal(|ui| {
                                let l = ui.label(egui::RichText::new(&i.line).color(c(t.text)));
                                ui.label(
                                    egui::RichText::new(i.word.word())
                                        .strong()
                                        .color(word_colour(t, i.word)),
                                );
                                let said =
                                    format!("{}: {}, {}", g.heading(), i.line, i.word.word());
                                ui.ctx().accesskit_node_builder(l.id, |n| n.set_label(said));
                                if i.word == ItemWord::Allowed {
                                    let b = ui.button(TURN_OFF);
                                    let said = format!("{TURN_OFF}: {}", i.line);
                                    ui.ctx().accesskit_node_builder(b.id, |n| n.set_label(said));
                                    if b.clicked() {
                                        out = Some((SectionAsk::TurnOff(i.raw.clone()), b.id));
                                    }
                                }
                            });
                        }
                    });
                    ui.ctx().accesskit_node_builder(grp.response.id, |n| {
                        n.set_role(egui::accesskit::Role::Group);
                        n.set_label(said);
                    });
                }
            });
            ui.horizontal(|ui| {
                if s.allowed() > 0 {
                    let b = ui.button(TURN_ALL_OFF);
                    if b.clicked() {
                        out = Some((SectionAsk::TurnAllOff, b.id));
                    }
                }
                if s.can_change {
                    let b = ui.button(CHANGE);
                    if b.clicked() {
                        out = Some((SectionAsk::Change, b.id));
                    }
                }
            });
            if let Some(n) = &s.note {
                let l = ui.label(egui::RichText::new(n).color(c(t.text_muted)));
                ui.ctx().accesskit_node_builder(l.id, |nb| {
                    nb.set_live(egui::accesskit::Live::Polite);
                });
            }
        })
        .response
    });
    let said = format!("{}, {} allowed", s.heading(), s.allowed());
    ui.ctx().accesskit_node_builder(group.inner.id, |n| {
        n.set_role(egui::accesskit::Role::Group);
        n.set_label(said);
    });
    out
}
