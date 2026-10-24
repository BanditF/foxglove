use crate::{PalmError, Result};
use std::sync::atomic::AtomicBool;

/// A key position and its physical geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub row: u16,
    pub col: u16,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

/// Physical key geometry for a device.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub name: String,
    pub keys: Vec<Key>,
}

/// A keymap layer and its optional display name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerInfo {
    pub id: u32,
    pub name: Option<String>,
}

/// A backend-defined action assigned to a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub kind: String,
    pub params: Vec<u32>,
    pub label: Option<String>,
}

/// How keymap edits are committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteSupport {
    None,
    Immediate,
    Session,
}

/// Reads key bindings and optionally edits them.
pub trait Keymap {
    /// Returns the physical key geometry.
    fn layout(&self) -> Result<Layout>;

    /// Lists keymap layers.
    fn layers(&self) -> Result<Vec<LayerInfo>>;

    /// Reads the binding at a layer and matrix position.
    fn binding(&self, layer: u32, row: u16, col: u16) -> Result<Binding>;

    /// Describes when edits take effect.
    fn write_support(&self) -> WriteSupport {
        WriteSupport::None
    }

    /// Replaces a binding when writes are supported.
    fn set_binding(&self, _layer: u32, _row: u16, _col: u16, _binding: Binding) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Commits pending session edits when supported.
    fn save(&self) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Discards pending session edits when supported.
    fn discard(&self) -> Result<()> {
        Err(PalmError::Unsupported)
    }
}

/// Reads and controls active layers.
pub trait Layers {
    /// Returns the active layer indices.
    fn active(&self) -> Result<Vec<u8>>;

    /// Replaces the active layers when supported.
    fn set(&self, _layers: &[u8]) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Toggles one layer when supported.
    fn toggle(&self, _layer: u8) -> Result<()> {
        Err(PalmError::Unsupported)
    }
}

/// Whether a behavior needs user confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    Safe,
    Confirm,
}

/// A callable device action and its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BehaviorInfo {
    pub id: u32,
    pub name: String,
    pub params: Vec<String>,
    pub risk: Risk,
}

/// Call before executing a behavior. Unknown ids are unsupported.
pub fn confirm_behavior(list: &[BehaviorInfo], id: u32, confirmed: bool) -> Result<()> {
    let info = list
        .iter()
        .find(|b| b.id == id)
        .ok_or(PalmError::Unsupported)?;
    if info.risk == Risk::Confirm && !confirmed {
        return Err(PalmError::Denied(info.name.clone()));
    }
    Ok(())
}

/// Lists and invokes device actions.
pub trait Behaviors {
    /// Lists available actions and their confirmation requirements.
    fn list(&self) -> Result<Vec<BehaviorInfo>>;

    /// Backends implement this hook and use the default checked entry point.
    fn invoke_confirmed(&self, id: u32, p1: u32, p2: u32) -> Result<()>;

    /// Checks confirmation before invoking an action.
    fn invoke(&self, id: u32, p1: u32, p2: u32, confirmed: bool) -> Result<()> {
        confirm_behavior(&self.list()?, id, confirmed)?;
        self.invoke_confirmed(id, p1, p2)
    }
}

/// An RGB color with eight-bit channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// An LED and its optional physical position.
#[derive(Clone, Debug, PartialEq)]
pub struct Led {
    pub id: u32,
    pub position: Option<(f32, f32, f32)>,
}

/// A named group of LEDs.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub name: String,
    pub leds: Vec<Led>,
}

/// A backend-defined lighting effect and parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    pub id: String,
    pub params: Vec<u32>,
}

/// The current lighting controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightingState {
    pub on: bool,
    pub hue: u16,
    pub sat: u8,
    pub brightness: u8,
    pub effect: Effect,
    pub speed: u8,
}

/// Reads lighting state and optionally controls LEDs.
pub trait Lighting {
    /// Lists LED zones and positions.
    fn layout(&self) -> Result<Vec<Zone>>;

    /// Reads the current lighting controls.
    fn state(&self) -> Result<LightingState>;

    /// Selects an effect when supported.
    fn set_effect(&self, _effect: Effect) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Sends colors in LED layout order when supported.
    fn set_frame(&self, _frame: &[Rgb]) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Returns lighting control to the device when supported.
    fn release(&self) -> Result<()> {
        Err(PalmError::Unsupported)
    }
}

/// The current screen art selection and transfer state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtStatus {
    pub selected: Option<u8>,
    pub bytes: usize,
    pub receiving: bool,
}

/// Optional panel and art transfer operations.
pub trait Screens {
    /// Sends a packed one-bit panel bitmap when supported.
    fn send_panel(&self, _bitmap: &[u8], _w: u16, _h: u16) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Reads the art selection and transfer state.
    fn art_status(&self) -> Result<ArtStatus> {
        Err(PalmError::Unsupported)
    }

    /// Transfers art with progress and cancellation when supported.
    fn push_art(
        &self,
        _bytes: &[u8],
        _progress: &mut dyn FnMut(usize, usize),
        _cancel: &AtomicBool,
    ) -> Result<()> {
        Err(PalmError::Unsupported)
    }

    /// Selects an art slot, or clears selection, when supported.
    fn select_art(&self, _index: Option<u8>) -> Result<()> {
        Err(PalmError::Unsupported)
    }
}

/// Optional power output and profile details.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PowerInfo {
    pub output: Option<String>,
    pub profile: Option<u8>,
    pub profile_connected: Option<bool>,
}

/// Reads battery levels and power details.
pub trait Power {
    /// Returns named battery percentages.
    fn batteries(&self) -> Result<Vec<(String, u8)>>;

    /// Reads optional power output and profile details.
    fn info(&self) -> Result<PowerInfo> {
        Ok(PowerInfo::default())
    }
}

/// Settings and facts share the same small value vocabulary.
pub use crate::facts::Value;

/// Reads and optionally writes device settings.
pub trait DevSettings {
    /// Reads a setting, returning None for an absent key.
    fn get(&self, key: &str) -> Result<Option<Value>>;

    /// Writes a setting when supported.
    fn set(&self, _key: &str, _value: Value) -> Result<()> {
        Err(PalmError::Unsupported)
    }
}
