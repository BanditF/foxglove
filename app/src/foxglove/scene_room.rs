//! Scenes' room map on this PC (#483). `foxglove_scene` places everything; this gathers what it
//! places from Windows and from what Foxglove already knows:
//! - the monitors: Windows' display layout and each panel's physical DPI;
//! - the keyboard: its board profile (halves, and their size from the drawing), lit as one
//!   strip per half until per-LED positions arrive (#115, #274);
//! - the lighting devices the Lights page found: catalog zones with their positions (the OMEN
//!   45L), LampArrays with their lamps (the G502), and devices added by hand.
//!
//! The user's drags live in the store's `[scene]` section ([`foxglove_scene::Layout`]).
//!
//! Monitors are read on the first request and again on each `WM_DISPLAYCHANGE`, which Windows
//! sends to a hidden window whose thread sleeps in `GetMessageW`. Nothing polls, and nothing
//! runs before something asks for the room.

use std::collections::BTreeMap;
use std::sync::Mutex;

use foxglove_devices::pc_lighting::{Control, Protocol};
use foxglove_scene::{DeviceInfo, DeviceKind, Layout, LightSpec, MonitorInfo, Point, RoomMap};

use super::boards::{Board, Drawing};
use super::pc_lights::{LampLayout, Row};

/// The store section with the user's drags and the case's side.
pub const SECTION: &str = "scene";

/// One key unit, in millimetres.
const KEY_MM: f32 = 19.05;

static MONITORS: Mutex<Option<Vec<MonitorInfo>>> = Mutex::new(None);

/// The user's layout changes, or none.
pub fn layout() -> Layout {
    super::store::try_load(SECTION).unwrap_or_default()
}

/// The room as it is now.
pub fn room() -> RoomMap {
    let devices = devices(
        super::boards::current().as_ref(),
        &super::pc_lights::snapshot().devices,
        &super::pc_lights::lamp_layouts(),
    );
    RoomMap::build(&monitors(), &devices, &layout())
}

/// The displays, read once and again after each display change.
pub fn monitors() -> Vec<MonitorInfo> {
    imp::watch();
    MONITORS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(imp::read)
        .clone()
}

/// A display change: read the monitors again.
#[cfg_attr(not(windows), allow(dead_code))]
fn display_changed() {
    let now = imp::read();
    let mut known = MONITORS.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref() != Some(&now) {
        eprintln!(
            "Foxglove: displays changed, the room map has {} monitor(s)",
            now.len()
        );
    }
    *known = Some(now);
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
            Control::Driver(protocol) => zones(row, protocol),
            Control::LampArray => lamp_array(row, ids.and_then(|ids| lamps.get(&ids))),
        };
        out.push(device);
    }
    out
}

/// The keyboard: both halves (or the one piece), sized from the board's drawing. With no
/// profile it is a Corne-shaped split, as the Keyboard page draws it.
fn keyboard(board: Option<&Board>) -> Vec<DeviceInfo> {
    let name = board.map_or("Keyboard", |b| b.name.as_str());
    let drawing = board.and_then(|b| b.drawing.clone()).unwrap_or_default();
    let (w, h) = Drawing::size(&drawing);
    let half = Point::new(w * KEY_MM, h * KEY_MM);
    if board.is_some_and(|b| b.halves == 1) {
        // A one-piece board's drawing is its left half; the right is the mirror.
        let whole = Point::new(half.x * 2.0, half.y);
        return vec![DeviceInfo::whole("keyboard", name, DeviceKind::Keyboard).with_size(whole)];
    }
    vec![
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
    ]
}

/// The `vvvv:pppp` a lighting device's key starts with.
fn vid_pid(key: &str) -> Option<(u16, u16)> {
    let mut parts = key.split(':');
    let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vid, pid))
}

fn size(s: Option<[u16; 2]>) -> Option<Point> {
    s.map(|[w, h]| Point::new(f32::from(w), f32::from(h)))
}

/// A catalog board: one light per zone, where the catalog puts it.
fn zones(row: &Row, protocol: Protocol) -> DeviceInfo {
    let kind = match protocol {
        Protocol::OmenTracer => DeviceKind::Case,
        Protocol::None => DeviceKind::Other,
    };
    let lights = row
        .zone_ids
        .iter()
        .enumerate()
        .map(|(i, id)| LightSpec {
            id: id.clone(),
            at: size(row.zone_positions.get(i).copied().flatten()),
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
    let kind = match layout.kind {
        2 => DeviceKind::Mouse,
        7 => DeviceKind::Case,
        _ => DeviceKind::Other,
    };
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
            .map(|(i, &[x, y])| LightSpec {
                id: i.to_string(),
                at: positioned.then_some(Point::new(x, y)),
            })
            .collect(),
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
        WINDOW_STYLE, WM_DISPLAYCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
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

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_devices::pc_lighting::{Catalog, LightCaps};

    /// A Lights page row from the bundled catalog's entry, as the worker builds it.
    fn row_from_catalog(vid: u16, pid: u16) -> Row {
        let c = Catalog::bundled();
        let d = c.find(vid, pid).unwrap();
        Row {
            key: format!("{vid:04x}:{pid:04x}"),
            name: d.name.clone(),
            control: match d.protocol {
                Protocol::None => Control::NotYet,
                p => Control::Driver(p),
            },
            zones: d.zones.iter().map(|z| (z.name.clone(), None)).collect(),
            match_keyboard: false,
            caps: d.caps(),
            entry: None,
            zone_ids: d.zones.iter().map(|z| z.id.to_string()).collect(),
            calibration: None,
            calibrated: false,
            size: d.size,
            zone_positions: d.zones.iter().map(|z| z.position).collect(),
        }
    }

    fn g502_row() -> Row {
        Row {
            key: "046d:c332".into(),
            name: "G502 Proteus Spectrum".into(),
            control: Control::LampArray,
            zones: vec![],
            match_keyboard: false,
            caps: LightCaps::default(),
            entry: None,
            zone_ids: vec![],
            calibration: None,
            calibrated: false,
            size: None,
            zone_positions: vec![],
        }
    }

    fn corne() -> Board {
        Board::parse(include_str!("../../resources/boards/eyelash-corne.toml"))
            .expect("the Corne's profile")
    }

    #[test]
    fn the_users_devices_come_in_from_what_foxglove_knows() {
        let rows = [
            row_from_catalog(0x103c, 0x84fd),
            row_from_catalog(0x03f0, 0x7397),
            g502_row(),
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
        let board = corne();
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
        // The 45L's zones carry the catalog's positions.
        let case = &devices[2];
        assert_eq!(case.size, Some(Point::new(205.0, 475.0)));
        assert_eq!(case.lights.len(), 5);
        assert!(case.lights.iter().all(|l| l.at.is_some()));
        // The G502's lamps where Windows says.
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

    #[test]
    fn unknown_board_unread_lamp_array_and_a_hand_added_device() {
        let devices = devices(None, &[g502_row()], &BTreeMap::new());
        assert_eq!(devices.len(), 3, "a Corne-shaped split stands in");
        assert_eq!(devices[2].kind, DeviceKind::Other);
        assert_eq!(devices[2].lights.len(), 1, "one light until it is read");
        // A board added by hand: zones without positions.
        let mut shelf = row_from_catalog(0x103c, 0x84fd);
        shelf.key = "1234:5678".into();
        shelf.size = None;
        shelf.zone_positions = vec![None; 5];
        let d = zones(&shelf, Protocol::OmenTracer);
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
        super::super::store::save(SECTION, &layout).unwrap();
        // A restart reads the file again.
        assert_eq!(super::layout(), layout);
    }
}
