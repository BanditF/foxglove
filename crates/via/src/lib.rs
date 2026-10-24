//! A device-layer backend for QMK keyboards that speak VIA over Raw HID (usage page 0xFF60, 32-byte
//! reports), first for the Keychron K5 Pro (#74).
//!
//! - [`protocol`]: VIA's commands and the allowlist every report passes through
//! - [`definition`]: a VIA JSON keyboard definition (matrix, layout, lighting)
//! - [`device`]: the keyboard as a device-layer device (keymap read, RGB Matrix lighting)
//! - `hid` (feature `hid`): the hidapi transport and backend
//!
//! Safety first: only allowlisted commands reach the keyboard. Reading is always allowed;
//! writes (keycodes, lighting) only when the device was opened with writes enabled; reset,
//! EEPROM reset, bootloader and Keychron's 0xAA-0xAB factory/DFU commands never.

pub mod definition;
pub mod device;
#[cfg(feature = "hid")]
pub mod hid;
pub mod location;
pub mod protocol;

pub use definition::Definition;
pub use device::ViaDevice;
pub use protocol::{Guarded, Transport};
