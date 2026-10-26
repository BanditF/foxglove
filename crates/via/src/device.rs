//! A VIA keyboard as a device-layer device: its keymap (read; written only when opened with writes)
//! and, when the definition has it, RGB Matrix lighting through VIA's custom values.
//!
//! Lighting changes are not saved to the keyboard's EEPROM (no CUSTOM_SAVE): they last until
//! the keyboard restarts, so experiments can't wear or overwrite the stored settings.

use foxglove_devices::calibration;
use foxglove_devices::{
    Binding, Caps, Device, DeviceError, DeviceInfo, Effect, Keymap, LayerInfo, Layout, Led,
    Lighting, LightingState, Result, WriteSupport, Zone,
};

use crate::definition::Definition;
use crate::protocol::{self, rgb_matrix, Guarded, Transport};

pub struct ViaDevice<T: Transport> {
    info: DeviceInfo,
    def: Definition,
    link: Guarded<T>,
    /// VIA protocol version, from the keyboard.
    pub protocol_version: u16,
    layers: u8,
}

impl<T: Transport> ViaDevice<T> {
    /// Asks the keyboard for its protocol version and layer count; fails on keyboards that
    /// don't answer VIA.
    pub fn open(info: DeviceInfo, def: Definition, link: Guarded<T>) -> Result<Self> {
        let v = link.send(&[protocol::GET_PROTOCOL_VERSION])?;
        let protocol_version = u16::from_be_bytes([v[1], v[2]]);
        let layers = link.send(&[protocol::DYNAMIC_KEYMAP_GET_LAYER_COUNT])?[1];
        Ok(ViaDevice {
            info,
            def,
            link,
            protocol_version,
            layers,
        })
    }

    fn custom_get(&self, id: u8) -> Result<[u8; 2]> {
        let r = self
            .link
            .send(&[protocol::CUSTOM_GET_VALUE, rgb_matrix::CHANNEL, id])?;
        Ok([r[3], r[4]])
    }

    fn custom_set(&self, id: u8, data: &[u8]) -> Result<()> {
        let mut report = vec![protocol::CUSTOM_SET_VALUE, rgb_matrix::CHANNEL, id];
        report.extend_from_slice(data);
        self.link.send(&report).map(|_| ())
    }
}

/// 0-255 (QMK) to 0-100 (the device layer) and back.
fn to_percent(v: u8) -> u8 {
    ((u16::from(v) * 100 + 127) / 255) as u8
}

fn from_percent(p: u8) -> u8 {
    ((u16::from(p.min(100)) * 255 + 50) / 100) as u8
}

impl<T: Transport> Device for ViaDevice<T> {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn caps(&self) -> Caps {
        let mut caps = Caps::KEYMAP;
        if self.link.writes() {
            caps |= Caps::KEYMAP_WRITE;
        }
        if self.def.rgb_matrix {
            caps |= Caps::LIGHTING;
        }
        caps
    }

    fn keymap(&self) -> Option<&dyn Keymap> {
        Some(self)
    }

    fn lighting(&self) -> Option<&dyn Lighting> {
        self.def.rgb_matrix.then_some(self as &dyn Lighting)
    }
}

impl<T: Transport> Keymap for ViaDevice<T> {
    fn layout(&self) -> Result<Layout> {
        Ok(self.def.layout.clone())
    }

    fn layers(&self) -> Result<Vec<LayerInfo>> {
        Ok((0..u32::from(self.layers))
            .map(|id| LayerInfo { id, name: None })
            .collect())
    }

    /// The QMK keycode, as `kind = "qmk"` with the 16-bit code as the only parameter.
    fn binding(&self, layer: u32, row: u16, col: u16) -> Result<Binding> {
        let (layer, row, col) = (byte(layer)?, byte(row.into())?, byte(col.into())?);
        let r = self
            .link
            .send(&[protocol::DYNAMIC_KEYMAP_GET_KEYCODE, layer, row, col])?;
        Ok(Binding {
            kind: "qmk".into(),
            params: vec![u32::from(u16::from_be_bytes([r[4], r[5]]))],
            label: None,
        })
    }

    fn write_support(&self) -> WriteSupport {
        if self.link.writes() {
            WriteSupport::Immediate
        } else {
            WriteSupport::None
        }
    }

    fn set_binding(&self, layer: u32, row: u16, col: u16, binding: Binding) -> Result<()> {
        if !self.link.writes() {
            return Err(DeviceError::Unsupported);
        }
        let code = match (binding.kind.as_str(), binding.params.as_slice()) {
            ("qmk", [code]) => u16::try_from(*code).map_err(|_| DeviceError::Unsupported)?,
            _ => return Err(DeviceError::Unsupported),
        };
        let [hi, lo] = code.to_be_bytes();
        let (layer, row, col) = (byte(layer)?, byte(row.into())?, byte(col.into())?);
        self.link
            .send(&[
                protocol::DYNAMIC_KEYMAP_SET_KEYCODE,
                layer,
                row,
                col,
                hi,
                lo,
            ])
            .map(|_| ())
    }
}

fn byte(v: u32) -> Result<u8> {
    u8::try_from(v).map_err(|_| DeviceError::Unsupported)
}

impl<T: Transport> Lighting for ViaDevice<T> {
    fn layout(&self) -> Result<Vec<Zone>> {
        Ok(vec![Zone {
            name: "keys".into(),
            leds: vec![Led {
                id: 0,
                position: None,
            }],
        }])
    }

    fn state(&self) -> Result<LightingState> {
        let brightness = self.custom_get(rgb_matrix::BRIGHTNESS)?[0];
        let effect = self.custom_get(rgb_matrix::EFFECT)?[0];
        let speed = self.custom_get(rgb_matrix::EFFECT_SPEED)?[0];
        let [hue, sat] = self.custom_get(rgb_matrix::COLOR)?;
        // In Foxglove's palette: the colour asked for, not its calibrated form.
        let key = calibration::key_for(&self.info.id);
        let (hue, sat, shown) = calibration::asked_hsb(
            &key,
            (
                ((u32::from(hue) * 360 + 128) / 256) as u16 % 360,
                to_percent(sat),
                to_percent(brightness),
            ),
        );
        Ok(LightingState {
            on: brightness > 0 && effect > 0,
            hue,
            sat,
            brightness: shown,
            effect: Effect {
                id: effect.to_string(),
                params: vec![],
            },
            speed,
            settled: true,
        })
    }

    /// RGB Matrix effects by QMK number (0 = off).
    fn set_effect(&self, effect: Effect) -> Result<()> {
        let n: u8 = effect.id.parse().map_err(|_| DeviceError::Unsupported)?;
        self.custom_set(rgb_matrix::EFFECT, &[n])
    }

    /// Through the keyboard's colour calibration.
    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> Result<()> {
        let key = calibration::key_for(&self.info.id);
        calibration::send_hsb(&key, (hue, sat, brightness), |(hue, sat, brightness)| {
            let h = ((u32::from(hue) * 256) / 360) as u8;
            self.custom_set(rgb_matrix::COLOR, &[h, from_percent(sat)])?;
            self.custom_set(rgb_matrix::BRIGHTNESS, &[from_percent(brightness)])
        })
    }

    fn release(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::SAMPLE;
    use crate::protocol::{Report, REPORT};
    use foxglove_devices::{BackendId, DeviceId, DeviceKind};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A VIA keyboard in memory.
    #[derive(Default)]
    struct FakeVia {
        keymap: Mutex<HashMap<(u8, u8, u8), u16>>,
        custom: Mutex<HashMap<u8, [u8; 2]>>,
        sent: Mutex<Vec<u8>>,
    }

    impl Transport for &'static FakeVia {
        fn exchange(&self, r: &Report) -> Result<Report> {
            self.sent.lock().unwrap().push(r[0]);
            let mut out = [0u8; REPORT];
            out[..4].copy_from_slice(&r[..4]);
            match r[0] {
                protocol::GET_PROTOCOL_VERSION => out[1..3].copy_from_slice(&12u16.to_be_bytes()),
                protocol::DYNAMIC_KEYMAP_GET_LAYER_COUNT => out[1] = 4,
                protocol::DYNAMIC_KEYMAP_GET_KEYCODE => {
                    let kc = self
                        .keymap
                        .lock()
                        .unwrap()
                        .get(&(r[1], r[2], r[3]))
                        .copied()
                        .unwrap_or(0);
                    out[4..6].copy_from_slice(&kc.to_be_bytes());
                }
                protocol::DYNAMIC_KEYMAP_SET_KEYCODE => {
                    self.keymap
                        .lock()
                        .unwrap()
                        .insert((r[1], r[2], r[3]), u16::from_be_bytes([r[4], r[5]]));
                }
                protocol::CUSTOM_GET_VALUE => {
                    let v = self
                        .custom
                        .lock()
                        .unwrap()
                        .get(&r[2])
                        .copied()
                        .unwrap_or([0, 0]);
                    out[3..5].copy_from_slice(&v);
                }
                protocol::CUSTOM_SET_VALUE => {
                    self.custom.lock().unwrap().insert(r[2], [r[3], r[4]]);
                }
                _ => out[0] = protocol::UNHANDLED,
            }
            Ok(out)
        }
    }

    fn open(writes: bool) -> (ViaDevice<&'static FakeVia>, &'static FakeVia) {
        let fake: &'static FakeVia = Box::leak(Box::default());
        fake.keymap.lock().unwrap().insert((0, 0, 0), 0x0029); // KC_ESC
        let def = Definition::parse(SAMPLE).unwrap();
        let id = DeviceId::new(
            BackendId("via".into()),
            def.vendor_id,
            def.product_id,
            None,
            "p",
        );
        let info = DeviceInfo {
            id: id.clone(),
            backend: id.backend.clone(),
            name: def.name.clone(),
            kind: DeviceKind::Keyboard,
            firmware: None,
        };
        let link = if writes {
            Guarded::with_writes(fake)
        } else {
            Guarded::read_only(fake)
        };
        (ViaDevice::open(info, def, link).unwrap(), fake)
    }

    #[test]
    fn read_only_device_passes_the_contract_and_never_writes() {
        let (dev, fake) = open(false);
        assert_eq!((dev.protocol_version, dev.layers), (12, 4));
        foxglove_devices::contract::check_device(&dev).unwrap();
        assert_eq!(dev.binding(0, 0, 0).unwrap().params, vec![0x29]);
        assert!(dev
            .lighting()
            .unwrap()
            .set_effect(Effect {
                id: "3".into(),
                params: vec![]
            })
            .is_err());
        let sent = fake.sent.lock().unwrap();
        assert!(!sent.iter().any(|c| [
            protocol::DYNAMIC_KEYMAP_SET_KEYCODE,
            protocol::CUSTOM_SET_VALUE,
            protocol::CUSTOM_SAVE
        ]
        .contains(c)));
    }

    #[test]
    fn writable_device_writes_keycodes_and_lighting_without_saving() {
        let (dev, fake) = open(true);
        foxglove_devices::contract::check_binding(
            &dev,
            1,
            1,
            2,
            Binding {
                kind: "qmk".into(),
                params: vec![0x0004],
                label: None,
            },
        )
        .unwrap();
        let lighting = dev.lighting().unwrap();
        lighting.set_color(180, 50, 100).unwrap();
        lighting
            .set_effect(Effect {
                id: "4".into(),
                params: vec![],
            })
            .unwrap();
        let s = lighting.state().unwrap();
        assert_eq!(
            (s.hue, s.sat, s.brightness, s.effect.id.as_str(), s.on),
            (180, 50, 100, "4", true)
        );
        assert!(
            !fake.sent.lock().unwrap().contains(&protocol::CUSTOM_SAVE),
            "nothing persisted"
        );
    }
}
