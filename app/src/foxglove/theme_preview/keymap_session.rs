//! The keymap editor's Studio session (#578), end to end and window-free: the real editor and
//! `Keyboard` over a fake Studio board (`foxglove::studio_fake`), so no keyboard is written.
//! Each desk's board runs: the second desk's Plank 3 (one piece, 61 keys, Studio locking on)
//! and the user's Corne (42 keys, locking off). Previews: `ui-keymap-session-*.png`.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use egui::{Color32, Pos2, Rect};
use zmk_studio_api::{Behavior, HidUsage};

use super::{keep, preview_ctx, render};
use crate::foxglove::keymap_session::{Lock, Prompt};
use crate::foxglove::studio_fake::{FakeBoard, FakeStudioProtocol, LAYOUT};
use crate::key_action::KeyAction;
use crate::keyboard::{Keyboard, OverlayConfig};
use crate::keymap_editor::{EditTarget, EditorOverlay, EditorState, ZmkSessionState};
use crate::ui_wake::UiWake;

const CONFIG: OverlayConfig = OverlayConfig {
    timeout_ms: -1,
    activation_delay_ms: 0,
    visible_layers: u32::MAX,
    held_layers: u32::MAX,
};

/// The canvas a preview is drawn on: the editor window and the dimmed desktop around it.
const CANVAS: egui::Vec2 = egui::vec2(500.0, 600.0);

struct Bench {
    ctx: egui::Context,
    board: FakeBoard,
    keyboard: Keyboard,
    editor: EditorState,
    wakes: Receiver<()>,
    style: crate::key_paint::KeyPaintStyle,
}

fn key(usage: u32) -> KeyAction {
    KeyAction::Zmk(Behavior::KeyPress(HidUsage::from_encoded(
        0x07_0000 | usage,
    )))
}

fn at(layer_index: usize, col: usize) -> EditTarget {
    EditTarget {
        layer_index,
        row: 0,
        col,
    }
}

impl Bench {
    /// The editor opened on `board`'s key 3 of the base layer.
    fn open(board: FakeBoard) -> Self {
        let (tx, wakes) = mpsc::channel();
        let wake = UiWake::new(Arc::new(move || {
            let _ = tx.send(());
        }));
        let protocol = FakeStudioProtocol::new(&board);
        let keyboard = Keyboard::new(Box::new(protocol), LAYOUT.into(), CONFIG, wake)
            .expect("the fake board connects");
        let settings = crate::settings::Settings::default();
        let style = crate::key_paint::KeyPaintStyle::from_settings(&settings)
            .with_unit(crate::keymap_editor::KEY_UNIT);
        let mut editor = EditorState::new();
        editor.retarget(&keyboard, at(0, 3));
        Self {
            ctx: preview_ctx(),
            board,
            keyboard,
            editor,
            wakes,
            style,
        }
    }

    fn frame(&mut self) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, CANVAS)),
            ..Default::default()
        };
        let (editor, keyboard, style) = (&mut self.editor, &self.keyboard, &self.style);
        let mut out = self
            .ctx
            .run_ui(raw, |ui| editor.draw_window(ui.ctx(), keyboard, style));
        out.textures_delta.clear();
    }

    /// Draws frames until `done` holds; between frames it waits for the board's answer to
    /// wake the UI (with a short fallback: the editor's key writes repaint while pending).
    fn settle(&mut self, what: &str, done: impl Fn(&EditorState) -> bool) {
        for _ in 0..400 {
            self.frame();
            if done(&self.editor) {
                return;
            }
            let _ = self.wakes.recv_timeout(Duration::from_millis(25));
        }
        panic!("the editor never got to: {what}");
    }

    fn idle(&mut self) {
        self.settle("idle", |e| !e.session_busy());
    }

    fn ready(&mut self) {
        self.settle("ready", |e| {
            e.zmk_session == ZmkSessionState::Ready && !e.session_busy()
        });
    }

    fn write(&mut self, target: EditTarget, action: KeyAction) {
        // As a click in the picker does: the key is selected, then written.
        self.editor.retarget(&self.keyboard, target);
        self.editor.apply_write(&self.keyboard, target, action);
        self.idle();
        self.editor.retarget(&self.keyboard, target);
    }

    fn shown(&self, target: EditTarget) -> Option<KeyAction> {
        target.action(&self.keyboard)
    }

    fn png(&mut self, name: &str) -> image::RgbaImage {
        // A fresh context: the rasterizer needs every texture from its first frame.
        let ctx = preview_ctx();
        let (editor, keyboard, style) = (&mut self.editor, &self.keyboard, &self.style);
        let img = render(&ctx, CANVAS, Color32::from_rgb(0x1c, 0x1c, 0x22), |ctx| {
            editor.draw_window(ctx, keyboard, style)
        });
        keep(name, &img);
        img
    }
}

/// The second desk's locked board: the prompt, then the editor once the board says unlocked,
/// with no timer in between.
#[test]
fn a_locked_board_prompts_until_its_unlock_key_is_pressed() {
    let mut bench = Bench::open(FakeBoard::plank3());
    bench.settle("the unlock prompt", |e| e.studio.lock == Lock::Locked);
    assert!(bench.editor.overlay() == Some(EditorOverlay::Session));
    // However long it waits, nothing changes until the board speaks.
    for _ in 0..10 {
        bench.frame();
        let _ = bench.wakes.recv_timeout(Duration::from_millis(30));
    }
    assert_eq!(bench.editor.studio.lock, Lock::Locked);
    assert!(bench.editor.zmk_session == ZmkSessionState::Opening);
    bench.png("ui-keymap-session-unlock.png");
    bench.board.press_unlock_key();
    bench.ready();
    assert_eq!(bench.editor.studio.lock, Lock::Unlocked);
    assert!(bench.editor.overlay().is_none());
    assert!(!bench.editor.zmk_dirty);
}

#[test]
fn undo_and_redo_walk_this_sessions_writes_on_each_desk() {
    for board in [FakeBoard::plank3(), FakeBoard::corne()] {
        board.unlock();
        let mut bench = Bench::open(board);
        bench.ready();
        let (a, b) = (at(0, 3), at(1, 10));
        let (a0, b0) = (bench.shown(a), bench.shown(b));
        bench.write(a, key(0x14));
        bench.write(b, key(0x2c));
        assert!(bench.editor.zmk_dirty);
        assert_eq!(bench.board.live_param(1, 10), 0x07_002c);

        // Undo goes back one write at a time, and the editor shows the key it changed.
        bench.editor.undo(&bench.keyboard);
        bench.idle();
        assert_eq!(bench.shown(b), b0);
        assert_eq!(bench.board.live_param(1, 10), 0, "transparent again");
        assert_eq!(bench.editor.target, Some(b));
        bench.editor.undo(&bench.keyboard);
        bench.idle();
        assert_eq!(bench.shown(a), a0);
        assert_eq!(bench.editor.target, Some(a));
        // Redo walks forward again.
        bench.editor.redo(&bench.keyboard);
        bench.idle();
        assert_eq!(bench.shown(a), Some(key(0x14)));
        assert_eq!(bench.board.live_param(0, 3), 0x07_0014);
        // A new write ends the redo list.
        bench.write(at(0, 4), key(0x1e));
        assert!(bench.editor.studio.history.next_redo().is_none());
    }
}

#[test]
fn save_keeps_changes_and_discard_returns_to_the_saved_keymap() {
    for (i, board) in [FakeBoard::plank3(), FakeBoard::corne()]
        .into_iter()
        .enumerate()
    {
        board.unlock();
        let mut bench = Bench::open(board);
        bench.ready();
        bench.write(at(0, 3), key(0x14));
        if i == 0 {
            bench.png("ui-keymap-session-unsaved.png");
        }
        bench.editor.save(&bench.keyboard);
        bench.idle();
        assert!(!bench.editor.zmk_dirty);
        assert_eq!(bench.board.saved_param(0, 3), 0x07_0014);

        bench.write(at(0, 5), key(0x2c));
        assert!(bench.editor.zmk_dirty);
        bench.editor.discard(&bench.keyboard, false);
        bench.idle();
        assert!(!bench.editor.zmk_dirty);
        assert_eq!(bench.board.live_param(0, 5), bench.board.saved_param(0, 5));
        // The overlay's keymap follows the board, and the history is gone (as in Studio).
        assert_ne!(bench.shown(at(0, 5)), Some(key(0x2c)));
        assert_eq!(bench.shown(at(0, 3)), Some(key(0x14)));
        assert!(bench.editor.studio.history.next_undo().is_none());
    }
}

#[test]
fn opening_shows_changes_left_unsaved_on_the_board() {
    let board = FakeBoard::corne();
    board.leave_unsaved_change(0, 7);
    let mut bench = Bench::open(board);
    bench.ready();
    assert!(bench.editor.zmk_dirty, "check_unsaved_changes on open");
}

#[test]
fn restore_stock_asks_first() {
    let board = FakeBoard::plank3();
    board.unlock();
    let mut bench = Bench::open(board);
    bench.ready();
    let stock = bench.board.saved_param(0, 3);
    bench.write(at(0, 3), key(0x14));
    bench.editor.save(&bench.keyboard);
    bench.idle();

    bench.editor.studio.prompt = Some(Prompt::RestoreStock);
    bench.frame();
    assert!(bench.editor.overlay() == Some(EditorOverlay::Session));
    bench.png("ui-keymap-session-restore.png");
    assert_eq!(bench.board.saved_param(0, 3), 0x07_0014, "nothing yet");
    // Cancel leaves the keyboard alone.
    bench.editor.studio.prompt = None;
    bench.frame();
    assert_eq!(bench.board.saved_param(0, 3), 0x07_0014);

    bench.editor.restore_stock(&bench.keyboard);
    bench.idle();
    assert_eq!(bench.board.saved_param(0, 3), stock);
    assert_eq!(bench.board.live_param(0, 3), stock);
    assert_ne!(bench.shown(at(0, 3)), Some(key(0x14)));
    assert!(!bench.editor.zmk_dirty);
}

#[test]
fn closing_with_unsaved_changes_asks_save_discard_or_keep_editing() {
    for save in [true, false] {
        let board = if save {
            FakeBoard::plank3()
        } else {
            FakeBoard::corne()
        };
        board.unlock();
        let mut bench = Bench::open(board);
        bench.ready();
        let saved = bench.board.saved_param(0, 3);
        bench.write(at(0, 3), key(0x14));
        assert!(!bench.editor.request_close(), "it asks first");
        assert_eq!(bench.editor.studio.prompt, Some(Prompt::Close));
        bench.frame();
        if save {
            bench.png("ui-keymap-session-close.png");
        }
        // Keep editing: the question goes, the editor stays.
        bench.editor.studio.prompt = None;
        bench.frame();
        assert!(bench.editor.target.is_some());

        assert!(!bench.editor.request_close());
        bench.editor.studio.prompt = None;
        if save {
            bench.editor.closing = true; // the prompt's Save
        } else {
            bench.editor.discard(&bench.keyboard, true); // the prompt's Discard
        }
        bench.settle("closed", |e| e.target.is_none());
        let expected = if save { 0x07_0014 } else { saved };
        assert_eq!(bench.board.saved_param(0, 3), expected);
        assert_eq!(bench.board.live_param(0, 3), expected);
    }
}

/// ZMK locks an idle board again: the next write brings the prompt back, and the editor carries
/// on once the board unlocks.
#[test]
fn a_board_that_locks_mid_session_prompts_again() {
    let board = FakeBoard::plank3();
    board.unlock();
    let mut bench = Bench::open(board);
    bench.ready();
    bench.board.lock();
    bench
        .editor
        .apply_write(&bench.keyboard, at(0, 3), key(0x14));
    bench.settle("the unlock prompt", |e| e.studio.lock == Lock::Locked);
    assert!(bench.editor.error.is_none(), "a prompt, not an error");
    bench.board.press_unlock_key();
    bench.settle("unlocked", |e| e.studio.lock == Lock::Unlocked);
    bench.write(at(0, 3), key(0x14));
    assert_eq!(bench.board.live_param(0, 3), 0x07_0014);
}

/// Closing the unlock prompt closes the editor and ends the wait.
#[test]
fn closing_the_unlock_prompt_ends_the_session() {
    let mut bench = Bench::open(FakeBoard::plank3());
    bench.settle("the unlock prompt", |e| e.studio.lock == Lock::Locked);
    assert!(
        bench.editor.request_close(),
        "a locked board closes as it is"
    );
    bench.keyboard.end_edit_session();
    assert!(bench.editor.target.is_none());
}

/// The layout guard (`layout.rs`) on the session bar, alone at the editor window's narrowest
/// (the window is at least 440 points wide; KeyPeek's own panels are not this guard's), and
/// on each prompt over the editor, at every scale.
#[test]
fn the_session_bar_and_prompts_lay_out_without_overlaps() {
    use super::layout::{check, report, SCALES};
    let mut found = Vec::new();
    let mut locked = Bench::open(FakeBoard::plank3());
    locked.settle("the unlock prompt", |e| e.studio.lock == Lock::Locked);
    let board = FakeBoard::plank3();
    board.unlock();
    let mut bench = Bench::open(board);
    bench.ready();
    for dirty in [false, true] {
        if dirty {
            bench.write(at(0, 3), key(0x14));
        }
        let name = if dirty { "bar, unsaved" } else { "bar, saved" };
        for ppp in SCALES {
            let (editor, keyboard) = (&mut bench.editor, &bench.keyboard);
            found.extend(check(
                name,
                egui::vec2(428.0, 40.0),
                ppp,
                false,
                None,
                |ui| editor.draw_session_bar(ui, keyboard),
            ));
        }
    }
    let mut prompts: Vec<(&str, &mut Bench, Option<Prompt>)> = vec![("Unlock", &mut locked, None)];
    prompts.push(("Restore stock?", &mut bench, Some(Prompt::RestoreStock)));
    for (name, bench, prompt) in prompts {
        bench.editor.studio.prompt = prompt;
        for ppp in SCALES {
            let (editor, keyboard, style) = (&mut bench.editor, &bench.keyboard, &bench.style);
            found.extend(check(name, CANVAS, ppp, false, None, |ui| {
                editor.draw_window(ui.ctx(), keyboard, style)
            }));
        }
    }
    bench.editor.studio.prompt = Some(Prompt::Close);
    for ppp in SCALES {
        let (editor, keyboard, style) = (&mut bench.editor, &bench.keyboard, &bench.style);
        found.extend(check(
            "Save your changes?",
            CANVAS,
            ppp,
            false,
            None,
            |ui| editor.draw_window(ui.ctx(), keyboard, style),
        ));
    }
    report(&found);
}

/// F6: what the keymap health rows say now (this test thread's editor noted it).
fn last_change() -> Option<crate::foxglove::keymap_health::Change> {
    crate::foxglove::keymap_health::now().last_change
}

/// F6: a change that fails Foxglove's checks is never sent: the board and the overlay keep
/// what they have, the editor says why, and the health row notes it.
#[test]
fn a_change_that_fails_the_checks_is_never_sent() {
    use crate::foxglove::keymap_health::Change;
    let board = FakeBoard::corne();
    let mut bench = Bench::open(board);
    bench.ready();
    let before = bench.board.live_param(0, 3);
    // A layer the board doesn't have.
    let nowhere = KeyAction::Zmk(Behavior::MomentaryLayer { layer_id: 9 });
    bench.editor.apply_write(&bench.keyboard, at(0, 3), nowhere);
    assert!(!bench.editor.session_busy(), "nothing was sent");
    let error = bench.editor.error.clone().unwrap_or_default();
    assert!(
        error.contains("a layer the keyboard doesn't have"),
        "{error}"
    );
    assert!(matches!(last_change(), Some(Change::Refused(_))));
    // A key that isn't on the layout.
    bench
        .editor
        .apply_write(&bench.keyboard, at(0, 999), key(0x14));
    assert!(!bench.editor.session_busy());
    assert_eq!(bench.board.live_param(0, 3), before);
    assert!(!bench.board.unsaved());
    bench.editor.retarget(&bench.keyboard, at(0, 3));
    bench.editor.apply_write(
        &bench.keyboard,
        at(0, 3),
        KeyAction::Zmk(Behavior::MomentaryLayer { layer_id: 9 }),
    );
    bench.frame();
    bench.png("ui-keymap-f6-refused.png");
}

/// F6: the board refuses a binding and keeps its own; the editor says so, reads the keymap
/// back and shows the board's binding.
#[test]
fn a_binding_the_board_refuses_leaves_the_key_as_it_was() {
    use crate::foxglove::keymap_health::Change;
    for board in [FakeBoard::plank3(), FakeBoard::corne()] {
        board.unlock();
        let mut bench = Bench::open(board);
        bench.ready();
        let before = bench.shown(at(0, 3));
        bench.board.refuse_next_write();
        bench
            .editor
            .apply_write(&bench.keyboard, at(0, 3), key(0x14));
        bench.idle();
        assert_eq!(
            bench.shown(at(0, 3)),
            before,
            "the overlay shows the board's binding"
        );
        assert_ne!(bench.board.live_param(0, 3), 0x07_0014);
        assert!(!bench.editor.zmk_dirty, "nothing changed on the board");
        let error = bench.editor.error.clone().unwrap_or_default();
        assert!(
            error.contains("didn't take that change: the keyboard doesn't accept those values"),
            "{error}"
        );
        assert!(matches!(last_change(), Some(Change::Rejected(_))));
        assert!(bench.editor.studio.history.next_undo().is_none());
    }
}

/// F6: the link breaks during a write, after the board took it: the editor reads the keymap
/// back on a new link and shows what the board has (the new binding, unsaved).
#[test]
fn a_write_whose_answer_is_lost_reads_the_keymap_back() {
    use crate::foxglove::keymap_health::Change;
    let board = FakeBoard::corne();
    let mut bench = Bench::open(board);
    bench.ready();
    bench.board.lose_next_reply();
    bench
        .editor
        .apply_write(&bench.keyboard, at(0, 3), key(0x14));
    bench.idle();
    assert_eq!(bench.board.live_param(0, 3), 0x07_0014, "the board took it");
    assert_eq!(bench.shown(at(0, 3)), Some(key(0x14)), "read back");
    assert!(bench.editor.zmk_dirty, "the board says unsaved");
    assert!(
        bench.board.lines_opened() >= 2,
        "a new link for the read-back"
    );
    let error = bench.editor.error.clone().unwrap_or_default();
    assert!(error.contains("may not have reached"), "{error}");
    assert!(!error.contains("Reading"), "the read-back is done: {error}");
    assert!(matches!(last_change(), Some(Change::LinkFailed(_))));
    assert!(crate::foxglove::keymap_health::now().unsaved);
}

/// F6: a save the board can't do keeps the changes on it, unsaved, and says why; the next
/// save works.
#[test]
fn a_failed_save_keeps_the_changes_and_says_why() {
    use zmk_studio_api::proto::zmk::keymap::SaveChangesErrorCode;
    let board = FakeBoard::plank3();
    board.unlock();
    let mut bench = Bench::open(board);
    bench.ready();
    bench.write(at(0, 3), key(0x14));
    bench
        .board
        .fail_next_save(SaveChangesErrorCode::SaveChangesErrNoSpace);
    bench.editor.save(&bench.keyboard);
    bench.idle();
    let error = bench.editor.error.clone().unwrap_or_default();
    assert!(
        error.contains("Not saved: the keyboard has no room left to save"),
        "{error}"
    );
    assert!(bench.editor.zmk_dirty);
    assert!(bench.board.unsaved());
    assert_eq!(bench.board.live_param(0, 3), 0x07_0014, "still live");
    assert_ne!(bench.board.saved_param(0, 3), 0x07_0014);
    let health = crate::foxglove::keymap_health::now();
    assert_eq!(
        health.last_save,
        Some(Err("the keyboard has no room left to save".into()))
    );
    assert!(health.unsaved);
    bench.png("ui-keymap-f6-save-failed.png");

    bench.editor.save(&bench.keyboard);
    bench.idle();
    assert!(!bench.editor.zmk_dirty);
    assert_eq!(bench.board.saved_param(0, 3), 0x07_0014);
    assert_eq!(
        crate::foxglove::keymap_health::now().last_save,
        Some(Ok(()))
    );
}

/// #582: a behaviour the board defines itself, with a key and a count (`hid_usage` + `range`),
/// is assigned from the Behaviours page's draft; the board stores it, and reading the keymap
/// back (the Studio client's own decoding) gives the same binding.
#[test]
fn a_behaviour_of_the_boards_own_is_assigned_and_round_trips() {
    use crate::foxglove::studio_session::StudioRequest;
    use crate::keymap_editor::{CustomDraft, ZmkDraft};
    for board in [FakeBoard::plank3(), FakeBoard::corne()] {
        board.unlock();
        let mut bench = Bench::open(board);
        bench.ready();
        let catalog = bench.keyboard.custom_behaviors();
        let names: Vec<&str> = catalog.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Profile", "Repeat key"], "read from the board");
        let repeat = catalog.iter().find(|c| c.name == "Repeat key").unwrap();

        let mut draft = CustomDraft::pick(repeat);
        assert_eq!(draft.staged(&catalog), None, "no key yet: nothing to write");
        draft.param1 = Some(0x07_0004);
        draft.param2 = Some(3);
        let staged = draft.staged(&catalog).expect("complete");
        bench.write(at(1, 6), KeyAction::Zmk(staged.clone()));
        assert_eq!(bench.board.live_binding(1, 6), (5, 0x07_0004, 3));
        assert!(bench.editor.zmk_dirty);

        // Read back through the Studio client: the same binding.
        let reply = bench.keyboard.studio(StudioRequest::Reread);
        reply
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(bench.shown(at(1, 6)), Some(KeyAction::Zmk(staged.clone())));
        // The key opens on the Behaviours page with its values.
        bench.editor.retarget(&bench.keyboard, at(1, 6));
        assert_eq!(
            bench.editor.zmk_draft.custom,
            CustomDraft::from_behavior(&staged)
        );

        // Values the board's metadata refuses never stage, and are refused if sent anyway.
        draft.param2 = Some(9);
        assert_eq!(draft.staged(&catalog), None);
        let over = match staged {
            Behavior::Custom {
                behavior_id,
                display_name,
                param1,
                ..
            } => Behavior::Custom {
                behavior_id,
                display_name,
                param1,
                param2: zmk_studio_api::BehaviorParam::Number(9),
            },
            _ => unreachable!(),
        };
        bench
            .editor
            .apply_write(&bench.keyboard, at(1, 7), KeyAction::Zmk(over));
        assert!(!bench.editor.session_busy(), "not sent");
        assert!(bench
            .editor
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("doesn't take those values"));
        assert_eq!(bench.board.live_binding(1, 7).0, 2, "still transparent");
        let _ = ZmkDraft::default();
    }
}

/// #582: the Behaviours page, for review: a key bound to the board's own behaviour, and one
/// whose second value follows its first (two parameter sets).
#[test]
fn the_behaviours_page_renders() {
    use crate::keymap_editor::CustomDraft;
    let board = FakeBoard::corne();
    let mut bench = Bench::open(board);
    bench.ready();
    let catalog = bench.keyboard.custom_behaviors();
    let repeat = catalog.iter().find(|c| c.name == "Repeat key").unwrap();
    let staged = repeat.stage(Some(0x07_0004), Some(3)).unwrap();
    bench.write(at(0, 3), KeyAction::Zmk(staged));
    bench.frame();
    assert!(
        bench.editor.zmk_draft.custom.is_some(),
        "the Behaviours page"
    );
    bench.png("ui-keymap-f6-behaviours.png");

    let profile = catalog.iter().find(|c| c.name == "Profile").unwrap();
    let staged = profile.stage(Some(1), Some(2)).unwrap();
    bench.write(at(0, 4), KeyAction::Zmk(staged));
    bench.frame();
    bench.png("ui-keymap-f6-behaviours-two-sets.png");
    // Picked but incomplete: the page asks for the rest and writes nothing.
    let before = bench.board.live_binding(0, 5);
    bench.editor.retarget(&bench.keyboard, at(0, 5));
    bench.editor.zmk_draft.custom = Some(CustomDraft::pick(repeat));
    bench.frame();
    assert_eq!(bench.board.live_binding(0, 5), before);
    bench.png("ui-keymap-f6-behaviours-pick.png");
}
