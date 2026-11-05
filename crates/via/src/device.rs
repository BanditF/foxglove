//! A VIA keyboard as a device-layer device. What it offers comes from what it answers when
//! opened ([`Probe`]) and, when one is loaded, its VIA definition:
//!
//! - keymap (read; written only when opened with writes): needs the definition's matrix and
//!   layout. A board found without a loaded one is asked for its own: Vial boards keep it
//!   compressed in the firmware ([`crate::vial`]); other boards have no keymap yet. The layout
//!   is the one for the layout options the keyboard has chosen (split backspace, ISO enter...),
//!   or the default when it doesn't say;
//! - lighting: the best lighting channel the keyboard answers (RGB Matrix, RGB Light, LED Matrix
//!   or backlight), through VIA's lighting values in the scheme its protocol version uses.
//!
//! Lighting changes are not saved to the keyboard's EEPROM (no CUSTOM_SAVE): they last until
//! the keyboard restarts, so experiments can't wear or overwrite the stored settings.

use foxglove_devices::calibration;
use foxglove_devices::{
    Binding, Caps, Device, DeviceError, DeviceInfo, Effect, Keymap, LayerInfo, Layout, Led,
    Lighting, LightingState, Result, WriteSupport, Zone,
};

use crate::catalog::Catalog;
use crate::client::{Channel, Client, Probe, Scheme};
use crate::definition::Definition;
use crate::protocol::{self, Guarded, Transport};
use crate::vial;

/// Where a device's definition came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A definition file loaded for it.
    Loaded,
    /// The keyboard's own, read from its Vial firmware.
    Vial { protocol: u32 },
    /// None: not a Vial board (`why` is `None`), or a Vial board whose definition couldn't be
    /// read (`why` says what went wrong). It has no keymap.
    Missing { why: Option<String> },
}

/// What an open board turned out to be, for the app to show next to it (#74). The app only
/// holds boards as `dyn Device`, so the backend keeps these (see [`crate::hid::Opened`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// `VIA protocol 12`, or `VIA protocol 9, Vial protocol 6`.
    pub firmware: Option<String>,
    /// The lighting channel shown, if any answered.
    pub lighting: Option<Channel>,
    pub source: Source,
    /// Layers, when it has a keymap.
    pub layers: Option<u8>,
}

pub struct ViaDevice<T: Transport> {
    info: DeviceInfo,
    def: Option<Definition>,
    /// Where `def` came from.
    pub source: Source,
    /// The definition's layout for the keyboard's layout options.
    layout: Option<Layout>,
    link: Guarded<T>,
    /// What the keyboard answered when opened.
    pub probe: Probe,
    /// The lighting channel shown, and how it's addressed.
    light: Option<(Scheme, Channel)>,
}

impl<T: Transport> ViaDevice<T> {
    /// Asks the keyboard for its protocol version, layers, layout options and lighting; fails
    /// on keyboards that don't answer VIA. The catalog decides how lighting is addressed.
    /// Without `def`, a Vial board's own definition is read (bounded, see [`crate::vial`]).
    pub fn open(
        mut info: DeviceInfo,
        def: Option<Definition>,
        link: Guarded<T>,
        catalog: &Catalog,
    ) -> Result<Self> {
        let (vid, pid) = (info.id.vid, info.id.pid);
        let probe = Client(&link).probe(|v| catalog.scheme(vid, pid, v))?;
        info.firmware = Some(format!("VIA protocol {}", probe.protocol_version));
        let (def, source) = match def {
            Some(d) => (Some(d), Source::Loaded),
            None => board_definition(&link, &info)?,
        };
        if let Source::Vial { protocol } = source {
            info.firmware = Some(format!(
                "VIA protocol {}, Vial protocol {protocol}",
                probe.protocol_version
            ));
        }
        let listed = def.as_ref().map(|d| d.lighting.as_slice()).unwrap_or(&[]);
        let light = probe.scheme.and_then(|scheme| {
            let answered = |c: &&Channel| probe.lighting.iter().any(|s| s.channel == **c);
            listed
                .iter()
                .find(answered)
                .or_else(|| probe.lighting.first().map(|s| &s.channel))
                .map(|&c| (scheme, c))
        });
        let layout = def
            .as_ref()
            .map(|d| d.layout_for(probe.layout_options.unwrap_or(0)));
        Ok(ViaDevice {
            info,
            def,
            source,
            layout,
            link,
            probe,
            light,
        })
    }

    /// The lighting channel Foxglove shows, if any answered.
    pub fn lighting_channel(&self) -> Option<Channel> {
        self.light.map(|(_, c)| c)
    }

    /// What it turned out to be: firmware, lighting channel, definition source, layers.
    pub fn summary(&self) -> Summary {
        Summary {
            firmware: self.info.firmware.clone(),
            lighting: self.lighting_channel(),
            source: self.source.clone(),
            layers: self.def.is_some().then_some(self.probe.layers),
        }
    }

    /// The whole keymap in one go (layer, row, column), through the keymap buffer; needs the
    /// definition's matrix.
    pub fn read_keymap(&self) -> Result<Vec<u16>> {
        let def = self.def.as_ref().ok_or(DeviceError::Unsupported)?;
        Client(&self.link).keymap(self.probe.layers, def.rows, def.cols)
    }

    fn set_value(&self, value: u8, data: &[u8]) -> Result<()> {
        let (scheme, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        let (mut report, _) = channel.request(scheme, protocol::CUSTOM_SET_VALUE, value)?;
        report.extend_from_slice(data);
        self.link.send(&report).map(|_| ())
    }
}

/// The keyboard's own definition, if it's a Vial board. Only a failing link (a lost connection,
/// a failed write) fails the open; a definition that can't be read leaves the board without a
/// keymap, and says why.
fn board_definition<T: Transport>(
    link: &Guarded<T>,
    info: &DeviceInfo,
) -> Result<(Option<Definition>, Source)> {
    let missing = |why: Option<String>| Ok((None, Source::Missing { why }));
    let id = match vial::keyboard_id(link) {
        Ok(id) => id,
        // Unhandled or unanswered: not a Vial board.
        Err(DeviceError::Unsupported | DeviceError::Timeout) => return missing(None),
        Err(e) => return Err(e),
    };
    let json = match vial::compressed(link, vial::BUDGET).and_then(|b| vial::decompress(&b)) {
        Ok(json) => json,
        Err(DeviceError::Disconnected) => return Err(DeviceError::Disconnected),
        Err(e) => return missing(Some(e.to_string())),
    };
    match Definition::from_board(&json, info.id.vid, info.id.pid, &info.name) {
        Ok(d) => Ok((
            Some(d),
            Source::Vial {
                protocol: id.protocol,
            },
        )),
        Err(e) => missing(Some(format!("the Vial definition doesn't parse: {e}"))),
    }
}

/// Value ids within a lighting channel.
const BRIGHTNESS: u8 = 1;
const EFFECT: u8 = 2;
const COLOR: u8 = 4;

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
        let mut caps = Caps::default();
        if self.def.is_some() {
            caps |= Caps::KEYMAP;
            if self.link.writes() {
                caps |= Caps::KEYMAP_WRITE;
            }
        }
        if self.light.is_some() {
            caps |= Caps::LIGHTING;
        }
        caps
    }

    fn keymap(&self) -> Option<&dyn Keymap> {
        self.def.is_some().then_some(self as &dyn Keymap)
    }

    fn lighting(&self) -> Option<&dyn Lighting> {
        self.light.is_some().then_some(self as &dyn Lighting)
    }
}

impl<T: Transport> Keymap for ViaDevice<T> {
    fn layout(&self) -> Result<Layout> {
        self.layout.clone().ok_or(DeviceError::Unsupported)
    }

    fn layers(&self) -> Result<Vec<LayerInfo>> {
        Ok((0..u32::from(self.probe.layers))
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
        let (scheme, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        let s = Client(&self.link).lighting(scheme, channel)?;
        let (hue, sat) = s.color.unwrap_or((0, 0));
        // In Foxglove's palette: the colour asked for, not its calibrated form.
        let key = calibration::key_for(&self.info.id);
        let (hue, sat, shown) = calibration::asked_hsb(
            &key,
            (
                ((u32::from(hue) * 360 + 128) / 256) as u16 % 360,
                to_percent(sat),
                to_percent(s.brightness),
            ),
        );
        Ok(LightingState {
            // A backlight's effect is breathing on or off; it's lit whenever it's bright.
            on: s.brightness > 0 && (s.effect > 0 || channel == Channel::Backlight),
            hue,
            sat,
            brightness: shown,
            effect: Effect {
                id: s.effect.to_string(),
                params: vec![],
            },
            speed: s.speed.unwrap_or(0),
            settled: true,
        })
    }

    /// Effects by QMK number (RGB Matrix and RGB Light: 0 = off).
    fn set_effect(&self, effect: Effect) -> Result<()> {
        let n: u8 = effect.id.parse().map_err(|_| DeviceError::Unsupported)?;
        self.set_value(EFFECT, &[n])
    }

    /// Through the keyboard's colour calibration; a channel without colour takes the
    /// brightness only.
    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> Result<()> {
        let (_, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        let key = calibration::key_for(&self.info.id);
        calibration::send_hsb(&key, (hue, sat, brightness), |(hue, sat, brightness)| {
            if channel.has_color() {
                let h = ((u32::from(hue) * 256) / 360) as u8;
                self.set_value(COLOR, &[h, from_percent(sat)])?;
            }
            self.set_value(BRIGHTNESS, &[from_percent(brightness)])
        })
    }

    fn release(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::fake::{leak, FakeQmk, FakeVial};
    use crate::definition::SAMPLE;
    use foxglove_devices::{BackendId, DeviceId, DeviceKind};

    /// A K5-like board: protocol 12, 4 layers, RGB Matrix and a backlight channel, ESC at 0,0.
    fn board(protocol_version: u16) -> &'static FakeQmk {
        let f = leak(FakeQmk {
            protocol_version,
            layers: 4,
            layout_options: Some(0),
            matrix: (2, 4),
            ..FakeQmk::default()
        });
        // 4 layers × 2 rows × 4 cols, all KC_NO but 0,0,0 = KC_ESC.
        let mut keymap = vec![0u8; 4 * 2 * 4 * 2];
        keymap[1] = 0x29;
        *f.keymap.lock().unwrap() = keymap;
        f.custom.lock().unwrap().extend([
            ((3, 1), [255, 0]),
            ((3, 2), [4, 0]),
            ((3, 3), [128, 0]),
            ((3, 4), [128, 255]),
            ((1, 1), [10, 0]),
            ((1, 2), [0, 0]),
        ]);
        f.legacy
            .lock()
            .unwrap()
            .extend([(0x09, [100, 0]), (0x0A, [0, 0])]);
        f
    }

    fn info(vid: u16, pid: u16) -> DeviceInfo {
        let id = DeviceId::new(BackendId("via".into()), vid, pid, None, "p");
        DeviceInfo {
            id: id.clone(),
            backend: id.backend.clone(),
            name: "test".into(),
            kind: DeviceKind::Keyboard,
            firmware: None,
        }
    }

    fn open_with(
        fake: &'static FakeQmk,
        def: Option<Definition>,
        writes: bool,
    ) -> ViaDevice<&'static FakeQmk> {
        let info = def
            .as_ref()
            .map(|d| info(d.vendor_id, d.product_id))
            .unwrap_or_else(|| info(0xFEED, 0x0001));
        let link = if writes {
            Guarded::with_writes(fake)
        } else {
            Guarded::read_only(fake)
        };
        ViaDevice::open(info, def, link, &Catalog::bundled()).unwrap()
    }

    fn open(writes: bool) -> (ViaDevice<&'static FakeQmk>, &'static FakeQmk) {
        let fake = board(12);
        let def = Definition::parse(SAMPLE).unwrap();
        (open_with(fake, Some(def), writes), fake)
    }

    #[test]
    fn read_only_device_passes_the_contract_and_never_writes() {
        let (dev, fake) = open(false);
        assert_eq!((dev.probe.protocol_version, dev.probe.layers), (12, 4));
        assert_eq!(dev.info().firmware.as_deref(), Some("VIA protocol 12"));
        assert_eq!(dev.lighting_channel(), Some(Channel::RgbMatrix));
        foxglove_devices::contract::check_device(&dev).unwrap();
        assert_eq!(dev.binding(0, 0, 0).unwrap().params, vec![0x29]);
        let all = dev.read_keymap().unwrap();
        assert_eq!((all.len(), all[0], all[1]), (32, 0x29, 0));
        let s = dev.lighting().unwrap().state().unwrap();
        assert_eq!(
            (s.on, s.hue, s.sat, s.brightness, s.speed),
            (true, 180, 100, 100, 128)
        );
        assert!(dev
            .lighting()
            .unwrap()
            .set_effect(Effect {
                id: "3".into(),
                params: vec![]
            })
            .is_err());
        let sent = fake.sent.lock().unwrap();
        assert!(!sent.iter().any(|r| protocol::is_write(r)), "{sent:x?}");
    }

    /// Systems, not products: a board with no VIA definition is still a device, with its
    /// lighting, but without a keymap until a definition gives its matrix.
    #[test]
    fn a_board_without_a_definition_has_lighting_but_no_keymap() {
        let fake = board(12);
        let dev = open_with(fake, None, false);
        assert_eq!(
            dev.source,
            Source::Missing { why: None },
            "not a Vial board"
        );
        assert!(dev.keymap().is_none() && !dev.caps().contains(Caps::KEYMAP));
        assert!(dev.caps().contains(Caps::LIGHTING));
        assert_eq!(dev.lighting_channel(), Some(Channel::RgbMatrix));
        assert_eq!(dev.read_keymap(), Err(DeviceError::Unsupported));
        assert_eq!(
            dev.summary(),
            Summary {
                firmware: Some("VIA protocol 12".into()),
                lighting: Some(Channel::RgbMatrix),
                source: Source::Missing { why: None },
                layers: None,
            }
        );
        foxglove_devices::contract::check_device(&dev).unwrap();
    }

    /// The layout is the one for the options the keyboard has chosen (#74); a keyboard that
    /// doesn't answer the layout options read gets the default. Either way it's only reads.
    #[test]
    fn the_layout_follows_the_keyboards_layout_options() {
        let def = Definition::parse(crate::definition::OPTIONS).unwrap();
        for (answer, value) in [(Some(0b1101), 0b1101), (Some(0), 0), (None, 0)] {
            let fake = leak(FakeQmk {
                protocol_version: 12,
                layers: 2,
                layout_options: answer,
                matrix: (3, 4),
                ..FakeQmk::default()
            });
            let dev = open_with(fake, Some(def.clone()), false);
            assert_eq!(dev.probe.layout_options, answer);
            let layout = Keymap::layout(&dev).unwrap();
            assert_eq!(layout, def.layout_for(value));
            let mut at: Vec<_> = layout.keys.iter().map(|k| (k.row, k.col)).collect();
            at.sort();
            at.dedup();
            assert_eq!(at.len(), layout.keys.len(), "each matrix position once");
            foxglove_devices::contract::check_device(&dev).unwrap();
            let sent = fake.sent.lock().unwrap();
            assert!(!sent.iter().any(|r| protocol::is_write(r)), "{sent:x?}");
        }
    }

    /// A Vial board's own definition (#74): no file loaded, and it still has a keymap, laid out
    /// for its layout options. Only reads go out.
    #[test]
    fn a_vial_board_without_a_file_gets_its_keymap_from_the_board() {
        let json = crate::definition::OPTIONS;
        let fake = leak(FakeQmk {
            protocol_version: 9,
            layers: 2,
            layout_options: Some(0b1101),
            matrix: (3, 4),
            vial: Some(FakeVial {
                protocol: 6,
                uid: *b"BANDIT01",
                blob: crate::vial::xz(json),
            }),
            ..FakeQmk::default()
        });
        let dev = open_with(fake, None, false);
        assert_eq!(dev.source, Source::Vial { protocol: 6 });
        assert_eq!(
            dev.info().firmware.as_deref(),
            Some("VIA protocol 9, Vial protocol 6")
        );
        assert!(dev.caps().contains(Caps::KEYMAP));
        let def = Definition::from_board(json, 0xFEED, 0x0001, "test").unwrap();
        assert_eq!(Keymap::layout(&dev).unwrap(), def.layout_for(0b1101));
        assert_eq!(dev.read_keymap().unwrap().len(), 2 * 3 * 4);
        let s = dev.summary();
        assert_eq!(
            (s.source, s.layers, s.firmware.as_deref()),
            (
                Source::Vial { protocol: 6 },
                Some(2),
                Some("VIA protocol 9, Vial protocol 6")
            )
        );
        foxglove_devices::contract::check_device(&dev).unwrap();
        let sent = fake.sent.lock().unwrap();
        assert!(!sent.iter().any(|r| protocol::is_write(r)), "{sent:x?}");
    }

    /// A Vial board whose definition doesn't unpack still opens, with its lighting, and says why
    /// it has no keymap.
    #[test]
    fn a_vial_board_with_a_bad_definition_opens_without_a_keymap() {
        let mut blob = crate::vial::xz(SAMPLE);
        let last = blob.len() - 1;
        blob[last] ^= 0xFF; // the footer's magic
        let fake = leak(FakeQmk {
            protocol_version: 9,
            layers: 4,
            vial: Some(FakeVial {
                protocol: 6,
                uid: [0; 8],
                blob,
            }),
            ..FakeQmk::default()
        });
        fake.legacy
            .lock()
            .unwrap()
            .extend([(0x09, [100, 0]), (0x0A, [0, 0])]);
        let dev = open_with(fake, None, false);
        assert!(
            matches!(&dev.source, Source::Missing { why: Some(_) }),
            "{:?}",
            dev.source
        );
        assert!(dev.keymap().is_none());
        assert_eq!(dev.lighting_channel(), Some(Channel::Backlight));
        foxglove_devices::contract::check_device(&dev).unwrap();
    }

    /// A loaded definition wins: the board isn't asked for its own.
    #[test]
    fn a_loaded_definition_sends_no_vial_reads() {
        let (dev, fake) = open(false);
        assert_eq!(dev.source, Source::Loaded);
        assert!(!fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|r| r[0] == protocol::VIAL_PREFIX));
    }

    #[test]
    fn the_definition_picks_among_the_channels_that_answered() {
        let fake = board(12);
        let def = Definition::parse(&SAMPLE.replace("qmk_rgb_matrix", "qmk_backlight")).unwrap();
        let dev = open_with(fake, Some(def), false);
        assert_eq!(dev.lighting_channel(), Some(Channel::Backlight));
        let s = dev.lighting().unwrap().state().unwrap();
        assert_eq!((s.on, s.sat, s.brightness, s.speed), (true, 0, 4, 0));
    }

    /// Vial and older VIA firmware report an older protocol; lighting is read through QMK's
    /// fixed ids instead of channels.
    #[test]
    fn an_older_protocol_reads_lighting_through_the_legacy_ids() {
        let fake = board(9);
        let dev = open_with(fake, None, false);
        assert_eq!(dev.probe.scheme, Some(Scheme::Legacy));
        assert_eq!(dev.lighting_channel(), Some(Channel::Backlight));
        let s = dev.lighting().unwrap().state().unwrap();
        assert_eq!((s.on, s.brightness), (true, 39));
        assert!(fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|r| r[..2] == [protocol::CUSTOM_GET_VALUE, 0x09]));
    }

    /// #74: a K5 Pro white (3434:0253, VIA protocol 11) with the user's VIA definition: matched
    /// by its vendor and product id, it gets its keymap; the catalog says it has no VIA
    /// lighting, so no lighting value is ever asked for, and nothing is written.
    #[test]
    fn a_k5_pro_white_with_a_loaded_definition_gets_its_keymap_and_no_lighting_reads() {
        let fake = board(11);
        let def = Definition::parse(&SAMPLE.replace("0x0250", "0x0253")).unwrap();
        let dev = open_with(fake, Some(def), false);
        assert_eq!(dev.source, Source::Loaded);
        assert_eq!(dev.info().firmware.as_deref(), Some("VIA protocol 11"));
        assert_eq!(dev.probe.scheme, None);
        assert!(dev.lighting().is_none());
        assert_eq!(dev.binding(0, 0, 0).unwrap().params, vec![0x29]);
        let sent = fake.sent.lock().unwrap();
        assert!(
            !sent.iter().any(|r| r[0] == protocol::CUSTOM_GET_VALUE),
            "{sent:x?}"
        );
        assert!(!sent.iter().any(|r| protocol::is_write(r)), "{sent:x?}");
        assert!(
            !sent.iter().any(|r| (0xA0..=0xAB).contains(&r[0])),
            "no Keychron command"
        );
    }

    #[test]
    fn a_board_with_no_lighting_has_no_lighting() {
        let fake = leak(FakeQmk {
            protocol_version: 12,
            layers: 2,
            ..FakeQmk::default()
        });
        let dev = open_with(fake, None, false);
        assert!(dev.lighting().is_none() && dev.caps() == Caps::default());
        foxglove_devices::contract::check_device(&dev).unwrap();
    }

    /// #581: with `via-writes` off (0.2), a device opened asking for writes is read-only: no
    /// keymap write support, every keycode and lighting write refused, reads still answered, and
    /// the fake keyboard (which fails on any write opcode) never sees one.
    #[cfg(not(feature = "via-writes"))]
    #[test]
    fn writes_off_nothing_is_written_even_when_writes_are_asked_for() {
        let (dev, fake) = open(true);
        assert!(!dev.caps().contains(Caps::KEYMAP_WRITE));
        assert_eq!(Keymap::write_support(&dev), WriteSupport::None);
        foxglove_devices::contract::check_device(&dev).unwrap();
        assert_eq!(
            dev.binding(0, 0, 0).unwrap().params,
            vec![0x29],
            "reads stay on"
        );
        assert!(dev
            .set_binding(
                0,
                0,
                0,
                Binding {
                    kind: "qmk".into(),
                    params: vec![0x0004],
                    label: None,
                }
            )
            .is_err());
        let lighting = dev.lighting().unwrap();
        assert!(lighting.state().is_ok(), "lighting reads stay on");
        assert!(lighting.set_color(180, 50, 100).is_err());
        assert!(lighting
            .set_effect(Effect {
                id: "4".into(),
                params: vec![]
            })
            .is_err());
        for &c in protocol::WRITE_OPCODES {
            assert!(dev.link.send(&[c, 0, 0, 0, 0, 0]).is_err(), "{c:#04x}");
        }
        let sent = fake.sent.lock().unwrap();
        assert!(!sent.iter().any(|r| protocol::is_write(r)), "{sent:x?}");
    }

    #[cfg(feature = "via-writes")]
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
            !fake
                .sent
                .lock()
                .unwrap()
                .iter()
                .any(|r| r[0] == protocol::CUSTOM_SAVE),
            "nothing persisted"
        );
    }
}
