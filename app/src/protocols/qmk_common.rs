//! Shared transport, feature probing, and key writing utilities for QMK/VIA/VIAL protocols.

use crate::key_action::{KeyAction, KeymapSnapshot};
use qmk_via_api::api::KeyboardApi;
pub use qmk_via_api::QmkFeatures;
use std::error::Error;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Returns an action filter that disables keycodes not supported by the keyboard's features.
pub fn qmk_action_filter(features: QmkFeatures) -> Option<super::ActionFilter> {
    Some(Arc::new(move |action| match action {
        KeyAction::Qmk(code) => features.is_keycode_supported(*code),
        _ => true,
    }))
}

/// Reads a complete keymap snapshot across all dynamic layers from a QMK/VIA/VIAL keyboard.
pub fn qmk_read_snapshot(
    api: &KeyboardApi,
    definition: &super::KeyboardDefinition,
) -> Result<KeymapSnapshot, Box<dyn Error>> {
    let layer_count = api
        .get_layer_count()
        .map_err(|e| format!("Failed to get layer count: {e}"))? as usize;
    let (rows, cols) = (definition.rows, definition.cols);
    let matrix_info = qmk_via_api::api::MatrixInfo {
        rows: rows as u8,
        cols: cols as u8,
    };

    let mut actions = vec![vec![vec![None; cols]; rows]; layer_count];
    for (layer, layer_actions) in actions.iter_mut().enumerate() {
        let raw_matrix = api
            .read_raw_matrix(matrix_info, layer as u8)
            .map_err(|e| format!("Failed to read layer {layer} keymap: {e}"))?;
        for (i, &keycode) in raw_matrix.iter().enumerate() {
            let row = i / cols;
            let col = i % cols;
            if row < rows && col < cols {
                layer_actions[row][col] = Some(KeyAction::Qmk(keycode));
            }
        }
    }

    Ok(KeymapSnapshot {
        layers: crate::key_action::LayerInfo::indexed(layer_count),
        actions,
    })
}

/// Foxglove (#581, #723): whether VIA/Vial keymap writes are on. One switch for the app and the
/// VIA backend: a `via-writes` build with the hidden dev switch `FOXGLOVE_VIA_WRITES=1`, for the
/// real-board test only. Off by default ("Foxglove never writes to these keyboards").
pub fn writes_enabled() -> bool {
    foxglove_via::writes_enabled()
}

/// Foxglove (#581): what a write answers while writes are off.
pub const WRITES_OFF: &str = "Foxglove only reads VIA/Vial keyboards; it doesn't write to them";

/// Foxglove (#581): the VIA/Vial adapters' write support. `None` while writes are off, so the
/// keymap editor offers no write controls for these keyboards.
pub fn qmk_write_support() -> super::WriteSupport {
    write_support_if(writes_enabled())
}

fn write_support_if(on: bool) -> super::WriteSupport {
    if on {
        super::WriteSupport::Immediate
    } else {
        super::WriteSupport::None
    }
}

/// Foxglove (#581): the two keymap calls a key write makes, so a test can stand in for the
/// keyboard. `set_key` sends VIA's `DYNAMIC_KEYMAP_SET_KEYCODE` (0x05), `get_key` its
/// `DYNAMIC_KEYMAP_GET_KEYCODE` (0x04).
pub trait QmkKeyLink {
    fn set_key(&self, layer: u8, row: u8, col: u8, code: u16) -> qmk_via_api::Result<u16>;
    fn get_key(&self, layer: u8, row: u8, col: u8) -> qmk_via_api::Result<u16>;
}

impl QmkKeyLink for KeyboardApi {
    fn set_key(&self, layer: u8, row: u8, col: u8, code: u16) -> qmk_via_api::Result<u16> {
        KeyboardApi::set_key(self, layer, row, col, code)
    }
    fn get_key(&self, layer: u8, row: u8, col: u8) -> qmk_via_api::Result<u16> {
        KeyboardApi::get_key(self, layer, row, col)
    }
}

/// Writes a key binding to a QMK keyboard. Refused before anything is sent while VIA/Vial
/// writes are off (#581), outside the definition's matrix, or for a bootloader, reboot or
/// EEPROM-clear keycode (#723).
pub fn qmk_set_key(
    api: &impl QmkKeyLink,
    definition: &super::KeyboardDefinition,
    layer_index: usize,
    row: usize,
    col: usize,
    action: &KeyAction,
) -> Result<(), Box<dyn Error>> {
    set_key_if(
        writes_enabled(),
        api,
        (definition.rows, definition.cols),
        layer_index,
        row,
        col,
        action,
    )
}

/// [`qmk_set_key`] with the switch given, for the tests.
fn set_key_if(
    on: bool,
    api: &impl QmkKeyLink,
    (rows, cols): (usize, usize),
    layer_index: usize,
    row: usize,
    col: usize,
    action: &KeyAction,
) -> Result<(), Box<dyn Error>> {
    if !on {
        return Err(WRITES_OFF.into());
    }
    if layer_index > usize::from(u8::MAX) || row >= rows || col >= cols || rows > 256 || cols > 256
    {
        return Err(format!(
            "layer {layer_index}, row {row}, column {col} is outside this keyboard's {rows}x{cols} \
             matrix"
        )
        .into());
    }
    match action {
        KeyAction::Qmk(code) if foxglove_via::protocol::FORBIDDEN_KEYCODES.contains(code) => {
            Err(format!(
                "Foxglove doesn't put {code:#06x} (bootloader, reboot or EEPROM clear) on a key"
            )
            .into())
        }
        KeyAction::Qmk(code) => qmk_set_key_with_retry(api, layer_index, row, col, *code),
        KeyAction::Zmk(_) => Err("Cannot apply a ZMK behavior to a QMK keyboard".into()),
    }
}

/// Writes a keycode via the VIA protocol with readback verification on error.
pub(crate) fn qmk_set_key_with_retry(
    api: &impl QmkKeyLink,
    layer_index: usize,
    row: usize,
    col: usize,
    code: u16,
) -> Result<(), Box<dyn Error>> {
    match api.set_key(layer_index as u8, row as u8, col as u8, code) {
        Ok(_) => Ok(()),
        Err(qmk_via_api::Error::BadCommandResponse(_)) => {
            for _ in 0..3 {
                thread::sleep(Duration::from_millis(50));
                if let Ok(readback) = api.get_key(layer_index as u8, row as u8, col as u8) {
                    if readback == code {
                        return Ok(());
                    }
                }
            }
            Err("Failed to set key: the device did not confirm the write".into())
        }
        Err(e) => Err(format!("Failed to set key: {e}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qmk_via_api::keycodes::Keycode;

    #[test]
    fn test_qmk_action_filter() {
        let features = QmkFeatures {
            has_backlight: false,
            has_rgblight: true,
            has_rgb_matrix: false,
            has_audio: false,
        };

        let filter = qmk_action_filter(features).expect("filter should be Some");
        assert!(filter(&KeyAction::Qmk(Keycode::KC_A as u16)));
        assert!(filter(&KeyAction::Qmk(Keycode::QK_UNDERGLOW_TOGGLE as u16)));
        assert!(!filter(&KeyAction::Qmk(
            Keycode::QK_BACKLIGHT_TOGGLE as u16
        )));
        assert!(!filter(&KeyAction::Qmk(
            Keycode::QK_RGB_MATRIX_TOGGLE as u16
        )));
        assert!(!filter(&KeyAction::Qmk(Keycode::QK_AUDIO_TOGGLE as u16)));
    }

    /// A VIA keyboard in memory that records each command's opcode and fails the test if a
    /// write opcode goes out while VIA/Vial writes are off (#581).
    #[derive(Default)]
    struct MockVia {
        sent: std::sync::Mutex<Vec<u8>>,
        takes_writes: bool,
    }

    const MATRIX: (usize, usize) = (4, 12);

    impl MockVia {
        fn send(&self, opcode: u8) {
            assert!(
                self.takes_writes || !foxglove_via::protocol::is_write(&[opcode]),
                "write {opcode:#04x} went out with VIA/Vial writes off"
            );
            self.sent.lock().unwrap().push(opcode);
        }
    }

    impl QmkKeyLink for MockVia {
        fn set_key(&self, _: u8, _: u8, _: u8, code: u16) -> qmk_via_api::Result<u16> {
            self.send(foxglove_via::protocol::DYNAMIC_KEYMAP_SET_KEYCODE);
            Ok(code)
        }
        fn get_key(&self, _: u8, _: u8, _: u8) -> qmk_via_api::Result<u16> {
            self.send(foxglove_via::protocol::DYNAMIC_KEYMAP_GET_KEYCODE);
            Ok(0)
        }
    }

    /// #581/#723: with writes off (the default), the VIA/Vial adapters offer no writes, and a
    /// key write is refused before any command reaches the keyboard.
    #[test]
    fn writes_off_no_key_write_reaches_a_via_keyboard() {
        if !foxglove_via::WRITES_BUILT {
            assert!(
                !writes_enabled(),
                "no switch turns writes on without the feature"
            );
            assert_eq!(qmk_write_support(), super::super::WriteSupport::None);
        }
        assert_eq!(write_support_if(false), super::super::WriteSupport::None);
        let kb = MockVia::default();
        for action in [
            KeyAction::Qmk(Keycode::KC_A as u16),
            KeyAction::Qmk(Keycode::KC_NO as u16),
        ] {
            let err = set_key_if(false, &kb, MATRIX, 0, 1, 2, &action).unwrap_err();
            assert_eq!(err.to_string(), WRITES_OFF);
        }
        assert!(kb.sent.lock().unwrap().is_empty(), "nothing was sent");
    }

    #[test]
    fn writes_on_a_key_write_goes_out() {
        assert_eq!(
            write_support_if(true),
            super::super::WriteSupport::Immediate
        );
        let kb = MockVia {
            takes_writes: true,
            ..MockVia::default()
        };
        set_key_if(
            true,
            &kb,
            MATRIX,
            0,
            1,
            2,
            &KeyAction::Qmk(Keycode::KC_A as u16),
        )
        .unwrap();
        assert_eq!(
            *kb.sent.lock().unwrap(),
            [foxglove_via::protocol::DYNAMIC_KEYMAP_SET_KEYCODE]
        );
    }

    /// #723, writes on: a key outside the matrix, past 255 layers, or a bootloader, reboot or
    /// EEPROM-clear keycode is refused before anything goes out.
    #[test]
    fn writes_on_bounds_and_forbidden_keycodes_are_refused() {
        let kb = MockVia {
            takes_writes: true,
            ..MockVia::default()
        };
        let a = KeyAction::Qmk(Keycode::KC_A as u16);
        for (layer, row, col) in [(0, 4, 0), (0, 0, 12), (256, 0, 0)] {
            assert!(set_key_if(true, &kb, MATRIX, layer, row, col, &a).is_err());
        }
        assert!(set_key_if(true, &kb, (300, 1), 0, 0, 0, &a).is_err());
        for code in [
            Keycode::QK_BOOTLOADER as u16,
            Keycode::QK_REBOOT as u16,
            Keycode::QK_CLEAR_EEPROM as u16,
        ] {
            assert!(foxglove_via::protocol::FORBIDDEN_KEYCODES.contains(&code));
            assert!(set_key_if(true, &kb, MATRIX, 0, 0, 0, &KeyAction::Qmk(code)).is_err());
        }
        assert!(kb.sent.lock().unwrap().is_empty(), "nothing was sent");
    }
}
