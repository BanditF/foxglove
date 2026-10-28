//! The keymap editor's session controls for Studio boards (#578): Save and Discard, undo and
//! redo, the unsaved-changes state (on close too), the unlock prompt and Restore stock keymap.
//!
//! Emulates ZMK Studio's editor (zmk-studio @ 1bb6bb84a3, Apache-2.0): its header's Undo, Redo,
//! Save, Discard and Restore Stock Settings, `undoRedo.ts` (a new change clears the redo list;
//! Discard and Restore clear both) and `UnlockModal.tsx` (a prompt that waits for the board's
//! unlock notification). The board is the source of truth for "unsaved": Foxglove shows what it
//! reports. Everything here works from what the board reports, for any Studio board.
//!
//! The KeyPeek editor (`crate::keymap_editor`) keeps its window and pickers; it calls in here.

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::RichText;
use egui_phosphor::regular as icon;

use crate::foxglove::studio_session::{Status, StudioRequest, LOCKED_MESSAGE};
use crate::key_action::KeyAction;
use crate::keyboard::Keyboard;
use crate::keymap_editor::{EditTarget, EditorState, PendingKind, ZmkSessionState};
use crate::protocols::WriteSupport;

/// Studio keeps every step; a long session keeps the last this many.
const HISTORY_LIMIT: usize = 200;

/// One write of this session: a key, what it was and what it became.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub target: EditTarget,
    pub before: KeyAction,
    pub after: KeyAction,
}

/// This session's writes, for undo and redo.
#[derive(Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// A new change: it can be undone, and what was undone before can no longer be redone.
    pub fn record(&mut self, edit: Edit) {
        self.undo.push(edit);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn next_undo(&self) -> Option<&Edit> {
        self.undo.last()
    }

    pub fn next_redo(&self) -> Option<&Edit> {
        self.redo.last()
    }

    fn undone(&mut self) {
        if let Some(edit) = self.undo.pop() {
            self.redo.push(edit);
        }
    }

    fn redone(&mut self) {
        if let Some(edit) = self.redo.pop() {
            self.undo.push(edit);
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// Which way a write moves through the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Do,
    Undo,
    Redo,
}

/// What the board said about its lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lock {
    #[default]
    Unknown,
    Locked,
    Unlocked,
}

/// A question the editor is asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    RestoreStock,
    Close,
}

struct Task {
    request: StudioRequest,
    reply: Receiver<Result<Status, String>>,
    then_close: bool,
}

/// The session's state in the editor.
#[derive(Default)]
pub struct KeymapSession {
    pub lock: Lock,
    pub prompt: Option<Prompt>,
    pub history: History,
    writing: Option<(Step, Edit)>,
    task: Option<Task>,
    /// The key an undo or redo just changed, for the editor to show.
    jump: Option<EditTarget>,
}

impl KeymapSession {
    /// Notes a write the user is making, so it can be undone (Studio boards only).
    pub fn begin_edit(&mut self, keyboard: &Keyboard, target: EditTarget, after: &KeyAction) {
        self.writing = None;
        if keyboard.write_support() != WriteSupport::Session {
            return;
        }
        if let Some(before) = target.action(keyboard) {
            self.writing = Some((
                Step::Do,
                Edit {
                    target,
                    before,
                    after: after.clone(),
                },
            ));
        }
    }

    /// The board took the write.
    pub fn write_done(&mut self) {
        match self.writing.take() {
            Some((Step::Do, edit)) if edit.before != edit.after => self.history.record(edit),
            Some((Step::Undo, edit)) => {
                self.history.undone();
                self.jump = Some(edit.target);
            }
            Some((Step::Redo, edit)) => {
                self.history.redone();
                self.jump = Some(edit.target);
            }
            _ => {}
        }
    }

    /// The board refused the write. True when it refused because it is locked: the editor
    /// then shows the unlock prompt instead of the error.
    pub fn write_failed(&mut self, error: &str) -> bool {
        self.writing = None;
        if error == LOCKED_MESSAGE {
            self.lock = Lock::Locked;
            return true;
        }
        false
    }

    /// The key an undo or redo just changed, for the editor to move to.
    pub fn take_jump(&mut self) -> Option<EditTarget> {
        self.jump.take()
    }

    /// The prompt or progress the session puts over the editor, if any.
    pub fn covers_editor(&self) -> bool {
        self.lock == Lock::Locked
            || self.prompt.is_some()
            || self.task.as_ref().is_some_and(|t| {
                matches!(
                    t.request,
                    StudioRequest::Discard | StudioRequest::RestoreStock
                )
            })
    }
}

impl EditorState {
    fn start_studio(&mut self, keyboard: &Keyboard, request: StudioRequest, then_close: bool) {
        self.error = None;
        self.studio.task = Some(Task {
            request,
            reply: keyboard.studio(request),
            then_close,
        });
    }

    /// Opens the Studio session (the window just opened, or Retry).
    pub(crate) fn open_studio(&mut self, keyboard: &Keyboard) {
        self.zmk_session = ZmkSessionState::Opening;
        self.start_studio(keyboard, StudioRequest::Open, false);
    }

    /// True while a write, save or session request is under way.
    pub(crate) fn session_busy(&self) -> bool {
        self.pending.is_some() || self.studio.task.is_some() || self.closing
    }

    /// Applies the board's answers. Each answer wakes the UI, so this needs no repaint loop.
    /// Returns true when the editor closed.
    pub(crate) fn poll_studio(&mut self, keyboard: &Keyboard) -> bool {
        // A write found the board locked: wait for its unlock key.
        if self.studio.lock == Lock::Locked && self.studio.task.is_none() {
            self.start_studio(keyboard, StudioRequest::WaitUnlock, false);
        }
        let Some(task) = &self.studio.task else {
            return false;
        };
        let result = match task.reply.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("Connection lost".into()),
        };
        let Some(task) = self.studio.task.take() else {
            return false;
        };
        match (task.request, result) {
            (StudioRequest::Open | StudioRequest::WaitUnlock, Ok(status)) if status.locked => {
                self.studio.lock = Lock::Locked;
                self.start_studio(keyboard, StudioRequest::WaitUnlock, false);
            }
            (StudioRequest::Open | StudioRequest::WaitUnlock, Ok(status)) => {
                self.studio.lock = Lock::Unlocked;
                self.zmk_session = ZmkSessionState::Ready;
                self.zmk_dirty = status.unsaved;
            }
            (StudioRequest::Open | StudioRequest::WaitUnlock, Err(e)) => {
                self.studio.lock = Lock::Unknown;
                self.zmk_session = ZmkSessionState::Failed;
                self.error = Some(e);
            }
            (StudioRequest::Discard | StudioRequest::RestoreStock, Ok(status)) => {
                self.zmk_dirty = status.unsaved;
                self.studio.history.clear();
                if task.then_close {
                    self.reset();
                    keyboard.end_edit_session();
                    return true;
                }
                if let Some(target) = self.target {
                    self.retarget(keyboard, target);
                }
            }
            (StudioRequest::Discard | StudioRequest::RestoreStock, Err(e)) => {
                self.error = Some(e);
            }
        }
        false
    }

    /// Undoes this session's last write (Studio's Undo).
    pub(crate) fn undo(&mut self, keyboard: &Keyboard) {
        self.step(keyboard, Step::Undo);
    }

    /// Writes again what was last undone (Studio's Redo).
    pub(crate) fn redo(&mut self, keyboard: &Keyboard) {
        self.step(keyboard, Step::Redo);
    }

    fn step(&mut self, keyboard: &Keyboard, step: Step) {
        if self.session_busy() {
            return;
        }
        let edit = match step {
            Step::Undo => self.studio.history.next_undo(),
            Step::Redo => self.studio.history.next_redo(),
            Step::Do => None,
        };
        let Some(edit) = edit.cloned() else {
            return;
        };
        let action = match step {
            Step::Undo => edit.before.clone(),
            _ => edit.after.clone(),
        };
        let receiver = edit.target.set_key(keyboard, action);
        self.studio.writing = Some((step, edit));
        self.start_task(PendingKind::Set, receiver);
    }

    /// Saves the board's changes to its flash.
    pub(crate) fn save(&mut self, keyboard: &Keyboard) {
        if self.zmk_dirty && !self.session_busy() {
            self.start_save(keyboard);
        }
    }

    /// Drops the board's unsaved changes, back to its saved keymap.
    pub(crate) fn discard(&mut self, keyboard: &Keyboard, then_close: bool) {
        if !self.session_busy() {
            self.start_studio(keyboard, StudioRequest::Discard, then_close);
        }
    }

    /// Restores the stock keymap (after the user confirmed it).
    pub(crate) fn restore_stock(&mut self, keyboard: &Keyboard) {
        if !self.session_busy() {
            self.start_studio(keyboard, StudioRequest::RestoreStock, false);
        }
    }

    /// Undo, Redo, the unsaved state, Save, Discard and Restore stock, above the layers.
    pub(crate) fn draw_session_bar(&mut self, ui: &mut egui::Ui, keyboard: &Keyboard) {
        let busy = self.session_busy();
        let dirty = self.zmk_dirty;
        let can_undo = !busy && self.studio.history.next_undo().is_some();
        let can_redo = !busy && self.studio.history.next_redo().is_some();
        let (mut undo, mut redo, mut save, mut discard, mut restore) =
            (false, false, false, false, false);
        // The shortcuts work unless a text box (the search) has the keyboard or a prompt is up.
        if self.overlay().is_none() && ui.ctx().memory(|m| m.focused().is_none()) {
            use egui::{Key, KeyboardShortcut, Modifiers};
            ui.input_mut(|i| {
                let shift = Modifiers::COMMAND | Modifiers::SHIFT;
                redo = i.consume_shortcut(&KeyboardShortcut::new(shift, Key::Z))
                    || i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Y));
                undo = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
                save = i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S));
            });
        }
        ui.horizontal(|ui| {
            let small = |text: &str| egui::Button::new(RichText::new(text).size(15.0));
            undo |= ui
                .add_enabled(can_undo, small(icon::ARROW_COUNTER_CLOCKWISE))
                .on_hover_text("Undo (Ctrl+Z)")
                .on_disabled_hover_text("Nothing to undo")
                .clicked();
            redo |= ui
                .add_enabled(can_redo, small(icon::ARROW_CLOCKWISE))
                .on_hover_text("Redo (Ctrl+Y)")
                .on_disabled_hover_text("Nothing to redo")
                .clicked();
            ui.add_space(4.0);
            if self.zmk_session != ZmkSessionState::Ready {
                // Locked or connecting: the board hasn't said yet.
            } else if dirty {
                let warn = ui.visuals().warn_fg_color;
                ui.label(RichText::new(format!("{} Unsaved changes", icon::CIRCLE)).color(warn))
                    .on_hover_text(
                        "These changes are on the keyboard now, but only until it restarts. \
                         Save keeps them.",
                    );
            } else {
                ui.weak("All changes saved");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.menu_button(RichText::new(icon::DOTS_THREE).size(15.0), |ui| {
                        if ui
                            .button("Restore stock keymap…")
                            .on_hover_text(
                                "Put back the keymap your keyboard's firmware was built with",
                            )
                            .clicked()
                        {
                            restore = true;
                            ui.close();
                        }
                    })
                    .response
                    .on_hover_text("More");
                });
                discard |= ui
                    .add_enabled(dirty && !busy, egui::Button::new("Discard"))
                    .on_hover_text("Go back to the keymap you last saved")
                    .clicked();
                let fill = if dirty && !busy {
                    ui.visuals().selection.bg_fill
                } else {
                    ui.visuals().widgets.inactive.weak_bg_fill
                };
                save |= ui
                    .add_enabled(dirty && !busy, egui::Button::new("Save").fill(fill))
                    .on_hover_text("Keep these changes on the keyboard (Ctrl+S)")
                    .clicked();
            });
        });
        if undo && can_undo {
            self.undo(keyboard);
        } else if redo && can_redo {
            self.redo(keyboard);
        } else if save {
            self.save(keyboard);
        } else if discard {
            self.discard(keyboard, false);
        } else if restore {
            self.studio.prompt = Some(Prompt::RestoreStock);
        }
    }

    /// The unlock prompt, a confirmation, or progress: a modal over the middle of the editor
    /// (`ui` is the editor window's), so the keys behind it take no clicks. Escape answers a
    /// confirmation with Cancel or Keep editing.
    pub(crate) fn draw_session_prompt(&mut self, ui: &mut egui::Ui, keyboard: &Keyboard) {
        if self.closing || !self.studio.covers_editor() {
            return;
        }
        let ctx = ui.ctx().clone();
        let centre = ui.max_rect().center();
        let width = (ui.max_rect().width() - 24.0).clamp(220.0, 360.0);
        let area = egui::Area::new(egui::Id::new("foxglove-keymap-session-prompt"))
            .kind(egui::UiKind::Modal)
            .sense(egui::Sense::hover())
            .order(egui::Order::Foreground)
            .interactable(true)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(centre);
        let shown = egui::Modal::new(egui::Id::new("foxglove-keymap-session-prompt"))
            .area(area)
            .backdrop_color(crate::foxglove::calibrate_ui::BACKDROP)
            .frame(egui::Frame::window(ui.style()).inner_margin(16.0))
            .show(&ctx, |ui| {
                ui.set_width(width);
                self.prompt_body(ui, keyboard);
            });
        let escape = shown.is_top_modal
            && !shown.any_popup_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if escape && self.studio.task.is_none() {
            self.studio.prompt = None;
        }
    }

    fn prompt_body(&mut self, ui: &mut egui::Ui, keyboard: &Keyboard) {
        let heading = |ui: &mut egui::Ui, glyph: &str, text: &str| {
            ui.label(
                RichText::new(format!("{glyph}  {text}"))
                    .size(16.0)
                    .strong(),
            );
            ui.add_space(6.0);
        };
        if self.studio.lock == Lock::Locked {
            heading(ui, icon::LOCK_KEY, "Unlock your keyboard");
            ui.label("Your keyboard is locked, so its keymap can't be changed yet.");
            ui.add_space(4.0);
            ui.label(
                "Press its unlock key: the key, or key combination, that your keymap gives \
                 to Studio unlock.",
            );
            ui.add_space(4.0);
            ui.weak("The editor opens as soon as the keyboard unlocks.");
            ui.add_space(4.0);
            ui.weak(
                "No unlock key? Add &studio_unlock to your keymap, or build the firmware with \
                 Studio locking turned off.",
            );
            ui.add_space(10.0);
            if ui.button("Close").clicked() {
                self.reset();
                keyboard.end_edit_session();
            }
            return;
        }
        if let Some(task) = &self.studio.task {
            let text = match task.request {
                StudioRequest::RestoreStock => "Restoring the stock keymap…",
                _ => "Discarding changes…",
            };
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(18.0));
                ui.label(RichText::new(text).strong());
            });
            return;
        }
        match self.studio.prompt {
            Some(Prompt::RestoreStock) => {
                heading(ui, icon::WARNING, "Restore the stock keymap?");
                ui.label(
                    "Every key on every layer goes back to the keymap your keyboard's firmware \
                     was built with. Changes saved from Foxglove or ZMK Studio are lost.",
                );
                ui.add_space(4.0);
                ui.weak("This can't be undone.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let danger = ui.visuals().error_fg_color;
                    let restore = egui::Button::new(
                        RichText::new("Restore stock keymap").color(egui::Color32::WHITE),
                    )
                    .fill(danger);
                    if ui.add(restore).clicked() {
                        self.studio.prompt = None;
                        self.restore_stock(keyboard);
                    }
                    if ui.button("Cancel").clicked() {
                        self.studio.prompt = None;
                    }
                });
            }
            Some(Prompt::Close) => {
                heading(ui, icon::FLOPPY_DISK, "Save your changes?");
                ui.label(
                    "Your keyboard has changes that aren't saved. Unsaved, they last only \
                     until the keyboard restarts.",
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let save = egui::Button::new("Save").fill(ui.visuals().selection.bg_fill);
                    if ui.add(save).clicked() {
                        // The window's own path: save, then close.
                        self.studio.prompt = None;
                        self.closing = true;
                    }
                    if ui.button("Discard").clicked() {
                        self.studio.prompt = None;
                        self.discard(keyboard, true);
                    }
                    if ui.button("Keep editing").clicked() {
                        self.studio.prompt = None;
                    }
                });
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zmk_studio_api::{Behavior, HidUsage};

    fn key(usage: u32) -> KeyAction {
        KeyAction::Zmk(Behavior::KeyPress(HidUsage::from_encoded(
            0x07_0000 | usage,
        )))
    }

    fn edit(col: usize, before: u32, after: u32) -> Edit {
        Edit {
            target: EditTarget {
                layer_index: 0,
                row: 0,
                col,
            },
            before: key(before),
            after: key(after),
        }
    }

    #[test]
    fn undo_and_redo_walk_the_writes_and_a_new_write_clears_redo() {
        let mut history = History::default();
        history.record(edit(1, 4, 5));
        history.record(edit(2, 6, 7));
        assert_eq!(history.next_undo(), Some(&edit(2, 6, 7)));
        history.undone();
        assert_eq!(history.next_undo(), Some(&edit(1, 4, 5)));
        assert_eq!(history.next_redo(), Some(&edit(2, 6, 7)));
        history.redone();
        assert_eq!(history.next_redo(), None);
        history.undone();
        history.record(edit(3, 8, 9));
        assert_eq!(history.next_redo(), None, "a new change ends the redo list");
        assert_eq!(history.next_undo(), Some(&edit(3, 8, 9)));
    }

    #[test]
    fn history_keeps_the_last_writes() {
        let mut history = History::default();
        for i in 0..(HISTORY_LIMIT + 5) {
            history.record(edit(i, 4, 5));
        }
        let mut count = 0;
        while history.next_undo().is_some() {
            history.undone();
            count += 1;
        }
        assert_eq!(count, HISTORY_LIMIT);
        assert_eq!(history.next_redo(), Some(&edit(5, 4, 5)));
    }

    #[test]
    fn a_locked_refusal_asks_for_the_unlock_key() {
        let mut session = KeymapSession::default();
        assert!(!session.write_failed("Set layer binding failed"));
        assert_eq!(session.lock, Lock::Unknown);
        assert!(session.write_failed(LOCKED_MESSAGE));
        assert_eq!(session.lock, Lock::Locked);
        assert!(session.covers_editor());
    }

    #[test]
    fn a_write_that_changes_nothing_is_not_an_undo_step() {
        let mut session = KeymapSession {
            writing: Some((Step::Do, edit(1, 4, 4))),
            ..Default::default()
        };
        session.write_done();
        assert_eq!(session.history.next_undo(), None);
    }
}
