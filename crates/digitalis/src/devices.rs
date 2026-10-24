//! device-layer device for a Digitalis keyboard (feature `devices`): everything the host link can do, behind
//! the device layer's capability traits. The keymap is not here: ZMK keymaps come over Studio RPC, and the
//! app adds them on top of this device.
//!
//! Capabilities from HELLO's bits:
//!
//! | Device layer | from |
//! |---|---|
//! | `SCREENS` | `PANEL` or `ART_RELAY`, or no bits at all (firmware older than the bits) |
//! | `LIGHTING`, `SETTINGS`, `BEHAVIORS` | `SETTINGS_RGB` (the 0xD7 family) |
//! | `POWER` | always: HELLO carries the right half's battery; `CONN_STATE` adds output and profile |
//! | `LAYERS`, `KEY_EVENTS` | always: the overlay's 0xFF/0xF1 reports on the same interface |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use foxglove_devices::{
    ArtStatus as DeviceArt, BehaviorInfo, Behaviors, Caps, Checked, DevSettings, Device, DeviceEvent,
    DeviceInfo, Effect, EventSink, Layers, Led, Lighting, LightingState, DeviceError,
    Power, PowerInfo, Screens, Value, Zone,
};

use crate::link::{Event, EventSubscription, Link};
use crate::protocol::{capabilities, Hello, InvokeStatus, RgbHalf, RgbState};
use crate::{art, panel, settings};

type PResult<T> = foxglove_devices::Result<T>;

/// First byte of the overlay's layer state report: size, default layers, active layers.
pub const LAYER_STATE: u8 = 0xFF;
/// First byte of a key event report: row, column, pressed.
pub const KEY_EVENT: u8 = 0xF1;
const MAX_LAYER_STATE_BYTES: usize = 4;

/// Active layers from a layer state report (the base layer always counts), or `None` for an
/// unrelated report that happens to start with 0xFF.
pub fn parse_layers(r: &[u8]) -> Option<Vec<u8>> {
    let size = *r.get(1)? as usize;
    if r[0] != LAYER_STATE || size == 0 || size > MAX_LAYER_STATE_BYTES || 2 + 2 * size > r.len() {
        return None;
    }
    let word = |b: &[u8]| {
        let mut w = [0u8; 4];
        w[..size].copy_from_slice(b);
        u32::from_le_bytes(w)
    };
    let active = word(&r[2..2 + size]) | word(&r[2 + size..2 + 2 * size]) | 1;
    Some((0..32).filter(|i| active & (1 << i) != 0).collect())
}

/// What device capabilities a keyboard has, from what it said to HELLO.
pub fn caps(hello: &Hello) -> Caps {
    let bits = hello.capabilities;
    let mut caps = Caps::LAYERS | Caps::KEY_EVENTS | Caps::POWER;
    if bits == 0 || bits & (capabilities::PANEL | capabilities::ART_RELAY) != 0 {
        caps |= Caps::SCREENS;
    }
    if bits & capabilities::SETTINGS_RGB != 0 {
        caps |= Caps::LIGHTING | Caps::SETTINGS | Caps::BEHAVIORS;
    }
    caps
}

fn err(e: anyhow::Error) -> DeviceError {
    let text = e.to_string();
    if text.contains("not connected") || text.contains("disconnected") || text.contains("link stopped") {
        DeviceError::Disconnected
    } else if text.contains("no reply") {
        DeviceError::Timeout
    } else {
        DeviceError::Backend(text)
    }
}

fn lighting_state(h: &RgbHalf) -> LightingState {
    LightingState {
        on: h.on,
        hue: h.hue,
        sat: h.sat,
        brightness: h.brightness,
        effect: Effect { id: h.effect.to_string(), params: vec![] },
        speed: h.speed,
        settled: true,
    }
}

/// Whether the right half (as it last reported) shows what the left does; brightness aside,
/// since each half dims on its own battery.
fn halves_agree(rgb: &RgbState) -> bool {
    let Some((r, _age)) = &rgb.right else { return true };
    let l = &rgb.left;
    let near = |a: u16, b: u16, wrap: u16| {
        let d = a.abs_diff(b);
        d.min(wrap.saturating_sub(d)) <= 2
    };
    l.on == r.on && near(l.hue, r.hue, 360) && near(l.sat.into(), r.sat.into(), 1000) && l.effect == r.effect
}

/// A Digitalis keyboard as a device-layer device. Events reach the sink for as long as it lives.
pub struct DigitalisDevice {
    info: DeviceInfo,
    link: Arc<Link>,
    hello: Mutex<Hello>,
    /// The PC panel's size, once asked ([`DigitalisDevice::panel_size`]).
    panel_size: Mutex<Option<(usize, usize)>>,
    caps: Caps,
    sink: EventSink,
    layers: Arc<Mutex<Vec<u8>>>,
    behaviors: Mutex<Vec<BehaviorInfo>>,
    underglow: Mutex<Option<u16>>,
    _events: EventSubscription,
    _reports: EventSubscription,
}

impl DigitalisDevice {
    /// Wraps a connected link; `hello` is what the keyboard answered when it connected.
    pub fn new(info: DeviceInfo, link: Arc<Link>, hello: Hello, sink: EventSink) -> Self {
        let events_sink = sink.clone();
        let events = link.subscribe_events(move |event| {
            if let Event::State(state) = event {
                events_sink.emit(match state.connected {
                    Some(_) => DeviceEvent::Connected,
                    None => DeviceEvent::Disconnected {
                        reason: state.error.unwrap_or_else(|| "not connected".into()),
                    },
                });
            }
        });
        let layers = Arc::new(Mutex::new(vec![0]));
        let (report_sink, report_layers) = (sink.clone(), layers.clone());
        // Runs on the link's reader thread: emitting only updates facts and queues events.
        let reports = link.subscribe_reports(move |_, r| {
            if r[0] == KEY_EVENT {
                report_sink.emit(DeviceEvent::Key { row: r[1].into(), col: r[2].into(), pressed: r[3] != 0 });
            } else if crate::protocol::is_reply(r, crate::protocol::SETTINGS, crate::protocol::settings::CONN_STATE) {
                // The firmware says when the connection changes (CONN_STATE_EVENT), and answers
                // a read the same way (#17).
                let c = crate::protocol::ConnState::parse(r);
                report_sink.emit(DeviceEvent::Link {
                    output: if c.endpoint == 0 { "USB" } else { "BLE" }.into(),
                    profile: c.active_profile,
                    connected: c.connected,
                });
            } else if let Some(active) = parse_layers(r) {
                let mut current = report_layers.lock().unwrap_or_else(|e| e.into_inner());
                if *current != active {
                    current.clone_from(&active);
                    report_sink.emit(DeviceEvent::LayerChanged { active });
                }
            }
        });
        if let Some(percent) = hello.right_battery {
            sink.emit(DeviceEvent::Battery { part: "right".into(), percent });
        }
        DigitalisDevice {
            info,
            link,
            caps: caps(&hello),
            hello: Mutex::new(hello),
            panel_size: Mutex::new(None),
            sink,
            layers,
            behaviors: Mutex::new(Vec::new()),
            underglow: Mutex::new(None),
            _events: events,
            _reports: reports,
        }
    }

    /// The behaviors [`Behaviors::list`] offers; the app fills it from the keymap it reads.
    pub fn set_behaviors(&self, list: Vec<BehaviorInfo>) {
        *self.behaviors.lock().unwrap_or_else(|e| e.into_inner()) = list;
    }

    /// The keymap's `&rgb_ug` behavior id, which lighting changes go through (the firmware has
    /// no direct colour command yet).
    pub fn set_underglow(&self, id: Option<u16>) {
        *self.underglow.lock().unwrap_or_else(|e| e.into_inner()) = id;
    }

    /// Sends one `&rgb_ug` command. Lighting commands are always safe, so they skip the
    /// action list.
    ///
    /// The firmware answers Busy while it's still releasing the previous command, so a quick
    /// run of changes (a plugin streaming colours) waits briefly and tries again instead of
    /// dropping them.
    fn underglow(&self, command: u32, param: u32) -> PResult<()> {
        let id = self.underglow.lock().unwrap_or_else(|e| e.into_inner()).ok_or(DeviceError::Unsupported)?;
        let mut tries = 0;
        loop {
            match status(settings::invoke(&*self.link, id, command, param, 0).map_err(err)?) {
                Err(DeviceError::Busy(_)) if tries < UNDERGLOW_BUSY_TRIES => {
                    tries += 1;
                    std::thread::sleep(UNDERGLOW_BUSY_WAIT);
                }
                other => return other,
            }
        }
    }

    pub fn link(&self) -> &Arc<Link> {
        &self.link
    }

    /// The PC panel's size: from the keyboard's screen descriptors when it has them (#289),
    /// read once, else from HELLO.
    fn panel_size(&self) -> (usize, usize) {
        let hello = *self.hello.lock().unwrap_or_else(|e| e.into_inner());
        let mut cached = self.panel_size.lock().unwrap_or_else(|e| e.into_inner());
        *cached.get_or_insert_with(|| {
            let screens = if hello.capabilities & capabilities::SCREENS != 0 { panel::screens(&*self.link).unwrap_or_default() } else { vec![] };
            panel::drawable_panel(&screens).map_or((hello.width, hello.height), |p| (usize::from(p.width), usize::from(p.height)))
        })
    }

    fn has(&self, bit: u32) -> bool {
        self.hello.lock().unwrap_or_else(|e| e.into_inner()).capabilities & bit != 0
    }
}

impl Device for DigitalisDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn caps(&self) -> Caps {
        self.caps
    }

    fn layers(&self) -> Option<&dyn Layers> {
        Some(self)
    }

    fn behaviors(&self) -> Option<&dyn Behaviors> {
        self.caps.contains(Caps::BEHAVIORS).then_some(self as &dyn Behaviors)
    }

    fn lighting(&self) -> Option<&dyn Lighting> {
        self.caps.contains(Caps::LIGHTING).then_some(self as &dyn Lighting)
    }

    fn screens(&self) -> Option<&dyn Screens> {
        self.caps.contains(Caps::SCREENS).then_some(self as &dyn Screens)
    }

    fn power(&self) -> Option<&dyn Power> {
        Some(self)
    }

    fn settings(&self) -> Option<&dyn DevSettings> {
        self.caps.contains(Caps::SETTINGS).then_some(self as &dyn DevSettings)
    }
}

impl Layers for DigitalisDevice {
    fn active(&self) -> PResult<Vec<u8>> {
        Ok(self.layers.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }
}

impl Behaviors for DigitalisDevice {
    fn list(&self) -> PResult<Vec<BehaviorInfo>> {
        Ok(self.behaviors.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }

    fn invoke_checked(&self, checked: Checked) -> PResult<()> {
        let a = checked.action();
        let id = u16::try_from(a.id).map_err(|_| DeviceError::Unsupported)?;
        status(settings::invoke(&*self.link, id, a.param1, a.param2, a.hold_ms).map_err(err)?)
    }
}

/// The keyboard's answer to INVOKE as a device-layer result; the keyboard's own words are kept.
fn status(s: InvokeStatus) -> PResult<()> {
    match s {
        InvokeStatus::Ok => Ok(()),
        InvokeStatus::Busy => Err(DeviceError::Busy(s.to_string())),
        InvokeStatus::Refused => Err(DeviceError::Denied(s.to_string())),
        other => Err(DeviceError::Backend(other.to_string())),
    }
}

/// Underglow commands that find the firmware busy are retried this many times, this far apart.
const UNDERGLOW_BUSY_TRIES: u32 = 10;
const UNDERGLOW_BUSY_WAIT: std::time::Duration = std::time::Duration::from_millis(20);

// `&rgb_ug` commands (ZMK's dt-bindings/zmk/rgb.h).
const RGB_ON: u32 = 1;
const RGB_OFF: u32 = 2;
const RGB_SPEED_UP: u32 = 9;
const RGB_SPEED_DOWN: u32 = 10;
const RGB_EFFECT_SET: u32 = 13;
/// ZMK's underglow speeds run 1-5, so this many steps reach any of them.
const SPEED_STEPS: u32 = 5;
const RGB_COLOR_HSB: u32 = 14;

impl Lighting for DigitalisDevice {
    fn layout(&self) -> PResult<Vec<Zone>> {
        let rgb = settings::rgb_state(&*self.link).map_err(err)?;
        let zone = |name: &str, id| Zone { name: name.into(), leds: vec![Led { id, position: None }] };
        let mut zones = vec![zone("left", 0)];
        if rgb.right.is_some() {
            zones.push(zone("right", 1));
        }
        Ok(zones)
    }

    fn state(&self) -> PResult<LightingState> {
        let rgb = settings::rgb_state(&*self.link).map_err(err)?;
        let state = LightingState { settled: halves_agree(&rgb), ..lighting_state(&rgb.left) };
        self.sink.emit(DeviceEvent::LightingChanged { state: state.clone() });
        Ok(state)
    }

    /// Effects by the firmware's number: 0 solid, 1 breathe, 2 spectrum, 3 swirl.
    fn set_effect(&self, effect: Effect) -> PResult<()> {
        let n: u32 = effect.id.parse().map_err(|_| DeviceError::Unsupported)?;
        self.underglow(RGB_EFFECT_SET, n)
    }

    fn set_on(&self, on: bool) -> PResult<()> {
        self.underglow(if on { RGB_ON } else { RGB_OFF }, 0)
    }

    fn set_color(&self, hue: u16, sat: u8, brightness: u8) -> PResult<()> {
        let hsb = (u32::from(hue.min(359)) << 16) | (u32::from(sat.min(100)) << 8) | u32::from(brightness.min(100));
        self.underglow(RGB_COLOR_HSB, hsb)
    }

    /// ZMK has no "set speed", only one step up or down: steps toward `speed`, reading it back
    /// each time, at most [`SPEED_STEPS`] times (#293).
    fn set_speed(&self, speed: u8) -> PResult<()> {
        for _ in 0..SPEED_STEPS {
            let now = settings::rgb_state(&*self.link).map_err(err)?.left.speed;
            if now == speed {
                return Ok(());
            }
            self.underglow(if now < speed { RGB_SPEED_UP } else { RGB_SPEED_DOWN }, 0)?;
        }
        Ok(())
    }

    fn release(&self) -> PResult<()> {
        // Changes go through the keymap's own behavior, so the keyboard always keeps control.
        Ok(())
    }
}

impl Screens for DigitalisDevice {
    fn send_panel(&self, bitmap: &[u8], w: u16, h: u16) -> PResult<()> {
        let (width, height) = self.panel_size();
        let stride = (w as usize).div_ceil(8);
        if (w as usize, h as usize) != (width, height) || bitmap.len() != stride * h as usize {
            return Err(DeviceError::Backend(format!("the panel is {width}x{height}; got {w}x{h} with {} bytes", bitmap.len())));
        }
        panel::show(&*self.link, bitmap, None, stride).map(|_| ()).map_err(err)
    }

    fn art_status(&self) -> PResult<DeviceArt> {
        let st = art::status(&*self.link).map_err(err)?;
        Ok(DeviceArt {
            selected: u8::try_from(st.right_set).ok(),
            bytes: st.length.into(),
            receiving: st.right_state == 1,
        })
    }

    fn push_art(&self, bytes: &[u8], progress: &mut dyn FnMut(usize, usize), cancel: &AtomicBool) -> PResult<()> {
        art::push_cancellable(
            &*self.link,
            bytes,
            |st| progress(st.pushed.into(), st.length.into()),
            || cancel.load(Ordering::Relaxed),
        )
        .map(|_| ())
        .map_err(err)
    }

    fn select_art(&self, index: Option<u8>) -> PResult<()> {
        let set = match index {
            None => -1,
            Some(n) => i8::try_from(n).map_err(|_| DeviceError::Unsupported)?,
        };
        art::select(&*self.link, set).map(|_| ()).map_err(err)
    }
}

impl Power for DigitalisDevice {
    fn batteries(&self) -> PResult<Vec<(String, u8)>> {
        let fresh = panel::hello(&*self.link).map_err(err)?;
        let mut hello = self.hello.lock().unwrap_or_else(|e| e.into_inner());
        if fresh.right_battery != hello.right_battery {
            if let Some(percent) = fresh.right_battery {
                self.sink.emit(DeviceEvent::Battery { part: "right".into(), percent });
            }
        }
        *hello = fresh;
        Ok(fresh.right_battery.map(|p| ("right".to_string(), p)).into_iter().collect())
    }

    fn info(&self) -> PResult<PowerInfo> {
        if !self.has(capabilities::CONN_STATE) {
            return Ok(PowerInfo::default());
        }
        let c = settings::conn_state(&*self.link).map_err(err)?;
        Ok(PowerInfo {
            output: Some(if c.endpoint == 0 { "USB" } else { "BLE" }.into()),
            profile: Some(c.active_profile),
            profile_connected: Some(c.connected),
        })
    }
}

impl DevSettings for DigitalisDevice {
    fn get(&self, key: &str) -> PResult<Option<Value>> {
        match key {
            "rgb_sync" => Ok(settings::rgb_sync(&*self.link, None).map_err(err)?.sync.map(Value::Bool)),
            _ => Ok(None),
        }
    }

    fn set(&self, key: &str, value: Value) -> PResult<()> {
        match (key, value) {
            ("rgb_sync", Value::Bool(on)) => settings::rgb_sync(&*self.link, Some(on)).map(|_| ()).map_err(err),
            _ => Err(DeviceError::Unsupported),
        }
    }
}

#[cfg(feature = "hid")]
mod backend {
    use super::*;
    use crate::hid::Match;
    use crate::protocol;
    use foxglove_devices::{Backend, BackendId, DeviceId, DeviceKind};
    use std::time::Duration;

    /// Finds Digitalis keyboards over hidapi; backend id `zmk`.
    #[derive(Default)]
    pub struct DigitalisBackend {
        /// Narrows discovery (product name, VID/PID); the default takes any Raw HID keyboard.
        pub filter: Match,
    }

    impl Backend for DigitalisBackend {
        fn id(&self) -> BackendId {
            BackendId("zmk".into())
        }

        fn discover(&self) -> PResult<Vec<DeviceInfo>> {
            let api = hidapi::HidApi::new().map_err(|e| DeviceError::Backend(e.to_string()))?;
            let mut found: Vec<DeviceInfo> = Vec::new();
            for d in api.device_list() {
                if d.usage_page() != protocol::USAGE_PAGE || d.usage() != protocol::USAGE || !self.filter.accepts(d) {
                    continue;
                }
                let path = d.path().to_string_lossy();
                let id = DeviceId::new(self.id(), d.vendor_id(), d.product_id(), d.serial_number(), &path);
                // A keyboard on USB and Bluetooth at once shows up twice; keep the first.
                if found.iter().all(|f| f.id != id) {
                    found.push(DeviceInfo {
                        id,
                        backend: self.id(),
                        name: d.product_string().unwrap_or("Keyboard").into(),
                        kind: DeviceKind::Keyboard,
                        firmware: None,
                    });
                }
            }
            Ok(found)
        }

        /// Opens the first interface with this VID/PID that answers HELLO; two identical
        /// keyboards at once can't be told apart yet.
        fn open(&self, info: &DeviceInfo, sink: EventSink) -> PResult<Box<dyn Device>> {
            let m = Match { vid_pid: Some((info.id.vid, info.id.pid)), ..Match::default() };
            let link = Arc::new(Link::hid(m).map_err(err)?);
            let connected = link.wait_connected(Duration::from_secs(3)).map_err(err)?;
            let hello = connected.hello.ok_or(DeviceError::Unsupported)?;
            Ok(Box::new(DigitalisDevice::new(info.clone(), link, hello, sink)))
        }
    }
}

#[cfg(feature = "hid")]
pub use backend::DigitalisBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{Connected, RawHid, Report};
    use crate::protocol::{self, REPORT};
    use foxglove_devices::{BackendId, DeviceId, DeviceKind, EventBus, Risk};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(2);

    /// A keyboard in memory that answers the host link's requests with canned replies.
    #[derive(Clone, Default)]
    struct Fake {
        inbox: Arc<(Mutex<VecDeque<Report>>, std::sync::Condvar)>,
        interrupted: Arc<AtomicBool>,
        written: Arc<Mutex<Vec<Vec<u8>>>>,
        /// INVOKEs to answer "busy" before answering OK.
        busy: Arc<std::sync::atomic::AtomicU32>,
    }

    fn report(bytes: &[u8]) -> Report {
        let mut r = [0u8; REPORT];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    impl Fake {
        fn push(&self, bytes: &[u8]) {
            self.inbox.0.lock().unwrap().push_back(report(bytes));
            self.inbox.1.notify_all();
        }
    }

    impl RawHid for Fake {
        fn read(&self, buf: &mut Report) -> anyhow::Result<usize> {
            let mut inbox = self.inbox.0.lock().unwrap();
            loop {
                if self.interrupted.load(Ordering::Relaxed) {
                    return Ok(0);
                }
                if let Some(r) = inbox.pop_front() {
                    *buf = r;
                    return Ok(REPORT);
                }
                inbox = self.inbox.1.wait(inbox).unwrap();
            }
        }

        fn interrupt(&self) {
            let _inbox = self.inbox.0.lock().unwrap();
            self.interrupted.store(true, Ordering::Relaxed);
            self.inbox.1.notify_all();
        }

        fn write(&self, out: &[u8]) -> anyhow::Result<()> {
            self.written.lock().unwrap().push(out.to_vec());
            let reply = match (out[0], out[1]) {
                (protocol::SETTINGS, protocol::settings::RGB_STATE) => {
                    report(&[0xD7, 0x83, 1, 0x68, 0x01, 80, 50, 3, 2, 0xff])
                }
                (protocol::SETTINGS, protocol::settings::INVOKE) => {
                    let busy = self.busy.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_ok();
                    report(&[0xD7, 0x82, if busy { 2 } else { 0 }])
                }
                (protocol::SETTINGS, protocol::settings::RGB_SYNC) => report(&[0xD7, 0x81, 1, 1, 2]),
                (protocol::PANEL, protocol::panel::HELLO) => report(&[0xD5, 0x81, 1, 64, 52, 0, 0, 1, 55]),
                _ => return Ok(()),
            };
            self.push(&reply);
            Ok(())
        }
    }

    fn hello(capabilities: u32) -> Hello {
        Hello { version: 1, width: 64, height: 52, right_link: 1, right_battery: Some(80), capabilities, ..Hello::default() }
    }

    fn device(capabilities: u32) -> (DigitalisDevice, Fake, EventBus) {
        let fake = Fake::default();
        let opener_fake = fake.clone();
        let link = Arc::new(Link::start(Box::new(move || {
            let connected = Connected { product: "Fake".into(), hello: Some(hello(capabilities)) };
            Ok((Box::new(opener_fake.clone()) as Box<dyn RawHid>, connected))
        })));
        link.wait_connected(WAIT).unwrap();
        let id = DeviceId::new(BackendId("zmk".into()), 0x1d50, 0x615e, Some("1"), "p");
        let info = DeviceInfo { id: id.clone(), backend: id.backend.clone(), name: "Fake".into(), kind: DeviceKind::Keyboard, firmware: None };
        let bus = EventBus::default();
        let dev = DigitalisDevice::new(info, link, hello(capabilities), bus.sink(id));
        (dev, fake, bus)
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        let end = Instant::now() + WAIT;
        while !done() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn caps_follow_hello_bits() {
        let legacy = caps(&hello(0));
        assert!(legacy.contains(Caps::SCREENS) && !legacy.contains(Caps::LIGHTING));
        let rgb_only = caps(&hello(capabilities::SETTINGS_RGB));
        assert!(rgb_only.contains(Caps::LIGHTING | Caps::SETTINGS | Caps::BEHAVIORS));
        assert!(!rgb_only.contains(Caps::SCREENS));
        assert!(caps(&hello(capabilities::PANEL)).contains(Caps::SCREENS | Caps::POWER | Caps::LAYERS));
    }

    #[test]
    fn layer_reports_parse_like_the_overlay() {
        // size 4, default layer 0, layers 2 and 3 active
        assert_eq!(parse_layers(&report(&[0xFF, 4, 1, 0, 0, 0, 0b1100, 0, 0, 0])), Some(vec![0, 2, 3]));
        assert_eq!(parse_layers(&report(&[0xFF, 0])), None);
        assert_eq!(parse_layers(&report(&[0xFF, 9])), None);
    }

    #[test]
    fn passes_the_device_contract() {
        let all = capabilities::PANEL | capabilities::ART_RELAY | capabilities::SETTINGS_RGB;
        let (dev, fake, _bus) = device(all);
        dev.set_behaviors(vec![
            BehaviorInfo { id: 3, param1: 0, param2: 0, name: "Bootloader".into(), hold_ms: 0, risk: Risk::Confirm },
            BehaviorInfo { id: 4, param1: 1, param2: 0, name: "RGB on".into(), hold_ms: 0, risk: Risk::Safe },
        ]);
        foxglove_devices::contract::check_device(&dev).unwrap();
        let behaviors = dev.behaviors().unwrap();
        assert!(matches!(behaviors.invoke(3, 0, 0, false), Err(DeviceError::Denied(_))));
        behaviors.invoke(4, 1, 0, false).unwrap();
        assert_eq!(behaviors.invoke(4, 2, 0, false), Err(DeviceError::Unsupported));
        let lighting = dev.lighting().unwrap();
        assert_eq!(lighting.state().unwrap().hue, 0x168);
        assert_eq!(lighting.set_on(true), Err(DeviceError::Unsupported), "no &rgb_ug known yet");
        dev.set_underglow(Some(7));
        lighting.set_color(400, 50, 100).unwrap();
        let sent = fake.written.lock().unwrap().last().unwrap().clone();
        assert_eq!(&sent[..12], &protocol::invoke(7, RGB_COLOR_HSB, (359 << 16) | (50 << 8) | 100, 0)[..12]);
        assert_eq!(dev.settings().unwrap().get("rgb_sync").unwrap(), Some(Value::Bool(true)));
        assert_eq!(dev.power().unwrap().batteries().unwrap(), vec![("right".to_string(), 55)]);
    }

    #[test]
    fn halves_agree_on_all_but_brightness() {
        let left = RgbHalf { on: true, hue: 0, sat: 100, brightness: 80, effect: 0, speed: 1 };
        assert!(halves_agree(&RgbState { left, right: None }), "no right half: nothing to wait for");
        let dim = RgbHalf { brightness: 30, ..left };
        assert!(halves_agree(&RgbState { left, right: Some((dim, 1)) }), "a half dimmed on battery still agrees");
        let behind = RgbHalf { hue: 240, ..left };
        assert!(!halves_agree(&RgbState { left, right: Some((behind, 2)) }));
        assert!(!halves_agree(&RgbState { left, right: Some((RgbHalf { on: false, ..left }, 0)) }));
        assert!(!halves_agree(&RgbState { left, right: Some((RgbHalf { effect: 3, ..left }, 0)) }));
    }

    #[test]
    fn underglow_waits_out_a_busy_keyboard() {
        let (dev, fake, _bus) = device(capabilities::PANEL | capabilities::SETTINGS_RGB);
        dev.set_underglow(Some(7));
        let lighting = dev.lighting().unwrap();
        fake.busy.store(3, Ordering::Relaxed);
        lighting.set_on(true).unwrap();
        let invokes = |f: &Fake| f.written.lock().unwrap().iter().filter(|w| w[1] == protocol::settings::INVOKE).count();
        assert_eq!(invokes(&fake), 4, "three busy answers, then OK");
        fake.busy.store(UNDERGLOW_BUSY_TRIES + 1, Ordering::Relaxed);
        assert!(matches!(lighting.set_on(false), Err(DeviceError::Busy(_))), "gives up after the retries");
    }

    #[test]
    fn panel_size_is_checked() {
        let (dev, _fake, _bus) = device(capabilities::PANEL);
        let screens = dev.screens().unwrap();
        assert!(screens.send_panel(&[0; 8 * 52], 64, 52).is_ok());
        assert!(screens.send_panel(&[0; 10], 64, 52).is_err());
        assert!(screens.send_panel(&[0; 8 * 50], 64, 50).is_err());
    }

    #[test]
    fn reports_become_events_and_keys_need_opt_in() {
        let (dev, fake, bus) = device(capabilities::PANEL);
        let plain = bus.subscribe(false);
        let keys = bus.subscribe(true);
        fake.push(&[0xFF, 4, 1, 0, 0, 0, 0b10, 0, 0, 0]);
        fake.push(&[0xF1, 2, 3, 1]);
        wait_for(|| dev.layers().unwrap().active().unwrap() == vec![0, 1]);
        assert_eq!(dev.layers().unwrap().active().unwrap(), vec![0, 1]);
        let mut saw_key = false;
        let end = Instant::now() + WAIT;
        while !saw_key && Instant::now() < end {
            if let Ok(e) = keys.recv_timeout(Duration::from_millis(50)) {
                saw_key = e.event == DeviceEvent::Key { row: 2, col: 3, pressed: true };
            }
        }
        assert!(saw_key);
        assert!(plain.try_iter().all(|e| !matches!(e.event, DeviceEvent::Key { .. })));
        let fact = format!("device.{}.layers", Device::info(&dev).id);
        assert_eq!(bus.facts().get(&fact), Some(Value::Bytes(vec![0, 1])));
    }
}
