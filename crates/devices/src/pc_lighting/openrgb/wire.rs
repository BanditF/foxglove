//! The OpenRGB SDK wire format, written from OpenRGB's public protocol document
//! (`Documentation/OpenRGBSDK.md`): the packet header, the packets a client sends, and the
//! controller data block a server answers with. Clean-room: no OpenRGB code was read or copied
//! (OpenRGB is GPL-2.0; this crate is MIT).
//!
//! Two things the document leaves open, taken as every OpenRGB host and client does in
//! practice, and to be confirmed against a real server in Test lab:
//! - integers are little-endian;
//! - an `RGBColor` is four bytes: red, green, blue, then one unused byte.
//!
//! Strings in a block are a `u16` length that counts the trailing NUL, then the bytes and the
//! NUL. Payload sizes ahead of a block (`data_size`) are written as the whole payload's length,
//! themselves included; when reading, the header's size is what counts.

use crate::{DeviceError, Result, Rgb};

/// Every packet starts with these four bytes.
pub const MAGIC: [u8; 4] = *b"ORGB";
/// Magic, device index, packet id, payload size: four 4-byte fields.
pub const HEADER_LEN: usize = 16;
/// The port an OpenRGB server listens on unless the user changed it.
pub const DEFAULT_PORT: u16 = 6742;
/// The highest protocol version this client speaks: 4 (OpenRGB 0.9, zone segments). A server
/// that answers lower is spoken to at its version.
pub const CLIENT_PROTOCOL: u32 = 4;
/// The largest payload accepted, so a broken peer can't make us allocate without bound. A big
/// controller (a full keyboard with modes) is tens of kilobytes.
pub const MAX_PAYLOAD: u32 = 4 * 1024 * 1024;

/// Packet ids (the document's table; only the ones Foxglove uses).
pub mod id {
    pub const REQUEST_CONTROLLER_COUNT: u32 = 0;
    pub const REQUEST_CONTROLLER_DATA: u32 = 1;
    pub const REQUEST_PROTOCOL_VERSION: u32 = 40;
    pub const SET_CLIENT_NAME: u32 = 50;
    /// Server push, no payload: the controller list changed, so indices may now mean other
    /// devices.
    pub const DEVICE_LIST_UPDATED: u32 = 100;
    pub const UPDATELEDS: u32 = 1050;
    pub const UPDATEZONELEDS: u32 = 1051;
    pub const SETCUSTOMMODE: u32 = 1100;
}

/// A packet header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The controller's index in the server's list (protocol 0 to 5).
    pub device: u32,
    pub id: u32,
    /// The payload's length in bytes.
    pub size: u32,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0; HEADER_LEN];
        out[..4].copy_from_slice(&MAGIC);
        out[4..8].copy_from_slice(&self.device.to_le_bytes());
        out[8..12].copy_from_slice(&self.id.to_le_bytes());
        out[12..].copy_from_slice(&self.size.to_le_bytes());
        out
    }

    /// Reads a header; refuses a wrong magic or a payload over [`MAX_PAYLOAD`].
    pub fn decode(raw: &[u8; HEADER_LEN]) -> Result<Header> {
        if raw[..4] != MAGIC {
            return Err(bad("not an OpenRGB packet (magic)"));
        }
        let word = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        let header = Header {
            device: word(4),
            id: word(8),
            size: word(12),
        };
        if header.size > MAX_PAYLOAD {
            return Err(bad("packet too large"));
        }
        Ok(header)
    }
}

/// A whole packet as read from the socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub device: u32,
    pub id: u32,
    pub payload: Vec<u8>,
}

/// A packet's bytes: header and payload.
pub fn packet(device: u32, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Header {
        device,
        id,
        size: payload.len() as u32,
    }
    .encode()
    .to_vec();
    out.extend_from_slice(payload);
    out
}

/// REQUEST_PROTOCOL_VERSION: the client's highest version. The server answers with its own.
pub fn protocol_version_request(version: u32) -> Vec<u8> {
    packet(0, id::REQUEST_PROTOCOL_VERSION, &version.to_le_bytes())
}

/// SET_CLIENT_NAME: the name OpenRGB lists this client under, NUL-terminated (an inner NUL
/// would end it early, so those are dropped).
pub fn client_name(name: &str) -> Vec<u8> {
    let mut payload: Vec<u8> = name.bytes().filter(|b| *b != 0).collect();
    payload.push(0);
    packet(0, id::SET_CLIENT_NAME, &payload)
}

/// REQUEST_CONTROLLER_COUNT.
pub fn controller_count_request() -> Vec<u8> {
    packet(0, id::REQUEST_CONTROLLER_COUNT, &[])
}

/// REQUEST_CONTROLLER_DATA for one controller, at the negotiated version (an empty payload
/// at version 0).
pub fn controller_data_request(device: u32, version: u32) -> Vec<u8> {
    if version == 0 {
        packet(device, id::REQUEST_CONTROLLER_DATA, &[])
    } else {
        packet(device, id::REQUEST_CONTROLLER_DATA, &version.to_le_bytes())
    }
}

/// RGBCONTROLLER_UPDATELEDS: one colour per LED of the controller, in its LED order.
pub fn update_leds(device: u32, colors: &[Rgb]) -> Result<Vec<u8>> {
    let mut body = Vec::with_capacity(6 + 4 * colors.len());
    put_colors(&mut body, colors)?;
    Ok(packet(device, id::UPDATELEDS, &sized(body)))
}

/// RGBCONTROLLER_UPDATEZONELEDS: one colour per LED of one of the controller's zones.
pub fn update_zone_leds(device: u32, zone: u32, colors: &[Rgb]) -> Result<Vec<u8>> {
    let mut body = Vec::with_capacity(10 + 4 * colors.len());
    body.extend_from_slice(&zone.to_le_bytes());
    put_colors(&mut body, colors)?;
    Ok(packet(device, id::UPDATEZONELEDS, &sized(body)))
}

/// RGBCONTROLLER_SETCUSTOMMODE: puts the controller in the mode that shows the colours sent
/// to it (direct or custom), no payload.
pub fn set_custom_mode(device: u32) -> Vec<u8> {
    packet(device, id::SETCUSTOMMODE, &[])
}

/// Prefixes `body` with `data_size`: the whole payload's length, itself included.
fn sized(body: Vec<u8>) -> Vec<u8> {
    let mut out = ((body.len() + 4) as u32).to_le_bytes().to_vec();
    out.extend(body);
    out
}

fn put_colors(out: &mut Vec<u8>, colors: &[Rgb]) -> Result<()> {
    let n = u16::try_from(colors.len()).map_err(|_| bad("more than 65535 colours"))?;
    out.extend_from_slice(&n.to_le_bytes());
    for c in colors {
        out.extend_from_slice(&[c.r, c.g, c.b, 0]);
    }
    Ok(())
}

fn bad(what: &str) -> DeviceError {
    DeviceError::Backend(format!("OpenRGB: {what}"))
}

/// One controller as the server describes it (REQUEST_CONTROLLER_DATA).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Controller {
    /// OpenRGB's device type number, kept as given (the document doesn't list the values).
    pub kind: i32,
    pub name: String,
    /// Empty before protocol 1.
    pub vendor: String,
    pub description: String,
    pub version: String,
    pub serial: String,
    pub location: String,
    pub active_mode: i32,
    pub modes: Vec<Mode>,
    pub zones: Vec<ControllerZone>,
    /// Every LED, zone after zone.
    pub leds: Vec<Led>,
    /// Each LED's colour now, in LED order.
    pub colors: Vec<Rgb>,
}

/// One of a controller's modes (effects).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub name: String,
    /// The server's own number for it, sent back when changing modes.
    pub value: i32,
    pub flags: u32,
    pub color_mode: u32,
    pub colors: Vec<Rgb>,
}

/// One of a controller's zones: a run of its LEDs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControllerZone {
    pub name: String,
    pub kind: i32,
    pub leds_min: u32,
    pub leds_max: u32,
    pub leds_count: u32,
    /// Where its LEDs sit in a grid, when it has one.
    pub matrix: Option<Matrix>,
    /// Named parts of the zone (protocol 4).
    pub segments: Vec<Segment>,
}

/// A zone's grid: `map[row * width + col]` is an LED index within the zone, or `u32::MAX`
/// where there is none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matrix {
    pub height: u32,
    pub width: u32,
    pub map: Vec<u32>,
}

/// A named run of LEDs within a zone (protocol 4).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub kind: i32,
    pub start: u32,
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Led {
    pub name: String,
    pub value: u32,
}

/// Reads a REQUEST_CONTROLLER_DATA answer (its `data_size`, then the block) at the negotiated
/// `version`. Every read is bounds-checked; a short or malformed block is an error, not a panic.
pub fn parse_controller(payload: &[u8], version: u32) -> Result<Controller> {
    let mut r = Reader {
        buf: payload,
        at: 0,
    };
    let _data_size = r.u32()?;
    let kind = r.i32()?;
    let name = r.string()?;
    let vendor = if version >= 1 {
        r.string()?
    } else {
        String::new()
    };
    let description = r.string()?;
    let fw_version = r.string()?;
    let serial = r.string()?;
    let location = r.string()?;
    let num_modes = r.u16()?;
    let active_mode = r.i32()?;
    let modes = (0..num_modes)
        .map(|_| r.mode(version))
        .collect::<Result<_>>()?;
    let num_zones = r.u16()?;
    let zones = (0..num_zones)
        .map(|_| r.zone(version))
        .collect::<Result<_>>()?;
    let num_leds = r.u16()?;
    let leds = (0..num_leds)
        .map(|_| {
            Ok(Led {
                name: r.string()?,
                value: r.u32()?,
            })
        })
        .collect::<Result<_>>()?;
    let colors = r.colors()?;
    Ok(Controller {
        kind,
        name,
        vendor,
        description,
        version: fw_version,
        serial,
        location,
        active_mode,
        modes,
        zones,
        leds,
        colors,
    })
}

/// A bounds-checked little-endian cursor.
struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.buf.len())
            .ok_or_else(|| bad("controller data cut short"))?;
        let out = &self.buf[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    /// A `u16` length counting the NUL, then the bytes. Text past a NUL is dropped; invalid
    /// UTF-8 is replaced, never refused (names come from device firmware).
    fn string(&mut self) -> Result<String> {
        let n = usize::from(self.u16()?);
        let raw = self.take(n)?;
        let text = raw.split(|b| *b == 0).next().unwrap_or_default();
        Ok(String::from_utf8_lossy(text).into_owned())
    }

    fn colors(&mut self) -> Result<Vec<Rgb>> {
        let n = usize::from(self.u16()?);
        let raw = self.take(4 * n)?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| Rgb {
                r: c[0],
                g: c[1],
                b: c[2],
            })
            .collect())
    }

    fn mode(&mut self, version: u32) -> Result<Mode> {
        let name = self.string()?;
        let value = self.i32()?;
        let flags = self.u32()?;
        let _speed_min = self.u32()?;
        let _speed_max = self.u32()?;
        if version >= 3 {
            let _brightness_min = self.u32()?;
            let _brightness_max = self.u32()?;
        }
        let _colors_min = self.u32()?;
        let _colors_max = self.u32()?;
        let _speed = self.u32()?;
        if version >= 3 {
            let _brightness = self.u32()?;
        }
        let _direction = self.u32()?;
        let color_mode = self.u32()?;
        let colors = self.colors()?;
        Ok(Mode {
            name,
            value,
            flags,
            color_mode,
            colors,
        })
    }

    fn zone(&mut self, version: u32) -> Result<ControllerZone> {
        let name = self.string()?;
        let kind = self.i32()?;
        let leds_min = self.u32()?;
        let leds_max = self.u32()?;
        let leds_count = self.u32()?;
        // The map's length in bytes, its height and width words included; 0 for none.
        let matrix_len = usize::from(self.u16()?);
        let matrix = match matrix_len {
            0 => None,
            n if n < 8 || (n - 8) % 4 != 0 => return Err(bad("zone matrix length")),
            n => {
                let height = self.u32()?;
                let width = self.u32()?;
                let map = self
                    .take(n - 8)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes(*c))
                    .collect();
                Some(Matrix { height, width, map })
            }
        };
        let segments = if version >= 4 {
            let n = self.u16()?;
            (0..n)
                .map(|_| {
                    Ok(Segment {
                        name: self.string()?,
                        kind: self.i32()?,
                        start: self.u32()?,
                        count: self.u32()?,
                    })
                })
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        Ok(ControllerZone {
            name,
            kind,
            leds_min,
            leds_max,
            leds_count,
            matrix,
            segments,
        })
    }
}

/// Builds controller data blocks the way the document lays them out, for tests (here and in the
/// client's fake server). Written from the document's tables, not captured from OpenRGB.
#[cfg(test)]
pub(crate) mod fixture {
    #[derive(Default)]
    pub struct Blob(pub Vec<u8>);

    impl Blob {
        pub fn u16(mut self, v: u16) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn u32(mut self, v: u32) -> Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn str(mut self, s: &str) -> Self {
            self = self.u16(s.len() as u16 + 1);
            self.0.extend_from_slice(s.as_bytes());
            self.0.push(0);
            self
        }
        pub fn rgb(mut self, c: [u8; 3]) -> Self {
            self.0.extend_from_slice(&[c[0], c[1], c[2], 0]);
            self
        }
        pub fn raw(mut self, b: &[u8]) -> Self {
            self.0.extend_from_slice(b);
            self
        }
        /// Prefixes `data_size` (the payload's length, itself included).
        pub fn sized(self) -> Vec<u8> {
            let mut out = ((self.0.len() + 4) as u32).to_le_bytes().to_vec();
            out.extend(self.0);
            out
        }
    }

    /// A zone as [`ControllerZone`] would read back, without a matrix unless given.
    pub struct ZoneFx<'a> {
        pub name: &'a str,
        pub leds: u32,
        pub matrix: Option<(u32, u32, Vec<u32>)>,
        pub segments: Vec<(&'a str, u32, u32)>,
    }

    /// One mode, "Direct", with no colours of its own.
    fn mode(b: Blob, version: u32) -> Blob {
        let b = b.str("Direct").u32(0x18).u32(0x20).u32(0).u32(0);
        let b = if version >= 3 { b.u32(0).u32(100) } else { b };
        let b = b.u32(0).u32(0).u32(0);
        let b = if version >= 3 { b.u32(100) } else { b };
        b.u32(0).u32(1).u16(0)
    }

    /// A controller block at `version`: one mode, `zones`, LEDs named by `led`, all colours
    /// `color`.
    pub fn controller(
        version: u32,
        name: &str,
        serial: &str,
        zones: &[ZoneFx],
        led: impl Fn(usize) -> String,
        color: [u8; 3],
    ) -> Vec<u8> {
        let mut b = Blob::default().u32(4).str(name);
        if version >= 1 {
            b = b.str("Fixture Vendor");
        }
        b = b
            .str("A fixture controller")
            .str("1.0")
            .str(serial)
            .str("HID: fixture");
        b = mode(b.u16(1).u32(0), version);
        b = b.u16(zones.len() as u16);
        for z in zones {
            b = b.str(z.name).u32(1).u32(z.leds).u32(z.leds).u32(z.leds);
            match &z.matrix {
                None => b = b.u16(0),
                Some((h, w, map)) => {
                    b = b.u16(8 + 4 * map.len() as u16).u32(*h).u32(*w);
                    for v in map {
                        b = b.u32(*v);
                    }
                }
            }
            if version >= 4 {
                b = b.u16(z.segments.len() as u16);
                for (name, start, count) in &z.segments {
                    b = b.str(name).u32(0).u32(*start).u32(*count);
                }
            }
        }
        let total: u32 = zones.iter().map(|z| z.leds).sum();
        b = b.u16(total as u16);
        for i in 0..total as usize {
            b = b.str(&led(i)).u32(i as u32);
        }
        b = b.u16(total as u16);
        for _ in 0..total {
            b = b.rgb(color);
        }
        b.sized()
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const TEAL: Rgb = Rgb {
        r: 0,
        g: 0x80,
        b: 0x80,
    };

    #[test]
    fn the_header_is_magic_index_id_size_little_endian() {
        let bytes = packet(3, id::UPDATELEDS, &[9, 9]);
        assert_eq!(
            bytes,
            vec![b'O', b'R', b'G', b'B', 3, 0, 0, 0, 0x1A, 0x04, 0, 0, 2, 0, 0, 0, 9, 9]
        );
        let head: [u8; 16] = bytes[..16].try_into().unwrap();
        assert_eq!(
            Header::decode(&head).unwrap(),
            Header {
                device: 3,
                id: 1050,
                size: 2
            }
        );
        let mut wrong = head;
        wrong[0] = b'X';
        assert!(Header::decode(&wrong).is_err());
        let huge = Header {
            device: 0,
            id: 1,
            size: MAX_PAYLOAD + 1,
        }
        .encode();
        assert!(Header::decode(&huge).is_err(), "bounded payloads");
    }

    #[test]
    fn client_packets_follow_the_document() {
        assert_eq!(
            &protocol_version_request(4)[8..],
            &[40, 0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0]
        );
        let name = client_name("Fox\0glove");
        assert_eq!(&name[12..16], &[9, 0, 0, 0], "strlen + 1");
        assert_eq!(&name[16..], b"Foxglove\0");
        assert_eq!(controller_count_request().len(), 16);
        assert_eq!(controller_data_request(2, 0).len(), 16, "v0: no payload");
        assert_eq!(
            &controller_data_request(2, 3)[4..],
            &[2, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0]
        );
        assert_eq!(
            &set_custom_mode(5)[4..],
            &[5, 0, 0, 0, 0x4C, 0x04, 0, 0, 0, 0, 0, 0]
        );

        let leds = update_leds(1, &[RED, TEAL]).unwrap();
        assert_eq!(
            &leds[12..],
            &[14, 0, 0, 0, 14, 0, 0, 0, 2, 0, 255, 0, 0, 0, 0, 0x80, 0x80, 0]
        );
        let zone = update_zone_leds(1, 2, &[TEAL]).unwrap();
        assert_eq!(&zone[8..12], &[0x1B, 0x04, 0, 0]);
        assert_eq!(
            &zone[16..],
            &[14, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x80, 0x80, 0]
        );
        assert!(update_leds(0, &vec![RED; 70_000]).is_err());
    }

    fn strip() -> Vec<ZoneFx<'static>> {
        vec![
            ZoneFx {
                name: "Strip",
                leds: 3,
                matrix: None,
                segments: vec![("Left", 0, 2)],
            },
            ZoneFx {
                name: "Logo",
                leds: 1,
                matrix: Some((1, 2, vec![0, u32::MAX])),
                segments: vec![],
            },
        ]
    }

    #[test]
    fn controller_data_parses_at_each_version() {
        for version in 0..=CLIENT_PROTOCOL {
            let blob = controller(
                version,
                "Desk strip",
                "SN1",
                &strip(),
                |i| format!("LED {}", i + 1),
                [1, 2, 3],
            );
            let c = parse_controller(&blob, version).unwrap();
            assert_eq!(c.name, "Desk strip");
            assert_eq!(c.vendor, if version >= 1 { "Fixture Vendor" } else { "" });
            assert_eq!(c.serial, "SN1");
            assert_eq!(c.location, "HID: fixture");
            assert_eq!(c.modes.len(), 1);
            assert_eq!(c.modes[0].name, "Direct");
            assert_eq!(c.modes[0].flags, 0x20);
            assert_eq!(c.zones.len(), 2);
            assert_eq!(c.zones[0].leds_count, 3);
            let segments = if version >= 4 { 1 } else { 0 };
            assert_eq!(c.zones[0].segments.len(), segments, "v{version}");
            assert_eq!(
                c.zones[1].matrix,
                Some(Matrix {
                    height: 1,
                    width: 2,
                    map: vec![0, u32::MAX]
                })
            );
            assert_eq!(c.leds.len(), 4);
            assert_eq!(c.leds[3].name, "LED 4");
            assert_eq!(c.colors, vec![Rgb { r: 1, g: 2, b: 3 }; 4]);
        }
    }

    #[test]
    fn a_short_or_wrong_version_block_is_an_error_not_a_panic() {
        let blob = controller(4, "x", "", &strip(), |_| "L".into(), [0; 3]);
        for cut in 0..blob.len() {
            assert!(parse_controller(&blob[..cut], 4).is_err(), "cut at {cut}");
        }
        // Read at the wrong version, fields shift: whatever it reads, it never panics.
        for version in 0..CLIENT_PROTOCOL {
            let _ = parse_controller(&blob, version);
        }
        let bad_matrix = Blob::default()
            .u32(1)
            .str("m")
            .str("")
            .str("")
            .str("")
            .str("")
            .u16(0)
            .u32(0)
            .u16(1)
            .str("z")
            .u32(0)
            .u32(1)
            .u32(1)
            .u32(1)
            .u16(5)
            .raw(&[0; 5])
            .sized();
        assert!(parse_controller(&bad_matrix, 0).is_err());
    }

    #[test]
    fn names_stop_at_their_nul_and_bad_utf8_is_replaced() {
        let blob = Blob::default()
            .u16(6)
            .raw(b"ab\0cd\0")
            .u16(3)
            .raw(&[0xff, b'x', 0]);
        let mut r = Reader {
            buf: &blob.0,
            at: 0,
        };
        assert_eq!(r.string().unwrap(), "ab");
        assert_eq!(r.string().unwrap(), "\u{fffd}x");
    }
}
