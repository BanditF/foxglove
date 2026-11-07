//! Foxglove as Windows' background light controller for the keyboard's own LampArrays (#179).
//!
//! With package identity Windows' "Background light control" order decides who drives each
//! Dynamic Lighting device while no app is in front. A device whose LampArray no app on the
//! list holds goes to Windows' own controller, which turns the keyboard's autonomous mode off,
//! and then the firmware refuses Foxglove's host link (it gives a LampArray the lights first).
//!
//! So while Foxglove runs with identity it **holds** every LampArray of the host-link keyboard
//! (over USB and over Bluetooth, which Windows lists as two devices), and never writes to them:
//!
//! - **Hypothesis H1** (the lab case `dl-hold-keeps-windows-out`, tests/hil/dl-hold.toml): a
//!   LampArray held by the controller Windows ranks first, with no write, keeps every other
//!   controller out and leaves the keyboard on its host link, so scenes, effects, battery dim
//!   and the art keep their richer path. The firmware's take-over counter (D7 90) says whether
//!   it held.
//! - **Fallback if H1 fails** (the firmware says a LampArray took the lights although Foxglove
//!   holds it): the keyboard is drawn through Windows, the existing #749 b path
//!   (`lights_owner::through_windows`), and [`draw_on`] limits those writes to the transport
//!   the host link uses now; the other one stays held without writing.
//! - `lights_owner` asks [`holds`] so an owner byte saying "LampArray" over a transport
//!   Foxglove holds is Foxglove, not "Windows holds the lights".
//!
//! The holds follow events only: a keyboard connect (the devices bus) and the Lights worker's
//! scan of Dynamic Lighting devices ([`devices_changed`]) reopen them; a watch on the LampArrays
//! Windows lists ([`Watched`]) opens one that appears later (the Bluetooth one usually comes after
//! USB) and lets go of one that goes; a connect of the same keyboard opens a transport Windows
//! lists but Foxglove doesn't hold; `AvailabilityChanged` updates who has each one. Every open
//! logs what it held and missed per transport, and the health view warns about a listed one not
//! held (`lamp_health::unheld_rows`). Quit lets them go ([`stop`]), as does the process ending: Windows
//! then hands each device to the next controller. **Pause lets them go** too, as scenes do: the
//! user reads pause as hands off, so Windows' order decides meanwhile; resuming holds them again
//! ([`pause_changed`]). Away and lock keep them (nothing is written, and letting go would hand
//! the keyboard to Windows' controller).
//! Without identity none of this runs: an unpackaged Foxglove only drives LampArrays from the
//! front.

use std::sync::{mpsc, Arc, Mutex};

use super::scene_lamps::{Changed, LampDevice};
use foxglove_link::lights_owner::Owner;

/// What Foxglove holds now: per transport, `Some(available)` while it holds that LampArray.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    pub usb: Option<bool>,
    pub ble: Option<bool>,
}

impl Holds {
    /// Foxglove holds the transport `owner` names, and Windows lets it drive it.
    pub fn ours(&self, owner: Owner) -> bool {
        match owner {
            Owner::LampArrayUsb => self.usb == Some(true),
            Owner::LampArrayBle => self.ble == Some(true),
            _ => false,
        }
    }
}

static HOLDS: Mutex<Holds> = Mutex::new(Holds {
    usb: None,
    ble: None,
});

/// What Foxglove holds now.
pub fn now() -> Holds {
    *HOLDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The owner byte names a LampArray Foxglove itself holds (with Windows' leave to drive it).
pub fn holds(owner: Owner) -> bool {
    now().ours(owner)
}

/// One LampArray Foxglove holds.
pub struct HeldArray {
    /// Windows' device id.
    pub id: String,
    /// Over Bluetooth LE, not USB.
    pub ble: bool,
    pub device: Box<dyn LampDevice>,
}

/// What one open found.
#[derive(Default)]
pub struct Opened {
    /// The keyboard's LampArrays (its VID/PID).
    pub held: Vec<HeldArray>,
    /// The LampArrays Windows lists, any device's.
    pub listed: usize,
    /// Those skipped because they report another vendor and product.
    pub skipped: usize,
}

/// Opens the LampArrays of one keyboard (Windows', or a fake in tests).
pub trait HoldOpener: Send {
    fn open(&mut self, vid: u16, pid: u16, changed: Changed) -> Opened;
}

/// Which transports Windows lists a LampArray of this keyboard on (the Lights worker's scan):
/// `(usb, ble)`.
pub type Listed = Box<dyn Fn(u16, u16) -> (bool, bool) + Send>;

/// The LampArray watch's events (`scene_lamps::watch_lamp_arrays`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Watched {
    /// The arrays already there are listed.
    Enumerated,
    /// One appeared (its device id).
    Added(String),
    /// One went.
    Removed(String),
}

enum Job {
    /// The host-link keyboard now (its VID/PID), or none.
    Keyboard(Option<(u16, u16)>),
    /// The Dynamic Lighting devices changed: open them afresh.
    Devices,
    /// Windows changed who may drive one of them.
    Changed,
    /// Windows lists a new LampArray (or finished listing them): open any not held.
    Listed,
    /// Windows no longer lists this LampArray: let it go.
    Gone(String),
    /// Foxglove paused (let every hold go) or resumed (hold them again).
    Paused(bool),
    Stop,
}

/// The holder's state, driven one batch of jobs at a time (synchronously in tests).
struct Core {
    opener: Box<dyn HoldOpener>,
    keyboard: Option<(u16, u16)>,
    held: Vec<HeldArray>,
    /// The transports Windows lists the keyboard's LampArrays on.
    listed: Listed,
    tx: mpsc::Sender<Job>,
    holds: Holds,
    /// Foxglove is paused: nothing is held, and nothing is opened until it resumes.
    paused: bool,
    /// Told when what Foxglove holds changes (a held keyboard retries its host link once).
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Core {
    /// Handles a batch; false once stopped.
    fn handle(&mut self, jobs: Vec<Job>) -> bool {
        let mut reopen = false;
        // Open what isn't held, keeping what is.
        let mut fill = false;
        for job in jobs {
            match job {
                Job::Keyboard(k) => {
                    if k != self.keyboard {
                        self.keyboard = k;
                        reopen = true;
                    } else {
                        // The same keyboard again: a transport Windows lists may have come
                        // since the first open.
                        let (usb, ble) = self.missing();
                        fill |= usb || ble;
                    }
                }
                Job::Devices => reopen = true,
                Job::Changed => {}
                Job::Listed => fill = true,
                Job::Gone(id) => {
                    let before = self.held.len();
                    self.held.retain(|h| h.id != id);
                    fill |= self.held.len() != before;
                }
                Job::Paused(p) => {
                    if p != self.paused {
                        self.paused = p;
                        reopen = true;
                    }
                }
                Job::Stop => {
                    self.held.clear();
                    self.publish();
                    return false;
                }
            }
        }
        if reopen {
            // Let go first: one unplugged and back is a new device to Windows.
            self.held.clear();
        }
        if reopen || fill {
            self.open(if reopen {
                "a reopen"
            } else {
                "a LampArray change"
            });
        }
        self.publish();
        true
    }

    /// Opens the keyboard's LampArrays, adding those not held yet (the held ones stay as they
    /// are), and logs what each transport got.
    fn open(&mut self, why: &str) {
        let Some((vid, pid)) = self.keyboard.filter(|_| !self.paused) else {
            return;
        };
        let tx = self.tx.clone();
        let changed: Changed = Arc::new(move || {
            let _ = tx.send(Job::Changed);
        });
        let opened = self.opener.open(vid, pid, changed);
        for a in opened.held {
            // A second handle on one held is dropped (closed) here.
            if !self.held.iter().any(|h| h.id == a.id) {
                self.held.push(a);
            }
        }
        let (usb, ble) = (self.listed)(vid, pid);
        eprintln!(
            "Foxglove: keyboard LampArrays after {why}: USB {}, BLE {} ({} LampArrays listed, {}              skipped by VID/PID)",
            claim_words(self.has(false), usb),
            claim_words(self.has(true), ble),
            opened.listed,
            opened.skipped
        );
    }

    /// Holds one of this transport's arrays.
    fn has(&self, ble: bool) -> bool {
        self.held.iter().any(|h| h.ble == ble)
    }

    /// Per transport `(usb, ble)`: Windows lists the keyboard's LampArray there, and none is
    /// held. Nothing is missing while paused or without a keyboard.
    fn missing(&self) -> (bool, bool) {
        let Some((vid, pid)) = self.keyboard.filter(|_| !self.paused) else {
            return (false, false);
        };
        let (usb, ble) = (self.listed)(vid, pid);
        (usb && !self.has(false), ble && !self.has(true))
    }

    fn publish(&mut self) {
        let side = |ble: bool| {
            let mine: Vec<bool> = self
                .held
                .iter()
                .filter(|h| h.ble == ble)
                .map(|h| h.device.available())
                .collect();
            (!mine.is_empty()).then(|| mine.iter().any(|a| *a))
        };
        let holds = Holds {
            usb: side(false),
            ble: side(true),
        };
        if holds != self.holds {
            eprintln!(
                "Foxglove: keyboard LampArrays held: USB {}, BLE {}",
                words(holds.usb),
                words(holds.ble)
            );
            self.holds = holds;
            *HOLDS.lock().unwrap_or_else(|e| e.into_inner()) = holds;
            (self.notify)();
        }
    }
}

/// One transport in the open's log line: held, missed (Windows lists it, nothing held), or not
/// listed.
fn claim_words(held: bool, listed: bool) -> &'static str {
    match (held, listed) {
        (true, _) => "held",
        (false, true) => "MISSED (listed by Windows, not held)",
        (false, false) => "not listed",
    }
}

/// The transports Windows lists the keyboard `vid`:`pid`'s LampArrays on, from `arrays` (the
/// Lights worker's scan): `(usb, ble)`.
pub fn listed_in(
    arrays: &[foxglove_devices::pc_lighting::KeyboardLampArray],
    vid: u16,
    pid: u16,
) -> (bool, bool) {
    let of = |ble: bool| {
        arrays
            .iter()
            .any(|a| (a.vid, a.pid) == (vid, pid) && a.bluetooth == ble)
    };
    (of(false), of(true))
}

/// [`listed_in`] for the host-link keyboard now (`None` without one): what the health rows
/// compare the holds with.
pub fn listed_now() -> Option<(bool, bool)> {
    let (vid, pid) = keyboard()?;
    Some(listed_in(
        &super::pc_lights::snapshot().keyboard_lamp_arrays,
        vid,
        pid,
    ))
}

fn words(h: Option<bool>) -> &'static str {
    match h {
        None => "not held",
        Some(true) => "Foxglove's",
        Some(false) => "held, another controller has it",
    }
}

/// The running holder's queue.
static TX: Mutex<Option<mpsc::Sender<Job>>> = Mutex::new(None);

fn send(job: Job) {
    if let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(job);
    }
}

/// The holder thread with `opener`, its queue kept for [`send`].
fn spawn(opener: Box<dyn HoldOpener>, listed: Listed, notify: Arc<dyn Fn() + Send + Sync>) {
    let (tx, rx) = mpsc::channel();
    let mut core = Core {
        opener,
        keyboard: None,
        held: Vec::new(),
        listed,
        tx: tx.clone(),
        holds: Holds::default(),
        paused: super::pause::paused(),
        notify,
    };
    *TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    super::services::spawn_named("lamp-hold", move || {
        // Blocks until a job; a burst is taken whole.
        while let Ok(job) = rx.recv() {
            let mut jobs = vec![job];
            jobs.extend(rx.try_iter());
            if !core.handle(jobs) {
                break;
            }
        }
    });
}

/// The host-link keyboard's VID/PID, without blocking.
fn keyboard() -> Option<(u16, u16)> {
    let key = super::devices::keyboard_key()?;
    let mut parts = key.split(':');
    let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vid, pid))
}

/// Starts holding the keyboard's LampArrays, with package identity only.
pub fn start() {
    if !super::scene_lamps::identity() || super::repaint_trace::disabled("lamp-hold") {
        return;
    }
    spawn(
        Box::new(super::scene_lamps::WindowsHolds),
        Box::new(|vid, pid| {
            listed_in(&super::pc_lights::snapshot().keyboard_lamp_arrays, vid, pid)
        }),
        Arc::new(|| {
            // Windows may have taken or given back the keyboard: a held scene tries its host
            // link once, and the pages repaint with the owner line.
            super::lights_owner::retry("Foxglove's hold on the keyboard's LampArrays changed");
            super::lights_owner::repaint();
        }),
    );
    watch();
    let events = super::devices::bus().subscribe(false);
    super::services::spawn_named("lamp-hold-events", move || {
        send(Job::Keyboard(keyboard()));
        for e in events {
            if e.event == foxglove_devices::DeviceEvent::Connected {
                send(Job::Keyboard(keyboard()));
            }
        }
    });
}

/// The watch on the LampArrays Windows lists, kept for the app's life (the process's end stops
/// it; after Quit the holder's queue is gone, so later events go nowhere).
fn watch() {
    #[cfg(target_os = "windows")]
    match super::scene_lamps::watch_lamp_arrays(|w| send(job_for(w))) {
        Ok(w) => std::mem::forget(w),
        Err(e) => eprintln!("Foxglove: can't watch the Dynamic Lighting devices: {e}"),
    }
}

/// The holder's job for a watch event.
fn job_for(w: Watched) -> Job {
    match w {
        Watched::Enumerated | Watched::Added(_) => Job::Listed,
        Watched::Removed(id) => Job::Gone(id),
    }
}

/// The Lights worker scanned the Dynamic Lighting devices: the holds are opened afresh.
pub fn devices_changed() {
    send(Job::Devices);
}

/// Foxglove paused or resumed (`pause::set`): paused lets every hold go, resumed holds them
/// again.
pub fn pause_changed(paused: bool) {
    send(Job::Paused(paused));
}

/// Quit: lets every held LampArray go (Windows hands each to the next controller).
pub fn stop() {
    if let Some(tx) = TX.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = tx.send(Job::Stop);
    }
}

/// The transport to draw a LampArray key on: for the host-link keyboard's own LampArrays (its
/// VID/PID), only the one its link uses now (`Some(true)` Bluetooth, `Some(false)` USB), so the
/// other stays held without writing; `None` (every matching device) for any other key, or while
/// the link's transport isn't known.
pub fn draw_filter(
    key: (u16, u16),
    keyboard: Option<(u16, u16)>,
    host_link: Option<&str>,
) -> Option<bool> {
    if keyboard != Some(key) {
        return None;
    }
    match host_link {
        Some("BLE") => Some(true),
        Some("USB") => Some(false),
        _ => None,
    }
}

/// [`draw_filter`] for the app now.
pub fn draw_on(key: (u16, u16)) -> Option<bool> {
    draw_filter(key, keyboard(), super::devices::host_link())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    #[derive(Default)]
    struct Fake {
        available: AtomicBool,
        writes: AtomicU32,
        dropped: AtomicU32,
    }

    struct FakeDevice(Arc<Fake>);

    impl LampDevice for FakeDevice {
        fn available(&self) -> bool {
            self.0.available.load(Ordering::SeqCst)
        }
        fn set(&self, _: &[[u8; 3]]) -> Result<(), String> {
            self.0.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl Drop for FakeDevice {
        fn drop(&mut self) {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The keyboard 1d50:615e over USB and BLE at once (BLE only while `ble_listed`, as when
    /// Windows publishes the Bluetooth LampArray after the first open).
    struct FakeOpener {
        usb: Arc<Fake>,
        ble: Arc<Fake>,
        opened: Arc<AtomicU32>,
        changed: Arc<Mutex<Option<Changed>>>,
        ble_listed: Arc<AtomicBool>,
    }

    impl HoldOpener for FakeOpener {
        fn open(&mut self, vid: u16, pid: u16, changed: Changed) -> Opened {
            if (vid, pid) != (0x1d50, 0x615e) {
                return Opened::default();
            }
            self.opened.fetch_add(1, Ordering::SeqCst);
            *self.changed.lock().unwrap() = Some(changed);
            let mut held = vec![HeldArray {
                id: "usb".into(),
                ble: false,
                device: Box::new(FakeDevice(self.usb.clone())),
            }];
            if self.ble_listed.load(Ordering::SeqCst) {
                held.push(HeldArray {
                    id: "ble".into(),
                    ble: true,
                    device: Box::new(FakeDevice(self.ble.clone())),
                });
            }
            let listed = held.len();
            Opened {
                held,
                listed,
                skipped: 0,
            }
        }
    }

    struct Rig {
        core: Core,
        rx: mpsc::Receiver<Job>,
        usb: Arc<Fake>,
        ble: Arc<Fake>,
        opened: Arc<AtomicU32>,
        changed: Arc<Mutex<Option<Changed>>>,
        told: Arc<AtomicU32>,
        /// Windows lists the Bluetooth LampArray (the opener finds it, the scan names it).
        ble_listed: Arc<AtomicBool>,
    }

    fn rig() -> Rig {
        let (usb, ble) = (Arc::new(Fake::default()), Arc::new(Fake::default()));
        let opened = Arc::new(AtomicU32::new(0));
        let changed = Arc::new(Mutex::new(None));
        let told = Arc::new(AtomicU32::new(0));
        let ble_listed = Arc::new(AtomicBool::new(true));
        let (tx, rx) = mpsc::channel();
        let t = told.clone();
        let l = ble_listed.clone();
        let core = Core {
            opener: Box::new(FakeOpener {
                usb: usb.clone(),
                ble: ble.clone(),
                opened: opened.clone(),
                changed: changed.clone(),
                ble_listed: ble_listed.clone(),
            }),
            keyboard: None,
            held: Vec::new(),
            listed: Box::new(move |vid, pid| {
                let ours = (vid, pid) == CORNE;
                (ours, ours && l.load(Ordering::SeqCst))
            }),
            tx,
            holds: Holds::default(),
            paused: false,
            notify: Arc::new(move || {
                t.fetch_add(1, Ordering::SeqCst);
            }),
        };
        Rig {
            core,
            rx,
            usb,
            ble,
            opened,
            changed,
            told,
            ble_listed,
        }
    }

    impl Rig {
        /// Windows' AvailabilityChanged, as the device's handler sends it.
        fn windows_says(&mut self, usb: bool, ble: bool) {
            self.usb.available.store(usb, Ordering::SeqCst);
            self.ble.available.store(ble, Ordering::SeqCst);
            (self.changed.lock().unwrap().as_ref().unwrap())();
            let jobs: Vec<Job> = self.rx.try_iter().collect();
            self.core.handle(jobs);
        }
    }

    const CORNE: (u16, u16) = (0x1d50, 0x615e);

    #[test]
    fn both_transports_are_held_and_never_written() {
        let mut r = rig();
        r.usb.available.store(true, Ordering::SeqCst);
        r.ble.available.store(true, Ordering::SeqCst);
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        assert_eq!(
            r.core.holds,
            Holds {
                usb: Some(true),
                ble: Some(true)
            }
        );
        assert!(r.core.holds.ours(Owner::LampArrayUsb) && r.core.holds.ours(Owner::LampArrayBle));
        assert!(!r.core.holds.ours(Owner::HostLink));
        // The same keyboard again (a reconnect's event) doesn't reopen them.
        r.core
            .handle(vec![Job::Keyboard(Some(CORNE)), Job::Changed]);
        assert_eq!(r.opened.load(Ordering::SeqCst), 1);
        assert_eq!(
            r.usb.writes.load(Ordering::SeqCst) + r.ble.writes.load(Ordering::SeqCst),
            0
        );
    }

    /// Windows ranks another controller above Foxglove for the BLE device: that one isn't ours,
    /// and each change is passed on once.
    #[test]
    fn availability_follows_windows_per_transport() {
        let mut r = rig();
        r.usb.available.store(true, Ordering::SeqCst);
        r.ble.available.store(true, Ordering::SeqCst);
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        let told = r.told.load(Ordering::SeqCst);
        r.windows_says(true, false);
        assert_eq!(r.core.holds.ble, Some(false));
        assert!(
            !r.core.holds.ours(Owner::LampArrayBle),
            "another controller has BLE"
        );
        assert!(r.core.holds.ours(Owner::LampArrayUsb));
        assert_eq!(r.told.load(Ordering::SeqCst), told + 1);
        r.windows_says(true, false);
        assert_eq!(
            r.told.load(Ordering::SeqCst),
            told + 1,
            "no change, no news"
        );
        r.windows_says(true, true);
        assert!(r.core.holds.ours(Owner::LampArrayBle));
    }

    #[test]
    fn a_device_change_reopens_and_stop_lets_go() {
        let mut r = rig();
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        r.core.handle(vec![Job::Devices]);
        assert_eq!(r.opened.load(Ordering::SeqCst), 2);
        assert_eq!(
            r.usb.dropped.load(Ordering::SeqCst),
            1,
            "the old one let go"
        );
        // Another keyboard (no LampArray): nothing held.
        r.core.handle(vec![Job::Keyboard(Some((0xfeed, 0x0001)))]);
        assert_eq!(r.core.holds, Holds::default());
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        assert!(!r.core.handle(vec![Job::Stop]));
        assert_eq!(r.usb.dropped.load(Ordering::SeqCst), 3);
        assert_eq!(r.ble.dropped.load(Ordering::SeqCst), 3);
        assert_eq!(r.core.holds, Holds::default());
    }

    /// The Hub's pick (11/07): pause is hands off, so the holds go as a scene's do, and come back
    /// on resume; while paused a reconnect or device change opens nothing.
    #[test]
    fn pause_lets_the_holds_go_and_resume_takes_them_again() {
        let mut r = rig();
        r.usb.available.store(true, Ordering::SeqCst);
        r.ble.available.store(true, Ordering::SeqCst);
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        let told = r.told.load(Ordering::SeqCst);
        r.core.handle(vec![Job::Paused(true)]);
        assert_eq!(r.core.holds, Holds::default(), "paused: nothing held");
        assert_eq!(r.usb.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(r.ble.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(
            r.told.load(Ordering::SeqCst),
            told + 1,
            "the owner line hears"
        );
        r.core.handle(vec![
            Job::Devices,
            Job::Keyboard(Some(CORNE)),
            Job::Paused(true),
        ]);
        assert_eq!(
            r.opened.load(Ordering::SeqCst),
            1,
            "nothing opened while paused"
        );
        r.core.handle(vec![Job::Paused(false)]);
        assert_eq!(r.opened.load(Ordering::SeqCst), 2);
        assert!(r.core.holds.ours(Owner::LampArrayUsb) && r.core.holds.ours(Owner::LampArrayBle));
        assert_eq!(
            r.usb.writes.load(Ordering::SeqCst) + r.ble.writes.load(Ordering::SeqCst),
            0
        );
    }

    /// The diagnosis' case (11/07): the first open finds only USB (Windows publishes the
    /// Bluetooth LampArray later); the watch's arrival opens BLE and keeps the USB hold as it is.
    #[test]
    fn a_lamp_array_that_appears_later_is_held_too() {
        let mut r = rig();
        r.usb.available.store(true, Ordering::SeqCst);
        r.ble.available.store(true, Ordering::SeqCst);
        r.ble_listed.store(false, Ordering::SeqCst);
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        assert_eq!(r.core.holds.ble, None);
        r.ble_listed.store(true, Ordering::SeqCst);
        r.core.handle(vec![job_for(Watched::Added("ble".into()))]);
        assert_eq!(r.core.holds.ble, Some(true));
        assert_eq!(r.core.holds.usb, Some(true));
        assert_eq!(
            r.usb.dropped.load(Ordering::SeqCst),
            1,
            "only the second USB handle closed: the first stays held"
        );
        assert_eq!(r.core.held.len(), 2);
        // It goes: let go of it alone, then hold it again when it comes back.
        r.ble_listed.store(false, Ordering::SeqCst);
        r.core.handle(vec![job_for(Watched::Removed("ble".into()))]);
        assert_eq!(r.core.holds.ble, None);
        assert_eq!(r.core.holds.usb, Some(true));
        assert_eq!(r.ble.dropped.load(Ordering::SeqCst), 1);
        assert_eq!(
            r.usb.writes.load(Ordering::SeqCst) + r.ble.writes.load(Ordering::SeqCst),
            0
        );
    }

    /// A connect of the same keyboard reopens only when Windows lists a transport not held.
    #[test]
    fn the_same_keyboard_again_opens_a_listed_transport_not_held() {
        let mut r = rig();
        r.ble_listed.store(false, Ordering::SeqCst);
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        assert_eq!(r.opened.load(Ordering::SeqCst), 1);
        // Nothing new listed: no open.
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        assert_eq!(r.opened.load(Ordering::SeqCst), 1);
        assert_eq!(r.core.missing(), (false, false));
        // The scan now lists BLE: the same keyboard's event opens it.
        r.ble_listed.store(true, Ordering::SeqCst);
        assert_eq!(r.core.missing(), (false, true));
        r.core.handle(vec![Job::Keyboard(Some(CORNE))]);
        assert_eq!(r.opened.load(Ordering::SeqCst), 2);
        assert!(r.core.holds.ble.is_some());
        assert_eq!(r.core.missing(), (false, false));
        // Paused: nothing is missing, nothing opens.
        r.core.handle(vec![Job::Paused(true)]);
        assert_eq!(r.core.missing(), (false, false));
    }

    #[test]
    fn the_log_words_say_held_missed_or_not_listed() {
        assert_eq!(claim_words(true, true), "held");
        assert!(claim_words(false, true).starts_with("MISSED"));
        assert_eq!(claim_words(false, false), "not listed");
        let arr = |key: &str, ble| foxglove_devices::pc_lighting::KeyboardLampArray {
            key: key.into(),
            name: "Corne".into(),
            vid: 0x1d50,
            pid: 0x615e,
            bluetooth: ble,
        };
        assert_eq!(
            listed_in(&[arr("1d50:615e", false)], 0x1d50, 0x615e),
            (true, false)
        );
        assert_eq!(
            listed_in(&[arr("a", false), arr("b", true)], 0x1d50, 0x615e),
            (true, true)
        );
        assert_eq!(listed_in(&[arr("b", true)], 0xfeed, 1), (false, false));
    }

    #[test]
    fn the_keyboard_is_drawn_only_on_its_links_transport() {
        assert_eq!(draw_filter(CORNE, Some(CORNE), Some("USB")), Some(false));
        assert_eq!(draw_filter(CORNE, Some(CORNE), Some("BLE")), Some(true));
        assert_eq!(
            draw_filter(CORNE, Some(CORNE), None),
            None,
            "unknown: every one"
        );
        assert_eq!(
            draw_filter((0x046d, 0xc08b), Some(CORNE), Some("USB")),
            None,
            "a mouse: every matching device"
        );
        assert_eq!(draw_filter(CORNE, None, Some("USB")), None);
    }
}
