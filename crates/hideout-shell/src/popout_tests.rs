// SPDX-License-Identifier: MIT
//! Window-free tests of pop-outs (H2.11, `docs/hideout-popouts-ux.md`): popping out and docking
//! back by every way, `ui.visibility` through a pop-out, Close page's ask, the palette, the
//! restore after a start in the tray, `[windows]` in `hideout.toml`, and the review PNGs.

use super::*;

const CHAT: (&str, &str) = ("mod.chat", "chat");

fn chat() -> TabId {
    TabId::panel(CHAT.0, CHAT.1)
}

/// Modules, Chat, Art open (in that order), Chat active, the router's end to read messages.
fn chat_hub(dir: &std::path::Path) -> (Hub, RouterEnd, egui::Context) {
    let (router, feed) = channel();
    let mut hub = hub_in(dir, Some(Box::new(feed)));
    let ctx = ctx();
    router.push(register(CHAT.0, "Chat", CHAT.1, "Chat"));
    router.push(register("mod.art", "Art", "art", "Art"));
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    hub.open(chat());
    hub.open(TabId::panel("mod.art", "art"));
    hub.tabs.activate(1);
    let _ = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    let _ = router.take();
    (hub, router, ctx)
}

fn frame(ctx: &egui::Context, hub: &mut Hub, events: Vec<Event>) {
    let _ = ctx.run(input(1100.0, 640.0, events), |c| hub.frame(c));
}

/// One frame of `id`'s pop-out on its own context, with `events`; what the user did.
fn popout_frame(hub: &mut Hub, id: &TabId, events: Vec<Event>) -> (Vec<PopCmd>, Vec<String>) {
    let pctx = ctx();
    let i = hub.tabs.position(id).unwrap();
    let t = theme::tokens(true);
    let mut cmds = Vec::new();
    // Two frames: a dialog shows from the second one; the input goes to the last.
    let _ = pctx.run(input(640.0, 420.0, vec![]), |c| {
        let _ = hub.popout_window_ui(c, &t, i);
    });
    let out = pctx.run(input(640.0, 420.0, events), |c| {
        cmds = hub.popout_window_ui(c, &t, i).cmds;
    });
    (cmds, drawn(&out))
}

fn saved_windows(dir: &std::path::Path) -> BTreeMap<String, PageWindow> {
    let (prefs, err) = Prefs::load(&dir.join("hideout.toml"));
    assert!(err.is_none(), "{err:?}");
    let (w, errors) = popout::load(&prefs.rest);
    assert!(errors.is_empty(), "{errors:?}");
    w
}

fn cs() -> Modifiers {
    Modifiers::COMMAND | Modifiers::SHIFT
}

#[test]
fn ctrl_shift_o_pops_out_and_docks_back_from_either_window() {
    let dir = temp_dir("pop-keys");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    frame(&ctx, &mut hub, vec![key(Key::O, cs())]);
    assert!(hub.tabs.tabs()[1].popped, "Chat is in its own window");
    assert_eq!(hub.tabs.active(), Some(2), "the tab to its right comes up");
    assert_eq!(hub.popouts.len(), 1);
    assert!(saved_windows(&dir)["mod.chat/chat"].popped_out);
    // Ctrl+Shift+O in the pop-out docks it back as the active tab.
    let (cmds, _) = popout_frame(&mut hub, &chat(), vec![key(Key::O, cs())]);
    assert_eq!(cmds, [PopCmd::DockBack]);
    hub.popout_cmd(&chat(), PopCmd::DockBack);
    assert!(!hub.tabs.tabs()[1].popped);
    assert_eq!(hub.tabs.active(), Some(1));
    assert!(hub.popouts.is_empty());
    assert!(!saved_windows(&dir)["mod.chat/chat"].popped_out);
    // From the main window: Ctrl+Shift+O on the active placeholder docks it back too.
    frame(&ctx, &mut hub, vec![key(Key::O, cs())]);
    hub.tabs.activate(1);
    frame(&ctx, &mut hub, vec![key(Key::O, cs())]);
    assert!(!hub.tabs.tabs()[1].popped);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_menu_and_the_button_pop_out_and_closing_docks_back_quietly() {
    let dir = temp_dir("pop-close");
    let (mut hub, _router, hctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    assert!(hub.tabs.tabs()[1].popped);
    hub.tabs.activate(0);
    frame(&hctx, &mut hub, vec![]);
    // × (a close request) and Ctrl+W both ask for the quiet dock.
    let mut close = input(640.0, 420.0, vec![]);
    close.viewports.insert(
        egui::ViewportId::ROOT,
        egui::ViewportInfo {
            events: vec![egui::ViewportEvent::Close],
            ..Default::default()
        },
    );
    let pctx = ctx();
    let t = theme::tokens(true);
    let mut cmds = Vec::new();
    let _ = pctx.run(close, |c| cmds = hub.popout_window_ui(c, &t, 1).cmds);
    assert_eq!(cmds, [PopCmd::Close]);
    let (cmds, _) = popout_frame(&mut hub, &chat(), vec![key(Key::W, Modifiers::COMMAND)]);
    assert_eq!(cmds, [PopCmd::Close]);
    hub.popout_cmd(&chat(), PopCmd::Close);
    assert!(!hub.tabs.tabs()[1].popped, "back in its tab");
    assert_eq!(
        hub.tabs.active(),
        Some(0),
        "not activated; Hideout left as it is"
    );
    assert!(hub.popouts.is_empty());
    // The ↗ button pops the active tab out.
    hub.tabs.activate(1);
    let mut popped = false;
    for _ in 0..3 {
        let rects = hub.strip_button_click(&hctx);
        popped |= rects;
    }
    assert!(popped && hub.tabs.tabs()[1].popped);
    let _ = std::fs::remove_dir_all(&dir);
}

impl Hub {
    /// Clicks the ↗ button at the strip's right end (1100 px wide window); whether Chat popped.
    fn strip_button_click(&mut self, ctx: &egui::Context) -> bool {
        let at = egui::pos2(1100.0 - strip::POPOUT_W / 2.0, strip::STRIP_H / 2.0);
        let ev = vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ];
        frame(ctx, self, ev);
        let up = vec![Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }];
        frame(ctx, self, up);
        self.tabs
            .position(&chat())
            .is_some_and(|i| self.tabs.tabs()[i].popped)
    }
}

#[test]
fn visibility_through_a_pop_out_and_the_tray() {
    let dir = temp_dir("pop-vis");
    let (mut hub, router, ctx) = chat_hub(&dir);
    frame(&ctx, &mut hub, vec![]);
    let _ = visibility(&router);
    hub.apply(Action::PopOut(1));
    // The window code says it is on screen (the tests draw no second window).
    hub.tabs.set_popout_shown(1, true);
    frame(&ctx, &mut hub, vec![]);
    let v = visibility(&router);
    assert!(v.contains(&("art".into(), Visibility::Visible)), "{v:?}");
    assert!(
        !v.iter().any(|(p, _)| p == "chat"),
        "popping out the active tab sends nothing new: {v:?}"
    );
    // Hideout to the tray: its tabs are hidden, the pop-out stays visible.
    hub.shared.hidden.store(true, Ordering::Release);
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(visibility(&router), [("art".into(), Visibility::Hidden)]);
    // Minimised: hidden.
    hub.tabs.set_popout_shown(1, false);
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(visibility(&router), [("chat".into(), Visibility::Hidden)]);
    hub.tabs.set_popout_shown(1, true);
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(visibility(&router), [("chat".into(), Visibility::Visible)]);
    // × docks back quietly while Hideout is in the tray: hidden, never closed.
    hub.popout_cmd(&chat(), PopCmd::Close);
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(visibility(&router), [("chat".into(), Visibility::Hidden)]);
    assert!(
        hub.shared.hidden.load(Ordering::Acquire),
        "Hideout stays in the tray"
    );
    // Out again; Dock back shows Hideout and lands as the active tab: visible.
    hub.apply(Action::PopOut(1));
    hub.tabs.set_popout_shown(1, true);
    frame(&ctx, &mut hub, vec![]);
    let _ = visibility(&router);
    hub.popout_cmd(&chat(), PopCmd::DockBack);
    assert!(
        !hub.shared.hidden.load(Ordering::Acquire),
        "Hideout came forward"
    );
    frame(&ctx, &mut hub, vec![]);
    // It was on screen in its window and is on screen as the active tab: nothing new, and
    // never `closed`.
    assert_eq!(hub.tabs.active_id(), Some(&chat()));
    assert_eq!(visibility(&router), []);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn close_page_asks_only_with_unsaved_work_and_sends_closed() {
    let dir = temp_dir("pop-closepage");
    let (mut hub, router, ctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    hub.tabs.set_popout_shown(1, true);
    frame(&ctx, &mut hub, vec![]);
    let _ = visibility(&router);
    badge(&mut hub, CHAT.0, CHAT.1, Badge::Dirty, None);
    hub.popout_cmd(&chat(), PopCmd::ClosePage);
    assert_eq!(hub.popout_asking, Some(chat()), "asks first");
    let (_, texts) = popout_frame(&mut hub, &chat(), vec![]);
    assert!(texts.iter().any(|s| s == "Close Chat?"), "{texts:?}");
    assert!(texts
        .iter()
        .any(|s| s == "Your changes in Chat aren't saved."));
    // Esc keeps it open (the default).
    let (cmds, _) = popout_frame(&mut hub, &chat(), vec![key(Key::Escape, Modifiers::NONE)]);
    assert_eq!(cmds, [PopCmd::KeepOpen]);
    hub.popout_cmd(&chat(), PopCmd::KeepOpen);
    assert!(hub.tabs.is_open(&chat()));
    hub.popout_cmd(&chat(), PopCmd::ClosePage);
    hub.popout_cmd(&chat(), PopCmd::CloseAnyway);
    assert!(!hub.tabs.is_open(&chat()));
    assert!(hub.popouts.is_empty());
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(visibility(&router), [("chat".into(), Visibility::Closed)]);
    // Without unsaved work Close page closes at once.
    hub.open(chat());
    let i = hub.tabs.position(&chat()).unwrap();
    hub.apply(Action::PopOut(i));
    badge(&mut hub, CHAT.0, CHAT.1, Badge::None, None);
    hub.popout_cmd(&chat(), PopCmd::ClosePage);
    assert!(hub.popout_asking.is_none() && !hub.tabs.is_open(&chat()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_palette_pops_out_a_page_that_isnt_open_yet() {
    let dir = temp_dir("pop-palette");
    let (mut hub, _router, ctx) = chat_hub(&dir);
    hub.tabs.close_now(&[TabId::panel("mod.art", "art")]);
    let (entries, _) = hub.palette_entries();
    let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
    assert!(labels.contains(&"Pop out Art"), "{labels:?}");
    assert!(labels.contains(&"Pop out Modules"));
    hub.palette.toggle();
    hub.palette.query = "pop out art".into();
    frame(&ctx, &mut hub, vec![key(Key::Enter, Modifiers::NONE)]);
    let art = TabId::panel("mod.art", "art");
    let i = hub
        .tabs
        .position(&art)
        .expect("Art has a tab (its placeholder)");
    assert!(hub.tabs.tabs()[i].popped);
    assert!(!hub.palette.open);
    // A page out already isn't offered again.
    let (entries, _) = hub.palette_entries();
    assert!(!entries.iter().any(|e| e.label == "Pop out Art"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pop_outs_come_back_at_once_after_a_restart_and_wait_after_a_tray_start() {
    let dir = temp_dir("pop-restore");
    {
        let (mut hub, _router, ctx) = chat_hub(&dir);
        hub.apply(Action::PopOut(1));
        frame(&ctx, &mut hub, vec![]);
    }
    // A hub restart (a crash, a normal open): the pop-out opens on the first frame.
    let ctx = ctx();
    let mut hub = hub_in(&dir, None);
    assert!(hub.tabs.tabs()[1].popped, "restored as its placeholder");
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(hub.popouts.len(), 1);
    assert_eq!(hub.popouts[0].id, chat());
    assert!(!hub.popouts[0].focus, "a restore never takes the focus");
    // Started in the tray at sign-in: it waits until Hideout is first shown.
    let mut hub = hub_in(&dir, None);
    hub.popouts_held = true;
    hub.shared.hidden.store(true, Ordering::Release);
    for _ in 0..3 {
        frame(&ctx, &mut hub, vec![]);
    }
    assert!(
        hub.popouts.is_empty(),
        "no window while Hideout is in the tray"
    );
    assert!(hub.tabs.tabs()[1].popped, "a placeholder meanwhile");
    hub.show_main();
    frame(&ctx, &mut hub, vec![]);
    assert_eq!(hub.popouts.len(), 1, "back with Hideout's first open");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn keep_on_top_is_off_by_default_and_remembered_per_page() {
    let dir = temp_dir("pop-top");
    let (mut hub, _router, _ctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    assert!(!hub.keep_on_top(&chat()));
    let (_, texts) = popout_frame(&mut hub, &chat(), vec![]);
    assert!(texts.iter().any(|s| s == "Chat"), "{texts:?}");
    assert!(texts.iter().any(|s| s.contains("Dock back")));
    assert!(
        !texts.iter().any(|s| s.contains("mod.") || s.contains('/')),
        "no ids in a pop-out: {texts:?}"
    );
    hub.popout_cmd(&chat(), PopCmd::ToggleTop);
    assert!(hub.keep_on_top(&chat()));
    let saved = saved_windows(&dir);
    assert!(saved["mod.chat/chat"].keep_on_top && saved["mod.chat/chat"].popped_out);
    // Docked and out again: still on, for that page only.
    hub.popout_cmd(&chat(), PopCmd::DockBack);
    hub.apply(Action::PopOut(1));
    assert!(hub.keep_on_top(&chat()));
    assert!(!hub.keep_on_top(&TabId::panel("mod.art", "art")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pop_out_shows_the_crash_bar_and_counts_down_while_the_hub_is_in_the_tray() {
    let dir = temp_dir("pop-crash");
    let (mut hub, _asked) = restarting(&dir, 4, 2, Duration::from_millis(200));
    let i = hub.tabs.position(&TabId::panel(SUP, "art")).unwrap();
    hub.apply(Action::PopOut(i));
    hub.shared.hidden.store(true, Ordering::Release);
    let pctx = ctx();
    let t = theme::tokens(true);
    let _ = pctx.run(input(720.0, 460.0, vec![]), |c| {
        let _ = hub.popout_window_ui(c, &t, i);
    });
    let out = pctx.run(input(720.0, 460.0, vec![]), |c| {
        let _ = hub.popout_window_ui(c, &t, i);
    });
    let texts = drawn(&out);
    assert!(
        texts
            .iter()
            .any(|s| s.starts_with("Art stopped unexpectedly")),
        "{texts:?}"
    );
    let d = repaint_delay(&out);
    assert!(
        d > Duration::from_millis(900) && d <= Duration::from_secs(1),
        "its countdown ticks: {d:?}"
    );
    // The stopped card too, with its one main button.
    let dir2 = temp_dir("pop-stopped");
    let (mut hub, _asked) = stopped(&dir2, "start-again", None);
    let i = hub.tabs.position(&TabId::panel(SUP, "art")).unwrap();
    hub.apply(Action::PopOut(i));
    let out = pctx.run(input(720.0, 460.0, vec![]), |c| {
        let _ = hub.popout_window_ui(c, &t, i);
    });
    assert!(drawn(&out).iter().any(|s| s == "Start again"));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[test]
fn a_bad_windows_entry_is_dropped_and_the_file_kept() {
    let dir = temp_dir("pop-bad");
    std::fs::create_dir_all(&dir).unwrap();
    let text = "theme = \"dark\"\n[windows.\"mod.chat/chat\"]\npopped_out = true\nkeep_on_top = true\n\n[windows.\"mod.art/art\"]\nplaces = 3\n";
    let (prefs, err) = Prefs::load(&{
        let f = dir.join("hideout.toml");
        std::fs::write(&f, text).unwrap();
        f
    });
    assert!(err.is_none(), "the file still reads: {err:?}");
    let (w, errors) = popout::load(&prefs.rest);
    assert_eq!(w.len(), 1);
    assert!(w["mod.chat/chat"].keep_on_top);
    assert_eq!(errors.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Review PNGs (HIDEOUT_REVIEW_DIR) ------------------------------------------------------

/// Windows' own caption over a rendered pop-out, so the review reads as a window.
fn caption(ctx: &egui::Context, title: &str) {
    egui::TopBottomPanel::top("review.caption")
        .exact_height(30.0)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(Color32::from_rgb(0x20, 0x20, 0x20))
                .inner_margin(egui::Margin::symmetric(10, 0)),
        )
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(12.0)
                        .color(Color32::from_rgb(0xe6, 0xe6, 0xe6)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("\u{2014}      \u{25a1}      \u{2715}")
                            .size(12.0)
                            .color(Color32::from_rgb(0xe6, 0xe6, 0xe6)),
                    );
                });
            });
        });
}

/// A pop-out window of `hub`'s tab `id`, with its caption, `events` per frame.
fn render_popout(
    hub: &mut Hub,
    id: &TabId,
    w: u32,
    h: u32,
    events: Vec<Vec<Event>>,
) -> Option<Vec<u8>> {
    let i = hub.tabs.position(id)?;
    let t = theme::tokens(true);
    let title = window_title(&hub.look(i).title);
    rasterise_events(w, h, events, |c| {
        caption(c, &title);
        let _ = hub.popout_window_ui(c, &t, i);
    })
}

/// Copies `src` (w × h) onto `dst` (dw wide) at (x, y), with a 1 px window border.
fn blit(dst: &mut [u8], dw: u32, src: &[u8], (w, h): (u32, u32), (x, y): (u32, u32)) {
    for r in 0..h {
        for col in 0..w {
            let s = ((r * w + col) * 4) as usize;
            let d = (((y + r) * dw + x + col) * 4) as usize;
            let edge = r == 0 || col == 0 || r == h - 1 || col == w - 1;
            let px = if edge {
                [0x55, 0x55, 0x55, 0xff]
            } else {
                [src[s], src[s + 1], src[s + 2], 0xff]
            };
            dst[d..d + 4].copy_from_slice(&px);
        }
    }
}

#[test]
fn renders_the_pop_outs_for_review() {
    // 1. Hideout with two placeholder tabs (Chat and Images) and their two pop-outs, Images
    //    kept on top.
    let dir = temp_dir("png-pop");
    let mut hub = mock_hub(&dir);
    for id in [chat(), TabId::panel("mod.images", "images")] {
        let i = hub.tabs.position(&id).unwrap();
        hub.tabs.dock_quiet(i);
        hub.apply(Action::PopOut(i));
    }
    hub.toggle_keep_on_top(&TabId::panel("mod.images", "images"));
    let lights = hub
        .tabs
        .position(&TabId::panel("mod.lights", "lights"))
        .unwrap();
    hub.tabs.activate(lights);
    let Some(main) = render(&mut hub, 1100, 600, |_| {}) else {
        eprintln!("no system font; pop-out review skipped");
        return;
    };
    let chat_win = render_popout(&mut hub, &chat(), 640, 430, vec![]).unwrap();
    let images = TabId::panel("mod.images", "images");
    let images_win = render_popout(&mut hub, &images, 600, 400, vec![]).unwrap();
    let (cw, ch) = (1760u32, 1060u32);
    let mut canvas: Vec<u8> = [0x18u8, 0x1c, 0x22, 0xff].repeat((cw * ch) as usize);
    blit(&mut canvas, cw, &main, (1100, 600), (30, 30));
    blit(&mut canvas, cw, &chat_win, (640, 430), (1090, 70));
    blit(&mut canvas, cw, &images_win, (600, 400), (1120, 620));
    save_png("built-popouts-hub-two-windows.png", cw, ch, canvas);

    // 2. A pop-out while its module restarts: the crash bar, the page kept below.
    let dir2 = temp_dir("png-pop-crash");
    let (mut hub, _asked) = restarting(&dir2, 4, 2, Duration::from_millis(200));
    let art = TabId::panel(SUP, "art");
    let i = hub.tabs.position(&art).unwrap();
    hub.apply(Action::PopOut(i));
    let img = render_popout(&mut hub, &art, 720, 460, vec![]).unwrap();
    save_png("built-popout-crash-bar.png", 720, 460, img);

    // 3. The More menu open (a click on …).
    let dir3 = temp_dir("png-pop-more");
    let (mut hub, _router, _ctx) = chat_hub(&dir3);
    hub.apply(Action::PopOut(1));
    let probe = egui::Context::default();
    install_fonts(&probe);
    set_theme(&probe, ThemeChoice::Dark);
    let t = theme::tokens(true);
    let title = window_title("Chat");
    let mut out = None;
    for _ in 0..2 {
        out = Some(probe.run(input(640.0, 430.0, vec![]), |c| {
            caption(c, &title);
            let _ = hub.popout_window_ui(c, &t, 1);
        }));
    }
    let x = text_x(out.as_ref().unwrap(), "\u{2026}").expect("the More button");
    let at = egui::pos2(x + 4.0, 30.0 + POPOUT_TITLE_H / 2.0);
    let press = |down: bool| Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: down,
        modifiers: Modifiers::NONE,
    };
    let events = vec![
        vec![Event::PointerMoved(at)],
        vec![press(true)],
        vec![press(false)],
        vec![],
        vec![],
    ];
    let img = render_popout(&mut hub, &chat(), 640, 430, events).unwrap();
    save_png("built-popout-more-menu.png", 640, 430, img);
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[test]
fn an_idle_pop_out_asks_for_no_frame_and_held_ones_cost_nothing() {
    let dir = temp_dir("pop-idle");
    let (mut hub, _router, hctx) = chat_hub(&dir);
    hub.apply(Action::PopOut(1));
    let pctx = ctx();
    let t = theme::tokens(true);
    let mut last = None;
    for _ in 0..3 {
        last = Some(pctx.run(input(640.0, 420.0, vec![]), |c| {
            let _ = hub.popout_window_ui(c, &t, 1);
        }));
    }
    assert_eq!(
        repaint_delay(last.as_ref().unwrap()),
        Duration::MAX,
        "a still page draws no frame (no polling)"
    );
    // Hidden in the tray with its pop-out waiting: the hub's frames ask for nothing either.
    let mut hub = hub_in(&dir, None);
    hub.popouts_held = true;
    hub.shared.hidden.store(true, Ordering::Release);
    let mut out = None;
    for _ in 0..3 {
        out = Some(hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c)));
    }
    assert!(hub.popouts.is_empty());
    assert_eq!(repaint_delay(out.as_ref().unwrap()), Duration::MAX);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- #1012: the tray start, announcements, focus, F6 and the page icon -----------------------

#[test]
fn a_tray_start_never_shows_the_main_viewport() {
    // Created hidden; `platform::hold_hidden` keeps eframe's show after the first frame off
    // (its own test shows a held window stays hidden), and only a real show lets go.
    assert_eq!(main_viewport("Modules", true).visible, Some(false));
    assert_eq!(
        main_viewport("Modules", false).visible,
        None,
        "a normal start as before"
    );
}

/// The live (announced) nodes' words in a frame's AccessKit tree.
fn live_words(out: &egui::FullOutput) -> Vec<String> {
    out.platform_output
        .accesskit_update
        .as_ref()
        .map(|u| {
            u.nodes
                .iter()
                .filter(|(_, n)| n.live().is_some_and(|l| l != egui::accesskit::Live::Off))
                .map(|(_, n)| n.label().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The label of the node that has the focus in a frame's AccessKit tree.
fn focus_label(out: &egui::FullOutput) -> Option<String> {
    let u = out.platform_output.accesskit_update.as_ref()?;
    u.nodes
        .iter()
        .find(|(id, _)| *id == u.focus)
        .and_then(|(_, n)| n.label().map(str::to_string))
}

fn a11y_ctx() -> egui::Context {
    let c = ctx();
    c.enable_accesskit();
    c
}

/// One frame of `id`'s pop-out on `pctx`.
fn popout_out(
    pctx: &egui::Context,
    hub: &mut Hub,
    id: &TabId,
    events: Vec<Event>,
) -> egui::FullOutput {
    let i = hub.tabs.position(id).unwrap();
    let t = theme::tokens(true);
    pctx.run(input(640.0, 420.0, events), |c| {
        let _ = hub.popout_window_ui(c, &t, i);
    })
}

#[test]
fn popping_out_and_docking_back_are_announced_once_politely() {
    let dir = temp_dir("pop-say");
    let (mut hub, _router, hctx) = chat_hub(&dir);
    hctx.enable_accesskit();
    let pctx = a11y_ctx();
    hub.apply(Action::PopOut(1));
    let out = popout_out(&pctx, &mut hub, &chat(), vec![]);
    assert_eq!(live_words(&out), ["Chat opened in its own window"]);
    let main = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert!(
        live_words(&main).is_empty(),
        "said in the window the page went to"
    );
    // Docked back: said in Hideout, with the tab's place.
    hub.popout_cmd(&chat(), PopCmd::DockBack);
    let main = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(live_words(&main), ["Chat docked, tab 2 of 3"]);
    // Once: stale, it leaves the tree with the next frame (none is asked for to drop it).
    // The dock's own frames (activation, focus) settle; the announcement asks for none.
    let main = frames(&hctx, &mut hub, vec![]);
    assert_eq!(repaint_delay(&main), Duration::MAX, "no frame asked for");
    assert!(hub.announce.is_some(), "still fresh");
    hub.announce.as_mut().unwrap().at -= ANNOUNCE_FOR;
    let main = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert!(live_words(&main).is_empty());
    assert!(hub.announce.is_none());
    // The close button docks back quietly, and still says where the page went, as a new node.
    hub.apply(Action::PopOut(1));
    let n = hub.announce.as_ref().unwrap().n;
    hub.popout_cmd(&chat(), PopCmd::Close);
    let a = hub.announce.as_ref().unwrap();
    assert_eq!(a.text, "Chat docked, tab 2 of 3");
    assert!(a.n > n, "a new node, so it is said again");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A hub page with two buttons, Send and Keep (focusable elements of the page).
struct FormPage;

impl Page for FormPage {
    fn id(&self) -> &str {
        "form"
    }
    fn title(&self) -> &str {
        "Form"
    }
    fn icon(&self) -> &str {
        "chat"
    }
    fn ui(&mut self, ui: &mut egui::Ui, _t: &Tokens) {
        let _ = ui.button("Send");
        let _ = ui.button("Keep");
    }
}

fn form_hub(dir: &std::path::Path) -> Hub {
    let root = dir.as_os_str().to_owned();
    let paths = HubPaths::resolve(|k| (k == "HIDEOUT_CONFIG_DIR").then(|| root.clone())).unwrap();
    let shared = Arc::new(Shared {
        paths,
        prefs: Mutex::new(Prefs::default()),
        paused: AtomicBool::new(false),
        hidden: AtomicBool::new(false),
        flyout_click: Mutex::new(None),
    });
    let pages: Vec<Box<dyn Page>> = vec![
        Box::new(FormPage),
        Box::new(FakePage("settings", "Settings")),
    ];
    Hub::new(shared, pages, None, None)
}

/// Tab presses on `ctx` until the focus is on `label`.
fn tab_to(ctx: &egui::Context, hub: &mut Hub, label: &str) {
    for _ in 0..16 {
        let tab = vec![key(Key::Tab, Modifiers::NONE)];
        let _ = ctx.run(input(1100.0, 640.0, tab), |c| hub.frame(c));
        let out = ctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
        if focus_label(&out).as_deref() == Some(label) {
            return;
        }
    }
    panic!("Tab never reached {label}");
}

#[test]
fn the_focus_goes_with_the_page_and_comes_back_on_dock() {
    let dir = temp_dir("pop-focus");
    let mut hub = form_hub(&dir);
    let form = TabId::page("form");
    let hctx = a11y_ctx();
    let _ = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    tab_to(&hctx, &mut hub, "Keep");
    // Ctrl+Shift+O: the pop-out opens with Keep focused, as it was in Hideout.
    let _ = hctx.run(input(1100.0, 640.0, vec![key(Key::O, cs())]), |c| {
        hub.frame(c)
    });
    assert!(hub.tabs.tabs()[0].popped);
    let pctx = a11y_ctx();
    let _ = popout_out(&pctx, &mut hub, &form, vec![]);
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Keep"));
    // Shift+Tab to Send in the pop-out, then Dock back: Send has the focus in Hideout.
    let back = vec![key(Key::Tab, Modifiers::SHIFT)];
    let _ = popout_out(&pctx, &mut hub, &form, back);
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Send"));
    hub.popout_cmd(&form, PopCmd::DockBack);
    let _ = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    let out = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    assert_eq!(focus_label(&out).as_deref(), Some("Send"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn f6_moves_between_the_title_row_and_the_page() {
    let dir = temp_dir("pop-f6");
    let mut hub = form_hub(&dir);
    let form = TabId::page("form");
    let hctx = ctx();
    let _ = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    hub.apply(Action::PopOut(0));
    let pctx = a11y_ctx();
    let f6 = || vec![key(Key::F6, Modifiers::NONE)];
    let _ = popout_out(&pctx, &mut hub, &form, vec![]);
    // Nothing focused: F6 goes to the title row's first element, Keep on top.
    let _ = popout_out(&pctx, &mut hub, &form, f6());
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Keep on top"));
    // From the title row: the page's first element (none had the focus there yet).
    let _ = popout_out(&pctx, &mut hub, &form, f6());
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Send"));
    // Tab on to Keep, Shift+F6 up to the title row, F6 back down to Keep (where it was).
    let _ = popout_out(&pctx, &mut hub, &form, vec![key(Key::Tab, Modifiers::NONE)]);
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Keep"));
    let _ = popout_out(&pctx, &mut hub, &form, vec![key(Key::F6, Modifiers::SHIFT)]);
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Keep on top"));
    let _ = popout_out(&pctx, &mut hub, &form, f6());
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    assert_eq!(focus_label(&out).as_deref(), Some("Keep"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_title_row_draws_the_page_icon_before_the_title() {
    let dir = temp_dir("pop-icon");
    let mut hub = form_hub(&dir);
    let form = TabId::page("form");
    let hctx = ctx();
    let _ = hctx.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    hub.apply(Action::PopOut(0));
    let pctx = ctx();
    let _ = popout_out(&pctx, &mut hub, &form, vec![]);
    let out = popout_out(&pctx, &mut hub, &form, vec![]);
    let icon = hideout_panels::tray::icon_glyph("chat");
    let (ix, tx) = (text_x(&out, icon), text_x(&out, "Form"));
    assert!(ix.is_some() && tx.is_some(), "{:?}", drawn(&out));
    assert!(ix < tx, "icon, then title");
    // A module page uses its panel's icon (an unknown name: the generic one).
    let (hub2, _r, _c) = chat_hub(&dir.join("m"));
    assert_eq!(
        hub2.page_icon(&chat()),
        hideout_panels::tray::icon_glyph("x")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `register` with a published icon name.
fn register_icon(module: &str, name: &str, panel: &str, title: &str, icon: &str) -> Inbound {
    match register(module, name, panel, title) {
        Inbound::Register {
            call,
            module,
            name,
            declared,
            actions,
            params,
        } => {
            let mut v = serde_json::to_value(&params).unwrap();
            v["panels"][0]["icon"] = serde_json::json!(icon);
            Inbound::Register {
                call,
                module,
                name,
                declared,
                actions,
                params: serde_json::from_value(v).unwrap(),
            }
        }
        _ => unreachable!(),
    }
}

#[test]
fn renders_the_title_row_icons_for_review() {
    // Two pop-outs' title rows (#1012): the page icon before the title; Images kept on top.
    let dir = temp_dir("png-pop-icons");
    let (router, feed) = channel();
    let mut hub = hub_in(&dir, Some(Box::new(feed)));
    router.push(register_icon(CHAT.0, "Chat", CHAT.1, "Chat", "chat"));
    router.push(register_icon(
        "mod.images",
        "Images",
        "images",
        "Images",
        "image",
    ));
    let probe = ctx();
    let _ = probe.run(input(1100.0, 640.0, vec![]), |c| hub.frame(c));
    let images = TabId::panel("mod.images", "images");
    for id in [chat(), images.clone()] {
        hub.open(id.clone());
        let i = hub.tabs.position(&id).unwrap();
        hub.apply(Action::PopOut(i));
    }
    hub.toggle_keep_on_top(&images);
    let (w, h) = (640u32, 160u32);
    let Some(chat_win) = render_popout(&mut hub, &chat(), w, h, vec![]) else {
        eprintln!("no system font; title row review skipped");
        return;
    };
    let images_win = render_popout(&mut hub, &images, w, h, vec![]).unwrap();
    let (cw, ch) = (w + 60, 2 * h + 90);
    let mut canvas: Vec<u8> = [0x18u8, 0x1c, 0x22, 0xff].repeat((cw * ch) as usize);
    blit(&mut canvas, cw, &chat_win, (w, h), (30, 30));
    blit(&mut canvas, cw, &images_win, (w, h), (30, h + 60));
    save_png("built-popout-title-row-icons.png", cw, ch, canvas);
    let _ = std::fs::remove_dir_all(&dir);
}
