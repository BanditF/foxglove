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
    earlier_label, keep_understand_said, keep_warning, keyboard_line, All, FirstRun, Group,
    ItemWord, Keep, KeepAnswer, KeepConfirm, PermAnswer, PermDialog, PermHeader, Section, CANCEL,
    CHANGE, FIRST_RUN_INTRO, HISTORY_SHOWN, KEEP_CONFIRM_OK, KEEP_CONFIRM_TITLE, KEEP_LABEL,
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

/// Text one point under body size (the old `.small()` was about 9 px).
fn fine(ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
    let body = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map_or(14.0, |f| f.size);
    egui::RichText::new(text).size(body - 1.0)
}

/// Gives checkboxes drawn inside `add` an outline in the muted text colour (at least 3:1 on the
/// window, light and dark), so an unchecked box is visible.
fn outlined<R>(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.bg_stroke = egui::Stroke::new(1.5_f32, c(t.text_muted));
        w.hovered.bg_stroke = egui::Stroke::new(1.5_f32, c(t.text));
        add(ui)
    })
    .inner
}

fn check(
    ui: &mut egui::Ui,
    t: &Tokens,
    on: &mut bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    outlined(ui, t, |ui| ui.checkbox(on, text))
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
    // The height of everything but the list, as drawn last frame; the first frame guesses.
    let fixed_id = egui::Id::new("hideout.permission-dialog.fixed");
    let fixed: f32 = ctx
        .data(|m| m.get_temp::<f32>(fixed_id))
        .unwrap_or(if d.keep.is_some() { 330.0 } else { 250.0 });
    let title = d.title();
    let modal = egui::Modal::new(egui::Id::new("hideout.permission-dialog")).show(ctx, |ui| {
        ui.set_width(620.0);
        let top = ui.cursor().top();
        ui.label(egui::RichText::new(&title).heading().color(c(t.text)));
        ui.add_space(6.0);
        for (i, line) in d.reason_lines().iter().enumerate() {
            let rt = egui::RichText::new(line).color(c(t.attention));
            ui.label(if i == 0 { rt.strong() } else { rt });
        }
        if d.reason.is_some() || !d.before_lines().is_empty() {
            ui.add_space(6.0);
        }
        ui.label(d.intro());
        ui.add_space(6.0);
        // All N: tri-state.
        let state = d.all_state();
        let mut all_on = state == All::All;
        let all = outlined(ui, t, |ui| {
            ui.add(
                egui::Checkbox::new(
                    &mut all_on,
                    egui::RichText::new(d.all_label()).strong().color(c(t.text)),
                )
                .indeterminate(state == All::Some),
            )
        });
        if all.changed() {
            d.toggle_all();
        }
        ids.push(all.id);
        ui.add_space(4.0);
        // Scrolls inside when the list is taller than the window allows; the buttons stay.
        let scroll = egui::ScrollArea::vertical()
            .max_height((screen_h * 0.8 - fixed).max(140.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                list_ui(ui, t, d, &mut ids);
            });
        let scroll_h = scroll.inner_rect.height();
        ui.add_space(8.0);
        ui.separator();
        binding_ui(ui, t, d, &mut ids);
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
        let total = ui.cursor().top() - top;
        ui.ctx()
            .data_mut(|m| m.insert_temp(fixed_id, (total - scroll_h + 40.0).max(120.0)));
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

/// What the first-run dialog answered: Allow carries each section's own choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FirstRunAnswer {
    Allow(Vec<(String, crate::permission_dialog::Allowed)>),
    NotNow,
}

pub fn first_run_dialog_ui(
    ctx: &egui::Context,
    t: &Tokens,
    d: &mut FirstRun,
    first: bool,
) -> Option<FirstRunAnswer> {
    first_run_dialog_ids(ctx, t, d, first).0
}

/// The first-run review (UX cut 3): one dialog, a section per app. Every section keeps its own
/// list, its own "I understand" for sensitive items and its own keep box; Allow sends each
/// section's own list code with its own counted items. Not now, Esc and close grant nothing.
pub(crate) fn first_run_dialog_ids(
    ctx: &egui::Context,
    t: &Tokens,
    d: &mut FirstRun,
    first: bool,
) -> (Option<FirstRunAnswer>, Vec<egui::Id>) {
    let mut answer = None;
    let mut ids: Vec<egui::Id> = Vec::new();
    let screen_h = ctx.content_rect().height();
    let title = d.title();
    let modal = egui::Modal::new(egui::Id::new("hideout.first-run-dialog")).show(ctx, |ui| {
        ui.set_width(620.0);
        ui.label(egui::RichText::new(&title).heading().color(c(t.text)));
        ui.add_space(6.0);
        ui.label(FIRST_RUN_INTRO);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height((screen_h * 0.8 - 190.0).max(160.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for sec in d.sections.iter_mut() {
                    ui.push_id(sec.id.clone(), |ui| {
                        ui.add_space(8.0);
                        ui.separator();
                        let head =
                            ui.label(egui::RichText::new(&sec.name).heading().color(c(t.text)));
                        let said = format!("{}, its permissions", sec.name);
                        ui.ctx()
                            .accesskit_node_builder(head.id, |n| n.set_label(said));
                        for (i, line) in sec.reason_lines().iter().enumerate() {
                            let rt = egui::RichText::new(line).color(c(t.attention));
                            ui.label(if i == 0 { rt.strong() } else { rt });
                        }
                        ui.label(sec.intro());
                        ui.add_space(6.0);
                        let state = sec.all_state();
                        let mut all_on = state == All::All;
                        let all = outlined(ui, t, |ui| {
                            ui.add(
                                egui::Checkbox::new(
                                    &mut all_on,
                                    egui::RichText::new(sec.all_label())
                                        .strong()
                                        .color(c(t.text)),
                                )
                                .indeterminate(state == All::Some),
                            )
                        });
                        if all.changed() {
                            sec.toggle_all();
                        }
                        ids.push(all.id);
                        ui.add_space(4.0);
                        list_ui(ui, t, sec, &mut ids);
                        ui.add_space(8.0);
                        binding_ui(ui, t, sec, &mut ids);
                    });
                }
            });
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
                answer = Some(FirstRunAnswer::Allow(d.allowed()));
            }
            let not_now = ui.add(
                egui::Button::new(egui::RichText::new(NOT_NOW).color(c(t.on_accent)))
                    .fill(c(t.accent)),
            );
            if first {
                not_now.request_focus();
            }
            if not_now.clicked() {
                answer = Some(FirstRunAnswer::NotNow);
            }
            ids.push(allow.id);
            ids.push(not_now.id);
        });
    });
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
        answer = answer.or(Some(FirstRunAnswer::NotNow));
    }
    (answer, ids)
}

/// The groups of items and the keyboard choice of one dialog, in Tab order.
fn list_ui(ui: &mut egui::Ui, t: &Tokens, d: &mut PermDialog, ids: &mut Vec<egui::Id>) {
    for g in Group::ALL {
        let n = d.group(g).count();
        if n == 0 {
            continue;
        }
        let said = format!("{}, {n} item{}", g.heading(), if n == 1 { "" } else { "s" });
        let grp = ui.vertical(|ui| {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(g.heading())
                    .strong()
                    .color(group_colour(t, g)),
            );
            let name = d.name.clone();
            for item in d.items.iter_mut().filter(|i| i.group == g) {
                if !item.allowable {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&item.line).color(c(t.text)));
                        if item.new {
                            ui.label(fine(ui, "New").strong().color(c(t.attention)));
                        }
                    });
                    if let Some(r) = &item.reason {
                        ui.label(fine(ui, r.replace("this module", &name)).color(c(t.text_muted)));
                    }
                    continue;
                }
                let r = ui
                    .horizontal(|ui| {
                        let r = check(ui, t, &mut item.checked, &item.line);
                        if item.new {
                            ui.label(fine(ui, "New").strong().color(c(t.attention)));
                        }
                        if item.before {
                            ui.label(
                                fine(ui, crate::permission_dialog::BEFORE_MARK)
                                    .color(c(t.text_muted)),
                            );
                        }
                        r
                    })
                    .inner;
                let said = item.said();
                ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(said));
                if r.changed() && !item.checked {
                    item.confirmed = false;
                }
                ids.push(r.id);
                if let Some(s) = item.sub_line() {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.label(fine(ui, s).color(c(t.text_muted)));
                    });
                }
                if item.sensitive && item.checked {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(item.confirm_line(&name)).color(c(t.attention)),
                            );
                            let u = check(ui, t, &mut item.confirmed, UNDERSTAND);
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
            egui::Button::new(egui::RichText::new(format!("{sign}  {word}")).color(c(t.accent)))
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
                    let on = check(ui, t, &mut k.on, "Allow this too");
                    ids.push(on.id);
                });
            });
        }
    }
}

/// The binding lines, the app path, the keep box and the list code of one dialog.
fn binding_ui(ui: &mut egui::Ui, t: &Tokens, d: &mut PermDialog, ids: &mut Vec<egui::Id>) {
    let [b0, b1, b2] = d.binding_lines();
    ui.label(b0);
    if !d.path.is_empty() {
        ui.add(
            egui::Label::new(egui::RichText::new(&d.path).monospace())
                .wrap()
                .selectable(true),
        );
    }
    ui.label(fine(ui, b1).color(c(t.text_muted)));
    ui.label(fine(ui, b2).color(c(t.text_muted)));
    if let Some(k) = d.keep.as_mut() {
        keep_block(ui, t, &d.name, &d.path, k, ids);
    }
    ui.add(
        egui::Label::new(fine(ui, format!("List code {}", d.code)).color(c(t.text_muted)))
            .selectable(true),
    );
}

/// The "Keep my OK after updates in this folder" box in the dialog, under the binding lines (so
/// it stays outside the scrolling list), with its helper, and once checked the warning and the
/// "I understand" box.
fn keep_block(
    ui: &mut egui::Ui,
    t: &Tokens,
    name: &str,
    path: &str,
    k: &mut Keep,
    ids: &mut Vec<egui::Id>,
) {
    let file = crate::permission_dialog::file_of(path);
    ui.add_space(8.0);
    let refused = k.is_refused();
    let mut on = k.on;
    let r = outlined(ui, t, |ui| {
        ui.add_enabled(
            !refused,
            egui::Checkbox::new(
                &mut on,
                egui::RichText::new(KEEP_LABEL).strong().color(c(t.text)),
            ),
        )
    });
    if r.changed() {
        k.set_on(on);
    }
    let said = k.said();
    let desc = k.description(name, &file);
    let node_desc = desc.clone();
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_label(said);
        n.set_description(node_desc);
    });
    if !refused {
        ids.push(r.id);
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.add(egui::Label::new(fine(ui, desc).color(c(t.text_muted))).wrap());
    });
    if k.on && !refused {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(keep_warning(name, &file)).color(c(t.attention)),
                    )
                    .wrap(),
                );
                let mut u = k.understood;
                let ur = check(ui, t, &mut u, UNDERSTAND);
                k.understood = u;
                let said = keep_understand_said(name, &file);
                ui.ctx()
                    .accesskit_node_builder(ur.id, |n| n.set_label(said));
                ids.push(ur.id);
            });
        });
    }
}

/// The small confirm of the Modules page's Turn on. Cancel takes the focus on the first frame
/// and carries the accent fill; Esc, close and Cancel answer Cancel. The ids are in Tab order,
/// the last two Keep my OK and Cancel.
pub(crate) fn keep_confirm_ids(
    ctx: &egui::Context,
    t: &Tokens,
    k: &KeepConfirm,
    first: bool,
) -> (Option<KeepAnswer>, Vec<egui::Id>) {
    let mut answer = None;
    let mut ids: Vec<egui::Id> = Vec::new();
    let modal = egui::Modal::new(egui::Id::new("hideout.keep-confirm")).show(ctx, |ui| {
        ui.set_width(480.0);
        ui.label(
            egui::RichText::new(KEEP_CONFIRM_TITLE)
                .heading()
                .color(c(t.text)),
        );
        ui.add_space(6.0);
        ui.add(egui::Label::new(egui::RichText::new(k.line()).color(c(t.attention))).wrap());
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(egui::RichText::new(&k.folder).monospace())
                .wrap()
                .selectable(true),
        );
        if let Some(e) = &k.error {
            ui.add_space(4.0);
            let l = ui.label(egui::RichText::new(e).color(c(t.attention)));
            ui.ctx().accesskit_node_builder(l.id, |n| {
                n.set_live(egui::accesskit::Live::Polite);
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let keep = ui.add(
                egui::Button::new(egui::RichText::new(KEEP_CONFIRM_OK).color(c(t.accent)))
                    .stroke(egui::Stroke::new(1.0_f32, c(t.accent))),
            );
            if keep.clicked() {
                answer = Some(KeepAnswer::Keep);
            }
            let cancel = ui.add(
                egui::Button::new(egui::RichText::new(CANCEL).color(c(t.on_accent)))
                    .fill(c(t.accent)),
            );
            if first {
                cancel.request_focus();
            }
            if cancel.clicked() {
                answer = Some(KeepAnswer::Cancel);
            }
            ids.push(keep.id);
            ids.push(cancel.id);
        });
    });
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
    let said = k.said();
    ctx.accesskit_node_builder(modal.response.id, |n| {
        n.set_role(egui::accesskit::Role::Dialog);
        n.set_label(KEEP_CONFIRM_TITLE);
        n.set_description(said);
    });
    if modal.should_close() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = answer.or(Some(KeepAnswer::Cancel));
    }
    (answer, ids)
}

pub fn keep_confirm_ui(
    ctx: &egui::Context,
    t: &Tokens,
    k: &KeepConfirm,
    first: bool,
) -> Option<KeepAnswer> {
    keep_confirm_ids(ctx, t, k, first).0
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
    /// Open the confirm of "Keep my OK after updates in this folder".
    KeepOn,
    /// Stop keeping it.
    KeepOff,
}

const STATE_W: f32 = 120.0;
const BUTTON_W: f32 = 80.0;

/// A fixed-width cell in a row.
fn cell<R>(
    ui: &mut egui::Ui,
    w: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.allocate_ui_with_layout(
        egui::vec2(w, 24.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_width(w);
            add(ui)
        },
    )
}

fn word_colour(t: &Tokens, w: ItemWord) -> egui::Color32 {
    c(match w {
        ItemWord::Allowed => t.ok,
        ItemWord::Off => t.text_muted,
        ItemWord::Waits => t.attention,
        ItemWord::CantBeAllowed => t.text,
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
            ui.label(fine(ui, s.heading()).color(c(t.text_muted)));
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
            if let Some(v) = &s.kept_void {
                ui.label(fine(ui, v).color(c(t.attention)));
            }
            let older = s.history.len().saturating_sub(HISTORY_SHOWN);
            if older > 0 {
                egui::CollapsingHeader::new(fine(ui, earlier_label(older)).color(c(t.text_muted)))
                    .id_salt(("hideout.permissions.earlier", &s.id))
                    .show(ui, |ui| {
                        for h in s.history.iter().take(older) {
                            ui.label(fine(ui, h).color(c(t.text_muted)));
                        }
                    });
            }
            for h in s.history.iter().skip(older) {
                ui.label(fine(ui, h).color(c(t.text_muted)));
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
                                .color(group_colour(t, g)),
                        );
                        // Fixed columns: the line, the state word, the Turn off button.
                        let line_w = (ui.available_width() - STATE_W - BUTTON_W - 16.0).max(160.0);
                        for i in items {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let l = cell(ui, line_w, |ui| {
                                    ui.label(egui::RichText::new(&i.line).color(c(t.text)))
                                })
                                .inner;
                                cell(ui, STATE_W, |ui| {
                                    ui.label(
                                        egui::RichText::new(i.word.word())
                                            .strong()
                                            .color(word_colour(t, i.word)),
                                    )
                                });
                                let said =
                                    format!("{}: {}, {}", g.heading(), i.line, i.word.word());
                                ui.ctx().accesskit_node_builder(l.id, |n| n.set_label(said));
                                cell(ui, BUTTON_W, |ui| {
                                    if i.word == ItemWord::Allowed {
                                        let b = ui.button(TURN_OFF);
                                        let said = format!("{TURN_OFF}: {}", i.line);
                                        ui.ctx()
                                            .accesskit_node_builder(b.id, |n| n.set_label(said));
                                        if b.clicked() {
                                            out = Some((SectionAsk::TurnOff(i.raw.clone()), b.id));
                                        }
                                    }
                                });
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
            if let Some(k) = &s.keep {
                ui.add_space(6.0);
                let row_said = k.said();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let l = cell(ui, 360.0, |ui| {
                        ui.label(egui::RichText::new(KEEP_LABEL).color(c(t.text)))
                    })
                    .inner;
                    ui.ctx()
                        .accesskit_node_builder(l.id, |n| n.set_label(row_said));
                    let on = k.word() == "On";
                    cell(ui, 60.0, |ui| {
                        ui.label(egui::RichText::new(k.word()).strong().color(if on {
                            c(t.ok)
                        } else {
                            c(t.text_muted)
                        }))
                    });
                    if let (Some(text), Some(said)) = (k.button(), k.button_said()) {
                        let b = ui.button(text);
                        ui.ctx().accesskit_node_builder(b.id, |n| n.set_label(said));
                        if b.clicked() {
                            let ask = if on {
                                SectionAsk::KeepOff
                            } else {
                                SectionAsk::KeepOn
                            };
                            out = Some((ask, b.id));
                        }
                    }
                });
                ui.label(fine(ui, k.helper()).color(c(t.text_muted)));
                ui.label(fine(ui, k.folder.as_str()).color(c(t.text_muted)));
            }
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
