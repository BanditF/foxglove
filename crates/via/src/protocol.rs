//! VIA's Raw HID commands (QMK `quantum/via.h`) and the allowlist in front of them.
//!
//! Every report starts with a command id; the keyboard answers with the same id, or 0xFF for a
//! command it doesn't handle.

use foxglove_devices::{DeviceError, Result};

pub const REPORT: usize = 32;
pub type Report = [u8; REPORT];

pub const GET_PROTOCOL_VERSION: u8 = 0x01;
pub const GET_KEYBOARD_VALUE: u8 = 0x02;
pub const SET_KEYBOARD_VALUE: u8 = 0x03;
pub const DYNAMIC_KEYMAP_GET_KEYCODE: u8 = 0x04;
pub const DYNAMIC_KEYMAP_SET_KEYCODE: u8 = 0x05;
pub const DYNAMIC_KEYMAP_RESET: u8 = 0x06;
pub const CUSTOM_SET_VALUE: u8 = 0x07;
pub const CUSTOM_GET_VALUE: u8 = 0x08;
pub const CUSTOM_SAVE: u8 = 0x09;
pub const EEPROM_RESET: u8 = 0x0A;
pub const BOOTLOADER_JUMP: u8 = 0x0B;
pub const DYNAMIC_KEYMAP_GET_LAYER_COUNT: u8 = 0x11;
pub const DYNAMIC_KEYMAP_GET_BUFFER: u8 = 0x12;
/// Vial's commands start with this byte (unlock, QMK settings, dynamic entries).
pub const VIAL_PREFIX: u8 = 0xFE;
pub const UNHANDLED: u8 = 0xFF;

/// `GET_KEYBOARD_VALUE` ids.
pub mod keyboard_value {
    pub const UPTIME: u8 = 1;
    pub const LAYOUT_OPTIONS: u8 = 2;
    /// The physical switches as they're pressed now: key events, so never read (#74).
    pub const SWITCH_MATRIX_STATE: u8 = 3;
}

/// VIA custom-value channel and ids for QMK RGB Matrix.
pub mod rgb_matrix {
    pub const CHANNEL: u8 = 3;
    pub const BRIGHTNESS: u8 = 1;
    pub const EFFECT: u8 = 2;
    pub const EFFECT_SPEED: u8 = 3;
    pub const COLOR: u8 = 4;
}

/// Commands that only read.
const READ: &[u8] = &[
    GET_PROTOCOL_VERSION,
    GET_KEYBOARD_VALUE,
    DYNAMIC_KEYMAP_GET_KEYCODE,
    CUSTOM_GET_VALUE,
    DYNAMIC_KEYMAP_GET_LAYER_COUNT,
    DYNAMIC_KEYMAP_GET_BUFFER,
];

/// Commands that change the keyboard, allowed only with writes enabled.
const WRITE: &[u8] = &[DYNAMIC_KEYMAP_SET_KEYCODE, CUSTOM_SET_VALUE, CUSTOM_SAVE];

/// Never sent, whatever the caller asks: resets, the bootloader, and Keychron's Bluetooth
/// DFU/factory-test namespace (0xAA-0xAB, which includes a bootloader jump). Everything not on
/// the read or write lists is refused too; this list is for the error message and the tests.
pub const FORBIDDEN: &[u8] = &[DYNAMIC_KEYMAP_RESET, EEPROM_RESET, BOOTLOADER_JUMP, 0xAA, 0xAB];

/// A connection that exchanges one report for its reply.
pub trait Transport: Send + Sync {
    fn exchange(&self, report: &Report) -> Result<Report>;
}

/// The allowlist in front of a transport. Only this type is handed to the device code, so a
/// report can't reach the keyboard without passing [`Guarded::check`].
pub struct Guarded<T> {
    inner: T,
    writes: bool,
}

impl<T: Transport> Guarded<T> {
    /// Read-only: the safe default (the K5 Pro is the user's daily keyboard).
    pub fn read_only(inner: T) -> Self {
        Guarded { inner, writes: false }
    }

    /// Also allows keycode and lighting writes; still never resets or the bootloader.
    pub fn with_writes(inner: T) -> Self {
        Guarded { inner, writes: true }
    }

    pub fn writes(&self) -> bool {
        self.writes
    }

    /// Whether a report may go out: its command, and for `GET_KEYBOARD_VALUE` which value.
    pub fn check(&self, bytes: &[u8]) -> Result<()> {
        let command = *bytes.first().ok_or(DeviceError::Unsupported)?;
        if command == GET_KEYBOARD_VALUE {
            return match bytes.get(1).copied() {
                Some(keyboard_value::UPTIME | keyboard_value::LAYOUT_OPTIONS) => Ok(()),
                Some(keyboard_value::SWITCH_MATRIX_STATE) => {
                    Err(DeviceError::Denied("the switch matrix shows keys as they're pressed; it's never read".into()))
                }
                other => Err(DeviceError::Denied(format!("keyboard value {other:?} isn't allowed"))),
            };
        }
        if READ.contains(&command) || (self.writes && WRITE.contains(&command)) {
            return Ok(());
        }
        Err(DeviceError::Denied(if FORBIDDEN.contains(&command) {
            format!("VIA command {command:#04x} (reset, bootloader or factory) is never sent")
        } else if command == VIAL_PREFIX {
            "Vial commands (unlock, settings) aren't sent; Foxglove reads Vial boards through VIA".into()
        } else if WRITE.contains(&command) {
            format!("VIA command {command:#04x} writes; this keyboard is open read-only")
        } else {
            format!("VIA command {command:#04x} isn't allowed")
        }))
    }

    /// Sends `bytes` (zero-padded) if allowed and returns the reply, which must echo the
    /// command.
    pub fn send(&self, bytes: &[u8]) -> Result<Report> {
        let command = *bytes.first().ok_or(DeviceError::Unsupported)?;
        self.check(bytes)?;
        let mut report = [0u8; REPORT];
        report[..bytes.len().min(REPORT)].copy_from_slice(&bytes[..bytes.len().min(REPORT)]);
        let reply = self.inner.exchange(&report)?;
        match reply[0] {
            c if c == command => Ok(reply),
            UNHANDLED => Err(DeviceError::Unsupported),
            other => Err(DeviceError::Backend(format!("reply {other:#04x} to command {command:#04x}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Echo(Mutex<Vec<u8>>);
    impl Transport for Echo {
        fn exchange(&self, report: &Report) -> Result<Report> {
            self.0.lock().unwrap().push(report[0]);
            Ok(*report)
        }
    }

    #[test]
    fn forbidden_commands_never_reach_the_keyboard() {
        let g = Guarded::with_writes(Echo(Mutex::default()));
        for &c in FORBIDDEN {
            assert!(matches!(g.send(&[c]), Err(DeviceError::Denied(_))), "{c:#04x}");
        }
        for c in 0x0C..=0xFE {
            if !READ.contains(&c) && !WRITE.contains(&c) {
                assert!(g.send(&[c]).is_err(), "{c:#04x} isn't allowlisted");
            }
        }
        // Vial: unlock (and everything else) never goes out.
        for sub in 0..=0x20 {
            assert!(matches!(g.send(&[VIAL_PREFIX, sub]), Err(DeviceError::Denied(m)) if m.contains("Vial")));
        }
        assert!(g.inner.0.lock().unwrap().is_empty(), "nothing was sent");
    }

    #[test]
    fn keyboard_values_are_read_but_never_the_switch_matrix() {
        let g = Guarded::read_only(Echo(Mutex::default()));
        assert!(g.send(&[GET_KEYBOARD_VALUE, keyboard_value::UPTIME]).is_ok());
        assert!(g.send(&[GET_KEYBOARD_VALUE, keyboard_value::LAYOUT_OPTIONS]).is_ok());
        assert!(matches!(g.send(&[GET_KEYBOARD_VALUE, keyboard_value::SWITCH_MATRIX_STATE]), Err(DeviceError::Denied(_))));
        assert!(g.send(&[GET_KEYBOARD_VALUE, 0x42]).is_err());
        assert!(g.send(&[GET_KEYBOARD_VALUE]).is_err());
        assert!(g.send(&[SET_KEYBOARD_VALUE, keyboard_value::LAYOUT_OPTIONS]).is_err());
        assert_eq!(*g.inner.0.lock().unwrap(), vec![GET_KEYBOARD_VALUE, GET_KEYBOARD_VALUE]);
    }

    #[test]
    fn read_only_refuses_writes_but_reads() {
        let g = Guarded::read_only(Echo(Mutex::default()));
        assert!(g.send(&[GET_PROTOCOL_VERSION]).is_ok());
        assert!(matches!(g.send(&[DYNAMIC_KEYMAP_SET_KEYCODE]), Err(DeviceError::Denied(_))));
        assert!(matches!(g.send(&[CUSTOM_SAVE]), Err(DeviceError::Denied(_))));
        assert_eq!(*g.inner.0.lock().unwrap(), vec![GET_PROTOCOL_VERSION]);
    }
}
