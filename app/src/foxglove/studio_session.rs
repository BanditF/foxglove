//! The keyboard side of a ZMK Studio edit session (#578), for any board that speaks Studio RPC.
//!
//! Studio's model, which this follows (zmk-studio @ 1bb6bb84a3, Apache-2.0): every change is
//! live on the board at once and held in its RAM until **Save** writes it to flash or
//! **Discard** drops it. The board says whether it has unsaved changes (`check_unsaved_changes`
//! and the `unsaved_changes_status_changed` notification). A board built with Studio locking
//! answers nothing until someone presses its unlock key; it then sends `lock_state_changed`.
//!
//! [`StudioEditor`] owns the link while the editor is open. It runs on the keyboard's reader
//! thread, except for an unlock wait: that moves the link to a thread of its own, which blocks
//! on the board's notifications and hands the link back when the board unlocks, the wait is
//! cancelled (the editor closes) or its time limit runs out.

use std::error::Error;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zmk_studio_api::proto::zmk::{core, keymap, meta, studio};
use zmk_studio_api::{Behavior, ClientError, ResolvedLayer, StudioClient};

use crate::foxglove::behavior_catalog::{self, Catalog, CustomBehavior, LayerRoom};
use crate::key_action::KeymapSnapshot;
use crate::key_matrix::KeyMatrix;
use crate::protocols::DeviceLocked;
use crate::ui_wake::UiWake;

/// The text a write gets when the board is locked; the editor shows the unlock prompt for it.
pub const LOCKED_MESSAGE: &str =
    "The keyboard is locked. Press its Studio unlock key, then try again.";

/// The fallback end of an unlock wait when nobody presses the key or closes the prompt. ZMK
/// itself locks an idle board again after 10 minutes, so the prompt lasts as long.
const UNLOCK_WAIT_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How long taking the link back from an unlock wait may take: the wait's next read returns
/// within the transport's read timeout (150 ms on USB).
const HAND_BACK_LIMIT: Duration = Duration::from_secs(3);

const STOPPED: &str = "the unlock wait was stopped";
const WAIT_EXPIRED: &str = "the unlock wait ran out of time";

/// A byte link to a Studio board: USB serial, Bluetooth, or a test's fake board.
pub trait Line: Read + Write + Send {}
impl<T: Read + Write + Send> Line for T {}

/// Stops a blocked read from outside: the editor closing, or the unlock wait's time limit.
#[derive(Default)]
struct Gate {
    stop: AtomicBool,
    until: Mutex<Option<Instant>>,
}

impl Gate {
    fn until(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.until.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A [`Line`] whose reads fail once its [`Gate`] says stop, so a blocking notification read
/// ends without waiting for the board.
pub struct Gated {
    inner: Box<dyn Line>,
    gate: Arc<Gate>,
}

impl Read for Gated {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.gate.stop.load(Ordering::Acquire) {
            return Err(io::Error::other(STOPPED));
        }
        if self.gate.until().is_some_and(|t| Instant::now() >= t) {
            return Err(io::Error::other(WAIT_EXPIRED));
        }
        self.inner.read(buf)
    }
}

impl Write for Gated {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub type Client = StudioClient<Gated>;

/// An open Studio client and the gate on its reads.
pub struct Link {
    client: Client,
    gate: Arc<Gate>,
}

impl Link {
    pub fn new(line: impl Line + 'static) -> Self {
        let gate = Arc::new(Gate::default());
        let client = StudioClient::new(Gated {
            inner: Box::new(line),
            gate: Arc::clone(&gate),
        });
        Self { client, gate }
    }

    pub fn client(&mut self) -> &mut Client {
        &mut self.client
    }
}

/// Opens a link to the board (the transport is the caller's: `zmk_rpc::studio_opener`).
pub type Opener = Box<dyn FnMut() -> Result<Link, Box<dyn Error>> + Send>;

/// What the board said about the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub locked: bool,
    pub unsaved: bool,
}

/// A change to the board's layers or physical layout (#583), as ZMK Studio's layer picker
/// makes them. Indexes are positions in the layer list; ids are the board's stable layer ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerOp {
    /// Enable a reserved layer slot (`add_layer`).
    Add,
    Remove {
        index: u32,
    },
    /// Put back a layer removed this session (`restore_layer`).
    Restore {
        id: u32,
        at: u32,
    },
    Move {
        from: u32,
        to: u32,
    },
    Rename {
        id: u32,
        name: String,
    },
    /// Switch the active physical layout (`set_active_physical_layout`).
    Layout {
        index: u32,
    },
}

/// A session request from the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StudioRequest {
    /// Open the session: the lock state, and whether the board has unsaved changes.
    Open,
    /// Wait for the board to unlock (answered when it does).
    WaitUnlock,
    /// Drop the board's unsaved changes and read its keymap again.
    Discard,
    /// The board's stock keymap (`reset_settings`), then its keymap again.
    RestoreStock,
    /// Read the keymap and the unsaved state again, changing nothing (F6: after a failed write,
    /// to show what the board really has).
    Reread,
    /// Change the layers or the physical layout, then read the keymap again (#583).
    Layers(LayerOp),
}

/// Answers one [`StudioRequest`]: the editor's channel, a repaint, and a new keymap for the
/// overlay when the request changed it.
pub struct StudioDone {
    respond: Sender<Result<Status, String>>,
    wake: UiWake,
    matrix: Arc<Mutex<KeyMatrix>>,
    catalog: Option<Catalog>,
}

impl StudioDone {
    pub fn new(
        respond: Sender<Result<Status, String>>,
        wake: UiWake,
        matrix: Arc<Mutex<KeyMatrix>>,
    ) -> Self {
        Self {
            respond,
            wake,
            matrix,
            catalog: None,
        }
    }

    /// Where the board's own behaviours go when the session opens (#582).
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = Some(catalog);
        self
    }

    /// Hands the editor the board's own behaviours (#582).
    fn publish(&self, behaviors: &[CustomBehavior]) {
        if let Some(catalog) = &self.catalog {
            catalog.lock().unwrap_or_else(|e| e.into_inner()).behaviors = behaviors.to_vec();
        }
    }

    /// Hands the editor the board's room for layers (#583).
    fn publish_room(&self, room: LayerRoom) {
        if let Some(catalog) = &self.catalog {
            catalog.lock().unwrap_or_else(|e| e.into_inner()).room = Some(room);
        }
    }

    pub fn send(self, result: Result<Status, String>) {
        let _ = self.respond.send(result);
        self.wake.request_repaint();
    }

    /// Puts `snapshot` in the overlay's keymap (keeping which keys are held), then answers.
    fn send_with_keymap(self, status: Status, snapshot: KeymapSnapshot) {
        {
            let mut matrix = self.matrix.lock().unwrap_or_else(|e| e.into_inner());
            let pressed = std::mem::take(&mut matrix.pressed);
            let rows = pressed.len();
            let cols = pressed.first().map_or(0, Vec::len);
            *matrix = KeyMatrix::from_snapshot(snapshot, rows, cols);
            matrix.pressed = pressed;
        }
        self.send(Ok(status));
    }
}

/// Whether an RPC error leaves the link unusable (Foxglove moved this here from `zmk.rs`).
fn should_drop_link(err: &ClientError) -> bool {
    !matches!(
        err,
        ClientError::SetLayerBindingFailed(_)
            | ClientError::SaveChangesFailed(_)
            | ClientError::SetActivePhysicalLayoutFailed(_)
            | ClientError::MoveLayerFailed(_)
            | ClientError::AddLayerFailed(_)
            | ClientError::RemoveLayerFailed(_)
            | ClientError::RestoreLayerFailed(_)
            | ClientError::SetLayerPropsFailed(_)
            | ClientError::InvalidLayerOrPosition { .. }
            | ClientError::MissingBehaviorRole(_)
            | ClientError::BehaviorIdOutOfRange { .. }
            | ClientError::Meta(_)
    )
}

/// `Some(locked)` for a `lock_state_changed` notification.
fn lock_change(n: &studio::Notification) -> Option<bool> {
    match &n.subsystem {
        Some(studio::notification::Subsystem::Core(core::Notification {
            notification_type: Some(core::notification::NotificationType::LockStateChanged(state)),
        })) => Some(*state == core::LockState::ZmkStudioCoreLockStateLocked as i32),
        _ => None,
    }
}

/// `Some(unsaved)` for an `unsaved_changes_status_changed` notification.
fn unsaved_change(n: &studio::Notification) -> Option<bool> {
    match &n.subsystem {
        Some(studio::notification::Subsystem::Keymap(keymap::Notification {
            notification_type:
                Some(keymap::notification::NotificationType::UnsavedChangesStatusChanged(unsaved)),
        })) => Some(*unsaved),
        _ => None,
    }
}

/// A link lent to an unlock wait, and the way it comes back.
struct Parked {
    gate: Arc<Gate>,
    back: Receiver<(Option<Link>, Status)>,
}

/// The edit session with one board.
pub struct StudioEditor {
    opener: Opener,
    link: Option<Link>,
    parked: Option<Parked>,
    status: Status,
    settled: Option<Settled>,
    /// The board's own behaviours (#582), read once per link to this board.
    custom: Option<Vec<CustomBehavior>>,
    /// The board's room for layers as last read (#583), and the layers removed this session.
    room: LayerRoom,
    /// The keymap as last read, to name a layer before it is removed.
    last_keymap: Option<keymap::Keymap>,
}

/// Told the keymap (and physical layouts) a board keeps after Save, Discard or Restore stock
/// (#592): the saved keymap copy follows it (`keymap_cache::refresh`).
pub type Settled = Box<dyn FnMut(&keymap::Keymap, &keymap::PhysicalLayouts) + Send>;

impl StudioEditor {
    pub fn new(opener: Opener) -> Self {
        Self {
            opener,
            link: None,
            parked: None,
            status: Status::default(),
            settled: None,
            custom: None,
            room: LayerRoom::default(),
            last_keymap: None,
        }
    }

    /// Calls `settled` after each Save, Discard and Restore stock that the board accepted.
    pub fn on_settled(mut self, settled: Settled) -> Self {
        self.settled = Some(settled);
        self
    }

    /// Tells the `settled` hook what the board keeps now: Save, Discard and Restore stock all
    /// leave its live keymap equal to its saved one. `raw` is the keymap if just read. A read
    /// that fails only costs the copy this update; the request itself succeeded.
    fn settle(&mut self, raw: Option<keymap::Keymap>) {
        let Some(mut settled) = self.settled.take() else {
            return;
        };
        let read = self.run(|c| {
            let raw = match raw {
                Some(raw) => raw,
                None => c.get_keymap()?,
            };
            Ok((raw, c.get_physical_layouts()?))
        });
        match read {
            Ok((raw, layouts)) => settled(&raw, &layouts),
            Err(e) => eprintln!("Foxglove: can't read the keymap for its saved copy: {e}"),
        }
        self.settled = Some(settled);
    }

    #[cfg(test)]
    pub fn status(&self) -> Status {
        self.status
    }

    /// Takes the link back from an unlock wait, stopping the wait if it still runs.
    fn reclaim(&mut self) {
        let Some(parked) = self.parked.take() else {
            return;
        };
        parked.gate.stop.store(true, Ordering::Release);
        match parked.back.recv_timeout(HAND_BACK_LIMIT) {
            Ok((link, status)) => {
                self.status = status;
                self.link = link;
                parked.gate.stop.store(false, Ordering::Release);
            }
            // Left stopped: the stuck thread lets go of the link at its next read.
            Err(_) => {
                eprintln!("Foxglove: the Studio unlock wait didn't hand the link back");
                self.link = None;
                self.status = Status::default();
            }
        }
    }

    /// The open link, opened (and its lock state read) if needed.
    fn link(&mut self) -> Result<&mut Link, Box<dyn Error>> {
        self.reclaim();
        if self.link.is_none() {
            let mut link = (self.opener)()?;
            let locked =
                link.client.get_lock_state()? == core::LockState::ZmkStudioCoreLockStateLocked;
            self.status = Status {
                locked,
                unsaved: false,
            };
            self.link = Some(link);
        }
        self.link.as_mut().ok_or_else(|| "no Studio link".into())
    }

    /// Notes what the board announced during the last call.
    fn absorb_notifications(&mut self) {
        let Some(link) = self.link.as_mut() else {
            return;
        };
        while let Some(n) = link.client.next_notification() {
            if let Some(locked) = lock_change(&n) {
                self.status.locked = locked;
            }
            if let Some(unsaved) = unsaved_change(&n) {
                self.status.unsaved = unsaved;
            }
        }
    }

    /// Runs `op` on the unlocked session. A locked board answers [`DeviceLocked`].
    fn run<T>(
        &mut self,
        op: impl FnOnce(&mut Client) -> Result<T, ClientError>,
    ) -> Result<T, Box<dyn Error>> {
        self.link()?;
        if self.status.locked {
            return Err(Box::new(DeviceLocked));
        }
        let link = self.link.as_mut().ok_or("no Studio link")?;
        let result = op(&mut link.client);
        self.absorb_notifications();
        match result {
            Ok(value) => Ok(value),
            Err(ClientError::Meta(meta::ErrorConditions::UnlockRequired)) => {
                // Locked again (idle, or by another tool) since the session opened.
                self.status.locked = true;
                Err(Box::new(DeviceLocked))
            }
            Err(e) => {
                if should_drop_link(&e) {
                    self.link = None;
                }
                Err(Box::new(e))
            }
        }
    }

    /// Opens the session: on an unlocked board, its behaviours and whether it has unsaved
    /// changes.
    pub fn open(&mut self) -> Result<Status, Box<dyn Error>> {
        self.link()?;
        if !self.status.locked {
            let read = self.custom.is_none();
            let (unsaved, custom, raw, layouts) = self.run(|c| {
                c.ensure_behavior_catalog()?;
                let custom = if read {
                    Some(behavior_catalog::read(c)?)
                } else {
                    None
                };
                let raw = c.get_keymap()?;
                let layouts = c.get_physical_layouts()?;
                Ok((c.check_unsaved_changes()?, custom, raw, layouts))
            })?;
            self.status.unsaved = unsaved;
            if custom.is_some() {
                self.custom = custom;
            }
            self.note_room(raw, &layouts);
        }
        Ok(self.status)
    }

    pub fn set_key(
        &mut self,
        layer_id: u32,
        key_position: i32,
        behavior: Behavior,
    ) -> Result<(), Box<dyn Error>> {
        self.run(|c| c.set_key_at(layer_id, key_position, behavior))?;
        self.status.unsaved = true;
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), Box<dyn Error>> {
        self.run(|c| c.save_changes())?;
        self.status.unsaved = false;
        self.room.removed.clear(); // saved: removed layers are gone for good
        self.settle(None);
        Ok(())
    }

    /// Ends the session: stops an unlock wait and closes the link.
    pub fn end(&mut self) {
        self.reclaim();
        self.link = None;
        self.status = Status::default();
    }

    /// The keymap as the board has it now, and whether it still has unsaved changes.
    fn reread(&mut self) -> Result<(Vec<ResolvedLayer>, keymap::Keymap), Box<dyn Error>> {
        let (layers, raw, layouts, unsaved) = self.run(|c| {
            let layers = c.resolve_keymap()?;
            let raw = c.get_keymap()?;
            let layouts = c.get_physical_layouts()?;
            Ok((layers, raw, layouts, c.check_unsaved_changes()?))
        })?;
        self.status.unsaved = unsaved;
        self.note_room(raw.clone(), &layouts);
        Ok((layers, raw))
    }

    /// Notes the board's room for layers from a keymap and its layouts just read (#583).
    fn note_room(&mut self, raw: keymap::Keymap, layouts: &keymap::PhysicalLayouts) {
        let removed = std::mem::take(&mut self.room.removed);
        self.room = LayerRoom {
            removed,
            ..room_from(&raw, layouts)
        };
        self.last_keymap = Some(raw);
    }

    /// Makes one layer change on the board (#583), keeping the list of layers removed this
    /// session for Restore.
    fn layer_op(&mut self, op: &LayerOp) -> Result<(), Box<dyn Error>> {
        let removing = match op {
            LayerOp::Remove { index } => self
                .last_keymap
                .as_ref()
                .and_then(|k| k.layers.get(*index as usize))
                .map(|l| (l.id, l.name.clone())),
            _ => None,
        };
        self.run(|c| match op {
            LayerOp::Add => c.add_layer().map(|_| ()),
            LayerOp::Remove { index } => c.remove_layer(*index),
            LayerOp::Restore { id, at } => c.restore_layer(*id, *at).map(|_| ()),
            LayerOp::Move { from, to } => c.move_layer(*from, *to).map(|_| ()),
            LayerOp::Rename { id, name } => c.set_layer_props(*id, name.clone()),
            LayerOp::Layout { index } => c.set_active_physical_layout(*index).map(|_| ()),
        })?;
        match op {
            LayerOp::Remove { .. } => self.room.removed.extend(removing),
            LayerOp::Restore { id, .. } => self.room.removed.retain(|(r, _)| r != id),
            _ => {}
        }
        Ok(())
    }

    /// Answers `request` through `done`: at once, or for an unlock wait when the board unlocks.
    /// `keymap` turns a re-read keymap into the overlay's (and updates the protocol's copy).
    pub fn handle(
        &mut self,
        request: StudioRequest,
        done: StudioDone,
        keymap: impl FnOnce(&[ResolvedLayer], &keymap::Keymap) -> KeymapSnapshot,
    ) {
        // Save-like requests leave the board's live keymap equal to its saved one (#592).
        let settles = matches!(
            request,
            StudioRequest::Discard | StudioRequest::RestoreStock
        );
        if settles {
            self.room.removed.clear(); // as in Studio: nothing removed is left to restore
        }
        let changed = match request {
            StudioRequest::Open => return self.answer_open(done),
            StudioRequest::WaitUnlock => return self.wait_unlock(done),
            StudioRequest::Discard => self.run(|c| c.discard_changes()).map(|_| ()),
            StudioRequest::Reread => Ok(()),
            StudioRequest::Layers(op) => self.layer_op(&op),
            StudioRequest::RestoreStock => self.run(|c| c.reset_settings()).and_then(|done| {
                done.then_some(())
                    .ok_or_else(|| "The keyboard didn't restore its stock keymap.".into())
            }),
        };
        let changed = changed.and_then(|()| self.reread());
        // The room is worth showing even when the change failed: read it back if it can be.
        done.publish_room(self.room.clone());
        match changed {
            Ok((layers, raw)) => {
                done.send_with_keymap(self.status, keymap(&layers, &raw));
                if settles {
                    self.settle(Some(raw));
                }
            }
            Err(e) => done.send(Err(error_text(e))),
        }
    }

    /// Opens the session and answers, handing the editor the board's own behaviours.
    fn answer_open(&mut self, done: StudioDone) {
        let result = self.open().map_err(error_text);
        if result.is_ok() {
            done.publish(self.custom.as_deref().unwrap_or_default());
            if !self.status.locked {
                done.publish_room(self.room.clone());
            }
        }
        done.send(result);
    }

    /// Lends the link to a thread that waits for the board's `lock_state_changed`.
    fn wait_unlock(&mut self, done: StudioDone) {
        if let Err(e) = self.link() {
            return done.send(Err(error_text(e)));
        }
        if !self.status.locked {
            return self.answer_open(done);
        }
        let Some(mut link) = self.link.take() else {
            return done.send(Err("no Studio link".into()));
        };
        *link.gate.until() = Some(Instant::now() + UNLOCK_WAIT_LIMIT);
        let gate = Arc::clone(&link.gate);
        let (back_tx, back) = mpsc::channel();
        let mut status = self.status;
        crate::foxglove::services::spawn_named("studio-unlock-wait", move || {
            let result = wait_for_unlock(&mut link, &mut status).map(|(status, custom, room)| {
                done.publish(&custom);
                done.publish_room(room);
                status
            });
            *link.gate.until() = None;
            let (link, status) = match &result {
                Ok(status) => (Some(link), *status),
                // Stopped or out of time: the link itself is fine.
                Err(WaitError::Stopped | WaitError::Expired) => (Some(link), status),
                Err(WaitError::Failed(_)) => (None, Status::default()),
            };
            // The link goes back first, so the editor's next request finds it.
            let _ = back_tx.send((link, status));
            done.send(result.map_err(|e| e.text()));
        });
        self.parked = Some(Parked { gate, back });
    }
}

impl Drop for StudioEditor {
    /// The keyboard went away with an unlock wait running: end the wait, so its thread lets go
    /// of the link now instead of at its time limit.
    fn drop(&mut self) {
        if let Some(parked) = &self.parked {
            parked.gate.stop.store(true, Ordering::Release);
        }
    }
}

enum WaitError {
    Stopped,
    Expired,
    Failed(String),
}

impl WaitError {
    fn from_client(e: ClientError) -> Self {
        match &e {
            ClientError::Io(io) if io.to_string() == STOPPED => Self::Stopped,
            ClientError::Io(io) if io.to_string() == WAIT_EXPIRED => Self::Expired,
            _ => Self::Failed(e.to_string()),
        }
    }

    fn text(self) -> String {
        match self {
            Self::Stopped => "Stopped waiting for the keyboard to unlock.".into(),
            Self::Expired => "The keyboard wasn't unlocked within 10 minutes. \
                Open the editor again to try once more."
                .into(),
            Self::Failed(e) => format!("Lost the keyboard while waiting for it to unlock: {e}"),
        }
    }
}

/// Blocks on the board's notifications until it reports `unlocked` (noted in `status`, so a
/// stop after it still hands the link back as unlocked), then gets the session ready as
/// [`StudioEditor::open`] does.
fn wait_for_unlock(
    link: &mut Link,
    status: &mut Status,
) -> Result<(Status, Vec<CustomBehavior>, LayerRoom), WaitError> {
    loop {
        let n = link
            .client
            .read_notification_blocking()
            .map_err(WaitError::from_client)?;
        if lock_change(&n) == Some(false) {
            status.locked = false;
            break;
        }
    }
    *link.gate.until() = None;
    let client = &mut link.client;
    client
        .ensure_behavior_catalog()
        .and_then(|()| behavior_catalog::read(client))
        .and_then(|custom| {
            let room = room_from(&client.get_keymap()?, &client.get_physical_layouts()?);
            Ok((client.check_unsaved_changes()?, custom, room))
        })
        .map(|(unsaved, custom, room)| {
            (
                Status {
                    locked: false,
                    unsaved,
                },
                custom,
                room,
            )
        })
        .map_err(WaitError::from_client)
}

/// The board's room for layers from its keymap and physical layouts (#583); nothing removed.
fn room_from(raw: &keymap::Keymap, layouts: &keymap::PhysicalLayouts) -> LayerRoom {
    LayerRoom {
        free: raw.available_layers,
        name_max: raw.max_layer_name_length,
        layouts: layouts
            .layouts
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if l.name.is_empty() {
                    format!("Layout {}", i + 1)
                } else {
                    l.name.clone()
                }
            })
            .collect(),
        active_layout: layouts.active_layout_index as usize,
        removed: Vec::new(),
    }
}

/// What the editor shows for a session error.
pub fn error_text(error: Box<dyn Error>) -> String {
    if error.is::<DeviceLocked>() {
        LOCKED_MESSAGE.to_string()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foxglove::studio_fake::FakeBoard;
    use zmk_studio_api::HidUsage;

    fn editor(board: &FakeBoard) -> StudioEditor {
        let board = board.clone();
        StudioEditor::new(Box::new(move || Ok(Link::new(board.line()))))
    }

    fn done() -> (StudioDone, Receiver<Result<Status, String>>) {
        let (tx, rx) = mpsc::channel();
        let snapshot = KeymapSnapshot {
            layers: vec![],
            actions: vec![],
        };
        let matrix = Arc::new(Mutex::new(KeyMatrix::from_snapshot(snapshot, 1, 1)));
        (
            StudioDone::new(tx, UiWake::new(Arc::new(|| {})), matrix),
            rx,
        )
    }

    fn ask(editor: &mut StudioEditor, request: StudioRequest) -> Result<Status, String> {
        let (done, rx) = done();
        editor.handle(request, done, |_, _| KeymapSnapshot {
            layers: vec![],
            actions: vec![],
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("an answer")
    }

    fn key(usage: u16) -> Behavior {
        Behavior::KeyPress(HidUsage::from_encoded(0x07_0000 | usage as u32))
    }

    /// Each desk's board: the second desk's one-piece Plank 3 has Studio locking on, the
    /// user's Corne build has it off.
    fn boards() -> [FakeBoard; 2] {
        [FakeBoard::plank3(), FakeBoard::corne()]
    }

    #[test]
    fn opening_reports_unsaved_changes_left_on_the_board() {
        for board in boards() {
            board.unlock();
            board.leave_unsaved_change(0, 3);
            let mut editor = editor(&board);
            assert_eq!(
                ask(&mut editor, StudioRequest::Open),
                Ok(Status {
                    locked: false,
                    unsaved: true
                })
            );
        }
    }

    #[test]
    fn a_write_marks_the_session_unsaved_and_save_clears_it() {
        for board in boards() {
            board.unlock();
            let mut editor = editor(&board);
            ask(&mut editor, StudioRequest::Open).unwrap();
            editor.set_key(0, 5, key(0x1d)).unwrap();
            assert!(editor.status().unsaved);
            assert_eq!(board.live_param(0, 5), 0x07_001d);
            assert_ne!(board.saved_param(0, 5), 0x07_001d);
            editor.save().unwrap();
            assert!(!editor.status().unsaved);
            assert_eq!(board.saved_param(0, 5), 0x07_001d);
        }
    }

    #[test]
    fn discard_returns_the_board_to_its_saved_keymap() {
        for board in boards() {
            board.unlock();
            let mut editor = editor(&board);
            ask(&mut editor, StudioRequest::Open).unwrap();
            let saved = board.saved_param(1, 2);
            editor.set_key(1, 2, key(0x2c)).unwrap();
            let status = ask(&mut editor, StudioRequest::Discard).unwrap();
            assert!(!status.unsaved);
            assert_eq!(board.live_param(1, 2), saved);
        }
    }

    #[test]
    fn restore_stock_puts_back_the_built_keymap() {
        for board in boards() {
            board.unlock();
            let mut editor = editor(&board);
            ask(&mut editor, StudioRequest::Open).unwrap();
            let stock = board.saved_param(0, 0);
            editor.set_key(0, 0, key(0x29)).unwrap();
            editor.save().unwrap();
            assert_eq!(board.saved_param(0, 0), 0x07_0029);
            ask(&mut editor, StudioRequest::RestoreStock).unwrap();
            assert_eq!(board.saved_param(0, 0), stock);
            assert_eq!(board.live_param(0, 0), stock);
        }
    }

    #[test]
    fn a_locked_board_opens_locked_and_unlocks_on_its_notification() {
        let board = FakeBoard::plank3();
        let mut editor = editor(&board);
        assert!(ask(&mut editor, StudioRequest::Open).unwrap().locked);
        assert!(
            editor.set_key(0, 1, key(0x04)).is_err(),
            "locked boards refuse"
        );
        let (done, rx) = done();
        editor.handle(StudioRequest::WaitUnlock, done, |_, _| unreachable!());
        // Nothing answers until the key is pressed: the wait is on the board, not a timer.
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        board.press_unlock_key();
        let status = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert!(!status.locked);
        // The link came back to the editor, unlocked.
        editor.set_key(0, 1, key(0x04)).unwrap();
        assert_eq!(board.live_param(0, 1), 0x07_0004);
        assert_eq!(board.lines_opened(), 1, "one link for the whole session");
    }

    #[test]
    fn closing_the_editor_stops_an_unlock_wait() {
        let board = FakeBoard::plank3();
        let mut editor = editor(&board);
        ask(&mut editor, StudioRequest::Open).unwrap();
        let (done, rx) = done();
        editor.handle(StudioRequest::WaitUnlock, done, |_, _| unreachable!());
        let started = Instant::now();
        editor.end();
        assert!(started.elapsed() < HAND_BACK_LIMIT);
        let answer = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(answer, Err(WaitError::Stopped.text()));
    }

    #[test]
    fn a_board_that_locks_again_mid_session_answers_locked() {
        let board = FakeBoard::plank3();
        board.unlock();
        let mut editor = editor(&board);
        ask(&mut editor, StudioRequest::Open).unwrap();
        board.lock();
        let err = editor.set_key(0, 1, key(0x04)).unwrap_err();
        assert_eq!(error_text(err), LOCKED_MESSAGE);
        assert!(editor.status().locked);
    }
}
