use super::layout_geometry::flattened_top_left_after_center_rotation;
use super::zmk_rpc::{self, ZmkData, ZmkTransport};
use super::{Key, KeyboardDefinition, KeyboardLayout, KeyboardProtocol, Reopener, WriteSupport};
use crate::foxglove::studio_session::{StudioDone, StudioEditor, StudioRequest};
use crate::key_action::{KeyAction, KeymapSnapshot, LayerInfo};
use foxglove_link::link::{Event as LinkEvent, EventSubscription, Link};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zmk_studio_api::{BehaviorBindingParametersSet, BehaviorRole, ResolvedLayer};

struct ZmkLayout {
    definition: KeyboardDefinition,
    snapshot: Mutex<KeymapSnapshot>,
    supported_behaviors: HashSet<BehaviorRole>,
    behavior_metadata: HashMap<BehaviorRole, Vec<BehaviorBindingParametersSet>>,
}

/// What the keyboard reader is woken for (#282): a report, the link going away, or (#578) a
/// keymap command waiting to run.
enum Incoming {
    Report([u8; foxglove_link::protocol::REPORT]),
    Disconnected,
    Wake,
}

pub struct ZmkProtocol {
    // Foxglove: reports come from the shared link instead of a HID handle of our own. The
    // reader blocks on them; a disconnect wakes it with `Incoming::Disconnected`, so an idle
    // keyboard costs no wakeups (#282). Dropping the protocol ends both taps.
    link: Arc<Link>,
    reports: Receiver<Incoming>,
    wake: mpsc::Sender<Incoming>,
    _taps: (EventSubscription, EventSubscription),
    layout: Arc<ZmkLayout>,
    transport: ZmkTransport,
    // Foxglove (#578): the Studio edit session (lock, Save/Discard, restore stock).
    studio: StudioEditor,
}

struct ZmkReopener {
    layout: Arc<ZmkLayout>,
    transport: ZmkTransport,
}

impl Reopener for ZmkReopener {
    fn reopen(&self) -> Result<Box<dyn KeyboardProtocol>, Box<dyn Error>> {
        // Foxglove: read the keymap again, so a keyboard that comes back with new firmware or
        // Studio edits shows them. The keyboard may still be starting, so try a few times; keep
        // the old keymap only if it can't be asked (Studio unreachable or locked).
        let (vid, pid) = (self.layout.definition.vid, self.layout.definition.pid);
        // Ask only once the keyboard is back, so a slow restart doesn't use up the tries.
        crate::foxglove::reconnect::require_hid(vid, pid)?;
        let fresh = (0..3).find_map(|attempt| {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(1500));
            }
            fetch_and_save(vid, pid, &self.transport)
                .ok()
                .and_then(|data| build_from_zmk_data(vid, pid, data).ok())
        });
        let layout = fresh.map_or_else(|| Arc::clone(&self.layout), Arc::new);
        Ok(Box::new(ZmkProtocol::open_hid(
            layout,
            self.transport.clone(),
        )?))
    }
}

/// Reads the keymap from the keyboard and keeps a copy of what it said (Foxglove).
fn fetch_and_save(vid: u16, pid: u16, transport: &ZmkTransport) -> Result<ZmkData, Box<dyn Error>> {
    let (data, recording) = zmk_rpc::fetch_zmk_data(transport)?;
    if let Some(recording) = recording {
        crate::foxglove::keymap_cache::save(vid, pid, &recording, &data);
    }
    Ok(data)
}

impl ZmkProtocol {
    pub fn connect_live(
        vid: u16,
        pid: u16,
        transport: &ZmkTransport,
    ) -> Result<Self, Box<dyn Error>> {
        crate::foxglove::reconnect::require_hid(vid, pid)?;
        // Foxglove: when Studio can't be asked (Bluetooth only, locked), start from the keymap
        // it last sent, so the overlay works without a cable.
        let zmk_data = match fetch_and_save(vid, pid, transport) {
            Ok(data) => data,
            Err(e) => crate::foxglove::keymap_cache::replay(vid, pid)
                .and_then(|client| zmk_rpc::fetch_zmk_data_from_client(client).ok())
                .ok_or(e)?,
        };
        let layout = build_from_zmk_data(vid, pid, zmk_data)?;
        Self::open_hid(Arc::new(layout), transport.clone())
    }

    fn open_hid(layout: Arc<ZmkLayout>, transport: ZmkTransport) -> Result<Self, Box<dyn Error>> {
        let (vid, pid) = (layout.definition.vid, layout.definition.pid);
        crate::foxglove::reconnect::require_hid(vid, pid).map_err(std::io::Error::other)?;
        let connect_error = |e: String| {
            std::io::Error::other(format!(
                "Failed to connect HID ({vid:04x}:{pid:04x}) after reappearance: {e}"
            ))
        };
        let link = crate::foxglove::link::shared(vid, pid).map_err(connect_error)?;
        if link.state().connected.is_none() {
            link.reconnect();
        }
        // Subscribe before waiting, so reports sent while the link connects aren't missed.
        let (tx, reports) = mpsc::channel();
        let report_tx = tx.clone();
        let wake = tx.clone();
        let report_tap = link.subscribe_reports(move |_, r| {
            let _ = report_tx.send(Incoming::Report(*r));
        });
        let state_tap = link.subscribe_events(move |event| {
            if let LinkEvent::State(state) = event {
                if state.connected.is_none() {
                    let _ = tx.send(Incoming::Disconnected);
                }
            }
        });
        link.wait_connected(Duration::from_secs(8))
            .map_err(|e| connect_error(e.to_string()))?;

        Ok(Self {
            link,
            reports,
            wake,
            _taps: (report_tap, state_tap),
            layout,
            // #592: the saved keymap copy follows Save, Discard and Restore stock.
            studio: StudioEditor::new(zmk_rpc::studio_opener(transport.clone())).on_settled(
                Box::new(move |raw, layouts| {
                    crate::foxglove::keymap_cache::refresh(vid, pid, raw, layouts);
                }),
            ),
            transport,
        })
    }

    fn update_cached_action(&self, layer_index: usize, row: usize, col: usize, action: KeyAction) {
        let mut snapshot = self.layout.snapshot.lock().unwrap();
        if let Some(cell) = snapshot
            .actions
            .get_mut(layer_index)
            .and_then(|layer| layer.get_mut(row))
            .and_then(|r| r.get_mut(col))
        {
            *cell = Some(action);
        }
    }
}

impl KeyboardProtocol for ZmkProtocol {
    fn get_layout_definition(&self) -> &KeyboardDefinition {
        &self.layout.definition
    }

    fn read_keymap(&self) -> Result<KeymapSnapshot, Box<dyn Error>> {
        Ok(self.layout.snapshot.lock().unwrap().clone())
    }

    fn hid_read(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        // Blocks until a report or a disconnect (#282), and errors at once while the keyboard
        // is gone (the reader treats repeated errors as a lost connection and reopens).
        let gone = || -> Result<Vec<u8>, Box<dyn Error>> { Err("HID read error: keyboard disconnected".into()) };
        if self.link.state().connected.is_none() {
            return gone();
        }
        match self.reports.recv() {
            Ok(Incoming::Report(report)) => Ok(report.to_vec()),
            Ok(Incoming::Wake) => Ok(Vec::new()),
            Ok(Incoming::Disconnected) | Err(_) => gone(),
        }
    }

    fn write_support(&self) -> WriteSupport {
        WriteSupport::Session
    }

    fn set_key(
        &mut self,
        layer: &LayerInfo,
        layer_index: usize,
        row: usize,
        col: usize,
        action: &KeyAction,
    ) -> Result<(), Box<dyn Error>> {
        let behavior = match action {
            KeyAction::Zmk(behavior) => behavior.clone(),
            KeyAction::Qmk(_) => return Err("Cannot apply a QMK keycode to a ZMK keyboard".into()),
        };
        if row != 0 {
            return Err(format!("Invalid ZMK key position {row}:{col}").into());
        }

        // ZMK's matrix is 1×N: the column is the key position, and the write
        // RPC addresses layers by their stable id.
        self.studio.set_key(layer.id, col as i32, behavior)?;
        self.update_cached_action(layer_index, row, col, action.clone());
        Ok(())
    }

    fn save_keymap(&mut self) -> Result<(), Box<dyn Error>> {
        self.studio.save()
    }

    fn end_edit_session(&mut self) {
        self.studio.end();
    }

    fn studio(&mut self, request: StudioRequest, done: StudioDone) {
        // Foxglove (#578): a discard or restore changes the keymap; the overlay, the bindings
        // the app can invoke and this protocol's copy follow it.
        let layout = &self.layout;
        let keys = layout.definition.cols;
        self.studio.handle(request, done, |layers, raw| {
            crate::foxglove::keyboard_actions::learn(layers, raw);
            crate::foxglove::layer_access::learn(layers);
            let snapshot = snapshot_from_resolved(layers, keys);
            *layout.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = snapshot.clone();
            snapshot
        });
    }

    fn reader_waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let wake = self.wake.clone();
        Some(Arc::new(move || {
            let _ = wake.send(Incoming::Wake);
        }))
    }

    fn reopener(&self) -> Option<Arc<dyn Reopener>> {
        Some(Arc::new(ZmkReopener {
            layout: Arc::clone(&self.layout),
            transport: self.transport.clone(),
        }))
    }

    fn action_filter(
        &self,
    ) -> Option<Arc<dyn Fn(&crate::key_action::KeyAction) -> bool + Send + Sync>> {
        let supported = self.layout.supported_behaviors.clone();
        let metadata = self.layout.behavior_metadata.clone();
        Some(Arc::new(move |action| match action {
            crate::key_action::KeyAction::Zmk(behavior) => match behavior.role() {
                Some(role) => {
                    if !supported.is_empty() && !supported.contains(&role) {
                        return false;
                    }
                    metadata
                        .get(&role)
                        .is_none_or(|sets| behavior.matches_metadata(sets))
                }
                None => true,
            },
            _ => true,
        }))
    }
}

fn build_from_zmk_data(vid: u16, pid: u16, data: ZmkData) -> Result<ZmkLayout, Box<dyn Error>> {
    const ACTIVE_LAYOUT_NAME: &str = "active physical layout";
    crate::foxglove::keyboard_actions::learn(&data.resolved_layers, &data.raw_keymap);
    crate::foxglove::layer_access::learn(&data.resolved_layers);

    let active_idx = data.physical_layouts.active_layout_index as usize;
    let proto_layouts = &data.physical_layouts.layouts;

    if proto_layouts.is_empty() {
        return Err("Device has no physical layouts".into());
    }

    let active_layout = proto_layouts
        .get(active_idx)
        .ok_or_else(|| format!("Invalid active layout index: {active_idx}"))?;
    let active_keys: Vec<Key> = active_layout
        .keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let w = k.width as f32 / 100.0;
            let h = k.height as f32 / 100.0;

            let x = k.x as f32 / 100.0;
            let y = k.y as f32 / 100.0;

            // Position is where the key's center lands after rotating around the pivot;
            // the rotation itself is applied at render time via `r`.
            let angle_deg = k.r as f32 / 100.0;
            let pivot_x = if k.rx == 0 { k.x } else { k.rx } as f32 / 100.0;
            let pivot_y = if k.ry == 0 { k.y } else { k.ry } as f32 / 100.0;
            let (x, y) =
                flattened_top_left_after_center_rotation(x, y, w, h, angle_deg, pivot_x, pivot_y);

            Key {
                row: 0,
                col: i,
                x,
                y,
                w,
                h,
                r: angle_deg,
            }
        })
        .collect();
    let num_keys = active_keys.len();
    crate::foxglove::boards::keymap_keys(num_keys);

    let definition = KeyboardDefinition {
        vid,
        pid,
        rows: 1,
        cols: num_keys,
        layouts: vec![KeyboardLayout {
            name: ACTIVE_LAYOUT_NAME.to_string(),
            keys: active_keys,
        }],
    };

    let snapshot = snapshot_from_resolved(&data.resolved_layers, num_keys);

    Ok(ZmkLayout {
        definition,
        snapshot: Mutex::new(snapshot),
        supported_behaviors: data.supported_behaviors,
        behavior_metadata: data.behavior_metadata,
    })
}

/// Builds a keymap snapshot from the device's resolved layers. Bindings beyond
/// `num_keys` are dropped and short layers are padded with `None`, matching the
/// matrix dimensions.
pub(crate) fn snapshot_from_resolved(
    resolved: &[ResolvedLayer],
    num_keys: usize,
) -> KeymapSnapshot {
    let layers = resolved
        .iter()
        .map(|layer| LayerInfo {
            id: layer.id,
            name: (!layer.name.is_empty()).then(|| layer.name.clone()),
        })
        .collect();

    let actions = resolved
        .iter()
        .map(|layer| {
            let mut row: Vec<Option<KeyAction>> = layer
                .bindings
                .iter()
                .map(|b| Some(KeyAction::Zmk(b.clone())))
                .collect();
            row.resize(num_keys, None);
            vec![row]
        })
        .collect();

    KeymapSnapshot { layers, actions }
}
