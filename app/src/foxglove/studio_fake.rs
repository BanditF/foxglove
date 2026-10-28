//! A fake ZMK Studio board for tests (#578): it answers Studio RPC from memory the way ZMK does
//! (live changes in RAM, Save, Discard, `reset_settings`, the lock and its notifications), so
//! the edit session runs end to end with no keyboard. Nothing here writes to a real board.
//!
//! [`FakeBoard::plank3`] is the second desk's one-piece board with Studio locking on;
//! [`FakeBoard::corne`] is the user's split board, whose build has locking off.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use prost::Message;
use zmk_studio_api::proto::zmk::{behaviors, core, keymap, meta, studio};
use zmk_studio_api::{Behavior, BehaviorRole, HidUsage, ResolvedLayer};

use super::keymap_cache::{encode_frame, Frames};
use super::studio_session::{Link, StudioDone, StudioEditor, StudioRequest};
use crate::key_action::{KeyAction, KeymapSnapshot, LayerInfo};
use crate::protocols::{Key, KeyboardDefinition, KeyboardLayout, KeyboardProtocol, WriteSupport};

/// The board's behaviours: (id, role). Ids start at 1 (0 means "no behaviour" to the client).
const BEHAVIORS: [(u32, BehaviorRole); 4] = [
    (1, BehaviorRole::KeyPress),
    (2, BehaviorRole::Transparent),
    (3, BehaviorRole::MomentaryLayer),
    (4, BehaviorRole::None),
];
const KEY_PRESS: i32 = 1;
const TRANSPARENT: i32 = 2;
const MOMENTARY: i32 = 3;

struct State {
    keys: usize,
    locked: bool,
    stock: Vec<keymap::Layer>,
    saved: Vec<keymap::Layer>,
    live: Vec<keymap::Layer>,
    unsaved: bool,
    out: VecDeque<u8>,
    lines_opened: usize,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Clone)]
pub(crate) struct FakeBoard(Arc<Shared>);

impl FakeBoard {
    fn new(keys: usize, layers: &[&str], locked: bool) -> Self {
        let stock: Vec<keymap::Layer> = layers
            .iter()
            .enumerate()
            .map(|(i, name)| keymap::Layer {
                id: i as u32,
                name: (*name).into(),
                bindings: (0..keys)
                    .map(|pos| match (i, pos) {
                        // The base layer types letters; its last key holds layer 1.
                        (0, p) if p + 1 == keys => binding(MOMENTARY, 1),
                        (0, p) => binding(KEY_PRESS, 0x07_0000 | (4 + (p % 26) as u32)),
                        _ => binding(TRANSPARENT, 0),
                    })
                    .collect(),
            })
            .collect();
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                keys,
                locked,
                saved: stock.clone(),
                live: stock.clone(),
                stock,
                unsaved: false,
                out: VecDeque::new(),
                lines_opened: 0,
            }),
            ready: Condvar::new(),
        }))
    }

    /// The second desk's Plank 3: one piece, 61 keys, Studio locking on (starts locked).
    pub(crate) fn plank3() -> Self {
        Self::new(
            crate::foxglove::fixtures::second_desk::KEYS,
            &["Base", "Nav", "Num"],
            true,
        )
    }

    /// The user's Corne: 42 keys, Studio locking off.
    pub(crate) fn corne() -> Self {
        Self::new(
            crate::foxglove::fixtures::users_desk::KEYS,
            &["Base", "Lower", "Raise", "Adjust"],
            false,
        )
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.state.lock().unwrap()
    }

    pub(crate) fn line(&self) -> FakeLine {
        self.state().lines_opened += 1;
        FakeLine {
            board: Arc::clone(&self.0),
            frames: Frames::default(),
        }
    }

    pub(crate) fn lines_opened(&self) -> usize {
        self.state().lines_opened
    }

    pub(crate) fn keys(&self) -> usize {
        self.state().keys
    }

    /// Unlocked before the test starts (no notification: nobody is listening yet).
    pub(crate) fn unlock(&self) {
        self.state().locked = false;
    }

    /// Someone pressed the board's `&studio_unlock` key.
    pub(crate) fn press_unlock_key(&self) {
        let mut state = self.state();
        state.locked = false;
        push(&mut state, lock_notification(false));
        self.0.ready.notify_all();
    }

    /// The board locked itself again (ZMK does after 10 idle minutes).
    pub(crate) fn lock(&self) {
        let mut state = self.state();
        state.locked = true;
        push(&mut state, lock_notification(true));
        self.0.ready.notify_all();
    }

    /// A change an earlier session made and never saved: still live on the board.
    pub(crate) fn leave_unsaved_change(&self, layer: usize, pos: usize) {
        let mut state = self.state();
        state.live[layer].bindings[pos] = binding(KEY_PRESS, 0x07_0035);
        state.unsaved = true;
    }

    pub(crate) fn live_param(&self, layer: usize, pos: usize) -> u32 {
        self.state().live[layer].bindings[pos].param1
    }

    pub(crate) fn saved_param(&self, layer: usize, pos: usize) -> u32 {
        self.state().saved[layer].bindings[pos].param1
    }

    /// The live keymap, resolved as the client would.
    pub(crate) fn resolved(&self) -> Vec<ResolvedLayer> {
        self.state()
            .live
            .iter()
            .map(|layer| ResolvedLayer {
                id: layer.id,
                name: layer.name.clone(),
                bindings: layer.bindings.iter().map(resolve).collect(),
            })
            .collect()
    }
}

fn binding(behavior_id: i32, param1: u32) -> keymap::BehaviorBinding {
    keymap::BehaviorBinding {
        behavior_id,
        param1,
        param2: 0,
    }
}

fn resolve(b: &keymap::BehaviorBinding) -> Behavior {
    match b.behavior_id {
        KEY_PRESS => Behavior::KeyPress(HidUsage::from_encoded(b.param1)),
        TRANSPARENT => Behavior::Transparent,
        MOMENTARY => Behavior::MomentaryLayer { layer_id: b.param1 },
        _ => Behavior::None,
    }
}

fn push(state: &mut State, response: studio::Response) {
    state.out.extend(encode_frame(&response.encode_to_vec()));
}

fn lock_notification(locked: bool) -> studio::Response {
    let state = if locked {
        core::LockState::ZmkStudioCoreLockStateLocked
    } else {
        core::LockState::ZmkStudioCoreLockStateUnlocked
    };
    notification(studio::notification::Subsystem::Core(core::Notification {
        notification_type: Some(core::notification::NotificationType::LockStateChanged(
            state as i32,
        )),
    }))
}

fn unsaved_notification(unsaved: bool) -> studio::Response {
    notification(studio::notification::Subsystem::Keymap(
        keymap::Notification {
            notification_type: Some(
                keymap::notification::NotificationType::UnsavedChangesStatusChanged(unsaved),
            ),
        },
    ))
}

fn notification(subsystem: studio::notification::Subsystem) -> studio::Response {
    studio::Response {
        r#type: Some(studio::response::Type::Notification(studio::Notification {
            subsystem: Some(subsystem),
        })),
    }
}

fn reply(request_id: u32, subsystem: studio::request_response::Subsystem) -> studio::Response {
    studio::Response {
        r#type: Some(studio::response::Type::RequestResponse(
            studio::RequestResponse {
                request_id,
                subsystem: Some(subsystem),
            },
        )),
    }
}

fn meta_error(condition: meta::ErrorConditions) -> studio::request_response::Subsystem {
    studio::request_response::Subsystem::Meta(meta::Response {
        response_type: Some(meta::response::ResponseType::SimpleError(condition as i32)),
    })
}

/// Sets `unsaved`, announcing a change as ZMK does.
fn mark_unsaved(state: &mut State, unsaved: bool) {
    if state.unsaved != unsaved {
        state.unsaved = unsaved;
        push(state, unsaved_notification(unsaved));
    }
}

fn answer_core(state: &mut State, r: core::request::RequestType) -> core::response::ResponseType {
    use core::response::ResponseType as R;
    match r {
        core::request::RequestType::GetLockState(_) => R::GetLockState(if state.locked {
            core::LockState::ZmkStudioCoreLockStateLocked as i32
        } else {
            core::LockState::ZmkStudioCoreLockStateUnlocked as i32
        }),
        core::request::RequestType::ResetSettings(_) => {
            state.saved = state.stock.clone();
            state.live = state.stock.clone();
            mark_unsaved(state, false);
            R::ResetSettings(true)
        }
        _ => R::GetDeviceInfo(core::GetDeviceInfoResponse {
            name: "Fake Studio board".into(),
            serial_number: vec![],
        }),
    }
}

fn answer_keymap(
    state: &mut State,
    r: keymap::request::RequestType,
) -> keymap::response::ResponseType {
    use keymap::request::RequestType as Q;
    use keymap::response::ResponseType as R;
    match r {
        Q::GetKeymap(_) => R::GetKeymap(keymap::Keymap {
            layers: state.live.clone(),
            available_layers: 0,
            max_layer_name_length: 20,
        }),
        Q::SetLayerBinding(set) => {
            let keys = state.keys;
            let slot = state
                .live
                .iter_mut()
                .find(|l| l.id == set.layer_id)
                .and_then(|l| {
                    usize::try_from(set.key_position)
                        .ok()
                        .filter(|p| *p < keys)
                        .map(|p| &mut l.bindings[p])
                });
            let code = match (slot, set.binding) {
                (Some(slot), Some(b)) => {
                    *slot = b;
                    mark_unsaved(state, true);
                    keymap::SetLayerBindingResponse::SetLayerBindingRespOk
                }
                _ => keymap::SetLayerBindingResponse::SetLayerBindingRespInvalidLocation,
            };
            R::SetLayerBinding(code as i32)
        }
        Q::CheckUnsavedChanges(_) => R::CheckUnsavedChanges(state.unsaved),
        Q::SaveChanges(_) => {
            state.saved = state.live.clone();
            mark_unsaved(state, false);
            R::SaveChanges(keymap::SaveChangesResponse {
                result: Some(keymap::save_changes_response::Result::Ok(true)),
            })
        }
        Q::DiscardChanges(_) => {
            let had = state.unsaved;
            state.live = state.saved.clone();
            mark_unsaved(state, false);
            R::DiscardChanges(had)
        }
        _ => R::GetPhysicalLayouts(keymap::PhysicalLayouts::default()),
    }
}

fn answer_behaviors(r: behaviors::request::RequestType) -> behaviors::response::ResponseType {
    use behaviors::response::ResponseType as R;
    match r {
        behaviors::request::RequestType::ListAllBehaviors(_) => {
            R::ListAllBehaviors(behaviors::ListAllBehaviorsResponse {
                behaviors: BEHAVIORS.iter().map(|(id, _)| *id).collect(),
            })
        }
        behaviors::request::RequestType::GetBehaviorDetails(q) => {
            let role = BEHAVIORS
                .iter()
                .find(|(id, _)| *id == q.behavior_id)
                .map_or(BehaviorRole::None, |(_, role)| *role);
            R::GetBehaviorDetails(behaviors::GetBehaviorDetailsResponse {
                id: q.behavior_id,
                display_name: role.label().into(),
                metadata: vec![],
            })
        }
    }
}

/// The board's answer to one request. A locked board answers only the lock state and its
/// device info (ZMK's unsecured requests).
fn answer(state: &mut State, request: studio::Request) -> studio::Response {
    use studio::request::Subsystem as Q;
    use studio::request_response::Subsystem as R;
    let id = request.request_id;
    let unsecured = matches!(
        &request.subsystem,
        Some(Q::Core(core::Request {
            request_type: Some(
                core::request::RequestType::GetLockState(_)
                    | core::request::RequestType::GetDeviceInfo(_)
            ),
        }))
    );
    if state.locked && !unsecured {
        return reply(id, meta_error(meta::ErrorConditions::UnlockRequired));
    }
    let subsystem = match request.subsystem {
        Some(Q::Core(core::Request {
            request_type: Some(r),
        })) => R::Core(core::Response {
            response_type: Some(answer_core(state, r)),
        }),
        Some(Q::Keymap(keymap::Request {
            request_type: Some(r),
        })) => R::Keymap(keymap::Response {
            response_type: Some(answer_keymap(state, r)),
        }),
        Some(Q::Behaviors(behaviors::Request {
            request_type: Some(r),
        })) => R::Behaviors(behaviors::Response {
            response_type: Some(answer_behaviors(r)),
        }),
        _ => meta_error(meta::ErrorConditions::RpcNotFound),
    };
    reply(id, subsystem)
}

/// One open link to the fake board.
pub(crate) struct FakeLine {
    board: Arc<Shared>,
    frames: Frames,
}

impl Read for FakeLine {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let state = self.board.state.lock().unwrap();
        // Like a serial port's read timeout: wait a little for bytes, then report none.
        let (mut state, _) = self
            .board
            .ready
            .wait_timeout_while(state, Duration::from_millis(20), |s| s.out.is_empty())
            .unwrap();
        if state.out.is_empty() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let n = buf.len().min(state.out.len());
        for (slot, byte) in buf.iter_mut().zip(state.out.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}

impl Write for FakeLine {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.board.state.lock().unwrap();
        for frame in self.frames.push(buf) {
            if let Ok(request) = studio::Request::decode(frame.as_slice()) {
                let response = answer(&mut state, request);
                push(&mut state, response);
            }
        }
        self.board.ready.notify_all();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A Studio keyboard protocol over the fake board, as `protocols::zmk` is over a real one: the
/// same [`StudioEditor`], with a reader that sleeps until a command wakes it.
pub(crate) struct FakeStudioProtocol {
    definition: KeyboardDefinition,
    snapshot: Mutex<KeymapSnapshot>,
    studio: StudioEditor,
    wake: Sender<()>,
    woken: Receiver<()>,
}

pub(crate) const LAYOUT: &str = "active physical layout";

impl FakeStudioProtocol {
    pub(crate) fn new(board: &FakeBoard) -> Self {
        let keys = board.keys();
        let per_row = 10;
        let definition = KeyboardDefinition {
            vid: 0x1d50,
            pid: 0x615e,
            rows: 1,
            cols: keys,
            layouts: vec![KeyboardLayout {
                name: LAYOUT.into(),
                keys: (0..keys)
                    .map(|i| Key {
                        row: 0,
                        col: i,
                        x: (i % per_row) as f32,
                        y: (i / per_row) as f32,
                        w: 1.0,
                        h: 1.0,
                        r: 0.0,
                    })
                    .collect(),
            }],
        };
        let snapshot = crate::protocols::zmk::snapshot_from_resolved(&board.resolved(), keys);
        let opener = board.clone();
        let (wake, woken) = mpsc::channel();
        Self {
            definition,
            snapshot: Mutex::new(snapshot),
            studio: StudioEditor::new(Box::new(move || Ok(Link::new(opener.line())))),
            wake,
            woken,
        }
    }
}

impl KeyboardProtocol for FakeStudioProtocol {
    fn get_layout_definition(&self) -> &KeyboardDefinition {
        &self.definition
    }

    fn read_keymap(&self) -> Result<KeymapSnapshot, Box<dyn std::error::Error>> {
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn hid_read(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        match self.woken.recv() {
            Ok(()) => Ok(Vec::new()),
            Err(_) => Err("gone".into()),
        }
    }

    fn write_support(&self) -> WriteSupport {
        WriteSupport::Session
    }

    fn set_key(
        &mut self,
        layer: &LayerInfo,
        layer_index: usize,
        _row: usize,
        col: usize,
        action: &KeyAction,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let KeyAction::Zmk(behavior) = action else {
            return Err("not a ZMK binding".into());
        };
        self.studio
            .set_key(layer.id, col as i32, behavior.clone())?;
        if let Some(cell) = self.snapshot.lock().unwrap().actions[layer_index][0].get_mut(col) {
            *cell = Some(action.clone());
        }
        Ok(())
    }

    fn save_keymap(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.studio.save()
    }

    fn end_edit_session(&mut self) {
        self.studio.end();
    }

    fn studio(&mut self, request: StudioRequest, done: StudioDone) {
        let keys = self.definition.cols;
        let snapshot = &self.snapshot;
        self.studio.handle(request, done, |layers, _| {
            let fresh = crate::protocols::zmk::snapshot_from_resolved(layers, keys);
            *snapshot.lock().unwrap() = fresh.clone();
            fresh
        });
    }

    fn reader_waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let wake = self.wake.clone();
        Some(Arc::new(move || {
            let _ = wake.send(());
        }))
    }
}
