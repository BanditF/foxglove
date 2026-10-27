//! Other devices' lighting (#423): the OMEN 45L case and other lights that aren't the
//! keyboard, through the device layer's `pc_lighting` discovery and drivers.
//!
//! One worker thread owns the drivers and blocks on its queue. It scans when the app starts,
//! when a HID interface arrives or goes (`device_watch`), and when the Lights page opens; it
//! writes only for a click on the page or a colour the case follows ("Match keyboard": the
//! keyboard's colour as Foxglove sets it, or the Windows accent). Nothing streams or polls.
//! The colours the user picks are saved in the store's `[pc_lighting]` and put back when the
//! board arrives (at startup too), unless OMEN Light Studio is running.
//!
//! Each device carries what it can do (`LightCaps`): from the catalog, or from a LampArray's own
//! attributes, read through Windows when one arrives. Boards the catalog doesn't know can be
//! added by hand: the page lists them, a zone test lights one zone only on a click and puts it
//! back, and the entry goes into the user's `lighting-catalog.local.toml`.
//!
//! Every driver writes through its device's colour calibration (`pc_lighting::open` wraps it);
//! the colours here, saved and shown, are the palette's, before calibration.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex, OnceLock};

use foxglove_devices::pc_lighting::{
    self as pcl, Catalog, CatalogDevice, Control, Found, HidWriter, LampArrayInfo, LightCaps,
    LightingDevice, Protocol, Unknown,
};
use foxglove_devices::{DeviceError, Rgb};
use serde::{Deserialize, Serialize};

/// The `[pc_lighting]` section: per device (by [`Found::key`]), what the user chose.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub devices: BTreeMap<String, Saved>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// Zone id (the board's number) to `#rrggbb`.
    pub colors: BTreeMap<String, String>,
    pub match_keyboard: bool,
}

/// One device as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub name: String,
    pub control: Control,
    /// Zone names and their colours as Foxglove last set (or saved) them.
    pub zones: Vec<(String, Option<[u8; 3]>)>,
    pub match_keyboard: bool,
    /// What the device can do.
    pub caps: LightCaps,
    /// The user's own catalog entry as TOML, to copy and share; `None` for bundled ones.
    pub entry: Option<String>,
    /// Each zone's id (the board's number), in the order of [`Self::zones`].
    pub zone_ids: Vec<String>,
    /// The model's default colour calibration, from the catalog.
    pub calibration: Option<foxglove_devices::calibration::Calibration>,
    /// The user calibrated this device.
    pub calibrated: bool,
    /// Its front in millimetres and each zone's position on it (in zone order), from the
    /// catalog, for scenes' room map (#483).
    pub size: Option<[u16; 2]>,
    pub zone_positions: Vec<Option<[u16; 2]>>,
    /// Each zone's role in a scene (point or ambient), in zone order, from the catalog.
    pub zone_roles: Vec<foxglove_devices::pc_lighting::ZoneRole>,
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub devices: Vec<Row>,
    /// OMEN Light Studio is running and may put its own colours back.
    pub light_studio: bool,
    /// The last action's result, for the status line.
    pub status: Option<String>,
    pub busy: bool,
    /// HID devices nothing recognises, for "Add a device".
    pub unknown: Vec<Unknown>,
    /// Where the user's own entries are saved.
    pub local_file: Option<String>,
}

/// A request to the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    Scan,
    /// Zone index (into the device's zones) and colour.
    Set {
        key: String,
        colors: Vec<(usize, [u8; 3])>,
    },
    Off(String),
    Identify(String),
    Match(String, bool),
    /// The colour "Match keyboard" follows changed.
    Follow([u8; 3]),
    /// Add a device: blink one zone of a board through a protocol, then put it back.
    TestZone {
        vid: u16,
        pid: u16,
        path: String,
        protocol: Protocol,
        zone: u8,
    },
    /// Add a device: save (or replace) the user's own catalog entry.
    SaveLocal(Box<CatalogDevice>),
    /// Remove the user's own entry for this VID/PID.
    RemoveLocal(u16, u16),
    /// Shows colours without saving them as the user's (the calibrator's steps, and putting
    /// the colours back after it). A burst for one device writes only the last.
    Show {
        key: String,
        colors: Vec<(usize, [u8; 3])>,
    },
    /// The device's calibration changed: its colours again, through the new calibration.
    Recalibrate(String),
    /// A scene takes the lights (#485): each board's colours now are kept for [`Msg::SceneEnd`].
    SceneStart,
    /// One scene frame for a board: every zone (index, colour); only changed zones are
    /// written. A burst for one board writes only the last; none while paused.
    SceneFrame {
        key: String,
        colors: Vec<(usize, [u8; 3])>,
    },
    /// The scene ended: each board gets back the colours from before it (or the ones the user
    /// picked meanwhile, or the keyboard's colour it follows).
    SceneEnd,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static QUEUE: OnceLock<Mutex<mpsc::Sender<Msg>>> = OnceLock::new();
static LAST_FRAME: Mutex<Option<u64>> = Mutex::new(None);
/// The settings window's context, to repaint it when the worker has news.
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// What the page shows now (empty before the first scan).
pub fn snapshot() -> Snapshot {
    SNAPSHOT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Queues a request; a no-op before [`start`].
pub fn send(msg: Msg) {
    if let Some(q) = QUEUE.get() {
        if matches!(
            msg,
            Msg::Set { .. } | Msg::Off(_) | Msg::Identify(_) | Msg::TestZone { .. }
        ) {
            if let Some(s) = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                s.busy = true;
            }
        }
        let _ = q.lock().unwrap_or_else(|e| e.into_inner()).send(msg);
    }
}

/// Called each frame the Lights page draws: the first frame after it wasn't shown scans again
/// (and checks for Light Studio), so the page is current whenever it's opened.
pub fn page_shown(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
    let frame = ctx.cumulative_frame_nr();
    let mut last = LAST_FRAME.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_none_or(|l| frame > l + 1) {
        send(Msg::Scan);
    }
    *last = Some(frame);
}

/// The keyboard's colour as Foxglove just set it (hue 0-359, saturation and brightness 0-100).
pub fn keyboard_color(hue: u16, sat: u8, brightness: u8) {
    send(Msg::Follow(hsv_rgb(hue, sat, brightness)));
}

/// An HSV colour (hue 0-359, saturation and value 0-100) as sRGB bytes.
pub fn hsv_rgb(hue: u16, sat: u8, value: u8) -> [u8; 3] {
    let h = f32::from(hue % 360) / 60.0;
    let s = f32::from(sat.min(100)) / 100.0;
    let v = f32::from(value.min(100)) / 100.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let byte = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(s, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn rgb(c: [u8; 3]) -> Rgb {
    Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

/// Opens a HID path for writing (the hidapi writer, or a recording one in tests).
pub type Writers = Box<dyn Fn(&str) -> foxglove_devices::Result<Box<dyn HidWriter>> + Send>;

/// Reads every LampArray's attributes (Windows), by VID/PID.
pub type LampArrays = Box<dyn FnMut() -> Vec<(u16, u16, LampArrayInfo)> + Send>;

/// The worker's state, apart from the OS: tests drive it with a mocked HID writer.
pub struct Worker {
    bundled: Catalog,
    local: Catalog,
    local_path: Option<PathBuf>,
    catalog: Catalog,
    writers: Writers,
    lamp_arrays: LampArrays,
    lamp_info: BTreeMap<(u16, u16), LampArrayInfo>,
    found: Vec<Found>,
    unknown: Vec<Unknown>,
    /// The catalog changed: scan again, without putting colours back.
    pub rescan: bool,
    drivers: BTreeMap<String, Box<dyn LightingDevice>>,
    pub settings: Settings,
    follow: Option<[u8; 3]>,
    light_studio: bool,
    status: Option<String>,
    /// Settings changed and need saving.
    pub dirty: bool,
    /// A scene holds the lights: each board's colours from before it, by key.
    scene: Option<BTreeMap<String, Vec<Option<Rgb>>>>,
    /// A scene write failed once already (logged once per scene).
    scene_error: bool,
}

impl Worker {
    pub fn new(
        bundled: Catalog,
        local_path: Option<PathBuf>,
        settings: Settings,
        writers: Writers,
        lamp_arrays: LampArrays,
    ) -> Self {
        let local = local_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| {
                Catalog::parse(&text).unwrap_or_else(|e| {
                    eprintln!("Foxglove: lighting-catalog.local.toml: {e}");
                    Catalog::default()
                })
            })
            .unwrap_or_default();
        Self {
            catalog: bundled.clone().with_local(&local),
            bundled,
            local,
            local_path,
            writers,
            lamp_arrays,
            lamp_info: BTreeMap::new(),
            found: Vec::new(),
            unknown: Vec::new(),
            rescan: false,
            drivers: BTreeMap::new(),
            settings,
            follow: None,
            light_studio: false,
            status: None,
            dirty: false,
            scene: None,
            scene_error: false,
        }
    }

    /// A new enumeration: opens drivers for arrivals and puts their saved colours back (unless
    /// Light Studio is running), and drops the ones that went.
    /// `restore`: false after a catalog change, which isn't a board arriving.
    pub fn scan(&mut self, hid: &[pcl::HidEntry], light_studio: bool, paused: bool, restore: bool) {
        self.light_studio = light_studio;
        let mut now = pcl::discover(&self.catalog, hid);
        self.unknown = pcl::unknown(&self.catalog, hid);
        // A LampArray not read yet: read them all once (Windows), then reuse.
        let unread = now.iter().any(|f| {
            f.control == Control::LampArray && !self.lamp_info.contains_key(&(f.vid, f.pid))
        });
        if unread {
            for (vid, pid, info) in (self.lamp_arrays)() {
                self.lamp_info.insert((vid, pid), info);
            }
        }
        for f in &mut now {
            if let (Control::LampArray, Some(info)) =
                (f.control, self.lamp_info.get(&(f.vid, f.pid)))
            {
                f.caps = LightCaps::from_lamp_array(*info);
            }
        }
        // Same device, same interface, same catalog entry: keep its driver.
        let same = |o: &Found, f: &Found| {
            o.key == f.key && o.path == f.path && o.zones == f.zones && o.control == f.control
        };
        self.drivers.retain(|key, _| {
            now.iter()
                .any(|f| &f.key == key && self.found.iter().any(|o| same(o, f)))
        });
        for f in &now {
            let arrived = !self.found.iter().any(|o| same(o, f));
            if !arrived || !matches!(f.control, Control::Driver(_)) {
                continue;
            }
            match (self.writers)(&f.path).and_then(|w| pcl::open(f, w)) {
                Ok(driver) => {
                    self.drivers.insert(f.key.clone(), driver);
                    if restore && !light_studio && !paused {
                        self.restore(f);
                    }
                }
                Err(e) => eprintln!("Foxglove: can't open {}: {e}", f.name),
            }
        }
        self.found = now;
    }

    /// The colour a board's zone shows now (by VID/PID and zone number), as sent: what
    /// Foxglove knows, else what the user saved for it, through the zone's calibration (the
    /// zone test writes it back raw).
    fn known_color(&self, vid: u16, pid: u16, zone: u8) -> Option<Rgb> {
        self.found
            .iter()
            .filter(|f| (f.vid, f.pid) == (vid, pid))
            .find_map(|f| {
                let i = f.zones.iter().position(|z| z.id == zone)?;
                let asked = self.drivers.get(&f.key)?.colors().get(i).copied().flatten();
                let asked = asked.or_else(|| self.saved_colors(f).get(i).copied().flatten())?;
                Some(foxglove_devices::calibration::global().rgb(
                    &f.key,
                    Some(&zone.to_string()),
                    f.calibration.as_ref(),
                    asked,
                ))
            })
    }

    /// The colours the user chose for a device, per zone: the followed colour while it matches
    /// the keyboard, else the saved ones (`None` where nothing was saved).
    fn saved_colors(&self, f: &Found) -> Vec<Option<Rgb>> {
        let saved = self.settings.devices.get(&f.key);
        if let (Some(true), Some(c)) = (saved.map(|s| s.match_keyboard), self.follow) {
            return vec![Some(rgb(c)); f.zones.len()];
        }
        f.zones
            .iter()
            .map(|z| {
                let hex = saved?.colors.get(&z.id.to_string())?;
                parse_hex(hex).map(rgb)
            })
            .collect()
    }

    /// Writes the user's own catalog and rebuilds the merged one.
    fn save_local(&mut self) {
        self.catalog = self.bundled.clone().with_local(&self.local);
        self.rescan = true;
        let Some(path) = &self.local_path else {
            return;
        };
        let tmp = path.with_extension("toml.tmp");
        let result =
            std::fs::write(&tmp, self.local.to_toml()).and_then(|()| std::fs::rename(&tmp, path));
        if let Err(e) = result {
            self.status = Some(format!("Can't save {}: {e}", path.display()));
        }
    }

    /// Saved colours (or the followed colour) back on a board that just arrived.
    fn restore(&mut self, f: &Found) {
        let colors: Vec<(usize, Rgb)> = self
            .saved_colors(f)
            .into_iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i, c?)))
            .collect();
        if colors.is_empty() {
            return;
        }
        if let Some(d) = self.drivers.get_mut(&f.key) {
            if let Err(e) = d.set_zones(&colors) {
                eprintln!("Foxglove: {}: {e}", f.name);
            }
        }
    }

    /// Handles one request. `paused` refuses writes (#404); `wait` sleeps between identify
    /// steps.
    pub fn handle(&mut self, msg: Msg, paused: bool, wait: &mut dyn FnMut(std::time::Duration)) {
        // Scene frames stop quietly while paused; the scene's end gives the colours back even
        // then, as leases do (#404).
        if paused && matches!(msg, Msg::SceneFrame { .. }) {
            return;
        }
        let writes = !matches!(
            msg,
            Msg::Scan
                | Msg::Follow(_)
                | Msg::Match(_, false)
                | Msg::SaveLocal(_)
                | Msg::RemoveLocal(..)
                | Msg::SceneStart
                | Msg::SceneEnd
        );
        if paused && writes {
            self.status = Some(super::pause::REFUSED.replace("the keyboard", "the lights"));
            return;
        }
        match msg {
            Msg::Scan => {}
            Msg::Set { key, colors } => {
                let colors: Vec<_> = colors.into_iter().map(|(i, c)| (i, rgb(c))).collect();
                self.scene_keeps(&key, &colors);
                self.write(&key, "Colours set", |d| d.set_zones(&colors), true);
            }
            Msg::Off(key) => {
                let zones = self.drivers.get(&key).map_or(0, |d| d.zones().len());
                let dark: Vec<_> = (0..zones).map(|i| (i, pcl::BLACK)).collect();
                self.scene_keeps(&key, &dark);
                self.write(&key, "Lights off", |d| d.off(), true);
            }
            Msg::SceneStart => {
                if self.scene.is_none() {
                    let before = self
                        .drivers
                        .iter()
                        .map(|(k, d)| (k.clone(), d.colors()))
                        .collect();
                    self.scene = Some(before);
                    self.scene_error = false;
                }
            }
            Msg::SceneFrame { key, colors } => self.scene_frame(&key, &colors),
            Msg::SceneEnd => {
                self.scene_end();
                scene_ended();
            }
            Msg::Show { key, colors } => {
                let colors: Vec<_> = colors.into_iter().map(|(i, c)| (i, rgb(c))).collect();
                self.write(&key, "Showing", |d| d.set_zones(&colors), false);
            }
            Msg::Recalibrate(key) => {
                // The palette again: known colours, else saved ones, else white (never dark).
                let Some(d) = self.drivers.get(&key) else {
                    return;
                };
                let saved = self
                    .found
                    .iter()
                    .find(|f| f.key == key)
                    .map(|f| self.saved_colors(f))
                    .unwrap_or_default();
                let colors = pcl::restore_colors(&d.colors(), &saved);
                self.write(&key, "Calibration applied", |d| d.set_zones(&colors), false);
            }
            Msg::Identify(key) => {
                // A write-only board Foxglove hasn't set yet goes back to the saved colours.
                let fallback = self
                    .found
                    .iter()
                    .find(|f| f.key == key)
                    .map(|f| self.saved_colors(f))
                    .unwrap_or_default();
                self.write(
                    &key,
                    "Identified",
                    |d| d.identify_restoring(&fallback, wait),
                    false,
                );
            }
            Msg::Match(key, on) => {
                self.settings
                    .devices
                    .entry(key.clone())
                    .or_default()
                    .match_keyboard = on;
                self.dirty = true;
                // A scene holds the lights: its end puts the followed colour on.
                if let (true, Some(c), None) = (on, self.follow, &self.scene) {
                    self.write(&key, "Matching the keyboard", |d| d.set_all(rgb(c)), false);
                }
            }
            Msg::Follow(c) => {
                // Paused: not taken, so the same colour applies once Foxglove resumes.
                if paused || self.follow == Some(c) {
                    return;
                }
                self.follow = Some(c);
                if self.scene.is_some() {
                    return;
                }
                let keys: Vec<String> = self
                    .settings
                    .devices
                    .iter()
                    .filter(|(k, s)| s.match_keyboard && self.drivers.contains_key(*k))
                    .map(|(k, _)| k.clone())
                    .collect();
                for key in keys {
                    self.write(&key, "Matching the keyboard", |d| d.set_all(rgb(c)), false);
                }
            }
            Msg::TestZone {
                vid,
                pid,
                path,
                protocol,
                zone,
            } => {
                let restore = self.known_color(vid, pid, zone);
                let result = (self.writers)(&path)
                    .and_then(|w| pcl::test_zone(protocol, zone, restore, w, wait));
                let back = if restore.is_some() {
                    "back to its colour"
                } else {
                    "left white (no colour known for it)"
                };
                self.status = Some(match result {
                    Ok(()) => format!("Zone {zone}: blinked, {back}"),
                    Err(DeviceError::Unsupported) => {
                        "Foxglove can't light zones with an unknown protocol".into()
                    }
                    Err(e) => format!("Zone {zone}: {e}"),
                });
            }
            Msg::SaveLocal(device) => {
                self.status = Some(format!("{}: saved as your own device", device.name));
                self.local.put(CatalogDevice {
                    local: true,
                    ..*device
                });
                self.save_local();
            }
            Msg::RemoveLocal(vid, pid) => {
                self.local.devices.retain(|d| (d.vid, d.pid) != (vid, pid));
                self.status = Some(format!("{vid:04x}:{pid:04x}: your entry removed"));
                self.save_local();
            }
        }
    }

    /// The user set colours on a board a scene holds: those go back on at the scene's end.
    fn scene_keeps(&mut self, key: &str, colors: &[(usize, Rgb)]) {
        let Some(before) = self.scene.as_mut().and_then(|s| s.get_mut(key)) else {
            return;
        };
        for &(i, c) in colors {
            if let Some(slot) = before.get_mut(i) {
                *slot = Some(c);
            }
        }
    }

    /// Writes the zones of a scene frame that differ from what the board shows. Ignored with
    /// no scene running.
    fn scene_frame(&mut self, key: &str, colors: &[(usize, [u8; 3])]) {
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        let Some(d) = self.drivers.get_mut(key) else {
            return;
        };
        let shown = d.colors();
        // A board that arrived during the scene: what it got on arrival comes back after.
        scene
            .entry(key.to_string())
            .or_insert_with(|| shown.clone());
        let changed: Vec<(usize, Rgb)> = colors
            .iter()
            .map(|&(i, c)| (i, rgb(c)))
            .filter(|&(i, c)| shown.get(i).copied().flatten() != Some(c))
            .collect();
        if changed.is_empty() {
            return;
        }
        if let Err(e) = d.set_zones(&changed) {
            if !std::mem::replace(&mut self.scene_error, true) {
                eprintln!("Foxglove: scene frame to {}: {e}", d.name());
            }
        }
    }

    /// The scene ended: each board it held gets its colours back.
    fn scene_end(&mut self) {
        let Some(before) = self.scene.take() else {
            return;
        };
        for (key, before) in before {
            let Some(f) = self.found.iter().find(|f| f.key == key).cloned() else {
                continue;
            };
            let saved = self.saved_colors(&f);
            let follows = self
                .settings
                .devices
                .get(&key)
                .is_some_and(|s| s.match_keyboard)
                && self.follow.is_some();
            // Following the keyboard: its colour now. Else the colours from before (white
            // where none was known: never dark).
            let colors = if follows {
                pcl::restore_colors(&saved, &[])
            } else {
                pcl::restore_colors(&before, &saved)
            };
            if let Some(d) = self.drivers.get_mut(&key) {
                if let Err(e) = d.set_zones(&colors) {
                    eprintln!("Foxglove: {}: colours back after the scene: {e}", f.name);
                }
            }
        }
    }

    /// Runs one driver call; `manual` saves the colours and stops matching the keyboard.
    fn write(
        &mut self,
        key: &str,
        done: &str,
        call: impl FnOnce(&mut dyn LightingDevice) -> foxglove_devices::Result<()>,
        manual: bool,
    ) {
        let Some(d) = self.drivers.get_mut(key) else {
            self.status = Some("That device isn't connected now".into());
            return;
        };
        let result = call(d.as_mut());
        // Under a scene, the other zones show its frame: the user's colours are the kept ones.
        let colors = match self.scene.as_ref().and_then(|s| s.get(key)) {
            Some(kept) if manual => kept.clone(),
            _ => d.colors(),
        };
        let name = d.name().to_string();
        self.status = Some(match &result {
            Ok(()) => format!("{name}: {}", done.to_lowercase()),
            Err(DeviceError::Disconnected) => format!("{name}: not connected"),
            Err(e) => format!("{name}: {e}"),
        });
        if result.is_err() || !manual {
            return;
        }
        let Some(found) = self.found.iter().find(|f| f.key == key) else {
            return;
        };
        let saved = self.settings.devices.entry(key.to_string()).or_default();
        saved.match_keyboard = false;
        for (zone, c) in found.zones.iter().zip(colors) {
            if let Some(c) = c {
                saved
                    .colors
                    .insert(zone.id.to_string(), hex([c.r, c.g, c.b]));
            }
        }
        self.dirty = true;
    }

    pub fn snapshot(&self) -> Snapshot {
        let devices = self
            .found
            .iter()
            .map(|f| {
                let saved = self.settings.devices.get(&f.key);
                let known = self.drivers.get(&f.key).map(|d| d.colors());
                let zones = f
                    .zones
                    .iter()
                    .enumerate()
                    .map(|(i, z)| {
                        let set = known
                            .as_ref()
                            .and_then(|k| k.get(i).copied().flatten())
                            .map(|c| [c.r, c.g, c.b]);
                        let saved = saved
                            .and_then(|s| s.colors.get(&z.id.to_string()))
                            .and_then(|h| parse_hex(h));
                        (z.name.clone(), set.or(saved))
                    })
                    .collect();
                Row {
                    key: f.key.clone(),
                    name: f.name.clone(),
                    control: f.control,
                    zones,
                    match_keyboard: saved.is_some_and(|s| s.match_keyboard),
                    caps: f.caps.clone(),
                    entry: f
                        .local
                        .then(|| self.local.find(f.vid, f.pid).map(CatalogDevice::to_toml))
                        .flatten(),
                    zone_ids: f.zones.iter().map(|z| z.id.to_string()).collect(),
                    calibration: f.calibration.clone(),
                    calibrated: foxglove_devices::calibration::global()
                        .device(&f.key)
                        .is_some(),
                    size: f.size,
                    zone_positions: f.zones.iter().map(|z| z.position).collect(),
                    zone_roles: f.zones.iter().map(|z| z.role).collect(),
                }
            })
            .collect();
        Snapshot {
            devices,
            light_studio: self.light_studio,
            status: self.status.clone(),
            busy: false,
            unknown: self.unknown.clone(),
            local_file: self.local_path.as_ref().map(|p| p.display().to_string()),
        }
    }
}

/// OMEN Light Studio's processes, matched exactly (case-insensitive).
pub const LIGHT_STUDIO: [&str; 2] = ["LightStudio-background.exe", "LightStudio.exe"];

/// Starts the worker, the HID arrival watch and the first scan.
pub fn start() {
    if super::repaint_trace::disabled("pc-lights") {
        return;
    }
    let (tx, rx) = mpsc::channel::<Msg>();
    if QUEUE.set(Mutex::new(tx.clone())).is_err() {
        return;
    }
    let settings = super::store::try_load::<Settings>("pc_lighting").unwrap_or_default();
    super::services::spawn_named("pc-lights", move || {
        #[cfg(target_os = "windows")]
        let _watch = {
            let tx = tx.clone();
            super::device_watch::DeviceWatch::interfaces(
                true,
                Box::new(move |_path| {
                    let _ = tx.send(Msg::Scan);
                }),
            )
        };
        let writers: Writers = Box::new(|path: &str| {
            Ok(Box::new(pcl::hid::PathWriter::new(path)?) as Box<dyn HidWriter>)
        });
        let local_path =
            super::store::path().map(|p| p.with_file_name("lighting-catalog.local.toml"));
        let mut worker = Worker::new(
            Catalog::bundled(),
            local_path,
            settings,
            writers,
            Box::new(lamp_arrays),
        );
        let _ = tx.send(Msg::Scan);
        while let Ok(first) = rx.recv() {
            // A burst of arrivals (a hub with several interfaces) scans once.
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            let batch = latest_shows(batch);
            let mut scanned = false;
            for msg in batch {
                if msg == Msg::Scan {
                    if !scanned {
                        scanned = true;
                        scan(&mut worker, true);
                    }
                    continue;
                }
                worker.handle(msg, super::pause::paused(), &mut std::thread::sleep);
            }
            if std::mem::take(&mut worker.rescan) {
                scan(&mut worker, false);
            }
            if std::mem::take(&mut worker.dirty) {
                if let Err(e) = super::store::save("pc_lighting", &worker.settings) {
                    eprintln!("Foxglove: can't save the PC lighting settings: {e}");
                }
            }
            *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(worker.snapshot());
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
            // A running scene places the boards found now (#485).
            if scanned {
                super::scene_run::devices_changed();
            }
        }
    });
}

/// Drops every [`Msg::Show`] (and [`Msg::SceneFrame`]) a later one for the same device
/// replaces: the calibrator's sliders and a scene can ask faster than a board writes, and only
/// the newest colour matters.
pub fn latest_shows(batch: Vec<Msg>) -> Vec<Msg> {
    let mut seen = std::collections::BTreeSet::new();
    let mut frames = std::collections::BTreeSet::new();
    let mut out: Vec<Msg> = batch
        .into_iter()
        .rev()
        .filter(|m| match m {
            Msg::Show { key, .. } => seen.insert(key.clone()),
            Msg::SceneFrame { key, .. } => frames.insert(key.clone()),
            _ => true,
        })
        .collect();
    out.reverse();
    out
}

/// How many scene ends the worker has handled, for [`end_scene`] to wait on.
static SCENE_ENDS: (Mutex<u64>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());

fn scene_ended() {
    let (count, changed) = &SCENE_ENDS;
    *count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    changed.notify_all();
}

/// Ends a scene on the boards and waits (at most `timeout`) until their colours are back, so
/// quitting right after still restores them. False if it didn't finish in time.
pub fn end_scene(timeout: std::time::Duration) -> bool {
    let (count, changed) = &SCENE_ENDS;
    let before = *count.lock().unwrap_or_else(|e| e.into_inner());
    let Some(q) = QUEUE.get() else {
        return true;
    };
    if q.lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(Msg::SceneEnd)
        .is_err()
    {
        return true;
    }
    let guard = count.lock().unwrap_or_else(|e| e.into_inner());
    let (_count, wait) = changed
        .wait_timeout_while(guard, timeout, |n| *n == before)
        .unwrap_or_else(|e| e.into_inner());
    !wait.timed_out()
}

fn scan(worker: &mut Worker, restore: bool) {
    match pcl::hid::enumerate() {
        Ok(hid) => worker.scan(
            &hid,
            light_studio_running(),
            super::pause::paused(),
            restore,
        ),
        Err(e) => eprintln!("Foxglove: lighting scan: {e}"),
    }
}

/// A LampArray's shape for scenes' room map (#483): its kind, its bounding box and each lamp's
/// position on it, in millimetres (x right, y down from the box's corner).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LampLayout {
    pub kind: u32,
    pub size_mm: [f32; 2],
    pub lamps: Vec<[f32; 2]>,
}

static LAMP_LAYOUTS: Mutex<BTreeMap<(u16, u16), LampLayout>> = Mutex::new(BTreeMap::new());

/// Each LampArray's shape by VID/PID, as the last read of its attributes found it. Two of one
/// model share an entry, as they share their attributes in [`Worker`]'s cache.
pub fn lamp_layouts() -> BTreeMap<(u16, u16), LampLayout> {
    LAMP_LAYOUTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Every LampArray's attributes, through Windows' Dynamic Lighting API: lamp count, whether
/// lamps have positions, the update interval and kind. Read once per new LampArray on a scan;
/// nothing is written to it. Each one's shape is kept for [`lamp_layouts`].
#[cfg(target_os = "windows")]
pub fn lamp_arrays() -> Vec<(u16, u16, LampArrayInfo)> {
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Lights::LampArray;
    let read = || -> windows::core::Result<Vec<(u16, u16, LampArrayInfo)>> {
        let selector = LampArray::GetDeviceSelector()?;
        let list = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()?;
        let mut out = Vec::new();
        for info in &list {
            let Ok(lamps) = LampArray::FromIdAsync(&info.Id()?).and_then(|op| op.join()) else {
                continue;
            };
            let count = lamps.LampCount().unwrap_or(0).max(0);
            // Windows gives metres.
            let at: Vec<_> = (0..count.min(512))
                .map(|i| lamps.GetLampInfo(i).and_then(|l| l.Position()).ok())
                .collect();
            let positions = at
                .iter()
                .flatten()
                .any(|p| p.X != 0.0 || p.Y != 0.0 || p.Z != 0.0);
            let kind = lamps.LampArrayKind().map_or(0, |k| k.0 as u32);
            let (vid, pid) = (lamps.HardwareVendorId()?, lamps.HardwareProductId()?);
            let size_mm = lamps
                .BoundingBox()
                .map_or([0.0; 2], |b| [b.X * 1000.0, b.Y * 1000.0]);
            let layout = LampLayout {
                kind,
                size_mm,
                lamps: at
                    .iter()
                    .map(|p| p.map_or([0.0; 2], |p| [p.X * 1000.0, p.Y * 1000.0]))
                    .collect(),
            };
            LAMP_LAYOUTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert((vid, pid), layout);
            out.push((
                vid,
                pid,
                LampArrayInfo {
                    lamp_count: count as u32,
                    positions,
                    // TimeSpan counts 100 ns.
                    min_update_us: lamps
                        .MinUpdateInterval()
                        .map_or(0, |t| (t.Duration.max(0) / 10) as u64),
                    kind,
                },
            ));
        }
        Ok(out)
    };
    read().unwrap_or_else(|e| {
        eprintln!("Foxglove: LampArray attributes: {e}");
        Vec::new()
    })
}

#[cfg(not(target_os = "windows"))]
pub fn lamp_arrays() -> Vec<(u16, u16, LampArrayInfo)> {
    Vec::new()
}

/// Whether OMEN Light Studio runs now: one process snapshot, taken only with a scan.
#[cfg(target_os = "windows")]
pub fn light_studio_running() -> bool {
    any_process_running(&LIGHT_STUDIO)
}

/// Whether a process with one of `names` (exe file names, any case) runs now: one snapshot,
/// taken when a caller asks (a scan, a Refresh or a change event), never on a timer.
#[cfg(target_os = "windows")]
pub(crate) fn any_process_running(names: &[&str]) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: a process snapshot handle, closed below on every path.
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return false;
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut found = false;
    // SAFETY: `entry` is a valid PROCESSENTRY32W with dwSize set; `snap` is open.
    let mut more = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while more {
        let end = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
        if names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            found = true;
            break;
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: closes the snapshot opened above, once.
    let _ = unsafe { CloseHandle(snap) };
    found
}

#[cfg(not(target_os = "windows"))]
pub fn light_studio_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::pc_lighting::{HidEntry, ZoneSpec};
    use std::sync::Arc;

    type Sent = Arc<Mutex<Vec<Vec<u8>>>>;

    /// The mocked HID writer: every report, in order.
    struct Recorder(Sent);

    impl HidWriter for Recorder {
        fn send(&mut self, reports: &[Vec<u8>]) -> foxglove_devices::Result<()> {
            self.0.lock().unwrap().extend(reports.iter().cloned());
            Ok(())
        }
    }

    fn worker(settings: Settings) -> (Worker, Sent) {
        worker_at(settings, None)
    }

    fn worker_at(settings: Settings, local: Option<PathBuf>) -> (Worker, Sent) {
        let sent: Sent = Arc::default();
        let s = sent.clone();
        let writers: Writers =
            Box::new(move |_path: &str| Ok(Box::new(Recorder(s.clone())) as Box<dyn HidWriter>));
        let lamps: LampArrays = Box::new(|| {
            vec![(
                0x046d,
                0xc332,
                LampArrayInfo {
                    lamp_count: 2,
                    positions: true,
                    min_update_us: 33_000,
                    kind: 2,
                },
            )]
        });
        (
            Worker::new(Catalog::bundled(), local, settings, writers, lamps),
            sent,
        )
    }

    fn case_hid() -> Vec<HidEntry> {
        vec![
            HidEntry {
                vid: 0x103c,
                pid: 0x84fd,
                usage_page: 1,
                path: "tracer".into(),
                ..Default::default()
            },
            HidEntry {
                vid: 0x03f0,
                pid: 0x7397,
                usage_page: 1,
                path: "pump".into(),
                ..Default::default()
            },
            HidEntry {
                vid: 0x046d,
                pid: 0xc332,
                usage_page: 0x59,
                usage: 1,
                path: "vhf".into(),
                product: Some("G502 Proteus Spectrum".into()),
                ..Default::default()
            },
        ]
    }

    /// The 45L's report for one zone, built from #423's layout.
    fn report(zone: u8, c: [u8; 3]) -> Vec<u8> {
        let mut b = vec![0u8; 59];
        b[2] = 0x12;
        b[3] = 0x01;
        b[4] = 1;
        b[5] = 1;
        let at = 0x08 + 3 * usize::from(zone - 1);
        b[at..at + 3].copy_from_slice(&c);
        b[0x30] = 0x64;
        b[0x31] = 0x02;
        b[0x36] = zone;
        b[0x37] = 0x01;
        b
    }

    fn no_wait() -> impl FnMut(std::time::Duration) {
        |_| {}
    }

    #[test]
    fn the_page_lists_each_device_and_how_it_is_controlled() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), true, false, true);
        let s = w.snapshot();
        let rows: Vec<_> = s
            .devices
            .iter()
            .map(|r| (r.name.as_str(), r.control, r.zones.len()))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "HP OMEN 45L case lights",
                    Control::Driver(pcl::Protocol::OmenTracer),
                    5
                ),
                ("HP OMEN liquid cooler", Control::NotYet, 0),
                ("G502 Proteus Spectrum", Control::LampArray, 0),
            ]
        );
        assert!(s.light_studio);
        assert!(
            sent.lock().unwrap().is_empty(),
            "a scan alone writes nothing"
        );
    }

    #[test]
    fn a_colour_pick_writes_its_zones_and_is_saved() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        w.handle(
            Msg::Set {
                key: "103c:84fd".into(),
                colors: vec![(0, [0xf0, 0x8a, 0x3c]), (4, [0, 0x80, 0x80])],
            },
            false,
            &mut no_wait(),
        );
        assert_eq!(
            *sent.lock().unwrap(),
            vec![report(1, [0xf0, 0x8a, 0x3c]), report(7, [0, 0x80, 0x80])]
        );
        assert!(w.dirty);
        let saved = &w.settings.devices["103c:84fd"];
        assert_eq!(saved.colors["1"], "#f08a3c");
        assert_eq!(saved.colors["7"], "#008080");
        let row = &w.snapshot().devices[0];
        assert_eq!(
            row.zones[0],
            ("Diamond display".into(), Some([0xf0, 0x8a, 0x3c]))
        );
        assert_eq!(row.zones[1].1, None, "never set");
    }

    #[test]
    fn saved_colours_come_back_on_arrival_unless_light_studio_runs() {
        let mut settings = Settings::default();
        settings.devices.insert(
            "103c:84fd".into(),
            Saved {
                colors: [("2".to_string(), "#112233".to_string())].into(),
                match_keyboard: false,
            },
        );
        let (mut w, sent) = worker(settings.clone());
        w.scan(&case_hid(), true, false, true);
        assert!(sent.lock().unwrap().is_empty(), "Light Studio is running");
        let (mut w, sent) = worker(settings);
        w.scan(&case_hid(), false, false, true);
        assert_eq!(*sent.lock().unwrap(), vec![report(2, [0x11, 0x22, 0x33])]);
        w.scan(&case_hid(), false, false, true);
        assert_eq!(
            sent.lock().unwrap().len(),
            1,
            "still there: no second write"
        );
        w.scan(&[], false, false, true);
        w.scan(&case_hid(), false, false, true);
        assert_eq!(sent.lock().unwrap().len(), 2, "replugged: put back again");
    }

    #[test]
    fn match_keyboard_follows_colour_changes_only() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        let key = "103c:84fd".to_string();
        w.handle(Msg::Follow([255, 0, 0]), false, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty(), "not matching yet");
        w.handle(Msg::Match(key.clone(), true), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 5, "the current colour, once");
        assert_eq!(sent.lock().unwrap()[4], report(7, [255, 0, 0]));
        w.handle(Msg::Follow([255, 0, 0]), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 5, "same colour: nothing");
        w.handle(Msg::Follow([0, 0, 255]), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 10);
        // A manual pick stops matching.
        w.handle(
            Msg::Set {
                key: key.clone(),
                colors: vec![(0, [1, 2, 3])],
            },
            false,
            &mut no_wait(),
        );
        assert!(!w.settings.devices[&key].match_keyboard);
        w.handle(Msg::Follow([0, 255, 0]), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 11);
        // Paused: the colour waits, and comes through once resumed.
        w.handle(Msg::Match(key.clone(), true), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 16);
        w.handle(Msg::Follow([9, 9, 9]), true, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 16);
        w.handle(Msg::Follow([9, 9, 9]), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 21);
    }

    #[test]
    fn off_identify_and_pause() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        let key = "103c:84fd".to_string();
        w.handle(Msg::Off(key.clone()), true, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty(), "paused: refused");
        assert!(w.snapshot().status.unwrap().contains("paused"));
        w.handle(Msg::Off(key.clone()), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap()[0], report(1, [0, 0, 0]));
        sent.lock().unwrap().clear();
        let mut waits = 0;
        w.handle(Msg::Identify(key.clone()), false, &mut |_| waits += 1);
        assert_eq!(waits, pcl::IDENTIFY_STEPS);
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 5 * pcl::IDENTIFY_STEPS + 5);
        assert_eq!(sent[sent.len() - 1], report(7, [0, 0, 0]), "back to off");
        w.handle(Msg::Off("gone".into()), false, &mut no_wait());
        assert!(w.snapshot().status.unwrap().contains("isn't connected"));
    }

    #[test]
    fn identify_with_no_prior_state_restores_the_saved_colours() {
        // Saved colours, but the board arrived while Light Studio ran: nothing was written,
        // so Foxglove knows no colours for it.
        let mut settings = Settings::default();
        settings.devices.insert(
            "103c:84fd".into(),
            Saved {
                colors: [("1".to_string(), "#112233".to_string())].into(),
                match_keyboard: false,
            },
        );
        let (mut w, sent) = worker(settings);
        w.scan(&case_hid(), true, false, true);
        assert!(sent.lock().unwrap().is_empty());
        w.handle(Msg::Identify("103c:84fd".into()), false, &mut no_wait());
        let sent = sent.lock().unwrap().clone();
        let back = &sent[sent.len() - 5..];
        assert_eq!(back[0], report(1, [0x11, 0x22, 0x33]), "the saved colour");
        assert_eq!(
            back[1],
            report(2, [255, 255, 255]),
            "nothing saved: white, not dark"
        );
    }

    #[test]
    fn colours_go_through_the_calibration_and_a_palette_change_keeps_it() {
        use foxglove_devices::calibration::{self, Calibration, DeviceCalibration};
        let key = "103c:84fd:PCLIGHTS1";
        let mut hid = case_hid();
        hid[0].serial = Some("PCLIGHTS1".into());
        let (mut w, sent) = worker(Settings::default());
        w.scan(&hid, false, false, true);
        let warm = DeviceCalibration {
            device: Calibration {
                gain: [1.0, 0.8, 0.5],
                ..Default::default()
            },
            zones: [(
                "7".to_string(),
                Calibration {
                    brightness: 0.5,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        calibration::global().set_device(key, Some(warm.clone()));
        let cal = |zone: &str, c: [u8; 3]| {
            let out = warm.zone(Some(zone)).apply_rgb(rgb(c));
            [out.r, out.g, out.b]
        };
        let orange = [0xf0, 0x8a, 0x3c];
        let pine = [0x5f, 0xc4, 0x8e];
        w.handle(
            Msg::Set {
                key: key.into(),
                colors: vec![(0, orange), (4, orange)],
            },
            false,
            &mut no_wait(),
        );
        // A new palette: other colours, the same calibration.
        w.handle(
            Msg::Set {
                key: key.into(),
                colors: vec![(0, pine), (4, pine)],
            },
            false,
            &mut no_wait(),
        );
        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                report(1, cal("1", orange)),
                report(7, cal("7", orange)),
                report(1, cal("1", pine)),
                report(7, cal("7", pine)),
            ],
            "zone 7 has its own calibration"
        );
        assert_eq!(calibration::global().device(key), Some(warm));
        // Saved and shown: the palette's colours, not the calibrated ones.
        assert_eq!(w.settings.devices[key].colors["1"], "#5fc48e");
        assert_eq!(w.snapshot().devices[0].zones[0].1, Some(pine));
        // A new calibration: Recalibrate sends the same palette through it, saving nothing;
        // zones never set go white, never dark.
        sent.lock().unwrap().clear();
        w.dirty = false;
        calibration::global().set_device(key, None);
        w.handle(Msg::Recalibrate(key.into()), false, &mut no_wait());
        let white = [255, 255, 255];
        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                report(1, pine),
                report(2, white),
                report(5, white),
                report(6, white),
                report(7, pine)
            ]
        );
        assert!(!w.dirty);
    }

    #[test]
    fn show_writes_without_saving_and_only_the_latest_of_a_burst() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        let key = "103c:84fd".to_string();
        let show = |c: u8| Msg::Show {
            key: key.clone(),
            colors: vec![(0, [c, c, c])],
        };
        let batch = latest_shows(vec![show(1), Msg::Scan, show(2), show(3)]);
        assert_eq!(batch, vec![Msg::Scan, show(3)]);
        for m in batch.into_iter().filter(|m| *m != Msg::Scan) {
            w.handle(m, false, &mut no_wait());
        }
        assert_eq!(*sent.lock().unwrap(), vec![report(1, [3, 3, 3])]);
        assert!(
            !w.dirty && !w.settings.devices.contains_key(&key),
            "not saved"
        );
        w.handle(show(9), true, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 1, "paused: refused");
    }

    #[test]
    fn colours_convert() {
        assert_eq!(hsv_rgb(0, 100, 100), [255, 0, 0]);
        assert_eq!(hsv_rgb(120, 100, 100), [0, 255, 0]);
        assert_eq!(hsv_rgb(240, 100, 50), [0, 0, 128]);
        assert_eq!(hsv_rgb(26, 0, 100), [255, 255, 255]);
        assert_eq!(parse_hex(&hex([1, 0xab, 0xff])), Some([1, 0xab, 0xff]));
        assert_eq!(parse_hex("f08a3c"), None);
        assert_eq!(parse_hex("#f08a3"), None);
    }

    #[test]
    fn devices_carry_caps_and_lamp_arrays_are_read_once() {
        let (mut w, _) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        let s = w.snapshot();
        assert_eq!(
            s.devices[0].caps.chips(false),
            vec!["Per-zone RGB · 5 zones", "5 onboard effects", "Brightness"]
        );
        assert_eq!(
            s.devices[1].caps.chips(false),
            vec!["Screen · images & animations"]
        );
        assert_eq!(
            s.devices[2].caps.chips(false)[0],
            "Per-LED RGB · 2 lamps · up to 30 fps"
        );
        assert!(s.unknown.is_empty(), "all three are placed");
    }

    /// The second desk (`fixtures`): another vendor's case board with no driver yet, and a
    /// lightbar on Dynamic Lighting, from the same scan code.
    #[test]
    fn the_second_desks_devices_are_found_and_described() {
        use crate::foxglove::fixtures::second_desk as desk;
        let sent: Sent = Arc::default();
        let s = sent.clone();
        let writers: Writers =
            Box::new(move |_path: &str| Ok(Box::new(Recorder(s.clone())) as Box<dyn HidWriter>));
        let lamps: LampArrays =
            Box::new(|| vec![(desk::LIGHTBAR.0, desk::LIGHTBAR.1, desk::lightbar_info())]);
        let mut w = Worker::new(desk::catalog(), None, Settings::default(), writers, lamps);
        w.scan(&desk::hid(), false, false, true);
        let snap = w.snapshot();
        let rows: Vec<_> = snap
            .devices
            .iter()
            .map(|r| (r.name.as_str(), r.control, r.zones.len()))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Nimbus Strata case lights", Control::NotYet, 4),
                ("Lumen desk lightbar", Control::LampArray, 0),
            ]
        );
        assert_eq!(
            snap.devices[0].caps.chips(false),
            vec!["Per-zone RGB · 4 zones"]
        );
        assert_eq!(
            snap.devices[1].caps.chips(false)[0],
            "Per-LED RGB · 12 lamps · up to 50 fps"
        );
        assert_eq!(snap.devices[0].size, Some([230, 500]));
        assert!(snap.unknown.is_empty(), "both are placed");
        assert!(sent.lock().unwrap().is_empty(), "nothing written");
    }

    #[test]
    fn a_hand_added_board_is_saved_found_and_tested_only_on_request() {
        let dir = crate::foxglove::store::test_config_dir().join("pc-lights-local");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("lighting-catalog.local.toml");
        let _ = std::fs::remove_file(&file);
        let (mut w, sent) = worker_at(Settings::default(), Some(file.clone()));
        let strip = HidEntry {
            vid: 0x1209,
            pid: 0x0001,
            usage_page: 0xff00,
            path: "strip".into(),
            product: Some("Desk strip".into()),
            ..Default::default()
        };
        let mut hid = case_hid();
        hid.push(strip);
        w.scan(&hid, false, false, true);
        let s = w.snapshot();
        assert_eq!(s.unknown.len(), 1);
        assert_eq!(s.unknown[0].name, "Desk strip");
        assert!(sent.lock().unwrap().is_empty(), "listing writes nothing");

        // A zone test: one zone blinks, then goes white (nothing known for it), never dark.
        w.handle(
            Msg::TestZone {
                vid: 0x1209,
                pid: 0x0001,
                path: "strip".into(),
                protocol: Protocol::OmenTracer,
                zone: 2,
            },
            false,
            &mut no_wait(),
        );
        {
            let sent = sent.lock().unwrap();
            assert_eq!(sent.len(), pcl::IDENTIFY_STEPS + 1);
            assert_eq!(*sent.last().unwrap(), report(2, [255, 255, 255]));
        }
        assert!(w.snapshot().status.unwrap().contains("left white"));
        // Paused: refused.
        w.handle(
            Msg::TestZone {
                vid: 0x1209,
                pid: 0x0001,
                path: "strip".into(),
                protocol: Protocol::OmenTracer,
                zone: 1,
            },
            true,
            &mut no_wait(),
        );
        assert_eq!(sent.lock().unwrap().len(), pcl::IDENTIFY_STEPS + 1);

        let entry = CatalogDevice::new(
            "Desk strip".into(),
            0x1209,
            0x0001,
            Protocol::OmenTracer,
            vec![ZoneSpec {
                id: 2,
                name: "Shelf".into(),
                position: None,
                role: Default::default(),
            }],
        );
        w.handle(Msg::SaveLocal(Box::new(entry)), false, &mut no_wait());
        assert!(w.rescan);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("name = \"Shelf\""), "{text}");
        let before = sent.lock().unwrap().len();
        w.rescan = false;
        w.scan(&hid, false, false, false);
        let s = w.snapshot();
        let row = s.devices.iter().find(|r| r.name == "Desk strip").unwrap();
        assert!(row.entry.as_ref().unwrap().contains("vid = 0x1209"));
        assert_eq!(row.caps.chips(false), vec!["Per-zone RGB · 1 zone"]);
        assert!(s.unknown.is_empty());
        assert_eq!(sent.lock().unwrap().len(), before, "saving writes nothing");

        // A fresh worker reads the local file back.
        let (mut again, _) = worker_at(Settings::default(), Some(file.clone()));
        again.scan(&hid, false, false, true);
        assert!(again
            .snapshot()
            .devices
            .iter()
            .any(|r| r.name == "Desk strip"));
        again.handle(Msg::RemoveLocal(0x1209, 0x0001), false, &mut no_wait());
        assert!(!std::fs::read_to_string(&file)
            .unwrap()
            .contains("Desk strip"));
    }

    #[test]
    fn a_zone_test_on_a_known_board_puts_its_colour_back() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        w.handle(
            Msg::Set {
                key: "103c:84fd".into(),
                colors: vec![(1, [9, 8, 7])],
            },
            false,
            &mut no_wait(),
        );
        sent.lock().unwrap().clear();
        w.handle(
            Msg::TestZone {
                vid: 0x103c,
                pid: 0x84fd,
                path: "tracer".into(),
                protocol: Protocol::OmenTracer,
                zone: 2,
            },
            false,
            &mut no_wait(),
        );
        assert_eq!(*sent.lock().unwrap().last().unwrap(), report(2, [9, 8, 7]));
    }
    /// Every zone of the 45L (zones 1, 2, 5, 6, 7) one colour, as a scene frame.
    fn frame(key: &str, c: [u8; 3]) -> Msg {
        Msg::SceneFrame {
            key: key.into(),
            colors: (0..5).map(|i| (i, c)).collect(),
        }
    }

    /// #485: a scene takes the case, writes only changed zones, and its end puts back the
    /// colours from before it; nothing is saved as the user's.
    #[test]
    fn a_scene_takes_the_case_and_gives_its_colours_back() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        let key = "103c:84fd".to_string();
        w.handle(
            Msg::Set {
                key: key.clone(),
                colors: (0..5).map(|i| (i, [10, 20, 30])).collect(),
            },
            false,
            &mut no_wait(),
        );
        w.dirty = false;
        sent.lock().unwrap().clear();
        // No scene: a stray frame writes nothing.
        w.handle(frame(&key, [1, 1, 1]), false, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty());
        w.handle(Msg::SceneStart, false, &mut no_wait());
        w.handle(frame(&key, [200, 100, 0]), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 5);
        // One zone changes: one report.
        let mut colors: Vec<(usize, [u8; 3])> = (0..5).map(|i| (i, [200, 100, 0])).collect();
        colors[4].1 = [255, 255, 0];
        w.handle(
            Msg::SceneFrame {
                key: key.clone(),
                colors,
            },
            false,
            &mut no_wait(),
        );
        assert_eq!(sent.lock().unwrap().len(), 6);
        assert_eq!(sent.lock().unwrap()[5], report(7, [255, 255, 0]));
        // Paused: frames stop quietly (no status line).
        let status = w.snapshot().status;
        w.handle(frame(&key, [9, 9, 9]), true, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 6);
        assert_eq!(w.snapshot().status, status);
        sent.lock().unwrap().clear();
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        let back = sent.lock().unwrap().clone();
        assert_eq!(back.len(), 5);
        assert_eq!(back[0], report(1, [10, 20, 30]));
        assert_eq!(back[4], report(7, [10, 20, 30]));
        assert!(!w.dirty, "the scene's colours were never the user's");
        // Over: frames write nothing again, and a second end does nothing.
        sent.lock().unwrap().clear();
        w.handle(frame(&key, [1, 1, 1]), false, &mut no_wait());
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty());
    }

    /// While a scene runs, the user's pick is what comes back after it, the followed keyboard
    /// colour waits for its end, and a board with nothing known goes back to white, not dark.
    #[test]
    fn picks_and_follows_during_a_scene_come_back_after_it() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), false, false, true);
        let key = "103c:84fd".to_string();
        w.handle(Msg::SceneStart, false, &mut no_wait());
        w.handle(frame(&key, [50, 50, 50]), false, &mut no_wait());
        w.handle(
            Msg::Set {
                key: key.clone(),
                colors: vec![(0, [1, 2, 3])],
            },
            false,
            &mut no_wait(),
        );
        assert_eq!(w.settings.devices[&key].colors["1"], "#010203", "saved");
        sent.lock().unwrap().clear();
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        let back = sent.lock().unwrap().clone();
        assert_eq!(back[0], report(1, [1, 2, 3]), "the pick");
        assert_eq!(back[1], report(2, [255, 255, 255]), "unknown before: white");

        // Matching the keyboard: a colour change during the scene isn't written until it ends.
        w.handle(Msg::Match(key.clone(), true), false, &mut no_wait());
        w.handle(Msg::Follow([0, 0, 255]), false, &mut no_wait());
        w.handle(Msg::SceneStart, false, &mut no_wait());
        sent.lock().unwrap().clear();
        w.handle(Msg::Follow([255, 0, 0]), false, &mut no_wait());
        assert!(
            sent.lock().unwrap().is_empty(),
            "the scene holds the lights"
        );
        // Paused at the end: the colours still go back, like leases.
        w.handle(Msg::SceneEnd, true, &mut no_wait());
        let back = sent.lock().unwrap().clone();
        assert_eq!(back.len(), 5);
        assert!(back
            .iter()
            .zip([1, 2, 5, 6, 7])
            .all(|(r, z)| *r == report(z, [255, 0, 0])));
    }

    /// A board that arrives during a scene gets its saved colours back at the end, and the
    /// user's Off during a scene is what comes back.
    #[test]
    fn a_board_arriving_mid_scene_and_off_during_one() {
        let mut settings = Settings::default();
        settings.devices.insert(
            "103c:84fd".into(),
            Saved {
                colors: (1..=7)
                    .map(|z| (z.to_string(), "#0a0b0c".to_string()))
                    .collect(),
                match_keyboard: false,
            },
        );
        let (mut w, sent) = worker(settings);
        let key = "103c:84fd".to_string();
        w.handle(Msg::SceneStart, false, &mut no_wait());
        w.scan(&case_hid(), false, false, true);
        w.handle(frame(&key, [90, 90, 90]), false, &mut no_wait());
        sent.lock().unwrap().clear();
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        assert_eq!(
            sent.lock().unwrap()[0],
            report(1, [10, 11, 12]),
            "its saved colour"
        );

        w.handle(Msg::SceneStart, false, &mut no_wait());
        w.handle(frame(&key, [90, 90, 90]), false, &mut no_wait());
        w.handle(Msg::Off(key.clone()), false, &mut no_wait());
        assert_eq!(w.settings.devices[&key].colors["7"], "#000000");
        sent.lock().unwrap().clear();
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        assert!(sent
            .lock()
            .unwrap()
            .iter()
            .zip([1, 2, 5, 6, 7])
            .all(|(r, z)| *r == report(z, [0, 0, 0])));
    }

    #[test]
    fn a_burst_of_scene_frames_writes_only_the_newest() {
        let a = |c: u8| frame("a", [c, c, c]);
        let batch = latest_shows(vec![
            a(1),
            frame("b", [5, 5, 5]),
            Msg::SceneStart,
            a(2),
            a(3),
        ]);
        assert_eq!(batch, vec![frame("b", [5, 5, 5]), Msg::SceneStart, a(3)]);
        assert!(
            end_scene(std::time::Duration::from_millis(10)),
            "no worker: nothing to wait for"
        );
    }
}
