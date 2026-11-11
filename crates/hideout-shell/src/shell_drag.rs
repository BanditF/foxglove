// SPDX-License-Identifier: MIT
//! Drags between Hideout's windows (#1012 item 7, #1054 item 4; `docs/hideout-popouts-ux.md`,
//! "Popping out" and "Docking back"), with Win32 doing what egui 0.33 can't:
//! - **Tear-off:** a tab dragged past [`drag::TEAR_OFF`] from the strip shows a ghost card in an
//!   extra always-on-top viewport that follows the pointer; letting go outside Hideout pops the
//!   page out with its title row under the pointer.
//! - **Dock by drag:** a pop-out moved by its title row reports `WM_MOVING` from its subclass;
//!   over the strip, with Hideout on top there, the strip draws the insertion bar (again in a
//!   thin always-on-top window, as the dragged window covers the strip), and letting go docks
//!   the page at the bar.
//!
//! Event-driven: the pointer moves of a drag bring the frames (egui's own for the tab, the
//! subclass's waker for the window); nothing polls. The keyboard and menu routes are unchanged.
//!
//! Clean-room: written from the pop-outs plan, egui's and winit's public docs and Win32's docs.

use super::*;
use crate::drag;

/// The ghost card's window title: never shown (a tool window with no caption), found by it.
const GHOST_TITLE: &str = "Hideout \u{b7} Let go to open in its own window";
/// The ghost card's words (the plan's wording table).
pub(super) const GHOST_LINE: &str = "Let go to open in its own window";
pub(super) const GHOST_ESC: &str = "Esc cancels";

/// The dock bar's window title: never shown either.
const BAR_TITLE: &str = "Hideout \u{b7} Let go to dock here";

/// One of the drag's always-on-top windows (the ghost card, the dock bar) while it is up.
#[derive(Default)]
pub(super) struct OverlayWin {
    /// Its window, once found by its title (eframe gives an immediate viewport no handle).
    hwnd: Option<isize>,
    /// Frames that couldn't find its window yet.
    misses: u8,
}

/// A window drag and its pointer on the main window's client points, as the tests report it.
#[cfg(test)]
pub(super) type Probe = (drag::WindowDrag, Option<(f32, f32)>);

#[cfg(test)]
thread_local! {
    /// A window drag as the subclass would report it, with its pointer already on the main
    /// window's client points (`None`: not over Hideout).
    pub(super) static PROBE: std::cell::Cell<Option<Probe>> =
        const { std::cell::Cell::new(None) };
}

fn ghost_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("hideout.drag-ghost")
}

fn bar_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("hideout.dock-bar")
}

impl Hub {
    /// A tab let go outside Hideout (`Action::TearOff`): pops it out, its window opening at
    /// `cursor` (screen pixels) instead of its saved place.
    pub(super) fn tear_off(&mut self, i: usize, cursor: Option<(i32, i32)>) {
        let Some(id) = self.tabs.tabs().get(i).map(|t| t.id.clone()) else {
            return;
        };
        self.pop_out(i);
        if let Some(k) = self.popout_index(&id) {
            self.popouts[k].drop_at = cursor;
        }
    }

    /// A pop-out dropped on the strip in insertion slot `slot`: it lands there as the active tab
    /// and its window closes. Its saved place stays the one from before the drag (it was let go
    /// over Hideout, not moved somewhere to keep).
    pub(super) fn dock_at(&mut self, i: usize, slot: usize) {
        let Some(id) = self.tabs.tabs().get(i).map(|t| t.id.clone()) else {
            return;
        };
        if let Some(k) = self.popout_index(&id) {
            self.popouts.remove(k);
        }
        self.tabs.move_tab(i, drag::slot_to_index(slot, i));
        if let Some(n) = self.tabs.position(&id) {
            // No window left to save a place for: `dock` docks, activates and says it.
            self.dock(n, false);
        }
    }

    /// Each frame: a pop-out moved by its title row, from its subclass. Over the strip the bar
    /// shows; the drop docks it.
    pub(super) fn window_drag(&mut self) {
        self.strip.dock_slot = None;
        let found = platform::take_window_drag().map(|d| (d, self.drag_point(d)));
        // Tests stand in for the subclass and the screen (no windows there).
        #[cfg(test)]
        let found = found.or_else(|| PROBE.with(|p| p.get()));
        let Some((d, at)) = found else {
            return;
        };
        let Some(k) = self.popouts.iter().position(|p| p.hwnd == Some(d.hwnd)) else {
            return;
        };
        self.window_dragged(k, at, d.ended);
    }

    /// The dragged window's pointer in the main window's client points, when the user sees
    /// Hideout's main window there (shown, and on top of everything but the dragged window and
    /// the drag's own overlays).
    fn drag_point(&self, d: drag::WindowDrag) -> Option<(f32, f32)> {
        if self.shared.hidden.load(Ordering::Acquire) {
            return None;
        }
        let origin = platform::main_client_origin()?;
        let [ghost, bar] = platform::overlay_hwnds();
        let skip = [d.hwnd, ghost, bar];
        let top = drag::top_at(&platform::window_layers(), d.cursor, &skip)?;
        (top == platform::main_hwnd()).then(|| drag::to_points(d.cursor, origin, self.ppp))
    }

    /// Pop-out `k` dragged with the pointer at `at` (main client points; `None` off Hideout):
    /// the bar while it moves, the dock when it is let go on the strip. Let go anywhere else, it
    /// stays where Windows put it.
    pub(super) fn window_dragged(&mut self, k: usize, at: Option<(f32, f32)>, ended: bool) {
        let slot = at.and_then(|p| {
            self.strip
                .layout()
                .and_then(|(area, spans)| drag::dock_slot(area, spans, p))
        });
        if !ended {
            self.strip.dock_slot = slot;
            return;
        }
        let Some(slot) = slot else {
            return;
        };
        let id = self.popouts[k].id.clone();
        if let Some(i) = self.tabs.position(&id) {
            log::info!("{} docked by a drag", self.look(i).title);
            self.dock_at(i, slot);
        }
    }

    /// The ghost card while a tab is dragged off the strip: an always-on-top window without
    /// focus that follows the pointer. Without a second window (tests) it is drawn on top of
    /// the main window at the pointer.
    pub(super) fn ghost_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        // A placeholder's page is out already: dragging it only reorders, so no card.
        let tabs = &self.tabs;
        let ghost = self
            .strip
            .ghost()
            .filter(|&(i, _)| tabs.tabs().get(i).is_some_and(|t| !t.popped));
        let Some((i, over_hub)) = ghost else {
            if self.ghost.take().is_some() {
                platform::forget_overlay(platform::Overlay::Ghost);
            }
            return;
        };
        let look = self.look(i);
        let skip: Vec<isize> = self.popouts.iter().filter_map(|p| p.hwnd).collect();
        let g = self.ghost.get_or_insert_with(OverlayWin::default);
        let size = [drag::GHOST_SIZE.0, drag::GHOST_SIZE.1];
        let h = overlay(
            ctx,
            g,
            ghost_id(),
            GHOST_TITLE,
            size,
            &skip,
            |vctx, embedded| {
                if embedded {
                    let at = vctx.pointer_latest_pos().unwrap_or_default()
                        - egui::vec2(drag::GHOST_GRAB.0 as f32, drag::GHOST_GRAB.1 as f32);
                    egui::Area::new(egui::Id::new("hideout.drag-ghost"))
                        .order(egui::Order::Tooltip)
                        .fixed_pos(at)
                        .constrain(false)
                        .fade_in(false)
                        .show(vctx, |ui| {
                            ui.set_width(drag::GHOST_SIZE.0);
                            ghost_frame(t, over_hub)
                                .show(ui, |ui| ghost_card(ui, t, &look, over_hub));
                        });
                } else {
                    egui::CentralPanel::default()
                        .frame(ghost_frame(t, over_hub))
                        .show(vctx, |ui| ghost_card(ui, t, &look, over_hub));
                }
            },
        );
        let (Some(h), Some(cursor)) = (h, platform::cursor_pos()) else {
            return;
        };
        let scale = ctx
            .input_for(ghost_id(), |i| i.viewport().native_pixels_per_point)
            .unwrap_or(self.ppp);
        platform::place_overlay(h, drag::ghost_corner(cursor, scale), None);
    }

    /// The insertion bar of a pop-out dragged over the strip, again in an always-on-top window
    /// of its own: the dragged window hangs from the pointer and covers the strip's own bar.
    pub(super) fn dock_bar_ui(&mut self, ctx: &egui::Context, t: &Tokens) {
        let bar = self.strip.dock_slot.and_then(|s| {
            let (a, spans) = self.strip.layout()?;
            let x = drag::bar_x(spans, s, (a.left, a.right)).unwrap_or(a.left + 1.0);
            Some((x, a))
        });
        let Some((x, a)) = bar else {
            if self.dock_bar.take().is_some() {
                platform::forget_overlay(platform::Overlay::Bar);
            }
            return;
        };
        let skip: Vec<isize> = self.popouts.iter().filter_map(|p| p.hwnd).collect();
        let g = self.dock_bar.get_or_insert_with(OverlayWin::default);
        let size = [drag::BAR_W, a.bottom - a.top];
        let h = overlay(
            ctx,
            g,
            bar_id(),
            BAR_TITLE,
            size,
            &skip,
            |vctx, embedded| {
                // In one window (tests) the strip's own bar is the one.
                if !embedded {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(c(t.accent)))
                        .show(vctx, |_| {});
                }
            },
        );
        let (Some(h), Some(origin)) = (h, platform::main_client_origin()) else {
            return;
        };
        let (at, size) = drag::bar_rect(origin, x, (a.top, a.bottom), self.ppp);
        platform::place_overlay(h, at, Some(size));
    }
}

/// One of the drag's always-on-top windows: an immediate viewport without decorations, taskbar
/// button or focus, created hidden and found by its title (eframe gives it no handle). `draw`
/// gets whether it is embedded (no second window: tests). Returns its window once found.
fn overlay(
    ctx: &egui::Context,
    g: &mut OverlayWin,
    id: egui::ViewportId,
    title: &str,
    size: [f32; 2],
    skip: &[isize],
    mut draw: impl FnMut(&egui::Context, bool),
) -> Option<isize> {
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_decorations(false)
        .with_resizable(false)
        .with_taskbar(false)
        .with_always_on_top()
        .with_active(false)
        // Shown once placed (no flash at a default spot).
        .with_visible(false)
        .with_inner_size(size);
    let embedded = ctx.show_viewport_immediate(id, builder, |vctx, class| {
        let embedded = class == egui::ViewportClass::Embedded;
        draw(vctx, embedded);
        embedded
    });
    if embedded {
        return None;
    }
    if let Some(h) = g.hwnd.filter(|&h| platform::window_alive(h)) {
        return Some(h);
    }
    let which = if title == GHOST_TITLE {
        platform::Overlay::Ghost
    } else {
        platform::Overlay::Bar
    };
    match platform::find_overlay(which, title, skip) {
        Some(h) => {
            g.hwnd = Some(h);
            Some(h)
        }
        None => {
            // egui makes the window at the end of this frame: one more frame finds it.
            if g.misses < 3 {
                g.misses += 1;
                ctx.request_repaint();
            }
            None
        }
    }
}

/// The card's frame: orange edge where letting go opens a window, the plain border over Hideout
/// (letting go there does nothing).
pub(super) fn ghost_frame(t: &Tokens, over_hub: bool) -> egui::Frame {
    let edge = if over_hub { t.border } else { t.accent };
    egui::Frame::new()
        .fill(c(t.chrome))
        .stroke(egui::Stroke::new(1.5_f32, c(edge)))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 12,
            bottom: 10,
        })
}

/// The page's icon and title, then "Let go to open in its own window" / "Esc cancels".
pub(super) fn ghost_card(ui: &mut egui::Ui, t: &Tokens, look: &Look, over_hub: bool) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(look.icon).size(15.0).color(c(t.text)));
        ui.label(
            egui::RichText::new(&look.title)
                .size(14.0)
                .strong()
                .color(c(t.text)),
        );
    });
    ui.add_space(6.0);
    let line = if over_hub { t.text_muted } else { t.text };
    ui.label(egui::RichText::new(GHOST_LINE).color(c(line)));
    ui.label(
        egui::RichText::new(GHOST_ESC)
            .size(12.0)
            .color(c(t.text_muted)),
    );
}
