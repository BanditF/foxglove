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
//! [`super::scene_output::lamp_interval`]'s rate, from the rates file, #486).
//!
//! The keyboard's own LampArray is one of them while Windows Dynamic Lighting holds the
//! keyboard's lights (#749 b): the scene output sends it frames under its key like any other,
//! and each change of who may drive a device is passed on ([`LampWriter::spawn`]'s `notify`), so
//! a held keyboard tries its host link once on it (`lights_owner::retry`).
//!
//! With package identity (#179) the keyboard's own LampArrays are written only on the transport
//! its host link uses now (`lamp_hold::draw_on`); `lamp_hold` holds both meanwhile, unwritten.

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
    /// Told the key each time Windows changes whether Foxglove may drive its devices.
    notify: Option<Notify>,
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
                        let flipped = h.available != now;
                        h.available = now;
                        if flipped {
                            if let Some(n) = &self.notify {
                                n(&key);
                            }
                        }
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
    /// `notify` gets a device's key each time Windows changes whether Foxglove may drive it.
    pub fn spawn(opener: Box<dyn Opener>, status: Status, notify: Option<Notify>) -> Self {
        *status.lock().unwrap_or_else(|e| e.into_inner()) = LampStatus::default();
        let (tx, rx) = mpsc::channel();
        let mut core = Core {
            opener,
            held: BTreeMap::new(),
            tx: tx.clone(),
            status,
            notify,
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
    use std::collections::BTreeMap;
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

    /// Every LampArray Windows lists with this vendor and product, with its device id, skipping
    /// the ids `skip` says.
    fn matching(
        vid: u16,
        pid: u16,
        skip: &dyn Fn(&str) -> bool,
        one_only: bool,
    ) -> windows::core::Result<Vec<(String, LampArray)>> {
        matching_counted(vid, pid, skip, one_only).map(|c| c.found)
    }

    /// What [`matching_counted`] found.
    struct Counted {
        /// The matching arrays (device id, array).
        found: Vec<(String, LampArray)>,
        /// How many LampArrays Windows lists.
        listed: usize,
        /// How many of them report another vendor and product.
        skipped: usize,
        /// Every listed one not taken, with why.
        not_held: Vec<super::super::lamp_hold::NotHeld>,
    }

    /// [`matching`], with how many LampArrays Windows lists, how many of them another vendor
    /// and product skipped, and why each one not taken wasn't (the hold's log lines, #179). One
    /// array Windows won't open or describe is a reason on that array, never the end of the
    /// list.
    fn matching_counted(
        vid: u16,
        pid: u16,
        skip: &dyn Fn(&str) -> bool,
        one_only: bool,
    ) -> windows::core::Result<Counted> {
        use super::super::lamp_hold::NotHeld;
        let selector = LampArray::GetDeviceSelector()?;
        let list = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()?;
        let mut c = Counted {
            found: Vec::new(),
            listed: list.Size()? as usize,
            skipped: 0,
            not_held: Vec::new(),
        };
        for (n, info) in (&list).into_iter().enumerate() {
            let Ok(id) = info.Id().map(|i| i.to_string()) else {
                c.not_held.push(NotHeld {
                    id: format!("#{n}"),
                    reason: "Windows gave no device id".into(),
                });
                continue;
            };
            let mut not = |reason: String| {
                c.not_held.push(NotHeld {
                    id: id.clone(),
                    reason,
                })
            };
            if one_only && !c.found.is_empty() {
                not("one device per key, and one is taken".into());
                continue;
            }
            if skip(&id) {
                not("another light or transport has it".into());
                continue;
            }
            let hid = windows::core::HSTRING::from(id.as_str());
            let array = match LampArray::FromIdAsync(&hid).and_then(|op| op.join()) {
                Ok(a) => a,
                Err(e) => {
                    not(format!(
                        "Windows didn't open it (LampArray.FromIdAsync: {e})"
                    ));
                    continue;
                }
            };
            let ids = array
                .HardwareVendorId()
                .and_then(|v| array.HardwareProductId().map(|p| (v, p)));
            match ids {
                Ok(ids) if ids == (vid, pid) => c.found.push((id, array)),
                Ok((v, p)) => {
                    c.skipped += 1;
                    not(format!("another device ({v:04x}:{p:04x})"));
                }
                Err(e) => not(format!("its vendor and product can't be read: {e}")),
            }
        }
        Ok(c)
    }

    /// The LampArrays behind a Lights page key: those whose vendor and product match, not
    /// claimed by another key; one only for a key with a serial. What `key` claimed before is
    /// let go first (it is opened afresh). Shared by scenes and the Lights worker (#691). The
    /// host-link keyboard's own are only those on its link's transport (`lamp_hold::draw_on`).
    pub(super) fn arrays_for(
        key: &str,
        claimed: &mut BTreeMap<String, String>,
    ) -> windows::core::Result<Vec<LampArray>> {
        let Some((vid, pid)) = vid_pid(key) else {
            return Ok(Vec::new());
        };
        claimed.retain(|_, k| k != key);
        let one_only = key.split(':').count() > 2;
        let transport = super::super::lamp_hold::draw_on((vid, pid));
        let skip = |id: &str| {
            claimed.contains_key(id)
                || transport
                    .is_some_and(|ble| foxglove_devices::pc_lighting::bluetooth_path(id) != ble)
        };
        let found = matching(vid, pid, &skip, one_only)?;
        let mut out = Vec::new();
        for (id, array) in found {
            claimed.insert(id, key.to_string());
            out.push(array);
        }
        Ok(out)
    }

    /// Holds the host-link keyboard's LampArrays, both transports, without writing (#179).
    impl super::super::lamp_hold::HoldOpener for super::WindowsHolds {
        fn open(
            &mut self,
            vid: u16,
            pid: u16,
            changed: Changed,
        ) -> super::super::lamp_hold::Opened {
            let Counted {
                found,
                listed,
                skipped,
                not_held,
            } = matching_counted(vid, pid, &|_| false, false).unwrap_or_else(|e| {
                eprintln!("Foxglove: holding the keyboard's Dynamic Lighting devices: {e}");
                Counted {
                    found: Vec::new(),
                    listed: 0,
                    skipped: 0,
                    not_held: Vec::new(),
                }
            });
            let held = found
                .into_iter()
                .map(|(id, array)| {
                    let tell = changed.clone();
                    let token = array
                        .AvailabilityChanged(&TypedEventHandler::new(move |_, _| {
                            tell();
                            Ok(())
                        }))
                        .ok();
                    let ble = foxglove_devices::pc_lighting::bluetooth_path(&id);
                    let device = Device {
                        count: array.LampCount().unwrap_or(0),
                        array,
                        token,
                    };
                    super::super::lamp_hold::HeldArray {
                        id,
                        ble,
                        device: Box::new(device),
                    }
                })
                .collect();
            super::super::lamp_hold::Opened {
                held,
                listed,
                skipped,
                not_held,
            }
        }
    }

    /// Keeps the LampArray watch running; stops it when dropped.
    pub struct LampWatch(windows::Devices::Enumeration::DeviceWatcher);

    impl Drop for LampWatch {
        fn drop(&mut self) {
            let _ = self.0.Stop();
        }
    }

    /// Watches the LampArrays Windows lists (`LampArray::GetDeviceSelector`), as `peripherals`
    /// watches Bluetooth: after the first enumeration (one [`Watched::Enumerated`]), each arrival
    /// and removal. Windows' events, no polling.
    pub fn watch_lamp_arrays(
        tell: impl Fn(super::super::lamp_hold::Watched) + Send + Sync + 'static,
    ) -> windows::core::Result<LampWatch> {
        use super::super::lamp_hold::Watched;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use windows::Devices::Enumeration::{DeviceInformationUpdate, DeviceWatcher};
        let selector = LampArray::GetDeviceSelector()?;
        let watcher = DeviceInformation::CreateWatcherAqsFilter(&selector)?;
        let tell = Arc::new(tell);
        // The first enumeration's Added events are the arrays already there: one Enumerated
        // stands for them all.
        let enumerated = Arc::new(AtomicBool::new(false));
        let (t, done) = (tell.clone(), enumerated.clone());
        watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
            move |_, info| {
                if let (true, Some(info)) = (done.load(Ordering::Acquire), info.as_ref()) {
                    t(Watched::Added(info.Id()?.to_string()));
                }
                Ok(())
            },
        ))?;
        // Windows sends arrivals after the first enumeration only to a watcher that also
        // handles Updated.
        watcher.Updated(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(|_, _| Ok(())),
        )?;
        let t = tell.clone();
        watcher.Removed(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, info| {
                if let Some(info) = info.as_ref() {
                    t(Watched::Removed(info.Id()?.to_string()));
                }
                Ok(())
            }),
        )?;
        let t = tell;
        watcher.EnumerationCompleted(&TypedEventHandler::<
            DeviceWatcher,
            windows::core::IInspectable,
        >::new(move |_, _| {
            enumerated.store(true, Ordering::Release);
            t(Watched::Enumerated);
            Ok(())
        }))?;
        watcher.Start()?;
        Ok(LampWatch(watcher))
    }

    impl Opener for super::WindowsLamps {
        fn open(&mut self, key: &str, changed: Changed) -> Vec<Box<dyn LampDevice>> {
            let arrays = arrays_for(key, &mut self.claimed).unwrap_or_else(|e| {
                eprintln!("Foxglove: scene: opening Dynamic Lighting device {key}: {e}");
                Vec::new()
            });
            arrays
                .into_iter()
                .map(|array| {
                    let tell = changed.clone();
                    let token = array
                        .AvailabilityChanged(&TypedEventHandler::new(move |_, _| {
                            tell();
                            Ok(())
                        }))
                        .ok();
                    Box::new(Device {
                        count: array.LampCount().unwrap_or(0),
                        array,
                        token,
                    }) as Box<dyn LampDevice>
                })
                .collect()
        }
    }

    /// The Lights worker's handle on one key's LampArrays (#691): every matching device gets
    /// the same colours. Windows shows them while Foxglove may drive the device (in front, or
    /// as the background controller) and keeps them for when it may.
    pub struct Handle {
        arrays: Vec<LampArray>,
        /// Each array's `AvailabilityChanged` registration, removed on drop.
        tokens: Vec<Option<i64>>,
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            for (a, t) in self.arrays.iter().zip(&self.tokens) {
                if let Some(t) = t {
                    let _ = a.RemoveAvailabilityChanged(*t);
                }
            }
        }
    }

    fn color(c: foxglove_devices::Rgb) -> Color {
        Color {
            A: 255,
            R: c.r,
            G: c.g,
            B: c.b,
        }
    }

    /// Runs `call` on every device, even after one fails.
    fn each(
        arrays: &[LampArray],
        mut call: impl FnMut(&LampArray) -> windows::core::Result<()>,
    ) -> foxglove_devices::Result<()> {
        let mut failed = None;
        for a in arrays {
            if let Err(e) = call(a) {
                failed = Some(e);
            }
        }
        match failed {
            Some(e) => Err(foxglove_devices::DeviceError::Backend(e.to_string())),
            None => Ok(()),
        }
    }

    impl foxglove_devices::pc_lighting::LampHandle for Handle {
        fn access(&self) -> foxglove_devices::pc_lighting::LampAccess {
            // Windows before 11 22H2 has neither: there, an app in front always may.
            foxglove_devices::pc_lighting::LampAccess {
                enabled: self.arrays.iter().all(|a| a.IsEnabled().unwrap_or(true)),
                available: self.arrays.iter().any(|a| a.IsAvailable().unwrap_or(true)),
            }
        }

        fn lamp_count(&self) -> usize {
            self.arrays
                .iter()
                .map(|a| a.LampCount().unwrap_or(0).max(0) as usize)
                .min()
                .unwrap_or(0)
        }

        fn set_all(&mut self, c: foxglove_devices::Rgb) -> foxglove_devices::Result<()> {
            each(&self.arrays, |a| a.SetColor(color(c)))
        }

        fn set_lamps(
            &mut self,
            lamps: &[(usize, foxglove_devices::Rgb)],
        ) -> foxglove_devices::Result<()> {
            let colours: Vec<Color> = lamps.iter().map(|&(_, c)| color(c)).collect();
            let indices: Vec<i32> = lamps.iter().map(|&(i, _)| i as i32).collect();
            each(&self.arrays, |a| a.SetColorsForIndices(&colours, &indices))
        }
    }

    /// Opens the Lights worker's handle for a key, claiming its devices in `claimed`; `None`
    /// when Windows lists none for it. `changed` runs (on Windows' thread) each time Windows
    /// changes who may drive one of them.
    pub fn handle(
        key: &str,
        claimed: &mut BTreeMap<String, String>,
        changed: Changed,
    ) -> Option<Handle> {
        let arrays = arrays_for(key, claimed).unwrap_or_else(|e| {
            eprintln!("Foxglove: opening Dynamic Lighting device {key}: {e}");
            Vec::new()
        });
        if arrays.is_empty() {
            return None;
        }
        let tokens = arrays
            .iter()
            .map(|a| {
                let tell = changed.clone();
                a.AvailabilityChanged(&TypedEventHandler::new(move |_, _| {
                    tell();
                    Ok(())
                }))
                .ok()
            })
            .collect();
        Some(Handle { arrays, tokens })
    }
}

/// Tells the Lights worker which device's access changed, by its key.
pub type Notify = Arc<dyn Fn(&str) + Send + Sync>;

/// The Lights worker's handles on Dynamic Lighting devices (#691), through Windows. It keeps
/// its own claims: scenes open the same devices separately, and both may hold them at once.
/// `notify` gets the device's key on each `AvailabilityChanged`.
#[cfg(target_os = "windows")]
pub use imp::watch_lamp_arrays;

#[cfg(target_os = "windows")]
pub fn worker_opener(notify: Notify) -> super::pc_lights::LampOpener {
    let mut claimed = BTreeMap::new();
    Box::new(move |f: &foxglove_devices::pc_lighting::Found| {
        let (tell, key) = (notify.clone(), f.key.clone());
        let changed: Changed = Arc::new(move || tell(&key));
        imp::handle(&f.key, &mut claimed, changed)
            .map(|h| Box::new(h) as Box<dyn foxglove_devices::pc_lighting::LampHandle>)
    })
}

/// Elsewhere there are no Dynamic Lighting devices.
#[cfg(not(target_os = "windows"))]
pub fn worker_opener(_notify: Notify) -> super::pc_lights::LampOpener {
    Box::new(|_| None)
}

/// Elsewhere there are no Dynamic Lighting devices.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct WindowsLamps;

/// Windows' holds on the keyboard's own LampArrays (`lamp_hold`, #179).
#[derive(Default)]
pub struct WindowsHolds;

#[cfg(not(target_os = "windows"))]
impl super::lamp_hold::HoldOpener for WindowsHolds {
    fn open(&mut self, _: u16, _: u16, _: Changed) -> super::lamp_hold::Opened {
        super::lamp_hold::Opened::default()
    }
}

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
            notify: None,
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

    /// #749 b: the keyboard's own LampArray while Foxglove isn't in front (`IsAvailable` false):
    /// nothing is written; the event that makes it available shows the newest frame at once,
    /// and each change of availability is passed on (a held keyboard retries its host link).
    #[test]
    fn the_keyboards_lamparray_waits_for_windows_and_says_each_change() {
        let fake = Arc::new(Fake::default());
        let (mut core, rx) = core(&fake);
        let told = Arc::new(Mutex::new(Vec::<String>::new()));
        let t = told.clone();
        core.notify = Some(Arc::new(move |k: &str| t.lock().unwrap().push(k.into())));
        core.handle(vec![frame(1), frame(2)]);
        assert!(fake.writes.lock().unwrap().is_empty(), "not Foxglove's yet");
        assert!(core.status().waiting.contains(KEY));
        windows_says(&mut core, &rx, &fake, true);
        assert_eq!(*fake.writes.lock().unwrap(), [vec![[2, 0, 0]; 8]]);
        // The same answer again changes nothing and isn't passed on.
        windows_says(&mut core, &rx, &fake, true);
        windows_says(&mut core, &rx, &fake, false);
        assert_eq!(*told.lock().unwrap(), [KEY, KEY]);
        assert_eq!(fake.writes.lock().unwrap().len(), 1);
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
            None,
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
