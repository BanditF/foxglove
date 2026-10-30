//! Foxglove's device-layer host: one event bus and facts store for the whole app, and the keyboard with
//! the host link as a device-layer device. Everything that talks to that keyboard goes through
//! [`device`], so the same code will serve other backends later.

use std::sync::{Arc, Condvar, Mutex, OnceLock};

use foxglove_devices::{
    BackendId, BehaviorInfo, Device, DeviceId, DeviceInfo, DeviceKind, EventBus,
};
use foxglove_link::devices::FoxgloveLinkDevice;

static BUS: OnceLock<EventBus> = OnceLock::new();
static DEVICE: Mutex<Option<Arc<FoxgloveLinkDevice>>> = Mutex::new(None);
/// The link generation (connection) whose firmware build the cached device's ids were checked
/// against (#633). Another one (a replug, a flash on the same link) checks again.
static CHECKED: Mutex<Option<u64>> = Mutex::new(None);

/// What the keymap lets the app invoke, its `&rgb_ug` id, and the firmware build those ids were
/// learned under (#633): a flash can renumber the behaviors, so the next connection takes them
/// only while it runs that same build.
#[derive(Default)]
struct Learned {
    actions: Vec<BehaviorInfo>,
    underglow: Option<u16>,
    /// `None`: learned while the build couldn't be read, so no later connection trusts them.
    build: Option<String>,
}

static ACTIONS: Mutex<Learned> = Mutex::new(Learned {
    actions: Vec::new(),
    underglow: None,
    build: None,
});

/// The connected keyboard's firmware build, as last checked (`None`: unreadable or none yet).
static BUILD: Mutex<Option<String>> = Mutex::new(None);

/// The app's event bus; its facts store holds every device's state under `device.<id>.*`.
pub fn bus() -> &'static EventBus {
    BUS.get_or_init(EventBus::default)
}

/// The keyboard with Foxglove's host link, found once and kept while it stays connected. May
/// block on discovery: call it off the UI thread.
pub fn device() -> Option<Arc<FoxgloveLinkDevice>> {
    let mut cached = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = cached
        .as_ref()
        .filter(|d| d.link().state().connected.is_some())
        .cloned()
    {
        // The same link connected again (a replug, a flash): its ids wait for its build (#633).
        let rechecked = recheck(&d);
        drop(cached);
        if let Some(build) = rechecked {
            let id = &d.info().id;
            super::keyboard_actions::learn_cached(id.vid, id.pid, build.as_deref());
        }
        return Some(d);
    }
    let mut found = None;
    let mut build = None;
    let mut name = String::new();
    let mut route = foxglove_link::link::Bus::Unknown;
    *cached = super::link::host_keyboard().map(|((vid, pid), link, hello)| {
        found = Some((vid, pid));
        let backend = BackendId("zmk".into());
        let (product, opened) = link
            .state()
            .connected
            .map(|c| (c.product, c.bus))
            .unwrap_or_default();
        name.clone_from(&product);
        route = opened;
        // Its board profile, read once now it's connected (#75).
        super::boards::connected(vid, pid, &product);
        let id = DeviceId::new(
            backend.clone(),
            vid,
            pid,
            None,
            &format!("{vid:04x}:{pid:04x}"),
        );
        let info = DeviceInfo {
            id: id.clone(),
            backend,
            name: product,
            kind: DeviceKind::Keyboard,
            firmware: None,
        };
        let device = Arc::new(FoxgloveLinkDevice::new(info, link, hello, bus().sink(id)));
        FRAMES.store(device.frames(), std::sync::atomic::Ordering::Relaxed);
        EFFECTS.store(device.effects(), std::sync::atomic::Ordering::Relaxed);
        EFFECT_LAMPS.store(device.effect_lamps(), std::sync::atomic::Ordering::Relaxed);
        // The learned ids only for the build they were learned under (#633).
        build = check(&device, device.link().state().generation);
        // Its LEDs by part, from the profile just read (#561): frames are that size, until the
        // firmware says (below).
        device.set_lamp_parts(super::boards::lamp_parts(super::boards::current().as_ref()));
        device
    });
    let out = cached.clone();
    drop(cached);
    // Nothing learned from the keymap yet (the overlay hasn't opened it): use the cached one,
    // so plugins can change the lights without the UI. Outside the lock: learning sets it.
    if let Some((vid, pid)) = found {
        // The bus of the interface the link opened, not of every interface the keyboard shows:
        // on USB and Bluetooth at once both are listed, and the link prefers USB.
        let transport = route.label();
        *HOST_LINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(transport);
        // Foxglove: the log said only "Waiting for a keyboard"; now it says when one arrives.
        let line = connected_line(transport, &name);
        if !LOGGED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .repeats(&line, std::time::Instant::now())
        {
            eprintln!("{line}");
        }
        super::keyboard_actions::learn_cached(vid, pid, build.as_deref());
        if let Some(d) = &out {
            learn_lamp_parts(d);
        }
    }
    out
}

/// The LEDs per part the keyboard found last reported (LAMP_PARTS, HELLO bit 22, #561); `None`
/// for firmware that doesn't say.
static FIRMWARE_PARTS: Mutex<Option<foxglove_link::lamps::LampParts>> = Mutex::new(None);

/// A keyboard connected: its own LED counts, when its firmware gives them, win over the profile's.
/// One request, on the connect.
fn learn_lamp_parts(d: &FoxgloveLinkDevice) {
    let parts = match d.read_lamp_parts() {
        Ok(p) => Some(p),
        Err(foxglove_devices::DeviceError::Unsupported) => None,
        Err(e) => {
            eprintln!("Foxglove: reading the keyboard's LED counts: {e}");
            None
        }
    };
    *FIRMWARE_PARTS.lock().unwrap_or_else(|e| e.into_inner()) = parts.clone();
    // Frames split (and calibrate) by the same counts the scenes size them by.
    d.set_lamp_parts(choose_parts(parts, super::boards::current().as_ref()));
}

/// The keyboard's LEDs by part (#561): what its firmware said when it connected, when that has
/// as many parts as `board` has halves; else the board profile's (`boards::lamp_parts`). Never
/// looks for a keyboard.
pub fn lamp_parts(board: Option<&super::boards::Board>) -> foxglove_link::lamps::LampParts {
    let firmware = FIRMWARE_PARTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    choose_parts(firmware, board)
}

/// The firmware's parts when they fit the board's halves, else the profile's.
fn choose_parts(
    firmware: Option<foxglove_link::lamps::LampParts>,
    board: Option<&super::boards::Board>,
) -> foxglove_link::lamps::LampParts {
    let halves = board.map_or(2, |b| if b.halves == 1 { 1 } else { 2 });
    firmware
        .filter(|p| p.len() == halves)
        .unwrap_or_else(|| super::boards::lamp_parts(board))
}

/// Whether the keyboard found last takes SET_FRAME frames (HELLO bit 14, #529). Never looks
/// for one, so the UI thread may ask.
pub fn frames() -> bool {
    FRAMES.load(std::sync::atomic::Ordering::Relaxed)
}

static FRAMES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the keyboard found last draws SET_EFFECT effects itself (HELLO bit 19, #542). Never
/// looks for one, so the UI thread may ask.
pub fn effects() -> bool {
    EFFECTS.load(std::sync::atomic::Ordering::Relaxed)
}

static EFFECTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the keyboard found last takes the LEDs' points for its effects (EFFECT LAMPS, HELLO
/// bit 21, #550). Never looks for one.
pub fn effect_lamps() -> bool {
    EFFECT_LAMPS.load(std::sync::atomic::Ordering::Relaxed)
}

static EFFECT_LAMPS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How Foxglove's host link reaches the keyboard found last (`USB` or `BLE`), which can differ
/// from how it types to this PC (USB Raw HID stays silent before firmware #434).
static HOST_LINK: Mutex<Option<&'static str>> = Mutex::new(None);

/// The host link's transport, once a keyboard with it was found (`USB` or `BLE`).
pub fn host_link() -> Option<&'static str> {
    *HOST_LINK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The last "Keyboard connected" line: two links answering for one connect logged it twice.
static LOGGED: Mutex<Logged> = Mutex::new(Logged(None));

struct Logged(Option<(String, std::time::Instant)>);

impl Logged {
    /// True when `line` was logged under [`SAME_CONNECT`] ago; records it otherwise.
    fn repeats(&mut self, line: &str, now: std::time::Instant) -> bool {
        if self
            .0
            .as_ref()
            .is_some_and(|(l, at)| l == line && now.duration_since(*at) < SAME_CONNECT)
        {
            return true;
        }
        self.0 = Some((line.to_string(), now));
        false
    }
}

/// How close together two finds count as the same connect.
const SAME_CONNECT: std::time::Duration = std::time::Duration::from_secs(2);

/// The log line for a keyboard that just connected: `Keyboard connected (USB): Eyelash Corne`.
fn connected_line(transport: &str, product: &str) -> String {
    if product.is_empty() {
        format!("Foxglove: Keyboard connected ({transport})")
    } else {
        format!("Foxglove: Keyboard connected ({transport}): {product}")
    }
}

/// Finds the keyboard with the host link each time a shared link connects, so the status strip,
/// the Keyboard page and plugins see a keyboard that connects after startup (or answers only
/// after a reconnect). Before, [`device`] ran only when something asked, and a "no keyboard"
/// from before the connection stayed until the next ask. Start: a link's State event with a
/// connection. Stop: none needed; the worker blocks on its channel, so an idle keyboard costs
/// no wake-ups. A link that drops takes the device's behavior ids with it (#633): the next
/// connection may run another build, and it gets them back only once that's checked.
pub fn start() {
    enum Msg {
        Link(Arc<foxglove_link::link::Link>),
        Connected,
        Lost(std::sync::Weak<foxglove_link::link::Link>),
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let links_tx = tx.clone();
    super::services::spawn_named("keyboard-discovery", move || {
        // Listeners only enqueue: the worker subscribes and discovers.
        let _links = super::link::subscribe_links(move |link| {
            let _ = links_tx.send(Msg::Link(link));
        });
        let mut subscriptions = Vec::new();
        for msg in rx {
            match msg {
                Msg::Link(link) => {
                    let tx = tx.clone();
                    let weak = Arc::downgrade(&link);
                    subscriptions.push(link.subscribe_events(move |event| {
                        if let foxglove_link::link::Event::State(s) = event {
                            let _ = tx.send(if s.connected.is_some() {
                                Msg::Connected
                            } else {
                                Msg::Lost(weak.clone())
                            });
                        }
                    }));
                }
                Msg::Connected => {
                    let _ = device();
                }
                Msg::Lost(link) => lost(&link),
            }
        }
    });
}

/// `link` dropped: if it's the device's, the device keeps no ids until its next connection is
/// checked (#633). The learned cache stays for a connection on the same build.
fn lost(link: &std::sync::Weak<foxglove_link::link::Link>) {
    let cached = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = cached
        .as_ref()
        .filter(|d| std::ptr::eq(link.as_ptr(), Arc::as_ptr(d.link())))
    {
        forget_ids(d);
    }
}

/// `d` invokes nothing until it is checked again.
fn forget_ids(d: &FoxgloveLinkDevice) {
    d.set_behaviors(Vec::new());
    d.set_underglow(None);
    *CHECKED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The current device's connection generation, without blocking or discovery (for the UI).
pub fn generation() -> Option<u64> {
    let cached = DEVICE.try_lock().ok()?;
    let state = cached.as_ref()?.link().state();
    state.connected.is_some().then_some(state.generation)
}

/// The connected keyboard's key in the colour calibration registry, without blocking or
/// discovery (for the UI).
pub fn keyboard_key() -> Option<String> {
    let cached = DEVICE.try_lock().ok()?;
    let d = cached.as_ref()?;
    d.link()
        .state()
        .connected
        .is_some()
        .then(|| d.calibration_key())
}

/// The connected keyboard's device id (as the lighting arbiter keys it), without blocking or
/// discovery (for the UI).
pub fn keyboard_id() -> Option<String> {
    use foxglove_devices::Device as _;
    let cached = DEVICE.try_lock().ok()?;
    let d = cached.as_ref()?;
    d.link()
        .state()
        .connected
        .is_some()
        .then(|| d.info().id.to_string())
}

/// What the app knows of the keyboard's lighting command, the keymap's `&rgb_ug` (#633).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Underglow {
    /// No keymap learned yet: a lighting write can't go out, but may once it is.
    Unknown,
    /// The keymap has no `&rgb_ug`: lighting writes are not supported.
    Absent,
    /// Its behavior id.
    Known(u16),
}

/// What is known of the lighting command, and a condvar that wakes whoever waits for it to
/// become known (#655: the local API's lighting writes). Set by the keymap read, never polled.
pub struct LightingKnown {
    now: Mutex<Underglow>,
    changed: Condvar,
}

impl LightingKnown {
    pub const fn new() -> LightingKnown {
        LightingKnown {
            now: Mutex::new(Underglow::Unknown),
            changed: Condvar::new(),
        }
    }

    /// What is known now.
    pub fn get(&self) -> Underglow {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sets it; a waiter wakes when it is no longer `Unknown`.
    pub fn set(&self, now: Underglow) {
        *self.now.lock().unwrap_or_else(|e| e.into_inner()) = now;
        self.changed.notify_all();
    }

    /// A keymap was learned with `underglow`: whether that's news (see [`learned`]).
    fn learn(&self, underglow: Option<u16>) -> bool {
        let news = {
            let mut known = self.now.lock().unwrap_or_else(|e| e.into_inner());
            let (now, news) = learned(*known, underglow);
            *known = now;
            news
        };
        self.changed.notify_all();
        news
    }

    /// Waits until the lighting command is known (or known absent), at most `timeout`; returns
    /// what is known then (`Unknown` when the timeout passed first). Starts at the call, stops
    /// at the keymap read's [`LightingKnown::set`]/[`set_actions`]; the timeout is the fallback.
    pub fn wait_known(&self, timeout: std::time::Duration) -> Underglow {
        let now = self.now.lock().unwrap_or_else(|e| e.into_inner());
        let (now, _) = self
            .changed
            .wait_timeout_while(now, timeout, |u| *u == Underglow::Unknown)
            .unwrap_or_else(|e| e.into_inner());
        *now
    }
}

static UNDERGLOW: LightingKnown = LightingKnown::new();

/// What the app knows of the keyboard's lighting command now.
pub fn underglow() -> Underglow {
    UNDERGLOW.get()
}

/// The keyboard's lighting command, to wait on (#655).
pub fn lighting_known() -> &'static LightingKnown {
    &UNDERGLOW
}

/// A keymap was learned with `underglow`: what is known now, and whether that's news (the
/// lighting writers waiting on it run then; the same keymap read again isn't).
fn learned(before: Underglow, underglow: Option<u16>) -> (Underglow, bool) {
    let now = underglow.map_or(Underglow::Absent, Underglow::Known);
    (now, now != before)
}

/// What the keymap lets the app invoke (the allowlist) and its `&rgb_ug` behavior, learned
/// under firmware `build` (#633: [`firmware_build`] for a live read, the saved copy's for a
/// replay; `None` when it couldn't be read, so the next connection doesn't trust them).
pub fn set_actions(actions: Vec<BehaviorInfo>, underglow: Option<u16>, build: Option<String>) {
    // The cache first: a device found meanwhile takes it, else it's set below.
    *ACTIONS.lock().unwrap_or_else(|e| e.into_inner()) = Learned {
        actions: actions.clone(),
        underglow,
        build,
    };
    if let Some(d) = DEVICE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        d.set_behaviors(actions);
        d.set_underglow(underglow);
    }
    let news = UNDERGLOW.learn(underglow);
    // Once the device has the id: a write that waited on it goes out now (#633).
    if news {
        super::reassert::fire(super::reassert::Trigger::LightingKnown);
    }
}

/// The firmware build of the keyboard connected now, which numbers its keymap's behaviors
/// (#633): the left half's (the central's) D7 09 reply, checked once per connection. `None`
/// with no keyboard, or when its build can't be read. May find the keyboard: off the UI thread.
pub fn firmware_build() -> Option<String> {
    device()?;
    BUILD.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A build as one comparable string: version, profile and short SHA.
fn build_key(b: &foxglove_link::protocol::BuildInfo) -> String {
    let v = &b.version;
    format!(
        "{}.{}.{}-{}-{}",
        v.major, v.minor, v.patch, v.profile, b.sha
    )
}

/// The left half's build on `link`'s connection; `None` for firmware without BUILD_INFO (HELLO
/// bit 12) or when it doesn't answer.
fn read_build(link: &foxglove_link::link::Link) -> Option<String> {
    let caps = link
        .state()
        .connected
        .and_then(|c| c.hello)
        .map_or(0, |h| h.capabilities);
    if caps & foxglove_link::protocol::capabilities::BUILD_INFO == 0 {
        return None;
    }
    match foxglove_link::settings::build_info(link, 0) {
        Ok(b) => Some(build_key(&b)),
        Err(e) => {
            eprintln!("Foxglove: can't read the keyboard's firmware build: {e}");
            None
        }
    }
}

/// `d`'s connection `generation` runs firmware `build` (#633): the ids learned under that build
/// go to the device; any others are dropped before anything can invoke them, and the lighting
/// command is `Unknown` again until a keymap is learned. Returns the build.
fn check(d: &FoxgloveLinkDevice, generation: u64) -> Option<String> {
    let build = read_build(d.link());
    let (actions, underglow) = connected_build(build.clone());
    d.set_behaviors(actions);
    d.set_underglow(underglow);
    *CHECKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(generation);
    build
}

/// Checks `d` again when its link is on a connection it wasn't checked on: `Some(build)` then.
fn recheck(d: &FoxgloveLinkDevice) -> Option<Option<String>> {
    let generation = d.link().state().generation;
    if *CHECKED.lock().unwrap_or_else(|e| e.into_inner()) == Some(generation) {
        return None;
    }
    Some(check(d, generation))
}

/// The keyboard connected with firmware `build` (`None`: unreadable). The learned ids when they
/// are that build's; else nothing, with the cache, the actions and the lighting command
/// forgotten until a keymap is learned again (`set_actions` then fires `LightingKnown`).
fn connected_build(build: Option<String>) -> (Vec<BehaviorInfo>, Option<u16>) {
    BUILD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone_from(&build);
    let mut learned = ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
    if build.is_some() && learned.build == build {
        return (learned.actions.clone(), learned.underglow);
    }
    let had = !learned.actions.is_empty() || learned.underglow.is_some();
    let was = std::mem::take(&mut *learned).build;
    drop(learned);
    super::keyboard_actions::forget();
    UNDERGLOW.set(Underglow::Unknown);
    if had {
        let on = was.unwrap_or_else(|| "an unknown build".into());
        match &build {
            Some(now) => eprintln!(
                "Foxglove: the keyboard runs firmware {now}, its behaviors were learned on {on}: \
                 they wait for the keymap to be read again"
            ),
            None => eprintln!(
                "Foxglove: the keyboard's firmware build can't be read: its behaviors (learned on \
                 {on}) wait for the keymap to be read again"
            ),
        }
    }
    (Vec::new(), None)
}

/// A device-layer error in the words the Keyboard panel uses.
pub fn describe(e: &foxglove_devices::DeviceError) -> String {
    match e {
        foxglove_devices::DeviceError::Unsupported => {
            "not supported by this keyboard or keymap".into()
        }
        foxglove_devices::DeviceError::Disconnected => "the keyboard is not connected".into(),
        // A lost reply (a transport switch, a busy link); a missing feature is `Unsupported`,
        // decided from HELLO's capability bits.
        foxglove_devices::DeviceError::Timeout => "no answer".into(),
        foxglove_devices::DeviceError::Busy(why)
        | foxglove_devices::DeviceError::Denied(why)
        | foxglove_devices::DeviceError::Backend(why) => why.clone(),
    }
}

/// Held by tests that change the learned behaviors, the lighting command or the device: they
/// are process-wide.
#[cfg(test)]
pub(crate) static STATE_LOCK: Mutex<()> = Mutex::new(());

/// #633: the behavior ids follow the firmware build, against an in-memory keyboard on a real
/// host link that can be "flashed" (reconnected with another build). Window-free; the saved
/// keymap copies go to the tests' temp `FOXGLOVE_CONFIG_DIR`.
#[cfg(test)]
mod build_tests {
    use super::*;
    use crate::foxglove::keyboard_actions;
    use crate::foxglove::reassert::Trigger;
    use foxglove_link::link::{Bus, Connected, Event, Link, RawHid, Report};
    use foxglove_link::protocol::{self, capabilities, Hello, REPORT};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Condvar, MutexGuard};
    use std::time::Duration;
    use zmk_studio_api::proto::zmk::keymap::{BehaviorBinding, Keymap, Layer};
    use zmk_studio_api::{Behavior, ResolvedLayer, UnderglowCommand};

    const WAIT: Duration = Duration::from_secs(5);
    const VID: u16 = 0xF633;

    /// What the keyboard runs and what it was asked, across connections.
    #[derive(Default)]
    struct Board {
        /// Its build's short SHA; `None`: firmware without BUILD_INFO (HELLO bit 12).
        build: Mutex<Option<&'static str>>,
        build_reads: AtomicU32,
        /// The behavior id of every INVOKE that reached it.
        invokes: Mutex<Vec<u16>>,
    }

    /// One connection to the board.
    struct Conn {
        board: Arc<Board>,
        inbox: (Mutex<VecDeque<Report>>, Condvar),
        interrupted: AtomicBool,
    }

    impl RawHid for Conn {
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
            let mut r = [0u8; REPORT];
            match (out[0], out[1]) {
                (protocol::SETTINGS, protocol::settings::BUILD_INFO) => {
                    self.board.build_reads.fetch_add(1, Ordering::SeqCst);
                    let sha = self.board.build.lock().unwrap().unwrap_or_default();
                    r[..8].copy_from_slice(&[0xD7, 0x89, 0, out[2], 0, 1, 0, 0]);
                    r[8..8 + sha.len()].copy_from_slice(sha.as_bytes());
                }
                (protocol::SETTINGS, protocol::settings::INVOKE) => {
                    let id = u16::from_le_bytes([out[2], out[3]]);
                    self.board.invokes.lock().unwrap().push(id);
                    r[..3].copy_from_slice(&[0xD7, 0x82, 0]);
                }
                _ => return Ok(()),
            }
            self.inbox.0.lock().unwrap().push_back(r);
            self.inbox.1.notify_all();
            Ok(())
        }
    }

    fn hello(board: &Board) -> Hello {
        let build = board
            .build
            .lock()
            .unwrap()
            .map_or(0, |_| capabilities::BUILD_INFO);
        Hello {
            version: 1,
            capabilities: capabilities::SETTINGS_RGB | build,
            ..Hello::default()
        }
    }

    /// The board on a host link, as the cached device (`DEVICE`), with nothing learned yet.
    struct Rig {
        board: Arc<Board>,
        link: Arc<Link>,
        device: Arc<FoxgloveLinkDevice>,
        _state: MutexGuard<'static, ()>,
    }

    impl Rig {
        fn new(pid: u16, build: Option<&'static str>) -> Rig {
            let state = STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            crate::foxglove::store::test_config_dir();
            let board = Arc::new(Board::default());
            *board.build.lock().unwrap() = build;
            let opened = board.clone();
            let link = Arc::new(Link::start(Box::new(move || {
                let conn = Conn {
                    board: opened.clone(),
                    inbox: Default::default(),
                    interrupted: AtomicBool::new(false),
                };
                let connected = Connected {
                    product: "Fake".into(),
                    hello: Some(hello(&opened)),
                    bus: Bus::Unknown,
                };
                Ok((Box::new(conn) as Box<dyn RawHid>, connected))
            })));
            link.wait_connected(WAIT).unwrap();
            let id = DeviceId::new(BackendId("zmk".into()), VID, pid, None, "fake");
            let info = DeviceInfo {
                id: id.clone(),
                backend: id.backend.clone(),
                name: "Fake".into(),
                kind: DeviceKind::Keyboard,
                firmware: None,
            };
            let device = Arc::new(FoxgloveLinkDevice::new(
                info,
                link.clone(),
                hello(&board),
                EventBus::default().sink(id),
            ));
            *ACTIONS.lock().unwrap() = Learned::default();
            keyboard_actions::forget();
            UNDERGLOW.set(Underglow::Unknown);
            *CHECKED.lock().unwrap() = None;
            *BUILD.lock().unwrap() = None;
            *DEVICE.lock().unwrap() = Some(device.clone());
            heard_lighting_known();
            Rig {
                board,
                link,
                device,
                _state: state,
            }
        }

        /// The board restarts on firmware `build` (a flash, or a replug when it's the same) and
        /// the link connects to it again.
        fn flash(&self, build: Option<&'static str>) {
            *self.board.build.lock().unwrap() = build;
            let before = self.link.state().generation;
            let (tx, rx) = std::sync::mpsc::channel();
            let _events = self.link.subscribe_events(move |e| {
                if let Event::State(s) = e {
                    let _ = tx.send(s);
                }
            });
            self.link.reconnect();
            loop {
                let s = rx.recv_timeout(WAIT).expect("the link reconnects");
                if s.connected.is_some() && s.generation > before {
                    break;
                }
            }
        }

        /// A lighting command, as the accent or a lease sends it.
        fn light(&self) -> foxglove_devices::Result<()> {
            self.device.lighting().unwrap().set_on(true)
        }

        fn invokes(&self) -> Vec<u16> {
            std::mem::take(&mut *self.board.invokes.lock().unwrap())
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            *DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    /// LightingKnown fires heard since the last call.
    fn heard_lighting_known() -> u32 {
        static HEARD: AtomicU32 = AtomicU32::new(0);
        static HOOK: std::sync::Once = std::sync::Once::new();
        HOOK.call_once(|| {
            crate::foxglove::reassert::register("test-633", |t| {
                if t == Trigger::LightingKnown {
                    HEARD.fetch_add(1, Ordering::SeqCst);
                }
            });
        });
        HEARD.swap(0, Ordering::SeqCst)
    }

    /// A live keymap read whose `&rgb_ug` is behavior `underglow`, under firmware `build`.
    fn read_keymap(underglow: u16, build: Option<String>) {
        let resolved = vec![ResolvedLayer {
            id: 0,
            name: "Base".into(),
            bindings: vec![Behavior::Underglow(UnderglowCommand::On)],
        }];
        let raw = Keymap {
            layers: vec![Layer {
                id: 0,
                name: String::new(),
                bindings: vec![BehaviorBinding {
                    behavior_id: underglow.into(),
                    param1: 1,
                    param2: 0,
                }],
            }],
            ..Default::default()
        };
        keyboard_actions::learn(&resolved, &raw, build);
    }

    /// The build key the fake's SHA reads as.
    fn key(sha: &str) -> Option<String> {
        Some(format!("0.1.0-0-{sha}"))
    }

    #[test]
    fn the_same_build_keeps_the_learned_ids_without_reading_the_keymap() {
        let rig = Rig::new(1, Some("aaaa1111"));
        assert_eq!(firmware_build(), key("aaaa1111"));
        read_keymap(4, firmware_build());
        assert_eq!(heard_lighting_known(), 1);
        // A copy that would replay as "no lighting command": reading it would show.
        crate::foxglove::keymap_cache::save_fake(VID, 1, key("aaaa1111").as_deref(), false);

        // The link drops: the device keeps no ids while it's away.
        lost(&Arc::downgrade(&rig.link));
        assert!(rig.light().is_err());
        assert!(rig.invokes().is_empty());

        rig.flash(Some("aaaa1111"));
        let reads = rig.board.build_reads.load(Ordering::SeqCst);
        assert!(Arc::ptr_eq(&device().unwrap(), &rig.device));
        assert_eq!(
            rig.board.build_reads.load(Ordering::SeqCst),
            reads + 1,
            "checked once"
        );
        assert_eq!(underglow(), Underglow::Known(4), "kept, not read again");
        assert_eq!(heard_lighting_known(), 0, "nothing new");
        rig.light().unwrap();
        assert_eq!(rig.invokes(), [4]);
        assert!(keyboard_actions::permitted((4, 1, 0)));
        // Asked again on the same connection: no new check.
        device().unwrap();
        assert_eq!(rig.board.build_reads.load(Ordering::SeqCst), reads + 1);
    }

    #[test]
    fn a_new_build_drops_the_ids_before_anything_invokes_them_then_learns_again() {
        let rig = Rig::new(2, Some("aaaa1111"));
        read_keymap(4, firmware_build());
        crate::foxglove::keymap_cache::save_fake(VID, 2, key("aaaa1111").as_deref(), false);
        heard_lighting_known();

        rig.flash(Some("bbbb2222"));
        // The first ask on the new connection checks it, before anyone gets the device.
        let d = device().unwrap();
        assert_eq!(firmware_build(), key("bbbb2222"));
        assert_eq!(underglow(), Underglow::Unknown, "lighting waits");
        assert!(keyboard_actions::actions().is_empty());
        assert!(!keyboard_actions::permitted((4, 1, 0)));
        assert!(d.behaviors().unwrap().list().unwrap().is_empty());
        assert!(rig.light().is_err());
        assert!(d.behaviors().unwrap().invoke(4, 1, 0, true).is_err());
        assert!(rig.invokes().is_empty(), "the old id never went out");
        // The other build's saved copy isn't replayed either.
        assert!(!keyboard_actions::learn_cached(
            VID,
            2,
            firmware_build().as_deref()
        ));
        assert_eq!(underglow(), Underglow::Unknown);
        assert_eq!(heard_lighting_known(), 0);

        // The keymap read from the new firmware: the new id, and the lighting writers wake once.
        read_keymap(9, firmware_build());
        assert_eq!(underglow(), Underglow::Known(9));
        assert_eq!(heard_lighting_known(), 1);
        rig.light().unwrap();
        assert_eq!(rig.invokes(), [9]);
    }

    #[test]
    fn an_unreadable_build_trusts_nothing_until_the_keymap_is_read() {
        let rig = Rig::new(3, Some("aaaa1111"));
        read_keymap(4, firmware_build());
        crate::foxglove::keymap_cache::save_fake(VID, 3, key("aaaa1111").as_deref(), false);

        // Firmware that doesn't say its build.
        rig.flash(None);
        device().unwrap();
        assert_eq!(firmware_build(), None);
        assert_eq!(underglow(), Underglow::Unknown);
        assert!(
            !keyboard_actions::learn_cached(VID, 3, None),
            "no copy is trusted"
        );
        assert!(rig.light().is_err());
        assert!(rig.invokes().is_empty());

        // Read from the keyboard: good for this connection only.
        read_keymap(9, firmware_build());
        rig.light().unwrap();
        assert_eq!(rig.invokes(), [9]);
        rig.flash(None);
        device().unwrap();
        assert_eq!(
            underglow(),
            Underglow::Unknown,
            "not trusted on the next one"
        );
        assert!(rig.light().is_err());
        assert!(rig.invokes().is_empty());
    }

    #[test]
    fn a_saved_copy_is_learned_only_on_the_build_it_was_read_from() {
        let rig = Rig::new(4, Some("cccc3333"));
        let now = firmware_build();
        // From before #633 (no build), and from another build: not learned.
        crate::foxglove::keymap_cache::save_fake(VID, 4, now.as_deref(), true);
        assert!(!keyboard_actions::learn_cached(VID, 4, now.as_deref()));
        crate::foxglove::keymap_cache::save_fake(VID, 4, key("aaaa1111").as_deref(), false);
        assert!(!keyboard_actions::learn_cached(VID, 4, now.as_deref()));
        crate::foxglove::keymap_cache::save_fake(VID, 4, None, false);
        assert!(!keyboard_actions::learn_cached(VID, 4, now.as_deref()));
        assert_eq!(underglow(), Underglow::Unknown);
        // Read from this build: learned (the fake keymap has no `&rgb_ug`).
        crate::foxglove::keymap_cache::save_fake(VID, 4, now.as_deref(), false);
        assert!(keyboard_actions::learn_cached(VID, 4, now.as_deref()));
        assert_eq!(underglow(), Underglow::Absent);
        assert_eq!(heard_lighting_known(), 1);
        drop(rig);
    }

    /// #639: the local API's `behaviors.list` and `behaviors.invoke` answer from the keymap read
    /// on the build the keyboard runs: "not known yet" before that, then the allowlist, and the
    /// app's refusals (`&bt`, `&sys_reset` never invocable) hold.
    #[test]
    fn the_api_lists_and_invokes_only_the_trusted_keymaps_behaviors() {
        use crate::foxglove::api::{behaviors_invoke, behaviors_list};
        use foxglove_plugin_api::methods::{Behaviors, InvokeParams};
        use foxglove_plugin_api::rpc::code;
        use zmk_studio_api::BluetoothCommand;
        let rig = Rig::new(5, Some("aaaa1111"));
        let d = device().unwrap();
        let invoke = |id, param1| {
            behaviors_invoke(
                d.as_ref(),
                &InvokeParams {
                    device: d.info().id.to_string(),
                    id,
                    param1,
                    param2: 0,
                },
            )
        };
        let unknown = behaviors_list(d.as_ref()).unwrap_err();
        assert_eq!(unknown.code, code::BUSY, "{}", unknown.message);
        assert!(unknown.message.contains("aren't known yet"));
        assert_eq!(invoke(4, 1).unwrap_err().code, code::BUSY);
        assert!(rig.invokes().is_empty());

        // The keymap read from this build: `&rgb_ug RGB_ON` (4), `&bt BT_NXT` (7), `&sys_reset` (8).
        let bindings = [
            (Behavior::Underglow(UnderglowCommand::On), 4, 1),
            (Behavior::Bluetooth(BluetoothCommand::Next), 7, 1),
            (Behavior::Reset, 8, 0),
        ];
        let resolved = vec![ResolvedLayer {
            id: 0,
            name: "Base".into(),
            bindings: bindings.iter().map(|(b, _, _)| b.clone()).collect(),
        }];
        let raw = Keymap {
            layers: vec![Layer {
                id: 0,
                name: String::new(),
                bindings: bindings
                    .iter()
                    .map(|(_, id, p1)| BehaviorBinding {
                        behavior_id: *id,
                        param1: *p1,
                        param2: 0,
                    })
                    .collect(),
            }],
            ..Default::default()
        };
        keyboard_actions::learn(&resolved, &raw, firmware_build());
        let list: Behaviors = serde_json::from_value(behaviors_list(d.as_ref()).unwrap()).unwrap();
        let ids: Vec<u32> = list.behaviors.iter().map(|b| b.id).collect();
        assert_eq!(ids, [4], "only the allowlist: {:?}", list.behaviors);
        assert!(!list.behaviors[0].needs_confirmation);
        invoke(4, 1).unwrap();
        assert_eq!(rig.invokes(), [4]);
        for (id, p1) in [(7, 1), (8, 0), (4, 2)] {
            assert_eq!(invoke(id, p1).unwrap_err().code, code::UNSUPPORTED, "{id}");
        }
        assert!(rig.invokes().is_empty(), "nothing refused went out");
    }
}

#[cfg(test)]
mod tests {
    use foxglove_link::lamps::LampParts;

    /// #633: the lighting command becoming known (or changing, or going) is news once; the same
    /// keymap learned again isn't.
    #[test]
    fn the_lighting_command_is_news_only_when_it_changes() {
        use super::{learned, Underglow::*};
        assert_eq!(learned(Unknown, Some(4)), (Known(4), true));
        assert_eq!(learned(Known(4), Some(4)), (Known(4), false));
        assert_eq!(learned(Known(4), Some(5)), (Known(5), true));
        assert_eq!(learned(Unknown, None), (Absent, true));
        assert_eq!(learned(Absent, None), (Absent, false));
        assert_eq!(learned(Absent, Some(4)), (Known(4), true));
    }

    /// The firmware's LED counts (#561) win when they fit the board's halves; else the
    /// profile's, as for firmware that doesn't say.
    #[test]
    fn the_firmwares_lamp_parts_win_when_they_fit_the_board() {
        use crate::foxglove::fixtures::{second_desk, users_desk};
        let corne = users_desk::board();
        let uneven = LampParts::new(&[30, 24]).unwrap();
        assert_eq!(
            super::choose_parts(Some(uneven.clone()), Some(&corne)),
            uneven
        );
        assert_eq!(
            super::choose_parts(None, Some(&corne)),
            LampParts::old_firmware()
        );
        // Two parts from the firmware on a one-piece board's profile: the profile's 61.
        let plank = second_desk::board();
        assert_eq!(super::choose_parts(Some(uneven), Some(&plank)).total(), 61);
        let one = LampParts::new(&[61]).unwrap();
        assert_eq!(super::choose_parts(Some(one.clone()), Some(&plank)), one);
    }

    /// One connect found twice is logged once; a later reconnect or another transport is not
    /// a repeat.
    #[test]
    fn a_connect_found_twice_is_logged_once() {
        let mut logged = super::Logged(None);
        let t = std::time::Instant::now();
        let usb = super::connected_line("USB", "Eyelash Corne");
        assert!(!logged.repeats(&usb, t));
        assert!(logged.repeats(&usb, t + std::time::Duration::from_millis(300)));
        assert!(!logged.repeats(&usb, t + super::SAME_CONNECT * 2));
        let ble = super::connected_line("BLE", "Eyelash_Corne");
        assert!(!logged.repeats(&ble, t + super::SAME_CONNECT * 2));
    }

    #[test]
    fn a_connect_is_logged_with_its_transport() {
        assert_eq!(
            super::connected_line("USB", "Eyelash Corne"),
            "Foxglove: Keyboard connected (USB): Eyelash Corne"
        );
        assert_eq!(
            super::connected_line("BLE", ""),
            "Foxglove: Keyboard connected (BLE)"
        );
    }
}
