//! Other devices' lighting (#423): case lights, coolers and other lights that aren't the
//! keyboard, through the device layer's `pc_lighting` discovery and drivers.
//!
//! One worker thread owns the drivers and blocks on its queue. It scans when the app starts,
//! when a HID interface arrives or goes (`device_watch`), and when the Lights page opens; it
//! writes only for a click on the page or for "Match keyboard", which follows what the keyboard
//! reports it shows (its RGB_STATE report, #616), not a colour Foxglove meant to send it.
//! Nothing streams or polls.
//! The colours the user picks are saved in the store's `[pc_lighting]` and put back when the
//! board arrives (at startup too), unless an app its catalog entry names as a conflict (a
//! vendor's lighting tool) is running.
//!
//! Each device carries what it can do (`LightCaps`): from the catalog, or from a LampArray's own
//! attributes, read through Windows when one arrives. Boards the catalog doesn't know can be
//! added by hand: the page lists them, a zone test lights one zone only on a click and puts it
//! back, and the entry goes into the user's `lighting-catalog.local.toml`.
//!
//! Every driver writes through its device's colour calibration (`pc_lighting::open` wraps it);
//! the colours here, saved and shown, are the palette's, before calibration.
//!
//! OpenRGB's controllers (#471) are light sources here too, once the user turns "Use OpenRGB's
//! devices" on (the store's `[openrgb]`, off by default): the worker connects to OpenRGB's SDK
//! server at start-up and when the setting changes, lists its controllers again on OpenRGB's
//! own DEVICE_LIST_UPDATED push, and (only if the user asked) tries again after OpenRGB goes, on
//! the crate's bounded backoff. Nothing polls; with the setting off there is no socket.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use foxglove_devices::pc_lighting::{
    self as pcl, openrgb, Catalog, CatalogDevice, Control, Found, HidWriter, LampArrayDesc,
    LampArrayInfo, LightCaps, LightingDevice, Protocol, Unknown,
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
    /// The gains its first match starts from (linear light), from the catalog's hint for its
    /// model or class (#473); `None` starts neutral.
    pub calibration_hint: Option<[f32; 3]>,
    /// The user calibrated this device.
    pub calibrated: bool,
    /// Its front in millimetres and each zone's position on it (in zone order), from the
    /// catalog, for scenes' room map (#483).
    pub size: Option<[u16; 2]>,
    pub zone_positions: Vec<Option<[u16; 2]>>,
    /// Each zone's role in a scene (point or ambient), in zone order, from the catalog.
    pub zone_roles: Vec<foxglove_devices::pc_lighting::ZoneRole>,
    /// What it is, from the catalog.
    pub kind: foxglove_devices::pc_lighting::DeviceKind,
    /// A Dynamic Lighting device Foxglove drives: whether Windows lets it now (#691).
    pub lamp_access: Option<pcl::LampAccess>,
}

/// An app the catalog names as a conflict for a found device, running now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflicting {
    /// The app's name.
    pub app: String,
    /// The device it may override: its name, and its key ([`Found::key`]).
    pub device: String,
    pub key: String,
    /// How to remove it or turn it off.
    pub hint: Option<String>,
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub devices: Vec<Row>,
    /// Apps running now that may put their own colours back on a found device (from the
    /// catalog's conflicts), one per app and device.
    pub conflicts: Vec<Conflicting>,
    /// The last action's result, for the status line.
    pub status: Option<String>,
    pub busy: bool,
    /// HID devices nothing recognises, for "Add a device".
    pub unknown: Vec<Unknown>,
    /// Where the user's own entries are saved.
    pub local_file: Option<String>,
    /// How the last write to each Dynamic Lighting device went, by key, for the health view
    /// (#691, [`super::lamp_health`]).
    pub lamp_applies: BTreeMap<String, super::lamp_health::Apply>,
    /// Keyboards that are also LampArrays (#754): Windows lists them under Dynamic Lighting,
    /// their own link drives them, so they aren't in [`Self::devices`]; the health view names
    /// them.
    pub keyboard_lamp_arrays: Vec<pcl::KeyboardLampArray>,
    /// "Use OpenRGB's devices" (#471): the user's settings and how the connection is.
    pub openrgb: OpenRgbView,
}

/// OpenRGB as the page shows it (#471).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenRgbView {
    pub settings: openrgb::Settings,
    /// Connected now.
    pub connected: bool,
    /// How it went, for the line under the toggle; `None` while it's off.
    pub status: Option<String>,
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
    /// A colour Foxglove is about to send the keyboard (the accent follower sends it). Ignored
    /// since #616: the push may not take (the keymap has no colour behavior) and an effect may
    /// hide it, so the case follows the keyboard's own report ([`Msg::Keyboard`]) instead.
    Follow([u8; 3]),
    /// What the keyboard reports it shows (its lighting state from RGB_STATE): what
    /// "Match keyboard" follows.
    Keyboard(KeyboardLights),
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
    /// Windows changed who may drive this Dynamic Lighting device (`AvailabilityChanged`,
    /// #691): its colours go back on when Foxglove may again.
    LampChanged(String),
    /// The user changed the OpenRGB settings (#471): saved, then connected (or closed) anew.
    OpenRgbSettings(openrgb::Settings),
    /// Connect to OpenRGB if it's on and not connected (start-up, a backoff try).
    OpenRgbConnect,
    /// OpenRGB's push or the connection's end, from the connection it numbers (an older one's
    /// news is dropped).
    OpenRgbEvent(u64, openrgb::Event),
    /// The Keyboard card's pick while Windows Dynamic Lighting holds the keyboard's lights
    /// (#749 b), drawn through the keyboard's own LampArray (`key`, from
    /// `scene_output::keyboard_lamp_key`) like any Dynamic Lighting device; `None` is lights
    /// off. Not saved: the keyboard keeps its own look over its link. `calibration` is the
    /// keyboard's calibration key (`devices::keyboard_key`): its colour calibration goes on the
    /// colour, as on the keyboard's own link. `animated`: the scene engine draws the pick
    /// (Breathe, Spectrum, Swirl, #768, `scene_card`), so the worker lets the LampArray go
    /// rather than show a steady colour over it.
    KeyboardLamp {
        key: String,
        look: Option<KeyboardLights>,
        calibration: Option<String>,
        animated: bool,
    },
    /// Windows let go of the keyboard's lights (its owner report): the keyboard's LampArray
    /// is let go too, and the link draws it again.
    KeyboardLampRelease,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static QUEUE: OnceLock<Mutex<mpsc::Sender<Msg>>> = OnceLock::new();
static LAST_FRAME: Mutex<Option<u64>> = Mutex::new(None);
/// The settings window's context, to repaint it when the worker has news.
static CTX: OnceLock<egui::Context> = OnceLock::new();

/// Requests queued and requests the worker is done with (its snapshot published), so an API
/// call can wait for its own write (#617): `send` numbers each request under the queue's lock,
/// and the queue is in order. The waiter sleeps on the condvar; the timeout is only a fallback.
static TICKETS: (Mutex<(u64, u64)>, Condvar) = (Mutex::new((0, 0)), Condvar::new());

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
    queue(msg);
}

/// Queues a request and waits until the worker has handled it and published its snapshot
/// (the local API's `pc_lighting.set_zone`, #617). `None`: no worker, or no answer in
/// `timeout`.
pub fn send_and_wait(msg: Msg, timeout: std::time::Duration) -> Option<Snapshot> {
    let ticket = queue(msg)?;
    let (lock, done) = &TICKETS;
    let tickets = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (tickets, _) = done
        .wait_timeout_while(tickets, timeout, |t| t.1 < ticket)
        .unwrap_or_else(|e| e.into_inner());
    (tickets.1 >= ticket).then(snapshot)
}

/// The worker handled this many more requests and published the snapshot.
struct Handled(u64);

impl Drop for Handled {
    fn drop(&mut self) {
        let (lock, done) = &TICKETS;
        lock.lock().unwrap_or_else(|e| e.into_inner()).1 += self.0;
        done.notify_all();
    }
}

/// Queues a request and gives its ticket.
fn queue(msg: Msg) -> Option<u64> {
    let q = QUEUE.get()?;
    {
        if matches!(
            msg,
            Msg::Set { .. } | Msg::Off(_) | Msg::Identify(_) | Msg::TestZone { .. }
        ) {
            if let Some(s) = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                s.busy = true;
            }
        }
        let q = q.lock().unwrap_or_else(|e| e.into_inner());
        ticketed(&q, msg)
    }
}

/// Sends on the worker's queue and numbers the request; every send goes through here, so the
/// numbers follow the queue's order.
fn ticketed(tx: &mpsc::Sender<Msg>, msg: Msg) -> Option<u64> {
    let mut tickets = TICKETS.0.lock().unwrap_or_else(|e| e.into_inner());
    tx.send(msg).ok()?;
    tickets.0 += 1;
    Some(tickets.0)
}

/// Called each frame the Lights page draws: the first frame after it wasn't shown scans again
/// (and checks for conflicting apps), so the page is current whenever it's opened.
pub fn page_shown(ctx: &egui::Context) {
    let _ = CTX.set(ctx.clone());
    let frame = ctx.cumulative_frame_nr();
    let mut last = LAST_FRAME.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_none_or(|l| frame > l + 1) {
        send(Msg::Scan);
    }
    *last = Some(frame);
}

/// What the keyboard reports it shows, in the palette's colours (hue 0-359, saturation and
/// brightness 0-100) with its effect: what "Match keyboard" follows (#616).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardLights {
    pub hue: u16,
    pub sat: u8,
    pub brightness: u8,
    /// The keyboard's effect: 0 solid, 1 breathe, 2 spectrum, 3 swirl (ZMK's underglow).
    pub effect: u8,
}

impl KeyboardLights {
    /// From a ZMK keyboard's reported lighting state; `None` for an effect it doesn't number.
    pub fn from_state(s: &foxglove_devices::LightingState) -> Option<Self> {
        Some(Self {
            hue: s.hue,
            sat: s.sat,
            brightness: s.brightness,
            effect: s.effect.id.parse().ok()?,
        })
    }

    /// From an RGB_STATE half (the left's: the half the PC talks to).
    pub fn from_half(h: &foxglove_link::protocol::RgbHalf) -> Self {
        Self {
            hue: h.hue,
            sat: h.sat,
            brightness: h.brightness,
            effect: h.effect,
        }
    }

    /// What a device of `zones` zones shows to match it, zone by zone, or `None` when it
    /// can't. An effect that shows the chosen colour (solid; breathe, one colour pulsing) gives
    /// that colour on every zone, steady. One that ignores it (Spectrum, Swirl: a rainbow;
    /// which ones is the effect table's data, `lights_page::EFFECT_TABLE`) gives `None`: no
    /// device driver here can run a rainbow effect from Foxglove yet, and the colour the
    /// keyboard was sent isn't on it, so there's nothing honest to copy.
    ///
    /// On/off isn't followed: the keyboard dims on its own idle timer, which a case shouldn't
    /// copy, and the report's on/off doesn't always match what is lit yet (#605).
    pub fn colors(&self, zones: usize) -> Option<Vec<[u8; 3]>> {
        let effect = super::lights_page::effect(self.effect)?;
        effect
            .uses_colour()
            .then(|| vec![hsv_rgb(self.hue, self.sat, self.brightness); zones])
    }

    /// The effect's name, for the status line (#670: just the name).
    fn effect_label(&self) -> String {
        super::lights_page::effect(self.effect)
            .map_or("an effect Foxglove doesn't know", |e| e.name)
            .to_string()
    }
}

/// The keyboard reported its lights (any RGB_STATE read or event): "Match keyboard" follows it.
pub fn keyboard_lights(s: &foxglove_devices::LightingState) {
    if let Some(k) = KeyboardLights::from_state(s) {
        send(Msg::Keyboard(k));
    }
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

/// Reads every LampArray's attributes and lamps (Windows).
pub type LampArrays = Box<dyn FnMut() -> Vec<LampArrayDesc> + Send>;

/// Opens the host's handle on a found Dynamic Lighting device (#691): Windows' LampArray, or a
/// fake in tests. `None` when the host has none for it.
pub type LampOpener =
    Box<dyn FnMut(&Found) -> Option<Box<dyn foxglove_devices::pc_lighting::LampHandle>> + Send>;

/// Where OpenRGB's news goes: the connection's number and the event (the worker's own queue;
/// a channel in tests).
pub type OpenRgbEvents = Arc<dyn Fn(u64, openrgb::Event) + Send + Sync>;

/// OpenRGB's side of the worker (#471).
struct OpenRgb {
    settings: openrgb::Settings,
    client: Option<Arc<openrgb::Client>>,
    /// Numbers each connection, so a closed one's late news is told apart.
    conn: u64,
    /// Its controllers as last listed.
    sources: Vec<openrgb::Source>,
    status: Option<String>,
    /// The reconnect waits left, while trying again (only when the user asked for it).
    backoff: Option<openrgb::Backoff>,
    events: OpenRgbEvents,
}

impl OpenRgb {
    fn new(settings: openrgb::Settings, events: OpenRgbEvents) -> Self {
        Self {
            settings,
            client: None,
            conn: 0,
            sources: Vec::new(),
            status: None,
            backoff: None,
            events,
        }
    }

    fn at(&self) -> String {
        format!("{}:{}", self.settings.host.trim(), self.settings.port)
    }
}

/// The worker's state, apart from the OS: tests drive it with a mocked HID writer.
pub struct Worker {
    bundled: Catalog,
    local: Catalog,
    local_path: Option<PathBuf>,
    catalog: Catalog,
    writers: Writers,
    lamp_arrays: LampArrays,
    lamp_info: BTreeMap<(u16, u16), LampArrayDesc>,
    lamp_opener: LampOpener,
    found: Vec<Found>,
    unknown: Vec<Unknown>,
    /// Keyboards' own LampArrays, from the last scan (#754).
    keyboard_lamps: Vec<pcl::KeyboardLampArray>,
    /// The catalog changed: scan again, without putting colours back.
    pub rescan: bool,
    drivers: BTreeMap<String, Box<dyn LightingDevice>>,
    pub settings: Settings,
    /// What the keyboard last reported it shows ("Match keyboard" follows it).
    follow: Option<KeyboardLights>,
    conflicts: Vec<Conflicting>,
    status: Option<String>,
    /// Settings changed and need saving.
    pub dirty: bool,
    /// A scene holds the lights: each board's colours from before it, by key.
    scene: Option<BTreeMap<String, Vec<Option<Rgb>>>>,
    /// A scene write failed once already (logged once per scene).
    scene_error: bool,
    /// The last write to each Dynamic Lighting device, by key (#691's health signal).
    lamp_applies: BTreeMap<String, super::lamp_health::Apply>,
    /// The last HID enumeration, for placing OpenRGB's list beside it without another one.
    hid: Vec<pcl::HidEntry>,
    openrgb: OpenRgb,
    /// OpenRGB's list changed: place it again, with the HID devices as last enumerated.
    pub relist: bool,
    /// The OpenRGB settings changed and need saving (`[openrgb]`).
    pub openrgb_dirty: bool,
    /// Try connecting to OpenRGB again after this wait (the bounded backoff's next step).
    pub openrgb_retry: Option<Duration>,
    /// The keyboard's own LampArray, opened for the Keyboard card while Windows Dynamic
    /// Lighting holds the keyboard's lights (#749 b).
    keyboard_lamp: Option<KeyboardLamp>,
}

/// The Keyboard card's pick while a scene draws the held keyboard through Windows (#768).
const SCENE_FIRST: &str = "a scene is running; this look shows when it ends";

/// The keyboard's own LampArray as the Lights worker draws it (#749 b): the same host handle as
/// any Dynamic Lighting device's, opened by the same opener, one colour on every lamp.
struct KeyboardLamp {
    key: String,
    name: String,
    handle: Box<dyn pcl::LampHandle>,
    /// What Foxglove last set on it (the pick, before calibration), put back when Windows lets
    /// Foxglove drive it again.
    shown: Option<Rgb>,
    /// The keyboard's calibration key; `None` sends the pick as it is.
    calibration: Option<String>,
}

impl KeyboardLamp {
    /// A pick as the lamps are sent it: through the keyboard's colour calibration (as its
    /// host-link colour is), off left dark.
    fn sent(&self, pick: Rgb) -> Rgb {
        match &self.calibration {
            Some(cal) if pick != pcl::BLACK => rgb(super::calibrate::on_device(
                cal,
                None,
                [pick.r, pick.g, pick.b],
            )),
            _ => pick,
        }
    }
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
            lamp_opener: Box::new(|_| None),
            found: Vec::new(),
            unknown: Vec::new(),
            keyboard_lamps: Vec::new(),
            rescan: false,
            drivers: BTreeMap::new(),
            settings,
            follow: None,
            conflicts: Vec::new(),
            status: None,
            dirty: false,
            scene: None,
            scene_error: false,
            lamp_applies: BTreeMap::new(),
            hid: Vec::new(),
            openrgb: OpenRgb::new(openrgb::Settings::default(), Arc::new(|_, _| {})),
            relist: false,
            openrgb_dirty: false,
            openrgb_retry: None,
            keyboard_lamp: None,
        }
    }

    /// OpenRGB's settings (the store's `[openrgb]`), and where its connection's news goes. It
    /// connects on [`Msg::OpenRgbConnect`], not here.
    pub fn with_openrgb(mut self, settings: openrgb::Settings, events: OpenRgbEvents) -> Self {
        self.openrgb = OpenRgb::new(settings, events);
        self
    }

    /// The OpenRGB settings as they are now (to save).
    pub fn openrgb_settings(&self) -> &openrgb::Settings {
        &self.openrgb.settings
    }

    /// Places OpenRGB's list again beside the HID devices as last enumerated (no new
    /// enumeration): arrivals get their saved colours back.
    pub fn relist(&mut self, running: &mut dyn FnMut(&[&str]) -> bool, paused: bool) {
        let hid = std::mem::take(&mut self.hid);
        self.scan(&hid, running, paused, true);
    }

    /// Connects when the setting is on (a connection already open stays, unless `fresh`);
    /// closes when it's off. The list follows on [`Self::relist`].
    fn openrgb_connect(&mut self, fresh: bool) {
        if self.openrgb.client.is_some() && !fresh {
            return;
        }
        let had = self.openrgb.client.take().is_some() || !self.openrgb.sources.is_empty();
        self.openrgb.sources.clear();
        self.openrgb.conn += 1;
        if !self.openrgb.settings.enabled {
            self.openrgb.status = None;
            self.openrgb.backoff = None;
            self.relist |= had;
            return;
        }
        let conn = self.openrgb.conn;
        let events = Arc::clone(&self.openrgb.events);
        match openrgb::connect(&self.openrgb.settings, move |e| events(conn, e)) {
            Ok(Some(client)) => {
                self.openrgb.backoff = None;
                self.openrgb.client = Some(client);
                self.openrgb_list();
            }
            Ok(None) => {}
            Err(e) => {
                let why = format!("Can't reach OpenRGB at {} ({e})", self.openrgb.at());
                self.openrgb_failed(why);
            }
        }
        self.relist = true;
    }

    /// Reads OpenRGB's controllers. A list that changed while being read waits for the push
    /// that announced it.
    fn openrgb_list(&mut self) {
        let Some(client) = &self.openrgb.client else {
            return;
        };
        match openrgb::list(client) {
            Ok(sources) => {
                let n = sources.len();
                self.openrgb.sources = sources;
                self.openrgb.status = Some(format!(
                    "Connected to OpenRGB at {}: {n} device{}",
                    self.openrgb.at(),
                    if n == 1 { "" } else { "s" }
                ));
            }
            Err(DeviceError::Busy(_)) => {}
            Err(e) => {
                self.openrgb.status = Some(format!("OpenRGB's device list: {e}"));
            }
        }
        self.relist = true;
    }

    /// The connection failed or ended: says why, and tries again only if the user asked to,
    /// at most the backoff's few times.
    fn openrgb_failed(&mut self, why: String) {
        self.openrgb.client = None;
        if self.openrgb.backoff.is_none() {
            self.openrgb.backoff = self.openrgb.settings.backoff();
        }
        let wait = self.openrgb.backoff.as_mut().and_then(Iterator::next);
        self.openrgb.status = Some(match (wait, self.openrgb.settings.reconnect) {
            (Some(w), _) => format!("{why}; trying again in {} s", w.as_secs()),
            (None, true) => format!("{why}; stopped trying"),
            (None, false) => why,
        });
        self.openrgb_retry = wait;
    }

    /// OpenRGB's news, from the connection open now.
    fn openrgb_event(&mut self, conn: u64, event: openrgb::Event) {
        if conn != self.openrgb.conn || self.openrgb.client.is_none() {
            return;
        }
        match event {
            openrgb::Event::DeviceListUpdated => self.openrgb_list(),
            openrgb::Event::Disconnected => {
                self.openrgb.sources.clear();
                self.relist = true;
                self.openrgb_failed("OpenRGB closed the connection".into());
            }
        }
    }

    /// Drives Dynamic Lighting devices through `opener` (#691); without one they are listed only.
    pub fn with_lamp_opener(mut self, opener: LampOpener) -> Self {
        self.lamp_opener = opener;
        self
    }

    /// A new enumeration: opens drivers for arrivals and puts their saved colours back (unless
    /// an app the catalog names as the device's conflict is running), and drops the ones that
    /// went. `running` says whether any of some exe names runs now; it is asked only here, for
    /// the found devices' conflicts.
    /// `restore`: false after a catalog change, which isn't a board arriving.
    pub fn scan(
        &mut self,
        hid: &[pcl::HidEntry],
        running: &mut dyn FnMut(&[&str]) -> bool,
        paused: bool,
        restore: bool,
    ) {
        let mut now = pcl::discover(&self.catalog, hid);
        self.hid = hid.to_vec();
        // OpenRGB's controllers, as last listed (#471).
        now.extend(self.openrgb.sources.iter().map(openrgb::found));
        // Each app's processes are looked for once a scan, however many devices name it.
        let mut asked: BTreeMap<Vec<String>, bool> = BTreeMap::new();
        let mut conflicts = Vec::new();
        for f in &now {
            for c in &f.conflicts {
                let runs = *asked.entry(c.processes.clone()).or_insert_with(|| {
                    let names: Vec<&str> = c.processes.iter().map(String::as_str).collect();
                    running(&names)
                });
                if runs {
                    conflicts.push(Conflicting {
                        app: c.app.clone(),
                        device: f.name.clone(),
                        key: f.key.clone(),
                        hint: c.hint.clone(),
                    });
                }
            }
        }
        self.conflicts = conflicts;
        self.unknown = pcl::unknown(&self.catalog, hid);
        self.keyboard_lamps = pcl::keyboard_lamp_arrays(hid);
        // The keyboard's LampArray went (unplugged, or another keyboard): let it go.
        if self
            .keyboard_lamp
            .as_ref()
            .is_some_and(|k| self.keyboard_lamp_array(&k.key).is_none())
        {
            self.keyboard_lamp = None;
        }
        // A LampArray not read yet: read them all once (Windows), then reuse.
        let unread = now.iter().any(|f| {
            f.control == Control::LampArray && !self.lamp_info.contains_key(&(f.vid, f.pid))
        });
        if unread {
            for desc in (self.lamp_arrays)() {
                self.lamp_info.insert((desc.vid, desc.pid), desc);
            }
        }
        // Its lamps are its zones (#691): one each, named by purpose, at its position.
        for f in &mut now {
            if let (Control::LampArray, Some(desc)) =
                (f.control, self.lamp_info.get(&(f.vid, f.pid)))
            {
                f.caps = LightCaps::from_lamp_array(desc.info);
                f.zones = pcl::lamp_array::lamp_zones(&desc.lamps);
                f.size = desc.size;
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
            if !arrived || f.control == Control::NotYet {
                continue;
            }
            let opened = match f.control {
                // A Dynamic Lighting device, its lamps read: the host's handle on it (#691).
                Control::LampArray if f.zones.is_empty() => continue,
                Control::LampArray => match (self.lamp_opener)(f) {
                    Some(handle) => pcl::open_lamp_array(f, handle),
                    None => continue,
                },
                Control::OpenRgb => {
                    let source = self.openrgb.sources.iter().find(|s| s.key == f.key);
                    match (&self.openrgb.client, source) {
                        (Some(client), Some(s)) => openrgb::open(Arc::clone(client), s),
                        _ => continue,
                    }
                }
                _ => (self.writers)(&f.path).and_then(|w| pcl::open(f, w)),
            };
            match opened {
                Ok(driver) => {
                    self.drivers.insert(f.key.clone(), driver);
                    let fought = self.conflicts.iter().any(|c| c.key == f.key);
                    if restore && !fought && !paused {
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
        if saved.is_some_and(|s| s.match_keyboard) {
            if let Some(colors) = self.follow.and_then(|k| k.colors(f.zones.len())) {
                return colors.into_iter().map(|c| Some(rgb(c))).collect();
            }
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
        if paused && matches!(msg, Msg::SceneFrame { .. } | Msg::LampChanged(_)) {
            return;
        }
        let writes = !matches!(
            msg,
            Msg::Scan
                | Msg::Follow(_)
                | Msg::Keyboard(_)
                | Msg::Match(_, false)
                | Msg::SaveLocal(_)
                | Msg::RemoveLocal(..)
                | Msg::SceneStart
                | Msg::SceneEnd
                | Msg::OpenRgbSettings(_)
                | Msg::OpenRgbConnect
                | Msg::OpenRgbEvent(..)
                | Msg::KeyboardLampRelease
        );
        if paused && writes {
            self.status = Some(super::pause::REFUSED.replace("the keyboard", "the lights"));
            return;
        }
        match msg {
            Msg::Scan => {}
            Msg::OpenRgbSettings(settings) => {
                if settings != self.openrgb.settings {
                    self.openrgb.settings = settings;
                    self.openrgb.backoff = None;
                    self.openrgb_dirty = true;
                    self.openrgb_connect(true);
                }
            }
            Msg::OpenRgbConnect => self.openrgb_connect(false),
            Msg::OpenRgbEvent(conn, event) => self.openrgb_event(conn, event),
            Msg::Set { key, colors } => {
                if let Some(line) = self.set_colors(&key, colors) {
                    eprintln!("{line}");
                }
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
            Msg::LampChanged(key) => self.lamp_changed(&key),
            Msg::KeyboardLamp {
                key,
                look,
                calibration,
                animated,
            } => self.keyboard_lamp_look(&key, look, calibration, animated),
            Msg::KeyboardLampRelease => {
                if let Some(k) = self.keyboard_lamp.take() {
                    eprintln!(
                        "Foxglove: {}: Windows let go of the keyboard's lights; its Dynamic \
                         Lighting device is let go",
                        k.name
                    );
                }
            }
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
                // A scene holds the lights: its end puts the matched colours on.
                if on && self.scene.is_none() {
                    self.match_keyboard(&key);
                }
            }
            // Superseded by the keyboard's report (#616); see the variant.
            Msg::Follow(_) => {}
            Msg::Keyboard(k) => {
                // Paused: not taken, so the same report applies once Foxglove resumes.
                if paused || self.follow == Some(k) {
                    return;
                }
                self.follow = Some(k);
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
                    self.match_keyboard(&key);
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
                let mut device = CatalogDevice {
                    local: true,
                    ..*device
                };
                // Your own entry for a bundled board keeps what the bundled one knows of it:
                // its kind, the apps that fight Foxglove over it and its calibration hint.
                if let Some(b) = self.bundled.find(device.vid, device.pid) {
                    if device.kind == pcl::DeviceKind::Other {
                        device.kind = b.kind;
                    }
                    if device.conflicts.is_empty() {
                        device.conflicts = b.conflicts.clone();
                    }
                    if device.calibration_hint.is_none() {
                        device.calibration_hint = b.calibration_hint.clone();
                    }
                }
                self.local.put(device);
                self.save_local();
            }
            Msg::RemoveLocal(vid, pid) => {
                self.local.devices.retain(|d| (d.vid, d.pid) != (vid, pid));
                self.status = Some(format!("{vid:04x}:{pid:04x}: your entry removed"));
                self.save_local();
            }
        }
    }

    /// Windows gave a Dynamic Lighting device back (#691): the colours Foxglove set on it go
    /// on again, unsaved (they are saved already), in case another app drew on it meanwhile.
    /// Nothing while a scene holds it (the scene's writer puts its frame back, and its end puts
    /// these back) or while Windows still keeps it from Foxglove.
    fn lamp_changed(&mut self, key: &str) {
        if self.scene.is_some() {
            return;
        }
        if self.keyboard_lamp.as_ref().is_some_and(|k| k.key == key) {
            self.keyboard_lamp_again();
            return;
        }
        let Some(d) = self.drivers.get_mut(key) else {
            return;
        };
        if !d.lamp_access().is_some_and(|a| a.available && a.enabled) {
            return;
        }
        let colors: Vec<(usize, Rgb)> = d
            .colors()
            .into_iter()
            .enumerate()
            .filter_map(|(i, c)| Some((i, c?)))
            .collect();
        if colors.is_empty() {
            return;
        }
        let result = d.set_zones(&colors);
        if let Err(e) = &result {
            eprintln!(
                "Foxglove: {}: colours back after Windows gave it back: {e}",
                d.name()
            );
        }
        self.lamp_applied(key, &result);
    }

    /// Notes how a write to a Dynamic Lighting device went, for the health view (#691).
    fn lamp_applied(&mut self, key: &str, result: &foxglove_devices::Result<()>) {
        if !self
            .drivers
            .get(key)
            .is_some_and(|d| d.lamp_access().is_some())
        {
            return;
        }
        let apply = match result {
            Ok(()) => super::lamp_health::Apply::Ok,
            Err(e) => super::lamp_health::Apply::Failed(e.to_string()),
        };
        self.lamp_applies.insert(key.to_string(), apply);
    }

    /// The user's colours on some zones (the page's Apply, the API's `pc_lighting.set_zone`),
    /// saved; gives the log line (#617).
    fn set_colors(&mut self, key: &str, colors: Vec<(usize, [u8; 3])>) -> Option<String> {
        let colors: Vec<_> = colors.into_iter().map(|(i, c)| (i, rgb(c))).collect();
        let before = self.shown_colors(key);
        self.scene_keeps(key, &colors);
        self.write(key, "Colours set", |d| d.set_zones(&colors), true);
        self.set_line(key, &before, &colors)
    }

    /// Each zone's colour as the page shows it: what Foxglove set, else what was saved.
    fn shown_colors(&self, key: &str) -> Vec<Option<[u8; 3]>> {
        self.snapshot_row(key)
            .map(|r| r.zones.into_iter().map(|(_, c)| c).collect())
            .unwrap_or_default()
    }

    fn snapshot_row(&self, key: &str) -> Option<Row> {
        self.snapshot().devices.into_iter().find(|r| r.key == key)
    }

    /// The log line for a zone write (#617): each zone old → new, and how it went.
    fn set_line(
        &self,
        key: &str,
        before: &[Option<[u8; 3]>],
        colors: &[(usize, Rgb)],
    ) -> Option<String> {
        let row = self.snapshot_row(key)?;
        let word = |c: Option<[u8; 3]>| c.map_or_else(|| "unset".to_string(), hex);
        let zones: Vec<String> = colors
            .iter()
            .map(|&(i, c)| {
                let name = row.zones.get(i).map_or("?", |(n, _)| n.as_str());
                let id = row.zone_ids.get(i).map_or("?", String::as_str);
                format!(
                    "zone {id} ({name}) {} → {}",
                    word(before.get(i).copied().flatten()),
                    hex([c.r, c.g, c.b])
                )
            })
            .collect();
        Some(format!(
            "Foxglove: zone colours of {} ({key}): {}; {}",
            row.name,
            zones.join(", "),
            self.status.as_deref().unwrap_or("no status")
        ))
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
                && self
                    .follow
                    .is_some_and(|k| k.colors(f.zones.len()).is_some());
            // Following the keyboard: what it shows now. Else the colours from before (white
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
        // The scene drew the held keyboard through Windows too (#749 b): the card's pick again.
        self.keyboard_lamp_again();
    }

    /// The keyboard's own LampArray Windows lists for a pairing key (`vvvv:pppp`, #749 b).
    fn keyboard_lamp_array(&self, key: &str) -> Option<&pcl::KeyboardLampArray> {
        self.keyboard_lamps
            .iter()
            .find(|a| key == format!("{:04x}:{:04x}", a.vid, a.pid))
    }

    /// The Keyboard card's pick while Windows Dynamic Lighting holds the keyboard's lights
    /// (#749 b): through the keyboard's own LampArray, opened on the first pick like any
    /// Dynamic Lighting device. An effect the scene engine animates (`animated`, #768:
    /// `scene_card` draws it on the scene tick) lets the steady handle go, so the two never
    /// draw over each other. Otherwise an effect that shows the colour shows it steady (as
    /// "Match keyboard" copies it); a rainbow with nothing to draw it has no colour to show,
    /// so the lamps keep theirs and the status says why. A running scene draws the keyboard
    /// meanwhile: the pick is kept and shows at its end. The colour goes through the keyboard's
    /// calibration (`calibration`, its key).
    fn keyboard_lamp_look(
        &mut self,
        key: &str,
        look: Option<KeyboardLights>,
        calibration: Option<String>,
        animated: bool,
    ) {
        if let Some(l) = look.filter(|_| animated) {
            let Some(name) = self.keyboard_lamp_array(key).map(|a| a.name.clone()) else {
                self.status = Some("The keyboard isn't listed under Dynamic Lighting now".into());
                return;
            };
            self.keyboard_lamp = None;
            self.status = Some(if self.scene.is_some() {
                format!("{name}: {SCENE_FIRST}")
            } else {
                format!(
                    "{name}: {} runs through Windows Dynamic Lighting",
                    l.effect_label()
                )
            });
            return;
        }
        if self.keyboard_lamp.as_ref().is_none_or(|k| k.key != key) {
            // Another keyboard's first: its devices are let go before these are claimed.
            self.keyboard_lamp = None;
            let Some(a) = self.keyboard_lamp_array(key).cloned() else {
                self.status = Some("The keyboard isn't listed under Dynamic Lighting now".into());
                return;
            };
            let found = Found {
                key: key.to_string(),
                name: a.name.clone(),
                vid: a.vid,
                pid: a.pid,
                control: Control::LampArray,
                path: String::new(),
                zones: Vec::new(),
                caps: LightCaps::default(),
                local: false,
                calibration: None,
                calibration_hint: None,
                size: None,
                kind: pcl::DeviceKind::default(),
                conflicts: Vec::new(),
            };
            let Some(handle) = (self.lamp_opener)(&found) else {
                self.status = Some(format!(
                    "{}: Windows has no Dynamic Lighting device for it now",
                    a.name
                ));
                return;
            };
            self.keyboard_lamp = Some(KeyboardLamp {
                key: key.to_string(),
                name: a.name,
                handle,
                shown: None,
                calibration: None,
            });
        }
        if let Some(k) = self.keyboard_lamp.as_mut() {
            k.calibration = calibration;
        }
        let colour = match look {
            None => pcl::BLACK,
            Some(l) => match l.colors(1) {
                Some(c) => rgb(c[0]),
                None => {
                    let name = self.keyboard_lamp.as_ref().map_or("", |k| &k.name);
                    self.status = Some(format!(
                        "{name}: {} is a rainbow the keyboard makes itself; through Windows \
                         Foxglove shows one colour, so the lights keep theirs",
                        l.effect_label()
                    ));
                    return;
                }
            },
        };
        let Some(k) = self.keyboard_lamp.as_mut() else {
            return;
        };
        if self.scene.is_some() {
            // The scene draws the keyboard through Windows (#761): its end puts this back.
            k.shown = Some(colour);
            self.status = Some(format!("{}: {SCENE_FIRST}", k.name));
            return;
        }
        let sent = k.sent(colour);
        let result = k.handle.set_all(sent);
        let available = k.handle.access().available;
        if result.is_ok() {
            k.shown = Some(colour);
        }
        let name = k.name.clone();
        self.status = Some(match &result {
            Ok(()) if available => format!("{name}: shown through Windows Dynamic Lighting"),
            Ok(()) => format!(
                "{name}: set through Windows Dynamic Lighting; it shows while Foxglove's window \
                 is in front"
            ),
            Err(e) => format!("{name}: {e}"),
        });
        let apply = match result {
            Ok(()) => super::lamp_health::Apply::Ok,
            Err(e) => super::lamp_health::Apply::Failed(e.to_string()),
        };
        self.lamp_applies.insert(key.to_string(), apply);
    }

    /// The card's pick on the keyboard's LampArray again (Windows gave it back, a scene
    /// ended), while Windows lets Foxglove drive it.
    fn keyboard_lamp_again(&mut self) {
        let Some(k) = self.keyboard_lamp.as_mut() else {
            return;
        };
        let Some(colour) = k.shown else {
            return;
        };
        let access = k.handle.access();
        if !(access.available && access.enabled) {
            return;
        }
        let sent = k.sent(colour);
        if let Err(e) = k.handle.set_all(sent) {
            eprintln!(
                "Foxglove: {}: colour back through Dynamic Lighting: {e}",
                k.name
            );
        }
    }

    /// Shows on one device what the keyboard last reported ("Match keyboard"), or says why it
    /// can't (no report yet; an effect that ignores the colour), leaving its colours as they
    /// are. Never the colour the keyboard was sent but doesn't show (#616).
    fn match_keyboard(&mut self, key: &str) {
        let Some(k) = self.follow else {
            self.status =
                Some("Match keyboard: waiting for the keyboard to report its lights".into());
            return;
        };
        let Some(d) = self.drivers.get(key) else {
            return;
        };
        let Some(colors) = k.colors(d.zones().len()) else {
            self.status = Some(format!(
                "Match keyboard: the keyboard plays {}, which {} can't match from Foxglove, so \
                 it keeps its colours",
                k.effect_label(),
                d.name()
            ));
            return;
        };
        let colors: Vec<(usize, Rgb)> = colors.into_iter().map(rgb).enumerate().collect();
        self.write(
            key,
            "Matching the keyboard",
            |d| d.set_zones(&colors),
            false,
        );
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
        self.lamp_applied(key, &result);
        let Some(d) = self.drivers.get_mut(key) else {
            return;
        };
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
                    calibration_hint: f.calibration_hint,
                    calibrated: foxglove_devices::calibration::global()
                        .device(&f.key)
                        .is_some(),
                    size: f.size,
                    zone_positions: f.zones.iter().map(|z| z.position).collect(),
                    zone_roles: f.zones.iter().map(|z| z.role).collect(),
                    kind: f.kind,
                    lamp_access: self.drivers.get(&f.key).and_then(|d| d.lamp_access()),
                }
            })
            .collect();
        Snapshot {
            devices,
            conflicts: self.conflicts.clone(),
            status: self.status.clone(),
            busy: false,
            unknown: self.unknown.clone(),
            local_file: self.local_path.as_ref().map(|p| p.display().to_string()),
            lamp_applies: self.lamp_applies.clone(),
            keyboard_lamp_arrays: self.keyboard_lamps.clone(),
            openrgb: OpenRgbView {
                settings: self.openrgb.settings.clone(),
                connected: self.openrgb.client.is_some(),
                status: self
                    .openrgb
                    .settings
                    .enabled
                    .then(|| self.openrgb.status.clone())
                    .flatten(),
            },
        }
    }
}

/// The Dynamic Lighting devices' health now (#691), for the shared health view: found,
/// controllable, what Windows lets Foxglove do, and the last apply. Read from the worker's
/// snapshot; asks nothing of the devices.
pub fn lamp_health() -> Vec<super::lamp_health::LampHealth> {
    let s = snapshot();
    super::lamp_health::from_snapshot(&s, &s.lamp_applies, super::scene_lamps::identity())
}

/// Starts the worker, the HID arrival watch and the first scan.
pub fn start() {
    if super::repaint_trace::disabled("pc-lights") {
        return;
    }
    let (tx, rx) = mpsc::channel::<Msg>();
    if QUEUE.set(Mutex::new(tx.clone())).is_err() {
        return;
    }
    // Supervised (#587): the queue outlives a panicking attempt, so `QUEUE` stays good and
    // nothing queued is lost; a restart reloads the settings and scans again.
    let rx = Mutex::new(rx);
    let _ = super::services::spawn_supervised("pc-lights", move || {
        let rx = rx.lock().unwrap_or_else(|e| e.into_inner());
        let settings = super::store::try_load::<Settings>("pc_lighting").unwrap_or_default();
        // Off unless the user turned it on (#471).
        let openrgb_settings =
            super::store::try_load::<openrgb::Settings>(OPENRGB_SECTION).unwrap_or_default();
        #[cfg(target_os = "windows")]
        let _watch = {
            let tx = tx.clone();
            super::device_watch::DeviceWatch::interfaces(
                true,
                Box::new(move |_path| {
                    let _ = ticketed(&tx, Msg::Scan);
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
        )
        .with_lamp_opener({
            // Windows' AvailabilityChanged, on its thread: one message to this queue.
            let tx = tx.clone();
            super::scene_lamps::worker_opener(std::sync::Arc::new(move |key: &str| {
                let _ = ticketed(&tx, Msg::LampChanged(key.to_string()));
                // And a held scene tries the keyboard once (#749): Windows may have let go.
                super::lights_owner::retry(
                    "Windows changed a Dynamic Lighting device's availability",
                );
            }))
        })
        .with_openrgb(openrgb_settings, {
            // OpenRGB's push, on its reader thread: one message to this queue.
            let tx = Mutex::new(tx.clone());
            Arc::new(move |conn, event| {
                let tx = tx.lock().unwrap_or_else(|e| e.into_inner());
                let _ = ticketed(&tx, Msg::OpenRgbEvent(conn, event));
            })
        });
        let _ = ticketed(&tx, Msg::Scan);
        if worker.openrgb_settings().enabled {
            let _ = ticketed(&tx, Msg::OpenRgbConnect);
        }
        while let Ok(first) = rx.recv() {
            // A burst of arrivals (a hub with several interfaces) scans once.
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            // Counted when this round ends, also by a panic (the supervisor restarts it).
            let _handled = Handled(batch.len() as u64);
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
            if std::mem::take(&mut worker.relist) {
                scanned = true;
                worker.relist(&mut |names| process_running(names), super::pause::paused());
            }
            if std::mem::take(&mut worker.openrgb_dirty) {
                if let Err(e) = super::store::save(OPENRGB_SECTION, worker.openrgb_settings()) {
                    eprintln!("Foxglove: can't save the OpenRGB settings: {e}");
                }
            }
            // The user asked to try again after OpenRGB goes: one bounded wait per try, on the
            // crate's backoff (at most a few, then it stops).
            if let Some(wait) = worker.openrgb_retry.take() {
                let tx = tx.clone();
                let _ = super::services::spawn_named("openrgb-retry", move || {
                    std::thread::sleep(wait);
                    let _ = ticketed(&tx, Msg::OpenRgbConnect);
                });
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

/// The store's section for the OpenRGB settings (#471).
pub const OPENRGB_SECTION: &str = "openrgb";

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
    // Unit tests see no real lights (#667).
    let found = if cfg!(test) {
        Ok(Vec::new())
    } else {
        pcl::hid::enumerate()
    };
    match found {
        Ok(hid) => worker.scan(
            &hid,
            &mut |names| process_running(names),
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

/// Every LampArray's attributes and lamps, through Windows' Dynamic Lighting API: lamp count,
/// each lamp's position, purposes and whether it takes a colour, the update interval and kind.
/// Read once per new LampArray on a scan; nothing is written to it. Each one's shape is kept
/// for [`lamp_layouts`].
#[cfg(target_os = "windows")]
pub fn lamp_arrays() -> Vec<LampArrayDesc> {
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Lights::LampArray;
    let read = || -> windows::core::Result<Vec<LampArrayDesc>> {
        let selector = LampArray::GetDeviceSelector()?;
        let list = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()?;
        let mut out = Vec::new();
        for info in &list {
            let Ok(lamps) = LampArray::FromIdAsync(&info.Id()?).and_then(|op| op.join()) else {
                continue;
            };
            let count = lamps.LampCount().unwrap_or(0).max(0);
            let infos: Vec<_> = (0..count.min(512))
                .map(|i| lamps.GetLampInfo(i).ok())
                .collect();
            // Windows gives metres.
            let at: Vec<_> = infos
                .iter()
                .map(|l| l.as_ref().and_then(|l| l.Position().ok()))
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
            let mm = |m: f32| (m * 1000.0).round().clamp(0.0, f32::from(u16::MAX)) as u16;
            let lamp_list = infos
                .iter()
                .zip(&at)
                .map(|(l, p)| pcl::Lamp {
                    position: positions.then(|| p.map_or([0; 2], |p| [mm(p.X), mm(p.Y)])),
                    purposes: l
                        .as_ref()
                        .and_then(|l| l.Purposes().ok())
                        .map_or(0, |p| p.0),
                    colour: l.as_ref().is_some_and(|l| {
                        let levels = [l.RedLevelCount(), l.GreenLevelCount(), l.BlueLevelCount()];
                        let fixed = l.FixedColor().and_then(|c| c.Value()).is_ok();
                        !fixed && levels.into_iter().any(|n| n.unwrap_or(0) > 0)
                    }),
                })
                .collect();
            out.push(LampArrayDesc {
                vid,
                pid,
                info: LampArrayInfo {
                    lamp_count: count as u32,
                    positions,
                    // TimeSpan counts 100 ns.
                    min_update_us: lamps
                        .MinUpdateInterval()
                        .map_or(0, |t| (t.Duration.max(0) / 10) as u64),
                    kind,
                },
                size: (size_mm[0] > 0.0 && size_mm[1] > 0.0)
                    .then(|| [mm(size_mm[0] / 1000.0), mm(size_mm[1] / 1000.0)]),
                lamps: lamp_list,
            });
        }
        Ok(out)
    };
    read().unwrap_or_else(|e| {
        eprintln!("Foxglove: LampArray attributes: {e}");
        Vec::new()
    })
}

#[cfg(not(target_os = "windows"))]
pub fn lamp_arrays() -> Vec<LampArrayDesc> {
    Vec::new()
}

/// Whether a process with one of `names` runs now (a catalog conflict): one process snapshot,
/// taken only with a scan.
#[cfg(target_os = "windows")]
fn process_running(names: &[&str]) -> bool {
    any_process_running(names)
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
fn process_running(_names: &[&str]) -> bool {
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
        let lamps: LampArrays = Box::new(|| vec![g502()]);
        (
            Worker::new(Catalog::bundled(), local, settings, writers, lamps),
            sent,
        )
    }

    /// The mouse as Windows reports it: a mouse-kind array of 2 lamps, a logo and an accent.
    fn g502() -> LampArrayDesc {
        use foxglove_devices::pc_lighting::lamp_array::purpose;
        LampArrayDesc {
            vid: 0x046d,
            pid: 0xc332,
            info: LampArrayInfo {
                lamp_count: 2,
                positions: true,
                min_update_us: 33_000,
                kind: 2,
            },
            size: Some([68, 132]),
            lamps: vec![
                pcl::Lamp {
                    position: Some([34, 90]),
                    purposes: purpose::BRANDING,
                    colour: true,
                },
                pcl::Lamp {
                    position: Some([34, 30]),
                    purposes: purpose::ACCENT,
                    colour: true,
                },
            ],
        }
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

    /// No conflicting app runs.
    fn nothing() -> impl FnMut(&[&str]) -> bool {
        |_| false
    }

    /// The 45L's vendor tool runs (the bundled catalog's conflict, by its exe name).
    fn studio() -> impl FnMut(&[&str]) -> bool {
        |names: &[&str]| {
            names
                .iter()
                .any(|n| n.eq_ignore_ascii_case("lightstudio.exe"))
        }
    }

    fn no_wait() -> impl FnMut(std::time::Duration) {
        |_| {}
    }

    /// #617: a zone write logs one line, each zone old → new and the result; the saved
    /// colour is the old one before Foxglove wrote anything.
    #[test]
    fn a_zone_write_logs_old_and_new_colours() {
        let mut settings = Settings::default();
        settings.devices.insert(
            "103c:84fd".into(),
            Saved {
                colors: [("1".to_string(), "#a57500".to_string())].into(),
                match_keyboard: false,
            },
        );
        let (mut w, _sent) = worker(settings);
        w.scan(&case_hid(), &mut nothing(), false, true);
        let line = w.set_colors("103c:84fd", vec![(0, [0xff, 0, 0]), (1, [0, 0, 0xff])]);
        let line = line.unwrap();
        assert!(
            line.starts_with(
                "Foxglove: zone colours of HP OMEN 45L case lights (103c:84fd): zone 1 ("
            ),
            "{line}"
        );
        assert!(line.contains(") #a57500 → #ff0000, zone "), "{line}");
        assert!(line.contains("unset → #0000ff"), "{line}");
        assert!(
            line.ends_with("; HP OMEN 45L case lights: colours set"),
            "{line}"
        );
        assert_eq!(
            w.settings.devices["103c:84fd"].colors["1"], "#ff0000",
            "saved as the user's"
        );
        // Not found: no line, nothing saved.
        assert_eq!(w.set_colors("dead:beef", vec![(0, [1, 2, 3])]), None);
    }

    #[test]
    fn the_page_lists_each_device_and_how_it_is_controlled() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), &mut studio(), false, true);
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
                ("G502 Proteus Spectrum", Control::LampArray, 2),
            ]
        );
        let names: Vec<_> = s
            .conflicts
            .iter()
            .map(|c| (c.app.as_str(), c.device.as_str(), c.key.as_str()))
            .collect();
        assert_eq!(
            names,
            [("OMEN Light Studio", "HP OMEN 45L case lights", "103c:84fd")],
            "the catalog's conflict for the 45L"
        );
        assert!(s.conflicts[0]
            .hint
            .as_deref()
            .is_some_and(|h| h.contains("Uninstall")));
        assert!(
            sent.lock().unwrap().is_empty(),
            "a scan alone writes nothing"
        );
    }

    #[test]
    fn a_colour_pick_writes_its_zones_and_is_saved() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), &mut nothing(), false, true);
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
    fn saved_colours_come_back_on_arrival_unless_a_conflicting_app_runs() {
        let mut settings = Settings::default();
        settings.devices.insert(
            "103c:84fd".into(),
            Saved {
                colors: [("2".to_string(), "#112233".to_string())].into(),
                match_keyboard: false,
            },
        );
        let (mut w, sent) = worker(settings.clone());
        w.scan(&case_hid(), &mut studio(), false, true);
        assert!(sent.lock().unwrap().is_empty(), "Light Studio is running");
        let (mut w, sent) = worker(settings);
        w.scan(&case_hid(), &mut nothing(), false, true);
        assert_eq!(*sent.lock().unwrap(), vec![report(2, [0x11, 0x22, 0x33])]);
        w.scan(&case_hid(), &mut nothing(), false, true);
        assert_eq!(
            sent.lock().unwrap().len(),
            1,
            "still there: no second write"
        );
        w.scan(&[], &mut nothing(), false, true);
        w.scan(&case_hid(), &mut nothing(), false, true);
        assert_eq!(sent.lock().unwrap().len(), 2, "replugged: put back again");
    }

    /// The keyboard reports a solid colour at full saturation and brightness.
    fn solid(hue: u16) -> KeyboardLights {
        KeyboardLights {
            hue,
            sat: 100,
            brightness: 100,
            effect: 0,
        }
    }

    fn grey() -> KeyboardLights {
        KeyboardLights {
            hue: 0,
            sat: 0,
            brightness: 50,
            effect: 0,
        }
    }

    /// #616: the effect table says which effects show the chosen colour, and that decides what
    /// "Match keyboard" can copy.
    #[test]
    fn the_effect_table_decides_what_match_keyboard_copies() {
        use super::super::lights_page::{effect, EFFECTS};
        assert_eq!(EFFECTS, ["Solid", "Breathe", "Spectrum", "Swirl"]);
        let uses: Vec<bool> = (0..4).map(|n| effect(n).unwrap().uses_colour()).collect();
        assert_eq!(uses, [true, true, false, false]);
        assert_eq!(
            effect(3).unwrap().label(),
            "Swirl (rainbow; colour not used yet)"
        );
        assert_eq!(
            effect(2).unwrap().label(),
            "Spectrum (rainbow; colour not used yet)"
        );
        assert_eq!(effect(0).unwrap().label(), "Solid");
        // The match decision.
        let breathe = KeyboardLights {
            effect: 1,
            ..solid(240)
        };
        assert_eq!(
            breathe.colors(3),
            Some(vec![[0, 0, 255]; 3]),
            "its colour, steady"
        );
        for effect in [2, 3, 9] {
            assert_eq!(
                KeyboardLights {
                    effect,
                    ..solid(40)
                }
                .colors(5),
                None,
                "{effect}"
            );
        }
        // From the device layer's state (ZMK numbers its effects).
        let state = foxglove_devices::LightingState {
            on: true,
            hue: 40,
            sat: 100,
            brightness: 100,
            effect: foxglove_devices::Effect {
                id: "3".into(),
                params: vec![],
            },
            speed: 3,
            settled: true,
        };
        assert_eq!(
            KeyboardLights::from_state(&state),
            Some(KeyboardLights {
                effect: 3,
                ..solid(40)
            })
        );
    }

    /// #616, the sitting: the keyboard plays Swirl while its accent push failed. "Match
    /// keyboard" gave the case the accent (gold); now the accent is ignored, the case keeps its
    /// colours and the page says it can't match Swirl, and a solid colour is matched.
    #[test]
    fn match_keyboard_follows_the_keyboards_report_not_the_accent() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), &mut nothing(), false, true);
        let key = "103c:84fd".to_string();
        // Matching before any report: nothing written, and the page says why.
        w.handle(Msg::Match(key.clone(), true), false, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty());
        assert!(w.status.as_deref().unwrap_or_default().contains("waiting"));
        // The accent sent toward the keyboard isn't what the keyboard shows.
        w.handle(Msg::Follow([0x9c, 0x6f, 0x05]), false, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty(), "the accent isn't followed");
        let swirl = KeyboardLights {
            hue: 40,
            sat: 100,
            brightness: 100,
            effect: 3,
        };
        w.handle(Msg::Keyboard(swirl), false, &mut no_wait());
        assert!(
            sent.lock().unwrap().is_empty(),
            "nothing to copy from a rainbow"
        );
        let status = w.status.clone().unwrap_or_default();
        assert!(
            status.contains("plays Swirl, which")
                && !status.contains("colour not used yet")
                && status.contains("can't match"),
            "{status}"
        );
        // The keyboard goes back to a solid colour: so does the case, on every zone.
        w.handle(Msg::Keyboard(solid(240)), false, &mut no_wait());
        let wrote = sent.lock().unwrap().clone();
        assert_eq!(wrote.len(), 5);
        for (r, z) in wrote.iter().zip([1, 2, 5, 6, 7]) {
            assert_eq!(*r, report(z, [0, 0, 255]), "zone {z}");
        }
    }

    #[test]
    fn match_keyboard_follows_colour_changes_only() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), &mut nothing(), false, true);
        let key = "103c:84fd".to_string();
        w.handle(Msg::Keyboard(solid(0)), false, &mut no_wait());
        assert!(sent.lock().unwrap().is_empty(), "not matching yet");
        w.handle(Msg::Match(key.clone(), true), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 5, "the current colour, once");
        assert_eq!(sent.lock().unwrap()[4], report(7, [255, 0, 0]));
        w.handle(Msg::Keyboard(solid(0)), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 5, "same colour: nothing");
        w.handle(Msg::Keyboard(solid(240)), false, &mut no_wait());
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
        w.handle(Msg::Keyboard(solid(120)), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 11);
        // Paused: the colour waits, and comes through once resumed.
        w.handle(Msg::Match(key.clone(), true), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 16);
        w.handle(Msg::Keyboard(grey()), true, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 16);
        w.handle(Msg::Keyboard(grey()), false, &mut no_wait());
        assert_eq!(sent.lock().unwrap().len(), 21);
    }

    #[test]
    fn off_identify_and_pause() {
        let (mut w, sent) = worker(Settings::default());
        w.scan(&case_hid(), &mut nothing(), false, true);
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
        w.scan(&case_hid(), &mut studio(), false, true);
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
        w.scan(&hid, &mut nothing(), false, true);
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
        w.scan(&case_hid(), &mut nothing(), false, true);
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

    /// What a fake Dynamic Lighting device was asked: `None` index = every lamp.
    type LampCalls = Arc<Mutex<Vec<(String, Option<usize>, [u8; 3])>>>;

    /// A fake LampArray of any vendor: records each write under its key.
    struct FakeLamps {
        key: String,
        count: usize,
        calls: LampCalls,
        access: Arc<Mutex<pcl::LampAccess>>,
    }

    impl pcl::LampHandle for FakeLamps {
        fn lamp_count(&self) -> usize {
            self.count
        }
        fn access(&self) -> pcl::LampAccess {
            *self.access.lock().unwrap()
        }
        fn set_all(&mut self, c: Rgb) -> foxglove_devices::Result<()> {
            let call = (self.key.clone(), None, [c.r, c.g, c.b]);
            self.calls.lock().unwrap().push(call);
            Ok(())
        }
        fn set_lamps(&mut self, lamps: &[(usize, Rgb)]) -> foxglove_devices::Result<()> {
            let mut calls = self.calls.lock().unwrap();
            for &(i, c) in lamps {
                calls.push((self.key.clone(), Some(i), [c.r, c.g, c.b]));
            }
            Ok(())
        }
    }

    /// Opens a fake for every Dynamic Lighting device, sized by its lamps.
    fn fake_lamps(calls: &LampCalls) -> LampOpener {
        fake_lamps_with(calls, &Arc::default())
    }

    /// The same, with what Windows lets Foxglove do with them set by the test.
    fn fake_lamps_with(calls: &LampCalls, access: &Arc<Mutex<pcl::LampAccess>>) -> LampOpener {
        let (calls, access) = (calls.clone(), access.clone());
        Box::new(move |f: &Found| {
            Some(Box::new(FakeLamps {
                key: f.key.clone(),
                count: f.zones.len(),
                calls: calls.clone(),
                access: access.clone(),
            }) as Box<dyn pcl::LampHandle>)
        })
    }

    /// A Dynamic Lighting device whose writes fail, as Windows' would for a device gone.
    struct BrokenLamps(usize);

    impl pcl::LampHandle for BrokenLamps {
        fn lamp_count(&self) -> usize {
            self.0
        }
        fn set_all(&mut self, _: Rgb) -> foxglove_devices::Result<()> {
            Err(DeviceError::Backend("the device is gone".into()))
        }
        fn set_lamps(&mut self, _: &[(usize, Rgb)]) -> foxglove_devices::Result<()> {
            Err(DeviceError::Backend("the device is gone".into()))
        }
    }

    /// #691's health signal: each Dynamic Lighting device reports found, controllable, what
    /// Windows lets Foxglove do and how the last apply went, and the rows change when they do.
    #[test]
    fn the_health_rows_follow_each_lamp_arrays_state() {
        use crate::foxglove::lamp_health::{from_snapshot, rows, Apply, Level};
        let calls = LampCalls::default();
        let access = Arc::new(Mutex::new(pcl::LampAccess::default()));
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(fake_lamps_with(&calls, &access));
        w.scan(&case_hid(), &mut nothing(), false, true);
        let health = |w: &Worker| {
            let s = w.snapshot();
            from_snapshot(&s, &s.lamp_applies, false)
        };
        let h = health(&w);
        assert_eq!(h.len(), 1, "only the Dynamic Lighting device");
        assert_eq!(
            (h[0].lamps, h[0].controllable, h[0].last_apply.clone()),
            (2, true, None)
        );
        w.handle(Msg::Off(MOUSE.into()), false, &mut no_wait());
        assert_eq!(health(&w)[0].last_apply, Some(Apply::Ok));
        // Windows takes it: the Windows row warns.
        access.lock().unwrap().available = false;
        w.handle(Msg::LampChanged(MOUSE.into()), false, &mut no_wait());
        let r = rows(&health(&w)[0]);
        assert_eq!(r[2].level, Level::Warn, "{r:?}");
        // A device whose writes fail: the last apply fails, with the reason.
        let (w2, _) = worker(Settings::default());
        let mut w2 = w2.with_lamp_opener(Box::new(|f: &Found| {
            Some(Box::new(BrokenLamps(f.zones.len())) as Box<dyn pcl::LampHandle>)
        }));
        w2.scan(&case_hid(), &mut nothing(), false, true);
        w2.handle(Msg::Off(MOUSE.into()), false, &mut no_wait());
        let r = rows(&health(&w2)[0]);
        assert_eq!(r[3].level, Level::Fail);
        assert!(r[3].detail.contains("the device is gone"), "{r:?}");
        // The case board isn't a Dynamic Lighting device: no rows, no apply noted.
        w2.handle(Msg::Off("103c:84fd".into()), false, &mut no_wait());
        assert!(!w2.snapshot().lamp_applies.contains_key("103c:84fd"));
        // No handle: found, not controllable.
        let (mut bare, _) = worker(Settings::default());
        bare.scan(&case_hid(), &mut nothing(), false, true);
        let r = rows(&health(&bare)[0]);
        assert_eq!((r[1].level, r.len()), (Level::Warn, 3));
    }

    /// #754: a keyboard that is also a LampArray isn't a device here (its link drives it), but
    /// the snapshot names it, so the health view's Dynamic Lighting rows aren't silent on it.
    #[test]
    fn a_keyboards_lamp_array_is_named_but_not_driven() {
        let calls = LampCalls::default();
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(fake_lamps(&calls));
        w.scan(&hid_with_corne(), &mut nothing(), false, true);
        let s = w.snapshot();
        assert!(
            s.devices.iter().all(|d| d.key != "1d50:615e"),
            "not driven here"
        );
        let k: Vec<_> = s
            .keyboard_lamp_arrays
            .iter()
            .map(|k| (k.key.as_str(), k.name.as_str(), k.bluetooth))
            .collect();
        assert_eq!(k, vec![("1d50:615e", "Eyelash Corne", false)]);
        let rows = crate::foxglove::lamp_health::keyboard_rows(&s.keyboard_lamp_arrays, false);
        assert_eq!(rows[0].item, "Eyelash Corne (keyboard) · Found");
    }

    /// The case's devices and a keyboard that is also a LampArray (#754).
    fn hid_with_corne() -> Vec<HidEntry> {
        let mut hid = case_hid();
        for (page, usage, path) in [
            (1, 6, "corne-keys"),
            (0xff60, 0x61, "corne-raw"),
            (0x59, 1, "corne-lamps"),
        ] {
            hid.push(HidEntry {
                vid: 0x1d50,
                pid: 0x615e,
                usage_page: page,
                usage,
                path: path.into(),
                product: Some("Eyelash Corne".into()),
                ..Default::default()
            });
        }
        hid
    }

    /// #749 b: while Windows Dynamic Lighting holds the keyboard's lights, the Keyboard card's
    /// picks go through the keyboard's own LampArray: opened once by the same opener as any
    /// Dynamic Lighting device, a colour on every lamp, dark for off, a rainbow left alone.
    /// Windows giving it back puts the pick back; the lights' release lets it go; nothing is
    /// saved as a device's colours, and nothing is written while paused.
    #[test]
    fn a_held_keyboards_picks_go_through_its_own_lamparray() {
        let calls = LampCalls::default();
        let access = Arc::new(Mutex::new(pcl::LampAccess::default()));
        let opened = Arc::new(Mutex::new(Vec::<String>::new()));
        let opener: LampOpener = {
            let (calls, access, opened) = (calls.clone(), access.clone(), opened.clone());
            Box::new(move |f: &Found| {
                opened.lock().unwrap().push(f.key.clone());
                Some(Box::new(FakeLamps {
                    key: f.key.clone(),
                    count: f.zones.len(),
                    calls: calls.clone(),
                    access: access.clone(),
                }) as Box<dyn pcl::LampHandle>)
            })
        };
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(opener);
        w.scan(&hid_with_corne(), &mut nothing(), false, true);
        opened.lock().unwrap().clear();
        const KB: &str = "1d50:615e";
        let look = |w: &mut Worker, look: Option<KeyboardLights>| {
            w.handle(
                Msg::KeyboardLamp {
                    key: KB.into(),
                    look,
                    calibration: None,
                    animated: false,
                },
                false,
                &mut no_wait(),
            );
        };
        let drained = || calls.lock().unwrap().drain(..).collect::<Vec<_>>();
        look(&mut w, Some(solid(0)));
        assert_eq!(drained(), vec![(KB.to_string(), None, [255, 0, 0])]);
        assert_eq!(*opened.lock().unwrap(), [KB], "opened like any LampArray");
        assert_eq!(
            w.status.as_deref(),
            Some("Eyelash Corne: shown through Windows Dynamic Lighting")
        );
        // Breathe shows its colour steady; Spectrum has none to show: the lamps keep theirs.
        look(
            &mut w,
            Some(KeyboardLights {
                effect: 1,
                ..solid(240)
            }),
        );
        assert_eq!(drained(), vec![(KB.to_string(), None, [0, 0, 255])]);
        look(
            &mut w,
            Some(KeyboardLights {
                effect: 2,
                ..solid(0)
            }),
        );
        assert!(drained().is_empty());
        assert!(
            w.status
                .as_deref()
                .unwrap()
                .contains("Spectrum is a rainbow"),
            "{:?}",
            w.status
        );
        look(&mut w, None);
        assert_eq!(drained(), vec![(KB.to_string(), None, [0, 0, 0])]);
        assert_eq!(opened.lock().unwrap().len(), 1, "opened once");
        // Not in front: set, and Windows shows it when Foxglove may again (its event).
        access.lock().unwrap().available = false;
        look(&mut w, Some(solid(120)));
        assert!(w
            .status
            .as_deref()
            .unwrap()
            .contains("while Foxglove's window is in front"));
        drained();
        w.handle(Msg::LampChanged(KB.into()), false, &mut no_wait());
        assert!(drained().is_empty(), "Windows keeps it: nothing");
        access.lock().unwrap().available = true;
        w.handle(Msg::LampChanged(KB.into()), false, &mut no_wait());
        assert_eq!(drained(), vec![(KB.to_string(), None, [0, 255, 0])]);
        // Never a device row or saved colours: the keyboard keeps its own look.
        let s = w.snapshot();
        assert!(s.devices.iter().all(|d| d.key != KB));
        assert!(!w.settings.devices.contains_key(KB));
        assert_eq!(
            s.lamp_applies.get(KB),
            Some(&super::super::lamp_health::Apply::Ok)
        );
        // Paused (#404): refused.
        look(&mut w, Some(solid(0)));
        drained();
        w.handle(
            Msg::KeyboardLamp {
                key: KB.into(),
                look: None,
                calibration: None,
                animated: false,
            },
            true,
            &mut no_wait(),
        );
        assert!(drained().is_empty());
        // Windows let go: the LampArray goes; Windows giving it back later writes nothing,
        // and the next hold's pick opens it again.
        w.handle(Msg::KeyboardLampRelease, false, &mut no_wait());
        w.handle(Msg::LampChanged(KB.into()), false, &mut no_wait());
        assert!(drained().is_empty());
        look(&mut w, Some(solid(0)));
        assert_eq!(opened.lock().unwrap().len(), 2);
        // The keyboard unplugged: let go; a pick then says it isn't listed.
        drained();
        w.scan(&case_hid(), &mut nothing(), false, true);
        look(&mut w, Some(solid(0)));
        assert!(drained().is_empty());
        assert_eq!(
            w.status.as_deref(),
            Some("The keyboard isn't listed under Dynamic Lighting now")
        );
    }

    /// #749 b follow-up: the Keyboard card's pick through the keyboard's LampArray goes through
    /// the keyboard's colour calibration (its key), as its host-link colour does, also when
    /// Windows gives the lamps back; off stays dark.
    #[test]
    fn a_held_keyboards_pick_goes_through_its_calibration() {
        use foxglove_devices::calibration::{self, Calibration, DeviceCalibration};
        let cal_key = "1d50:615e:CAL749D";
        let warm = DeviceCalibration {
            device: Calibration {
                gain: [1.0, 0.5, 0.25],
                ..Default::default()
            },
            ..Default::default()
        };
        calibration::global().set_device(cal_key, Some(warm.clone()));
        let calls = LampCalls::default();
        let access = Arc::new(Mutex::new(pcl::LampAccess::default()));
        let opener: LampOpener = {
            let (calls, access) = (calls.clone(), access.clone());
            Box::new(move |f: &Found| {
                Some(Box::new(FakeLamps {
                    key: f.key.clone(),
                    count: f.zones.len(),
                    calls: calls.clone(),
                    access: access.clone(),
                }) as Box<dyn pcl::LampHandle>)
            })
        };
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(opener);
        w.scan(&hid_with_corne(), &mut nothing(), false, true);
        const KB: &str = "1d50:615e";
        let look = |w: &mut Worker, look: Option<KeyboardLights>| {
            w.handle(
                Msg::KeyboardLamp {
                    key: KB.into(),
                    look,
                    calibration: Some(cal_key.into()),
                    animated: false,
                },
                false,
                &mut no_wait(),
            );
        };
        let drained = || calls.lock().unwrap().drain(..).collect::<Vec<_>>();
        let yellow = warm.zone(None).apply_rgb(rgb([255, 255, 0]));
        let yellow = [yellow.r, yellow.g, yellow.b];
        assert_ne!(yellow, [255, 255, 0], "the calibration changes it");
        look(&mut w, Some(solid(60)));
        assert_eq!(drained(), vec![(KB.to_string(), None, yellow)]);
        // Windows gives the lamps back: the same pick, calibrated as now.
        w.handle(Msg::LampChanged(KB.into()), false, &mut no_wait());
        assert_eq!(drained(), vec![(KB.to_string(), None, yellow)]);
        look(&mut w, None);
        assert_eq!(drained(), vec![(KB.to_string(), None, [0, 0, 0])]);
        calibration::global().set_device(cal_key, None);
    }

    /// #768: an effect the scene engine animates (`scene_card`) lets the steady handle go and
    /// writes nothing here, so the two never draw over each other; the status names it. A
    /// running scene comes first: a steady pick is kept and shows at its end, and an animated
    /// one says it waits.
    #[test]
    fn an_animated_pick_lets_go_and_a_scene_comes_first() {
        let calls = LampCalls::default();
        let opened = Arc::new(Mutex::new(0));
        let opener: LampOpener = {
            let (calls, opened) = (calls.clone(), opened.clone());
            Box::new(move |f: &Found| {
                if f.key == "1d50:615e" {
                    *opened.lock().unwrap() += 1;
                }
                Some(Box::new(FakeLamps {
                    key: f.key.clone(),
                    count: f.zones.len(),
                    calls: calls.clone(),
                    access: Arc::default(),
                }) as Box<dyn pcl::LampHandle>)
            })
        };
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(opener);
        w.scan(&hid_with_corne(), &mut nothing(), false, true);
        const KB: &str = "1d50:615e";
        let look = |w: &mut Worker, look: Option<KeyboardLights>, animated: bool| {
            w.handle(
                Msg::KeyboardLamp {
                    key: KB.into(),
                    look,
                    calibration: None,
                    animated,
                },
                false,
                &mut no_wait(),
            );
        };
        // The keyboard's writes only (a scene's end puts the other devices back too).
        let drained = || {
            calls
                .lock()
                .unwrap()
                .drain(..)
                .filter(|c| c.0 == KB)
                .collect::<Vec<_>>()
        };
        look(&mut w, Some(solid(0)), false);
        assert_eq!(drained(), vec![(KB.to_string(), None, [255, 0, 0])]);
        let spectrum = KeyboardLights {
            effect: 2,
            ..solid(0)
        };
        look(&mut w, Some(spectrum), true);
        assert!(drained().is_empty(), "the scene engine draws it");
        assert_eq!(
            w.status.as_deref(),
            Some("Eyelash Corne: Spectrum runs through Windows Dynamic Lighting")
        );
        // Let go: Windows giving the lamps back puts no steady colour over the effect.
        w.handle(Msg::LampChanged(KB.into()), false, &mut no_wait());
        assert!(drained().is_empty());
        // A scene runs: a steady pick is kept for its end; an animated one waits too.
        w.handle(Msg::SceneStart, false, &mut no_wait());
        look(&mut w, Some(solid(240)), false);
        assert!(drained().is_empty(), "the scene draws the keyboard");
        let waits = "Eyelash Corne: a scene is running; this look shows when it ends";
        assert_eq!(w.status.as_deref(), Some(waits));
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        assert_eq!(drained(), vec![(KB.to_string(), None, [0, 0, 255])]);
        w.handle(Msg::SceneStart, false, &mut no_wait());
        look(&mut w, Some(spectrum), true);
        assert_eq!(w.status.as_deref(), Some(waits));
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        assert!(
            drained().is_empty(),
            "the effect comes back from scene_card"
        );
        assert_eq!(*opened.lock().unwrap(), 2, "opened again after the let-go");
    }

    /// #691: "Match keyboard" on a Dynamic Lighting device follows the keyboard's report (so
    /// the accent or wallpaper it follows), a scene holds it without the worker writing, and
    /// the scene's end puts the matched colour back.
    #[test]
    fn a_lamp_array_matches_the_keyboard_and_comes_back_after_a_scene() {
        let calls = LampCalls::default();
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(fake_lamps(&calls));
        w.scan(&case_hid(), &mut nothing(), false, true);
        w.handle(Msg::Keyboard(solid(0)), false, &mut no_wait());
        w.handle(Msg::Match(MOUSE.into(), true), false, &mut no_wait());
        let red = (MOUSE.to_string(), None, [255, 0, 0]);
        assert_eq!(
            calls.lock().unwrap().drain(..).collect::<Vec<_>>(),
            vec![red.clone()]
        );
        assert!(w.snapshot_row(MOUSE).unwrap().match_keyboard);
        // A scene takes the lights (its own writer draws on the device): the worker keeps out.
        w.handle(Msg::SceneStart, false, &mut no_wait());
        w.handle(Msg::Keyboard(solid(240)), false, &mut no_wait());
        w.handle(Msg::LampChanged(MOUSE.into()), false, &mut no_wait());
        assert!(calls.lock().unwrap().is_empty(), "the scene holds it");
        // Its end: the colour the keyboard shows now.
        w.handle(Msg::SceneEnd, false, &mut no_wait());
        let back = calls.lock().unwrap().drain(..).collect::<Vec<_>>();
        assert_eq!(back, vec![(MOUSE.to_string(), None, [0, 0, 255])]);
    }

    /// #691: Windows takes the device (Foxglove's window went to the back, another app came
    /// first) and gives it back: on its event the colours go on again, and the page knows
    /// which it is. Nothing polls; nothing is written while Windows keeps it.
    #[test]
    fn a_lamp_array_gets_its_colours_back_when_windows_gives_it_back() {
        let calls = LampCalls::default();
        let access = Arc::new(Mutex::new(pcl::LampAccess::default()));
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(fake_lamps_with(&calls, &access));
        w.scan(&case_hid(), &mut nothing(), false, true);
        let pine = [0x5f, 0xc4, 0x8e];
        w.handle(
            Msg::Set {
                key: MOUSE.into(),
                colors: vec![(0, pine)],
            },
            false,
            &mut no_wait(),
        );
        calls.lock().unwrap().clear();
        assert_eq!(
            w.snapshot_row(MOUSE).unwrap().lamp_access,
            Some(pcl::LampAccess::default())
        );
        access.lock().unwrap().available = false;
        w.handle(Msg::LampChanged(MOUSE.into()), false, &mut no_wait());
        assert!(
            calls.lock().unwrap().is_empty(),
            "Windows keeps it: nothing"
        );
        assert_eq!(
            w.snapshot_row(MOUSE)
                .unwrap()
                .lamp_access
                .map(|a| a.available),
            Some(false)
        );
        access.lock().unwrap().available = true;
        // Paused (#404): the event is dropped; the next one puts the colours back.
        w.handle(Msg::LampChanged(MOUSE.into()), true, &mut no_wait());
        assert!(calls.lock().unwrap().is_empty());
        w.handle(Msg::LampChanged(MOUSE.into()), false, &mut no_wait());
        assert_eq!(
            *calls.lock().unwrap(),
            vec![(MOUSE.to_string(), Some(0), pine)],
            "only the lamp Foxglove set"
        );
        // Turned off in Windows' settings: left alone.
        calls.lock().unwrap().clear();
        access.lock().unwrap().enabled = false;
        w.handle(Msg::LampChanged(MOUSE.into()), false, &mut no_wait());
        assert!(calls.lock().unwrap().is_empty());
        // A case board has no such state.
        assert_eq!(w.snapshot_row("103c:84fd").unwrap().lamp_access, None);
    }

    const MOUSE: &str = "046d:c332";

    /// #691: the mouse on Dynamic Lighting takes a colour on all its lamps or on one, goes
    /// dark, blinks and comes back, and its colours are saved like any device's.
    #[test]
    fn a_lamp_array_takes_colours_per_lamp_off_and_identify() {
        let calls = LampCalls::default();
        let (w, _) = worker(Settings::default());
        let mut w = w.with_lamp_opener(fake_lamps(&calls));
        w.scan(&case_hid(), &mut nothing(), false, true);
        assert!(
            calls.lock().unwrap().is_empty(),
            "nothing saved: nothing written"
        );
        let orange = [0xf0, 0x8a, 0x3c];
        w.handle(
            Msg::Set {
                key: MOUSE.into(),
                colors: vec![(0, orange), (1, orange)],
            },
            false,
            &mut no_wait(),
        );
        w.handle(
            Msg::Set {
                key: MOUSE.into(),
                colors: vec![(1, [0, 0, 255])],
            },
            false,
            &mut no_wait(),
        );
        assert_eq!(
            calls.lock().unwrap().drain(..).collect::<Vec<_>>(),
            vec![
                (MOUSE.to_string(), None, orange),
                (MOUSE.to_string(), Some(1), [0, 0, 255]),
            ],
            "one colour on every lamp is one call; one lamp is that lamp"
        );
        let row = w.snapshot_row(MOUSE).unwrap();
        assert_eq!(row.zones[0], ("Lamp 1 · Logo".to_string(), Some(orange)));
        assert_eq!(row.zones[1].1, Some([0, 0, 255]));
        assert_eq!(
            w.settings.devices[MOUSE].colors,
            BTreeMap::from([
                ("0".to_string(), "#f08a3c".to_string()),
                ("1".to_string(), "#0000ff".to_string())
            ])
        );
        assert_eq!(
            w.status.as_deref(),
            Some("G502 Proteus Spectrum: colours set")
        );
        w.handle(Msg::Off(MOUSE.into()), false, &mut no_wait());
        assert_eq!(
            calls.lock().unwrap().drain(..).collect::<Vec<_>>(),
            vec![(MOUSE.to_string(), None, [0, 0, 0])]
        );
        // Identify blinks the whole device, then puts back what it had (dark, after Off).
        w.handle(Msg::Identify(MOUSE.into()), false, &mut no_wait());
        let blink = calls.lock().unwrap().drain(..).collect::<Vec<_>>();
        assert_eq!(blink.len(), pcl::IDENTIFY_STEPS + 1);
        assert_eq!(blink[0], (MOUSE.to_string(), None, [255, 255, 255]));
        assert_eq!(*blink.last().unwrap(), (MOUSE.to_string(), None, [0, 0, 0]));
        // Paused (#404): refused, nothing written.
        w.handle(
            Msg::Set {
                key: MOUSE.into(),
                colors: vec![(0, orange)],
            },
            true,
            &mut no_wait(),
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    /// #691: a Dynamic Lighting device's saved colours go back on when it arrives, as a case
    /// board's do, and one with no handle (Windows lists none) is listed only.
    #[test]
    fn a_lamp_arrays_saved_colours_come_back_on_arrival() {
        let calls = LampCalls::default();
        let mut settings = Settings::default();
        settings.devices.insert(
            MOUSE.into(),
            Saved {
                colors: BTreeMap::from([
                    ("0".into(), "#ff0000".into()),
                    ("1".into(), "#00ff00".into()),
                ]),
                match_keyboard: false,
            },
        );
        let (w, _) = worker(settings.clone());
        let mut w = w.with_lamp_opener(fake_lamps(&calls));
        w.scan(&case_hid(), &mut nothing(), false, true);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                (MOUSE.to_string(), Some(0), [255, 0, 0]),
                (MOUSE.to_string(), Some(1), [0, 255, 0]),
            ]
        );
        // The same scan again: not an arrival, nothing written.
        w.scan(&case_hid(), &mut nothing(), false, true);
        assert_eq!(calls.lock().unwrap().len(), 2);
        // No handle: listed, not driven.
        let (mut bare, _) = worker(settings);
        bare.scan(&case_hid(), &mut nothing(), false, true);
        bare.handle(Msg::Off(MOUSE.into()), false, &mut no_wait());
        assert_eq!(
            bare.status.as_deref(),
            Some("That device isn't connected now")
        );
        assert_eq!(bare.snapshot_row(MOUSE).unwrap().zones.len(), 2);
    }

    /// Systems, not products (#563): the second desk's 12-lamp lightbar, another vendor and
    /// kind, through the same code.
    #[test]
    fn any_lamp_array_takes_a_colour_per_lamp() {
        use crate::foxglove::fixtures::second_desk as desk;
        let calls = LampCalls::default();
        let writers: Writers =
            Box::new(|_path: &str| Ok(Box::new(Recorder(Arc::default())) as Box<dyn HidWriter>));
        let lamps: LampArrays = Box::new(|| vec![desk::lightbar_desc()]);
        let mut w = Worker::new(desk::catalog(), None, Settings::default(), writers, lamps)
            .with_lamp_opener(fake_lamps(&calls));
        w.scan(&desk::hid(), &mut nothing(), false, true);
        let key = w.snapshot().devices[1].key.clone();
        let colors: Vec<_> = (0..12).map(|i| (i, [i as u8 * 20, 0, 0])).collect();
        w.handle(
            Msg::Set {
                key: key.clone(),
                colors,
            },
            false,
            &mut no_wait(),
        );
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 12);
        assert_eq!(calls[11], (key, Some(11), [220, 0, 0]));
    }

    #[test]
    fn devices_carry_caps_and_lamp_arrays_are_read_once() {
        let (mut w, _) = worker(Settings::default());
        w.scan(&case_hid(), &mut nothing(), false, true);
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
        // #691: its lamps, by purpose and position, from what the device reports.
        let mouse = &s.devices[2];
        assert_eq!(
            mouse.zones,
            vec![
                ("Lamp 1 · Logo".to_string(), None),
                ("Lamp 2 · Accent".to_string(), None)
            ]
        );
        assert_eq!(mouse.zone_ids, vec!["0", "1"]);
        assert_eq!(mouse.zone_positions, vec![Some([34, 90]), Some([34, 30])]);
        assert_eq!(mouse.size, Some([68, 132]));
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
        let lamps: LampArrays = Box::new(|| vec![desk::lightbar_desc()]);
        let mut w = Worker::new(desk::catalog(), None, Settings::default(), writers, lamps);
        w.scan(&desk::hid(), &mut nothing(), false, true);
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
                ("Lumen desk lightbar", Control::LampArray, 12),
            ]
        );
        // Its lamps are its zones, at the positions it reports (#691).
        assert_eq!(snap.devices[1].zones[0].0, "Lamp 1 · Accent");
        assert_eq!(snap.devices[1].zone_ids[11], "11");
        assert_eq!(snap.devices[1].zone_positions[0], Some([16, 12]));
        assert_eq!(snap.devices[1].size, Some([400, 24]));
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

    /// #558: the second desk's case names its own vendor tool; while it runs, the page warns
    /// about that app on that device and the case's saved colours wait. The 45L's tool doesn't
    /// matter here.
    #[test]
    fn a_conflict_comes_from_the_devices_catalog_entry() {
        use crate::foxglove::fixtures::second_desk as desk;
        let writers: Writers =
            Box::new(|_path: &str| Ok(Box::new(Recorder(Arc::default())) as Box<dyn HidWriter>));
        let lamps: LampArrays = Box::new(Vec::new);
        let mut w = Worker::new(desk::catalog(), None, Settings::default(), writers, lamps);
        let mut asked: Vec<Vec<String>> = Vec::new();
        let mut glow = |names: &[&str]| {
            asked.push(names.iter().map(|n| n.to_string()).collect());
            names.contains(&desk::CONFLICT_EXE)
        };
        w.scan(&desk::hid(), &mut glow, false, true);
        assert_eq!(
            asked,
            [vec![desk::CONFLICT_EXE.to_string()]],
            "only its own exe"
        );
        let s = w.snapshot();
        assert_eq!(s.conflicts.len(), 1);
        assert_eq!(s.conflicts[0].app, desk::CONFLICT_APP);
        assert_eq!(s.conflicts[0].device, "Nimbus Strata case lights");
        assert!(s.conflicts[0].hint.is_some());
        assert_eq!(
            s.devices[0].kind,
            foxglove_devices::pc_lighting::DeviceKind::Case
        );
        // The 45L's tool running changes nothing on this desk.
        w.scan(&desk::hid(), &mut studio(), false, true);
        assert!(w.snapshot().conflicts.is_empty());
    }

    /// #558: a conflict holds back only its own device's colours on arrival. A second driven
    /// board (another model on the same protocol, no conflict) gets its colours back while the
    /// 45L's tool runs; the 45L waits.
    #[test]
    fn a_conflict_holds_back_only_its_own_devices_colours() {
        let dir = std::env::temp_dir().join(format!("foxglove-conflict-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("lighting-catalog.local.toml");
        std::fs::write(
            &local,
            "[[device]]
name = \"Shelf\"
vid = 0x1234
pid = 0x5678
protocol = \"omen-tracer\"
kind = \"strip\"
zones = [{ id = 3, name = \"Logo\" }]
",
        )
        .unwrap();
        let mut settings = Settings::default();
        for key in ["103c:84fd", "1234:5678"] {
            settings.devices.insert(
                key.into(),
                Saved {
                    colors: [("3".to_string(), "#0000ff".to_string())].into(),
                    match_keyboard: false,
                },
            );
        }
        settings
            .devices
            .get_mut("103c:84fd")
            .unwrap()
            .colors
            .insert("2".into(), "#112233".into());
        let (mut w, sent) = worker_at(settings, Some(local));
        let mut hid = case_hid();
        hid.push(HidEntry {
            vid: 0x1234,
            pid: 0x5678,
            usage_page: 1,
            path: "shelf".into(),
            ..Default::default()
        });
        w.scan(&hid, &mut studio(), false, true);
        assert_eq!(
            *sent.lock().unwrap(),
            vec![report(3, [0, 0, 0xff])],
            "only the shelf's colour"
        );
        let s = w.snapshot();
        assert_eq!(s.conflicts.len(), 1, "the shelf names no conflict");
        assert_eq!(s.conflicts[0].key, "103c:84fd");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Your own entry for a bundled board keeps the bundled one's kind, conflicts and
    /// calibration hint, so the vendor tool's warning (and the calibrator's starting white)
    /// doesn't vanish when you rename its zones.
    #[test]
    fn your_own_entry_for_a_bundled_board_keeps_its_kind_and_conflicts() {
        let dir = std::env::temp_dir().join(format!("foxglove-own-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let local = dir.join("lighting-catalog.local.toml");
        let (mut w, _) = worker_at(Settings::default(), Some(local.clone()));
        let mine = CatalogDevice::new(
            "My case".into(),
            0x103c,
            0x84fd,
            Protocol::OmenTracer,
            vec![ZoneSpec {
                id: 1,
                name: "Top".into(),
                position: None,
                role: Default::default(),
            }],
        );
        // No bundled model names a hint yet (#473): give this one a hint to keep.
        w.bundled
            .devices
            .iter_mut()
            .find(|d| (d.vid, d.pid) == (0x103c, 0x84fd))
            .unwrap()
            .calibration_hint = Some("typical-smd5050".into());
        w.handle(Msg::SaveLocal(Box::new(mine)), false, &mut no_wait());
        let saved = Catalog::parse(&std::fs::read_to_string(&local).unwrap()).unwrap();
        let d = &saved.devices[0];
        assert_eq!(d.kind, pcl::DeviceKind::Case);
        assert_eq!(d.calibration_hint.as_deref(), Some("typical-smd5050"));
        assert_eq!(
            d.conflicts,
            Catalog::bundled().find(0x103c, 0x84fd).unwrap().conflicts
        );
        w.scan(&case_hid(), &mut studio(), false, true);
        assert_eq!(w.snapshot().conflicts.len(), 1, "still warned about");
        let _ = std::fs::remove_dir_all(dir);
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
        w.scan(&hid, &mut nothing(), false, true);
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
        w.scan(&hid, &mut nothing(), false, false);
        let s = w.snapshot();
        let row = s.devices.iter().find(|r| r.name == "Desk strip").unwrap();
        assert!(row.entry.as_ref().unwrap().contains("vid = 0x1209"));
        assert_eq!(row.caps.chips(false), vec!["Per-zone RGB · 1 zone"]);
        assert!(s.unknown.is_empty());
        assert_eq!(sent.lock().unwrap().len(), before, "saving writes nothing");

        // A fresh worker reads the local file back.
        let (mut again, _) = worker_at(Settings::default(), Some(file.clone()));
        again.scan(&hid, &mut nothing(), false, true);
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
        w.scan(&case_hid(), &mut nothing(), false, true);
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
        w.scan(&case_hid(), &mut nothing(), false, true);
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
        w.scan(&case_hid(), &mut nothing(), false, true);
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
        w.handle(Msg::Keyboard(solid(240)), false, &mut no_wait());
        w.handle(Msg::SceneStart, false, &mut no_wait());
        sent.lock().unwrap().clear();
        w.handle(Msg::Keyboard(solid(0)), false, &mut no_wait());
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
        w.scan(&case_hid(), &mut nothing(), false, true);
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

    /// A fake OpenRGB server (#471) serving one four-LED memory module, and a worker pointed at
    /// it whose OpenRGB news lands in a channel.
    mod openrgb_fake {
        use super::*;
        use foxglove_devices::pc_lighting::openrgb::client::fake::{Answers, Fake};
        use foxglove_devices::pc_lighting::openrgb::wire::fixture::{controller, ZoneFx};

        pub const KEY: &str = "openrgb:Memory module:SN-1";

        pub fn server() -> Fake {
            Fake::start(
                Answers::Version(4),
                vec![Box::new(|v| {
                    controller(
                        v,
                        "Memory module",
                        "SN-1",
                        &[ZoneFx {
                            name: "Stick",
                            leds: 4,
                            matrix: None,
                            segments: vec![],
                        }],
                        |i| format!("LED {}", i + 1),
                        [0; 3],
                    )
                })],
            )
        }

        pub fn on(port: u16, reconnect: bool) -> openrgb::Settings {
            openrgb::Settings {
                enabled: true,
                port,
                reconnect,
                ..Default::default()
            }
        }

        pub type News = mpsc::Receiver<(u64, openrgb::Event)>;

        pub fn worker(settings: Settings, openrgb: openrgb::Settings) -> (Worker, News) {
            let (w, _) = super::worker(settings);
            let (tx, rx) = mpsc::channel();
            let tx = Mutex::new(tx);
            let events: OpenRgbEvents = Arc::new(move |conn, e| {
                let _ = tx.lock().unwrap().send((conn, e));
            });
            (w.with_openrgb(openrgb, events), rx)
        }

        /// The next news, handed to the worker, and the list placed again.
        pub fn next(w: &mut Worker, news: &News) -> openrgb::Event {
            let (conn, e) = news.recv_timeout(openrgb::TIMEOUT).expect("OpenRGB's news");
            w.handle(Msg::OpenRgbEvent(conn, e), false, &mut no_wait());
            if std::mem::take(&mut w.relist) {
                w.relist(&mut nothing(), false);
            }
            e
        }

        pub fn connect(w: &mut Worker) {
            w.handle(Msg::OpenRgbConnect, false, &mut no_wait());
            assert!(std::mem::take(&mut w.relist));
            w.relist(&mut nothing(), false);
        }
    }

    /// #471: off by default, nothing connects and no OpenRGB row shows; on, OpenRGB's
    /// controllers are rows beside the HID devices ("Through OpenRGB"), saved colours go back
    /// on when they arrive, scenes drive them, and OpenRGB's push lists them again.
    #[test]
    fn openrgb_controllers_are_light_sources_once_turned_on() {
        use foxglove_devices::pc_lighting::openrgb::wire::id;
        use openrgb_fake::*;
        // Off (the default): a connect request opens nothing.
        let (mut off, _news) = openrgb_fake::worker(Settings::default(), Default::default());
        off.scan(&case_hid(), &mut nothing(), false, true);
        off.handle(Msg::OpenRgbConnect, false, &mut no_wait());
        assert!(!off.relist && off.openrgb_retry.is_none());
        let s = off.snapshot();
        assert!(!s.openrgb.settings.enabled && !s.openrgb.connected);
        assert_eq!(s.openrgb.status, None);
        assert!(s.devices.iter().all(|r| r.control != Control::OpenRgb));

        let fake = server();
        let mut saved = Settings::default();
        saved.devices.insert(
            KEY.into(),
            Saved {
                colors: [("1".to_string(), "#ff0000".to_string())].into(),
                match_keyboard: false,
            },
        );
        let (mut w, news) = openrgb_fake::worker(saved, on(fake.addr.port(), false));
        w.scan(&case_hid(), &mut nothing(), false, true);
        let hid_rows = w.snapshot().devices.len();
        connect(&mut w);
        let s = w.snapshot();
        assert!(s.openrgb.connected, "{:?}", s.openrgb);
        assert_eq!(
            s.openrgb.status.as_deref(),
            Some(
                format!(
                    "Connected to OpenRGB at 127.0.0.1:{}: 1 device",
                    fake.addr.port()
                )
                .as_str()
            )
        );
        assert_eq!(s.devices.len(), hid_rows + 1, "beside the HID devices");
        let row = s.devices.iter().find(|r| r.key == KEY).unwrap();
        assert_eq!(row.control, Control::OpenRgb);
        assert_eq!(row.name, "Memory module");
        assert_eq!(row.zone_ids, vec!["0", "1", "2", "3"]);
        assert_eq!(row.zones[1], ("LED 2".to_string(), Some([255, 0, 0])));

        // A scene drives it like any board.
        w.handle(Msg::SceneStart, false, &mut no_wait());
        w.handle(
            Msg::SceneFrame {
                key: KEY.into(),
                colors: (0..4).map(|i| (i, [0, 0, 255])).collect(),
            },
            false,
            &mut no_wait(),
        );
        // OpenRGB's push lists it again (a round trip: the fake has read every write).
        fake.push.send(Some(())).unwrap();
        assert_eq!(next(&mut w, &news), openrgb::Event::DeviceListUpdated);
        // Read in order: the saved colour on LED 2 alone, then the frame on every LED.
        assert!(
            !fake.sent(id::SETCUSTOMMODE).is_empty(),
            "custom mode first"
        );
        let zone = fake.sent(id::UPDATEZONELEDS);
        assert!(
            zone.first()
                .is_some_and(|p| p.payload[14..18] == [255, 0, 0, 0]),
            "the saved colour: {zone:?}"
        );
        let all = fake.sent(id::UPDATELEDS);
        assert!(
            all.first()
                .is_some_and(|p| p.payload[6..10] == [0, 0, 255, 0]),
            "the frame lit every LED: {all:?}"
        );
        assert!(
            w.snapshot().devices.iter().any(|r| r.key == KEY),
            "listed again"
        );

        // OpenRGB quits: its rows go, and (not asked to) nothing tries again.
        drop(fake);
        assert_eq!(next(&mut w, &news), openrgb::Event::Disconnected);
        let s = w.snapshot();
        assert!(s.devices.iter().all(|r| r.control != Control::OpenRgb));
        assert_eq!(s.devices.len(), hid_rows);
        assert_eq!(
            s.openrgb.status.as_deref(),
            Some("OpenRGB closed the connection")
        );
        assert!(!s.openrgb.connected && w.openrgb_retry.is_none());

        // Turned off: saved, no status line.
        w.handle(
            Msg::OpenRgbSettings(openrgb::Settings::default()),
            false,
            &mut no_wait(),
        );
        assert!(w.openrgb_dirty);
        assert_eq!(w.snapshot().openrgb.status, None);
    }

    /// #471: with nothing listening, the status says so; "try again" (its own opt-in) waits
    /// on the crate's bounded backoff, a connection's late news is dropped, and a new address
    /// connects anew.
    #[test]
    fn openrgb_retries_only_when_asked_and_on_a_bounded_backoff() {
        use openrgb_fake::*;
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let (mut w, _news) = openrgb_fake::worker(Settings::default(), on(port, false));
        connect(&mut w);
        let status = w.snapshot().openrgb.status.unwrap();
        assert!(
            status.starts_with(&format!("Can't reach OpenRGB at 127.0.0.1:{port}")),
            "{status}"
        );
        assert_eq!(w.openrgb_retry, None, "not asked to try again");

        w.handle(Msg::OpenRgbSettings(on(port, true)), false, &mut no_wait());
        assert!(w.openrgb_dirty);
        let mut waits = Vec::new();
        while let Some(wait) = w.openrgb_retry.take() {
            waits.push(wait.as_secs());
            w.handle(Msg::OpenRgbConnect, false, &mut no_wait());
        }
        assert_eq!(waits, vec![1, 2, 4, 8, 16, 30, 30, 30], "then it stops");
        assert!(
            w.snapshot()
                .openrgb
                .status
                .unwrap()
                .ends_with("stopped trying"),
            "says so"
        );

        // A server at another address: connected, and the old connection's news is dropped.
        let fake = server();
        w.handle(
            Msg::OpenRgbSettings(on(fake.addr.port(), true)),
            false,
            &mut no_wait(),
        );
        w.relist(&mut nothing(), false);
        assert!(w.snapshot().openrgb.connected);
        w.handle(
            Msg::OpenRgbEvent(0, openrgb::Event::Disconnected),
            false,
            &mut no_wait(),
        );
        assert!(w.snapshot().openrgb.connected, "an older connection's news");
        assert!(w.snapshot().devices.iter().any(|r| r.key == KEY));
    }
}
