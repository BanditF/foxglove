//! Wallpaper sync, stage 1 (#493, #377; the plan on #39): Foxglove tells each Bandit wallpaper
//! the keyboard's layer, whether game mode is on, and the running scene's clock, through
//! Wallpaper Engine's own command line:
//!
//! ```text
//! wallpaper64.exe -control applyProperties -location <monitor> -properties RAW~({"foxglove_game":true,"foxglove_layer":"GAME","foxglove_scene":"v=1;..."})~END
//! ```
//!
//! The wallpaper gets them in `wallpaperPropertyListener.applyUserProperties`, the path the
//! editor uses, and draws its monitor's part of the scene from the shared epoch
//! (`foxglove_scene::wallpaper`, `shared/foxglove-sync.js` in bandit-wallpapers). Nothing is
//! streamed: one push per change, and only to the monitors that show a Bandit wallpaper.
//!
//! - **When:** a scene starts, stops or is placed again; the layer or game mode changes; the
//!   wallpapers on screen change (a new wallpaper gets the current state). A push goes out only
//!   when a monitor's properties differ from what it was last sent, and one push runs at a time:
//!   changes that arrive meanwhile collapse to the newest.
//! - **Facts only:** command lines are visible to other processes, so the properties are the
//!   layer's name, a bool and the scene's timing, colours and geometry. Never a token.
//! - **Optional:** without Wallpaper Engine, or while it isn't running, nothing is sent and no
//!   thread starts (its command line would start it). The worker starts the first time Wallpaper
//!   Engine shows a Bandit wallpaper.
//! - **Game mode:** a layer named GAME... is active (the GAME / GAME+ layers, docs/layers.md).
//!
//! Later (#320): the live socket carries key presses, PC stats and notifications. A transport is
//! one [`Apply`]; the state and the change detection here stay as they are.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

use foxglove_scene::{Clock, Effect, Rect, RoomMap, WallpaperScene};

/// The properties the Bandit wallpapers declare (bandit-wallpapers `shared/properties.json`).
pub const LAYER: &str = "foxglove_layer";
pub const GAME: &str = "foxglove_game";
pub const SCENE: &str = "foxglove_scene";

/// How long one push may take before it is given up (and its process ended).
const PUSH_LIMIT: Duration = Duration::from_secs(5);
/// How long quitting waits for the wallpapers to be told Foxglove has gone.
const QUIT_LIMIT: Duration = Duration::from_secs(6);

/// A running scene, as the wallpapers need it.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub id: String,
    pub effect: Effect,
    pub clock: Clock,
    pub room: RoomMap,
    /// Counts the scene's placements (start, devices placed again): the monitors' interface
    /// paths are read again on each.
    pub placement: u64,
}

/// What the wallpapers are told.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The highest active layer's name; `None` without a keyboard.
    pub layer: Option<String>,
    pub game: bool,
    pub scene: Option<Scene>,
}

/// A layer name as a property: letters, digits, `+`, `-` and `_`, upper case, at most 16.
fn clean_layer(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_'))
        .take(16)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// The highest active layer's name (`L<n>` when the keyboard didn't name it).
pub fn layer_name(active: &[u8], names: &[Option<String>]) -> Option<String> {
    let top = *active.iter().max()?;
    let named = names
        .get(usize::from(top))
        .and_then(|n| n.as_deref())
        .map(clean_layer)
        .filter(|n| !n.is_empty());
    Some(named.unwrap_or_else(|| format!("L{top}")))
}

/// Game mode: any active layer is named GAME... (GAME, GAME+).
pub fn game_mode(active: &[u8], names: &[Option<String>]) -> bool {
    active.iter().any(|&i| {
        names
            .get(usize::from(i))
            .and_then(|n| n.as_deref())
            .is_some_and(|n| clean_layer(n).starts_with("GAME"))
    })
}

/// One monitor's properties as compact JSON: no spaces, and no quotes inside the values, so it
/// passes through `RAW~(...)~END` as typed.
pub fn properties(state: &State, monitor: Option<Rect>) -> String {
    let scene = state.scene.as_ref().map_or_else(String::new, |s| {
        WallpaperScene::new(&s.id, &s.effect, &s.clock, s.room.bounds, monitor).encode()
    });
    serde_json::json!({
        LAYER: state.layer.clone().unwrap_or_default(),
        GAME: state.game,
        SCENE: scene,
    })
    .to_string()
}

/// What quitting leaves: no layer, no game mode, no scene (the wallpaper runs on its own).
fn gone() -> String {
    properties(&State::default(), None)
}

/// The command line's arguments after the program, the properties last (they go in raw, as a
/// user types them in a terminal: Wallpaper Engine reads them between `RAW~(` and `)~END`).
pub fn arguments(location: &str, props: &str) -> (Vec<String>, String) {
    (
        vec![
            "-control".into(),
            "applyProperties".into(),
            "-location".into(),
            location.into(),
        ],
        format!("-properties RAW~({props})~END"),
    )
}

/// Wallpaper Engine's monitor key (`//?/DISPLAY#DEL40EA#...#{e6f07b5f-...}` in its config.json,
/// the monitor's device interface path) against Windows' own, ignoring case and slashes.
fn same_monitor(we_key: &str, interface: &str) -> bool {
    let norm = |s: &str| s.trim().replace('\\', "/").to_ascii_lowercase();
    !interface.trim().is_empty() && norm(we_key) == norm(interface)
}

/// The rectangle in the room of the monitor Wallpaper Engine calls `location`. `interfaces`
/// pairs each Windows display (`\\.\DISPLAY1`) with its interface path.
pub fn monitor_rect(
    location: &str,
    room: &RoomMap,
    interfaces: &[(String, String)],
) -> Option<Rect> {
    let (gdi, _) = interfaces
        .iter()
        .find(|(_, iface)| same_monitor(location, iface))?;
    room.device(&format!("monitor:{gdi}")).map(|p| p.rect)
}

/// Sends one monitor its properties: Wallpaper Engine's command line, or a fake in tests.
pub trait Apply: Send {
    /// Whether anything may be sent now (Wallpaper Engine runs).
    fn ready(&mut self) -> bool;
    fn apply(&mut self, location: &str, props: &str) -> Result<(), String>;
    /// Each Windows display's interface path, for [`monitor_rect`].
    fn interfaces(&mut self) -> Vec<(String, String)>;
}

/// One monitor showing a Bandit wallpaper: Wallpaper Engine's key for it and the wallpaper's
/// file (another wallpaper on the same monitor starts from its defaults, so it is sent again).
pub type Target = (String, String);

/// What has been sent, per monitor, and the pushes that keep it current.
pub struct Pusher<A: Apply> {
    apply: A,
    /// Per monitor: the wallpaper's file and the properties it was sent.
    sent: BTreeMap<String, (String, String)>,
    /// The interface paths, read once per scene placement.
    interfaces: Option<Vec<(String, String)>>,
    placement_seen: Option<u64>,
}

impl<A: Apply> Pusher<A> {
    pub fn new(apply: A) -> Self {
        Pusher {
            apply,
            sent: BTreeMap::new(),
            interfaces: None,
            placement_seen: None,
        }
    }

    /// Brings every target up to `state`; returns how many monitors were sent something.
    pub fn sync(&mut self, state: &State, targets: &[Target]) -> usize {
        self.sent.retain(|k, _| targets.iter().any(|(t, _)| t == k));
        if targets.is_empty() {
            return 0;
        }
        if !self.apply.ready() {
            // Everything goes again once it runs (it starts with the wallpapers' defaults).
            self.sent.clear();
            return 0;
        }
        let placement = state.scene.as_ref().map(|s| s.placement);
        if placement != self.placement_seen {
            self.placement_seen = placement;
            self.interfaces = None;
        }
        let mut pushed = 0;
        for (location, file) in targets {
            let monitor = state.scene.as_ref().and_then(|s| {
                let ifaces = self
                    .interfaces
                    .get_or_insert_with(|| self.apply.interfaces());
                monitor_rect(location, &s.room, ifaces)
            });
            let props = properties(state, monitor);
            if self
                .sent
                .get(location)
                .is_some_and(|(f, p)| f == file && *p == props)
            {
                continue;
            }
            match self.apply.apply(location, &props) {
                Ok(()) => {
                    self.sent.insert(location.clone(), (file.clone(), props));
                    pushed += 1;
                }
                Err(e) => {
                    self.sent.remove(location);
                    eprintln!("Foxglove: wallpaper sync: can't reach {location}: {e}");
                }
            }
        }
        pushed
    }

    /// Quitting: the monitors that were sent something are told Foxglove has gone.
    pub fn quit(&mut self) {
        let told: Vec<String> = self.sent.keys().cloned().collect();
        if told.is_empty() || !self.apply.ready() {
            return;
        }
        let props = gone();
        for location in told {
            if self.sent.get(&location).map(|(_, p)| p) != Some(&props) {
                let _ = self.apply.apply(&location, &props);
            }
        }
        self.sent.clear();
    }
}

// ------------------------------------------------------------------------- the app's worker

enum Wake {
    Changed,
    Quit(Sender<()>),
}

#[derive(Default)]
struct Shared {
    state: State,
    /// The monitors showing a Bandit wallpaper.
    targets: Vec<Target>,
    wake: Option<Sender<Wake>>,
    /// Foxglove is quitting: no worker starts again.
    quitting: bool,
    placements: u64,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
    let mut g = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Shared::default))
}

fn changed(f: impl FnOnce(&mut Shared)) {
    let wake = with(|s| {
        f(s);
        s.wake.clone()
    });
    if let Some(w) = wake {
        let _ = w.send(Wake::Changed);
    }
}

/// A scene started (`scene_run`): its id, effect, clock and room.
pub fn scene_started(id: &str, effect: &Effect, clock: Clock, room: RoomMap) {
    eprintln!(
        "Foxglove: wallpaper sync: scene {id} on epoch {} ms",
        clock.epoch_unix_ms()
    );
    changed(|s| {
        s.placements += 1;
        s.state.scene = Some(Scene {
            id: id.to_string(),
            effect: effect.clone(),
            clock,
            room,
            placement: s.placements,
        });
    });
}

/// The running scene placed its devices again.
pub fn scene_room(room: RoomMap) {
    changed(|s| {
        s.placements += 1;
        if let Some(scene) = s.state.scene.as_mut() {
            scene.room = room;
            scene.placement = s.placements;
        }
    });
}

/// The scene ended: the wallpapers go back to their own clocks.
pub fn scene_stopped() {
    changed(|s| s.state.scene = None);
}

/// The wallpapers on screen, from Wallpaper Engine's config (`wallpaper.rs`, on each change):
/// `targets` are the monitors showing a Bandit wallpaper (and its file), empty without Wallpaper
/// Engine or while it isn't running. The first non-empty list starts the worker.
pub fn wallpapers(targets: Vec<Target>) {
    let start = with(|s| s.wake.is_none() && !s.quitting && !targets.is_empty());
    if start {
        start_worker();
    }
    changed(|s| s.targets = targets);
}

/// Quitting: tells the wallpapers Foxglove has gone (bounded), so they don't keep showing a
/// layer that no longer is.
pub fn quit() {
    let Some(wake) = with(|s| {
        s.quitting = true;
        s.wake.take()
    }) else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    if wake.send(Wake::Quit(tx)).is_ok() {
        let _ = rx.recv_timeout(QUIT_LIMIT);
    }
}

fn start_worker() {
    let (tx, rx) = mpsc::channel::<Wake>();
    // Seed the layer from the facts the keyboard already reported, then follow its changes.
    let facts = super::devices::bus().facts();
    let layers = facts.subscribe("keyboard.layers");
    set_layers(match facts.get("keyboard.layers") {
        Some(foxglove_devices::facts::Value::Bytes(b)) => Some(b),
        _ => None,
    });
    with(|s| s.wake = Some(tx.clone()));
    let forward = tx.clone();
    super::services::spawn_named("wallpaper-sync-layers", move || {
        for change in layers {
            set_layers(match change.value {
                Some(foxglove_devices::facts::Value::Bytes(b)) => Some(b),
                _ => None,
            });
            if forward.send(Wake::Changed).is_err() {
                break;
            }
        }
    });
    super::services::spawn_named("wallpaper-sync", move || run(&rx, Cli::default()));
    let _ = tx.send(Wake::Changed);
}

fn set_layers(active: Option<Vec<u8>>) {
    let names = super::layer_names::remembered();
    with(|s| {
        s.state.layer = active.as_deref().and_then(|a| layer_name(a, &names));
        s.state.game = active.as_deref().is_some_and(|a| game_mode(a, &names));
    });
}

/// The worker: waits for a change, takes every change waiting (newest wins), pushes.
fn run(rx: &Receiver<Wake>, apply: impl Apply) {
    let mut pusher = Pusher::new(apply);
    let mut last_scene: Option<(String, i64)> = None;
    while let Ok(first) = rx.recv() {
        let mut quit = match first {
            Wake::Quit(ack) => Some(ack),
            Wake::Changed => None,
        };
        while let Ok(next) = rx.try_recv() {
            if let Wake::Quit(ack) = next {
                quit = Some(ack);
            }
        }
        if let Some(ack) = quit {
            pusher.quit();
            let _ = ack.send(());
            return;
        }
        let (state, targets) = with(|s| (s.state.clone(), s.targets.clone()));
        let pushed = pusher.sync(&state, &targets);
        let scene = state
            .scene
            .as_ref()
            .map(|s| (s.id.clone(), s.clock.epoch_unix_ms()));
        if pushed > 0 && scene != last_scene {
            match &scene {
                Some((id, epoch)) => eprintln!(
                    "Foxglove: wallpaper sync: scene {id} (epoch {epoch}) sent to {pushed} monitor(s)"
                ),
                None => eprintln!("Foxglove: wallpaper sync: no scene, sent to {pushed} monitor(s)"),
            }
            last_scene = scene;
        }
    }
}

/// Wallpaper Engine's command line.
#[derive(Default)]
struct Cli {
    exe: Option<PathBuf>,
}

impl Apply for Cli {
    fn ready(&mut self) -> bool {
        if !super::wallpaper_engine::running_now() {
            return false;
        }
        if self.exe.is_none() {
            self.exe = super::wallpaper_engine::cli_exe(&super::wallpaper_engine::presence());
        }
        self.exe.is_some()
    }

    fn apply(&mut self, location: &str, props: &str) -> Result<(), String> {
        // Checked again before every push: if Wallpaper Engine quit meanwhile, its command
        // line would start it.
        if !super::wallpaper_engine::running_now() {
            return Err("Wallpaper Engine isn't running".into());
        }
        let exe = self.exe.clone().ok_or("no Wallpaper Engine program")?;
        let (args, raw) = arguments(location, props);
        run_cli(&exe, &args, &raw)
    }

    fn interfaces(&mut self) -> Vec<(String, String)> {
        interfaces()
    }
}

#[cfg(windows)]
fn run_cli(exe: &std::path::Path, args: &[String], raw: &str) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{WaitForSingleObject, CREATE_NO_WINDOW};

    let mut child = std::process::Command::new(exe)
        .args(args)
        .raw_arg(raw)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Waits on the process handle (an event), bounded.
    let handle = HANDLE(child.as_raw_handle());
    // SAFETY: the child's process handle, alive while `child` is.
    let waited = unsafe { WaitForSingleObject(handle, PUSH_LIMIT.as_millis() as u32) };
    if waited != WAIT_OBJECT_0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("no answer in {} s", PUSH_LIMIT.as_secs()));
    }
    let _ = child.wait();
    Ok(())
}

#[cfg(not(windows))]
fn run_cli(_exe: &std::path::Path, _args: &[String], _raw: &str) -> Result<(), String> {
    Err("Wallpaper Engine runs on Windows only".into())
}

/// Each Windows display (`\\.\DISPLAY1`) with its monitor's interface path, the key Wallpaper
/// Engine uses in its config.
#[cfg(windows)]
fn interfaces() -> Vec<(String, String)> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
    use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;

    let text = |w: &[u16]| {
        let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..n])
    };
    super::scene_room::monitors()
        .into_iter()
        .filter_map(|m| {
            let name: Vec<u16> = m.id.encode_utf16().chain(std::iter::once(0)).collect();
            let mut dd = DISPLAY_DEVICEW {
                cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            // SAFETY: a NUL-terminated name and a sized, writable DISPLAY_DEVICEW.
            let ok = unsafe {
                EnumDisplayDevicesW(
                    PCWSTR(name.as_ptr()),
                    0,
                    &mut dd,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            };
            ok.as_bool()
                .then(|| (m.id.clone(), text(&dd.DeviceID)))
                .filter(|(_, iface)| !iface.is_empty())
        })
        .collect()
}

#[cfg(not(windows))]
fn interfaces() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_scene::{Layout, MonitorInfo};

    fn names() -> Vec<Option<String>> {
        ["QWERTY", "NUMBER", "SYMBOL", "GAME", "GAME+", "FUNCTION"]
            .iter()
            .map(|n| Some((*n).to_string()))
            .collect()
    }

    fn room() -> RoomMap {
        let m = |id: &str, x: i32, primary: bool| MonitorInfo {
            id: id.into(),
            primary,
            rect_px: [x, 0, x + 2560, 1440],
            raw_dpi: Some(109),
            dpi: 96,
        };
        RoomMap::build(
            &[m(r"\\.\DISPLAY1", 0, true), m(r"\\.\DISPLAY2", 2560, false)],
            &[],
            &Layout::default(),
        )
    }

    const WE_LEFT: &str =
        "//?/DISPLAY#SAM0E3C#5&26cfee32&1&UID8449#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
    const WE_RIGHT: &str =
        "//?/DISPLAY#DEL40EA#5&26cfee32&1&UID8451#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";

    fn ifaces() -> Vec<(String, String)> {
        vec![
            (
                r"\\.\DISPLAY1".into(),
                r"\\?\DISPLAY#SAM0E3C#5&26cfee32&1&UID8449#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
                    .into(),
            ),
            (
                r"\\.\DISPLAY2".into(),
                r"\\?\DISPLAY#DEL40EA#5&26cfee32&1&UID8451#{E6F07B5F-EE97-4A90-B076-33F57BF4EAA7}"
                    .into(),
            ),
        ]
    }

    fn scene() -> Scene {
        Scene {
            id: "demo scanline".into(),
            effect: super::super::scene_run::demo_effect(),
            clock: Clock::aligned(1_761_000_000_000),
            room: room(),
            placement: 1,
        }
    }

    /// Records every push; `running` plays Wallpaper Engine being up.
    #[derive(Default)]
    struct Fake {
        running: bool,
        pushes: Vec<(String, String)>,
        interface_reads: usize,
        fail: bool,
    }

    impl Apply for &mut Fake {
        fn ready(&mut self) -> bool {
            self.running
        }
        fn apply(&mut self, location: &str, props: &str) -> Result<(), String> {
            if self.fail {
                return Err("refused".into());
            }
            self.pushes.push((location.into(), props.into()));
            Ok(())
        }
        fn interfaces(&mut self) -> Vec<(String, String)> {
            self.interface_reads += 1;
            ifaces()
        }
    }

    #[test]
    fn layer_names_and_game_mode_come_from_the_active_layers() {
        let n = names();
        assert_eq!(layer_name(&[0], &n).as_deref(), Some("QWERTY"));
        assert_eq!(layer_name(&[0, 3, 4], &n).as_deref(), Some("GAME+"));
        assert_eq!(layer_name(&[0, 9], &n).as_deref(), Some("L9"));
        assert_eq!(layer_name(&[], &n), None);
        assert!(game_mode(&[0, 3], &n));
        assert!(game_mode(&[0, 3, 4, 5], &n));
        assert!(!game_mode(&[0, 1, 2], &n));
        assert!(!game_mode(&[3], &[]));
        let odd = vec![Some(" nav \"layer\" ".to_string())];
        assert_eq!(layer_name(&[0], &odd).as_deref(), Some("NAV_LAYER"));
    }

    #[test]
    fn the_command_line_is_compact_json_between_the_raw_markers() {
        let state = State {
            layer: Some("GAME".into()),
            game: true,
            scene: None,
        };
        let props = properties(&state, None);
        assert_eq!(
            props,
            r#"{"foxglove_game":true,"foxglove_layer":"GAME","foxglove_scene":""}"#
        );
        let (args, raw) = arguments(WE_LEFT, &props);
        assert_eq!(args, ["-control", "applyProperties", "-location", WE_LEFT]);
        assert_eq!(raw, format!("-properties RAW~({props})~END"));
        assert_eq!(raw.matches(' ').count(), 1, "one space, after -properties");
        assert_eq!(
            gone(),
            r#"{"foxglove_game":false,"foxglove_layer":"","foxglove_scene":""}"#
        );
    }

    #[test]
    fn each_monitor_gets_its_own_rectangle_in_the_room() {
        let r = room();
        let left = monitor_rect(WE_LEFT, &r, &ifaces()).unwrap();
        let right = monitor_rect(WE_RIGHT, &r, &ifaces()).unwrap();
        assert_eq!(left, r.device(r"monitor:\\.\DISPLAY1").unwrap().rect);
        assert!(right.x >= left.x + left.w - 0.5, "{left:?} {right:?}");
        assert_eq!(monitor_rect("Monitor0", &r, &ifaces()), None);
        let mut state = State {
            scene: Some(scene()),
            ..State::default()
        };
        state.layer = Some("QWERTY".into());
        let a = properties(&state, Some(left));
        let b = properties(&state, Some(right));
        assert_ne!(a, b);
        assert!(
            a.contains("fx=scanline;e=1761000000000;p=4000;a=90;w=80"),
            "{a}"
        );
        assert!(a.contains(";mon=0,0,"), "{a}");
        assert!(!a.contains(' '));
    }

    #[test]
    fn pushes_go_only_on_change_and_only_while_wallpaper_engine_runs() {
        let mut fake = Fake::default();
        let hud = "C:/WE/projects/myprojects/bandit-hud/index.html".to_string();
        let targets = vec![
            (WE_LEFT.to_string(), hud.clone()),
            (WE_RIGHT.to_string(), hud.clone()),
        ];
        let mut state = State::default();
        {
            let mut p = Pusher::new(&mut fake);
            // Not running: nothing (its command line would start it).
            assert_eq!(p.sync(&state, &targets), 0);
        }
        assert!(fake.pushes.is_empty());
        fake.running = true;
        let mut p = Pusher::new(&mut fake);
        assert_eq!(p.sync(&state, &targets), 2, "the first sync tells both");
        assert_eq!(p.sync(&state, &targets), 0, "nothing changed");
        state.layer = Some("NUMBER".into());
        assert_eq!(p.sync(&state, &targets), 2);
        assert_eq!(
            p.sync(&state, &targets[..1]),
            0,
            "a monitor going adds nothing"
        );
        state.scene = Some(scene());
        assert_eq!(p.sync(&state, &targets), 2, "a scene start");
        assert_eq!(p.sync(&state, &targets), 0);
        // Another wallpaper on one monitor starts from its defaults: it is sent again.
        let radar = "C:/WE/projects/myprojects/bandit-bandit-radar/index.html".to_string();
        let swapped = vec![targets[0].clone(), (WE_RIGHT.to_string(), radar)];
        assert_eq!(p.sync(&state, &swapped), 1, "a swapped wallpaper");
        assert_eq!(p.sync(&state, &swapped), 0);
        // The devices placed again: the interface paths are read again.
        state.scene.as_mut().unwrap().placement = 2;
        assert_eq!(p.sync(&state, &swapped), 0, "same room, nothing to send");
        state.scene = None;
        assert_eq!(p.sync(&state, &targets), 2, "a scene stop");
        assert_eq!(p.sync(&state, &[]), 0, "no Bandit wallpaper: nothing");
        p.quit();
        drop(p);
        assert_eq!(fake.pushes.len(), 9);
        assert_eq!(
            fake.interface_reads, 2,
            "interfaces read once per scene placement"
        );
        let scene_push = &fake.pushes[4].1;
        assert!(scene_push.contains("v=1;id=demo-scanline;fx=scanline"));
        assert!(fake.pushes[7].1.ends_with(r#""foxglove_scene":""}"#));
    }

    #[test]
    fn quitting_clears_what_was_sent_and_failures_are_sent_again() {
        let mut fake = Fake {
            running: true,
            ..Fake::default()
        };
        let targets = vec![(WE_LEFT.to_string(), "bandit-hud".to_string())];
        let state = State {
            layer: Some("GAME".into()),
            game: true,
            scene: Some(scene()),
        };
        {
            let mut p = Pusher::new(&mut fake);
            p.sync(&state, &targets);
            p.quit();
            p.quit();
        }
        assert_eq!(fake.pushes.len(), 2);
        assert_eq!(fake.pushes[1].1, gone());
        fake.pushes.clear();
        fake.fail = true;
        let mut p = Pusher::new(&mut fake);
        assert_eq!(p.sync(&state, &targets), 0);
        p.apply.fail = false;
        assert_eq!(
            p.sync(&state, &targets),
            1,
            "a failed push goes again on the next change"
        );
        // Wallpaper Engine quit and came back: everything goes again.
        p.apply.running = false;
        assert_eq!(p.sync(&state, &targets), 0);
        p.apply.running = true;
        assert_eq!(p.sync(&state, &targets), 1);
    }
}
