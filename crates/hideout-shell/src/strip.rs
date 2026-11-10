// SPDX-License-Identifier: MIT
//! The tab strip along the top of the page area (§5.9, `docs/hideout-tabs-ux.md`): draws
//! [`Tabs`] from each tab's [`Look`] and returns what the user did as [`Action`]s, so the window
//! applies them and tests can drive the strip without a window.

use crate::tabs::{Look, Shown, TabState, Tabs};
use crate::theme::{self, Rgb, Tokens};
use eframe::egui::{
    self, accesskit, Align2, Color32, CornerRadius, FontId, Key, Modifiers, Pos2, Rect, Sense,
    Stroke, Ui, Vec2, WidgetText,
};
use hideout_panels::{HostedPanel, PanelState};

pub const STRIP_H: f32 = 36.0;
/// Tabs shrink to this before the strip scrolls.
pub const MIN_TAB_W: f32 = 96.0;
const MAX_TAB_W: f32 = 220.0;
/// ‹ › ˅ at the right end when the tabs don't fit.
const OVERFLOW_W: f32 = 84.0;
/// The ↗ Pop out button at the strip's right end, for the active tab (H2.11).
pub const POPOUT_W: f32 = 36.0;
/// Its tooltip.
pub const POPOUT_TIP: &str = "Pop out (Ctrl+Shift+O)";

/// What the user did in the strip, by strip position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Activate(usize),
    Close(usize),
    CloseOthers(usize),
    CloseRight(usize),
    Move { from: usize, to: usize },
    PopOut(usize),
    DockBack(usize),
    Reopen,
}

/// The strip's own view state: the scroll offset when it overflows, and a drag in progress.
#[derive(Clone, Debug, Default)]
pub struct StripState {
    pub scroll: f32,
    /// Scroll the active tab into view on the next draw (after a switch).
    pub follow: bool,
    /// A tab is being dragged: its position.
    drag: Option<usize>,
    last_active: Option<usize>,
}

fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// A module panel's look from the panel host: its title, its state in §7.8 words, its badge.
pub fn panel_look(p: &HostedPanel, popped: bool) -> Look {
    use crate::tabs::Mark;
    use hideout_panels::Badge;
    Look {
        title: p.title.clone(),
        state: panel_state(&p.state),
        mark: match (p.badge, p.badge_count) {
            (Badge::Attention, _) => Mark::Attention,
            (Badge::Dirty, _) => Mark::Dirty,
            (Badge::Count, Some(n)) => Mark::Count(n),
            _ => Mark::None,
        },
        popped,
    }
}

/// The strip's state for a panel state (§7.8 words, never ids).
pub fn panel_state(s: &PanelState) -> TabState {
    match s {
        PanelState::Live => TabState::Live,
        PanelState::Greyed(id) => match id.as_str() {
            "starting" | "updating" | "restarting" => {
                TabState::Busy(hideout_panels::state_words(id).into())
            }
            "off" => TabState::Off,
            other => TabState::Stopped(hideout_panels::state_words(other).into()),
        },
        PanelState::Rejected(_) | PanelState::Broken => {
            TabState::Stopped("Couldn't be shown".into())
        }
        PanelState::Quarantined => TabState::Stopped("Paused after repeated hub restarts".into()),
    }
}

/// A strip shortcut (the window consumes the keys, so the page never sees them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Next,
    Prev,
    Number(u8),
    Close,
    Reopen,
    MoveLeft,
    MoveRight,
    /// Ctrl+Shift+O: pop the active tab out, or dock it back when it is out.
    PopOut,
}

/// Takes this frame's strip shortcuts out of the input: Ctrl+Tab / Ctrl+Shift+Tab and
/// Ctrl+PageDown / Ctrl+PageUp switch in strip order, Ctrl+1–9, Ctrl+W, Ctrl+Shift+T, and
/// Ctrl+Shift+PageUp / PageDown move the active tab. Shifted keys are read first: egui's match
/// ignores an extra Shift.
pub fn shortcuts(ctx: &egui::Context) -> Vec<Shortcut> {
    let cs = Modifiers::COMMAND | Modifiers::SHIFT;
    ctx.input_mut(|i| {
        let mut out = Vec::new();
        let mut take = |m: Modifiers, k: Key, s: Shortcut| {
            for _ in 0..i.count_and_consume_key(m, k) {
                out.push(s);
            }
        };
        take(cs, Key::Tab, Shortcut::Prev);
        take(cs, Key::PageUp, Shortcut::MoveLeft);
        take(cs, Key::PageDown, Shortcut::MoveRight);
        take(cs, Key::T, Shortcut::Reopen);
        take(cs, Key::O, Shortcut::PopOut);
        take(Modifiers::COMMAND, Key::Tab, Shortcut::Next);
        take(Modifiers::COMMAND, Key::PageDown, Shortcut::Next);
        take(Modifiers::COMMAND, Key::PageUp, Shortcut::Prev);
        take(Modifiers::COMMAND, Key::W, Shortcut::Close);
        let nums = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (n, k) in nums.into_iter().enumerate() {
            take(Modifiers::COMMAND, k, Shortcut::Number(n as u8 + 1));
        }
        out
    })
}

/// The width a tab wants for its title and badges.
fn ideal_width(ui: &Ui, l: &Look) -> f32 {
    let font = FontId::proportional(14.0);
    let text = ui
        .painter()
        .layout_no_wrap(l.title.clone(), font, Color32::WHITE)
        .size()
        .x;
    let mut w = 12.0 + text + 34.0;
    if has_mark(l) {
        w += 22.0;
    }
    if let Shown::Count(s) = l.shown() {
        w += 14.0 + 7.0 * s.len() as f32;
    }
    if l.popped {
        w += 16.0;
    }
    w.clamp(MIN_TAB_W, MAX_TAB_W)
}

fn has_mark(l: &Look) -> bool {
    l.busy() || matches!(l.shown(), Shown::Stopped | Shown::Attention)
}

/// Draws the strip in `ui` (a full-width row `STRIP_H` high) and returns what the user did.
pub fn strip_ui(
    ui: &mut Ui,
    tabs: &Tabs,
    looks: &[Look],
    t: &Tokens,
    state: &mut StripState,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let whole = ui.available_rect_before_wrap();
    let whole = Rect::from_min_size(whole.min, Vec2::new(whole.width(), STRIP_H));
    // The ↗ button keeps its place at the right end; the tabs get the rest.
    let full = Rect::from_min_size(
        whole.min,
        Vec2::new((whole.width() - POPOUT_W).max(0.0), STRIP_H),
    );
    ui.painter().rect_filled(whole, 0.0, c(t.chrome));
    ui.painter().hline(
        whole.x_range(),
        whole.bottom() - 0.5,
        Stroke::new(1.0_f32, c(t.border)),
    );
    popout_button_ui(ui, whole, tabs, t, &mut actions);
    if tabs.active() != state.last_active {
        state.follow = true;
        state.last_active = tabs.active();
    }
    let n = tabs.len();
    let mut widths: Vec<f32> = looks.iter().map(|l| ideal_width(ui, l)).collect();
    let total: f32 = widths.iter().sum();
    let overflow = total > full.width() && n > 0;
    if overflow {
        // Shrink the widest first, down to the minimum.
        let room = full.width() - OVERFLOW_W;
        let even = (room / n as f32).max(MIN_TAB_W);
        for w in widths.iter_mut() {
            *w = w.min(even);
        }
    }
    let strip_w = if overflow {
        full.width() - OVERFLOW_W
    } else {
        full.width()
    };
    let tabs_rect = Rect::from_min_size(full.min, Vec2::new(strip_w, STRIP_H));
    let list_id = ui.unique_id().with("hideout.tablist");
    ui.ctx().accesskit_node_builder(list_id, |node| {
        node.set_role(accesskit::Role::TabList);
        node.set_label("Open pages");
    });
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(tabs_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Min))
            .id_salt("hideout.tabs"),
    );
    let content_w: f32 = widths.iter().sum();
    let max_scroll = (content_w - strip_w).max(0.0);
    if state.follow {
        if let Some(a) = tabs.active() {
            let x0: f32 = widths[..a].iter().sum();
            let x1 = x0 + widths[a];
            if x0 < state.scroll {
                state.scroll = x0;
            } else if x1 > state.scroll + strip_w {
                state.scroll = x1 - strip_w;
            }
        }
        state.follow = false;
    }
    state.scroll = state.scroll.clamp(0.0, max_scroll);
    child.set_clip_rect(tabs_rect.intersect(ui.clip_rect()));
    let mut x = tabs_rect.left() - state.scroll;
    let mut rects = Vec::with_capacity(n);
    for (i, (tab, l)) in tabs.tabs().iter().zip(looks).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, full.top()), Vec2::new(widths[i], STRIP_H));
        x += widths[i];
        rects.push(r);
        let active = tabs.active() == Some(i);
        tab_ui(
            &mut child,
            i,
            n,
            tab.popped,
            l,
            r,
            active,
            t,
            state,
            &mut actions,
        );
    }
    // The drag: a 2 px orange bar where the tab will land.
    if let Some(from) = state.drag {
        let pointer = ui.ctx().pointer_latest_pos();
        if let Some(p) = pointer {
            let to = drop_index(&rects, p.x, from);
            let bar_x = if to <= from {
                rects.get(to).map(|r| r.left())
            } else {
                rects.get(to).map(|r| r.right())
            };
            if let Some(bx) = bar_x {
                child.painter().rect_filled(
                    Rect::from_x_y_ranges(bx - 1.0..=bx + 1.0, full.y_range()),
                    0.0,
                    c(t.accent),
                );
            }
            if ui.ctx().input(|i| i.pointer.any_released()) {
                if to != from {
                    actions.push(Action::Move { from, to });
                }
                state.drag = None;
            }
        }
    }
    if overflow {
        overflow_ui(ui, full, tabs, looks, t, state, max_scroll, &mut actions);
    }
    // Scroll wheel over the strip moves it sideways.
    if overflow && ui.rect_contains_pointer(tabs_rect) {
        let d = ui.ctx().input(|i| i.smooth_scroll_delta);
        let d = if d.x != 0.0 { d.x } else { d.y };
        if d != 0.0 {
            state.scroll = (state.scroll - d).clamp(0.0, max_scroll);
        }
    }
    ui.allocate_rect(whole, Sense::hover());
    actions
}

/// The ↗ Pop out button for the active tab (the pop-outs plan, Decision 3). Not drawn while the
/// active tab is out already or nothing is open.
fn popout_button_ui(ui: &mut Ui, whole: Rect, tabs: &Tabs, t: &Tokens, actions: &mut Vec<Action>) {
    let Some(i) = tabs.active().filter(|&i| !tabs.tabs()[i].popped) else {
        return;
    };
    let r = Rect::from_center_size(
        Pos2::new(whole.right() - POPOUT_W / 2.0, whole.center().y),
        Vec2::splat(26.0),
    );
    let resp = ui.interact(r, ui.id().with("hideout.popout-button"), Sense::click());
    if resp.hovered() || resp.has_focus() {
        ui.painter()
            .rect_filled(r, CornerRadius::same(4), c(t.surface));
    }
    ui.painter().text(
        r.center(),
        Align2::CENTER_CENTER,
        "\u{2197}",
        FontId::proportional(16.0),
        c(t.text),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Pop out"));
    if resp.on_hover_text(POPOUT_TIP).clicked() {
        actions.push(Action::PopOut(i));
    }
}

/// Where a tab dragged from `from` lands for a pointer at `x`.
pub fn drop_index(rects: &[Rect], x: f32, from: usize) -> usize {
    let mut to = from;
    for (i, r) in rects.iter().enumerate() {
        if x >= r.left() && x < r.right() {
            to = i;
        }
    }
    if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
        if x < first.left() {
            to = 0;
        } else if x >= last.right() {
            to = rects.len() - 1;
        }
    }
    to
}

#[allow(clippy::too_many_arguments)]
fn tab_ui(
    ui: &mut Ui,
    i: usize,
    n: usize,
    popped: bool,
    l: &Look,
    r: Rect,
    active: bool,
    t: &Tokens,
    state: &mut StripState,
    actions: &mut Vec<Action>,
) {
    let id = ui.id().with(("tab", i));
    // One stop in the Tab order: only the active tab takes keyboard focus (WAI-ARIA tabs).
    let sense = if active {
        Sense::click_and_drag()
    } else {
        Sense::CLICK | Sense::DRAG
    };
    let resp = ui.interact(r, id, sense);
    let hovered = resp.hovered() || resp.has_focus();
    let fill = c(theme::tab_fill(t, active));
    let fg = c(theme::tab_label(t, active, l.muted()));
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, fill);
    if active {
        p.rect_filled(
            Rect::from_min_size(r.min, Vec2::new(r.width(), 2.0)),
            0.0,
            c(t.accent),
        );
    } else {
        p.hline(
            r.x_range(),
            r.bottom() - 0.5,
            Stroke::new(1.0_f32, c(t.border)),
        );
    }
    p.vline(
        r.right() - 0.5,
        r.top() + 8.0..=r.bottom() - 8.0,
        Stroke::new(1.0_f32, c(t.border)),
    );
    let mid = r.center().y;
    let mut lx = r.left() + 12.0;
    let shown = l.shown();
    if has_mark(l) {
        let mc = Pos2::new(lx + 7.0, mid);
        match shown {
            Shown::Stopped => {
                p.text(
                    mc,
                    Align2::CENTER_CENTER,
                    "\u{26a0}",
                    FontId::proportional(14.0),
                    c(t.danger),
                );
            }
            Shown::Attention => {
                p.circle_filled(mc, 4.0, c(t.accent));
            }
            _ => {}
        }
        if l.busy() {
            // A ring with a gap, round a small dot: "working on it".
            let ring = Stroke::new(1.5_f32, c(t.accent));
            let pts: Vec<Pos2> = (0..=20)
                .map(|k| {
                    let a = 0.6 + k as f32 * (5.2 / 20.0);
                    mc + Vec2::angled(a) * 6.5
                })
                .collect();
            p.add(egui::Shape::line(pts, ring));
            if !matches!(shown, Shown::Attention) {
                p.circle_filled(mc, 2.5, fg);
            }
        }
        lx += 22.0;
    }
    let close = Rect::from_center_size(Pos2::new(r.right() - 18.0, mid), Vec2::splat(20.0));
    let mut right = close.left() - 4.0;
    if let Shown::Count(s) = &shown {
        let font = FontId::proportional(11.0);
        let g = p.layout_no_wrap(s.clone(), font, c(t.on_accent));
        let pw = (g.size().x + 10.0).max(18.0);
        let pill = Rect::from_min_max(
            Pos2::new(right - pw, mid - 8.0),
            Pos2::new(right, mid + 8.0),
        );
        p.rect_filled(pill, CornerRadius::same(8), c(t.accent));
        p.galley(pill.center() - g.size() / 2.0, g, c(t.on_accent));
        right = pill.left() - 4.0;
    }
    if popped {
        p.text(
            Pos2::new(right - 6.0, mid),
            Align2::CENTER_CENTER,
            "\u{2197}",
            FontId::proportional(12.0),
            c(t.text_muted),
        );
        right -= 16.0;
    }
    let label = WidgetText::from(egui::RichText::new(&l.title).size(14.0).color(fg)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (right - lx).max(8.0),
        egui::TextStyle::Body,
    );
    p.galley(Pos2::new(lx, mid - label.size().y / 2.0), label, fg);
    // The close slot: the unsaved dot, × on hover or on the active tab.
    let close_resp = ui.interact(close, id.with("close"), Sense::click());
    let dirty = shown == Shown::Dirty;
    if dirty && !(hovered || close_resp.hovered()) {
        p.circle_filled(close.center(), 4.0, fg);
    } else if active || hovered || close_resp.hovered() {
        if close_resp.hovered() {
            p.rect_filled(close, CornerRadius::same(4), c(t.surface));
        }
        p.text(
            close.center(),
            Align2::CENTER_CENTER,
            "\u{2715}",
            FontId::proportional(11.0),
            fg,
        );
    }
    let close_name = format!("Close {}", l.title);
    close_resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, close_name.clone())
    });
    let name = l.accessible_name();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, &name));
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_role(accesskit::Role::Tab);
        node.set_selected(active);
        node.set_position_in_set(i + 1);
        node.set_size_of_set(n);
    });
    // Input.
    if close_resp.clicked() || resp.middle_clicked() {
        actions.push(Action::Close(i));
    } else if resp.clicked() {
        // A placeholder's click brings its window forward (the hub, `Activate`); it never docks.
        actions.push(Action::Activate(i));
    }
    if resp.drag_started() {
        state.drag = Some(i);
    }
    if resp.has_focus() {
        let keys = ui.ctx().input_mut(|inp| {
            (
                inp.consume_key(Modifiers::NONE, Key::ArrowLeft),
                inp.consume_key(Modifiers::NONE, Key::ArrowRight),
                inp.consume_key(Modifiers::NONE, Key::Home),
                inp.consume_key(Modifiers::NONE, Key::End),
                inp.consume_key(Modifiers::NONE, Key::Delete),
            )
        });
        let to = match keys {
            (true, ..) => Some((i + n - 1) % n),
            (_, true, ..) => Some((i + 1) % n),
            (_, _, true, ..) => Some(0),
            (_, _, _, true, _) => Some(n - 1),
            _ => None,
        };
        if let Some(to) = to {
            actions.push(Action::Activate(to));
        }
        if keys.4 {
            actions.push(Action::Close(i));
        }
    }
    let tip = l.tooltip();
    let resp = resp.on_hover_text(tip);
    resp.context_menu(|ui| {
        let mut item = |ui: &mut Ui, text: &str, a: Action| {
            if ui.button(text).clicked() {
                actions.push(a);
                ui.close();
            }
        };
        item(ui, "Close", Action::Close(i));
        item(ui, "Close others", Action::CloseOthers(i));
        item(ui, "Close to the right", Action::CloseRight(i));
        ui.separator();
        if popped {
            item(ui, "Dock back", Action::DockBack(i));
        } else {
            item(ui, "Pop out", Action::PopOut(i));
        }
        if i > 0 {
            item(ui, "Move left", Action::Move { from: i, to: i - 1 });
        }
        if i + 1 < n {
            item(ui, "Move right", Action::Move { from: i, to: i + 1 });
        }
        ui.separator();
        item(ui, "Reopen closed tab", Action::Reopen);
    });
}

#[allow(clippy::too_many_arguments)]
fn overflow_ui(
    ui: &mut Ui,
    full: Rect,
    tabs: &Tabs,
    looks: &[Look],
    t: &Tokens,
    state: &mut StripState,
    max_scroll: f32,
    actions: &mut Vec<Action>,
) {
    let x0 = full.right() - OVERFLOW_W;
    let p = ui.painter();
    p.rect_filled(
        Rect::from_x_y_ranges(x0..=full.right(), full.y_range()),
        0.0,
        c(t.chrome),
    );
    p.vline(
        x0,
        full.top() + 6.0..=full.bottom() - 6.0,
        Stroke::new(1.0_f32, c(t.border)),
    );
    let step = MIN_TAB_W;
    let buttons = [
        ("\u{2039}", "Scroll tabs left"),
        ("\u{203a}", "Scroll tabs right"),
    ];
    for (k, (glyph, name)) in buttons.iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(x0 + 4.0 + k as f32 * 26.0, full.top() + 6.0),
            Vec2::new(24.0, 24.0),
        );
        let resp = ui.interact(r, ui.id().with(("scroll", k)), Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(r, CornerRadius::same(4), c(t.surface));
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            *glyph,
            FontId::proportional(18.0),
            c(t.text),
        );
        let name = name.to_string();
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
        if resp.clicked() {
            let d = if k == 0 { -step } else { step };
            state.scroll = (state.scroll + d).clamp(0.0, max_scroll);
        }
    }
    let r = Rect::from_min_size(
        Pos2::new(x0 + 56.0, full.top() + 6.0),
        Vec2::new(24.0, 24.0),
    );
    let mut list = ui.new_child(egui::UiBuilder::new().max_rect(r));
    list.style_mut().visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    let resp = list.menu_button(egui::RichText::new("\u{2304}").color(c(t.text)), |ui| {
        for (i, l) in looks.iter().enumerate() {
            let mut text = l.title.clone();
            match l.shown() {
                Shown::Count(s) => text.push_str(&format!("  ({s})")),
                Shown::Dirty => text.push_str("  \u{25cf}"),
                Shown::Stopped => text.push_str("  \u{26a0}"),
                Shown::Attention => text.push_str("  \u{2022}"),
                Shown::None => {}
            }
            let num = match i {
                0..=7 => format!("Ctrl+{}", i + 1),
                _ if i + 1 == looks.len() => "Ctrl+9".into(),
                _ => String::new(),
            };
            let selected = tabs.active() == Some(i);
            let b = egui::Button::selectable(
                selected,
                egui::RichText::new(text).color(c(theme::palette_label(t, selected, false))),
            )
            .shortcut_text(num);
            if ui.add(b).clicked() {
                actions.push(Action::Activate(i));
                ui.close();
            }
        }
    });
    resp.response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "All open tabs"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_states_read_in_words() {
        assert_eq!(
            panel_state(&PanelState::Greyed("updating".into())),
            TabState::Busy("Updating".into())
        );
        assert_eq!(
            panel_state(&PanelState::Greyed("crash-stopped".into())),
            TabState::Stopped("Stopped after 5 crashes".into())
        );
        assert_eq!(
            panel_state(&PanelState::Greyed("off".into())),
            TabState::Off
        );
        assert!(matches!(
            panel_state(&PanelState::Rejected("ui.limits".into())),
            TabState::Stopped(w) if !w.contains('.')
        ));
    }

    #[test]
    fn a_drop_lands_on_the_tab_under_the_pointer() {
        let rects: Vec<Rect> = (0..4)
            .map(|i| Rect::from_min_size(Pos2::new(i as f32 * 100.0, 0.0), Vec2::new(100.0, 36.0)))
            .collect();
        assert_eq!(drop_index(&rects, 250.0, 0), 2);
        assert_eq!(drop_index(&rects, -5.0, 3), 0);
        assert_eq!(drop_index(&rects, 999.0, 0), 3);
    }
}
