// SPDX-License-Identifier: MIT
//! The tab strip along the top of the page area (§5.9, `docs/hideout-tabs-ux.md`): draws
//! [`Tabs`] from each tab's [`Look`] and returns what the user did as [`Action`]s, so the window
//! applies them and tests can drive the strip without a window.

use crate::drag;
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
    Move {
        from: usize,
        to: usize,
    },
    PopOut(usize),
    /// The tab was dragged out of Hideout's window and let go there (#1054.4): pop it out at
    /// the pointer.
    TearOff(usize),
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
    /// Where the dragged tab is (strip, over Hideout, outside).
    zone: drag::Zone,
    /// The drag's last known pointer (egui forgets it while the pointer is outside the window).
    pointer: Option<Pos2>,
    /// A pop-out dragged over the strip by its title row lands in this slot (0..=n): the window
    /// code sets it, the strip draws the bar.
    pub dock_slot: Option<usize>,
    /// The tabs' area and each tab's span as last drawn, for the window code's drop test.
    layout: Option<(drag::Area, Vec<drag::Span>)>,
    last_active: Option<usize>,
}

impl StripState {
    /// The ghost card to show: the dragged tab and whether Hideout is under the pointer (letting
    /// go there does nothing). `None` while no tab is dragged past [`drag::TEAR_OFF`].
    pub fn ghost(&self) -> Option<(usize, bool)> {
        let from = self.drag?;
        match self.zone {
            drag::Zone::Strip => None,
            drag::Zone::Hub => Some((from, true)),
            drag::Zone::Outside => Some((from, false)),
        }
    }

    /// The tabs' area and spans as last drawn (main window client points).
    pub fn layout(&self) -> Option<(drag::Area, &[drag::Span])> {
        self.layout.as_ref().map(|(a, s)| (*a, s.as_slice()))
    }
}

fn area(r: Rect) -> drag::Area {
    drag::Area {
        left: r.left(),
        top: r.top(),
        right: r.right(),
        bottom: r.bottom(),
    }
}

/// Hideout's whole window (its frame and caption too) in its client points; the client area
/// when egui doesn't know the frame (tests).
fn window_area(ctx: &egui::Context) -> drag::Area {
    ctx.input(|i| {
        let v = i.viewport();
        match (v.outer_rect, v.inner_rect) {
            (Some(o), Some(inner)) => area(o.translate(-inner.min.to_vec2())),
            _ => area(i.viewport_rect()),
        }
    })
}

fn c(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// The open arc of the "working on it" ring round `centre`: the tab strip and the sidebar draw
/// the same shape (docs/hideout-supervisor-ux.md, S and T).
pub fn ring_points(centre: Pos2, radius: f32) -> Vec<Pos2> {
    (0..=24)
        .map(|k| {
            let a = 0.6 + k as f32 * (5.2 / 24.0);
            centre + Vec2::angled(a) * radius
        })
        .collect()
}

/// A module panel's look from the panel host: its title, its state in §7.8 words, its badge.
pub fn panel_look(p: &HostedPanel, popped: bool) -> Look {
    use crate::tabs::Mark;
    use hideout_panels::Badge;
    Look {
        title: p.title.clone(),
        icon: hideout_panels::tray::icon_glyph(&p.icon),
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
    // The icon (or the stopped mark in its place) always has its slot.
    let mut w = 12.0 + ICON_SLOT + text + 34.0;
    if let Shown::Count(s) = l.shown() {
        w += 14.0 + 7.0 * s.len() as f32;
    }
    if l.popped {
        w += 16.0;
    }
    w.clamp(MIN_TAB_W, MAX_TAB_W)
}

/// The icon's slot before the title: the page's icon, the stopped mark in its place, the
/// attention dot on its corner and the busy ring round it.
const ICON_SLOT: f32 = 22.0;

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
    let spans: Vec<drag::Span> = rects
        .iter()
        .map(|r| drag::Span {
            left: r.left(),
            right: r.right(),
        })
        .collect();
    drag_ui(&child, whole, tabs, &rects, t, state, &mut actions);
    // A pop-out dragged over the strip by its title row: the bar where it will land.
    if let Some(bx) = state
        .dock_slot
        .and_then(|s| drag::bar_x(&spans, s, (tabs_rect.left(), tabs_rect.right())))
    {
        insertion_bar(&child, bx, full, t);
    } else if state.dock_slot.is_some() && n == 0 {
        insertion_bar(&child, tabs_rect.left() + 1.0, full, t);
    }
    state.layout = Some((area(tabs_rect), spans));
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

/// The 2 px orange bar where a dragged tab or window will land.
fn insertion_bar(ui: &Ui, x: f32, full: Rect, t: &Tokens) {
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(x - 1.0..=x + 1.0, full.y_range()),
        0.0,
        c(t.accent),
    );
}

/// A tab drag in progress: the bar while it reorders; past [`drag::TEAR_OFF`] the window code
/// shows the ghost card ([`StripState::ghost`]). Letting go reorders on the strip and pops the
/// page out outside Hideout; anywhere else, Esc, or a button that came up unseen (focus lost)
/// cancels.
fn drag_ui(
    ui: &Ui,
    whole: Rect,
    tabs: &Tabs,
    rects: &[Rect],
    t: &Tokens,
    state: &mut StripState,
    actions: &mut Vec<Action>,
) {
    let Some(from) = state.drag else {
        return;
    };
    let ctx = ui.ctx().clone();
    if let Some(p) = ctx.pointer_latest_pos() {
        state.pointer = Some(p);
    }
    if let Some(p) = state.pointer {
        state.zone = drag::zone(area(whole), window_area(&ctx), (p.x, p.y));
    }
    let to = state.pointer.map_or(from, |p| drop_index(rects, p.x, from));
    if state.zone == drag::Zone::Strip && state.pointer.is_some() {
        let bar = if to <= from {
            rects.get(to).map(|r| r.left())
        } else {
            rects.get(to).map(|r| r.right())
        };
        if let Some(bx) = bar {
            insertion_bar(ui, bx, whole, t);
        }
    }
    let (released, down, esc) = ctx.input_mut(|i| {
        (
            i.pointer.any_released(),
            i.pointer.any_down(),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if !(released || esc || !down) {
        return;
    }
    let popped = tabs.tabs().get(from).is_some_and(|tab| tab.popped);
    match drag::drop(from, state.zone, to, esc || !released, popped) {
        drag::Drop::Move { from, to } => actions.push(Action::Move { from, to }),
        drag::Drop::PopOut(i) => actions.push(Action::TearOff(i)),
        drag::Drop::Nothing => {}
    }
    state.drag = None;
    state.zone = drag::Zone::Strip;
    state.pointer = None;
    // One more frame takes the ghost away.
    ctx.request_repaint();
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
    // The icon slot: the page's icon, or the red warning mark in its place when stopped
    // (tabs plan: "Red warning mark replaces the icon").
    let mc = Pos2::new(lx + 7.0, mid);
    if shown == Shown::Stopped {
        p.text(
            mc,
            Align2::CENTER_CENTER,
            "\u{26a0}",
            FontId::proportional(14.0),
            c(t.danger),
        );
    } else {
        p.text(
            mc,
            Align2::CENTER_CENTER,
            l.icon,
            FontId::proportional(14.0),
            fg,
        );
    }
    if l.busy() {
        // A thin ring round the icon: "working on it".
        let ring = Stroke::new(1.5_f32, c(t.accent));
        p.add(egui::Shape::line(ring_points(mc, 9.5), ring));
    }
    if shown == Shown::Attention {
        // The orange dot on the icon's top-right corner, ringed in the tab's fill so it reads
        // on any glyph.
        let dot = mc + Vec2::new(6.0, -6.0);
        p.circle_filled(dot, 4.5, fill);
        p.circle_filled(dot, 3.5, c(t.accent));
    }
    lx += ICON_SLOT;
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
    let mut menu_key = false;
    if resp.has_focus() {
        let keys = ui.ctx().input_mut(|inp| {
            // Shift+F10, or the Menu key (the window reports it as Shift+F10): the tab's menu.
            menu_key = inp.count_and_consume_key(Modifiers::SHIFT, Key::F10) > 0;
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
    // The same menu as a right-click, opened under the tab from the keyboard; its first item
    // takes the focus, and Esc gives the focus back to the tab (as a Windows menu does).
    let popup = egui::Popup::default_response_id(&resp);
    let (focus_first, back) = (id.with("menu-focus"), id.with("menu-return"));
    let mut menu = egui::Popup::context_menu(&resp);
    if menu_key {
        ui.data_mut(|d| {
            d.insert_temp(focus_first, true);
            d.insert_temp(back, true);
        });
        menu = menu.open_memory(egui::SetOpenCommand::Bool(true));
        ui.ctx().request_repaint();
    } else if !egui::Popup::is_id_open(ui.ctx(), popup)
        && ui.data_mut(|d| d.remove_temp::<bool>(back)) == Some(true)
    {
        resp.request_focus();
    }
    if ui.data(|d| d.get_temp::<bool>(back)) == Some(true) {
        // Opened from the keyboard: under the tab, not at the pointer.
        menu = menu.at_position(r.left_bottom());
    }
    menu.show(|ui| {
        let mut first = ui.data_mut(|d| d.remove_temp::<bool>(focus_first)) == Some(true);
        let mut item = |ui: &mut Ui, text: &str, a: Action| {
            let b = ui.button(text);
            if std::mem::take(&mut first) {
                b.request_focus();
            }
            if b.clicked() {
                // A pick moves on (the tab may close or move): the focus isn't given back.
                ui.data_mut(|d| d.remove_temp::<bool>(back));
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
