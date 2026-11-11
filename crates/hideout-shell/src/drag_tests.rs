// SPDX-License-Identifier: MIT
//! Window-free tests of the drags between windows (#1054.4, `docs/hideout-popouts-ux.md`): a
//! tab dragged on the strip, off it (the ghost) and out of the window (a pop-out); Esc; a
//! placeholder; a pop-out dropped on the strip (through the subclass's stand-in, `PROBE`); and
//! the review PNGs.

use super::popouts::{blit, chat, chat_hub, frame, render_popout};
use super::*;
use crate::drag::{self, WindowDrag};
use crate::shell::drag_windows::{
    ghost_card, ghost_frame, GHOST_ESC, GHOST_LINE, GHOST_LINE_HUB, PROBE,
};
use eframe::egui::Pos2;

const W: f32 = 1100.0;
const H: f32 = 640.0;

fn press(at: Pos2, down: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: down,
        modifiers: Modifiers::NONE,
    }
}

/// The middle of tab `i` as last drawn.
fn centre(hub: &Hub, i: usize) -> Pos2 {
    let (a, s) = hub.strip.layout().expect("the strip was drawn");
    egui::pos2((s[i].left + s[i].right) / 2.0, (a.top + a.bottom) / 2.0)
}

/// The frames of a drag of tab `i` along `path`: press, a first step (egui's drag starts past a
/// few points), then one frame per point. Returns the last frame's drawn strings.
fn drag_tab(ctx: &egui::Context, hub: &mut Hub, i: usize, path: &[Pos2]) -> Vec<String> {
    let a = centre(hub, i);
    frame(ctx, hub, vec![Event::PointerMoved(a), press(a, true)]);
    frame(
        ctx,
        hub,
        vec![Event::PointerMoved(a + egui::vec2(12.0, 0.0))],
    );
    let mut out = Vec::new();
    for &p in path {
        let o = ctx.run(input(W, H, vec![Event::PointerMoved(p)]), |c| hub.frame(c));
        out = drawn(&o);
    }
    out
}

/// One more step of a drag in progress; the frame's drawn strings.
fn step(ctx: &egui::Context, hub: &mut Hub, p: Pos2) -> Vec<String> {
    let o = ctx.run(input(W, H, vec![Event::PointerMoved(p)]), |c| hub.frame(c));
    drawn(&o)
}

fn ids(hub: &Hub) -> Vec<TabId> {
    hub.tabs.tabs().iter().map(|t| t.id.clone()).collect()
}

#[test]
fn a_tab_dragged_along_the_strip_reorders_as_before() {
    let dir = temp_dir("drag-reorder");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    let before = ids(&hub);
    let to = centre(&hub, 2);
    drag_tab(&ctx, &mut hub, 0, &[to]);
    assert_eq!(hub.strip.ghost(), None, "no ghost on the strip");
    frame(&ctx, &mut hub, vec![press(to, false)]);
    assert_eq!(
        ids(&hub),
        [&before[1], &before[2], &before[0]].map(|i| i.clone())
    );
    assert!(hub.popouts.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tab_dragged_out_of_the_window_pops_out_where_it_is_let_go() {
    let dir = temp_dir("drag-tear");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    let i = hub.tabs.position(&chat()).unwrap();
    // Within 24 points below the strip: still a reorder, no ghost.
    let below = centre(&hub, i) + egui::vec2(0.0, 30.0);
    let shown = drag_tab(&ctx, &mut hub, i, &[below]);
    assert_eq!(hub.strip.ghost(), None);
    assert!(!shown.iter().any(|s| s == GHOST_LINE));
    // On, over the page area: the ghost shows, over Hideout.
    step(&ctx, &mut hub, egui::pos2(600.0, 300.0));
    assert_eq!(hub.strip.ghost(), Some((i, true)));
    // (Drawn on top of the window here, as an egui area: shown from its second frame.)
    let shown = step(&ctx, &mut hub, egui::pos2(602.0, 300.0));
    // Over Hideout letting go does nothing: the card says how to pop out instead.
    assert!(shown.iter().any(|s| s == GHOST_LINE_HUB), "{shown:?}");
    assert!(!shown.iter().any(|s| s == GHOST_LINE), "{shown:?}");
    assert!(shown.iter().any(|s| s == GHOST_ESC));
    // On, out of the window: the ghost, not over Hideout; letting go pops Chat out.
    let out = egui::pos2(W + 200.0, 300.0);
    step(&ctx, &mut hub, out);
    assert_eq!(hub.strip.ghost(), Some((i, false)));
    // (Off the canvas here, so read the card as its own window draws it.)
    let words = card_words(&hub, i, false);
    assert!(words.iter().any(|s| s == GHOST_LINE), "{words:?}");
    assert!(!words.iter().any(|s| s == GHOST_LINE_HUB));
    assert!(card_words(&hub, i, true)
        .iter()
        .any(|s| s == GHOST_LINE_HUB));
    frame(&ctx, &mut hub, vec![press(out, false)]);
    assert_eq!(hub.strip.ghost(), None, "the ghost goes with the drag");
    assert!(hub.tabs.tabs()[i].popped, "Chat is in its own window");
    assert_eq!(hub.popouts.len(), 1);
    assert!(saved_popped(&dir, "mod.chat/chat"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The strings the ghost card of tab `i` draws in its own window.
fn card_words(hub: &Hub, i: usize, over_hub: bool) -> Vec<String> {
    let t = theme::tokens(true);
    let look = hub.look(i);
    let out = ctx().run(input(300.0, 92.0, vec![]), |c| {
        egui::CentralPanel::default()
            .frame(ghost_frame(&t, over_hub))
            .show(c, |ui| ghost_card(ui, &t, &look, over_hub));
    });
    drawn(&out)
}

fn saved_popped(dir: &std::path::Path, key: &str) -> bool {
    let (prefs, _) = Prefs::load(&dir.join("hideout.toml"));
    let (w, _) = popout::load(&prefs.rest);
    w.get(key).is_some_and(|p| p.popped_out)
}

#[test]
fn a_torn_off_window_opens_at_the_pointer() {
    let dir = temp_dir("drag-at");
    let (mut hub, _router, _ctx) = chat_hub(&dir);
    hub.tear_off(1, Some((2500, 400)));
    assert!(hub.tabs.tabs()[1].popped);
    assert_eq!(hub.popouts[0].drop_at, Some((2500, 400)));
    // Without a pointer (none on screen) it opens at its usual place.
    let mut hub2 = hub_in(&temp_dir("drag-at-2"), None);
    hub2.open(chat());
    hub2.tear_off(hub2.tabs.position(&chat()).unwrap(), None);
    assert_eq!(hub2.popouts[0].drop_at, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn letting_go_on_the_page_area_or_esc_cancels() {
    let dir = temp_dir("drag-cancel");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    let before = ids(&hub);
    let i = hub.tabs.position(&chat()).unwrap();
    let page = egui::pos2(600.0, 300.0);
    drag_tab(&ctx, &mut hub, i, &[page]);
    frame(&ctx, &mut hub, vec![press(page, false)]);
    assert!(!hub.tabs.tabs()[i].popped);
    assert_eq!(ids(&hub), before);
    // Esc while out of the window: the drag ends there; the later release does nothing.
    let out = egui::pos2(W + 200.0, 300.0);
    drag_tab(&ctx, &mut hub, i, &[out]);
    assert!(hub.strip.ghost().is_some());
    frame(&ctx, &mut hub, vec![key(Key::Escape, Modifiers::NONE)]);
    assert_eq!(hub.strip.ghost(), None);
    frame(&ctx, &mut hub, vec![press(out, false)]);
    assert!(!hub.tabs.tabs()[i].popped);
    assert!(hub.popouts.is_empty());
    assert_eq!(ids(&hub), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_placeholder_dragged_out_gets_no_ghost_and_stays_one_window() {
    let dir = temp_dir("drag-placeholder");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    frame(&ctx, &mut hub, vec![]);
    let out = egui::pos2(W + 200.0, 300.0);
    let shown = drag_tab(&ctx, &mut hub, 1, &[out]);
    assert!(!shown.iter().any(|s| s == GHOST_LINE), "{shown:?}");
    frame(&ctx, &mut hub, vec![press(out, false)]);
    assert!(hub.tabs.tabs()[1].popped);
    assert_eq!(hub.popouts.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn window(hwnd: isize, ended: bool) -> WindowDrag {
    WindowDrag {
        hwnd,
        cursor: (0, 0),
        ended,
    }
}

/// One hub frame with the subclass's stand-in reporting `probe`.
fn probe_frame(ctx: &egui::Context, hub: &mut Hub, probe: Option<(WindowDrag, Option<Pos2>)>) {
    PROBE.with(|p| p.set(probe.map(|(d, at)| (d, at.map(|a| (a.x, a.y))))));
    frame(ctx, hub, vec![]);
    PROBE.with(|p| p.set(None));
}

#[test]
fn a_pop_out_dragged_onto_the_strip_shows_the_bar_and_docks_there() {
    let dir = temp_dir("drag-dock");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    // Modules, Chat, Art: Chat out, its window known by a handle.
    hub.apply(Action::PopOut(1));
    frame(&ctx, &mut hub, vec![]);
    hub.popouts[0].hwnd = Some(4242);
    let (_, spans) = hub.strip.layout().unwrap();
    let end = egui::pos2(spans[2].right - 4.0, 18.0);
    // Moving over the strip: the bar after Art (slot 3), nothing docked yet.
    probe_frame(&ctx, &mut hub, Some((window(4242, false), Some(end))));
    assert_eq!(hub.strip.dock_slot, Some(3));
    assert!(hub.tabs.tabs()[1].popped);
    // Over the page area, or off Hideout: no bar.
    probe_frame(
        &ctx,
        &mut hub,
        Some((window(4242, false), Some(egui::pos2(600.0, 300.0)))),
    );
    assert_eq!(hub.strip.dock_slot, None);
    probe_frame(&ctx, &mut hub, Some((window(4242, false), None)));
    assert_eq!(hub.strip.dock_slot, None);
    // Another window's drag (not a pop-out) is no drop.
    probe_frame(&ctx, &mut hub, Some((window(77, false), Some(end))));
    assert_eq!(hub.strip.dock_slot, None);
    // Let go on the strip: Chat lands after Art, active, its window gone.
    probe_frame(&ctx, &mut hub, Some((window(4242, true), Some(end))));
    assert_eq!(
        ids(&hub),
        [
            TabId::page("modules"),
            TabId::panel("mod.art", "art"),
            chat()
        ]
    );
    assert!(!hub.tabs.tabs()[2].popped);
    assert_eq!(hub.tabs.active(), Some(2));
    assert!(hub.popouts.is_empty());
    assert!(!saved_popped(&dir, "mod.chat/chat"));
    assert_eq!(hub.strip.dock_slot, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pop_out_dropped_on_the_strip_after_esc_stays_out() {
    let dir = temp_dir("drag-dock-esc");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    frame(&ctx, &mut hub, vec![]);
    hub.popouts[0].hwnd = Some(4242);
    let (_, spans) = hub.strip.layout().unwrap();
    let end = egui::pos2(spans[2].right - 4.0, 18.0);
    probe_frame(&ctx, &mut hub, Some((window(4242, false), Some(end))));
    assert_eq!(hub.strip.dock_slot, Some(3));
    // Esc: Windows puts the window back where the move started and still ends the loop with
    // the pointer over the strip; the subclass's end of the move reports no drop.
    let start = placement::Rect {
        x: 900,
        y: 300,
        w: 640,
        h: 480,
    };
    let ended = drag::end_move(window(4242, false), Some(start), Some(start), Some((0, 0)));
    assert_eq!(ended, None);
    probe_frame(&ctx, &mut hub, ended.map(|d| (d, Some(end))));
    assert!(hub.tabs.tabs()[1].popped, "still in its own window");
    assert_eq!(hub.popouts.len(), 1);
    assert_eq!(hub.strip.dock_slot, None, "the bar goes");
    assert!(saved_popped(&dir, "mod.chat/chat"));
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn a_pop_out_let_go_off_the_strip_stays_out() {
    let dir = temp_dir("drag-dock-off");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    frame(&ctx, &mut hub, vec![]);
    hub.popouts[0].hwnd = Some(4242);
    let page = egui::pos2(600.0, 300.0);
    probe_frame(&ctx, &mut hub, Some((window(4242, true), Some(page))));
    assert!(hub.tabs.tabs()[1].popped);
    assert_eq!(hub.popouts.len(), 1);
    probe_frame(&ctx, &mut hub, Some((window(4242, true), None)));
    assert!(hub.tabs.tabs()[1].popped);
    // Dropped before the first tab: Chat goes first.
    let (_, spans) = hub.strip.layout().unwrap();
    let first = egui::pos2(spans[0].left + 4.0, 18.0);
    probe_frame(&ctx, &mut hub, Some((window(4242, true), Some(first))));
    assert_eq!(hub.tabs.position(&chat()), Some(0));
    assert!(!hub.tabs.tabs()[0].popped);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Review PNGs (HIDEOUT_REVIEW_DIR) ------------------------------------------------------

/// The ghost card as its own window draws it.
fn render_ghost(hub: &Hub, i: usize, over_hub: bool) -> Option<Vec<u8>> {
    let t = theme::tokens(true);
    let look = hub.look(i);
    let (w, h) = (drag::GHOST_SIZE.0 as u32, drag::GHOST_SIZE.1 as u32);
    rasterise(w, h, |c| {
        egui::CentralPanel::default()
            .frame(ghost_frame(&t, over_hub))
            .show(c, |ui| ghost_card(ui, &t, &look, over_hub));
    })
}

/// Events for a drag of the tab at `a` to `to`, one frame each.
fn drag_events(a: Pos2, to: Pos2) -> Vec<Vec<Event>> {
    vec![
        vec![],
        vec![],
        vec![Event::PointerMoved(a), press(a, true)],
        vec![Event::PointerMoved(a + egui::vec2(12.0, 0.0))],
        vec![Event::PointerMoved(to)],
        vec![Event::PointerMoved(to)],
    ]
}

#[test]
fn renders_the_drags_for_review() {
    // 1. Tear-off: Chat dragged out of Hideout, the ghost card beside the window.
    let dir = temp_dir("png-drag-tear");
    let mut hub = mock_hub(&dir);
    let chat_i = hub.tabs.position(&chat()).unwrap();
    hub.tabs.activate(chat_i);
    let probe = egui::Context::default();
    install_fonts(&probe);
    set_theme(&probe, ThemeChoice::Dark);
    let _ = probe.run(input(1100.0, 600.0, vec![]), |c| hub.frame(c));
    let a = centre(&hub, chat_i);
    let Some(main) = rasterise_events(1100, 600, drag_events(a, egui::pos2(1400.0, 320.0)), |c| {
        hub.frame(c)
    }) else {
        eprintln!("no system font; drag review skipped");
        return;
    };
    let ghost = render_ghost(&hub, chat_i, false).unwrap();
    let (gw, gh) = (drag::GHOST_SIZE.0 as u32, drag::GHOST_SIZE.1 as u32);
    let (cw, ch) = (1520u32, 660u32);
    let mut canvas: Vec<u8> = [0x18u8, 0x1c, 0x22, 0xff].repeat((cw * ch) as usize);
    blit(&mut canvas, cw, &main, (1100, 600), (30, 30));
    blit(&mut canvas, cw, &ghost, (gw, gh), (1160, 330));
    save_png("built-drag-tear-off.png", cw, ch, canvas);

    // 2. The same drag over Hideout's page area: the card, plain-edged (letting go cancels).
    let dir2 = temp_dir("png-drag-hub");
    let mut hub = mock_hub(&dir2);
    hub.tabs.activate(chat_i);
    let _ = probe.run(input(1100.0, 600.0, vec![]), |c| hub.frame(c));
    let a = centre(&hub, chat_i);
    let img = rasterise_events(1100, 600, drag_events(a, egui::pos2(640.0, 300.0)), |c| {
        hub.frame(c)
    })
    .unwrap();
    save_png("built-drag-ghost-over-hideout.png", 1100, 600, img);

    // 3. Dock by drag: the Images pop-out moved by its title row over the strip, the orange
    //    bar before Lights.
    let dir3 = temp_dir("png-drag-dock");
    let mut hub = mock_hub(&dir3);
    let lights = hub
        .tabs
        .position(&TabId::panel("mod.lights", "lights"))
        .unwrap();
    hub.tabs.activate(lights);
    let _ = probe.run(input(1100.0, 600.0, vec![]), |c| hub.frame(c));
    let images = TabId::panel("mod.images", "images");
    let k = hub.popout_index(&images).expect("Images' window");
    hub.popouts[k].hwnd = Some(4242);
    let (_, spans) = hub.strip.layout().unwrap();
    let at = egui::pos2(spans[lights].left + 6.0, 18.0);
    PROBE.with(|p| p.set(Some((window(4242, false), Some((at.x, at.y))))));
    let main = rasterise(1100, 600, |c| hub.frame(c)).unwrap();
    PROBE.with(|p| p.set(None));
    assert_eq!(hub.strip.dock_slot, Some(lights));
    let win = render_popout(&mut hub, &images, 600, 360, vec![]).unwrap();
    let (cw, ch) = (1160u32, 660u32);
    let mut canvas: Vec<u8> = [0x18u8, 0x1c, 0x22, 0xff].repeat((cw * ch) as usize);
    blit(&mut canvas, cw, &main, (1100, 600), (30, 30));
    // The window's caption just under the pointer, as Windows moves it.
    let (wx, wy) = ((30.0 + at.x - 120.0) as u32, (30.0 + at.y - 12.0) as u32);
    blit(&mut canvas, cw, &win, (600, 360), (wx, wy));
    // The bar's own always-on-top window, above the dragged one.
    let (_, spans) = hub.strip.layout().unwrap();
    let bx = drag::bar_x(spans, lights, (spans[0].left, 1100.0)).unwrap();
    let ((bl, bt), (bw, bh)) = drag::bar_rect((30, 30), bx, (0.0, strip::STRIP_H), 1.0);
    let accent = theme::tokens(true).accent;
    for y in bt..bt + bh {
        for x in bl..bl + bw {
            let d = ((y as u32 * cw + x as u32) * 4) as usize;
            canvas[d..d + 4].copy_from_slice(&[accent.0, accent.1, accent.2, 0xff]);
        }
    }
    save_png("built-drag-dock-bar.png", cw, ch, canvas);
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(&d);
    }
}
