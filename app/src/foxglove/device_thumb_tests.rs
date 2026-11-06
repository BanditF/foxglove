//! Tests of the device thumbnails (#803): the shape ladder, the tints, each fixture desk's
//! devices, and no product in the painter's source.

use super::*;
use crate::foxglove::fixtures::{second_desk, users_desk};
use crate::foxglove::key_layout::{self, tests::corne_from_keyboard};
use crate::foxglove::theme::{DARK, LIGHT};

/// Every device on the two fixture desks, with the shape it must take.
pub(crate) fn fixture_thumbs(tint: impl Fn(usize) -> Tint) -> Vec<(&'static str, Thumb, Shape)> {
    let corne = users_desk::board();
    let plank = second_desk::board();
    let mouse_lamps = users_desk::lamps();
    let bar_lamps = second_desk::lamps();
    let with = |mut t: Thumb, n: usize| {
        t.tint = Some(tint(n));
        t
    };
    vec![
        (
            "Users desk · keyboard (Studio layout)",
            Thumb::keyboard(corne_from_keyboard(), tint(users_desk::KEYS)),
            Shape::Split,
        ),
        (
            "Users desk · keyboard (profile)",
            Thumb::keyboard(
                key_layout::resolve(Some(&corne), &[], users_desk::KEYS),
                tint(users_desk::KEYS),
            ),
            Shape::Split,
        ),
        (
            "Users desk · case",
            with(
                Thumb::of_row(&users_desk::catalog_row(users_desk::CASE), None),
                5,
            ),
            Shape::Case,
        ),
        (
            "Users desk · cooler",
            with(
                Thumb::of_row(&users_desk::catalog_row(users_desk::COOLER), None),
                1,
            ),
            Shape::Cooler,
        ),
        (
            "Users desk · mouse (LampArray)",
            with(
                Thumb::of_row(
                    &users_desk::mouse_row(),
                    mouse_lamps.get(&users_desk::MOUSE),
                ),
                2,
            ),
            Shape::Mouse,
        ),
        (
            "Second desk · keyboard (profile)",
            Thumb::keyboard(
                key_layout::resolve(Some(&plank), &[], second_desk::KEYS),
                tint(second_desk::KEYS),
            ),
            Shape::Keyboard,
        ),
        (
            "Second desk · case",
            with(Thumb::of_row(&second_desk::case_row(), None), 4),
            Shape::Case,
        ),
        (
            "Second desk · lightbar (LampArray)",
            with(
                Thumb::of_row(
                    &second_desk::lightbar_row(),
                    bar_lamps.get(&second_desk::LIGHTBAR),
                ),
                second_desk::LIGHTBAR_LAMPS,
            ),
            Shape::Strip,
        ),
    ]
}

/// `n` colours round the colour wheel.
pub(crate) fn rainbow(n: usize) -> Tint {
    Tint::Lit(
        (0..n.max(1))
            .map(|i| egui::ecolor::Hsva::new(i as f32 / n.max(1) as f32, 0.9, 1.0, 1.0).into())
            .collect(),
    )
}

fn area() -> Rect {
    Rect::from_min_size(Pos2::new(10.0, 20.0), ROW)
}

#[test]
fn each_fixture_device_has_its_shape() {
    for (name, thumb, shape) in fixture_thumbs(rainbow) {
        assert_eq!(thumb.shape(), shape, "{name}");
        let marks = thumb.marks(area(), &DARK);
        assert!(!marks.is_empty(), "{name}");
    }
}

#[test]
fn shapes_come_from_data_in_order() {
    let all = Thumb {
        catalog_shape: Some(CatalogShape::Mouse),
        lamp_kind: Some(lamp_kind::HEADSET),
        catalog_kind: Some(DeviceKind::Strip),
        bt: Some(BtCategory::Gamepad),
        lamps: vec![[1.0, 1.0], [5.0, 9.0]],
        ..Default::default()
    };
    assert_eq!(all.shape(), Shape::Mouse, "the catalog's shape first");
    let no_shape = Thumb {
        catalog_shape: None,
        ..all.clone()
    };
    assert_eq!(no_shape.shape(), Shape::Headset, "then the LampArray kind");
    let no_lamp_kind = Thumb {
        lamp_kind: None,
        ..no_shape.clone()
    };
    assert_eq!(
        no_lamp_kind.shape(),
        Shape::Strip,
        "then the catalog's kind"
    );
    let no_kind = Thumb {
        catalog_kind: Some(DeviceKind::Other),
        ..no_lamp_kind.clone()
    };
    assert_eq!(
        no_kind.shape(),
        Shape::Controller,
        "then the Bluetooth category"
    );
    let no_bt = Thumb {
        bt: None,
        ..no_kind.clone()
    };
    assert_eq!(no_bt.shape(), Shape::Lamps, "then its lamps");
    let nothing = Thumb {
        lamps: vec![],
        ..no_bt
    };
    assert_eq!(nothing.shape(), Shape::Unknown);
    // A kind Windows reports that no shape fits (a speaker) falls through.
    let speaker = Thumb {
        lamp_kind: Some(13),
        catalog_kind: Some(DeviceKind::Cooler),
        ..Default::default()
    };
    assert_eq!(speaker.shape(), Shape::Cooler);
    // A scene is a strip only when its lamps are in a row.
    let scene = |lamps: Vec<[f32; 2]>| Thumb {
        lamp_kind: Some(lamp_kind::SCENE),
        lamps,
        ..Default::default()
    };
    assert_eq!(
        scene(vec![[0.0, 5.0], [100.0, 6.0], [200.0, 5.0]]).shape(),
        Shape::Strip
    );
    assert_eq!(
        scene(vec![[0.0, 0.0], [100.0, 90.0], [20.0, 60.0]]).shape(),
        Shape::Lamps
    );
    // A key layout makes a keyboard, whatever else is said.
    let keyboard = Thumb {
        keys: Some(key_layout::resolve(Some(&second_desk::board()), &[], 61)),
        ..all
    };
    assert_eq!(keyboard.shape(), Shape::Keyboard);
    assert_eq!(Thumb::bluetooth(BtCategory::Mouse).shape(), Shape::Mouse);
    assert_eq!(
        Thumb::bluetooth(BtCategory::Keyboard).shape(),
        Shape::Keyboard
    );
    assert_eq!(
        Thumb::bluetooth(BtCategory::Headset).shape(),
        Shape::Headset
    );
    assert_eq!(
        Thumb::bluetooth(BtCategory::Gamepad).shape(),
        Shape::Controller
    );
    assert_eq!(Thumb::bluetooth(BtCategory::Other).shape(), Shape::Unknown);
}

#[test]
fn a_keyboard_is_its_own_layout_shrunk() {
    let studio = corne_from_keyboard();
    let thumb = Thumb::keyboard(studio.clone(), rainbow(42));
    let marks = thumb.marks(area(), &DARK);
    let keys: Vec<&Mark> = marks
        .iter()
        .filter(|m| matches!(m, Mark::Poly { .. }))
        .collect();
    assert_eq!(keys.len(), 42, "one mark per key");
    let inside = area().expand(0.01);
    for m in &keys {
        let Mark::Poly { points, .. } = m else {
            unreachable!()
        };
        assert!(points.iter().all(|p| inside.contains(*p)), "fits the box");
    }
    // The turned thumbs stay turned: some key isn't axis-aligned.
    assert!(keys.iter().any(|m| {
        let Mark::Poly { points, .. } = m else {
            return false;
        };
        (points[0].y - points[1].y).abs() > 0.05
    }));
    // From the profile it is another drawing of the same keys.
    let profile = key_layout::resolve(Some(&users_desk::board()), &[], 42);
    let other = Thumb::keyboard(profile, rainbow(42)).marks(area(), &DARK);
    assert_ne!(marks, other);
    // No layout known: a plain board.
    let plain = Thumb {
        catalog_shape: Some(CatalogShape::Keyboard),
        tint: Some(Tint::Off),
        ..Default::default()
    };
    assert_eq!(plain.shape(), Shape::Keyboard);
    assert_eq!(plain.marks(area(), &DARK).len(), 48);
    // Rainbow keys: every key its own colour.
    let colours: std::collections::BTreeSet<[u8; 4]> =
        keys.iter().map(|m| m.colour().to_array()).collect();
    assert_eq!(colours.len(), 42);
}

#[test]
fn lit_off_and_unknown_tints() {
    let red = Color32::from_rgb(255, 0, 0);
    for p in [&*DARK, &*LIGHT] {
        for shape in Shape::ALL {
            let thumb = |tint: Tint| Thumb {
                tint: Some(tint),
                ..sample(shape)
            };
            let lit = thumb(Tint::Lit(vec![red])).marks(area(), p);
            let off = thumb(Tint::Off).marks(area(), p);
            let unknown = thumb(Tint::Unknown).marks(area(), p);
            if shape != Shape::Unknown {
                assert!(lit.iter().any(|m| m.colour() == red), "{shape:?} lit");
                assert!(off.iter().all(|m| m.colour() != red), "{shape:?} off");
                assert_ne!(lit, off, "{shape:?}");
            }
            // Unknown: no fill but the body's, grey outlines, never a tint.
            for m in &unknown {
                assert!(m.colour() != red, "{shape:?} unknown");
                match m {
                    Mark::Poly { fill, stroke, .. }
                    | Mark::Rect { fill, stroke, .. }
                    | Mark::Circle { fill, stroke, .. } => {
                        assert_eq!(*fill, Color32::TRANSPARENT, "{shape:?}");
                        assert!(
                            stroke.color == p.muted || stroke.width == 0.0,
                            "{shape:?}: {stroke:?}"
                        );
                    }
                    Mark::Line { stroke, .. } => assert_eq!(stroke.color, p.muted),
                    Mark::Dashed { color, .. } | Mark::Text { color, .. } => {
                        assert_eq!(*color, p.muted)
                    }
                }
            }
            let inside = area().expand(0.5);
            for m in lit.iter().chain(&off).chain(&unknown) {
                let ok = match m {
                    Mark::Poly { points, .. } | Mark::Line { points, .. } => {
                        points.iter().all(|q| inside.contains(*q))
                    }
                    Mark::Rect { rect, .. } | Mark::Dashed { rect, .. } => {
                        inside.contains_rect(*rect)
                    }
                    Mark::Circle { centre, radius, .. } => inside
                        .contains_rect(Rect::from_center_size(*centre, Vec2::splat(radius * 2.0))),
                    Mark::Text { at, .. } => inside.contains(*at),
                };
                assert!(ok, "{shape:?}: {m:?} leaves the box");
            }
        }
    }
}

#[test]
fn tints_go_through_the_calibration() {
    let key = "thumb-test:0001";
    let tint = Tint::through(key, &[(Some("1"), Some([200, 40, 10])), (Some("2"), None)]);
    let [r, g, b] = crate::foxglove::calibrate::on_screen(key, Some("1"), [200, 40, 10]);
    assert_eq!(
        tint,
        Tint::Lit(vec![Color32::from_rgb(r, g, b), Color32::BLACK]),
        "an unset zone is off"
    );
    assert_eq!(
        Tint::through(key, &[(None, Some([0, 0, 0])), (None, None)]),
        Tint::Off
    );
    // A device Foxglove can't drive, or holds no colour for, is unknown.
    let mut row = users_desk::catalog_row(users_desk::COOLER);
    assert_eq!(Tint::of_row(&row), Tint::Unknown);
    row = users_desk::catalog_row(users_desk::CASE);
    assert_eq!(Tint::of_row(&row), Tint::Unknown, "no colour held yet");
    row.zones[0].1 = Some([0, 255, 0]);
    assert!(matches!(Tint::of_row(&row), Tint::Lit(cs) if cs.len() == 5));
}

#[test]
fn a_strip_shows_at_most_twelve_dots() {
    let strip = Thumb {
        catalog_kind: Some(DeviceKind::Strip),
        tint: Some(rainbow(61)),
        ..Default::default()
    };
    let dots = strip
        .marks(area(), &DARK)
        .iter()
        .filter(|m| matches!(m, Mark::Circle { .. }))
        .count();
    assert_eq!(dots, STRIP_DOTS);
}

/// The painter knows classes, never products: no device, vendor or board name from the catalog,
/// the board profiles or the fixture desks is in its source.
#[test]
fn no_per_product_art() {
    let source = include_str!("device_thumb.rs").to_lowercase();
    let mut names: Vec<String> = foxglove_devices::pc_lighting::Catalog::bundled()
        .devices
        .iter()
        .chain(second_desk::catalog().devices.iter())
        .map(|d| d.name.clone())
        .collect();
    names.extend(
        [
            users_desk::board(),
            second_desk::board(),
            crate::foxglove::boards::Board::parse(include_str!(
                "../../resources/boards/toucan2.toml"
            ))
            .unwrap(),
            crate::foxglove::boards::Board::parse(include_str!(
                "../../resources/boards/halcyon-corne-epaper.toml"
            ))
            .unwrap(),
        ]
        .iter()
        .map(|b| b.name.clone()),
    );
    names.extend([
        users_desk::mouse_row().name,
        second_desk::lightbar_row().name,
        second_desk::CONFLICT_APP.to_string(),
    ]);
    // Each name whole, and its brand (the first word).
    let mut words: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    words.extend(
        names
            .iter()
            .filter_map(|n| n.split_whitespace().next())
            .filter(|w| w.len() >= 3)
            .map(str::to_lowercase),
    );
    words.extend(
        [
            "logitech", "omen", "corne", "eyelash", "g502", "razer", "corsair",
        ]
        .map(String::from),
    );
    for w in words {
        let hit = source
            .split(|c: char| !c.is_alphanumeric())
            .any(|token| token == w)
            || (w.contains(' ') && source.contains(&w));
        assert!(!hit, "device_thumb.rs names a product: {w:?}");
    }
}

/// A thumbnail of each shape, as the preview sheet draws it (tint set by the caller).
pub(crate) fn sample(shape: Shape) -> Thumb {
    match shape {
        Shape::Keyboard => Thumb {
            keys: Some(key_layout::resolve(Some(&second_desk::board()), &[], 61)),
            ..Default::default()
        },
        Shape::Split => Thumb {
            keys: Some(corne_from_keyboard()),
            ..Default::default()
        },
        Shape::Case => Thumb::of_row(&second_desk::case_row(), None),
        Shape::Cooler => Thumb {
            catalog_kind: Some(DeviceKind::Cooler),
            ..Default::default()
        },
        Shape::Strip => Thumb::of_row(
            &second_desk::lightbar_row(),
            second_desk::lamps().get(&second_desk::LIGHTBAR),
        ),
        Shape::Mouse => Thumb::of_row(
            &users_desk::mouse_row(),
            users_desk::lamps().get(&users_desk::MOUSE),
        ),
        Shape::Headset => Thumb {
            lamp_kind: Some(lamp_kind::HEADSET),
            ..Default::default()
        },
        Shape::Controller => Thumb::bluetooth(BtCategory::Gamepad),
        Shape::Lamps => Thumb {
            lamp_kind: Some(lamp_kind::SCENE),
            size_mm: Some([300.0, 200.0]),
            lamps: (0..9)
                .map(|i| {
                    let a = i as f32 / 9.0 * std::f32::consts::TAU;
                    [150.0 + a.cos() * 120.0, 100.0 + a.sin() * 80.0]
                })
                .collect(),
            ..Default::default()
        },
        Shape::Unknown => Thumb::default(),
    }
}
