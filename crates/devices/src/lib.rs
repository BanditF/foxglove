//! One device interface for Foxglove backends. Discovery is caller-driven.

pub mod actor;

mod capabilities;

mod events;

pub mod facts;

mod registry;

mod types;

pub use capabilities::*;
pub use events::*;
pub use registry::*;
pub use types::*;

#[cfg(any(test, feature = "contract"))]
pub mod contract;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

/// The result of a device-layer operation.
pub type Result<T> = std::result::Result<T, DeviceError>;

/// An operation failure reported by the device layer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceError {
    #[error("unsupported operation")]
    Unsupported,
    #[error("device disconnected")]
    Disconnected,
    #[error("busy: {0}")]
    Busy(String),
    #[error("operation timed out")]
    Timeout,
    #[error("denied: {0}")]
    Denied(String),
    #[error("backend: {0}")]
    Backend(String),
}

/// Vendor device registration: presents a device-layer device inside another lighting ecosystem (a
/// SignalRGB device plugin, an OpenRGB virtual controller) so that ecosystem can drive it.
/// Plugins implement it; it is not a device capability.
pub trait Exporter: Send + Sync {
    fn id(&self) -> BackendId;

    /// Registers the device and its LED zones with the ecosystem.
    fn export(&self, device: &DeviceInfo, zones: &[Zone]) -> Result<()>;

    /// Removes a device registered by [`Exporter::export`].
    fn withdraw(&self, device: &DeviceId) -> Result<()>;
}

/// Discovers and opens devices in one identity namespace.
pub trait Backend: Send + Sync {
    /// Returns the namespace used by all discovered devices.
    fn id(&self) -> BackendId;

    /// Lists currently present devices in this backend.
    fn discover(&self) -> Result<Vec<DeviceInfo>>;

    /// Opens a discovered device and publishes through its sink.
    fn open(&self, info: &DeviceInfo, sink: EventSink) -> Result<Box<dyn Device>>;
}

/// An open device with optional capability interfaces.
pub trait Device: Send {
    /// Returns the identity and discovery metadata.
    fn info(&self) -> &DeviceInfo;

    /// Returns the supported capability flags.
    fn caps(&self) -> Caps;

    /// Returns the keymap interface when supported.
    fn keymap(&self) -> Option<&dyn Keymap> {
        None
    }

    /// Returns the layers interface when supported.
    fn layers(&self) -> Option<&dyn Layers> {
        None
    }

    /// Returns the behaviors interface when supported.
    fn behaviors(&self) -> Option<&dyn Behaviors> {
        None
    }

    /// Returns the lighting interface when supported.
    fn lighting(&self) -> Option<&dyn Lighting> {
        None
    }

    /// Returns the color-in interface when supported.
    fn color_source(&self) -> Option<&dyn ColorSource> {
        None
    }

    /// Returns the screens interface when supported.
    fn screens(&self) -> Option<&dyn Screens> {
        None
    }

    /// Returns the power interface when supported.
    fn power(&self) -> Option<&dyn Power> {
        None
    }

    /// Returns the settings interface when supported.
    fn settings(&self) -> Option<&dyn DevSettings> {
        None
    }
}
