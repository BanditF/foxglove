//! A fake ZMK Studio board for tests (#578): it answers Studio RPC from memory the way ZMK does
//! (live changes in RAM, Save, Discard, `reset_settings`, the lock and its notifications), so
//! the edit session runs end to end with no keyboard. Nothing here writes to a real board.
//!
//! [`FakeBoard::plank3`] is the second desk's one-piece board with Studio locking on;
//! [`FakeBoard::corne`] is the user's split board, whose build has locking off.
//!
//! #583: reserved layer slots (`available_layers`) that Add layer enables, Remove and Restore,
//! Move, Rename, and physical layouts (the Plank 3 has two).

/// A board's shape beyond its keys: reserved layer slots and physical layout names.
struct Shape {
    reserved: usize,
    layouts: &'static [&'static str],
}

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use prost::Message;
use zmk_studio_api::proto::zmk::{behaviors, core, keymap, meta, studio};
use zmk_studio_api::{Behavior, BehaviorRole, HidUsage, ResolvedLayer};

use super::behavior_catalog::CustomBehavior;
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
/// #582: behaviours the board defines in its own keymap, with their parameter metadata: a key
/// and a count (`hid_usage` + `range`), and two parameter sets (`constant`s, then a `range`).
fn custom_behaviors() -> Vec<CustomBehavior> {
    use crate::foxglove::behavior_catalog::tests::{key_and_count, two_sets};
    vec![key_and_count(5), two_sets(6)]
}
const KEY_PRESS: i32 = 1;
const TRANSPARENT: i32 = 2;
const MOMENTARY: i32 = 3;

struct State {
    keys: usize,
    locked: bool,
    stock: Vec<keymap::Layer>,
    saved: Vec<keymap::Layer>,
    live: Vec<keymap::Layer>,
    /// #583: disabled layer slots (reserved, or removed this session), live/saved/stock.
    spare: Vec<keymap::Layer>,
    saved_spare: Vec<keymap::Layer>,
    stock_spare: Vec<keymap::Layer>,
    layouts: Vec<String>,
    active_layout: u32,
    saved_layout: u32,
    unsaved: bool,
    out: VecDeque<u8>,
    lines_opened: usize,
    /// F6: the next binding write is refused (`INVALID_PARAMETERS`), the binding kept.
    refuse_write: bool,
    /// F6: the next request is applied but its answer is lost and the line breaks.
    lose_reply: bool,
    broken: bool,
    /// F6: the next save fails with this code, the changes kept live and unsaved.
    save_error: Option<keymap::SaveChangesErrorCode>,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
}

#[derive(Clone)]
pub(crate) struct FakeBoard(Arc<Shared>);

impl FakeBoard {
    fn new(keys: usize, layers: &[&str], locked: bool, shape: Shape) -> Self {
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
        let spare: Vec<keymap::Layer> = (0..shape.reserved)
            .map(|i| keymap::Layer {
                id: (layers.len() + i) as u32,
                name: String::new(),
                bindings: vec![binding(TRANSPARENT, 0); keys],
            })
            .collect();
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                keys,
                locked,
                saved: stock.clone(),
                live: stock.clone(),
                stock,
                saved_spare: spare.clone(),
                stock_spare: spare.clone(),
                spare,
                layouts: shape.layouts.iter().map(|s| s.to_string()).collect(),
                active_layout: 0,
                saved_layout: 0,
                unsaved: false,
                out: VecDeque::new(),
                lines_opened: 0,
                refuse_write: false,
                lose_reply: false,
                broken: false,
                save_error: None,
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
            Shape {
                reserved: 2,
                layouts: &["Standard", "Split space"],
            },
        )
    }

    /// The user's Corne: 42 keys, Studio locking off.
    pub(crate) fn corne() -> Self {
        Self::new(
            crate::foxglove::fixtures::users_desk::KEYS,
            &["Base", "Lower", "Raise", "Adjust"],
            false,
            // #580: the Corne build reserves 3 layers for Studio.
            Shape {
                reserved: 3,
                layouts: &["Eyelash Corne"],
            },
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

    /// F6: the board refuses the next binding write and keeps the binding it has.
    pub(crate) fn refuse_next_write(&self) {
        self.state().refuse_write = true;
    }

    /// F6: the board applies the next request, but its answer never arrives: the line breaks.
    pub(crate) fn lose_next_reply(&self) {
        self.state().lose_reply = true;
    }

    /// F6: the next save fails with `code`; the changes stay live and unsaved, as in ZMK.
    pub(crate) fn fail_next_save(&self, code: keymap::SaveChangesErrorCode) {
        self.state().save_error = Some(code);
    }

    pub(crate) fn unsaved(&self) -> bool {
        self.state().unsaved
    }

    /// The live binding at a key: behaviour id and both parameters.
    pub(crate) fn live_binding(&self, layer: usize, pos: usize) -> (i32, u32, u32) {
        let b = self.state().live[layer].bindings[pos];
        (b.behavior_id, b.param1, b.param2)
    }

    /// #583: the live layers' names, in order.
    pub(crate) fn layer_names(&self) -> Vec<String> {
        self.state().live.iter().map(|l| l.name.clone()).collect()
    }

    /// #583: the active physical layout.
    pub(crate) fn active_layout(&self) -> u32 {
        self.state().active_layout
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
        id => match custom_behaviors().into_iter().find(|c| c.id as i32 == id) {
            Some(c) => {
                let (param1, param2) =
                    crate::foxglove::behavior_catalog::typed_params(&c.sets, b.param1, b.param2);
                Behavior::Custom {
                    behavior_id: c.id,
                    display_name: c.name,
                    param1,
                    param2,
                }
            }
            None => Behavior::None,
        },
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
            state.spare = state.stock_spare.clone();
            state.saved_spare = state.stock_spare.clone();
            state.active_layout = 0;
            state.saved_layout = 0;
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
        Q::GetKeymap(_) => R::GetKeymap(live_keymap(state)),
        Q::SetLayerBinding(_) if std::mem::take(&mut state.refuse_write) => R::SetLayerBinding(
            keymap::SetLayerBindingResponse::SetLayerBindingRespInvalidParameters as i32,
        ),
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
        Q::SaveChanges(_) if state.save_error.is_some() => {
            let code = state.save_error.take().map_or(0, |c| c as i32);
            R::SaveChanges(keymap::SaveChangesResponse {
                result: Some(keymap::save_changes_response::Result::Err(code)),
            })
        }
        Q::SaveChanges(_) => {
            state.saved = state.live.clone();
            state.saved_spare = state.spare.clone();
            state.saved_layout = state.active_layout;
            mark_unsaved(state, false);
            R::SaveChanges(keymap::SaveChangesResponse {
                result: Some(keymap::save_changes_response::Result::Ok(true)),
            })
        }
        Q::DiscardChanges(_) => {
            let had = state.unsaved;
            state.live = state.saved.clone();
            state.spare = state.saved_spare.clone();
            state.active_layout = state.saved_layout;
            mark_unsaved(state, false);
            R::DiscardChanges(had)
        }
        Q::GetPhysicalLayouts(_) => R::GetPhysicalLayouts(keymap::PhysicalLayouts {
            active_layout_index: state.active_layout,
            layouts: state
                .layouts
                .iter()
                .map(|name| keymap::PhysicalLayout {
                    name: name.clone(),
                    keys: vec![],
                })
                .collect(),
        }),
        q => answer_layers(state, q),
    }
}

fn live_keymap(state: &State) -> keymap::Keymap {
    keymap::Keymap {
        layers: state.live.clone(),
        available_layers: state.spare.len() as u32,
        max_layer_name_length: 20,
    }
}

/// #583: the layer and physical layout requests, as ZMK answers them (each whole or not at all).
fn answer_layers(
    state: &mut State,
    request: keymap::request::RequestType,
) -> keymap::response::ResponseType {
    use keymap::request::RequestType as Q;
    use keymap::response::ResponseType as R;
    match request {
        Q::AddLayer(_) => {
            let result = if state.spare.is_empty() {
                keymap::add_layer_response::Result::Err(
                    keymap::AddLayerErrorCode::AddLayerErrNoSpace as i32,
                )
            } else {
                let mut layer = state.spare.remove(0);
                layer.name.clear();
                layer.bindings = vec![binding(TRANSPARENT, 0); state.keys];
                state.live.push(layer.clone());
                mark_unsaved(state, true);
                keymap::add_layer_response::Result::Ok(keymap::AddLayerResponseDetails {
                    index: (state.live.len() - 1) as u32,
                    layer: Some(layer),
                })
            };
            R::AddLayer(keymap::AddLayerResponse {
                result: Some(result),
            })
        }
        Q::RemoveLayer(q) => {
            let index = q.layer_index as usize;
            let result = if index < state.live.len() && state.live.len() > 1 {
                let layer = state.live.remove(index);
                state.spare.push(layer);
                mark_unsaved(state, true);
                keymap::remove_layer_response::Result::Ok(keymap::RemoveLayerOk {})
            } else {
                keymap::remove_layer_response::Result::Err(
                    keymap::RemoveLayerErrorCode::RemoveLayerErrInvalidIndex as i32,
                )
            };
            R::RemoveLayer(keymap::RemoveLayerResponse {
                result: Some(result),
            })
        }
        Q::RestoreLayer(q) => {
            let found = state.spare.iter().position(|l| l.id == q.layer_id);
            let result = match found {
                Some(i) if q.at_index as usize <= state.live.len() => {
                    let layer = state.spare.remove(i);
                    state.live.insert(q.at_index as usize, layer.clone());
                    mark_unsaved(state, true);
                    keymap::restore_layer_response::Result::Ok(layer)
                }
                Some(_) => keymap::restore_layer_response::Result::Err(
                    keymap::RestoreLayerErrorCode::RestoreLayerErrInvalidIndex as i32,
                ),
                None => keymap::restore_layer_response::Result::Err(
                    keymap::RestoreLayerErrorCode::RestoreLayerErrInvalidId as i32,
                ),
            };
            R::RestoreLayer(keymap::RestoreLayerResponse {
                result: Some(result),
            })
        }
        Q::MoveLayer(q) => {
            let (from, to) = (q.start_index as usize, q.dest_index as usize);
            let result = if from < state.live.len() && to < state.live.len() {
                let layer = state.live.remove(from);
                state.live.insert(to, layer);
                mark_unsaved(state, true);
                keymap::move_layer_response::Result::Ok(live_keymap(state))
            } else {
                keymap::move_layer_response::Result::Err(
                    keymap::MoveLayerErrorCode::MoveLayerErrInvalidDestination as i32,
                )
            };
            R::MoveLayer(keymap::MoveLayerResponse {
                result: Some(result),
            })
        }
        Q::SetLayerProps(q) => {
            let code = match state.live.iter_mut().find(|l| l.id == q.layer_id) {
                Some(layer) => {
                    layer.name = q.name;
                    mark_unsaved(state, true);
                    keymap::SetLayerPropsResponse::SetLayerPropsRespOk
                }
                None => keymap::SetLayerPropsResponse::SetLayerPropsRespErrInvalidId,
            };
            R::SetLayerProps(code as i32)
        }
        Q::SetActivePhysicalLayout(index) => {
            let result = if (index as usize) < state.layouts.len() {
                state.active_layout = index;
                mark_unsaved(state, true);
                keymap::set_active_physical_layout_response::Result::Ok(live_keymap(state))
            } else {
                keymap::set_active_physical_layout_response::Result::Err(
                    keymap::SetActivePhysicalLayoutErrorCode::SetActivePhysicalLayoutErrInvalidLayoutIndex
                        as i32,
                )
            };
            R::SetActivePhysicalLayout(keymap::SetActivePhysicalLayoutResponse {
                result: Some(result),
            })
        }
        _ => R::GetPhysicalLayouts(keymap::PhysicalLayouts::default()),
    }
}

fn answer_behaviors(r: behaviors::request::RequestType) -> behaviors::response::ResponseType {
    use behaviors::response::ResponseType as R;
    match r {
        behaviors::request::RequestType::ListAllBehaviors(_) => {
            R::ListAllBehaviors(behaviors::ListAllBehaviorsResponse {
                behaviors: BEHAVIORS
                    .iter()
                    .map(|(id, _)| *id)
                    .chain(custom_behaviors().iter().map(|c| c.id))
                    .collect(),
            })
        }
        behaviors::request::RequestType::GetBehaviorDetails(q)
            if custom_behaviors().iter().any(|c| c.id == q.behavior_id) =>
        {
            let c = custom_behaviors()
                .into_iter()
                .find(|c| c.id == q.behavior_id)
                .expect("listed");
            R::GetBehaviorDetails(behaviors::GetBehaviorDetailsResponse {
                id: c.id,
                display_name: c.name,
                metadata: c.sets,
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
        let mut state = self.board.state.lock().unwrap();
        if std::mem::take(&mut state.broken) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
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
                if std::mem::take(&mut state.lose_reply) {
                    state.broken = true;
                    continue;
                }
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
