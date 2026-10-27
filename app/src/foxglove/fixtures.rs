//! Test fixtures for two desks (#557, epic #563). Foxglove's features work for any device of
//! their class; the user's devices are only the test devices. So every cross-cutting suite (the
//! room map, scene output, the Lights page, the LED map) runs once on each desk:
//!
//! - [`users_desk`]: the Eyelash Corne (split, two screens, 2 x 21 LEDs), the HP OMEN 45L case
//!   board, a Logitech mouse on Windows Dynamic Lighting, and two monitors.
//! - [`second_desk`]: a desk that differs on every axis: the made-up "Plank 3" (one piece,
//!   three screens of different sizes and roles, 61 LEDs, a whole-board drawing), a case board
//!   from another (made-up) vendor that Foxglove has no driver for (`protocol = "none"`, driven
//!   here by a fake driver), a LampArray **lightbar** (not a mouse), and two monitors of
//!   different DPI.
//!
//! A test that fails on the second desk is marked `#[ignore = "#<sub-issue>"]` with the epic's
//! sub-issue that fixes it: the ignored tests are the epic's to-do list.

use std::collections::BTreeMap;

use foxglove_devices::pc_lighting::{
    Catalog, CatalogDevice, Control, HidEntry, LampArrayInfo, LightCaps,
};
use foxglove_scene::MonitorInfo;

use super::boards::Board;
use super::pc_lights::{LampLayout, Row};

/// A Lights page row for a catalog entry, as the worker builds it, controlled by `control`.
pub(crate) fn row(d: &CatalogDevice, control: Control) -> Row {
    Row {
        key: format!("{:04x}:{:04x}", d.vid, d.pid),
        name: d.name.clone(),
        control,
        zones: d.zones.iter().map(|z| (z.name.clone(), None)).collect(),
        match_keyboard: false,
        caps: d.caps(),
        entry: None,
        zone_ids: d.zones.iter().map(|z| z.id.to_string()).collect(),
        calibration: None,
        calibrated: false,
        size: d.size,
        zone_positions: d.zones.iter().map(|z| z.position).collect(),
        zone_roles: d.zones.iter().map(|z| z.role).collect(),
        kind: d.kind,
    }
}

/// A LampArray's row before its attributes are read: no zones, no catalog entry.
fn lamp_array_row(key: &str, name: &str) -> Row {
    Row {
        key: key.into(),
        name: name.into(),
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
        zone_roles: vec![],
        kind: Default::default(),
    }
}

/// The user's desk: the devices Foxglove was first built on.
pub(crate) mod users_desk {
    use super::*;
    use foxglove_devices::pc_lighting::Protocol;

    /// The case board's VID/PID (HP OMEN 45L) and the cooler's.
    pub(crate) const CASE: (u16, u16) = (0x103c, 0x84fd);
    pub(crate) const COOLER: (u16, u16) = (0x03f0, 0x7397);
    /// The mouse's VID/PID (a Logitech G502 on Dynamic Lighting).
    pub(crate) const MOUSE: (u16, u16) = (0x046d, 0xc332);

    /// The Eyelash Corne's built-in profile.
    pub(crate) fn board() -> Board {
        Board::parse(include_str!("../../resources/boards/eyelash-corne.toml"))
            .expect("the Corne's profile")
    }

    /// A row from the bundled catalog's entry, as the worker builds it.
    pub(crate) fn catalog_row((vid, pid): (u16, u16)) -> Row {
        let c = Catalog::bundled();
        let d = c.find(vid, pid).expect("a bundled catalog entry");
        let control = match d.protocol {
            Protocol::None => Control::NotYet,
            p => Control::Driver(p),
        };
        row(d, control)
    }

    /// The mouse on Dynamic Lighting, before its attributes are read.
    pub(crate) fn mouse_row() -> Row {
        lamp_array_row("046d:c332", "G502 Proteus Spectrum")
    }

    /// The mouse's lamps as Windows reports them: a mouse (kind 2), two lamps.
    pub(crate) fn lamps() -> BTreeMap<(u16, u16), LampLayout> {
        BTreeMap::from([(
            MOUSE,
            LampLayout {
                kind: 2,
                size_mm: [68.0, 132.0],
                lamps: vec![[34.0, 40.0], [34.0, 100.0]],
            },
        )])
    }

    /// A 27" 1440p monitor and a 1080p one to its left.
    pub(crate) fn monitors() -> Vec<MonitorInfo> {
        vec![
            MonitorInfo {
                id: r"\\.\DISPLAY1".into(),
                primary: true,
                rect_px: [0, 0, 2560, 1440],
                raw_dpi: Some(109),
                dpi: 96,
            },
            MonitorInfo {
                id: r"\\.\DISPLAY2".into(),
                primary: false,
                rect_px: [-1920, 360, 0, 1440],
                raw_dpi: Some(92),
                dpi: 96,
            },
        ]
    }
}

/// The second desk: different on every axis from the user's.
pub(crate) mod second_desk {
    use super::*;
    use foxglove_devices::pc_lighting::Protocol;

    /// "Plank 3": a one-piece board (no halves) with three screens of different sizes and
    /// roles, 61 LEDs placed under its keys and a drawing of the whole board (10 columns of 6
    /// rows and a space bar, 61 keys). Digitalis-like: what a second real board looks like.
    pub(crate) const PLANK3: &str = r#"
format = "foxglove-board"
version = 1
id = "plank-3"
name = "Plank 3"
status = "untested"
protocol = "zmk"
halves = 1

[[match]]
vid = 0x1d50
pid = 0x615e
product = "Plank 3"

[[screens]]
kind = "panel"
half = "left"
width = 128
height = 32
format = "1bit"

[[screens]]
kind = "art"
half = "middle"
width = 160
height = 80
format = "1bit"

[[screens]]
kind = "display"
half = "right"
width = 240
height = 135
format = "1bit"

[lighting]
lamps = 61
layout = "per key, one strip"
# One snake through the rows (row 0 left to right, row 1 back, ...), then the space bar.
led_keys = [0, 6, 12, 18, 24, 30, 36, 42, 48, 54, 55, 49, 43, 37, 31, 25, 19, 13, 7, 1, 2, 8, 14, 20, 26, 32, 38, 44, 50, 56, 57, 51, 45, 39, 33, 27, 21, 15, 9, 3, 4, 10, 16, 22, 28, 34, 40, 46, 52, 58, 59, 53, 47, 41, 35, 29, 23, 17, 11, 5, 60]
led_map = "shipped"

# The whole board, not a half.
[drawing]
columns = 10
rows = 6
thumbs = 1
"#;

    /// Plank 3's LEDs and screens.
    pub(crate) const LAMPS: usize = 61;
    pub(crate) const SCREENS: usize = 3;

    /// Lighting devices from made-up vendors: a case board Foxglove has no driver for, and a
    /// lightbar that only Windows Dynamic Lighting knows (it needs no catalog entry).
    pub(crate) const CATALOG: &str = r#"
[[device]]
name = "Nimbus Strata case lights"
vid = 0x1A2B
pid = 0x3C4D
protocol = "none"
kind = "case"
size = [230, 500]
zones = [
    { id = 1, name = "Top light bar", position = [115, 20] },
    { id = 2, name = "Front ring upper", position = [115, 150] },
    { id = 3, name = "Front ring lower", position = [115, 330] },
    { id = 4, name = "Glass glow", position = [140, 250], role = "ambient" },
]
# Its vendor's own lighting tool fights Foxglove over the case.
[[device.conflict]]
app = "Nimbus Glow"
processes = ["NimbusGlow.exe"]
hint = "Quit Nimbus Glow from its tray icon, or turn off its lighting control."
"#;

    /// The case board's vendor tool, as the catalog names it, and its exe.
    pub(crate) const CONFLICT_APP: &str = "Nimbus Glow";
    pub(crate) const CONFLICT_EXE: &str = "NimbusGlow.exe";

    /// The case board's VID/PID and its room id.
    pub(crate) const CASE: (u16, u16) = (0x1a2b, 0x3c4d);
    pub(crate) const CASE_ID: &str = "light:1a2b:3c4d";
    /// The lightbar's VID/PID and its room id.
    pub(crate) const LIGHTBAR: (u16, u16) = (0x2f1e, 0x0b42);
    pub(crate) const LIGHTBAR_ID: &str = "light:2f1e:0b42";
    /// The lightbar's lamps, evenly along its 400 mm.
    pub(crate) const LIGHTBAR_LAMPS: usize = 12;

    pub(crate) fn board() -> Board {
        Board::parse(PLANK3).expect("Plank 3's profile")
    }

    /// The bundled catalog with the second desk's entries over it.
    pub(crate) fn catalog() -> Catalog {
        Catalog::bundled().with_local(&Catalog::parse(CATALOG).expect("the second desk's catalog"))
    }

    /// The case board's catalog entry.
    pub(crate) fn case_entry() -> CatalogDevice {
        catalog()
            .find(CASE.0, CASE.1)
            .expect("the second desk's case")
            .clone()
    }

    /// The case board's row as if a driver drove it (the fake driver). Its catalog entry says
    /// `protocol = "none"`, which the worker shows as "not controllable yet" and scenes skip
    /// (`pc_lights`' second-desk test checks that). `Control::Driver(Protocol::None)` is the
    /// stand-in for "some driver Foxglove has that isn't the OMEN one": the room places it and
    /// the page shows it as driven, so the suites test the case's data and not the driver's
    /// name. Never a state the app makes itself.
    pub(crate) fn case_row() -> Row {
        row(&case_entry(), Control::Driver(Protocol::None))
    }

    /// The lightbar on Dynamic Lighting, before its attributes are read.
    pub(crate) fn lightbar_row() -> Row {
        lamp_array_row("2f1e:0b42", "Lumen desk lightbar")
    }

    /// The lightbar as Windows reports it: LampArrayKind Scene (5), 12 lamps in a row.
    pub(crate) fn lightbar_info() -> LampArrayInfo {
        LampArrayInfo {
            lamp_count: LIGHTBAR_LAMPS as u32,
            positions: true,
            min_update_us: 20_000,
            kind: 5,
        }
    }

    pub(crate) fn lamps() -> BTreeMap<(u16, u16), LampLayout> {
        let step = 400.0 / LIGHTBAR_LAMPS as f32;
        BTreeMap::from([(
            LIGHTBAR,
            LampLayout {
                kind: 5,
                size_mm: [400.0, 24.0],
                lamps: (0..LIGHTBAR_LAMPS)
                    .map(|i| [step * (i as f32 + 0.5), 12.0])
                    .collect(),
            },
        )])
    }

    /// What a HID enumeration finds: the case board on a vendor page, the lightbar's LampArray.
    pub(crate) fn hid() -> Vec<HidEntry> {
        vec![
            HidEntry {
                vid: CASE.0,
                pid: CASE.1,
                usage_page: 0xff00,
                usage: 1,
                path: "strata".into(),
                ..Default::default()
            },
            HidEntry {
                vid: LIGHTBAR.0,
                pid: LIGHTBAR.1,
                usage_page: 0x59,
                usage: 1,
                path: "lightbar".into(),
                product: Some("Lumen desk lightbar".into()),
                ..Default::default()
            },
        ]
    }

    /// Two monitors of different DPI: a 27" 4K at 150 % and a 24" 1080p at 100 % to its right.
    pub(crate) fn monitors() -> Vec<MonitorInfo> {
        vec![
            MonitorInfo {
                id: r"\\.\DISPLAY1".into(),
                primary: true,
                rect_px: [0, 0, 3840, 2160],
                raw_dpi: Some(163),
                dpi: 144,
            },
            MonitorInfo {
                id: r"\\.\DISPLAY2".into(),
                primary: false,
                rect_px: [3840, 0, 5760, 1080],
                raw_dpi: Some(92),
                dpi: 96,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_desk_differs_from_the_users_on_every_axis() {
        let (mine, other) = (users_desk::board(), second_desk::board());
        assert_eq!((mine.halves, other.halves), (2, 1), "board shape");
        assert_eq!(
            (mine.screens.len(), other.screens.len()),
            (2, second_desk::SCREENS),
            "screen count"
        );
        let kinds: Vec<_> = other.screens.iter().map(|s| s.kind.as_str()).collect();
        assert_eq!(kinds, ["panel", "art", "display"], "screen roles");
        let lamps = |b: &Board| b.lighting.as_ref().map(|l| usize::from(l.lamps));
        assert_eq!(
            (lamps(&mine), lamps(&other)),
            (Some(42), Some(second_desk::LAMPS))
        );
        let l = other.lighting.as_ref().unwrap();
        assert_eq!(l.led_keys.len(), second_desk::LAMPS, "one key per LED");
        let d = other.drawing.as_ref().expect("a whole-board drawing");
        let (c, r, t) = d.grid();
        assert_eq!(c * r + t, second_desk::LAMPS);
        let mut keys = l.led_keys.clone();
        keys.sort_unstable();
        assert_eq!(keys, (0..61).collect::<Vec<u8>>(), "every key once");
        // The case board: another vendor, no driver; the lightbar isn't in any catalog.
        let catalog = second_desk::catalog();
        let case = second_desk::case_entry();
        assert_ne!(case.vid, users_desk::CASE.0);
        assert_eq!(case.protocol, foxglove_devices::pc_lighting::Protocol::None);
        assert_eq!(case.zones.len(), 4);
        assert!(catalog
            .find(second_desk::LIGHTBAR.0, second_desk::LIGHTBAR.1)
            .is_none());
        assert!(
            catalog
                .find(users_desk::CASE.0, users_desk::CASE.1)
                .is_some(),
            "the bundled entries stay"
        );
        assert_ne!(second_desk::lightbar_info().kind, 2, "not a mouse");
        // Monitors of different DPI.
        let dpi: Vec<_> = second_desk::monitors().iter().map(|m| m.dpi).collect();
        assert_eq!(dpi, [144, 96]);
    }
}
