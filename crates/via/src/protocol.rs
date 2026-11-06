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
pub const DYNAMIC_KEYMAP_MACRO_SET_BUFFER: u8 = 0x0F;
pub const DYNAMIC_KEYMAP_MACRO_RESET: u8 = 0x10;
pub const DYNAMIC_KEYMAP_GET_LAYER_COUNT: u8 = 0x11;
pub const DYNAMIC_KEYMAP_GET_BUFFER: u8 = 0x12;
pub const DYNAMIC_KEYMAP_SET_BUFFER: u8 = 0x13;
pub const DYNAMIC_KEYMAP_SET_ENCODER: u8 = 0x15;
/// Vial's commands start with this byte (unlock, QMK settings, dynamic entries).
pub const VIAL_PREFIX: u8 = 0xFE;
pub const UNHANDLED: u8 = 0xFF;

/// Whether this build carries VIA/Vial writes at all: the `via-writes` Cargo feature, off by
/// default (#581). A build without it never writes, whatever the environment says.
pub const WRITES_BUILT: bool = cfg!(feature = "via-writes");

/// The hidden, dev-only switch for the real-board write test (#723): in a `via-writes` build,
/// `FOXGLOVE_VIA_WRITES=1` turns writes on. Nothing in the UI or the settings shows or sets it.
pub const WRITES_ENV: &str = "FOXGLOVE_VIA_WRITES";

/// Writes are on only in a `via-writes` build with [`WRITES_ENV`] set to exactly `1`. Pure, for
/// the tests; [`writes_enabled`] reads the real environment.
pub fn writes_switch(built: bool, env: Option<&str>) -> bool {
    built && env == Some("1")
}

/// Whether Foxglove may write to VIA/Vial keyboards: [`writes_switch`] on this build and the
/// process's environment, decided once. Off (the default), [`Guarded::with_writes`] opens
/// read-only like [`Guarded::read_only`], and the app's VIA/Vial keymap adapters refuse every
/// write, so no write command goes out. On, it says so loudly on stderr, once.
pub fn writes_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let env = std::env::var(WRITES_ENV).ok();
        let on = writes_switch(WRITES_BUILT, env.as_deref());
        if on {
            eprintln!("{}", WRITES_ON_BANNER);
        } else if let Some(v) = env {
            eprintln!(
                "Foxglove: {WRITES_ENV}={v:?} is ignored: {}; VIA/Vial writes stay off",
                if WRITES_BUILT {
                    "only 1 turns them on"
                } else {
                    "this build has no via-writes feature"
                }
            );
        }
        on
    })
}

/// What the log says, once, when writes are on.
pub const WRITES_ON_BANNER: &str = "\
Foxglove: !!!!!!!! VIA/Vial WRITES ARE ON (FOXGLOVE_VIA_WRITES=1, via-writes build) !!!!!!!!\n\
Foxglove: !!!!!!!! dev test switch for #723: keycode, lighting and layout-option writes \
can reach VIA keyboards. Every write is logged. Not for daily use. !!!!!!!!";

/// Every VIA command that changes the keyboard (QMK `quantum/via.h`): values, keycodes, the
/// keymap buffer, macros, encoders, lighting (custom values), saves, resets and the bootloader.
pub const WRITE_OPCODES: &[u8] = &[
    SET_KEYBOARD_VALUE,
    DYNAMIC_KEYMAP_SET_KEYCODE,
    DYNAMIC_KEYMAP_RESET,
    CUSTOM_SET_VALUE,
    CUSTOM_SAVE,
    EEPROM_RESET,
    BOOTLOADER_JUMP,
    DYNAMIC_KEYMAP_MACRO_SET_BUFFER,
    DYNAMIC_KEYMAP_MACRO_RESET,
    DYNAMIC_KEYMAP_SET_BUFFER,
    DYNAMIC_KEYMAP_SET_ENCODER,
];

/// The Vial commands (after [`VIAL_PREFIX`]) that only read: keyboard id, definition size, a
/// definition block. Every other Vial command (unlock, encoders, QMK settings, dynamic entries)
/// counts as a write.
pub const VIAL_READS: &[u8] = &[0x00, 0x01, 0x02];

/// Whether a report would change the keyboard: a VIA write ([`WRITE_OPCODES`]), Keychron's
/// 0xAA-0xAB factory/DFU commands, or a Vial command that isn't a read. For tests and mocks
/// that must fail on any write.
pub fn is_write(report: &[u8]) -> bool {
    match report {
        [] => false,
        [VIAL_PREFIX, rest @ ..] => !rest.first().is_some_and(|c| VIAL_READS.contains(c)),
        [c, ..] => WRITE_OPCODES.contains(c) || matches!(*c, 0xAA | 0xAB),
    }
}

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

/// Commands that change the keyboard, allowed only with writes enabled. `SET_KEYBOARD_VALUE`
/// is allowed for the layout options only (see [`Guarded::check`]).
const WRITE: &[u8] = &[DYNAMIC_KEYMAP_SET_KEYCODE, CUSTOM_SET_VALUE, CUSTOM_SAVE];

/// QMK keycodes that are never written to a key, writes on or off: one press would jump to the
/// bootloader, reboot, or clear the EEPROM (the keymap and settings). Both encodings: QMK's
/// current `QK_BOOT` (0x7C00), `QK_REBOOT` (0x7C01) and `QK_CLEAR_EEPROM` (0x7C03), and the
/// pre-0.19 `RESET` (0x5C00), so an older firmware can't be handed a bootloader key either.
pub const FORBIDDEN_KEYCODES: &[u16] = &[0x7C00, 0x7C01, 0x7C03, 0x5C00];

/// Never sent, whatever the caller asks: resets, the bootloader, and Keychron's Bluetooth
/// DFU/factory-test namespace (0xAA-0xAB, which includes a bootloader jump). Everything not on
/// the read or write lists is refused too; this list is for the error message and the tests.
pub const FORBIDDEN: &[u8] = &[
    DYNAMIC_KEYMAP_RESET,
    EEPROM_RESET,
    BOOTLOADER_JUMP,
    0xAA,
    0xAB,
];

/// A connection that exchanges one report for its reply.
pub trait Transport: Send + Sync {
    /// Sends a report and returns the reply that echoes its command (or marks it unhandled).
    fn exchange(&self, report: &Report) -> Result<Report>;

    /// Sends a report and returns the next reply, whatever its first byte: Vial's reads answer
    /// with data from byte 0, so they never echo the command. Same timeout as [`exchange`].
    ///
    /// [`exchange`]: Transport::exchange
    fn exchange_any(&self, report: &Report) -> Result<Report> {
        self.exchange(report)
    }
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
        Guarded {
            inner,
            writes: false,
        }
    }

    /// Also allows keycode, lighting and layout-option writes; still never resets, the
    /// bootloader or Vial unlock. Only with [`writes_enabled`] (a `via-writes` build and the
    /// dev switch, #581/#723): otherwise this is read-only too.
    pub fn with_writes(inner: T) -> Self {
        Guarded {
            inner,
            writes: writes_enabled(),
        }
    }

    /// Writes on, whatever the build and environment: for the write-path tests against fakes.
    #[cfg(test)]
    pub(crate) fn writable(inner: T) -> Self {
        Guarded {
            inner,
            writes: true,
        }
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
                Some(keyboard_value::SWITCH_MATRIX_STATE) => Err(DeviceError::Denied(
                    "the switch matrix shows keys as they're pressed; it's never read".into(),
                )),
                other => Err(DeviceError::Denied(format!(
                    "keyboard value {other:?} isn't allowed"
                ))),
            };
        }
        if command == SET_KEYBOARD_VALUE {
            return match bytes.get(1).copied() {
                Some(keyboard_value::LAYOUT_OPTIONS) if self.writes && bytes.len() >= 6 => Ok(()),
                Some(keyboard_value::LAYOUT_OPTIONS) if self.writes => Err(DeviceError::Denied(
                    "a layout options write needs its four bytes".into(),
                )),
                Some(keyboard_value::LAYOUT_OPTIONS) => Err(DeviceError::Denied(
                    "the layout options are a write; this keyboard is open read-only".into(),
                )),
                other => Err(DeviceError::Denied(format!(
                    "setting keyboard value {other:?} isn't allowed"
                ))),
            };
        }
        if command == DYNAMIC_KEYMAP_SET_KEYCODE && self.writes {
            // Layer, row, column and the keycode: a short report would write zeros somewhere.
            let [_, _, _, _, hi, lo, ..] = *bytes else {
                return Err(DeviceError::Denied(
                    "a keycode write needs layer, row, column and keycode".into(),
                ));
            };
            let code = u16::from_be_bytes([hi, lo]);
            if FORBIDDEN_KEYCODES.contains(&code) {
                return Err(DeviceError::Denied(format!(
                    "keycode {code:#06x} (bootloader, reboot or EEPROM clear) is never written"
                )));
            }
        }
        if READ.contains(&command) || (self.writes && WRITE.contains(&command)) {
            return Ok(());
        }
        Err(DeviceError::Denied(if FORBIDDEN.contains(&command) {
            format!("VIA command {command:#04x} (reset, bootloader or factory) is never sent")
        } else if command == VIAL_PREFIX {
            "Vial commands (unlock, settings) aren't sent; only the definition reads go out, \
             through Guarded::vial_read"
                .into()
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
        if is_write(&report) {
            // Only with writes on (#723): every write is on the record, before it goes out.
            eprintln!("Foxglove: VIA WRITE {:02x?}", &report[..8]);
        }
        let reply = self.inner.exchange(&report)?;
        match reply[0] {
            c if c == command => Ok(reply),
            UNHANDLED => Err(DeviceError::Unsupported),
            other => Err(DeviceError::Backend(format!(
                "reply {other:#04x} to command {command:#04x}"
            ))),
        }
    }

    /// Sends one of Vial's reads ([`VIAL_PREFIX`], a command from [`VIAL_READS`], then `args`)
    /// and returns the raw reply, which carries its data from byte 0. Every other Vial command
    /// (unlock, settings, dynamic entries) is refused here, writes on or off; [`Guarded::send`]
    /// refuses all of them.
    pub fn vial_read(&self, command: u8, args: &[u8]) -> Result<Report> {
        if !VIAL_READS.contains(&command) {
            return Err(DeviceError::Denied(format!(
                "Vial command {command:#04x} isn't a read; it's never sent"
            )));
        }
        let mut report = [0u8; REPORT];
        report[0] = VIAL_PREFIX;
        report[1] = command;
        let n = args.len().min(REPORT - 2);
        report[2..2 + n].copy_from_slice(&args[..n]);
        debug_assert!(!is_write(&report));
        self.inner.exchange_any(&report)
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
            assert!(
                matches!(g.send(&[c]), Err(DeviceError::Denied(_))),
                "{c:#04x}"
            );
        }
        for c in 0x0C..=0xFE {
            if !READ.contains(&c) && !WRITE.contains(&c) {
                assert!(g.send(&[c]).is_err(), "{c:#04x} isn't allowlisted");
            }
        }
        // Vial: unlock (and everything else) never goes out.
        for sub in 0..=0x20 {
            assert!(
                matches!(g.send(&[VIAL_PREFIX, sub]), Err(DeviceError::Denied(m)) if m.contains("Vial"))
            );
        }
        assert!(g.inner.0.lock().unwrap().is_empty(), "nothing was sent");
    }

    /// Only Vial's id, size and definition reads go out, and only through `vial_read`; unlock
    /// and the rest never do, even with writes asked for.
    #[test]
    fn vial_reads_go_out_and_nothing_else_of_vial() {
        let g = Guarded::with_writes(Echo(Mutex::default()));
        for &sub in VIAL_READS {
            let r = g.vial_read(sub, &[1, 2, 3, 4]).unwrap();
            assert_eq!(r[..6], [VIAL_PREFIX, sub, 1, 2, 3, 4]);
        }
        for sub in 0..=0xFF {
            if !VIAL_READS.contains(&sub) {
                assert!(matches!(g.vial_read(sub, &[]), Err(DeviceError::Denied(_))));
            }
        }
        assert_eq!(*g.inner.0.lock().unwrap(), vec![VIAL_PREFIX; 3]);
    }

    #[test]
    fn keyboard_values_are_read_but_never_the_switch_matrix() {
        let g = Guarded::read_only(Echo(Mutex::default()));
        assert!(g
            .send(&[GET_KEYBOARD_VALUE, keyboard_value::UPTIME])
            .is_ok());
        assert!(g
            .send(&[GET_KEYBOARD_VALUE, keyboard_value::LAYOUT_OPTIONS])
            .is_ok());
        assert!(matches!(
            g.send(&[GET_KEYBOARD_VALUE, keyboard_value::SWITCH_MATRIX_STATE]),
            Err(DeviceError::Denied(_))
        ));
        assert!(g.send(&[GET_KEYBOARD_VALUE, 0x42]).is_err());
        assert!(g.send(&[GET_KEYBOARD_VALUE]).is_err());
        assert!(g
            .send(&[SET_KEYBOARD_VALUE, keyboard_value::LAYOUT_OPTIONS])
            .is_err());
        assert_eq!(
            *g.inner.0.lock().unwrap(),
            vec![GET_KEYBOARD_VALUE, GET_KEYBOARD_VALUE]
        );
    }

    /// #581: with `via-writes` off (0.2), asking for writes still opens read-only, and not one
    /// write report reaches the transport.
    #[cfg(not(feature = "via-writes"))]
    #[test]
    fn with_writes_stays_read_only_while_writes_are_off() {
        let g = Guarded::with_writes(Echo(Mutex::default()));
        assert!(!g.writes());
        for &c in WRITE_OPCODES {
            assert!(
                matches!(g.send(&[c, 0, 0, 0, 0, 0]), Err(DeviceError::Denied(_))),
                "{c:#04x}"
            );
        }
        for sub in 0..=0xFF {
            if is_write(&[VIAL_PREFIX, sub]) {
                assert!(g.send(&[VIAL_PREFIX, sub]).is_err());
            }
        }
        assert!(g.send(&[GET_PROTOCOL_VERSION]).is_ok(), "reads stay on");
        assert_eq!(*g.inner.0.lock().unwrap(), vec![GET_PROTOCOL_VERSION]);
    }

    #[test]
    fn write_opcodes_are_writes_and_reads_are_not() {
        for &c in WRITE_OPCODES.iter().chain(FORBIDDEN) {
            assert!(is_write(&[c]), "{c:#04x}");
        }
        for &c in READ {
            assert!(!is_write(&[c]), "{c:#04x}");
        }
        assert!(WRITE.iter().all(|c| WRITE_OPCODES.contains(c)));
        assert!(!is_write(&[VIAL_PREFIX, 0x02]) && is_write(&[VIAL_PREFIX, 0x06]));
        assert!(
            is_write(&[VIAL_PREFIX]),
            "a bare Vial prefix isn't a known read"
        );
    }

    /// #723: the dev switch is on only in a `via-writes` build with exactly `1`.
    #[test]
    fn the_dev_switch_needs_the_build_and_exactly_one() {
        assert!(writes_switch(true, Some("1")));
        for env in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("yes"),
            Some(" 1"),
        ] {
            assert!(!writes_switch(true, env), "{env:?}");
        }
        for env in [None, Some("1")] {
            assert!(!writes_switch(false, env), "never without the feature");
        }
        if !WRITES_BUILT {
            assert!(!writes_enabled());
        }
    }

    /// #723, writes on: keycodes, lighting, saves and the layout options go out; resets, the
    /// bootloader, factory commands, Vial (unlock included), other keyboard values, unknown
    /// commands and bootloader keycodes still don't.
    #[test]
    fn writes_on_allow_only_the_write_list() {
        let g = Guarded::writable(Echo(Mutex::default()));
        assert!(g.writes());
        let allowed: &[&[u8]] = &[
            &[DYNAMIC_KEYMAP_SET_KEYCODE, 0, 1, 2, 0x00, 0x04],
            &[CUSTOM_SET_VALUE, 3, 1, 128],
            &[CUSTOM_SAVE, 3],
            &[
                SET_KEYBOARD_VALUE,
                keyboard_value::LAYOUT_OPTIONS,
                0,
                0,
                0,
                1,
            ],
        ];
        for r in allowed {
            assert!(g.send(r).is_ok(), "{r:x?}");
        }
        let refused: &[&[u8]] = &[
            &[DYNAMIC_KEYMAP_RESET],
            &[EEPROM_RESET],
            &[BOOTLOADER_JUMP],
            &[0xAA],
            &[0xAB],
            &[DYNAMIC_KEYMAP_SET_BUFFER, 0, 0, 2, 0, 4],
            &[DYNAMIC_KEYMAP_MACRO_SET_BUFFER, 0, 0, 1, 0],
            &[DYNAMIC_KEYMAP_MACRO_RESET],
            &[DYNAMIC_KEYMAP_SET_ENCODER, 0, 0, 0, 0, 4],
            &[SET_KEYBOARD_VALUE, keyboard_value::UPTIME, 0, 0, 0, 0],
            &[SET_KEYBOARD_VALUE, keyboard_value::SWITCH_MATRIX_STATE],
            &[SET_KEYBOARD_VALUE, 0x42],
            &[SET_KEYBOARD_VALUE],
            &[SET_KEYBOARD_VALUE, keyboard_value::LAYOUT_OPTIONS, 0],
            &[DYNAMIC_KEYMAP_SET_KEYCODE, 0, 1, 2],
            &[DYNAMIC_KEYMAP_SET_KEYCODE, 0, 1, 2, 0x7C, 0x00],
            &[DYNAMIC_KEYMAP_SET_KEYCODE, 0, 1, 2, 0x7C, 0x01],
            &[DYNAMIC_KEYMAP_SET_KEYCODE, 0, 1, 2, 0x7C, 0x03],
            &[DYNAMIC_KEYMAP_SET_KEYCODE, 0, 1, 2, 0x5C, 0x00],
            &[0x0C],
            &[0x14],
            &[0x77],
        ];
        for r in refused {
            assert!(
                matches!(g.send(r), Err(DeviceError::Denied(_))),
                "{r:x?} went out"
            );
        }
        // Vial: unlock start (0x06), poll (0x07), lock (0x08) and the rest never go out.
        for sub in 0..=0xFF {
            assert!(g.send(&[VIAL_PREFIX, sub]).is_err());
            if !VIAL_READS.contains(&sub) {
                assert!(g.vial_read(sub, &[]).is_err(), "{sub:#04x}");
            }
        }
        assert_eq!(
            *g.inner.0.lock().unwrap(),
            vec![
                DYNAMIC_KEYMAP_SET_KEYCODE,
                CUSTOM_SET_VALUE,
                CUSTOM_SAVE,
                SET_KEYBOARD_VALUE
            ]
        );
    }

    #[test]
    fn read_only_refuses_layout_option_writes() {
        let g = Guarded::read_only(Echo(Mutex::default()));
        assert!(matches!(
            g.send(&[
                SET_KEYBOARD_VALUE,
                keyboard_value::LAYOUT_OPTIONS,
                0,
                0,
                0,
                1
            ]),
            Err(DeviceError::Denied(_))
        ));
        assert!(g.inner.0.lock().unwrap().is_empty());
    }

    #[test]
    fn read_only_refuses_writes_but_reads() {
        let g = Guarded::read_only(Echo(Mutex::default()));
        assert!(g.send(&[GET_PROTOCOL_VERSION]).is_ok());
        assert!(matches!(
            g.send(&[DYNAMIC_KEYMAP_SET_KEYCODE]),
            Err(DeviceError::Denied(_))
        ));
        assert!(matches!(
            g.send(&[CUSTOM_SAVE]),
            Err(DeviceError::Denied(_))
        ));
        assert_eq!(*g.inner.0.lock().unwrap(), vec![GET_PROTOCOL_VERSION]);
    }
}
