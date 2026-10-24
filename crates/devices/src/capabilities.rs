use crate::{DeviceError, Result};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

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
        Err(DeviceError::Unsupported)
    }

    /// Commits pending session edits when supported.
    fn save(&self) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Discards pending session edits when supported.
    fn discard(&self) -> Result<()> {
        Err(DeviceError::Unsupported)
    }
}

/// Reads and controls active layers.
pub trait Layers {
    /// Returns the active layer indices.
    fn active(&self) -> Result<Vec<u8>>;

    /// Replaces the active layers when supported.
    fn set(&self, _layers: &[u8]) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Toggles one layer when supported.
    fn toggle(&self, _layer: u8) -> Result<()> {
        Err(DeviceError::Unsupported)
    }
}

/// Whether a behavior needs user confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    Safe,
    Confirm,
}

/// One action the device offers: a behavior with its parameters, as a key binding has them
/// (`&bt BT_SEL 0` and `&bt BT_CLR` are separate actions with their own risk).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BehaviorInfo {
    pub id: u32,
    pub param1: u32,
    pub param2: u32,
    pub name: String,
    /// How long the key is held; 0 = a tap.
    pub hold_ms: u16,
    pub risk: Risk,
}

/// Finds a listed action and checks its risk. Actions that aren't listed are unsupported, so
/// the list is also the allowlist.
pub fn confirm_behavior(
    list: &[BehaviorInfo],
    id: u32,
    p1: u32,
    p2: u32,
    confirmed: bool,
) -> Result<BehaviorInfo> {
    let info = list
        .iter()
        .find(|b| (b.id, b.param1, b.param2) == (id, p1, p2))
        .ok_or(DeviceError::Unsupported)?;
    if info.risk == Risk::Confirm && !confirmed {
        return Err(DeviceError::Denied(info.name.clone()));
    }
    Ok(info.clone())
}

/// Proof that the device layer checked an action's risk, carrying the action. Only the device layer can make one, so
/// nothing can reach a backend's [`Behaviors::invoke_checked`] around the check.
pub struct Checked(BehaviorInfo);

impl Checked {
    pub fn action(&self) -> &BehaviorInfo {
        &self.0
    }
}

/// Lists and invokes device actions. Callers use `invoke` on `dyn Behaviors`.
pub trait Behaviors {
    /// Lists available actions and their confirmation requirements.
    fn list(&self) -> Result<Vec<BehaviorInfo>>;

    /// Runs an action the device layer has already checked; backends implement this.
    fn invoke_checked(&self, checked: Checked) -> Result<()>;
}

impl dyn Behaviors + '_ {
    /// Invokes an action; a [`Risk::Confirm`] action needs `confirmed` (a user's click).
    pub fn invoke(&self, id: u32, p1: u32, p2: u32, confirmed: bool) -> Result<()> {
        let action = confirm_behavior(&self.list()?, id, p1, p2, confirmed)?;
        self.invoke_checked(Checked(action))
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
    /// Every part of the device shows this: false while a split keyboard's other half hasn't
    /// caught up yet (it can lag seconds behind a change).
    pub settled: bool,
}

/// Reads lighting state and optionally controls LEDs.
pub trait Lighting {
    /// Lists LED zones and positions.
    fn layout(&self) -> Result<Vec<Zone>>;

    /// Reads the current lighting controls.
    fn state(&self) -> Result<LightingState>;

    /// Selects an effect when supported.
    fn set_effect(&self, _effect: Effect) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Turns the lights on or off when supported.
    fn set_on(&self, _on: bool) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Sets one colour for the whole device (hue 0-359, saturation and brightness 0-100).
    fn set_color(&self, _hue: u16, _sat: u8, _brightness: u8) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Sets the effect's speed (as [`LightingState::speed`] reports it) when supported.
    fn set_speed(&self, _speed: u8) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Sends colors in LED layout order when supported.
    fn set_frame(&self, _frame: &[Rgb]) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Returns lighting control to the device when supported.
    fn release(&self) -> Result<()> {
        Err(DeviceError::Unsupported)
    }
}

/// Color in: colors computed elsewhere (vendor software, an E1.31 sender) that Foxglove can
/// mirror onto other devices. Separate from [`Lighting`], which is RGB out.
pub trait ColorSource {
    /// Lists the zones the frames describe.
    fn layout(&self) -> Result<Vec<Zone>>;

    /// Streams frames in layout order until the receiver is dropped.
    fn subscribe(&self) -> Result<Receiver<Vec<Rgb>>>;
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
        Err(DeviceError::Unsupported)
    }

    /// Reads the art selection and transfer state.
    fn art_status(&self) -> Result<ArtStatus> {
        Err(DeviceError::Unsupported)
    }

    /// Transfers art with progress and cancellation when supported.
    fn push_art(
        &self,
        _bytes: &[u8],
        _progress: &mut dyn FnMut(usize, usize),
        _cancel: &AtomicBool,
    ) -> Result<()> {
        Err(DeviceError::Unsupported)
    }

    /// Selects an art slot, or clears selection, when supported.
    fn select_art(&self, _index: Option<u8>) -> Result<()> {
        Err(DeviceError::Unsupported)
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
        Err(DeviceError::Unsupported)
    }
}
