//! A device-layer backend for QMK keyboards that speak VIA over Raw HID (usage page 0xFF60, 32-byte
//! reports), first for the Keychron K5 Pro (#74).
//!
//! - [`protocol`]: VIA's commands and the allowlist every report passes through
//! - [`client`]: the read client (protocol version, layers, layout options, keymap buffer,
//!   lighting channels)
//! - [`catalog`]: vendor and board quirks, as data (`qmk-catalog.toml`)
//! - [`definition`]: a VIA JSON keyboard definition (matrix, layout and its layout options,
//!   lighting)
//! - [`qmk`]: a QMK keyboard.json / info.json as a definition (matrix and a `LAYOUT_*` layout),
//!   for boards with no VIA definition
//! - [`kle`]: a raw KLE (keyboard-layout-editor.com) layout
//! - [`layout_file`]: a layout file the user loads for a keyboard's drawing (VIA, QMK or KLE),
//!   bounded in size (#798)
//! - [`device`]: the keyboard as a device-layer device (keymap read, lighting)
//! - [`vial`]: a Vial board's own definition, read from the board (bounded) and decompressed
//! - [`discovery`]: which Raw HID interfaces are VIA keyboards (with a definition, or any QMK
//!   board in generic mode)
//! - `hid` (feature `hid`): the hidapi transport and backend
//!
//! Safety first: only allowlisted commands reach the keyboard. Reading is always allowed;
//! writes (keycodes, lighting values and saves, layout options) only when the crate is built
//! with the `via-writes` feature (off by default, #581), the hidden dev switch
//! `FOXGLOVE_VIA_WRITES=1` is set (#723, for the real-board test), and the device was opened with
//! writes; reset, EEPROM reset, bootloader, Vial unlock and Keychron's 0xAA-0xAB factory/DFU
//! commands never.

pub mod catalog;
pub mod client;
pub mod definition;
pub mod device;
pub mod discovery;
#[cfg(feature = "hid")]
pub mod hid;
pub mod kle;
pub mod layout_file;
pub mod location;
pub mod protocol;
pub mod qmk;
pub mod vial;

pub use catalog::Catalog;
pub use definition::Definition;
pub use device::{Source, Summary, ViaDevice};
pub use protocol::{writes_enabled, Guarded, Transport, WRITES_BUILT, WRITES_ENV};
