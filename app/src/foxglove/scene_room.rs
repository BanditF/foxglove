//! Scenes' room map on this PC (#483). `foxglove_scene` places everything; this gathers what it
//! places from Windows and from what Foxglove already knows:
//! - the monitors: Windows' display layout and each panel's physical DPI;
//! - the keyboard: its board profile (halves, and their size from the drawing), lit as one
//!   light per half, or one per LED once the profile's LED map is checked on the board (#529);
//! - the lighting devices the Lights page found: catalog zones with their positions and the
//!   kind the catalog gives, LampArrays with their lamps and the kind Windows gives, and
//!   devices added by hand;
//! - every other keyboard the focus knows (#801): a VIA keyboard, connected, drawn from its
//!   layout and lit by nothing (a LampArray keyboard is already there as a lighting device). The
//!   Scenes page rings the focused keyboard ([`focused_ids`]).
//!
//! The user's drags live in the store's `[scene]` section ([`foxglove_scene::Layout`]).
//!
//! Monitors are read on the first request and again on each `WM_DISPLAYCHANGE`, which Windows
//! sends to a hidden window whose thread sleeps in `GetMessageW`. Nothing polls, and nothing
//! runs before something asks for the room.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use foxglove_devices::pc_lighting::{Control, DeviceKind as LightKind, ZoneRole};
use foxglove_scene::{
    DeviceInfo, DeviceKind, Layout, LightRole, LightSpec, MonitorInfo, Point, RoomMap,
};

use super::boards::Board;
use super::key_layout::KeyLayout;
use super::palette_file::Tier;
use super::pc_lights::{LampLayout, Row};

/// The store section with the user's drags and the case's side.
pub const SECTION: &str = "scene";

/// One key unit, in millimetres.
pub(crate) const KEY_MM: f32 = 19.05;

static MONITORS: Mutex<Option<Vec<MonitorInfo>>> = Mutex::new(None);

/// Counts changes to what the room is built from (displays, devices, the user's drags), so the
/// Scenes page builds its map again only after one.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Something the room is built from changed.
pub fn changed() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// The count of changes so far; a different value means the room may have changed.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The user's layout changes, or none.
pub fn layout() -> Layout {
    super::store::section(STORE_OWNER, SECTION)
        .try_load()
        .unwrap_or_default()
}

/// The room as it is now.
pub fn room() -> RoomMap {
    let rows = super::pc_lights::snapshot().devices;
    let mut devices = devices(
        super::boards::current().as_ref(),
        &rows,
        &super::pc_lights::lamp_layouts(),
    );
    let keyboards = super::keyboard_focus::keyboards();
    devices.extend(other_keyboards(&keyboards, &rows, |k| {
        super::keyboard_focus::layout(k)
    }));
    RoomMap::build(&monitors(), &devices, &layout())
}

/// A keyboard's room id when it is drawn on its own (a VIA keyboard).
pub fn keyboard_id(key: &str) -> String {
    format!("{OTHER_KEYBOARD}{key}")
}

const OTHER_KEYBOARD: &str = "other-keyboard:";

/// The keyboards the room has no device for yet (#801): connected VIA keyboards without a
/// LampArray of their own in `rows`, each as wide as its layout and lit by nothing (the scene
/// can't light them; Foxglove only reads them).
pub fn other_keyboards(
    keyboards: &[super::keyboard_focus::Keyboard],
    rows: &[Row],
    layout: impl Fn(&super::keyboard_focus::Keyboard) -> KeyLayout,
) -> Vec<DeviceInfo> {
    use super::keyboard_focus::Source;
    keyboards
        .iter()
        .filter(|k| k.source == Source::Via && k.connected)
        .filter(|k| !rows.iter().any(|r| vid_pid(&r.key) == Some((k.vid, k.pid))))
        .map(|k| {
            let l = layout(k);
            let parts = l.parts.len().max(1) as f32;
            DeviceInfo {
                lights: Vec::new(),
                ..DeviceInfo::whole(keyboard_id(&k.key), k.name.clone(), DeviceKind::Other)
                    .with_size(Point::new(
                        (l.size.x * parts + (parts - 1.0)) * KEY_MM,
                        l.size.y * KEY_MM,
                    ))
            }
        })
        .collect()
}

/// The room ids of `focus`'s devices, for the Scenes page's ring: the host-link keyboard's
/// halves (or its one piece), a keyboard drawn on its own, and a lighting device with its
/// VID/PID (its LampArray).
pub fn focused_ids(room: &RoomMap, focus: Option<&super::keyboard_focus::Keyboard>) -> Vec<String> {
    use super::keyboard_focus::Source;
    let Some(k) = focus else {
        return Vec::new();
    };
    let own = keyboard_id(&k.key);
    room.devices
        .iter()
        .filter(|d| {
            (k.source == Source::HostLink && d.id.starts_with(super::scene_output::KEYBOARD))
                || d.id == own
                || d.id
                    .strip_prefix("light:")
                    .and_then(vid_pid)
                    .is_some_and(|ids| ids == (k.vid, k.pid))
        })
        .map(|d| d.id.clone())
        .collect()
}

/// The displays, read once and again after each display change.
pub fn monitors() -> Vec<MonitorInfo> {
    imp::watch();
    watch_away();
    MONITORS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(imp::read)
        .clone()
}

/// A display change: read the monitors again.
#[cfg_attr(not(windows), allow(dead_code))]
fn display_changed() {
    // Screen glow's capture tries again on any display change (#496); a no-op without a scene.
    super::scene_run::display_changed();
    let now = imp::read();
    let mut known = MONITORS.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref() != Some(&now) {
        let what = difference(known.as_deref().unwrap_or(&[]), &now);
        let line = QUIET.lock().unwrap_or_else(|e| e.into_inner()).change(
            super::away::away(),
            &what,
            now.len(),
        );
        if let Some(line) = line {
            eprintln!("{line}");
        }
        changed();
    }
    *known = Some(now);
}

/// Display changes while the user is away (#903): a sleeping monitor's link can drop and come
/// back every few seconds all night (12.6 changes a minute on the user's PC). The first one of
/// each away stretch is logged in full; the rest are counted and summed up in one line when the
/// user is back. The room still follows every one.
static QUIET: Mutex<Quiet> = Mutex::new(Quiet {
    changes: 0,
    left: BTreeMap::new(),
});

/// The away subscription that sums the stretch up on return.
static AWAY: std::sync::OnceLock<super::away::Subscription> = std::sync::OnceLock::new();

fn watch_away() {
    AWAY.get_or_init(|| {
        super::away::subscribe(|away| {
            if away {
                return;
            }
            let line = QUIET.lock().unwrap_or_else(|e| e.into_inner()).back();
            if let Some(line) = line {
                eprintln!("{line}");
            }
        })
    });
}

/// The display changes of one away stretch.
#[derive(Debug, Default, PartialEq, Eq)]
struct Quiet {
    /// Changes seen while away.
    changes: u32,
    /// Times each monitor left while away, by its id.
    left: BTreeMap<String, u32>,
}

/// What one display change did: monitors that left, that arrived, and whether one moved or
/// changed size.
#[derive(Debug, Default, PartialEq, Eq)]
struct Difference {
    left: Vec<String>,
    arrived: Vec<String>,
}

impl Difference {
    fn words(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.left.is_empty() {
            parts.push(format!("{} left", self.left.join(", ")));
        }
        if !self.arrived.is_empty() {
            parts.push(format!("{} arrived", self.arrived.join(", ")));
        }
        if parts.is_empty() {
            "a monitor moved or changed size".into()
        } else {
            parts.join("; ")
        }
    }
}

/// A monitor in the log's words: Windows' name and its size in pixels.
fn monitor_words(m: &MonitorInfo) -> String {
    let [l, t, r, b] = m.rect_px;
    format!("{} ({}x{})", m.id, r - l, b - t)
}

fn difference(before: &[MonitorInfo], now: &[MonitorInfo]) -> Difference {
    let gone = |a: &MonitorInfo, list: &[MonitorInfo]| !list.iter().any(|m| m.id == a.id);
    Difference {
        left: before
            .iter()
            .filter(|m| gone(m, now))
            .map(monitor_words)
            .collect(),
        arrived: now
            .iter()
            .filter(|m| gone(m, before))
            .map(monitor_words)
            .collect(),
    }
}

impl Quiet {
    /// A display change (`away` the user is away now, `count` the monitors now): the line to log,
    /// if any.
    fn change(&mut self, away: bool, what: &Difference, count: usize) -> Option<String> {
        let line = format!(
            "Foxglove: displays changed, the room map has {count} monitor(s): {}",
            what.words()
        );
        if !away {
            return Some(line);
        }
        self.changes += 1;
        for m in &what.left {
            *self.left.entry(m.clone()).or_default() += 1;
        }
        (self.changes == 1)
            .then(|| format!("{line} (while away; more are counted, one line on return)"))
    }

    /// The user is back: the stretch's summary when more than the first change came.
    fn back(&mut self) -> Option<String> {
        let q = std::mem::take(self);
        if q.changes <= 1 {
            return None;
        }
        let left: Vec<String> = q
            .left
            .iter()
            .map(|(m, n)| format!("{m} left {n} time(s)"))
            .collect();
        let detail = if left.is_empty() {
            String::new()
        } else {
            format!(" ({})", left.join(", "))
        };
        Some(format!(
            "Foxglove: displays changed {} times while away{detail}",
            q.changes
        ))
    }
}

/// Everything to place but the monitors.
pub fn devices(
    board: Option<&Board>,
    rows: &[Row],
    lamps: &BTreeMap<(u16, u16), LampLayout>,
) -> Vec<DeviceInfo> {
    let mut out = keyboard(board);
    for row in rows {
        let ids = vid_pid(&row.key);
        let device = match row.control {
            Control::NotYet => continue,
            Control::Driver(_) | Control::OpenRgb => zones(row),
            Control::LampArray => lamp_array(row, ids.and_then(|ids| lamps.get(&ids))),
        };
        out.push(device);
    }
    out
}

/// The keyboard: both halves of a split, or the one piece, sized from the keyboard's layout as
/// the Keyboard page draws it (#559, #796: the keyboard's own, else its profile's, else a plain
/// grid of its keymap's keys).
fn keyboard(board: Option<&Board>) -> Vec<DeviceInfo> {
    let name = board.map_or("Keyboard", |b| b.name.as_str());
    let drawing = super::key_layout::of(board);
    let half = Point::new(drawing.size.x * KEY_MM, drawing.size.y * KEY_MM);
    if !super::parts::Parts::of(board).is_split() {
        // A one-piece board's drawing is the whole board (#559).
        let mut one = DeviceInfo::whole("keyboard", name, DeviceKind::Keyboard).with_size(half);
        if let Some(lights) = leds(board, &drawing, 0) {
            one.lights = lights;
        }
        return vec![one];
    }
    let mut halves = vec![
        DeviceInfo::whole(
            "keyboard:left",
            format!("{name} left"),
            DeviceKind::KeyboardLeft,
        )
        .with_size(half),
        DeviceInfo::whole(
            "keyboard:right",
            format!("{name} right"),
            DeviceKind::KeyboardRight,
        )
        .with_size(half),
    ];
    for (i, d) in halves.iter_mut().enumerate() {
        if let Some(lights) = leds(board, &drawing, i) {
            d.lights = lights;
        }
    }
    halves
}

/// The keyboard's LEDs that a harmony palette lights in another accent (#871): `tone_key` of
/// each LED (`keyboard:left/3`) to its tone, 1 the second accent, 2 the third, from the board
/// profile's `accent2_keys` / `accent3_keys` and its LED map. Only LEDs off the first accent
/// are in it; a board without a usable map, or whose LEDs each light keys of mixed roles (the
/// Eyelash Corne's columns), gives an empty map.
pub fn keyboard_tones(tier: Tier) -> BTreeMap<String, u8> {
    tones(super::boards::current().as_ref(), tier)
}

/// [`keyboard_tones`] for `board`. A bold-tier palette takes the profile's `bold_accent*_keys`
/// where it has them.
pub fn tones(board: Option<&Board>, tier: Tier) -> BTreeMap<String, u8> {
    let mut out = BTreeMap::new();
    let Some(b) = board else { return out };
    let Some(l) = b.lighting.as_ref() else {
        return out;
    };
    let pick = |bold: &[u8], plain: &'_ [u8]| -> Vec<u8> {
        if tier == Tier::Bold && !bold.is_empty() {
            bold.to_vec()
        } else {
            plain.to_vec()
        }
    };
    let accent2 = pick(&l.bold_accent2_keys, &l.accent2_keys);
    let accent3 = pick(&l.bold_accent3_keys, &l.accent3_keys);
    if !l.led_map_usable() || (accent2.is_empty() && accent3.is_empty()) {
        return out;
    }
    let Some(parts) = l.lamp_parts(b.halves) else {
        return out;
    };
    let ids: &[&str] = if super::parts::Parts::of(board).is_split() {
        &["keyboard:left", "keyboard:right"]
    } else {
        &["keyboard"]
    };
    for (part, id) in ids.iter().enumerate() {
        let Some(lamps) = l.part_keys(&parts, part) else {
            continue;
        };
        for (i, lamp) in lamps.iter().enumerate() {
            let tone = lamp_tone(&lamp.0, &accent2, &accent3);
            if tone != 0 {
                out.insert(foxglove_scene::tone_key(id, &i.to_string()), tone);
            }
        }
    }
    out
}

/// One LED's tone from the keys it lights: 2 when most of them are third-accent keys, 1 when
/// most are second-accent keys, else 0. An LED of one key is that key's.
pub fn lamp_tone(keys: &[u8], accent2: &[u8], accent3: &[u8]) -> u8 {
    let most = |set: &[u8]| {
        !keys.is_empty() && keys.iter().filter(|k| set.contains(k)).count() * 2 > keys.len()
    };
    if most(accent3) {
        2
    } else if most(accent2) {
        1
    } else {
        0
    }
}

/// One part's LEDs at their keys (part 0 the left half or the one piece, 1 the right half),
/// numbered in its strip's order (the scene output's SET_FRAME lamps of that part), from the
/// board profile's LED map once it is checked on the board (#90 row G) or shipped with the
/// profile. As many as the profile gives the part (#561), less the ones that light nothing: an
/// LED that drives a group of keys sits at their centre, and a dead one has no light, so the
/// frame leaves it dark (#608). `None` (one light for the part) without such a map, or when it
/// names a key the drawing hasn't.
fn leds(board: Option<&Board>, drawing: &KeyLayout, part: usize) -> Option<Vec<LightSpec>> {
    let b = board?;
    let l = b.lighting.as_ref()?;
    if !l.led_map_usable() {
        return None;
    }
    let parts = l.lamp_parts(b.halves)?;
    if !l.led_points.is_empty() {
        return found_leds(l, &parts, drawing, part);
    }
    let lamps = l
        .part_keys(&parts, part)
        .filter(|k| k.iter().any(|lamp| !lamp.dead()))?;
    let keys = drawing.rects(part);
    lamps
        .iter()
        .enumerate()
        .filter(|(_, lamp)| !lamp.dead())
        .map(|(i, lamp)| {
            let c = lamp.center(&keys)?;
            Some(LightSpec::point(
                i.to_string(),
                Some(Point::new(c.x * KEY_MM, c.y * KEY_MM)),
            ))
        })
        .collect()
}

/// One part's LEDs where "Find my LEDs" saw them (points on the part's profile drawing, key
/// units, moved onto the keyboard's layout, #796), numbered in strip order. One the user couldn't see sits in the part's centre, so it takes
/// the colour of the middle of the part rather than a random spot, unless the profile's map
/// says it lights nothing: that one has no light (#608).
fn found_leds(
    l: &super::boards::Lighting,
    parts: &foxglove_link::lamps::LampParts,
    drawing: &KeyLayout,
    part: usize,
) -> Option<Vec<LightSpec>> {
    let points = l.led_points.get(parts.range(part))?;
    let dead = |i: usize| {
        l.part_keys(parts, part)
            .and_then(|lamps| lamps.get(i))
            .is_some_and(super::boards::Lamp::dead)
    };
    let (w, h) = (drawing.size.x, drawing.size.y);
    Some(
        points
            .iter()
            .enumerate()
            .filter(|&(i, p)| !(p.is_none() && dead(i)))
            .map(|(i, p)| {
                let [x, y] = p.map_or([w / 2.0, h / 2.0], |p| drawing.point_from_profile(part, p));
                LightSpec::point(i.to_string(), Some(Point::new(x * KEY_MM, y * KEY_MM)))
            })
            .collect(),
    )
}

/// The `vvvv:pppp` a lighting device's key starts with.
pub(super) fn vid_pid(key: &str) -> Option<(u16, u16)> {
    let mut parts = key.split(':');
    let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vid, pid))
}

fn size(s: Option<[u16; 2]>) -> Option<Point> {
    s.map(|[w, h]| Point::new(f32::from(w), f32::from(h)))
}

/// A catalog board: one light per zone, where the catalog puts it, in the role it gives (a
/// case's interior light is ambient), of the kind the catalog says (not its driver's).
fn zones(row: &Row) -> DeviceInfo {
    let kind = match row.kind {
        LightKind::Case => DeviceKind::Case,
        LightKind::Cooler | LightKind::Strip | LightKind::Peripheral | LightKind::Other => {
            DeviceKind::Other
        }
    };
    let lights = row
        .zone_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let role = match row.zone_roles.get(i) {
                Some(ZoneRole::Ambient) => LightRole::Ambient,
                _ => LightRole::Point,
            };
            LightSpec::point(
                id.clone(),
                size(row.zone_positions.get(i).copied().flatten()),
            )
            .with_role(role)
        })
        .collect();
    DeviceInfo {
        id: format!("light:{}", row.key),
        name: row.name.clone(),
        kind,
        size: size(row.size),
        lights,
    }
}

/// A LampArray: one light per lamp, at its position, its kind from Windows. Until its
/// attributes are read it is one light.
fn lamp_array(row: &Row, layout: Option<&LampLayout>) -> DeviceInfo {
    let id = format!("light:{}", row.key);
    let Some(layout) = layout.filter(|l| !l.lamps.is_empty()) else {
        return DeviceInfo::whole(id, row.name.clone(), DeviceKind::Other);
    };
    let kind = lamp_array_kind(layout.kind);
    let positioned = layout.lamps.iter().any(|&[x, y]| x != 0.0 || y != 0.0);
    let [w, h] = layout.size_mm;
    DeviceInfo {
        id,
        name: row.name.clone(),
        kind,
        size: (w > 0.0 && h > 0.0).then_some(Point::new(w, h)),
        lights: layout
            .lamps
            .iter()
            .enumerate()
            .map(|(i, &[x, y])| {
                LightSpec::point(i.to_string(), positioned.then_some(Point::new(x, y)))
            })
            .collect(),
    }
}

/// Where a LampArray goes in the room, by the kind it tells Windows (`LampArrayKind`), never
/// by vendor or model: a mouse beside the keyboard, a chassis beside the monitors, anything
/// else (a strip, a headset, a second keyboard, a speaker) with the other devices.
fn lamp_array_kind(kind: u32) -> DeviceKind {
    const MOUSE: u32 = 2;
    const CHASSIS: u32 = 7;
    match kind {
        MOUSE => DeviceKind::Mouse,
        CHASSIS => DeviceKind::Case,
        _ => DeviceKind::Other,
    }
}

#[cfg(windows)]
mod imp {
    use foxglove_scene::MonitorInfo;
    use std::sync::Once;
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI, MDT_RAW_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, MSG,
        WINDOW_STYLE, WM_DISPLAYCHANGE, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
    };

    const MONITORINFOF_PRIMARY: u32 = 1;

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the Vec `read` passed, alive for the whole enumeration.
        let out = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `monitor` comes from the enumeration; `info` is ours and sized above.
        let ok = unsafe {
            GetMonitorInfoW(
                monitor,
                &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
        };
        if ok.as_bool() {
            let len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let r = info.monitorInfo.rcMonitor;
            out.push(MonitorInfo {
                id: String::from_utf16_lossy(&info.szDevice[..len]),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                rect_px: [r.left, r.top, r.right, r.bottom],
                raw_dpi: dpi(monitor, true),
                dpi: dpi(monitor, false).unwrap_or(96),
            });
        }
        BOOL(1)
    }

    /// The raw DPI (the panel's own density, from its EDID) or the effective one (Windows'
    /// scaling). The process is per-monitor DPI aware (winit), so both are the display's own.
    fn dpi(monitor: HMONITOR, raw: bool) -> Option<u32> {
        let kind = if raw { MDT_RAW_DPI } else { MDT_EFFECTIVE_DPI };
        let (mut x, mut y) = (0u32, 0u32);
        // SAFETY: `monitor` comes from the enumeration; the outputs are ours.
        match unsafe { GetDpiForMonitor(monitor, kind, &mut x, &mut y) } {
            Ok(()) if x > 0 => Some(x),
            _ => None,
        }
    }

    pub fn read() -> Vec<MonitorInfo> {
        let mut out: Vec<MonitorInfo> = Vec::new();
        // SAFETY: the callback only writes into `out`, which outlives the call.
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&mut out as *mut Vec<MonitorInfo> as isize),
            );
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DISPLAYCHANGE {
            // A panic must not unwind into Windows.
            let _ = std::panic::catch_unwind(super::display_changed);
            return LRESULT(0);
        }
        if message == WM_SETTINGCHANGE {
            // Screen glow reads an HDR screen's SDR white level again (#1085). Only a count:
            // nothing runs here.
            super::super::screen_capture::settings_changed();
        }
        // SAFETY: the default handling for this window's other messages.
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    /// Follows display changes from the first call on. `WM_DISPLAYCHANGE` goes to top-level
    /// windows only (a message-only one gets no broadcasts), so this is a top-level window
    /// that is never shown.
    pub fn watch() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            super::super::services::spawn_named("scene-displays", || {
                let class: Vec<u16> = "FoxgloveSceneDisplays\0".encode_utf16().collect();
                // SAFETY: plain window-class and window creation on this thread; the class
                // name outlives both calls, and the window lives as long as this thread's
                // message loop. The window is never shown.
                unsafe {
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(window_proc),
                        lpszClassName: PCWSTR(class.as_ptr()),
                        ..Default::default()
                    };
                    if RegisterClassW(&wc) == 0 {
                        eprintln!("Foxglove: can't follow display changes (no window class)");
                        return;
                    }
                    if CreateWindowExW(
                        WS_EX_TOOLWINDOW,
                        PCWSTR(class.as_ptr()),
                        PCWSTR::null(),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        None,
                        None,
                    )
                    .is_err()
                    {
                        eprintln!("Foxglove: can't follow display changes (no window)");
                        return;
                    }
                    // A change between the first read and the window's creation is caught here.
                    super::display_changed();
                    let mut msg = MSG::default();
                    // 0 is WM_QUIT and -1 an error: both end the loop.
                    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                        let _ = DispatchMessageW(&msg);
                    }
                }
            });
        });
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn read() -> Vec<foxglove_scene::MonitorInfo> {
        Vec::new()
    }

    pub fn watch() {}
}

/// The feature that owns this file's store sections.
const STORE_OWNER: &str = "hideout.scenes";

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn monitor(id: &str, w: i32) -> MonitorInfo {
        MonitorInfo {
            id: id.into(),
            primary: false,
            rect_px: [0, 0, w, w * 9 / 16],
            raw_dpi: None,
            dpi: 96,
        }
    }

    /// #903: a monitor dropping out every few seconds while away logs once, then one summary.
    #[test]
    fn a_flap_while_away_is_one_line_then_a_summary() {
        let both = [
            monitor(r"\\.\DISPLAY1", 2560),
            monitor(r"\\.\DISPLAY2", 3840),
        ];
        let one = [both[0].clone()];
        let mut q = Quiet::default();
        let first = q
            .change(true, &difference(&both, &one), 1)
            .expect("the first is logged");
        assert!(first.contains("DISPLAY2 (3840x2160) left"), "{first}");
        assert!(first.contains("while away"), "{first}");
        assert_eq!(q.change(true, &difference(&one, &both), 2), None);
        for _ in 0..188 {
            assert_eq!(q.change(true, &difference(&both, &one), 1), None);
            assert_eq!(q.change(true, &difference(&one, &both), 2), None);
        }
        let summary = q.back().expect("a summary on return");
        assert!(summary.contains("378 times while away"), "{summary}");
        assert!(
            summary.contains("DISPLAY2 (3840x2160) left 189 time(s)"),
            "{summary}"
        );
        assert_eq!(q, Quiet::default(), "the next stretch starts afresh");
        assert_eq!(q.back(), None, "nothing to sum up");
    }

    #[test]
    fn present_changes_are_each_logged_and_one_away_change_needs_no_summary() {
        let a = [monitor("A", 2560)];
        let b = [monitor("A", 2560), monitor("B", 1920)];
        let mut q = Quiet::default();
        let line = q.change(false, &difference(&a, &b), 2).expect("logged");
        assert!(line.ends_with("B (1920x1080) arrived"), "{line}");
        assert!(q.change(false, &difference(&b, &a), 1).is_some());
        assert_eq!(q.changes, 0);
        assert!(q.change(true, &difference(&a, &b), 2).is_some());
        assert_eq!(q.back(), None);
        let moved = [monitor("A", 1920)];
        assert_eq!(
            difference(&a, &moved).words(),
            "a monitor moved or changed size"
        );
    }
    use crate::foxglove::fixtures::{second_desk, users_desk};

    /// The user's room for the Scenes page's previews: a 27" 1440p monitor and a 1080p one to
    /// its left, the Corne, the case board and the mouse.
    pub(crate) fn sample_room() -> RoomMap {
        let rows = [
            users_desk::catalog_row(users_desk::CASE),
            users_desk::mouse_row(),
        ];
        let board = users_desk::board();
        RoomMap::build(
            &users_desk::monitors(),
            &devices(Some(&board), &rows, &users_desk::lamps()),
            &Layout::default(),
        )
    }

    /// No displays read and no lighting devices: the keyboard alone.
    pub(crate) fn sample_room_empty() -> RoomMap {
        RoomMap::build(
            &[],
            &devices(None, &[], &BTreeMap::new()),
            &Layout::default(),
        )
    }

    /// The case's room id in [`sample_room`].
    pub(crate) const SAMPLE_CASE: &str = "light:103c:84fd";

    /// The Corne over the host link and a K5 Pro over VIA, as C1's model lists them.
    pub(crate) fn two_keyboards() -> Vec<super::super::keyboard_focus::Keyboard> {
        use super::super::keyboard_focus::{Keyboard, Source};
        vec![
            Keyboard {
                key: "zmk-1d50:615e".into(),
                name: "Eyelash Corne".into(),
                source: Source::HostLink,
                vid: 0x1d50,
                pid: 0x615e,
                device: None,
                connected: true,
            },
            Keyboard {
                key: "via-k5".into(),
                name: "K5 Pro".into(),
                source: Source::Via,
                vid: 0x3434,
                pid: 0x0250,
                device: None,
                connected: true,
            },
        ]
    }

    /// [`sample_room`] with the K5 Pro beside the Corne (#801): two keyboards.
    pub(crate) fn sample_room_two_keyboards() -> RoomMap {
        let rows = [
            users_desk::catalog_row(users_desk::CASE),
            users_desk::mouse_row(),
        ];
        let board = users_desk::board();
        let mut all = devices(Some(&board), &rows, &users_desk::lamps());
        // No VIA definition drawn yet (B2): the K5 Pro's keys are B1's guess from its keymap.
        all.extend(other_keyboards(&two_keyboards(), &rows, |_| {
            crate::foxglove::key_layout::resolve(None, &[], 104)
        }));
        RoomMap::build(&users_desk::monitors(), &all, &Layout::default())
    }

    /// #801: every keyboard is in the room, and the ring goes on the focused one's devices.
    #[test]
    fn the_room_has_every_keyboard_and_rings_the_focused_one() {
        let room = sample_room_two_keyboards();
        let [corne, k5] = two_keyboards().try_into().unwrap();
        let k5_id = keyboard_id(&k5.key);
        let placed = room.device(&k5_id).expect("the K5 Pro is in the room");
        assert_eq!(placed.name, "K5 Pro");
        assert!(placed.lights.is_empty(), "shown only: no scene lights it");
        assert!(placed.rect.w > 200.0, "as wide as its keys");
        assert_eq!(
            focused_ids(&room, Some(&corne)),
            ["keyboard:left", "keyboard:right"]
        );
        assert_eq!(focused_ids(&room, Some(&k5)), std::slice::from_ref(&k5_id));
        assert!(focused_ids(&room, None).is_empty());

        // A gone VIA keyboard isn't drawn; one with its own LampArray is that lighting device.
        let gone = super::super::keyboard_focus::Keyboard {
            connected: false,
            ..k5.clone()
        };
        let guess = |_: &super::super::keyboard_focus::Keyboard| {
            crate::foxglove::key_layout::resolve(None, &[], 104)
        };
        assert!(other_keyboards(&[gone], &[], guess).is_empty());
        let mut lamp = users_desk::mouse_row();
        lamp.key = "3434:0250".into();
        assert!(other_keyboards(std::slice::from_ref(&k5), &[lamp], guess).is_empty());
        assert_eq!(
            other_keyboards(&[corne], &[], guess),
            [],
            "the host link is \"keyboard\""
        );
    }

    /// The second desk's devices: Plank 3, the other vendor's case and the lightbar.
    fn second_desk_devices() -> Vec<DeviceInfo> {
        let rows = [second_desk::case_row(), second_desk::lightbar_row()];
        devices(Some(&second_desk::board()), &rows, &second_desk::lamps())
    }

    #[test]
    fn the_second_desks_devices_come_in_from_what_foxglove_knows() {
        let devices = second_desk_devices();
        let ids: Vec<_> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            ["keyboard", second_desk::CASE_ID, second_desk::LIGHTBAR_ID],
            "a one-piece board is one device"
        );
        assert_eq!(devices[0].kind, DeviceKind::Keyboard);
        // The case's zones carry its catalog's positions and roles.
        let case = &devices[1];
        assert_eq!(case.size, Some(Point::new(230.0, 500.0)));
        let roles: Vec<_> = case
            .lights
            .iter()
            .map(|l| (l.id.as_str(), l.role, l.at.is_some()))
            .collect();
        assert_eq!(
            roles,
            [
                ("1", LightRole::Point, true),
                ("2", LightRole::Point, true),
                ("3", LightRole::Point, true),
                ("4", LightRole::Ambient, true),
            ]
        );
        // The lightbar: every lamp where Windows says, and not taken for a mouse.
        let bar = &devices[2];
        assert_eq!(bar.kind, DeviceKind::Other);
        assert_eq!(bar.lights.len(), second_desk::LIGHTBAR_LAMPS);
        assert_eq!(bar.size, Some(Point::new(400.0, 24.0)));
        assert!(bar.lights.iter().all(|l| l.at.is_some_and(|p| p.y == 12.0)));
        // Both monitors of different DPI, and every device, are in the room.
        let room = RoomMap::build(&second_desk::monitors(), &devices, &Layout::default());
        let monitors: Vec<_> = room
            .devices
            .iter()
            .filter(|d| d.kind == DeviceKind::Monitor)
            .collect();
        assert_eq!(monitors.len(), 2);
        // Physical sizes from the raw DPI: the 4K panel is wider than the 1080p one.
        assert!(
            monitors[0].rect.w > monitors[1].rect.w,
            "{}",
            room.summary()
        );
        assert_eq!(room.devices.len(), 5);
        assert!(room.device(second_desk::LIGHTBAR_ID).is_some());
    }

    #[test]
    fn the_second_desks_case_is_a_case_by_its_catalog_entry() {
        let devices = second_desk_devices();
        let case = devices
            .iter()
            .find(|d| d.id == second_desk::CASE_ID)
            .unwrap();
        assert_eq!(case.kind, DeviceKind::Case, "the catalog says what it is");
    }

    #[test]
    fn a_one_piece_board_is_as_wide_as_its_whole_drawing() {
        let board = second_desk::board();
        let (w, h) = board.drawing.as_ref().unwrap().size();
        let devices = second_desk_devices();
        let size = devices[0].size.unwrap();
        assert!((size.x - w * KEY_MM).abs() < 0.5, "{size:?} for {w} keys");
        assert!((size.y - h * KEY_MM).abs() < 0.5, "{size:?}");
    }

    #[test]
    fn a_one_piece_board_lights_each_of_its_leds_at_its_key() {
        let devices = second_desk_devices();
        let board = &devices[0];
        assert_eq!(board.lights.len(), second_desk::LAMPS);
        assert_eq!(board.lights[60].id, "60");
        let size = board.size.unwrap();
        assert!(board.lights.iter().all(|l| l
            .at
            .is_some_and(|p| p.x > 0.0 && p.x < size.x && p.y > 0.0 && p.y < size.y)));
    }

    #[test]
    fn the_users_devices_come_in_from_what_foxglove_knows() {
        let rows = [
            users_desk::catalog_row(users_desk::CASE),
            users_desk::catalog_row(users_desk::COOLER),
            users_desk::mouse_row(),
        ];
        let mut lamps = BTreeMap::new();
        lamps.insert(
            (0x046d, 0xc332),
            LampLayout {
                kind: 2,
                size_mm: [68.0, 132.0],
                lamps: vec![[34.0, 40.0], [34.0, 100.0]],
            },
        );
        let board = users_desk::board();
        let devices = devices(Some(&board), &rows, &lamps);
        let kinds: Vec<_> = devices.iter().map(|d| (d.id.as_str(), d.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("keyboard:left", DeviceKind::KeyboardLeft),
                ("keyboard:right", DeviceKind::KeyboardRight),
                ("light:103c:84fd", DeviceKind::Case),
                ("light:046d:c332", DeviceKind::Mouse),
            ],
            "the cooler has no lights Foxglove can drive"
        );
        // The Corne's half from its drawing: 6.5 x 4.75 key units.
        let half = devices[0].size.unwrap();
        assert!((half.x - 6.5 * KEY_MM).abs() < 0.5, "{half:?}");
        // The case's zones carry the catalog's positions.
        let case = &devices[2];
        assert_eq!(case.size, Some(Point::new(205.0, 475.0)));
        assert_eq!(case.lights.len(), 5);
        assert!(case.lights.iter().all(|l| l.at.is_some()));
        // Its interior light (zone 2) is an area light; the fans and the diamond are points.
        let roles: Vec<_> = case
            .lights
            .iter()
            .map(|l| (l.id.as_str(), l.role))
            .collect();
        assert_eq!(
            roles,
            [
                ("1", LightRole::Point),
                ("2", LightRole::Ambient),
                ("5", LightRole::Point),
                ("6", LightRole::Point),
                ("7", LightRole::Point),
            ]
        );
        // The mouse's lamps where Windows says.
        assert_eq!(devices[3].lights[1].at, Some(Point::new(34.0, 100.0)));

        let monitors = [MonitorInfo {
            id: r"\\.\DISPLAY1".into(),
            primary: true,
            rect_px: [0, 0, 2560, 1440],
            raw_dpi: Some(109),
            dpi: 96,
        }];
        let room = RoomMap::build(&monitors, &devices, &Layout::default());
        assert_eq!(room.devices.len(), 5);
        assert!(
            room.summary().contains("mouse 68x132"),
            "{}",
            room.summary()
        );
    }

    /// Any Dynamic Lighting device is placed by its own kind and lamps, whoever made it.
    #[test]
    fn any_lamparray_is_placed_by_its_kind_not_its_vendor() {
        let row = |key: &str, name: &str| Row {
            key: key.into(),
            name: name.into(),
            ..users_desk::mouse_row()
        };
        let rows = [
            row("2a5f:0101", "Desk headset"),
            row("1209:a0a0", "Some mouse"),
            row("1209:b0b0", "Unread strip"),
        ];
        let mut lamps = BTreeMap::new();
        // A headset (kind 11) with 8 lamps round its cups.
        lamps.insert(
            (0x2a5f, 0x0101),
            LampLayout {
                kind: 11,
                size_mm: [180.0, 200.0],
                lamps: (0..8).map(|i| [10.0 + 20.0 * i as f32, 100.0]).collect(),
            },
        );
        // A mouse from another maker: the mouse's place, as any mouse.
        lamps.insert(
            (0x1209, 0xa0a0),
            LampLayout {
                kind: 2,
                size_mm: [60.0, 120.0],
                lamps: vec![[30.0, 20.0]],
            },
        );
        let devices = devices(None, &rows, &lamps);
        let by_id = |id: &str| devices.iter().find(|d| d.id == id).unwrap();
        let headset = by_id("light:2a5f:0101");
        assert_eq!(headset.kind, DeviceKind::Other);
        assert_eq!(headset.lights.len(), 8);
        assert_eq!(headset.lights[7].at, Some(Point::new(150.0, 100.0)));
        assert_eq!(headset.size, Some(Point::new(180.0, 200.0)));
        assert_eq!(by_id("light:1209:a0a0").kind, DeviceKind::Mouse);
        // Not read yet: one light, still in the room.
        assert_eq!(by_id("light:1209:b0b0").lights.len(), 1);
        let room = RoomMap::build(&[], &devices, &Layout::default());
        for id in ["light:2a5f:0101", "light:1209:a0a0", "light:1209:b0b0"] {
            assert!(room.device(id).is_some(), "{id} placed");
        }
    }

    /// crkbd's per-key chain, the Corne's map before #608: an old one-key-per-LED profile.
    const PER_KEY: [u8; 21] = [
        20, 17, 16, 15, 12, 13, 14, 19, 18, 11, 10, 9, 6, 7, 8, 5, 4, 3, 0, 1, 2,
    ];

    /// #608: the Corne's LEDs are column groups. Each half has one light per live pixel, at
    /// its group's centre, and none for the pixels that light nothing; a frame keeps those
    /// dark, and a found map's unseen dead pixels stay out too.
    #[test]
    fn grouped_leds_sit_at_their_keys_centre_and_dead_ones_get_no_light() {
        let board = users_desk::board();
        let halves = keyboard(Some(&board));
        let ids: Vec<&str> = halves[0].lights.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(
            ids,
            ["0", "1", "2", "3", "4", "5"],
            "pixels 6-20 light nothing"
        );
        assert_eq!(halves[1].lights.len(), 6);
        // Pixel 0: T, G, B and Space, at their centre.
        let drawing = super::super::boards::drawing(Some(&board));
        let keys = drawing.keys(false);
        let centre = [15, 16, 17, 20]
            .iter()
            .fold(egui::Vec2::ZERO, |s, &k| s + keys[k].center().to_vec2())
            / 4.0;
        let at = halves[0].lights[0].at.unwrap();
        assert!((at.x - centre.x * KEY_MM).abs() < 0.01 && (at.y - centre.y * KEY_MM).abs() < 0.01);
        // A frame: the live pixels get their light's colour, the dead ones black (not the mix).
        let parts = super::super::boards::lamp_parts(Some(&board));
        let red = foxglove_scene::Rgb([255, 0, 0]);
        let lit: Vec<(&str, &str, foxglove_scene::Rgb)> = halves
            .iter()
            .flat_map(|d| {
                d.lights
                    .iter()
                    .map(move |l| (d.id.as_str(), l.id.as_str(), red))
            })
            .collect();
        let frame = super::super::scene_output::keyboard_frame(&lit, &parts).unwrap();
        let black = foxglove_scene::Rgb([0, 0, 0]);
        for half in 0..2 {
            let lamps = &frame.lamps[parts.range(half)];
            assert!(lamps[..6].iter().all(|c| *c == red), "half {half}");
            assert!(
                lamps[6..].iter().all(|c| *c == black),
                "half {half}: no dead pixel lit"
            );
        }
        // "Find my LEDs" left pixels 6-20 unseen: still no light for them.
        let mut found = board.clone();
        let l = found.lighting.as_mut().unwrap();
        l.led_points = (0..42)
            .map(|i| (i % 21 < 6).then_some([1.0, 1.0]))
            .collect();
        assert!(keyboard(Some(&found)).iter().all(|d| d.lights.len() == 6));
    }

    /// #871: the thumbs and homing keys take the palette's other accents on a per-key board;
    /// the Corne's column pixels keep the first accent (each lights three alphas and a thumb).
    #[test]
    fn harmony_tones_follow_the_profile_and_need_per_key_leds() {
        let corne = users_desk::board();
        assert!(
            tones(Some(&corne), Tier::Harmony).is_empty(),
            "column pixels stay orange"
        );
        // The bold tier: the outer column has its own pixel (5), so it shows on the Corne.
        let bold = tones(Some(&corne), Tier::Bold);
        assert_eq!(
            bold.into_iter().collect::<Vec<_>>(),
            [
                (foxglove_scene::tone_key("keyboard:left", "5"), 1),
                (foxglove_scene::tone_key("keyboard:right", "5"), 1)
            ]
        );
        let mut board = corne.clone();
        board.lighting.as_mut().unwrap().led_keys = super::super::boards::Lamp::chain(&PER_KEY);
        let t = tones(Some(&board), Tier::Harmony);
        // PER_KEY: LEDs 0, 7 and 8 sit under the thumbs (20, 19, 18), LED 5 under F (13).
        for half in ["keyboard:left", "keyboard:right"] {
            for led in ["0", "7", "8"] {
                assert_eq!(
                    t.get(&foxglove_scene::tone_key(half, led)),
                    Some(&1),
                    "{half} {led}"
                );
            }
            assert_eq!(
                t.get(&foxglove_scene::tone_key(half, "5")),
                Some(&2),
                "{half}"
            );
        }
        assert_eq!(t.len(), 8, "{t:?}");
        // The ids are the room's.
        let ids: Vec<String> = keyboard(Some(&board))
            .iter()
            .flat_map(|d| {
                d.lights
                    .iter()
                    .map(|l| foxglove_scene::tone_key(&d.id, &l.id))
            })
            .collect();
        assert!(t.keys().all(|k| ids.contains(k)));
        // A provisional map, or a profile without accent keys: nothing.
        board.lighting.as_mut().unwrap().led_map = "provisional".into();
        assert!(tones(Some(&board), Tier::Bold).is_empty());
        assert!(tones(None, Tier::Harmony).is_empty());
        assert_eq!(lamp_tone(&[15, 16, 17, 20], &[18, 19, 20], &[13]), 0);
        assert_eq!(lamp_tone(&[19, 20], &[18, 19, 20], &[13]), 1);
        assert_eq!(lamp_tone(&[], &[18], &[13]), 0);
    }

    #[test]
    fn the_keyboard_is_one_light_per_half_until_it_has_a_usable_led_map() {
        let mut board = users_desk::board();
        board.lighting.as_mut().unwrap().led_keys = super::super::boards::Lamp::chain(&PER_KEY);
        // A provisional map isn't used.
        board.lighting.as_mut().unwrap().led_map = "provisional".into();
        let halves = keyboard(Some(&board));
        assert!(
            halves.iter().all(|d| d.lights.len() == 1),
            "the provisional map isn't used"
        );
        // The shipped map is (the dot test confirms it later); so is a checked one.
        let lighting = board.lighting.as_mut().unwrap();
        lighting.led_map = super::super::boards::LED_MAP_SHIPPED.into();
        assert!(keyboard(Some(&board)).iter().all(|d| d.lights.len() == 21));
        let lighting = board.lighting.as_mut().unwrap();
        lighting.led_map = super::super::boards::LED_MAP_CHECKED.into();
        let halves = keyboard(Some(&board));
        let (left, right) = (&halves[0], &halves[1]);
        assert_eq!((left.lights.len(), right.lights.len()), (21, 21));
        assert_eq!(left.lights[0].id, "0");
        let at = |d: &DeviceInfo, i: usize| d.lights[i].at.unwrap();
        // LED 0 under the inner thumb (the bottom row, right of the middle); LED 18 under the
        // outer column's top key; the right half mirrors it.
        assert!(at(left, 0).x > KEY_MM * 4.0 && at(left, 0).y > KEY_MM * 3.0);
        assert!(at(left, 18).x < KEY_MM && at(left, 18).y < KEY_MM * 1.5);
        let size = right.size.unwrap();
        assert!((at(right, 0).x - (size.x - at(left, 0).x)).abs() < 0.01);
        // Every LED inside its half.
        assert!(left.lights.iter().all(|l| l
            .at
            .is_some_and(|p| p.x > 0.0 && p.x < size.x && p.y < size.y)));
        // A map naming a key the drawing hasn't: one light per half again.
        board.lighting.as_mut().unwrap().led_keys[3] = 99.into();
        assert!(keyboard(Some(&board)).iter().all(|d| d.lights.len() == 1));
    }

    #[test]
    fn uneven_halves_light_each_halfs_own_leds() {
        // Not the Corne's 21 + 21: a split board of 21 + 18, its map listing every LED.
        let mut board = users_desk::board();
        let lighting = board.lighting.as_mut().unwrap();
        lighting.led_map = super::super::boards::LED_MAP_CHECKED.into();
        lighting.led_keys = super::super::boards::Lamp::chain(&PER_KEY);
        let left = lighting.led_keys.clone();
        lighting.led_keys.extend_from_slice(&left[..18]);
        (lighting.lamps, lighting.parts) = (39, vec![21, 18]);
        let halves = keyboard(Some(&board));
        assert_eq!((halves[0].lights.len(), halves[1].lights.len()), (21, 18));
        assert_eq!(halves[1].lights[17].id, "17");
        // Parts that don't add up to the lamps: shared out evenly, and the map fits neither.
        let lighting = board.lighting.as_mut().unwrap();
        (lighting.lamps, lighting.parts) = (40, vec![30, 30]);
        assert!(keyboard(Some(&board)).iter().all(|d| d.lights.len() == 1));
    }

    #[test]
    fn unknown_board_unread_lamp_array_and_a_hand_added_device() {
        let devices = devices(None, &[users_desk::mouse_row()], &BTreeMap::new());
        assert_eq!(
            devices.len(),
            2,
            "one keyboard stands in, not a Corne-shaped split"
        );
        assert_eq!(devices[0].kind, DeviceKind::Keyboard);
        assert_eq!(devices[1].kind, DeviceKind::Other);
        assert_eq!(devices[1].lights.len(), 1, "one light until it is read");
        // A board added by hand: zones without positions.
        let mut shelf = users_desk::catalog_row((0x103c, 0x84fd));
        shelf.key = "1234:5678".into();
        shelf.size = None;
        shelf.zone_positions = vec![None; 5];
        let d = zones(&shelf);
        assert!(d.size.is_none() && d.lights.iter().all(|l| l.at.is_none()));
        assert_eq!(vid_pid("1234:5678:SER1"), Some((0x1234, 0x5678)));
        assert_eq!(vid_pid("nonsense"), None);
    }

    #[test]
    fn the_layout_is_kept_in_the_store_across_restarts() {
        let _dir = super::super::store::test_config_dir();
        let mut layout = Layout {
            case_side: foxglove_scene::Side::Left,
            ..Layout::default()
        };
        layout.nudge("light:103c:84fd", Point::new(-40.0, 12.5));
        super::super::store::section(STORE_OWNER, SECTION)
            .save(&layout)
            .unwrap();
        // A restart reads the file again.
        assert_eq!(super::layout(), layout);
    }
}
