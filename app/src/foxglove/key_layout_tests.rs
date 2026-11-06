//! Tests of the one key layout (#796): the ladder, the Studio layout drawn as the overlay draws
//! it, and the profile's key order kept.

use super::*;
use crate::foxglove::fixtures::users_desk;
use zmk_studio_api::proto::zmk::keymap;

/// A Corne-shaped ZMK Studio physical layout, as ZMK's `corne` shield describes it (hundredths of
/// a key; the inner thumbs turned 12 and 30 degrees around their pivots), and a 36-key one as
/// its second choice. 42 keys: three rows of 12 (left then right), then the 6 thumbs.
pub(crate) fn corne_studio() -> keymap::PhysicalLayouts {
    let key = |w: i32, h: i32, x: i32, y: i32, r: i32, rx: i32, ry: i32| keymap::KeyPhysicalAttrs {
        width: w,
        height: h,
        x,
        y,
        r,
        rx,
        ry,
    };
    let drops = [37, 37, 12, 0, 12, 24];
    let mut keys = Vec::new();
    for row in 0..3 {
        for c in 0..6 {
            keys.push(key(
                100,
                100,
                c * 100,
                drops[c as usize] + row * 100,
                0,
                0,
                0,
            ));
        }
        for c in 0..6 {
            let drop = drops[5 - c as usize];
            keys.push(key(100, 100, 800 + c * 100, drop + row * 100, 0, 0, 0));
        }
    }
    keys.extend([
        key(100, 100, 350, 312, 0, 0, 0),
        key(100, 100, 450, 312, 1200, 450, 412),
        key(100, 150, 548, 283, 3000, 548, 433),
        key(100, 150, 748, 283, -3000, 848, 433),
        key(100, 100, 850, 312, -1200, 950, 412),
        key(100, 100, 950, 312, 0, 0, 0),
    ]);
    let small: Vec<keymap::KeyPhysicalAttrs> = keys
        .iter()
        .enumerate()
        .filter(|(i, _)| *i >= 36 || (i % 12 != 0 && i % 12 != 11))
        .map(|(_, k)| *k)
        .collect();
    keymap::PhysicalLayouts {
        active_layout_index: 0,
        layouts: vec![
            keymap::PhysicalLayout {
                name: "Eyelash 42".into(),
                keys,
            },
            keymap::PhysicalLayout {
                name: "Eyelash 36".into(),
                keys: small,
            },
        ],
    }
}

/// The Corne's layout as the pages take it from its Studio layout.
pub(crate) fn corne_from_keyboard() -> KeyLayout {
    let studio = crate::protocols::zmk::studio_layouts(&corne_studio());
    resolve(Some(&users_desk::board()), &[studio], 42)
}

fn count(l: &KeyLayout) -> usize {
    l.parts.iter().map(Vec::len).sum()
}

fn near(a: Rect, b: Rect) -> bool {
    (a.min - b.min).length() < 1e-4 && (a.max - b.max).length() < 1e-4
}

/// AC 1: with a Studio layout (live or the cached copy, which keeps the reply as it came), every
/// key the pages draw is the overlay's key: the same rectangle, the same turn.
#[test]
fn a_studio_layout_draws_the_overlays_key_rectangles() {
    let studio = corne_studio();
    let overlay = crate::protocols::zmk::physical_keys(&studio.layouts[0]);
    let layout = corne_from_keyboard();
    assert_eq!(layout.source, LayoutSource::Keyboard);
    assert_eq!(layout.name.as_deref(), Some("Eyelash 42"));
    assert_eq!(layout.choices, ["Eyelash 42", "Eyelash 36"]);
    assert!(!layout.mismatch);
    assert_eq!(
        layout.parts.iter().map(Vec::len).collect::<Vec<_>>(),
        [21, 21]
    );
    let mut seen: Vec<usize> = Vec::new();
    for (p, part) in layout.parts.iter().enumerate() {
        for k in part {
            let o = &overlay[k.index];
            let theirs = Rect::from_min_size(egui::pos2(o.x, o.y), egui::vec2(o.w, o.h));
            assert!(
                near(k.rect.translate(layout.origins[p]), theirs),
                "key {}: {:?} vs the overlay's {theirs:?}",
                k.index,
                k.rect.translate(layout.origins[p])
            );
            assert_eq!(k.r, o.r, "key {}", k.index);
            seen.push(k.index);
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..42).collect::<Vec<_>>(), "every key once");
    // The thumbs are turned, as on the board.
    assert!(layout.part(0).iter().any(|k| k.r == 30.0));
    assert!(layout.part(1).iter().any(|k| k.r == -30.0));
}

/// AC 3: the keyboard's keys take the profile's key order, so zones and the LED map (#611) keep
/// their indices: profile key 15 (T, the left's inner column top) is Studio's key 5, Space (20)
/// its inner thumb (38); the right half mirrors.
#[test]
fn the_keyboards_keys_keep_the_profiles_indices() {
    let layout = corne_from_keyboard();
    let index = |p: usize, i: usize| layout.part(p)[i].index;
    // Columns top to bottom, outer first.
    assert_eq!([index(0, 0), index(0, 1), index(0, 2)], [0, 12, 24]);
    assert_eq!([index(0, 15), index(0, 16), index(0, 17)], [5, 17, 29]);
    assert_eq!([index(0, 18), index(0, 19), index(0, 20)], [36, 37, 38]);
    assert_eq!([index(1, 0), index(1, 1), index(1, 2)], [11, 23, 35]);
    assert_eq!([index(1, 15), index(1, 20)], [6, 39]);
    // The shipped LED map's pixel 0 (T, G, B and Space) lights the inner column and its thumb.
    let lamp = &users_desk::board().lighting.unwrap().led_keys[0];
    let studio: Vec<usize> = lamp.keys().iter().map(|&k| index(0, k.into())).collect();
    assert_eq!(studio, [5, 17, 29, 38]);
}

/// AC 2: without a Studio layout the profile's drawing, key for key as `Drawing::keys` gives it.
/// A keyboard that sends no keys (an empty layout) falls back the same way.
#[test]
fn without_a_studio_layout_the_profiles_drawing_as_before() {
    let board = users_desk::board();
    let drawing = board.drawing.clone().unwrap();
    let empty = Supplied {
        source: LayoutSource::Keyboard,
        layouts: vec![("Default".into(), Vec::new())],
        active: 0,
    };
    for supplied in [vec![], vec![empty]] {
        let layout = resolve(Some(&board), &supplied, 42);
        assert_eq!(layout.source, LayoutSource::Profile);
        assert_eq!(layout.rects(0), drawing.keys(false));
        assert_eq!(layout.rects(1), drawing.keys(true));
        assert!(layout.parts.iter().flatten().all(|k| k.r == 0.0));
        let (w, h) = drawing.size();
        assert_eq!(layout.size, egui::vec2(w, h));
        assert_eq!(layout.source.chip(), "Layout from Foxglove's board profile");
        assert_eq!(layout.source.dot(), None);
        // Points stay where they are.
        assert_eq!(layout.point_from_profile(0, [1.25, 2.5]), [1.25, 2.5]);
    }
}

/// Step 4: no profile and no layout: a grid of the keymap's keys, with the amber dot and its line.
#[test]
fn an_unknown_board_is_a_guessed_grid() {
    let layout = resolve(None, &[], 36);
    assert_eq!(layout.source, LayoutSource::Guessed(36));
    assert_eq!(count(&layout), 36);
    assert_eq!(layout.source.chip(), "Shape guessed from 36 keys");
    assert_eq!(layout.source.dot(), Some(Dot::Warn));
    assert!(layout.source.note().is_some());
    // The keyboard's layout wins over the guess, as one piece without a profile to split it.
    let studio = crate::protocols::zmk::studio_layouts(&corne_studio());
    let layout = resolve(None, &[studio], 42);
    assert_eq!(layout.source, LayoutSource::Keyboard);
    assert_eq!((layout.parts.len(), count(&layout)), (1, 42));
    assert_eq!(layout.source.chip(), "Layout from the keyboard");
    assert_eq!(layout.source.dot(), Some(Dot::Ok));
}

/// The ladder takes the best source handed over, whatever the order (B2/B3 add definitions).
#[test]
fn the_keyboard_beats_a_definition() {
    let mut studio = crate::protocols::zmk::studio_layouts(&corne_studio());
    let mut definition = studio.clone();
    definition.source = LayoutSource::Definition(DefinitionFormat::Via);
    definition.layouts[0].0 = "From VIA".into();
    let layout = resolve(
        Some(&users_desk::board()),
        &[definition.clone(), studio.clone()],
        42,
    );
    assert_eq!(layout.source, LayoutSource::Keyboard);
    studio.layouts.clear();
    let layout = resolve(Some(&users_desk::board()), &[studio, definition], 42);
    assert_eq!(layout.source.chip(), "Layout from its VIA definition");
    assert_eq!(layout.name.as_deref(), Some("From VIA"));
}

/// Another key count than the profile's: the keyboard wins, in its own order, and says so.
#[test]
fn a_layout_with_other_keys_than_the_profile_is_drawn_and_flagged() {
    let mut studio = crate::protocols::zmk::studio_layouts(&corne_studio());
    studio.active = 1;
    let layout = resolve(Some(&users_desk::board()), &[studio], 36);
    assert!(layout.mismatch);
    assert_eq!(count(&layout), 36);
    assert_eq!(layout.name.as_deref(), Some("Eyelash 36"));
    assert!(hover(&layout).contains("(2 of 2)"), "{}", hover(&layout));
    assert!(hover(&layout).contains("zones from the profile may not match"));
    // In its own order, so a point isn't moved by a key order it doesn't have.
    assert_eq!(layout.point_from_profile(0, [1.0, 1.0]), [1.0, 1.0]);
    let first: Vec<usize> = layout.part(0).iter().map(|k| k.index).take(3).collect();
    assert_eq!(first, [0, 1, 2]);
}

/// A point saved on the profile's drawing (an LED Check zones placed) lands on the same key of
/// the keyboard's layout, and back.
#[test]
fn saved_points_move_with_their_key() {
    let board = users_desk::board();
    let profile = board.drawing.clone().unwrap().keys(false);
    let layout = corne_from_keyboard();
    let c = profile[15].center();
    let moved = layout.point_from_profile(0, [c.x, c.y]);
    let there = layout.part(0)[15].rect.center();
    assert!((egui::pos2(moved[0], moved[1]) - there).length() < 1e-4);
    let back = layout.point_to_profile(0, moved);
    assert!((egui::pos2(back[0], back[1]) - c).length() < 1e-4);
}

/// A turned key is hit where it is drawn, not where its unturned rectangle is.
#[test]
fn turned_keys_are_hit_as_drawn() {
    let k = LayoutKey::new(
        Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 3.0)),
        90.0,
        0,
    );
    assert!(k.contains(egui::pos2(-0.8, 1.5)), "turned onto its side");
    assert!(
        !k.contains(egui::pos2(0.5, 0.2)),
        "its unturned top is empty"
    );
    let b = k.bounds();
    assert!((b.width() - 3.0).abs() < 1e-4 && (b.height() - 1.0).abs() < 1e-4);
}

/// The keyboard's layout is handed over per keyboard: another keyboard connected doesn't take it.
#[test]
fn a_handed_over_layout_belongs_to_its_keyboard() {
    let studio = crate::protocols::zmk::studio_layouts(&corne_studio());
    set_for_test(0x1d50, 0x615e, Some(studio));
    assert_eq!(supplied_now().len(), 1);
    connected(0x1234, 0x0001);
    assert!(supplied_now().is_empty());
    connected(0x1d50, 0x615e);
    assert_eq!(supplied_now().len(), 1);
    set_for_test(0x1d50, 0x615e, None);
}

/// The chip is A1's flat status chip (#793, which tests its fill and missing outline), with the
/// source's words and hover.
#[test]
fn the_source_chip_is_the_flat_hint_chip() {
    let src = include_str!("key_layout.rs");
    let start = src.find("pub fn source_chip").unwrap();
    let end = src[start..].find("pub fn hover").unwrap() + start;
    assert!(src[start..end].contains("theme::hint_chip("));
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    let layout = corne_from_keyboard();
    let mut drawn = None;
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        drawn = Some(source_chip(ui, &layout).rect);
    });
    out.textures_delta.clear();
    assert!(drawn.is_some_and(|r| r.width() > 0.0));
    assert!(hover(&layout).contains("\u{201c}Eyelash 42\u{201d} (1 of 2)"));
}

/// What the sources handed over for keyboard (vid, pid), for other modules' tests.
pub(crate) fn handed_for(vid: u16, pid: u16) -> Vec<Supplied> {
    with_store(|s| {
        s.supplied
            .iter()
            .filter(|(id, _)| *id == (vid, pid))
            .map(|(_, l)| l.clone())
            .collect()
    })
}

/// The keyboard connected now, for other modules' tests.
pub(crate) fn connected_now() -> Option<(u16, u16)> {
    with_store(|s| s.keyboard)
}

/// #797: a definition's layout is kept for its own keyboard and leaves the connected one alone
/// (a VIA board is another keyboard beside the host-link one); withdrawn, it's gone, and only
/// its own step is withdrawn.
#[test]
fn a_definition_is_kept_beside_the_connected_keyboard_and_can_be_withdrawn() {
    set_for_test(0x1d50, 0x615e, Some(crate::protocols::zmk::studio_layouts(&corne_studio())));
    let mut via = crate::protocols::zmk::studio_layouts(&corne_studio());
    via.source = LayoutSource::Definition(DefinitionFormat::Via);
    from_definition(0x3434, 0x0250, via.clone());
    assert_eq!(connected_now(), Some((0x1d50, 0x615e)));
    assert_eq!(handed_for(0x3434, 0x0250), vec![via.clone()]);
    assert_eq!(supplied_now().len(), 1, "the connected keyboard's own only");
    withdraw(0x3434, 0x0250, LayoutSource::Keyboard);
    assert_eq!(handed_for(0x3434, 0x0250), vec![via], "another step stays");
    withdraw(0x3434, 0x0250, LayoutSource::Definition(DefinitionFormat::QmkInfo));
    assert!(handed_for(0x3434, 0x0250).is_empty());
    assert_eq!(handed_for(0x1d50, 0x615e).len(), 1);
}
