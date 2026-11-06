//! "Load a layout…" (#798): window-free, with synthetic VIA, QMK and KLE files and a fake file
//! picker, so no dialog opens and no device is asked.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;
use crate::foxglove::key_layout::{of_keyboard, resolve, set_for_test};

/// A split 3+3 KLE board with a turned thumb key on each side.
pub(crate) const KLE: &str = r#"[{"name": "Tiny split"},
    ["Q", "W", "E", {"x": 2}, "I", "O", "P"],
    ["A", "S", "D", {"x": 2}, "K", "L", ";"],
    [{"x": 2, "r": 15, "rx": 3, "ry": 2}, "Space", {"r": -15, "rx": 5, "x": -1}, "Enter"]]"#;

const VIA: &str = r#"{"name": "Via Pad", "vendorId": "0xFEED", "productId": "0x0001",
    "matrix": {"rows": 2, "cols": 2},
    "layouts": {"keymap": [["0,0", "0,1"], [{"w": 2}, "1,0"]]}}"#;

const QMK: &str = r#"{"keyboard_name": "Qmk Pad", "layouts": {"LAYOUT": {"layout": [
    {"matrix": [0, 0], "x": 0, "y": 0}, {"matrix": [0, 1], "x": 1, "y": 0},
    {"matrix": [0, 2], "x": 2, "y": 0}]}}}"#;

/// Every test first points the store at the tests' own folder, never the user's settings.
fn isolated() {
    crate::foxglove::store::test_config_dir();
}

fn dir() -> PathBuf {
    let d = crate::foxglove::store::test_config_dir()
        .parent()
        .unwrap()
        .join("layout-files");
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn file(name: &str, text: &str) -> PathBuf {
    let p = dir().join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn saved() -> Saved {
    crate::foxglove::store::try_load(SECTION).unwrap_or_default()
}

/// Each test its own keyboard ids: the store file is shared by the tests of this process.
fn source(vid: u16, pid: u16) -> LayoutSource {
    of_keyboard(vid, pid, None).source
}

#[test]
fn via_qmk_and_kle_files_draw_the_keyboard_each_with_its_chip() {
    isolated();
    set_for_test(0xB301, 0x0001, None);
    for (pid, name, text, format, keys, chip) in [
        (
            1,
            "pad.json",
            VIA,
            DefinitionFormat::Via,
            3,
            "Layout from its VIA definition",
        ),
        (
            2,
            "info.json",
            QMK,
            DefinitionFormat::QmkInfo,
            3,
            "Layout from a QMK layout",
        ),
        (
            3,
            "tiny.json",
            KLE,
            DefinitionFormat::Kle,
            14,
            "Layout from its KLE file",
        ),
    ] {
        assert!(matches!(source(0xB301, pid), LayoutSource::Guessed(_)));
        load(0xB301, pid, &file(name, text)).unwrap();
        let l = of_keyboard(0xB301, pid, None);
        assert_eq!(l.source, LayoutSource::Definition(format), "{name}");
        assert_eq!(l.source.chip(), chip);
        assert_eq!(l.parts.iter().map(Vec::len).sum::<usize>(), keys, "{name}");
        assert!(loaded(0xB301, pid));
        let entry = &saved().keyboards[&format!("b301:{pid:04x}")];
        assert_eq!(entry.file, name);
        assert_eq!(entry.keys.len(), keys);
    }
    // The KLE board's turned thumbs keep their turn.
    let kle = of_keyboard(0xB301, 3, None);
    let turns: Vec<f32> = kle.parts[0].iter().map(|k| k.r).collect();
    assert!(turns.contains(&15.0) && turns.contains(&-15.0), "{turns:?}");
    assert!(crate::foxglove::key_layout::hover(&kle).contains("Tiny split \u{b7} tiny.json"));
}

/// The AC: the loaded layout wins over the guess on the next connect, without a restart.
#[test]
fn a_loaded_layout_wins_over_the_guess_on_the_next_connect() {
    isolated();
    // Another keyboard is connected while the layout is loaded for this one.
    set_for_test(0xB302, 0x0999, None);
    load(0xB302, 0x0002, &file("next.json", KLE)).unwrap();
    // It connects: `boards::connected` tells `key_layout`, and the page's layout is the file's.
    crate::foxglove::key_layout::connected(0xB302, 0x0002);
    let now = crate::foxglove::key_layout::current();
    assert_eq!(now.source, LayoutSource::Definition(DefinitionFormat::Kle));
    // Over a profile too: it is the ladder's second step.
    let corne = crate::foxglove::fixtures::users_desk::board();
    let supplied = of_keyboard(0xB302, 0x0002, None);
    assert_eq!(
        supplied.source,
        LayoutSource::Definition(DefinitionFormat::Kle)
    );
    let with_profile = of_keyboard(0xB302, 0x0002, Some(&corne));
    assert_eq!(
        with_profile.source,
        LayoutSource::Definition(DefinitionFormat::Kle)
    );
}

/// After a restart: `start` hands the saved layouts over before any keyboard connects.
#[test]
fn saved_layouts_are_handed_over_at_start() {
    isolated();
    crate::foxglove::store::test_config_dir();
    let kle = foxglove_via::layout_file::parse(Path::new("start.json"), KLE).unwrap();
    crate::foxglove::store::update::<Saved>(SECTION, |s| {
        s.keyboards
            .insert("b303:0003".into(), Entry::from_file("start.json", &kle));
        // One edited out of shape by hand is skipped, not drawn.
        s.keyboards.insert(
            "b303:0004".into(),
            Entry {
                format: "kle".into(),
                keys: vec![[0.0, 0.0, f32::NAN, 1.0, 0.0]],
                ..Entry::default()
            },
        );
    })
    .unwrap();
    // A fresh run: nothing handed over yet.
    set_for_test(0xB303, 0x0999, None);
    assert!(matches!(source(0xB303, 3), LayoutSource::Guessed(_)));
    start();
    assert_eq!(
        source(0xB303, 3),
        LayoutSource::Definition(DefinitionFormat::Kle)
    );
    assert!(matches!(source(0xB303, 4), LayoutSource::Guessed(_)));
    assert!(loaded(0xB303, 3));
}

#[test]
fn forgetting_goes_back_to_the_guess_and_out_of_the_store() {
    isolated();
    set_for_test(0xB304, 0x0999, None);
    load(0xB304, 0x0001, &file("forget.json", KLE)).unwrap();
    assert!(saved().keyboards.contains_key("b304:0001"));
    forget(0xB304, 0x0001).unwrap();
    assert!(matches!(source(0xB304, 1), LayoutSource::Guessed(_)));
    assert!(!saved().keyboards.contains_key("b304:0001"));
    assert!(!loaded(0xB304, 0x0001));
}

/// The scan's VIA definition for the same board, as `via_host` hands it over on each scan.
fn scanned_k5() -> Supplied {
    let (summary, layout) = crate::foxglove::via_host::tests::k5_pro_opened(0);
    crate::foxglove::via_host::layout_of(&summary, &layout).unwrap()
}

/// User intent beats auto-detect: a loaded file survives a rescan that hands a VIA definition
/// for the same vid:pid, and wins over it.
#[test]
fn a_loaded_file_survives_a_rescan_that_hands_a_via_definition() {
    isolated();
    let kb = (0xB308, 0x0001);
    set_for_test(0xB308, 0x0999, None);
    // The scan first, then the file, then rescans (each hands its definition over again).
    crate::foxglove::key_layout::from_definition(kb.0, kb.1, scanned_k5());
    load(kb.0, kb.1, &file("rescan.json", KLE)).unwrap();
    for _ in 0..2 {
        crate::foxglove::key_layout::from_definition(kb.0, kb.1, scanned_k5());
        assert_eq!(
            source(kb.0, kb.1),
            LayoutSource::Definition(DefinitionFormat::Kle)
        );
    }
    // A scan that no longer finds the board withdraws its definition, never the file.
    crate::foxglove::key_layout::withdraw(
        kb.0,
        kb.1,
        LayoutSource::Definition(DefinitionFormat::Via),
    );
    assert_eq!(
        source(kb.0, kb.1),
        LayoutSource::Definition(DefinitionFormat::Kle)
    );
    // Loaded before the scan, at start: the scan's hand-over still doesn't replace it.
    let kb2 = (0xB308, 0x0002);
    load(kb2.0, kb2.1, &file("first.json", KLE)).unwrap();
    crate::foxglove::key_layout::from_definition(kb2.0, kb2.1, scanned_k5());
    assert_eq!(
        source(kb2.0, kb2.1),
        LayoutSource::Definition(DefinitionFormat::Kle)
    );
    // And the page offers "Forget this layout" on it.
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    LAST_FORGET.set(None);
    frame(&ctx, &of_keyboard(kb2.0, kb2.1, None), Some(kb2), vec![]);
    assert!(LAST_FORGET.get().is_some());
}

/// After "Forget this layout" the scan's VIA definition draws the board again, with no rescan.
#[test]
fn after_forget_the_via_definition_draws_again() {
    isolated();
    let kb = (0xB309, 0x0001);
    set_for_test(0xB309, 0x0999, None);
    crate::foxglove::key_layout::from_definition(kb.0, kb.1, scanned_k5());
    load(kb.0, kb.1, &file("forget-via.json", KLE)).unwrap();
    assert_eq!(
        source(kb.0, kb.1),
        LayoutSource::Definition(DefinitionFormat::Kle)
    );
    forget(kb.0, kb.1).unwrap();
    let back = of_keyboard(kb.0, kb.1, None);
    assert_eq!(back.source, LayoutSource::Definition(DefinitionFormat::Via));
    assert_eq!(
        back.parts.iter().map(Vec::len).sum::<usize>(),
        scanned_k5().layouts[0].1.len(),
        "the K5 Pro's own keys"
    );
    // No forget link on the scan's own definition.
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    LAST_FORGET.set(None);
    frame(&ctx, &back, Some(kb), vec![]);
    assert!(LAST_FORGET.get().is_none());
}

/// The store is read-modify-write: a load keeps the section's other keyboards and the store's
/// other sections.
#[test]
fn a_load_keeps_the_other_keyboards_and_sections() {
    isolated();
    set_for_test(0xB305, 0x0999, None);
    let other = Saved {
        keyboards: [("0000:0000".to_string(), Entry::default())].into(),
    };
    crate::foxglove::store::save("b305_other", &other).unwrap();
    load(0xB305, 0x0001, &file("one.json", KLE)).unwrap();
    load(0xB305, 0x0002, &file("two.json", VIA)).unwrap();
    let s = saved();
    assert!(s.keyboards.contains_key("b305:0001") && s.keyboards.contains_key("b305:0002"));
    assert_eq!(
        crate::foxglove::store::try_load::<Saved>("b305_other"),
        Some(other)
    );
}

/// The AC: a bad file says why in one line, the size is bounded, and nothing is saved or drawn.
#[test]
fn a_bad_file_says_why_in_one_line_and_changes_nothing() {
    isolated();
    set_for_test(0xB306, 0x0999, None);
    let big = format!(
        "[[\"A\"]]{}",
        " ".repeat(foxglove_via::layout_file::MAX_BYTES)
    );
    for (name, text, says) in [
        ("layout.txt", KLE, "Couldn't load layout.txt: only .json files (VIA, QMK or KLE) can be loaded."),
        ("broken.json", "[[\"A\", ", "Couldn't load broken.json: it isn't valid JSON (EOF while parsing a value at line 1 column 7)."),
        ("other.json", r#"{"theme": "dark"}"#, "Couldn't load other.json: it isn't a VIA definition, a QMK keyboard.json / info.json or a KLE layout."),
        ("big.json", &big, "Couldn't load big.json: it's over 1024 KiB, too big for a layout."),
        ("empty.json", "[]", "Couldn't load empty.json: it has no keys."),
    ] {
        let e = load(0xB306, 0x0001, &file(name, text)).unwrap_err();
        assert_eq!(e, says);
        assert!(!e.contains('\n'));
    }
    let e = load(0xB306, 0x0001, &dir().join("gone.json")).unwrap_err();
    assert!(
        e.starts_with("Couldn't load gone.json: it can't be opened (") && !e.contains('\n'),
        "{e}"
    );
    assert!(!saved().keyboards.contains_key("b306:0001"));
    assert!(matches!(source(0xB306, 1), LayoutSource::Guessed(_)));
}

/// A picker that hands over `next` once it's been opened; counts the opens. No dialog.
struct Fake {
    opened: Rc<Cell<u32>>,
    next: Rc<RefCell<Option<PathBuf>>>,
}

impl Picker for Fake {
    fn open(&mut self) {
        self.opened.set(self.opened.get() + 1);
    }
    fn update(&mut self, _ctx: &egui::Context) {}
    fn take(&mut self) -> Option<PathBuf> {
        (self.opened.get() > 0)
            .then(|| self.next.borrow_mut().take())
            .flatten()
    }
}

fn frame(
    ctx: &egui::Context,
    layout: &KeyLayout,
    target: Option<(u16, u16)>,
    events: Vec<egui::Event>,
) {
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 300.0),
        )),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| {
        ui.horizontal(|ui| forget_link(ui, layout, target));
        note_row(ui, layout, target);
    });
    out.textures_delta.clear();
}

fn click(ctx: &egui::Context, layout: &KeyLayout, target: Option<(u16, u16)>, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(
        ctx,
        layout,
        target,
        vec![egui::Event::PointerMoved(at), button(true)],
    );
    frame(ctx, layout, target, vec![button(false)]);
}

/// The page: "Load a layout…" under a guessed drawing opens the picker for the focused keyboard;
/// the picked file draws it; a bad one says why under the drawing; "Forget this layout" undoes it.
#[test]
fn load_a_layout_through_the_page_with_a_fake_picker() {
    isolated();
    let ctx = crate::foxglove::pc_lights_ui::tests::test_ctx();
    let kb = (0xB307, 0x0001);
    set_for_test(kb.0, kb.1, None);
    let opened = Rc::new(Cell::new(0));
    let next = Rc::new(RefCell::new(None));
    set_picker(Box::new(Fake {
        opened: opened.clone(),
        next: next.clone(),
    }));
    let guessed = resolve(None, &[], 36);

    // No focused keyboard: the note, but nothing to load a layout for.
    LAST_LOAD.set(None);
    frame(&ctx, &guessed, None, vec![]);
    assert!(LAST_LOAD.get().is_none());

    // A bad file first: one line under the drawing, nothing loaded.
    *next.borrow_mut() = Some(file("notes.txt", KLE));
    frame(&ctx, &guessed, Some(kb), vec![]);
    let at = LAST_LOAD
        .get()
        .expect("the link under a guessed drawing")
        .center();
    click(&ctx, &guessed, Some(kb), at);
    assert_eq!(opened.get(), 1, "the (fake) picker opened");
    frame(&ctx, &guessed, Some(kb), vec![]);
    assert_eq!(
        message_for(kb).as_deref(),
        Some("Couldn't load notes.txt: only .json files (VIA, QMK or KLE) can be loaded.")
    );
    assert!(!loaded(kb.0, kb.1));

    // Then a KLE file: drawn from it, and the problem line goes.
    *next.borrow_mut() = Some(file("page.json", KLE));
    click(&ctx, &guessed, Some(kb), at);
    frame(&ctx, &guessed, Some(kb), vec![]);
    assert_eq!(opened.get(), 2);
    assert!(message_for(kb).is_none());
    let drawn = of_keyboard(kb.0, kb.1, None);
    assert_eq!(
        drawn.source,
        LayoutSource::Definition(DefinitionFormat::Kle)
    );

    // Its drawing has no note, so no link; "Forget this layout" sits by the chip.
    LAST_LOAD.set(None);
    LAST_FORGET.set(None);
    frame(&ctx, &drawn, Some(kb), vec![]);
    assert!(LAST_LOAD.get().is_none());
    let forget_at = LAST_FORGET.get().expect("the forget link").center();
    click(&ctx, &drawn, Some(kb), forget_at);
    assert!(!loaded(kb.0, kb.1));
    assert!(matches!(source(kb.0, kb.1), LayoutSource::Guessed(_)));
    // Another keyboard's loaded layout: no forget link for this one.
    LAST_FORGET.set(None);
    frame(&ctx, &drawn, Some((0xB307, 0x0002)), vec![]);
    assert!(LAST_FORGET.get().is_none());
}

/// The dialog shows only `.json` files, and the page's words stay as reviewed.
#[test]
fn the_dialog_filters_json_and_the_words_stay() {
    isolated();
    let src = include_str!("layout_files.rs");
    let dialog = src
        .split("impl Dialog {")
        .nth(1)
        .and_then(|s| s.split("impl Picker for Dialog").next())
        .unwrap();
    assert!(dialog.contains(".add_file_filter_extensions(FILTER, vec![\"json\"])"));
    assert!(dialog.contains(".default_file_filter(FILTER)"));
    assert_eq!(LOAD, "Load a layout\u{2026}");
    assert_eq!(FORGET, "Forget this layout");
    // Only the drawing: nothing here reaches a device.
    for word in ["via_host", "devices::", "hidapi", "send("] {
        let code = src.split("#[cfg(test)]\n#[path").next().unwrap();
        assert!(!code.contains(word), "{word}");
    }
}
