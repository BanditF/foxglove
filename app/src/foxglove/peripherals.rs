//! Foxglove (#504): the batteries of the other devices on the desk, in one list next to the
//! halves': Bluetooth devices that report a battery, and game controllers.
//!
//! - **Bluetooth:** Windows keeps each paired device's level in a device property
//!   (`{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2`, a byte, from the Battery Service or the
//!   hands-free link), and whether it's connected in `{83DA6326-97A6-4088-9453-A1923F573B29} 15`.
//!   A `DeviceWatcher` over the Bluetooth class with those properties sends each change as it
//!   happens, so nothing polls. Devices without the property stay out of the list, quietly, and
//!   so does one that isn't connected (its level would be stale), as in Windows' own Settings.
//! - **Controllers:** `Windows.Gaming.Input` says when one comes or goes, but a level has no
//!   change event. A controller is read when it arrives, and then at most once a minute while
//!   the Keyboard page or the tray panel is on screen ([`visible`]); with neither open, or with
//!   no controller, nothing runs.
//! - **The keyboard itself** is left out (its container is the remembered keyboard's): its
//!   halves have their own lines.
//! - **Heads-up:** the halves' low-battery notices (#372) cover these devices too, with their
//!   own switch.
//!
//! [`run`] does the work with a [`Controllers`] source, so tests feed it fake devices.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::battery::Alerts;

/// The slowest a controller's level goes stale while it's on screen.
pub const CONTROLLER_REFRESH: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Bluetooth,
    Controller,
}

/// A device in the list, as the UI shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Peripheral {
    pub name: String,
    pub kind: Kind,
    pub percent: u8,
    pub charging: bool,
}

/// What a source knows of one device.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// The physical device: Windows' container id. Its Bluetooth parts (a mouse has a few)
    /// share it, so they show once.
    pub group: String,
    pub name: String,
    pub kind: Kind,
    /// `None`: it has no battery (most Bluetooth parts), so it isn't listed.
    pub percent: Option<u8>,
    pub connected: bool,
    pub charging: bool,
}

/// A change from a source.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Arrived, or read again.
    Seen(String, Record),
    /// Some of its properties changed; `None` leaves that one as it was.
    Updated {
        id: String,
        percent: Option<u8>,
        connected: Option<bool>,
    },
    Gone(String),
    /// Every controller as read now: one missing has gone.
    Controllers(Vec<(String, Record)>),
}

/// Every device the sources told about, listed or not.
#[derive(Clone, Debug, Default)]
pub struct List {
    records: BTreeMap<String, Record>,
}

impl List {
    pub fn apply(&mut self, change: Change) {
        match change {
            Change::Seen(id, r) => {
                self.records.insert(id, r);
            }
            Change::Updated {
                id,
                percent,
                connected,
            } => {
                if let Some(r) = self.records.get_mut(&id) {
                    if percent.is_some() {
                        r.percent = percent;
                    }
                    if let Some(c) = connected {
                        r.connected = c;
                    }
                }
            }
            Change::Gone(id) => {
                self.records.remove(&id);
            }
            Change::Controllers(now) => {
                self.records.retain(|_, r| r.kind != Kind::Controller);
                self.records.extend(now);
            }
        }
    }

    /// Whether a controller is known (so a refresh while on screen means something).
    pub fn has_controllers(&self) -> bool {
        self.records.values().any(|r| r.kind == Kind::Controller)
    }

    /// The list: connected devices with a level, once per physical device, without the ones
    /// in `hidden` (the keyboard), Bluetooth first, then by name.
    pub fn shown(&self, hidden: &[String]) -> Vec<(String, Peripheral)> {
        let mut seen = BTreeMap::new();
        for r in self.records.values() {
            let Some(percent) = r.percent.filter(|p| *p <= 100) else {
                continue;
            };
            if !r.connected || hidden.contains(&r.group) || seen.contains_key(&r.group) {
                continue;
            }
            seen.insert(
                r.group.clone(),
                Peripheral {
                    name: r.name.trim().to_string(),
                    kind: r.kind,
                    percent,
                    charging: r.charging,
                },
            );
        }
        let mut out: Vec<_> = seen.into_iter().collect();
        out.sort_by(|(_, a), (_, b)| {
            (a.kind, a.name.to_lowercase()).cmp(&(b.kind, b.name.to_lowercase()))
        });
        out
    }
}

/// A controller's level from its battery report: `None` for a wired one (no battery), or one
/// that doesn't know its capacity yet.
pub fn controller_percent(remaining_mwh: Option<i32>, full_mwh: Option<i32>) -> Option<u8> {
    let (r, f) = (remaining_mwh?, full_mwh?);
    if f <= 0 || r < 0 {
        return None;
    }
    Some(((i64::from(r) * 100 + i64::from(f) / 2) / i64::from(f)).min(100) as u8)
}

/// A level in words: `61%`, `61% · charging`.
pub fn level_text(p: &Peripheral) -> String {
    if p.charging {
        format!("{}% · charging", p.percent)
    } else {
        format!("{}%", p.percent)
    }
}

/// The notification's title and text for a device.
pub fn notice(name: &str, threshold: u8, percent: u8) -> (String, String) {
    if threshold <= super::battery::THRESHOLDS[super::battery::THRESHOLDS.len() - 1] {
        (
            format!("{name} battery very low"),
            format!("{percent}% left. It will switch off soon: charge it."),
        )
    } else {
        (
            format!("{name} battery low"),
            format!("{percent}% left. Charge it soon."),
        )
    }
}

/// When the controllers are read again while the list is on screen.
#[derive(Debug, Default)]
pub struct Refresh {
    last: Option<Instant>,
}

impl Refresh {
    /// The list is on screen at `now`: whether to read the controllers now, and when to look
    /// again. Never called while nothing shows the list, so nothing runs then.
    pub fn visible(&mut self, now: Instant) -> (bool, Duration) {
        match self.last {
            Some(at) if now.duration_since(at) < CONTROLLER_REFRESH => {
                (false, CONTROLLER_REFRESH - now.duration_since(at))
            }
            _ => {
                self.last = Some(now);
                (true, CONTROLLER_REFRESH)
            }
        }
    }

    /// The controllers were read for another reason (one came or went).
    pub fn read(&mut self, now: Instant) {
        self.last = Some(now);
    }
}

/// Reads the game controllers.
pub trait Controllers {
    fn read(&self) -> Vec<(String, Record)>;
}

/// What the worker is told.
#[derive(Debug)]
pub enum Msg {
    Change(Change),
    /// A controller came or went: read them all.
    ControllersChanged,
    /// The list is on screen and the controllers' levels are a minute old.
    ReadControllers,
}

/// What the worker publishes after each change.
pub trait Output {
    fn shown(&mut self, list: &[(String, Peripheral)], has_controllers: bool);
    fn notice(&mut self, title: &str, text: &str);
    /// The containers to leave out (the keyboard), asked again when a device with a battery
    /// arrives.
    fn hidden(&mut self) -> Vec<String>;
}

/// The worker: applies each change, publishes the list when it changes, and tells each
/// threshold crossing once. Returns when every sender is gone.
pub fn run(rx: &Receiver<Msg>, controllers: &dyn Controllers, out: &mut dyn Output) {
    let mut list = List::default();
    let mut alerts = Alerts::default();
    let mut hidden = Vec::new();
    let mut last: Option<Vec<(String, Peripheral)>> = None;
    for msg in rx {
        match msg {
            Msg::Change(c) => {
                if matches!(&c, Change::Seen(_, r) if r.percent.is_some()) {
                    hidden = out.hidden();
                }
                list.apply(c);
            }
            Msg::ControllersChanged => {
                REFRESH
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .read(Instant::now());
                list.apply(Change::Controllers(controllers.read()));
            }
            Msg::ReadControllers => list.apply(Change::Controllers(controllers.read())),
        }
        let mut shown = list.shown(&hidden);
        // A device new to the list (one that connected, or a keyboard remembered since): ask
        // again which to leave out, so the keyboard never shows as a device.
        let new = |s: &[(String, Peripheral)]| {
            s.iter().any(|(g, _)| {
                !last
                    .as_ref()
                    .is_some_and(|l| l.iter().any(|(lg, _)| lg == g))
            })
        };
        if new(&shown) {
            hidden = out.hidden();
            shown = list.shown(&hidden);
        }
        if last.as_ref() == Some(&shown) {
            continue;
        }
        for (group, p) in &shown {
            if let Some(t) = alerts.level(&format!("device:{group}"), p.percent) {
                let (title, text) = notice(&p.name, t, p.percent);
                out.notice(&title, &text);
            }
        }
        out.shown(&shown, list.has_controllers());
        last = Some(shown);
    }
}

/// The list as last published.
static SHOWN: Mutex<Vec<Peripheral>> = Mutex::new(Vec::new());
static HAS_CONTROLLERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TX: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();
static REFRESH: Mutex<Refresh> = Mutex::new(Refresh { last: None });

/// The devices with a battery, for the tray panel and the Keyboard page.
pub fn list() -> Vec<Peripheral> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The Keyboard page or the tray panel shows the list this frame: with a controller known,
/// its level is read again when it's a minute old, and one more frame is asked for then. With
/// no controller, nothing.
pub fn visible(ctx: &egui::Context) {
    let has = HAS_CONTROLLERS.load(std::sync::atomic::Ordering::Relaxed);
    let mut refresh = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    let Some((read, again)) = on_screen(has, &mut refresh, Instant::now()) else {
        return;
    };
    drop(refresh);
    if read {
        send(Msg::ReadControllers);
    }
    ctx.request_repaint_after(again);
}

/// [`visible`]'s decision: nothing without a controller, else [`Refresh::visible`].
fn on_screen(has_controllers: bool, r: &mut Refresh, now: Instant) -> Option<(bool, Duration)> {
    has_controllers.then(|| r.visible(now))
}

fn send(msg: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(msg);
    }
}

struct AppOutput;

impl Output for AppOutput {
    fn shown(&mut self, list: &[(String, Peripheral)], has_controllers: bool) {
        *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) =
            list.iter().map(|(_, p)| p.clone()).collect();
        HAS_CONTROLLERS.store(has_controllers, std::sync::atomic::Ordering::Relaxed);
        super::battery::wake();
    }

    fn notice(&mut self, title: &str, text: &str) {
        if super::battery::device_notices_on() {
            super::battery::show_notice(title, text);
        }
    }

    fn hidden(&mut self) -> Vec<String> {
        #[cfg(windows)]
        return super::connection::keyboard_containers()
            .into_iter()
            .map(imp::group)
            .collect();
        #[cfg(not(windows))]
        Vec::new()
    }
}

/// Starts the Bluetooth watch and the controller events, and the worker that follows them.
pub fn start() {
    let (tx, rx) = std::sync::mpsc::channel();
    if TX.set(Mutex::new(tx.clone())).is_err() {
        return;
    }
    // Supervised (#587): the queue outlives a panicking attempt, so `TX` stays good; a restart
    // watches again and starts from an empty list.
    let rx = Mutex::new(rx);
    let _ = super::services::spawn_supervised("peripherals", move || {
        let rx = rx.lock().unwrap_or_else(|e| e.into_inner());
        #[cfg(windows)]
        let _watch = match imp::watch(tx.clone()) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("Foxglove: can't watch the Bluetooth batteries: {e}");
                None
            }
        };
        #[cfg(not(windows))]
        let _ = &tx;
        run(&rx, &imp::GameInput, &mut AppOutput);
    });
}

#[cfg(windows)]
mod imp {
    use super::{Change, Kind, Msg, Record};
    use std::sync::mpsc::Sender;
    use windows::core::{IInspectable, Interface, HSTRING};
    use windows::Devices::Enumeration::{
        DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher,
    };
    use windows::Foundation::{EventHandler, IPropertyValue, TypedEventHandler};
    use windows::Gaming::Input::RawGameController;
    use windows::System::Power::BatteryStatus;
    use windows_collections::IMapView;

    const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
    const CONNECTED: &str = "{83DA6326-97A6-4088-9453-A1923F573B29} 15";
    const CONTAINER: &str = "System.Devices.ContainerId";
    /// Present devices of the Bluetooth class: PnP device nodes, so no radio scan (#100).
    const QUERY: &str = r#"System.Devices.ClassGuid:="{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}" AND System.Devices.Present:=System.StructuredQueryType.Boolean#True"#;

    pub fn group(container: u128) -> String {
        format!("{container:032x}")
    }

    fn value(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<IPropertyValue> {
        props
            .Lookup(&HSTRING::from(key))
            .ok()
            .and_then(|v| v.cast::<IPropertyValue>().ok())
    }

    fn percent(props: &IMapView<HSTRING, IInspectable>) -> Option<u8> {
        value(props, BATTERY).and_then(|v| v.GetUInt8().ok())
    }

    fn connected(props: &IMapView<HSTRING, IInspectable>) -> Option<bool> {
        value(props, CONNECTED).and_then(|v| v.GetBoolean().ok())
    }

    /// Keeps the watch running; stops it when dropped.
    pub struct Watch(DeviceWatcher);

    impl Drop for Watch {
        fn drop(&mut self) {
            let _ = self.0.Stop();
        }
    }

    /// The Bluetooth watch and the controllers' arrival events, each sending to `tx`. The
    /// controller events stay registered for the process's life.
    pub fn watch(tx: Sender<Msg>) -> windows::core::Result<Watch> {
        let properties = windows_collections::IIterable::from(vec![
            HSTRING::from(BATTERY),
            HSTRING::from(CONNECTED),
            HSTRING::from(CONTAINER),
        ]);
        let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
            &HSTRING::from(QUERY),
            &properties,
            DeviceInformationKind::Device,
        )?;
        let added = tx.clone();
        watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
            move |_, info| {
                let Some(info) = info.as_ref() else {
                    return Ok(());
                };
                let props = info.Properties()?;
                let container = value(&props, CONTAINER)
                    .and_then(|v| v.GetGuid().ok())
                    .map_or_else(
                        || info.Id().map(|i| i.to_string()),
                        |g| Ok(group(g.to_u128())),
                    )?;
                let record = Record {
                    group: container,
                    name: info.Name()?.to_string(),
                    kind: Kind::Bluetooth,
                    percent: percent(&props),
                    connected: connected(&props).unwrap_or(false),
                    charging: false,
                };
                let _ = added.send(Msg::Change(Change::Seen(info.Id()?.to_string(), record)));
                Ok(())
            },
        ))?;
        let updated = tx.clone();
        watcher.Updated(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, info| {
                let Some(info) = info.as_ref() else {
                    return Ok(());
                };
                let props = info.Properties()?;
                let _ = updated.send(Msg::Change(Change::Updated {
                    id: info.Id()?.to_string(),
                    percent: percent(&props),
                    connected: connected(&props),
                }));
                Ok(())
            }),
        )?;
        let removed = tx.clone();
        watcher.Removed(
            &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, info| {
                if let Some(info) = info.as_ref() {
                    let _ = removed.send(Msg::Change(Change::Gone(info.Id()?.to_string())));
                }
                Ok(())
            }),
        )?;
        watcher.Start()?;
        // The ones already plugged in, once; then each arrival and removal.
        let _ = tx.send(Msg::ControllersChanged);
        let came = tx.clone();
        let went = tx;
        let controllers = (|| -> windows::core::Result<()> {
            RawGameController::RawGameControllerAdded(&EventHandler::new(move |_, _| {
                let _ = came.send(Msg::ControllersChanged);
                Ok(())
            }))?;
            RawGameController::RawGameControllerRemoved(&EventHandler::new(move |_, _| {
                let _ = went.send(Msg::ControllersChanged);
                Ok(())
            }))?;
            Ok(())
        })();
        if let Err(e) = controllers {
            eprintln!("Foxglove: can't follow the game controllers: {e}");
        }
        Ok(Watch(watcher))
    }

    pub struct GameInput;

    impl super::Controllers for GameInput {
        fn read(&self) -> Vec<(String, Record)> {
            let read = || -> windows::core::Result<Vec<(String, Record)>> {
                let all = RawGameController::RawGameControllers()?;
                let mut out = Vec::new();
                for i in 0..all.Size()? {
                    let c = all.GetAt(i)?;
                    let id = c.NonRoamableId()?.to_string();
                    let report = c.TryGetBatteryReport().ok();
                    let status = report.as_ref().and_then(|r| r.Status().ok());
                    let level = report.as_ref().and_then(|r| {
                        super::controller_percent(
                            r.RemainingCapacityInMilliwattHours()
                                .and_then(|v| v.Value())
                                .ok(),
                            r.FullChargeCapacityInMilliwattHours()
                                .and_then(|v| v.Value())
                                .ok(),
                        )
                    });
                    let percent = level.filter(|_| status != Some(BatteryStatus::NotPresent));
                    out.push((
                        format!("pad:{id}"),
                        Record {
                            group: format!("pad:{id}"),
                            name: c.DisplayName()?.to_string(),
                            kind: Kind::Controller,
                            percent,
                            connected: true,
                            charging: status == Some(BatteryStatus::Charging),
                        },
                    ));
                }
                Ok(out)
            };
            read().unwrap_or_else(|e| {
                eprintln!("Foxglove: can't read the game controllers: {e}");
                Vec::new()
            })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct GameInput;

    impl super::Controllers for GameInput {
        fn read(&self) -> Vec<(String, super::Record)> {
            Vec::new()
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::Cell;

    fn bt(group: &str, name: &str, percent: Option<u8>, connected: bool) -> Record {
        Record {
            group: group.into(),
            name: name.into(),
            kind: Kind::Bluetooth,
            percent,
            connected,
            charging: false,
        }
    }

    fn pad(name: &str, percent: Option<u8>, charging: bool) -> (String, Record) {
        (
            format!("pad:{name}"),
            Record {
                group: format!("pad:{name}"),
                name: name.into(),
                kind: Kind::Controller,
                percent,
                connected: true,
                charging,
            },
        )
    }

    /// A fake Gaming.Input: the controllers it holds, and how often it was read.
    #[derive(Default)]
    struct FakePads {
        now: std::sync::Mutex<Vec<(String, Record)>>,
        reads: Cell<usize>,
    }

    impl Controllers for FakePads {
        fn read(&self) -> Vec<(String, Record)> {
            self.reads.set(self.reads.get() + 1);
            self.now.lock().unwrap().clone()
        }
    }

    #[derive(Default)]
    struct Seen {
        lists: Vec<Vec<String>>,
        notices: Vec<String>,
        hidden: Vec<String>,
    }

    impl Output for Seen {
        fn shown(&mut self, list: &[(String, Peripheral)], _: bool) {
            self.lists.push(
                list.iter()
                    .map(|(_, p)| format!("{} {}", p.name, level_text(p)))
                    .collect(),
            );
        }
        fn notice(&mut self, title: &str, _: &str) {
            self.notices.push(title.into());
        }
        fn hidden(&mut self) -> Vec<String> {
            self.hidden.clone()
        }
    }

    fn worker(msgs: Vec<Msg>, pads: &FakePads, hidden: &[&str]) -> Seen {
        let (tx, rx) = std::sync::mpsc::channel();
        for m in msgs {
            tx.send(m).unwrap();
        }
        drop(tx);
        let mut out = Seen {
            hidden: hidden.iter().map(|h| h.to_string()).collect(),
            ..Default::default()
        };
        run(&rx, pads, &mut out);
        out
    }

    fn seen(id: &str, r: Record) -> Msg {
        Msg::Change(Change::Seen(id.into(), r))
    }

    /// A fake Bluetooth watch: the radio, the mouse's parts (one with a battery), a headset that
    /// isn't connected, and the keyboard itself. Only the mouse is listed, once, with no error
    /// for the parts without a battery and no controller read.
    #[test]
    fn bluetooth_devices_without_a_battery_are_left_out() {
        let pads = FakePads::default();
        let out = worker(
            vec![
                seen("radio", bt("pc", "Bluetooth radio", None, true)),
                seen("mouse-hid", bt("m", "MX Master 3", None, true)),
                seen("mouse-le", bt("m", "MX Master 3", Some(80), true)),
                seen("buds", bt("b", "Buds", Some(40), false)),
                seen("corne", bt("k", "Eyelash Corne", Some(100), true)),
            ],
            &pads,
            &["k"],
        );
        assert_eq!(out.lists.last().unwrap(), &vec!["MX Master 3 80%"]);
        assert_eq!(pads.reads.get(), 0, "no controller was read");
        assert!(out.notices.is_empty());
    }

    /// Windows' property updates move the level and the connection; each change publishes once.
    #[test]
    fn updates_follow_the_device() {
        let pads = FakePads::default();
        let up = |percent, connected| {
            Msg::Change(Change::Updated {
                id: "buds".into(),
                percent,
                connected,
            })
        };
        let out = worker(
            vec![
                seen("buds", bt("b", "Buds", Some(40), false)),
                up(None, Some(true)),
                up(Some(39), None),
                up(Some(39), None),
                up(None, Some(false)),
                Msg::Change(Change::Gone("buds".into())),
            ],
            &pads,
            &[],
        );
        assert_eq!(
            out.lists,
            vec![
                vec![],
                vec!["Buds 40%".to_string()],
                vec!["Buds 39%".to_string()],
                vec![],
            ]
        );
    }

    /// Controllers are read on arrival and on a refresh only; a wired one (no battery) is out.
    #[test]
    fn controllers_are_read_on_arrival() {
        let pads = FakePads::default();
        *pads.now.lock().unwrap() = vec![
            pad("Xbox Wireless Controller", Some(55), true),
            pad("Wired pad", None, false),
        ];
        let out = worker(
            vec![
                seen("mouse", bt("m", "MX Master 3", Some(80), true)),
                Msg::ControllersChanged,
            ],
            &pads,
            &[],
        );
        assert_eq!(pads.reads.get(), 1);
        assert_eq!(
            out.lists.last().unwrap(),
            &vec!["MX Master 3 80%", "Xbox Wireless Controller 55% · charging"]
        );
        // One gone: a read without it drops it.
        let pads = FakePads::default();
        let out = worker(vec![Msg::ControllersChanged], &pads, &[]);
        assert_eq!(out.lists, vec![Vec::<String>::new()], "nothing to show");
    }

    /// The heads-up: once per crossing per device, as for the halves.
    #[test]
    fn a_low_device_tells_once_per_crossing() {
        let pads = FakePads::default();
        let level = |p| {
            Msg::Change(Change::Updated {
                id: "mouse".into(),
                percent: Some(p),
                connected: None,
            })
        };
        let out = worker(
            vec![
                seen("mouse", bt("m", "MX Master 3", Some(30), true)),
                level(16),
                level(14),
                level(13),
                level(4),
                level(25),
                level(12),
            ],
            &pads,
            &[],
        );
        assert_eq!(
            out.notices,
            vec![
                "MX Master 3 battery low",
                "MX Master 3 battery very low",
                "MX Master 3 battery low"
            ]
        );
    }

    /// With nothing on screen the refresh never runs; on screen, once a minute at most.
    #[test]
    fn no_refresh_while_nothing_is_open() {
        let mut r = Refresh::default();
        let t = Instant::now();
        // Never asked: nothing runs (there is no timer of its own).
        let (read, again) = r.visible(t);
        assert!(read, "opening the page reads them");
        assert_eq!(again, CONTROLLER_REFRESH);
        let (read, again) = r.visible(t + Duration::from_secs(10));
        assert!(!read);
        assert_eq!(again, Duration::from_secs(50));
        assert!(r.visible(t + CONTROLLER_REFRESH).0);
        // An arrival read counts.
        r.read(t + Duration::from_secs(200));
        assert!(!r.visible(t + Duration::from_secs(230)).0);
        // Without a controller, nothing: no read and no frame asked for.
        let mut fresh = Refresh::default();
        assert_eq!(on_screen(false, &mut fresh, t), None);
        assert_eq!(fresh.last, None);
        assert_eq!(
            on_screen(true, &mut fresh, t),
            Some((true, CONTROLLER_REFRESH))
        );
    }

    #[test]
    fn controller_levels_from_the_report() {
        assert_eq!(controller_percent(Some(500), Some(1000)), Some(50));
        assert_eq!(controller_percent(Some(1000), Some(999)), Some(100));
        assert_eq!(controller_percent(None, Some(1000)), None);
        assert_eq!(controller_percent(Some(5), Some(0)), None);
        assert_eq!(controller_percent(Some(-1), Some(10)), None);
    }

    #[test]
    fn a_level_over_100_is_not_a_level() {
        let mut l = List::default();
        l.apply(Change::Seen("x".into(), bt("x", "X", Some(255), true)));
        assert!(l.shown(&[]).is_empty());
    }

    /// This PC's devices, read-only: `cargo test live_batteries -- --ignored --nocapture`.
    /// Prints what kind of devices report a battery, never their names.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads this PC's devices"]
    fn live_batteries() {
        let (tx, rx) = std::sync::mpsc::channel();
        let watch = imp::watch(tx).expect("the watch starts");
        let until = Instant::now() + Duration::from_secs(3);
        let mut list = List::default();
        let mut controllers = false;
        while let Ok(m) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
            match m {
                Msg::Change(c) => list.apply(c),
                _ => controllers = true,
            }
        }
        drop(watch);
        let with: Vec<_> = list
            .records
            .values()
            .filter(|r| r.percent.is_some())
            .collect();
        eprintln!(
            "bluetooth parts: {}, with a battery: {}, connected: {}, controllers event: {controllers}, controllers: {}",
            list.records.len(),
            with.len(),
            with.iter().filter(|r| r.connected).count(),
            imp::GameInput.read().len()
        );
    }

    /// The previews' devices.
    pub fn sample() -> Vec<Peripheral> {
        vec![
            Peripheral {
                name: "MX Master 3".into(),
                kind: Kind::Bluetooth,
                percent: 80,
                charging: false,
            },
            Peripheral {
                name: "WH-1000XM4".into(),
                kind: Kind::Bluetooth,
                percent: 64,
                charging: true,
            },
            Peripheral {
                name: "Xbox Wireless Controller".into(),
                kind: Kind::Controller,
                percent: 12,
                charging: false,
            },
        ]
    }
}
