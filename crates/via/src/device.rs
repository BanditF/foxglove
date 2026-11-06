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
//! Lighting changes through the device layer are not saved to the keyboard's EEPROM (no
//! CUSTOM_SAVE): they last until the keyboard restarts, so experiments can't wear or overwrite
//! the stored settings. Only [`ViaDevice::save_lighting`], called on purpose, saves.
//!
//! Every write needs a link opened with writes on ([`Guarded::with_writes`]: a `via-writes`
//! build and the dev switch, #723), and is checked here before it goes out: keycodes against the
//! layers and matrix, layout options against the definition.

use foxglove_devices::calibration;
use foxglove_devices::{
    Binding, Caps, Device, DeviceError, DeviceInfo, Effect, Keymap, LayerInfo, Layout, Led,
    Lighting, LightingState, Result, WriteSupport, Zone,
};

use crate::catalog::Catalog;
use crate::client::{Channel, ChannelState, Client, Probe, Scheme};
use crate::definition::Definition;
use crate::protocol::{self, Guarded, Transport};
use crate::vial;

/// Where a device's definition came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A VIA definition file loaded for it.
    Loaded,
    /// A QMK keyboard.json / info.json loaded for it: the physical layout is the named
    /// `LAYOUT_*`, the layers the board's own keymap read.
    QmkLayout { layout: String },
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
    /// Whether its lighting was asked for: false when the catalog says the model has none
    /// (`lighting = "none"`), so it never was.
    pub lighting_asked: bool,
    pub source: Source,
    /// Layers, when it has a keymap.
    pub layers: Option<u8>,
    /// The layout options the keyboard has chosen, in words ([`Definition::chosen_names`]):
    /// what its drawn layout follows. Empty without a definition or options.
    pub layout_options: Vec<String>,
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
            Some(d) => {
                let source = match &d.qmk_layout {
                    Some(layout) => Source::QmkLayout {
                        layout: layout.clone(),
                    },
                    None => Source::Loaded,
                };
                (Some(d), source)
            }
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
            lighting_asked: self.probe.scheme.is_some(),
            source: self.source.clone(),
            layers: self.def.is_some().then_some(self.probe.layers),
            layout_options: self.def.as_ref().map_or_else(Vec::new, |d| {
                d.chosen_names(self.probe.layout_options.unwrap_or(0))
            }),
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
        Client(&self.link).set_lighting_value(scheme, channel, value, data)
    }

    /// Stores the shown channel's current lighting in the keyboard's EEPROM (`CUSTOM_SAVE`).
    /// The device layer's lighting never calls this (its changes last until a restart); only
    /// an explicit save does. Needs writes on.
    pub fn save_lighting(&self) -> Result<()> {
        let (scheme, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        Client(&self.link).save_lighting(scheme, channel)
    }

    /// The shown channel's lighting as the keyboard has it (QMK's 0-255 values), for an exact
    /// snapshot: the device layer's [`Lighting::state`] is in percent and degrees, which
    /// rounds.
    pub fn raw_lighting(&self) -> Result<ChannelState> {
        let (scheme, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        Client(&self.link).lighting(scheme, channel)
    }

    /// Puts a [`ViaDevice::raw_lighting`] snapshot back exactly (RAM only, no save): colour,
    /// speed, effect, then brightness. Needs writes on.
    pub fn restore_raw_lighting(&self, s: &ChannelState) -> Result<()> {
        let (_, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        if s.channel != channel {
            return Err(DeviceError::Denied(format!(
                "a {} snapshot doesn't go on {}",
                s.channel.label(),
                channel.label()
            )));
        }
        if let Some((h, sat)) = s.color {
            self.set_value(COLOR, &[h, sat])?;
        }
        if let Some(speed) = s.speed {
            self.set_value(EFFECT_SPEED, &[speed])?;
        }
        self.set_value(EFFECT, &[s.effect])?;
        self.set_value(BRIGHTNESS, &[s.brightness])
    }

    /// The layout options the keyboard reports now (a fresh read, not the one from the open).
    pub fn read_layout_options(&self) -> Result<u32> {
        Client(&self.link).layout_options()
    }

    /// Sets the keyboard's layout options (#723). Needs writes on, a definition with layout
    /// options, and a value it can mean ([`Definition::valid_layout_options`]). The keyboard
    /// stores it; the drawn layout follows on the next open.
    pub fn set_layout_options(&self, value: u32) -> Result<()> {
        if !self.link.writes() {
            return Err(DeviceError::Unsupported);
        }
        let def = self.def.as_ref().ok_or(DeviceError::Unsupported)?;
        if def.options.is_empty() || self.probe.layout_options.is_none() {
            return Err(DeviceError::Unsupported);
        }
        if !def.valid_layout_options(value) {
            return Err(DeviceError::Denied(format!(
                "layout options {value:#x} aren't ones this keyboard's definition has"
            )));
        }
        Client(&self.link).set_layout_options(value)
    }

    /// Layer, row and column as bytes, checked against the keyboard's layers and the
    /// definition's matrix: a write outside them would land in another key's (or another
    /// setting's) EEPROM bytes.
    fn position(&self, layer: u32, row: u16, col: u16) -> Result<(u8, u8, u8)> {
        let def = self.def.as_ref().ok_or(DeviceError::Unsupported)?;
        if layer >= u32::from(self.probe.layers) || row >= def.rows || col >= def.cols {
            return Err(DeviceError::Denied(format!(
                "layer {layer}, row {row}, column {col} is outside this keyboard's {} layers \
                 of {}x{}",
                self.probe.layers, def.rows, def.cols
            )));
        }
        Ok((byte(layer)?, byte(row.into())?, byte(col.into())?))
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
const EFFECT_SPEED: u8 = 3;
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
        let (layer, row, col) = self.position(layer, row, col)?;
        Client(&self.link).set_keycode(layer, row, col, code)
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

    /// The effect speed, 0-255 as QMK has it; a channel without one (backlight) has none.
    fn set_speed(&self, speed: u8) -> Result<()> {
        let (_, channel) = self.light.ok_or(DeviceError::Unsupported)?;
        if !channel.has_speed() {
            return Err(DeviceError::Unsupported);
        }
        self.set_value(EFFECT_SPEED, &[speed])
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

    /// Writes on whatever the build (the write-path tests, #723), on a fake that takes them.
    fn open_writable(
        protocol_version: u16,
        def: Definition,
        layout_options: Option<u32>,
    ) -> (ViaDevice<&'static FakeQmk>, &'static FakeQmk) {
        let fake = board(protocol_version);
        // `board` leaks a fake that takes no writes; this one does, with the same contents.
        let fake = leak(FakeQmk {
            protocol_version,
            layers: fake.layers,
            layout_options,
            matrix: fake.matrix,
            keymap: std::sync::Mutex::new(fake.keymap.lock().unwrap().clone()),
            custom: std::sync::Mutex::new(fake.custom.lock().unwrap().clone()),
            legacy: std::sync::Mutex::new(fake.legacy.lock().unwrap().clone()),
            accept_writes: true,
            ..FakeQmk::default()
        });
        let info = info(def.vendor_id, def.product_id);
        let dev = ViaDevice::open(
            info,
            Some(def),
            Guarded::writable(fake),
            &Catalog::bundled(),
        )
        .unwrap();
        (dev, fake)
    }

    fn qmk(code: u32) -> Binding {
        Binding {
            kind: "qmk".into(),
            params: vec![code],
            label: None,
        }
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
                lighting_asked: true,
                source: Source::Missing { why: None },
                layers: None,
                layout_options: vec![],
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
            assert_eq!(dev.summary().layout_options, def.chosen_names(value));
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

    /// A board with no VIA definition anywhere (#74, the K5 Pro case): a loaded QMK
    /// keyboard.json gives the matrix and layout, the board's own keymap read the layers. Only
    /// reads go out, and no Vial ones.
    #[test]
    fn a_qmk_layout_gives_a_keymap_with_the_boards_own_layers() {
        let def = crate::qmk::board(&["white", "ansi"]);
        let fake = leak(FakeQmk {
            protocol_version: 11,
            layers: 4,
            matrix: (2, 4),
            ..FakeQmk::default()
        });
        let dev = open_with(fake, Some(def.clone()), false);
        assert_eq!(
            dev.source,
            Source::QmkLayout {
                layout: "LAYOUT_7_ansi".into()
            }
        );
        assert!(dev.caps().contains(Caps::KEYMAP));
        assert!(!dev.caps().contains(Caps::KEYMAP_WRITE));
        assert_eq!(Keymap::layout(&dev).unwrap(), def.layout);
        assert_eq!(dev.read_keymap().unwrap().len(), 4 * 2 * 4);
        assert_eq!(dev.summary().layers, Some(4));
        foxglove_devices::contract::check_device(&dev).unwrap();
        let sent = fake.sent.lock().unwrap();
        assert!(!sent.iter().any(|r| protocol::is_write(r)), "{sent:x?}");
        assert!(!sent.iter().any(|r| r[0] == protocol::VIAL_PREFIX));
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
        let s = dev.summary();
        assert_eq!((s.lighting, s.lighting_asked), (None, false));
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

    /// #581/#723: with writes off (the default: no `via-writes` build, or no dev switch), a
    /// device opened asking for writes is read-only: no keymap write support, every keycode,
    /// lighting, save and layout-option write refused, reads still answered, and the fake
    /// keyboard (which fails on any write opcode) never sees one.
    #[test]
    fn writes_off_nothing_is_written_even_when_writes_are_asked_for() {
        if protocol::writes_enabled() {
            return; // a dev running the tests with the switch on
        }
        let (dev, fake) = open(true);
        assert!(dev.save_lighting().is_err());
        assert!(dev.set_layout_options(0).is_err());
        assert!(dev.lighting().unwrap().set_speed(10).is_err());
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

    #[test]
    fn writable_device_writes_keycodes_and_lighting_without_saving() {
        let (dev, fake) = open_writable(12, Definition::parse(SAMPLE).unwrap(), Some(0));
        assert!(dev.caps().contains(Caps::KEYMAP_WRITE));
        assert_eq!(Keymap::write_support(&dev), WriteSupport::Immediate);
        foxglove_devices::contract::check_device(&dev).unwrap();
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

    /// #723: a keycode write outside the keyboard's layers or the definition's matrix, a
    /// bootloader keycode, or a binding that isn't one QMK keycode never goes out.
    #[test]
    fn keycode_writes_are_bounded() {
        let (dev, fake) = open_writable(12, Definition::parse(SAMPLE).unwrap(), Some(0));
        let def = Definition::parse(SAMPLE).unwrap();
        let (rows, cols) = (def.rows, def.cols);
        let sent_before = fake.sent.lock().unwrap().len();
        for (layer, row, col) in [(4, 0, 0), (0, rows, 0), (0, 0, cols), (300, 0, 0)] {
            assert!(
                matches!(
                    dev.set_binding(layer, row, col, qmk(4)),
                    Err(DeviceError::Denied(_))
                ),
                "{layer},{row},{col}"
            );
        }
        for &code in protocol::FORBIDDEN_KEYCODES {
            assert!(matches!(
                dev.set_binding(0, 0, 0, qmk(u32::from(code))),
                Err(DeviceError::Denied(_))
            ));
        }
        for b in [
            qmk(0x1_0000),
            Binding {
                kind: "zmk".into(),
                params: vec![4],
                label: None,
            },
            Binding {
                kind: "qmk".into(),
                params: vec![4, 5],
                label: None,
            },
        ] {
            assert_eq!(dev.set_binding(0, 0, 0, b), Err(DeviceError::Unsupported));
        }
        assert_eq!(
            fake.sent.lock().unwrap().len(),
            sent_before,
            "none went out"
        );
        // The last key of the last layer is in bounds.
        dev.set_binding(3, rows - 1, cols - 1, qmk(4)).unwrap();
        assert_eq!(dev.binding(3, rows - 1, cols - 1).unwrap().params, vec![4]);
    }

    /// #723: a raw snapshot comes back exactly, where the device layer's percent would round
    /// (brightness 200 is 78 %, which is 199 again).
    #[test]
    fn a_raw_lighting_snapshot_restores_exactly() {
        let (dev, fake) = open_writable(12, Definition::parse(SAMPLE).unwrap(), Some(0));
        fake.custom.lock().unwrap().insert((3, 1), [200, 0]);
        let before = dev.raw_lighting().unwrap();
        assert_eq!(before.brightness, 200);
        let lighting = dev.lighting().unwrap();
        lighting.set_color(10, 20, 30).unwrap();
        lighting.set_speed(7).unwrap();
        lighting
            .set_effect(Effect {
                id: "1".into(),
                params: vec![],
            })
            .unwrap();
        assert_ne!(dev.raw_lighting().unwrap(), before);
        dev.restore_raw_lighting(&before).unwrap();
        assert_eq!(dev.raw_lighting().unwrap(), before);
        assert!(!fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|r| r[0] == protocol::CUSTOM_SAVE));
        let other = ChannelState {
            channel: Channel::Backlight,
            ..before
        };
        assert!(matches!(
            dev.restore_raw_lighting(&other),
            Err(DeviceError::Denied(_))
        ));
    }

    /// #723: speed and an explicit save; the save names the shown channel.
    #[test]
    fn speed_and_an_explicit_save() {
        let (dev, fake) = open_writable(12, Definition::parse(SAMPLE).unwrap(), Some(0));
        let lighting = dev.lighting().unwrap();
        let before = lighting.state().unwrap().speed;
        lighting.set_speed(200).unwrap();
        assert_eq!(lighting.state().unwrap().speed, 200);
        lighting.set_speed(before).unwrap();
        assert_eq!(lighting.state().unwrap().speed, before);
        dev.save_lighting().unwrap();
        let saves: Vec<_> = fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[0] == protocol::CUSTOM_SAVE)
            .map(|r| r[1])
            .collect();
        assert_eq!(saves, vec![Channel::RgbMatrix.id()]);
    }

    /// Legacy firmware (and Vial): lighting writes go to QMK's fixed ids; a backlight has no
    /// speed.
    #[test]
    fn legacy_lighting_writes_use_the_fixed_ids() {
        let (dev, fake) = open_writable(9, Definition::parse(SAMPLE).unwrap(), None);
        assert_eq!(dev.lighting_channel(), Some(Channel::Backlight));
        let lighting = dev.lighting().unwrap();
        lighting.set_color(0, 0, 50).unwrap();
        assert_eq!(lighting.state().unwrap().brightness, 50);
        assert_eq!(lighting.set_speed(1), Err(DeviceError::Unsupported));
        assert!(fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|r| r[..3] == [protocol::CUSTOM_SET_VALUE, 0x09, from_percent(50)]));
    }

    /// #723: the layout options are written only as a value the definition can mean, and only
    /// to a keyboard that reported them.
    #[test]
    fn layout_option_writes_are_checked_against_the_definition() {
        let def = Definition::parse(crate::definition::OPTIONS).unwrap();
        let (dev, fake) = open_writable(12, def.clone(), Some(0));
        let before = dev.read_layout_options().unwrap();
        dev.set_layout_options(0b1101).unwrap();
        assert_eq!(dev.read_layout_options().unwrap(), 0b1101);
        for bad in [0b0110, 0b1_0000] {
            assert!(matches!(
                dev.set_layout_options(bad),
                Err(DeviceError::Denied(_))
            ));
        }
        dev.set_layout_options(before).unwrap();
        assert_eq!(dev.read_layout_options().unwrap(), before);
        let writes = fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[0] == protocol::SET_KEYBOARD_VALUE)
            .count();
        assert_eq!(writes, 2);

        // No options in the definition, or none reported: nothing to set.
        let mut no_options = Definition::parse(SAMPLE).unwrap();
        no_options.options.clear();
        let (plain, _) = open_writable(12, no_options, Some(0));
        assert_eq!(plain.set_layout_options(0), Err(DeviceError::Unsupported));
        let (silent, _) = open_writable(12, def, None);
        assert_eq!(silent.set_layout_options(0), Err(DeviceError::Unsupported));
    }

    /// #723: a Vial board with writes on gets keycode writes, and never a Vial command that
    /// isn't a definition read (unlock, settings): the fake fails the test on one.
    #[test]
    fn a_writable_vial_board_is_never_sent_unlock() {
        let json = crate::definition::OPTIONS;
        let fake = leak(FakeQmk {
            protocol_version: 9,
            layers: 2,
            layout_options: Some(0),
            matrix: (3, 4),
            vial: Some(FakeVial {
                protocol: 6,
                uid: *b"BANDIT01",
                blob: crate::vial::xz(json),
            }),
            accept_writes: true,
            ..FakeQmk::default()
        });
        let dev = ViaDevice::open(
            info(0xFEED, 0x0001),
            None,
            Guarded::writable(fake),
            &Catalog::bundled(),
        )
        .unwrap();
        assert_eq!(dev.source, Source::Vial { protocol: 6 });
        let before = dev.binding(1, 2, 3).unwrap();
        dev.set_binding(1, 2, 3, qmk(4)).unwrap();
        assert_eq!(dev.binding(1, 2, 3).unwrap().params, vec![4]);
        dev.set_binding(1, 2, 3, before.clone()).unwrap();
        assert_eq!(dev.binding(1, 2, 3).unwrap(), before);
        let vial: Vec<u8> = fake
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[0] == protocol::VIAL_PREFIX)
            .map(|r| r[1])
            .collect();
        assert!(
            vial.iter().all(|c| protocol::VIAL_READS.contains(c)),
            "{vial:x?}"
        );
    }
}
