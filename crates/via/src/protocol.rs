//! VIA's Raw HID commands (QMK `quantum/via.h`) and the allowlist in front of them.
//!
//! Every report starts with a command id; the keyboard answers with the same id, or 0xFF for a
//! command it doesn't handle.

use palm::{PalmError, Result};

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
pub const UNHANDLED: u8 = 0xFF;

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

    pub fn check(&self, command: u8) -> Result<()> {
        if READ.contains(&command) || (self.writes && WRITE.contains(&command)) {
            return Ok(());
        }
        Err(PalmError::Denied(if FORBIDDEN.contains(&command) {
            format!("VIA command {command:#04x} (reset, bootloader or factory) is never sent")
        } else if WRITE.contains(&command) {
            format!("VIA command {command:#04x} writes; this keyboard is open read-only")
        } else {
            format!("VIA command {command:#04x} isn't allowed")
        }))
    }

    /// Sends `bytes` (zero-padded) if allowed and returns the reply, which must echo the
    /// command.
    pub fn send(&self, bytes: &[u8]) -> Result<Report> {
        let command = *bytes.first().ok_or(PalmError::Unsupported)?;
        self.check(command)?;
        let mut report = [0u8; REPORT];
        report[..bytes.len().min(REPORT)].copy_from_slice(&bytes[..bytes.len().min(REPORT)]);
        let reply = self.inner.exchange(&report)?;
        match reply[0] {
            c if c == command => Ok(reply),
            UNHANDLED => Err(PalmError::Unsupported),
            other => Err(PalmError::Backend(format!("reply {other:#04x} to command {command:#04x}"))),
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
            assert!(matches!(g.send(&[c]), Err(PalmError::Denied(_))), "{c:#04x}");
        }
        for c in 0x0C..=0xFE {
            if !READ.contains(&c) && !WRITE.contains(&c) {
                assert!(g.send(&[c]).is_err(), "{c:#04x} isn't allowlisted");
            }
        }
        assert!(g.inner.0.lock().unwrap().is_empty(), "nothing was sent");
    }

    #[test]
    fn read_only_refuses_writes_but_reads() {
        let g = Guarded::read_only(Echo(Mutex::default()));
        assert!(g.send(&[GET_PROTOCOL_VERSION]).is_ok());
        assert!(matches!(g.send(&[DYNAMIC_KEYMAP_SET_KEYCODE]), Err(PalmError::Denied(_))));
        assert!(matches!(g.send(&[CUSTOM_SAVE]), Err(PalmError::Denied(_))));
        assert_eq!(*g.inner.0.lock().unwrap(), vec![GET_PROTOCOL_VERSION]);
    }
}
