//! A complete in-memory backend for tests and examples.
use crate::*;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// An in-memory keyboard backend with controllable presence.
#[derive(Clone)]
pub struct MockBackend {
    info: DeviceInfo,
    pub present: Arc<AtomicBool>,
}

impl Default for MockBackend {
    fn default() -> Self {
        let backend = BackendId("mock".into());
        Self {
            info: DeviceInfo {
                id: DeviceId::new(backend.clone(), 0x1234, 1, Some("mock-1"), "mock/path"),
                backend,
                name: "Mock keyboard".into(),
                kind: DeviceKind::Keyboard,
                firmware: Some("1".into()),
            },
            present: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl Backend for MockBackend {
    fn id(&self) -> BackendId {
        self.info.backend.clone()
    }

    fn discover(&self) -> Result<Vec<DeviceInfo>> {
        Ok(if self.present.load(Ordering::Relaxed) {
            vec![self.info.clone()]
        } else {
            vec![]
        })
    }

    fn open(&self, info: &DeviceInfo, sink: EventSink) -> Result<Box<dyn Device>> {
        if !self.present.load(Ordering::Relaxed) || info.id != self.info.id {
            return Err(DeviceError::Disconnected);
        }
        Ok(Box::new(MockDevice::new(info.clone(), sink)))
    }
}

struct State {
    binding: Binding,
    layers: Vec<u8>,
    lighting: LightingState,
    frame: Vec<Rgb>,
    released: bool,
    panel: Vec<u8>,
    art: Vec<u8>,
    art_status: ArtStatus,
    settings: HashMap<String, Value>,
    invocations: Vec<(u32, u32, u32)>,
}

/// An in-memory device implementing all capabilities.
pub struct MockDevice {
    info: DeviceInfo,
    pub sink: EventSink,
    state: Mutex<State>,
}

impl MockDevice {
    pub fn new(info: DeviceInfo, sink: EventSink) -> Self {
        Self {
            info,
            sink,
            state: Mutex::new(State {
                binding: Binding {
                    kind: "key".into(),
                    params: vec![4],
                    label: Some("A".into()),
                },
                layers: vec![0],
                lighting: LightingState {
                    on: true,
                    hue: 0,
                    sat: 255,
                    brightness: 128,
                    effect: Effect {
                        id: "solid".into(),
                        params: vec![],
                    },
                    speed: 0,
                    settled: true,
                },
                frame: vec![],
                released: true,
                panel: vec![],
                art: vec![],
                art_status: ArtStatus::default(),
                settings: HashMap::new(),
                invocations: vec![],
            }),
        }
    }

    pub fn frame(&self) -> Vec<Rgb> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame
            .clone()
    }

    pub fn released(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .released
    }

    pub fn panel(&self) -> Vec<u8> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .panel
            .clone()
    }

    pub fn art(&self) -> Vec<u8> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .art
            .clone()
    }

    pub fn invocations(&self) -> Vec<(u32, u32, u32)> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invocations
            .clone()
    }
}

impl Device for MockDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn caps(&self) -> Caps {
        Caps::KEYMAP
            | Caps::KEYMAP_WRITE
            | Caps::LAYERS
            | Caps::KEY_EVENTS
            | Caps::BEHAVIORS
            | Caps::LIGHTING
            | Caps::SCREENS
            | Caps::POWER
            | Caps::SETTINGS
    }

    fn keymap(&self) -> Option<&dyn Keymap> {
        Some(self)
    }

    fn layers(&self) -> Option<&dyn Layers> {
        Some(self)
    }

    fn behaviors(&self) -> Option<&dyn Behaviors> {
        Some(self)
    }

    fn lighting(&self) -> Option<&dyn Lighting> {
        Some(self)
    }

    fn screens(&self) -> Option<&dyn Screens> {
        Some(self)
    }

    fn power(&self) -> Option<&dyn Power> {
        Some(self)
    }

    fn settings(&self) -> Option<&dyn DevSettings> {
        Some(self)
    }
}

impl Keymap for MockDevice {
    fn layout(&self) -> Result<Layout> {
        Ok(Layout {
            name: "Mock".into(),
            keys: vec![Key {
                row: 0,
                col: 0,
                x: 0.,
                y: 0.,
                w: 1.,
                h: 1.,
                r: 0.,
            }],
        })
    }

    fn layers(&self) -> Result<Vec<LayerInfo>> {
        Ok(vec![LayerInfo {
            id: 0,
            name: Some("Base".into()),
        }])
    }

    fn binding(&self, layer: u32, row: u16, col: u16) -> Result<Binding> {
        if (layer, row, col) != (0, 0, 0) {
            return Err(DeviceError::Unsupported);
        }
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .binding
            .clone())
    }

    fn write_support(&self) -> WriteSupport {
        WriteSupport::Immediate
    }

    fn set_binding(&self, layer: u32, row: u16, col: u16, binding: Binding) -> Result<()> {
        self.binding(layer, row, col)?;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).binding = binding;
        Ok(())
    }

    fn save(&self) -> Result<()> {
        Ok(())
    }

    fn discard(&self) -> Result<()> {
        Ok(())
    }
}

impl Layers for MockDevice {
    fn active(&self) -> Result<Vec<u8>> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .layers
            .clone())
    }

    fn set(&self, layers: &[u8]) -> Result<()> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).layers = layers.to_vec();
        self.sink.emit(DeviceEvent::LayerChanged {
            active: layers.to_vec(),
        });
        Ok(())
    }

    fn toggle(&self, layer: u8) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.layers.contains(&layer) {
            state.layers.retain(|l| *l != layer);
        } else {
            state.layers.push(layer);
        }
        let active = state.layers.clone();
        drop(state);
        self.sink.emit(DeviceEvent::LayerChanged { active });
        Ok(())
    }
}

impl Behaviors for MockDevice {
    fn list(&self) -> Result<Vec<BehaviorInfo>> {
        Ok(vec![
            BehaviorInfo {
                id: 0,
                param1: 1,
                param2: 2,
                name: "Reset".into(),
                hold_ms: 0,
                risk: Risk::Confirm,
            },
            BehaviorInfo {
                id: 1,
                param1: 0,
                param2: 0,
                name: "Ping".into(),
                hold_ms: 0,
                risk: Risk::Safe,
            },
        ])
    }

    fn invoke_checked(&self, checked: Checked) -> Result<()> {
        let a = checked.action();
        let (id, p1, p2) = (a.id, a.param1, a.param2);
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invocations
            .push((id, p1, p2));
        Ok(())
    }
}

impl Lighting for MockDevice {
    fn layout(&self) -> Result<Vec<Zone>> {
        Ok(vec![Zone {
            name: "Keys".into(),
            leds: vec![Led {
                id: 0,
                position: Some((0., 0., 0.)),
            }],
        }])
    }

    fn state(&self) -> Result<LightingState> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lighting
            .clone())
    }

    fn set_effect(&self, effect: Effect) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.lighting.effect = effect;
        state.released = false;
        let lighting = state.lighting.clone();
        drop(state);
        self.sink
            .emit(DeviceEvent::LightingChanged { state: lighting });
        Ok(())
    }

    fn set_frame(&self, frame: &[Rgb]) -> Result<()> {
        if frame.len() != 1 {
            return Err(DeviceError::Backend("expected one LED".into()));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.frame = frame.to_vec();
        state.released = false;
        Ok(())
    }

    fn release(&self) -> Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .released = true;
        Ok(())
    }
}

impl Screens for MockDevice {
    fn send_panel(&self, bitmap: &[u8], w: u16, h: u16) -> Result<()> {
        if bitmap.len() != (usize::from(w) * usize::from(h)).div_ceil(8) {
            return Err(DeviceError::Backend("invalid 1-bit bitmap size".into()));
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).panel = bitmap.to_vec();
        Ok(())
    }

    fn art_status(&self) -> Result<ArtStatus> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .art_status
            .clone())
    }

    fn push_art(
        &self,
        bytes: &[u8],
        progress: &mut dyn FnMut(usize, usize),
        cancel: &AtomicBool,
    ) -> Result<()> {
        // Stage locally, so cancellation never replaces the previous art.
        let mut staged = Vec::new();
        progress(0, bytes.len());
        for chunk in bytes.chunks(64) {
            if cancel.load(Ordering::Relaxed) {
                return Err(DeviceError::Denied("art transfer cancelled".into()));
            }
            staged.extend_from_slice(chunk);
            progress(staged.len(), bytes.len());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(DeviceError::Denied("art transfer cancelled".into()));
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.art = staged;
        state.art_status.bytes = bytes.len();
        Ok(())
    }

    fn select_art(&self, index: Option<u8>) -> Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .art_status
            .selected = index;
        Ok(())
    }
}

impl Power for MockDevice {
    fn batteries(&self) -> Result<Vec<(String, u8)>> {
        Ok(vec![("main".into(), 100)])
    }

    fn info(&self) -> Result<PowerInfo> {
        Ok(PowerInfo {
            output: Some("USB".into()),
            profile: None,
            profile_connected: None,
        })
    }
}

impl DevSettings for MockDevice {
    fn get(&self, key: &str) -> Result<Option<Value>> {
        Ok(self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .settings
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, value: Value) -> Result<()> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .settings
            .insert(key.into(), value);
        Ok(())
    }
}
