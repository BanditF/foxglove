use super::*;

fn monitor(id: &str, primary: bool, rect_px: [i32; 4], raw_dpi: Option<u32>) -> MonitorInfo {
    MonitorInfo {
        id: format!(r"\\.\{id}"),
        primary,
        rect_px,
        raw_dpi,
        dpi: 96,
    }
}

fn halves() -> Vec<DeviceInfo> {
    vec![
        DeviceInfo::whole(
            "keyboard:left",
            "Eyelash Corne left",
            DeviceKind::KeyboardLeft,
        ),
        DeviceInfo::whole(
            "keyboard:right",
            "Eyelash Corne right",
            DeviceKind::KeyboardRight,
        ),
    ]
}

fn g502() -> DeviceInfo {
    DeviceInfo::whole(
        "light:046d:c332",
        "G502 Proteus Spectrum",
        DeviceKind::Mouse,
    )
}

/// The OMEN 45L as the lighting catalog describes it (zones 1, 2, 5, 6, 7).
fn omen_45l() -> DeviceInfo {
    let zone = |id: &str, x: f32, y: f32| LightSpec {
        id: id.into(),
        at: Some(Point::new(x, y)),
    };
    DeviceInfo {
        id: "light:103c:84fd".into(),
        name: "HP OMEN 45L case lights".into(),
        kind: DeviceKind::Case,
        size: Some(Point::new(205.0, 475.0)),
        lights: vec![
            zone("1", 102.0, 55.0),
            zone("2", 120.0, 250.0),
            zone("5", 102.0, 405.0),
            zone("6", 102.0, 285.0),
            zone("7", 102.0, 165.0),
        ],
    }
}

/// Every device has a finite, non-empty rectangle, and no two overlap.
fn assert_sane(room: &RoomMap) {
    for d in &room.devices {
        let r = d.rect;
        assert!(
            [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite()) && r.w > 0.0 && r.h > 0.0,
            "{d:?}"
        );
        for l in &d.lights {
            assert!(l.at.x.is_finite() && l.at.y.is_finite(), "{l:?}");
        }
    }
    for (i, a) in room.devices.iter().enumerate() {
        for b in &room.devices[i + 1..] {
            assert!(!a.rect.overlaps(&b.rect), "{} overlaps {}", a.id, b.id);
        }
    }
}

fn light(room: &RoomMap, device: &str, id: &str) -> Point {
    room.device(device)
        .and_then(|d| d.lights.iter().find(|l| l.id == id))
        .map(|l| l.at)
        .unwrap_or_else(|| panic!("{device} {id}"))
}

#[test]
fn the_users_desk_places_itself() {
    // A 27" 1440p primary and a 24" 1080p to its right, the Corne, the G502, the 45L.
    let monitors = [
        monitor("DISPLAY1", true, [0, 0, 2560, 1440], Some(109)),
        monitor("DISPLAY2", false, [2560, 0, 4480, 1080], Some(92)),
    ];
    let mut devices = halves();
    devices.push(g502());
    devices.push(omen_45l());
    let room = RoomMap::build(&monitors, &devices, &Layout::default());
    assert_sane(&room);
    assert_eq!(room.devices.len(), 6);

    let primary = room.device(r"monitor:\\.\DISPLAY1").unwrap().rect;
    assert!(
        (primary.w - 596.6).abs() < 1.0,
        "a 27-inch panel: {primary:?}"
    );
    let second = room.device(r"monitor:\\.\DISPLAY2").unwrap().rect;
    assert!((second.x - primary.right()).abs() < 0.5, "side by side");
    assert_eq!(second.y, primary.y, "tops level, as Windows arranges them");

    // The keyboard is centred below the primary monitor, the halves either side of centre.
    let left = room.device("keyboard:left").unwrap().rect;
    let right = room.device("keyboard:right").unwrap().rect;
    assert!(left.y > primary.bottom() && right.y == left.y);
    let centre = (left.x + right.right()) / 2.0;
    assert!((centre - primary.centre().x).abs() < 0.5);
    assert!(left.right() < primary.centre().x && right.x > primary.centre().x);

    // The mouse to the right of the keyboard, on its row.
    let mouse = room.device("light:046d:c332").unwrap().rect;
    assert!(mouse.x > right.right());
    assert!((mouse.centre().y - right.centre().y).abs() < 0.5);

    // The case beside the monitors, standing on the desk row.
    let case = room.device("light:103c:84fd").unwrap();
    assert!(case.rect.x > second.right());
    assert!((case.rect.bottom() - right.bottom().max(mouse.bottom())).abs() < 0.5);
    // Its front fans run 7, 6, 5 top to bottom; the diamond sits above them.
    let y = |z: &str| light(&room, "light:103c:84fd", z).y;
    assert!(y("1") < y("7") && y("7") < y("6") && y("6") < y("5"));
    assert!(case.rect.y < y("1") && y("5") < case.rect.bottom());

    // A whole-strip half has one light, in its middle.
    assert_eq!(light(&room, "keyboard:left", "all"), left.centre());
    assert!(room.bounds.w > 1400.0 && room.bounds.h > 590.0, "{room:?}");
    assert!(
        room.summary().contains("case 205x475"),
        "{}",
        room.summary()
    );
}

#[test]
fn one_monitor_and_nothing_else_still_places() {
    let monitors = [monitor("DISPLAY1", true, [0, 0, 1920, 1080], None)];
    let room = RoomMap::build(&monitors, &halves(), &Layout::default());
    assert_sane(&room);
    // No raw DPI: Windows' scaling DPI (96) stands in.
    let m = room.devices[0].rect;
    assert!((m.w - 508.0).abs() < 0.5, "{m:?}");
    assert_eq!(room.devices.len(), 3);
}

#[test]
fn three_monitors_of_mixed_density_touch_and_dont_overlap() {
    // A 1440p primary in the middle, a 4K 27" to its left, a portrait 1080p to its right.
    let monitors = [
        monitor("DISPLAY2", false, [-3840, -200, 0, 1960], Some(163)),
        monitor("DISPLAY1", true, [0, 0, 2560, 1440], Some(109)),
        monitor("DISPLAY3", false, [2560, -300, 3640, 1620], Some(92)),
    ];
    let mut devices = halves();
    devices.push(g502());
    devices.push(omen_45l());
    let room = RoomMap::build(&monitors, &devices, &Layout::default());
    assert_sane(&room);
    let r = |id: &str| room.device(&format!(r"monitor:\\.\{id}")).unwrap().rect;
    let (left, mid, right) = (r("DISPLAY2"), r("DISPLAY1"), r("DISPLAY3"));
    assert!((left.right() - mid.x).abs() < 0.5, "{left:?} {mid:?}");
    assert!((right.x - mid.right()).abs() < 0.5);
    assert!(right.h > right.w, "portrait");
    // The case goes beyond the rightmost monitor.
    assert!(room.device("light:103c:84fd").unwrap().rect.x > right.right());
}

#[test]
fn no_case_no_mouse_no_monitor() {
    let room = RoomMap::build(&[], &halves(), &Layout::default());
    assert_sane(&room);
    assert_eq!(room.devices.len(), 2);
    assert!(room.devices.iter().all(|d| d.kind != DeviceKind::Case));
    // Anchored to an assumed monitor at the origin: the keyboard is below where it would be.
    assert!(room.device("keyboard:left").unwrap().rect.y > DEFAULT_MONITOR.y);
    assert_eq!(
        RoomMap::build(&[], &[], &Layout::default()).summary(),
        "empty"
    );
}

#[test]
fn a_device_added_by_hand_and_a_case_on_the_left() {
    // "Add a device": a shelf strip with two named zones and no positions.
    let shelf = DeviceInfo {
        id: "light:1234:5678".into(),
        name: "Shelf".into(),
        kind: DeviceKind::Other,
        size: Some(Point::new(400.0, 20.0)),
        lights: vec![
            LightSpec {
                id: "1".into(),
                at: None,
            },
            LightSpec {
                id: "3".into(),
                at: None,
            },
        ],
    };
    let monitors = [monitor("DISPLAY1", true, [0, 0, 2560, 1440], Some(109))];
    let mut devices = halves();
    devices.push(shelf);
    devices.push(omen_45l());
    let layout = Layout {
        case_side: Side::Left,
        ..Layout::default()
    };
    let room = RoomMap::build(&monitors, &devices, &layout);
    assert_sane(&room);
    let shelf = room.device("light:1234:5678").unwrap();
    assert!(shelf.rect.right() < room.device("keyboard:left").unwrap().rect.x);
    // Spread evenly along its long side.
    let (a, b) = (shelf.lights[0].at, shelf.lights[1].at);
    assert_eq!(a.y, b.y);
    assert!(
        (a.x - (shelf.rect.x + 100.0)).abs() < 0.01 && (b.x - (shelf.rect.x + 300.0)).abs() < 0.01
    );
    let case = room.device("light:103c:84fd").unwrap().rect;
    assert!(
        case.right() < shelf.rect.x && case.right() < 0.0,
        "left of everything"
    );
}

#[test]
fn a_drag_moves_the_device_and_its_lights_and_round_trips() {
    let monitors = [monitor("DISPLAY1", true, [0, 0, 2560, 1440], Some(109))];
    let mut devices = halves();
    devices.push(omen_45l());
    let before = RoomMap::build(&monitors, &devices, &Layout::default());
    let mut layout = Layout::default();
    layout.nudge("light:103c:84fd", Point::new(-30.0, 10.0));
    layout.nudge("light:103c:84fd", Point::new(-20.0, 0.0));
    let text = toml::to_string(&layout).unwrap();
    let back: Layout = toml::from_str(&text).unwrap();
    assert_eq!(back, layout, "{text}");
    let after = RoomMap::build(&monitors, &devices, &back);
    let (a, b) = (
        before.device("light:103c:84fd").unwrap(),
        after.device("light:103c:84fd").unwrap(),
    );
    assert_eq!(b.rect, a.rect.moved(Point::new(-50.0, 10.0)));
    assert_eq!(b.lights[0].at, a.lights[0].at + Point::new(-50.0, 10.0));
    assert_eq!(
        after.device("keyboard:left"),
        before.device("keyboard:left"),
        "the rest stay"
    );
    layout.reset("light:103c:84fd");
    assert!(layout.offsets.is_empty());
    // An empty section reads as the defaults.
    assert_eq!(toml::from_str::<Layout>("").unwrap(), Layout::default());
}
