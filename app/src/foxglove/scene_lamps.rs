//! A running scene's frames to Windows Dynamic Lighting devices (#503): every LampArray Windows
//! lists, whatever its vendor or kind, through `Windows.Devices.Lights`.
//!
//! Windows decides who may drive them. An app without package identity (Foxglove until #179)
//! gets them only while its window is in front; another app, or the order in Settings,
//! Personalization, Dynamic Lighting, can take them at any time. Windows says so with
//! `LampArray.AvailabilityChanged`: on that event the writer reads `IsAvailable` again, stops
//! writing while it is false and puts the scene's newest frame back when it is true. Nothing
//! polls. The Scenes page says why a device stopped ([`waiting`], [`note`]).
//!
//! One writer per scene, on its own thread: it opens each device once (on its first frame, and
//! again only after the Lights worker's devices changed), writes only what changed, keeps only
//! the newest frame per device and, at the scene's end or on pause and away, lets go of every
//! device (Windows' own lighting comes back). Frames are paced by `scene_output` (at most
//! [`super::scene_output::LAMP_INTERVAL`]).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One Dynamic Lighting device, as the writer drives it (Windows', or a fake in tests).
pub trait LampDevice: Send {
    /// Whether Windows lets Foxglove drive it now.
    fn available(&self) -> bool;
    /// One colour per lamp in index order, or one colour for every lamp.
    fn set(&self, colours: &[[u8; 3]]) -> Result<(), String>;
}

/// Calls back (from any thread) when Windows changes who may drive a device.
pub type Changed = Arc<dyn Fn() + Send + Sync>;

/// Opens the devices behind a Lights page key. Dropping what it returns lets them go.
pub trait Opener: Send {
    fn open(&mut self, key: &str, changed: Changed) -> Vec<Box<dyn LampDevice>>;
}

/// A device's state for the Scenes page and diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LampStatus {
    /// Keys Windows doesn't let Foxglove drive right now.
    pub waiting: BTreeSet<String>,
    /// Keys showing the scene.
    pub showing: BTreeSet<String>,
    /// Frames written, all devices together.
    pub frames: u64,
}

pub type Status = Arc<Mutex<LampStatus>>;

enum Job {
    Frame(String, Vec<[u8; 3]>),
    /// Windows changed who may drive this key's devices.
    Changed(String),
    /// The devices found changed: let every device go and open them afresh on their next
    /// frame (one unplugged and back is a new device to Windows).
    Forget,
    /// Let every device go (pause, away); the next frame opens them again.
    Release,
    Close,
}

struct Held {
    devices: Vec<Box<dyn LampDevice>>,
    available: bool,
    latest: Option<Vec<[u8; 3]>>,
    shown: Option<Vec<[u8; 3]>>,
    /// A write failed and was logged: later failures aren't, until one succeeds.
    failing: bool,
}

/// The writer's state, driven one batch of jobs at a time (synchronously in tests).
struct Core {
    opener: Box<dyn Opener>,
    /// `None`: looked for and not found (not looked for again until [`Job::Forget`]).
    held: BTreeMap<String, Option<Held>>,
    tx: mpsc::Sender<Job>,
    status: Status,
}

impl Core {
    /// Handles a batch; false once closed.
    fn handle(&mut self, jobs: Vec<Job>) -> bool {
        let mut touched = BTreeSet::new();
        for job in jobs {
            match job {
                Job::Frame(key, colours) => {
                    if !self.held.contains_key(&key) {
                        let opened = self.open(&key);
                        self.held.insert(key.clone(), opened);
                    }
                    if let Some(Some(h)) = self.held.get_mut(&key) {
                        h.latest = Some(colours);
                        touched.insert(key);
                    }
                }
                Job::Changed(key) => {
                    if let Some(Some(h)) = self.held.get_mut(&key) {
                        let now = h.devices.iter().any(|d| d.available());
                        if now && !h.available {
                            // Someone else drew on it meanwhile: the newest frame goes again.
                            h.shown = None;
                        }
                        if now != h.available {
                            eprintln!(
                                "Foxglove: scene: Dynamic Lighting device {key} {}",
                                if now {
                                    "is Foxglove's again"
                                } else {
                                    "taken by Windows"
                                }
                            );
                        }
                        h.available = now;
                        touched.insert(key);
                    }
                }
                Job::Forget => self.held.clear(),
                Job::Release => {
                    self.held.clear();
                    let mut s = self.status();
                    s.waiting.clear();
                    s.showing.clear();
                }
                Job::Close => {
                    self.held.clear();
                    let mut s = self.status();
                    s.waiting.clear();
                    s.showing.clear();
                    return false;
                }
            }
        }
        for key in touched {
            self.draw(&key);
        }
        true
    }

    fn status(&self) -> std::sync::MutexGuard<'_, LampStatus> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn open(&mut self, key: &str) -> Option<Held> {
        let tx = self.tx.clone();
        let k = key.to_string();
        let changed: Changed = Arc::new(move || {
            let _ = tx.send(Job::Changed(k.clone()));
        });
        let devices = self.opener.open(key, changed);
        if devices.is_empty() {
            eprintln!("Foxglove: scene: no Dynamic Lighting device for {key}");
            return None;
        }
        let available = devices.iter().any(|d| d.available());
        Some(Held {
            devices,
            available,
            latest: None,
            shown: None,
            failing: false,
        })
    }

    fn draw(&mut self, key: &str) {
        let Some(Some(h)) = self.held.get_mut(key) else {
            return;
        };
        let mut wrote = false;
        if h.available && h.latest.is_some() && h.latest != h.shown {
            let colours = h.latest.clone().unwrap_or_default();
            // Every device is written, even after one fails.
            let mut failed = None;
            for d in &h.devices {
                if let Err(e) = d.set(&colours) {
                    failed = Some(e);
                }
            }
            match failed {
                Some(e) if !h.failing => {
                    eprintln!("Foxglove: scene: Dynamic Lighting device {key}: {e}");
                    h.failing = true;
                }
                Some(_) => {}
                None => h.failing = false,
            }
            h.shown = Some(colours);
            wrote = true;
        }
        let available = h.available;
        let mut s = self.status();
        if wrote {
            s.frames += 1;
        }
        if available {
            s.waiting.remove(key);
            s.showing.insert(key.to_string());
        } else {
            s.showing.remove(key);
            s.waiting.insert(key.to_string());
        }
    }
}

/// Posts frames to the writer; cheap to clone (the scheduler's thread keeps one).
#[derive(Clone)]
pub struct Poster(mpsc::Sender<Job>);

impl Poster {
    pub fn post(&self, key: String, colours: Vec<[u8; 3]>) {
        let _ = self.0.send(Job::Frame(key, colours));
    }
}

/// One scene's writer. Dropping it lets every device go and joins its thread.
pub struct LampWriter {
    tx: mpsc::Sender<Job>,
    thread: Option<JoinHandle<()>>,
}

impl LampWriter {
    pub fn spawn(opener: Box<dyn Opener>, status: Status) -> Self {
        *status.lock().unwrap_or_else(|e| e.into_inner()) = LampStatus::default();
        let (tx, rx) = mpsc::channel();
        let mut core = Core {
            opener,
            held: BTreeMap::new(),
            tx: tx.clone(),
            status,
        };
        let thread = super::services::spawn_named("scene-lamps", move || {
            // Blocks until a job; a burst is taken whole, so only each device's newest frame
            // is written.
            while let Ok(job) = rx.recv() {
                let mut jobs = vec![job];
                jobs.extend(rx.try_iter());
                if !core.handle(jobs) {
                    break;
                }
            }
        });
        LampWriter {
            tx,
            thread: Some(thread),
        }
    }

    pub fn poster(&self) -> Poster {
        Poster(self.tx.clone())
    }

    /// The Lights worker's devices changed: devices not found before are looked for again.
    pub fn devices_changed(&self) {
        let _ = self.tx.send(Job::Forget);
    }

    /// Lets every device go (pause, away). The next frame takes them again.
    pub fn release(&self) {
        let _ = self.tx.send(Job::Release);
    }
}

impl Drop for LampWriter {
    fn drop(&mut self) {
        let _ = self.tx.send(Job::Close);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Whether Foxglove runs with package identity (Windows' background light control), read once:
/// it only changes with a restart.
pub fn identity() -> bool {
    static IDENTITY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IDENTITY.get_or_init(super::identity::has_identity)
}

/// What the Scenes page says about Dynamic Lighting devices: `waiting` are the names of the
/// ones Windows took back, `any` says the scene has any, `identity` whether Foxglove runs with
/// package identity (background control).
pub fn note(waiting: &[String], any: bool, identity: bool) -> Option<String> {
    if !waiting.is_empty() {
        let names = waiting.join(", ");
        return Some(if identity {
            format!(
                "{names}: another app has these lights now. Settings, Personalization, Dynamic Lighting sets which app comes first."
            )
        } else {
            format!(
                "{names}: paused while Foxglove's window isn't in front. Windows lets Foxglove drive Dynamic Lighting devices only from the front until it can run with package identity."
            )
        });
    }
    (any && !identity).then(|| {
        "Dynamic Lighting devices follow the scene while Foxglove's window is in front; Windows takes them back in the background.".to_string()
    })
}

/// Windows' devices: the LampArrays whose vendor and product match the key. Windows doesn't
/// give a LampArray's serial, so two of one model are shared out: a device another key holds
/// is skipped, and a key with a serial takes one device only.
#[cfg(target_os = "windows")]
#[derive(Default)]
pub struct WindowsLamps {
    /// Windows' device id -> the key that opened it.
    claimed: BTreeMap<String, String>,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{Changed, LampDevice, Opener};
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Lights::LampArray;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Color;

    struct Device {
        array: LampArray,
        count: i32,
        token: Option<i64>,
    }

    impl LampDevice for Device {
        fn available(&self) -> bool {
            // Windows before 11 22H2 has no IsAvailable: there, an app in front always may.
            self.array.IsAvailable().unwrap_or(true)
        }

        fn set(&self, colours: &[[u8; 3]]) -> Result<(), String> {
            let c = |[r, g, b]: [u8; 3]| Color {
                A: 255,
                R: r,
                G: g,
                B: b,
            };
            let result = match colours {
                [] => Ok(()),
                [one, ..] if colours.len() == 1 || self.count <= 0 => self.array.SetColor(c(*one)),
                many => {
                    let n = many.len().min(self.count.max(0) as usize);
                    let colours: Vec<Color> = many[..n].iter().map(|&v| c(v)).collect();
                    let indices: Vec<i32> = (0..n as i32).collect();
                    self.array.SetColorsForIndices(&colours, &indices)
                }
            };
            result.map_err(|e| e.to_string())
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            if let Some(t) = self.token {
                let _ = self.array.RemoveAvailabilityChanged(t);
            }
        }
    }

    fn vid_pid(key: &str) -> Option<(u16, u16)> {
        let mut parts = key.split(':');
        let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
        let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
        Some((vid, pid))
    }

    impl Opener for super::WindowsLamps {
        fn open(&mut self, key: &str, changed: Changed) -> Vec<Box<dyn LampDevice>> {
            let Some((vid, pid)) = vid_pid(key) else {
                return Vec::new();
            };
            // Opened afresh: what this key held before is let go.
            self.claimed.retain(|_, k| k != key);
            let one_only = key.split(':').count() > 2;
            let claimed = &mut self.claimed;
            let mut read = || -> windows::core::Result<Vec<Box<dyn LampDevice>>> {
                let selector = LampArray::GetDeviceSelector()?;
                let list = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()?;
                let mut out: Vec<Box<dyn LampDevice>> = Vec::new();
                for info in &list {
                    if one_only && !out.is_empty() {
                        break;
                    }
                    let id = info.Id()?;
                    if claimed.contains_key(&id.to_string()) {
                        continue;
                    }
                    let Ok(array) = LampArray::FromIdAsync(&id).and_then(|op| op.join()) else {
                        continue;
                    };
                    if array.HardwareVendorId()? != vid || array.HardwareProductId()? != pid {
                        continue;
                    }
                    claimed.insert(id.to_string(), key.to_string());
                    let tell = changed.clone();
                    let token = array
                        .AvailabilityChanged(&TypedEventHandler::new(move |_, _| {
                            tell();
                            Ok(())
                        }))
                        .ok();
                    out.push(Box::new(Device {
                        count: array.LampCount().unwrap_or(0),
                        array,
                        token,
                    }));
                }
                Ok(out)
            };
            read().unwrap_or_else(|e| {
                eprintln!("Foxglove: scene: opening Dynamic Lighting device {key}: {e}");
                Vec::new()
            })
        }
    }
}

/// Elsewhere there are no Dynamic Lighting devices.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct WindowsLamps;

#[cfg(not(target_os = "windows"))]
impl Opener for WindowsLamps {
    fn open(&mut self, _: &str, _: Changed) -> Vec<Box<dyn LampDevice>> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// A fake LampArray: any vendor, any kind. Shares its state with the test.
    #[derive(Default)]
    struct Fake {
        available: AtomicBool,
        writes: Mutex<Vec<Vec<[u8; 3]>>>,
        opened: AtomicU32,
        dropped: AtomicU32,
        changed: Mutex<Option<Changed>>,
    }

    struct FakeDevice(Arc<Fake>);

    impl LampDevice for FakeDevice {
        fn available(&self) -> bool {
            self.0.available.load(Ordering::SeqCst)
        }
        fn set(&self, colours: &[[u8; 3]]) -> Result<(), String> {
            self.0.writes.lock().unwrap().push(colours.to_vec());
            Ok(())
        }
    }

    impl Drop for FakeDevice {
        fn drop(&mut self) {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Opens the fake for `key` only (a headset-kind strip from no particular vendor).
    struct FakeOpener {
        key: &'static str,
        fake: Arc<Fake>,
    }

    impl Opener for FakeOpener {
        fn open(&mut self, key: &str, changed: Changed) -> Vec<Box<dyn LampDevice>> {
            if key != self.key {
                return Vec::new();
            }
            self.fake.opened.fetch_add(1, Ordering::SeqCst);
            *self.fake.changed.lock().unwrap() = Some(changed);
            vec![Box::new(FakeDevice(self.fake.clone()))]
        }
    }

    const KEY: &str = "1234:5678";

    fn core(fake: &Arc<Fake>) -> (Core, mpsc::Receiver<Job>) {
        let (tx, rx) = mpsc::channel();
        let core = Core {
            opener: Box::new(FakeOpener {
                key: KEY,
                fake: fake.clone(),
            }),
            held: BTreeMap::new(),
            tx,
            status: Status::default(),
        };
        (core, rx)
    }

    fn frame(c: u8) -> Job {
        Job::Frame(KEY.into(), vec![[c, 0, 0]; 8])
    }

    /// Windows' event, as the device's handler sends it, handled by the writer.
    fn windows_says(core: &mut Core, rx: &mpsc::Receiver<Job>, fake: &Fake, available: bool) {
        fake.available.store(available, Ordering::SeqCst);
        (fake.changed.lock().unwrap().as_ref().unwrap())();
        let jobs: Vec<Job> = rx.try_iter().collect();
        assert_eq!(jobs.len(), 1, "one job per event");
        core.handle(jobs);
    }

    #[test]
    fn availability_stops_and_resumes_the_scene() {
        let fake = Arc::new(Fake::default());
        fake.available.store(true, Ordering::SeqCst);
        let (mut core, rx) = core(&fake);
        core.handle(vec![frame(10)]);
        assert_eq!(fake.writes.lock().unwrap().len(), 1);
        assert_eq!(fake.writes.lock().unwrap()[0].len(), 8, "a colour per lamp");
        // Windows gives the lights to another app (Foxglove went to the background).
        windows_says(&mut core, &rx, &fake, false);
        core.handle(vec![frame(20)]);
        core.handle(vec![frame(30)]);
        assert_eq!(fake.writes.lock().unwrap().len(), 1, "nothing while taken");
        assert!(core.status().waiting.contains(KEY));
        // Back in front: the newest frame goes at once, without waiting for the next one.
        windows_says(&mut core, &rx, &fake, true);
        let writes = fake.writes.lock().unwrap().clone();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[1][0], [30, 0, 0]);
        assert!(core.status().showing.contains(KEY));
        assert!(core.status().waiting.is_empty());
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1, "opened once");
    }

    #[test]
    fn only_changes_and_only_the_newest_frame_are_written() {
        let fake = Arc::new(Fake::default());
        fake.available.store(true, Ordering::SeqCst);
        let (mut core, _rx) = core(&fake);
        // A burst: one write, of the newest.
        core.handle(vec![frame(1), frame(2), frame(3)]);
        core.handle(vec![frame(3)]);
        let writes = fake.writes.lock().unwrap().clone();
        assert_eq!(writes, vec![vec![[3, 0, 0]; 8]]);
        assert_eq!(core.status().frames, 1);
    }

    #[test]
    fn release_and_close_let_the_device_go() {
        let fake = Arc::new(Fake::default());
        fake.available.store(true, Ordering::SeqCst);
        let (mut core, _rx) = core(&fake);
        core.handle(vec![frame(1)]);
        core.handle(vec![Job::Release]);
        assert_eq!(fake.dropped.load(Ordering::SeqCst), 1, "paused: let go");
        assert!(core.status().showing.is_empty());
        // The next frame takes it again and draws even an unchanged frame.
        core.handle(vec![frame(1)]);
        assert_eq!(fake.opened.load(Ordering::SeqCst), 2);
        assert_eq!(fake.writes.lock().unwrap().len(), 2);
        assert!(!core.handle(vec![Job::Close]));
        assert_eq!(fake.dropped.load(Ordering::SeqCst), 2, "the end lets go");
    }

    #[test]
    fn devices_are_looked_for_again_only_after_a_change() {
        let fake = Arc::new(Fake::default());
        fake.available.store(true, Ordering::SeqCst);
        let (mut core, _rx) = core(&fake);
        let other = |c| Job::Frame("abcd:0001".into(), vec![[c, c, c]]);
        core.handle(vec![other(1), frame(1)]);
        core.handle(vec![other(2), frame(2)]);
        assert!(core.held.get("abcd:0001").is_some_and(Option::is_none));
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
        // Unplugged and back: a new device to Windows, so every key is opened afresh.
        core.handle(vec![Job::Forget]);
        assert!(core.held.is_empty());
        assert_eq!(fake.dropped.load(Ordering::SeqCst), 1, "the old one let go");
        core.handle(vec![frame(2)]);
        assert_eq!(fake.opened.load(Ordering::SeqCst), 2);
        assert_eq!(fake.writes.lock().unwrap().len(), 3, "drawn in full again");
    }

    #[test]
    fn the_writer_thread_ends_and_lets_go_on_drop() {
        let fake = Arc::new(Fake::default());
        fake.available.store(true, Ordering::SeqCst);
        let status = Status::default();
        let writer = LampWriter::spawn(
            Box::new(FakeOpener {
                key: KEY,
                fake: fake.clone(),
            }),
            status.clone(),
        );
        writer.poster().post(KEY.into(), vec![[9, 9, 9]]);
        drop(writer);
        // Whether the frame went before the close is the thread's timing; either way the device
        // it opened is let go.
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
        assert_eq!(fake.dropped.load(Ordering::SeqCst), 1);
        assert!(status.lock().unwrap().showing.is_empty());
    }

    #[test]
    fn the_page_says_why_a_device_stopped() {
        let names = vec!["Desk strip".to_string()];
        let bg = note(&names, true, false).unwrap();
        assert!(bg.contains("Desk strip") && bg.contains("in front"), "{bg}");
        let other = note(&names, true, true).unwrap();
        assert!(other.contains("Dynamic Lighting sets which app"), "{other}");
        assert!(note(&[], true, false).unwrap().contains("in front"));
        assert_eq!(note(&[], true, true), None);
        assert_eq!(note(&[], false, false), None);
    }
}
