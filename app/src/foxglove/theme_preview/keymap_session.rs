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
