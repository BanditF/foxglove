//! A read client for VIA over QMK Raw HID (#74): protocol version, layer count, layout options,
//! the keymap through buffer reads, and the lighting values of each lighting channel.
//!
//! Written from VIA's protocol documentation and QMK's docs. Everything here only reads, and
//! every report still goes through [`Guarded`], so nothing that changes the keyboard can go out.
//!
//! Lighting is addressed one of two ways, by VIA protocol version (see [`Scheme`]); which one a
//! board uses is decided by the catalog ([`crate::catalog::Catalog::scheme`]).

use foxglove_devices::{DeviceError, Result};

use crate::protocol::{self, keyboard_value, Guarded, Transport, REPORT};

/// How lighting values are addressed on `CUSTOM_GET_VALUE` (0x08).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// VIA protocol 12 and later: `[0x08, channel, value id]`, the reply's data from byte 3.
    Custom,
    /// Older firmware and Vial: `[0x08, value id]` with QMK's fixed lighting ids, the reply's
    /// data from byte 2. Firmware that maps RGB Matrix onto the RGB Light ids answers as
    /// [`Channel::RgbLight`].
    Legacy,
}

/// A QMK lighting feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Single-colour backlight: brightness and effect (breathing).
    Backlight,
    /// RGB Light (underglow): brightness, effect, speed, colour.
    RgbLight,
    /// RGB Matrix (per-key RGB): brightness, effect, speed, colour.
    RgbMatrix,
    /// LED Matrix (per-key single colour): brightness, effect, speed.
    LedMatrix,
}

/// Value ids within a channel (custom scheme).
mod value {
    pub const BRIGHTNESS: u8 = 1;
    pub const EFFECT: u8 = 2;
    pub const EFFECT_SPEED: u8 = 3;
    pub const COLOR: u8 = 4;
}

/// QMK's fixed lighting value ids (legacy scheme).
mod legacy {
    pub const BACKLIGHT_BRIGHTNESS: u8 = 0x09;
    pub const BACKLIGHT_EFFECT: u8 = 0x0A;
    pub const RGBLIGHT_BRIGHTNESS: u8 = 0x80;
    pub const RGBLIGHT_EFFECT: u8 = 0x81;
    pub const RGBLIGHT_EFFECT_SPEED: u8 = 0x82;
    pub const RGBLIGHT_COLOR: u8 = 0x83;
}

impl Channel {
    /// The channel number in the custom scheme.
    pub fn id(self) -> u8 {
        match self {
            Channel::Backlight => 1,
            Channel::RgbLight => 2,
            Channel::RgbMatrix => 3,
            Channel::LedMatrix => 5,
        }
    }

    /// Whether it has a colour (hue and saturation).
    pub fn has_color(self) -> bool {
        matches!(self, Channel::RgbLight | Channel::RgbMatrix)
    }

    /// Whether it has an effect speed.
    pub fn has_speed(self) -> bool {
        !matches!(self, Channel::Backlight)
    }

    /// The channels a scheme can address, best first (the one Foxglove shows).
    pub fn all(scheme: Scheme) -> &'static [Channel] {
        match scheme {
            Scheme::Custom => &[
                Channel::RgbMatrix,
                Channel::RgbLight,
                Channel::LedMatrix,
                Channel::Backlight,
            ],
            Scheme::Legacy => &[Channel::RgbLight, Channel::Backlight],
        }
    }

    /// The legacy id for one of the channel's values, if the legacy scheme has it.
    fn legacy_id(self, value: u8) -> Option<u8> {
        match (self, value) {
            (Channel::Backlight, value::BRIGHTNESS) => Some(legacy::BACKLIGHT_BRIGHTNESS),
            (Channel::Backlight, value::EFFECT) => Some(legacy::BACKLIGHT_EFFECT),
            (Channel::RgbLight, value::BRIGHTNESS) => Some(legacy::RGBLIGHT_BRIGHTNESS),
            (Channel::RgbLight, value::EFFECT) => Some(legacy::RGBLIGHT_EFFECT),
            (Channel::RgbLight, value::EFFECT_SPEED) => Some(legacy::RGBLIGHT_EFFECT_SPEED),
            (Channel::RgbLight, value::COLOR) => Some(legacy::RGBLIGHT_COLOR),
            _ => None,
        }
    }

    /// The report that reads (`command` = `CUSTOM_GET_VALUE`) or sets one value, and where its
    /// data starts in the reply.
    pub(crate) fn request(
        self,
        scheme: Scheme,
        command: u8,
        value: u8,
    ) -> Result<(Vec<u8>, usize)> {
        match scheme {
            Scheme::Custom => Ok((vec![command, self.id(), value], 3)),
            Scheme::Legacy => {
                let id = self.legacy_id(value).ok_or(DeviceError::Unsupported)?;
                Ok((vec![command, id], 2))
            }
        }
    }
}

/// One lighting channel's values as the keyboard reports them (QMK's 0-255 ranges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelState {
    pub channel: Channel,
    pub brightness: u8,
    pub effect: u8,
    pub speed: Option<u8>,
    /// Hue and saturation, 0-255.
    pub color: Option<(u8, u8)>,
}

/// What a VIA keyboard says about itself when it's opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub protocol_version: u16,
    pub layers: u8,
    /// The layout options bitfield (`None` when the keyboard doesn't answer it).
    pub layout_options: Option<u32>,
    /// How lighting is addressed (`None`: the catalog says not to read it).
    pub scheme: Option<Scheme>,
    /// The lighting channels that answered, best first.
    pub lighting: Vec<ChannelState>,
}

/// Reads over an allowlisted link. Every method only reads.
pub struct Client<'a, T: Transport>(pub &'a Guarded<T>);

impl<T: Transport> Client<'_, T> {
    pub fn protocol_version(&self) -> Result<u16> {
        let r = self.0.send(&[protocol::GET_PROTOCOL_VERSION])?;
        Ok(u16::from_be_bytes([r[1], r[2]]))
    }

    pub fn layer_count(&self) -> Result<u8> {
        Ok(self.0.send(&[protocol::DYNAMIC_KEYMAP_GET_LAYER_COUNT])?[1])
    }

    /// The layout options bitfield (which split backspace, ISO enter and so on).
    pub fn layout_options(&self) -> Result<u32> {
        let r = self
            .0
            .send(&[protocol::GET_KEYBOARD_VALUE, keyboard_value::LAYOUT_OPTIONS])?;
        Ok(u32::from_be_bytes([r[2], r[3], r[4], r[5]]))
    }

    /// One lighting value's data bytes.
    pub fn lighting_value(&self, scheme: Scheme, channel: Channel, value: u8) -> Result<[u8; 2]> {
        let (report, at) = channel.request(scheme, protocol::CUSTOM_GET_VALUE, value)?;
        let r = self.0.send(&report)?;
        if r[1..at] != report[1..at] {
            return Err(DeviceError::Backend(format!(
                "lighting reply for {:x?}, asked {:x?}",
                &r[1..at],
                &report[1..at]
            )));
        }
        Ok([r[at], r[at + 1]])
    }

    /// A channel's values; `Unsupported` when the keyboard doesn't have it.
    pub fn lighting(&self, scheme: Scheme, channel: Channel) -> Result<ChannelState> {
        let brightness = self.lighting_value(scheme, channel, value::BRIGHTNESS)?[0];
        let effect = self.lighting_value(scheme, channel, value::EFFECT)?[0];
        let speed = match channel.has_speed() {
            true => Some(self.lighting_value(scheme, channel, value::EFFECT_SPEED)?[0]),
            false => None,
        };
        let color = match channel.has_color() {
            true => {
                let [h, s] = self.lighting_value(scheme, channel, value::COLOR)?;
                Some((h, s))
            }
            false => None,
        };
        Ok(ChannelState {
            channel,
            brightness,
            effect,
            speed,
            color,
        })
    }

    /// Every channel the scheme can address that the keyboard answers, best first. A channel
    /// it marks unhandled is skipped; one it doesn't answer at all ends the search, so a
    /// firmware that stays silent costs one reply timeout, not one per channel.
    pub fn lighting_channels(&self, scheme: Scheme) -> Result<Vec<ChannelState>> {
        let mut found = Vec::new();
        for &channel in Channel::all(scheme) {
            match self.lighting(scheme, channel) {
                Ok(state) => found.push(state),
                Err(DeviceError::Unsupported) => {}
                Err(DeviceError::Timeout) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(found)
    }

    /// The whole dynamic keymap, `layers × rows × cols` keycodes (layer, then row, then column),
    /// read through the keymap buffer in 28-byte pieces.
    pub fn keymap(&self, layers: u8, rows: u16, cols: u16) -> Result<Vec<u16>> {
        const PIECE: usize = REPORT - 4;
        let bytes = usize::from(layers) * usize::from(rows) * usize::from(cols) * 2;
        if bytes > usize::from(u16::MAX) + 1 {
            return Err(DeviceError::Unsupported);
        }
        let mut buf = Vec::with_capacity(bytes);
        while buf.len() < bytes {
            let offset = buf.len() as u16;
            let size = (bytes - buf.len()).min(PIECE);
            let [hi, lo] = offset.to_be_bytes();
            let r = self
                .0
                .send(&[protocol::DYNAMIC_KEYMAP_GET_BUFFER, hi, lo, size as u8])?;
            if r[1..3] != [hi, lo] {
                return Err(DeviceError::Backend(format!(
                    "keymap buffer reply at {:#06x}, asked {offset:#06x}",
                    u16::from_be_bytes([r[1], r[2]])
                )));
            }
            buf.extend_from_slice(&r[4..4 + size]);
        }
        let (pairs, _) = buf.as_chunks::<2>();
        Ok(pairs.iter().map(|&p| u16::from_be_bytes(p)).collect())
    }

    /// Protocol version, layers, layout options and lighting. A keyboard that doesn't answer
    /// the protocol version or layer count isn't a VIA keyboard (the error says why); layout
    /// options and lighting are optional, so a firmware that doesn't answer them still opens.
    pub fn probe(&self, scheme: impl FnOnce(u16) -> Option<Scheme>) -> Result<Probe> {
        let protocol_version = self.protocol_version()?;
        let layers = self.layer_count()?;
        let layout_options = match self.layout_options() {
            Ok(v) => Some(v),
            Err(DeviceError::Unsupported | DeviceError::Timeout) => None,
            Err(e) => return Err(e),
        };
        let scheme = scheme(protocol_version);
        let lighting = match scheme {
            Some(s) => self.lighting_channels(s)?,
            None => Vec::new(),
        };
        Ok(Probe {
            protocol_version,
            layers,
            layout_options,
            scheme,
            lighting,
        })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A QMK keyboard in memory that answers VIA like the protocol docs describe.

    use super::*;
    use crate::protocol::Report;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeQmk {
        pub protocol_version: u16,
        pub layers: u8,
        pub layout_options: Option<u32>,
        /// Rows and columns, for single-key reads and writes.
        pub matrix: (u8, u8),
        /// Dynamic keymap bytes (big-endian keycodes).
        pub keymap: Mutex<Vec<u8>>,
        /// Lighting values by (channel id, value id) in the custom scheme.
        pub custom: Mutex<HashMap<(u8, u8), [u8; 2]>>,
        /// Lighting values by legacy id.
        pub legacy: Mutex<HashMap<u8, [u8; 2]>>,
        /// Every report that reached it.
        pub sent: Mutex<Vec<Vec<u8>>>,
    }

    impl FakeQmk {
        pub fn custom_scheme(&self) -> bool {
            self.protocol_version >= 12
        }
    }

    impl Transport for &'static FakeQmk {
        fn exchange(&self, r: &Report) -> Result<Report> {
            assert!(
                protocol::WRITES_ENABLED || !protocol::is_write(r),
                "write {:#04x} reached the keyboard with VIA/Vial writes off",
                r[0]
            );
            self.sent.lock().unwrap().push(r[..6].to_vec());
            let mut out = *r;
            let unhandled = |mut out: Report| {
                out[0] = protocol::UNHANDLED;
                Ok(out)
            };
            match r[0] {
                protocol::GET_PROTOCOL_VERSION => {
                    out[1..3].copy_from_slice(&self.protocol_version.to_be_bytes())
                }
                protocol::DYNAMIC_KEYMAP_GET_LAYER_COUNT => out[1] = self.layers,
                protocol::GET_KEYBOARD_VALUE => match (r[1], self.layout_options) {
                    (keyboard_value::LAYOUT_OPTIONS, Some(v)) => {
                        out[2..6].copy_from_slice(&v.to_be_bytes())
                    }
                    _ => return unhandled(out),
                },
                protocol::DYNAMIC_KEYMAP_GET_KEYCODE | protocol::DYNAMIC_KEYMAP_SET_KEYCODE => {
                    let (rows, cols) = (usize::from(self.matrix.0), usize::from(self.matrix.1));
                    let at = ((usize::from(r[1]) * rows + usize::from(r[2])) * cols
                        + usize::from(r[3]))
                        * 2;
                    let mut keymap = self.keymap.lock().unwrap();
                    if keymap.len() < at + 2 {
                        keymap.resize(at + 2, 0);
                    }
                    if r[0] == protocol::DYNAMIC_KEYMAP_SET_KEYCODE {
                        keymap[at..at + 2].copy_from_slice(&r[4..6]);
                    } else {
                        out[4..6].copy_from_slice(&keymap[at..at + 2]);
                    }
                }
                protocol::DYNAMIC_KEYMAP_GET_BUFFER => {
                    let offset = usize::from(u16::from_be_bytes([r[1], r[2]]));
                    let size = usize::from(r[3]);
                    let keymap = self.keymap.lock().unwrap();
                    for i in 0..size.min(28) {
                        out[4 + i] = keymap.get(offset + i).copied().unwrap_or(0);
                    }
                }
                protocol::CUSTOM_GET_VALUE | protocol::CUSTOM_SET_VALUE => {
                    let get = r[0] == protocol::CUSTOM_GET_VALUE;
                    if self.custom_scheme() {
                        let mut values = self.custom.lock().unwrap();
                        match (get, values.get(&(r[1], r[2])).copied()) {
                            (_, None) => return unhandled(out),
                            (true, Some(v)) => out[3..5].copy_from_slice(&v),
                            (false, Some(_)) => {
                                values.insert((r[1], r[2]), [r[3], r[4]]);
                            }
                        }
                    } else {
                        let mut values = self.legacy.lock().unwrap();
                        match (get, values.get(&r[1]).copied()) {
                            (_, None) => return unhandled(out),
                            (true, Some(v)) => out[2..4].copy_from_slice(&v),
                            (false, Some(_)) => {
                                values.insert(r[1], [r[2], r[3]]);
                            }
                        }
                    }
                }
                _ => return unhandled(out),
            }
            Ok(out)
        }
    }

    pub fn leak(f: FakeQmk) -> &'static FakeQmk {
        Box::leak(Box::new(f))
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{leak, FakeQmk};
    use super::*;

    fn modern() -> &'static FakeQmk {
        let f = leak(FakeQmk {
            protocol_version: 12,
            layers: 4,
            layout_options: Some(0x0000_0005),
            ..FakeQmk::default()
        });
        let mut c = f.custom.lock().unwrap();
        c.insert((3, 1), [200, 0]);
        c.insert((3, 2), [7, 0]);
        c.insert((3, 3), [128, 0]);
        c.insert((3, 4), [85, 255]);
        c.insert((1, 1), [64, 0]);
        c.insert((1, 2), [1, 0]);
        drop(c);
        f
    }

    #[test]
    fn probes_a_modern_board_through_custom_channels() {
        let f = modern();
        let link = Guarded::read_only(f);
        let p = Client(&link).probe(|_| Some(Scheme::Custom)).unwrap();
        assert_eq!(
            (p.protocol_version, p.layers, p.layout_options),
            (12, 4, Some(5))
        );
        assert_eq!(
            p.lighting,
            vec![
                ChannelState {
                    channel: Channel::RgbMatrix,
                    brightness: 200,
                    effect: 7,
                    speed: Some(128),
                    color: Some((85, 255)),
                },
                ChannelState {
                    channel: Channel::Backlight,
                    brightness: 64,
                    effect: 1,
                    speed: None,
                    color: None,
                },
            ]
        );
        let sent = f.sent.lock().unwrap();
        assert!(sent.iter().all(|r| !protocol::is_write(r)), "only reads");
        assert!(
            !sent
                .iter()
                .any(|r| r[..2] == [protocol::GET_KEYBOARD_VALUE, 3]),
            "the switch matrix is never read"
        );
    }

    #[test]
    fn probes_a_legacy_board_through_fixed_ids() {
        let f = leak(FakeQmk {
            protocol_version: 9,
            layers: 4,
            ..FakeQmk::default()
        });
        f.legacy.lock().unwrap().extend([
            (0x80, [255, 0]),
            (0x81, [2, 0]),
            (0x82, [3, 0]),
            (0x83, [10, 20]),
        ]);
        let link = Guarded::read_only(f);
        let p = Client(&link).probe(|_| Some(Scheme::Legacy)).unwrap();
        assert_eq!(
            p.layout_options, None,
            "unanswered layout options are optional"
        );
        assert_eq!(
            p.lighting,
            vec![ChannelState {
                channel: Channel::RgbLight,
                brightness: 255,
                effect: 2,
                speed: Some(3),
                color: Some((10, 20)),
            }]
        );
        assert!(f
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|r| r[..2] == [protocol::CUSTOM_GET_VALUE, 0x80]));
    }

    #[test]
    fn no_scheme_reads_no_lighting() {
        let f = modern();
        let link = Guarded::read_only(f);
        let p = Client(&link).probe(|_| None).unwrap();
        assert!(p.lighting.is_empty());
        assert!(!f
            .sent
            .lock()
            .unwrap()
            .iter()
            .any(|r| r[0] == protocol::CUSTOM_GET_VALUE));
    }

    #[test]
    fn a_board_that_doesnt_answer_via_fails_the_probe() {
        struct Silent;
        impl Transport for Silent {
            fn exchange(&self, _: &protocol::Report) -> Result<protocol::Report> {
                Err(DeviceError::Timeout)
            }
        }
        let link = Guarded::read_only(Silent);
        assert_eq!(
            Client(&link).probe(|_| Some(Scheme::Custom)),
            Err(DeviceError::Timeout)
        );
    }

    #[test]
    fn silence_on_optional_reads_still_opens_after_one_timeout() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        /// Answers the protocol version and layer count, nothing else.
        struct Terse(AtomicUsize);
        impl Transport for &'static Terse {
            fn exchange(&self, r: &protocol::Report) -> Result<protocol::Report> {
                match r[0] {
                    protocol::GET_PROTOCOL_VERSION | protocol::DYNAMIC_KEYMAP_GET_LAYER_COUNT => {
                        let mut out = *r;
                        out[1..3].copy_from_slice(&[0, 12]);
                        Ok(out)
                    }
                    _ => {
                        self.0.fetch_add(1, Ordering::Relaxed);
                        Err(DeviceError::Timeout)
                    }
                }
            }
        }
        let t: &'static Terse = Box::leak(Box::new(Terse(AtomicUsize::new(0))));
        let link = Guarded::read_only(t);
        let p = Client(&link).probe(|_| Some(Scheme::Custom)).unwrap();
        assert_eq!((p.layout_options, p.lighting.len()), (None, 0));
        assert_eq!(
            t.0.load(Ordering::Relaxed),
            2,
            "layout options, then one lighting read"
        );
    }

    #[test]
    fn reads_the_whole_keymap_through_the_buffer() {
        let f = modern();
        // 2 layers × 3 rows × 7 cols = 42 keycodes = 84 bytes: three full pieces of 28.
        let codes: Vec<u16> = (0..42).map(|i| 0x0100 + i).collect();
        *f.keymap.lock().unwrap() = codes.iter().flat_map(|c| c.to_be_bytes()).collect();
        let link = Guarded::read_only(f);
        assert_eq!(Client(&link).keymap(2, 3, 7).unwrap(), codes);
        let reads: Vec<(u16, u8)> = f
            .sent
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[0] == protocol::DYNAMIC_KEYMAP_GET_BUFFER)
            .map(|r| (u16::from_be_bytes([r[1], r[2]]), r[3]))
            .collect();
        assert_eq!(reads, vec![(0, 28), (28, 28), (56, 28)]);
        // An odd remainder: 1 × 1 × 15 = 30 bytes = 28 + 2.
        assert_eq!(Client(&link).keymap(1, 1, 15).unwrap(), codes[..15]);
    }

    #[test]
    fn a_misplaced_buffer_reply_is_an_error() {
        struct Shifted;
        impl Transport for Shifted {
            fn exchange(&self, r: &protocol::Report) -> Result<protocol::Report> {
                let mut out = *r;
                out[2] = out[2].wrapping_add(1);
                Ok(out)
            }
        }
        let link = Guarded::read_only(Shifted);
        assert!(matches!(
            Client(&link).keymap(1, 1, 4),
            Err(DeviceError::Backend(_))
        ));
    }
}
