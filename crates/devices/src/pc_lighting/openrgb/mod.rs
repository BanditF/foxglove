//! OpenRGB devices as Foxglove lights (#471): Foxglove as a client of a running OpenRGB's SDK
//! server, so every controller OpenRGB drives (RAM, GPUs, motherboards, fans, peripherals)
//! is one more light source, with the same zones, calibration and driver trait as the rest.
//!
//! - [`wire`]: the protocol's packets and the controller data block, from OpenRGB's public
//!   protocol document (clean-room; OpenRGB is GPL-2.0)
//! - [`client`]: one TCP connection, bounded waits, server pushes as [`Event`]s
//! - here: the opt-in [`Settings`], each controller as a [`Source`] (its zones and caps), and
//!   [`OpenRgbDevice`], its [`LightingDevice`] driver
//!
//! Off by default: nothing connects unless the user turned it on ([`Settings::enabled`]).
//! The host connects on events (start-up, the setting turned on, OpenRGB's own push), never
//! on a timer; with no server there is no socket, and the feature stays hidden. Reconnecting
//! after OpenRGB goes is the user's choice too ([`Settings::reconnect`]), and then only on a
//! bounded [`Backoff`].

pub mod client;
pub mod wire;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use client::{Client, Event};
pub use wire::Controller;

use super::caps::{Color, LightCaps};
use super::zones::{Zone, ZoneSource, Zones};
use super::{calibrated, Control, DeviceKind, Found, LightingDevice, ZoneRole, ZoneSpec};
use crate::{DeviceError, Result, Rgb};

/// How long a connect or an answer may take before it counts as failed. OpenRGB on the same
/// PC answers in milliseconds.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// The name OpenRGB lists this client under.
pub const CLIENT_NAME: &str = "Foxglove";
/// The most LEDs that get a zone each (zone ids are one byte). A larger controller gets one
/// zone per OpenRGB zone instead.
pub const MAX_LEDS: usize = 256;
/// Every OpenRGB source's key starts with this, so it never meets a HID device's.
pub const KEY_PREFIX: &str = "openrgb:";

/// The user's OpenRGB settings. Everything is off until the user turns it on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Connect to OpenRGB at all.
    pub enabled: bool,
    /// The server's address: an IP address, or `localhost`. No name lookups (unbounded).
    pub host: String,
    pub port: u16,
    /// Try again, on a bounded [`Backoff`], when OpenRGB goes away or isn't there yet.
    pub reconnect: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            host: "127.0.0.1".into(),
            port: wire::DEFAULT_PORT,
            reconnect: false,
        }
    }
}

impl Settings {
    /// The address to connect to; `None` when the host isn't an IP address or `localhost`.
    pub fn addr(&self) -> Option<SocketAddr> {
        let ip = match self.host.trim() {
            h if h.eq_ignore_ascii_case("localhost") => IpAddr::V4(Ipv4Addr::LOCALHOST),
            h => h.parse().ok()?,
        };
        Some(SocketAddr::new(ip, self.port))
    }

    /// The waits between reconnect tries, when the user turned reconnecting on; `None` means
    /// don't try again (wait for the next event).
    pub fn backoff(&self) -> Option<Backoff> {
        (self.enabled && self.reconnect).then(Backoff::default)
    }
}

/// Reconnect waits: 1 s, doubling to at most 30 s, then no more after [`Backoff::TRIES`].
/// A connect that works starts a fresh one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backoff {
    next: Duration,
    tries: u32,
}

impl Backoff {
    pub const FIRST: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(30);
    pub const TRIES: u32 = 8;
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            next: Self::FIRST,
            tries: 0,
        }
    }
}

impl Iterator for Backoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Duration> {
        if self.tries >= Self::TRIES {
            return None;
        }
        self.tries += 1;
        let wait = self.next;
        self.next = (self.next * 2).min(Self::MAX);
        Some(wait)
    }
}

/// Connects when the user turned OpenRGB on: `Ok(None)` when it's off (nothing is opened).
/// `events` gets the server's pushes and the connection's end.
pub fn connect(
    settings: &Settings,
    events: impl FnMut(Event) + Send + 'static,
) -> Result<Option<Arc<Client>>> {
    if !settings.enabled {
        return Ok(None);
    }
    let addr = settings
        .addr()
        .ok_or_else(|| DeviceError::Backend(format!("OpenRGB: bad host {:?}", settings.host)))?;
    Client::connect(addr, CLIENT_NAME, TIMEOUT, events).map(|c| Some(Arc::new(c)))
}

/// One OpenRGB controller as a Foxglove light source.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// Its index in the server's list, valid for [`Self::generation`] only.
    pub index: u32,
    /// The client's list generation it was read at.
    pub generation: u64,
    /// Stable across restarts while the device keeps its serial (or its place): see
    /// [`source_key`].
    pub key: String,
    pub name: String,
    pub zones: Vec<ZoneSpec>,
    pub caps: LightCaps,
    pub kind: DeviceKind,
    /// The controller as the server described it.
    pub controller: Controller,
}

/// Lists every controller as a [`Source`]. A list change while reading is a `Busy` error: the
/// push that announced it is the host's cue to list again.
pub fn list(client: &Client) -> Result<Vec<Source>> {
    let generation = client.generation();
    let controllers = client.controllers()?;
    if client.generation() != generation {
        return Err(DeviceError::Busy("OpenRGB's device list changed".into()));
    }
    Ok(sources(&controllers, generation))
}

/// Each controller as a [`Source`], in index order; keys made unique.
pub fn sources(controllers: &[Controller], generation: u64) -> Vec<Source> {
    let mut out: Vec<Source> = Vec::with_capacity(controllers.len());
    for (i, c) in controllers.iter().enumerate() {
        let mut key = source_key(c, i);
        let base = key.clone();
        let mut n = 2;
        while out.iter().any(|s| s.key == key) {
            key = format!("{base}#{n}");
            n += 1;
        }
        let zones = zones(c);
        out.push(Source {
            index: i as u32,
            generation,
            key,
            name: match c.name.trim() {
                "" => format!("OpenRGB device {}", i + 1),
                name => name.to_string(),
            },
            caps: caps(c, &zones),
            zones,
            // OpenRGB's device types aren't in its protocol document; until they are mapped
            // (in data), every source is "other".
            kind: DeviceKind::Other,
            controller: c.clone(),
        });
    }
    out
}

/// The source as a found light ([`Control::OpenRgb`]), for the host's device list beside the
/// HID ones. Its path names the list generation and index, so a list change (which may move
/// indices) reopens its driver. No VID/PID: OpenRGB's devices aren't the catalog's.
pub fn found(source: &Source) -> Found {
    Found {
        key: source.key.clone(),
        name: source.name.clone(),
        vid: 0,
        pid: 0,
        control: Control::OpenRgb,
        path: format!("{KEY_PREFIX}{}:{}", source.generation, source.index),
        zones: source.zones.clone(),
        caps: source.caps.clone(),
        local: false,
        calibration: None,
        size: None,
        kind: source.kind,
        conflicts: Vec::new(),
    }
}

/// `openrgb:<name>:<serial>`, else `:<location>` (its port or bus address), else the index.
pub fn source_key(c: &Controller, index: usize) -> String {
    let which = [c.serial.trim(), c.location.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .map_or_else(|| format!("#{index}"), str::to_string);
    format!("{KEY_PREFIX}{}:{which}", c.name.trim())
}

/// The controller's zones: one per LED (named by its LED's name) up to [`MAX_LEDS`], else one
/// per OpenRGB zone that has LEDs (named by the zone).
pub fn zones(c: &Controller) -> Vec<ZoneSpec> {
    let spec = |i: usize, name: String| ZoneSpec {
        id: i as u8,
        name,
        position: None,
        role: ZoneRole::Point,
    };
    if c.leds.len() <= MAX_LEDS {
        return c
            .leds
            .iter()
            .enumerate()
            .map(|(i, led)| match led.name.trim() {
                "" => spec(i, format!("LED {}", i + 1)),
                name => spec(i, name.to_string()),
            })
            .collect();
    }
    led_runs(c)
        .into_iter()
        .filter(|(_, run)| !run.is_empty())
        .take(MAX_LEDS)
        .enumerate()
        .map(|(i, (zone, _))| match c.zones[zone].name.trim() {
            "" => spec(i, format!("Zone {}", zone + 1)),
            name => spec(i, name.to_string()),
        })
        .collect()
}

/// Each OpenRGB zone's LEDs, as a range of the controller's LED list (zones follow each other;
/// a count past the list's end is cut at it).
fn led_runs(c: &Controller) -> Vec<(usize, Range<usize>)> {
    let mut at = 0;
    c.zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            let start = at.min(c.leds.len());
            at = at.saturating_add(z.leds_count as usize);
            (i, start..at.min(c.leds.len()))
        })
        .collect()
}

/// For each Foxglove zone (in [`zones`]' order), the LEDs it lights.
fn zone_leds(c: &Controller) -> Vec<Range<usize>> {
    if c.leds.len() <= MAX_LEDS {
        return (0..c.leds.len()).map(|i| i..i + 1).collect();
    }
    led_runs(c)
        .into_iter()
        .map(|(_, run)| run)
        .filter(|run| !run.is_empty())
        .take(MAX_LEDS)
        .collect()
}

fn caps(c: &Controller, zones: &[ZoneSpec]) -> LightCaps {
    let color = match (c.leds.len(), c.leds.len() <= MAX_LEDS) {
        (0, _) => Color::None,
        (1, _) => Color::Single,
        (_, true) => Color::PerLed,
        (_, false) => Color::PerZone,
    };
    LightCaps {
        color,
        count: c.leds.len() as u32,
        effects: c.modes.iter().map(|m| m.name.clone()).collect(),
        zones: (!zones.is_empty()).then(|| Zones {
            list: zones
                .iter()
                .map(|z| Zone {
                    part: 0,
                    keys: Vec::new(),
                    area: Some(z.name.clone()),
                })
                .collect(),
            dead: vec![0],
            source: ZoneSource::Reported,
            reported: None,
        }),
        ..Default::default()
    }
}

/// Opens the driver for a source, wrapped in the calibration every driver gets.
pub fn open(client: Arc<Client>, source: &Source) -> Result<Box<dyn LightingDevice>> {
    let driver = OpenRgbDevice::new(client, source)?;
    Ok(Box::new(calibrated::Calibrated::new(
        Box::new(driver),
        source.key.clone(),
        &source.zones,
        None,
    )))
}

/// One OpenRGB controller as a [`LightingDevice`]. Its first write puts it in OpenRGB's custom
/// (direct) mode; each write then sends every LED (UPDATELEDS), or one OpenRGB zone's LEDs
/// when only that zone changes (UPDATEZONELEDS). It refuses to write once the server's list
/// has changed, since its index may then name another device.
pub struct OpenRgbDevice {
    client: Arc<Client>,
    index: u32,
    generation: u64,
    name: String,
    zones: Vec<String>,
    zone_leds: Vec<Range<usize>>,
    /// Each OpenRGB zone's LEDs, for UPDATEZONELEDS.
    runs: Vec<(usize, Range<usize>)>,
    /// Every LED's colour as last sent (the server's, before the first write).
    leds: Vec<Rgb>,
    colors: Vec<Option<Rgb>>,
    custom: bool,
}

impl OpenRgbDevice {
    pub fn new(client: Arc<Client>, source: &Source) -> Result<Self> {
        let c = &source.controller;
        let zone_leds = zone_leds(c);
        if zone_leds.is_empty() || zone_leds.len() != source.zones.len() {
            return Err(DeviceError::Unsupported);
        }
        let mut leds = c.colors.clone();
        leds.resize(c.leds.len(), super::BLACK);
        Ok(OpenRgbDevice {
            client,
            index: source.index,
            generation: source.generation,
            name: source.name.clone(),
            zones: source.zones.iter().map(|z| z.name.clone()).collect(),
            zone_leds,
            runs: led_runs(c),
            leds,
            colors: vec![None; source.zones.len()],
            custom: false,
        })
    }
}

impl LightingDevice for OpenRgbDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn zones(&self) -> Vec<String> {
        self.zones.clone()
    }

    fn colors(&self) -> Vec<Option<Rgb>> {
        self.colors.clone()
    }

    fn set_zones(&mut self, colors: &[(usize, Rgb)]) -> Result<()> {
        if colors.iter().any(|&(i, _)| i >= self.zones.len()) {
            return Err(DeviceError::Unsupported);
        }
        if colors.is_empty() {
            return Ok(());
        }
        if self.client.generation() != self.generation {
            return Err(DeviceError::Disconnected);
        }
        let mut leds = self.leds.clone();
        for &(i, c) in colors {
            leds[self.zone_leds[i].clone()].fill(c);
        }
        let changed: Vec<usize> = (0..leds.len())
            .filter(|&i| leds[i] != self.leds[i])
            .collect();
        if !self.custom {
            self.client.set_custom_mode(self.index)?;
            self.custom = true;
        }
        let one_zone = self.runs.iter().find(|(_, run)| {
            changed
                .first()
                .zip(changed.last())
                .is_some_and(|(a, b)| run.contains(a) && run.contains(b))
        });
        match one_zone {
            Some((zone, run)) if changed.len() < leds.len() => {
                self.client
                    .update_zone_leds(self.index, *zone as u32, &leds[run.clone()])?
            }
            _ => self.client.update_leds(self.index, &leds)?,
        }
        self.leds = leds;
        for &(i, c) in colors {
            self.colors[i] = Some(c);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::client::fake::{Answers, Build, Fake};
    use super::wire::fixture::{controller, ZoneFx};
    use super::wire::id;
    use super::*;

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const BLUE: Rgb = Rgb { r: 0, g: 0, b: 255 };

    fn b(f: fn(u32) -> Vec<u8>) -> Build {
        Box::new(f)
    }

    fn board(v: u32) -> Vec<u8> {
        controller(
            v,
            "Fixture RAM",
            "SN-7",
            &[
                ZoneFx {
                    name: "Stick 1",
                    leds: 2,
                    matrix: None,
                    segments: vec![],
                },
                ZoneFx {
                    name: "Stick 2",
                    leds: 2,
                    matrix: None,
                    segments: vec![],
                },
            ],
            |i| {
                if i == 3 {
                    String::new()
                } else {
                    format!("RAM LED {}", i + 1)
                }
            },
            [10, 20, 30],
        )
    }

    fn big(v: u32) -> Vec<u8> {
        let zone = |name| ZoneFx {
            name,
            leds: 150,
            matrix: None,
            segments: vec![],
        };
        controller(
            v,
            "Long strip",
            "",
            &[
                zone("Desk"),
                zone(""),
                ZoneFx {
                    leds: 0,
                    ..zone("Unused")
                },
            ],
            |_| "L".into(),
            [0; 3],
        )
    }

    #[test]
    fn it_is_off_until_the_user_turns_it_on() {
        let s = Settings::default();
        assert!(!s.enabled && !s.reconnect);
        assert_eq!(s.addr(), Some("127.0.0.1:6742".parse().unwrap()));
        assert!(
            matches!(connect(&s, |_| {}), Ok(None)),
            "off: no socket at all"
        );
        assert_eq!(s.backoff(), None);
        let parsed: Settings = toml::from_str("enabled = true").unwrap();
        assert_eq!((parsed.enabled, parsed.port), (true, 6742));
        assert_eq!(parsed.backoff(), None, "reconnecting is its own opt-in");
        let local = Settings {
            host: "localhost".into(),
            ..Default::default()
        };
        assert_eq!(local.addr(), Some("127.0.0.1:6742".parse().unwrap()));
        let named = Settings {
            host: "openrgb.example".into(),
            ..Default::default()
        };
        assert_eq!(named.addr(), None, "no name lookups");
    }

    #[test]
    fn reconnect_waits_are_bounded() {
        let s = Settings {
            enabled: true,
            reconnect: true,
            ..Default::default()
        };
        let waits: Vec<u64> = s.backoff().unwrap().map(|d| d.as_secs()).collect();
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn each_controller_is_a_source_with_a_zone_per_led() {
        let fake = Fake::start(Answers::Version(4), vec![b(board), b(board), b(big)]);
        let settings = Settings {
            enabled: true,
            port: fake.addr.port(),
            ..Default::default()
        };
        let client = connect(&settings, |_| {}).unwrap().unwrap();
        let sources = list(&client).unwrap();
        let s = &sources[0];
        assert_eq!(s.key, "openrgb:Fixture RAM:SN-7");
        assert_eq!(
            sources[1].key, "openrgb:Fixture RAM:SN-7#2",
            "two of a kind stay two"
        );
        assert_eq!(s.name, "Fixture RAM");
        assert_eq!(
            s.zones
                .iter()
                .map(|z| (z.id, z.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (0, "RAM LED 1"),
                (1, "RAM LED 2"),
                (2, "RAM LED 3"),
                (3, "LED 4")
            ]
        );
        assert_eq!((s.caps.color, s.caps.count), (Color::PerLed, 4));
        assert_eq!(s.caps.effects, vec!["Direct"]);
        assert_eq!(s.caps.zones.as_ref().unwrap().list.len(), 4);
        assert_eq!(s.kind, DeviceKind::Other);
        let f = found(s);
        assert_eq!(
            (f.key.as_str(), f.control, f.path.as_str(), f.zones.len()),
            (
                "openrgb:Fixture RAM:SN-7",
                Control::OpenRgb,
                "openrgb:0:0",
                4
            )
        );
        assert_eq!(found(&sources[1]).path, "openrgb:0:1");

        let long = &sources[2];
        assert_eq!(
            long.key, "openrgb:Long strip:HID: fixture",
            "no serial: its place"
        );
        assert_eq!(
            long.zones
                .iter()
                .map(|z| z.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Desk", "Zone 2"],
            "300 LEDs: a zone per OpenRGB zone, empty ones left out"
        );
        assert_eq!((long.caps.color, long.caps.count), (Color::PerZone, 300));
    }

    #[test]
    fn the_driver_goes_custom_once_and_sends_zone_or_whole_updates() {
        let fake = Fake::start(Answers::Version(4), vec![b(board), b(big)]);
        let client = Arc::new(Client::connect(fake.addr, CLIENT_NAME, TIMEOUT, |_| {}).unwrap());
        let sources = list(&client).unwrap();
        let mut d = open(Arc::clone(&client), &sources[0]).unwrap();
        assert_eq!(d.zones().len(), 4);
        d.set_zones(&[(1, RED)]).unwrap();
        d.set_all(BLUE).unwrap();
        d.set_zones(&[(2, RED), (3, RED)]).unwrap();
        let mut long = open(Arc::clone(&client), &sources[1]).unwrap();
        long.set_zones(&[(1, RED)]).unwrap();
        client.controller_count().unwrap(); // a round trip: the fake has read every write

        let custom = fake.sent(id::SETCUSTOMMODE);
        assert_eq!(
            custom.iter().map(|p| p.device).collect::<Vec<_>>(),
            vec![0, 1],
            "once each"
        );
        let zone = fake.sent(id::UPDATEZONELEDS);
        let (z, colors) = zone_body(&zone[0].payload);
        assert_eq!((zone[0].device, z), (0, 0), "only stick 1 changed");
        assert_eq!(
            colors,
            vec![[10, 20, 30], [255, 0, 0]],
            "the rest as the server had them"
        );
        let (z, colors) = zone_body(&zone[1].payload);
        assert_eq!(z, 1, "stick 2");
        assert_eq!(colors, vec![[255, 0, 0]; 2]);
        let (z, colors) = zone_body(&zone[2].payload);
        assert_eq!(
            (zone[2].device, z, colors.len()),
            (1, 1, 150),
            "the strip's second zone"
        );
        let all = fake.sent(id::UPDATELEDS);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].payload[4..6], [4, 0]);
        assert_eq!(
            d.colors(),
            vec![Some(BLUE), Some(BLUE), Some(RED), Some(RED)]
        );
        assert_eq!(d.set_zones(&[(4, RED)]), Err(DeviceError::Unsupported));
    }

    #[test]
    fn after_a_list_change_old_drivers_refuse_to_write() {
        let fake = Fake::start(Answers::Version(4), vec![b(board)]);
        let (tx, rx) = std::sync::mpsc::channel();
        let client = Arc::new(
            Client::connect(fake.addr, CLIENT_NAME, TIMEOUT, move |e| {
                let _ = tx.send(e);
            })
            .unwrap(),
        );
        let sources = list(&client).unwrap();
        let mut d = open(Arc::clone(&client), &sources[0]).unwrap();
        fake.push.send(Some(())).unwrap();
        assert_eq!(rx.recv_timeout(TIMEOUT), Ok(Event::DeviceListUpdated));
        assert_eq!(d.set_all(RED), Err(DeviceError::Disconnected));
        assert_eq!(d.colors(), vec![None; 4], "nothing was written");
        let again = list(&client).unwrap();
        assert_eq!(again[0].generation, 1);
        open(Arc::clone(&client), &again[0])
            .unwrap()
            .set_all(RED)
            .unwrap();
    }

    /// UPDATEZONELEDS' payload: zone index and colours.
    fn zone_body(p: &[u8]) -> (u32, Vec<[u8; 3]>) {
        let zone = u32::from_le_bytes(p[4..8].try_into().unwrap());
        let n = usize::from(u16::from_le_bytes([p[8], p[9]]));
        let colors = p[10..10 + 4 * n]
            .chunks(4)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        (zone, colors)
    }
}
