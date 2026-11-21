//! Screen-reader tree checks (#1376), window-free: a bare `egui::Context` with AccessKit on, the
//! tree read from the frame's output, as the other `enable_accesskit` tests do. Nothing here
//! opens a window or talks to the system's UI Automation.

use egui::accesskit::{Live, Node, Role, Toggled};

use super::scene_looks::{Kind, Look};
use super::scenes_ui::{self, tests as scenes_tests, Draft};
use super::shell::{self, Page};
use super::temperature_card::TempView;
use super::temperature_feed::Status;
use super::theme;

/// One frame of `add` in a fresh context with AccessKit on: every node of the tree.
fn nodes_of(add: impl FnOnce(&mut egui::Ui)) -> Vec<Node> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1100.0, 1200.0),
        )),
        ..Default::default()
    };
    let mut add = Some(add);
    let mut out = ctx.run_ui(raw, |ui| {
        if let Some(add) = add.take() {
            add(ui);
        }
    });
    out.textures_delta.clear();
    out.platform_output
        .accesskit_update
        .expect("a tree")
        .nodes
        .into_iter()
        .map(|(_, n)| n)
        .collect()
}

fn named<'a>(nodes: &'a [Node], role: Role, label: &str) -> Option<&'a Node> {
    nodes
        .iter()
        .find(|n| n.role() == role && n.label() == Some(label))
}

/// The Settings window's shell: its sidebar lists every page as a selectable entry, the
/// current one selected.
#[test]
fn the_settings_shell_lists_its_pages() {
    super::store::test_config_dir();
    shell::preview_page(Page::Scenes);
    let nodes = nodes_of(|ui| shell::frame(ui, |_| {}));
    for page in Page::ALL {
        let n = named(&nodes, Role::Button, page.label())
            .or_else(|| named(&nodes, Role::Tab, page.label()))
            .or_else(|| {
                nodes.iter().find(|n| {
                    n.label() == Some(page.label())
                        && n.supports_action(egui::accesskit::Action::Click)
                })
            })
            .unwrap_or_else(|| panic!("a sidebar entry named {}", page.label()));
        let selected = n.is_selected() == Some(true) || n.toggled() == Some(Toggled::True);
        assert_eq!(selected, page == Page::Scenes, "{}", page.label());
    }
}

/// Scenes with the Temperature look: the Effect row is a radio group with Temperature checked,
/// Start is a button, and the stepper's polite live region is there.
#[test]
fn the_scenes_page_has_an_effect_group_a_start_button_and_a_live_region() {
    super::store::test_config_dir();
    let v = scenes_ui::View {
        look: Look {
            kind: Kind::Temperature,
            ..Look::default()
        },
        running: None,
        sound: Default::default(),
        temperature: TempView {
            status: Status::Reading,
            hosted: true,
            live: false,
        },
        ..scenes_tests::sample_none()
    };
    let nodes = nodes_of(|ui| {
        scenes_ui::draw(ui, &v, &mut Draft::default());
    });
    assert!(
        named(&nodes, Role::RadioGroup, "Effect").is_some(),
        "a radio group named Effect"
    );
    let temperature =
        named(&nodes, Role::RadioButton, "Temperature").expect("the Temperature pill");
    assert_eq!(temperature.toggled(), Some(Toggled::True), "checked");
    let other = named(&nodes, Role::RadioButton, "Breathe").expect("another effect");
    assert_eq!(other.toggled(), Some(Toggled::False), "not checked");
    assert!(
        named(&nodes, Role::Button, "Start").is_some(),
        "a button named Start"
    );
    let live = nodes
        .iter()
        .filter(|n| n.live() == Some(Live::Polite))
        .count();
    assert_eq!(live, 1, "one polite live region");
}

/// The build keeps AccessKit's Windows adapter: fails if the feature is dropped from the Windows
/// target again (commit 23b354fa turned it off for the crash the
/// vendored accesskit_windows now avoids, #1376).
#[cfg(windows)]
#[test]
fn the_windows_adapter_is_compiled_in() {
    // Compile-time: the build fails if eframe loses its accesskit feature (#1376, commit 23b354fa).
    const { assert!(eframe::ACCESSKIT_ADAPTER) };
}

/// The vendored accesskit_windows returns the install error instead of panicking (Windows'
/// global atom table full makes `SetPropW` fail the same way an invalid window does). Uses a
/// window handle that is not a window; no window is created or touched.
#[cfg(windows)]
#[test]
fn a_failed_subclass_install_is_an_error_not_a_panic() {
    use egui::accesskit::{ActionHandler, ActionRequest, ActivationHandler, TreeUpdate};
    struct Quiet;
    impl ActivationHandler for Quiet {
        fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
            None
        }
    }
    impl ActionHandler for Quiet {
        fn do_action(&mut self, _: ActionRequest) {}
    }
    let hwnd = windows::Win32::Foundation::HWND(0x7FFF_FFF0 as *mut core::ffi::c_void);
    match accesskit_windows::SubclassingAdapter::try_new(hwnd, Quiet, Quiet) {
        Ok(_) => panic!("an invalid window cannot be subclassed"),
        Err((mut inert, _err)) => {
            // The inert adapter is harmless: it never has a tree, and dropping it is quiet.
            assert!(inert
                .update_if_active(|| -> TreeUpdate { unreachable!("inert") })
                .is_none());
        }
    }
    // The infallible constructor degrades the same way (one line on stderr, no panic).
    let _inert = accesskit_windows::SubclassingAdapter::new(hwnd, Quiet, Quiet);
}

/// The words of the polite live nodes in one frame of `add` in `ctx`.
fn live_words(ctx: &egui::Context, mut add: impl FnMut(&mut egui::Ui), time: f64) -> Vec<String> {
    let raw = egui::RawInput {
        time: Some(time),
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1100.0, 1200.0),
        )),
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| add(ui));
    out.textures_delta.clear();
    out.platform_output
        .accesskit_update
        .expect("a tree")
        .nodes
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| n.live() == Some(Live::Polite))
        .map(|n| n.label().unwrap_or_default().to_string())
        .collect()
}

/// A notice reaches the tree as the window's polite live region's name; the same words twice
/// are not said again, and a second one inside the gap waits for its turn.
#[test]
fn a_notice_reaches_the_tree_as_a_polite_update_once_and_not_too_often() {
    use super::announce;
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let frame = |ctx: &egui::Context, t: f64| live_words(ctx, announce::app, t);
    assert_eq!(frame(&ctx, 1.0), vec![String::new()]);
    announce::notice_at(&ctx, shell::KEYBOARD_ON, 1.0);
    assert_eq!(frame(&ctx, 1.0), vec![shell::KEYBOARD_ON.to_string()]);
    // A repeat and a quick change: the reader still hears the first.
    announce::notice_at(&ctx, shell::KEYBOARD_ON, 1.5);
    announce::notice_at(&ctx, shell::KEYBOARD_OFF, 1.6);
    assert_eq!(frame(&ctx, 2.0), vec![shell::KEYBOARD_ON.to_string()]);
    // Its time comes: the held one is said by the next frame.
    assert_eq!(frame(&ctx, 3.5), vec![shell::KEYBOARD_OFF.to_string()]);
}

/// The scene's state changes are said through the window's region: opening the page says
/// nothing, a start, a refusal and a stop each say their words.
#[test]
fn scenes_say_start_refusal_and_stop() {
    use super::announce;
    super::store::test_config_dir();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    let idle = scenes_ui::View {
        running: None,
        stopping: false,
        ..scenes_tests::sample_none()
    };
    let running = scenes_ui::View {
        running: Some("Breathe".into()),
        ..idle.clone()
    };
    let refused = scenes_ui::View {
        look: Look {
            kind: Kind::ScreenGlow,
            ..Look::default()
        },
        screen: scenes_ui::ScreenView {
            refused: true,
            ..idle.screen.clone()
        },
        ..idle.clone()
    };
    let mut t = 0.0;
    let mut show = |v: &scenes_ui::View| -> Option<String> {
        t += 10.0;
        // What the tree holds (not the memory): the one polite node's name.
        let words = live_words(
            &ctx,
            |ui| {
                scenes_ui::draw(ui, v, &mut Draft::default());
                announce::app(ui);
            },
            t,
        );
        assert_eq!(words.len(), 1, "one polite node: {words:?}");
        words.into_iter().next().filter(|w| !w.is_empty())
    };
    assert_eq!(show(&idle), None, "opening the page says nothing");
    assert_eq!(show(&running).as_deref(), Some("Scene started: Breathe"));
    assert_eq!(show(&idle).as_deref(), Some(scenes_ui::SAID_STOPPED));
    assert_eq!(show(&refused).as_deref(), Some(scenes_ui::SAID_REFUSED));
    // The refusal going away because another look was picked is not "stopped".
    assert_eq!(show(&idle).as_deref(), Some(scenes_ui::SAID_REFUSED));
}

/// The firmware update's states have words; its progress lines do not (no spam).
#[test]
fn the_firmware_update_is_spoken_by_state_not_by_progress() {
    use super::firmware_update::{spoken, View};
    assert_eq!(spoken(&View::Idle), None);
    assert_eq!(
        spoken(&View::Running(vec!["a".into()])),
        spoken(&View::Running(vec!["a".into(), "b".into()]))
    );
    assert!(spoken(&View::Failed {
        steps: vec![],
        message: "no drive".into(),
        recovery: String::new()
    })
    .unwrap()
    .ends_with("no drive"));
}

/// A context with AccessKit on and the app's fonts, as the tree tests above set one up.
fn ctx_with_tree() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut fonts = egui::FontDefinitions::default();
    crate::platform::add_phosphor_to_fonts(&mut fonts);
    theme::add_fonts(&mut fonts);
    ctx.set_fonts(fonts);
    ctx
}

/// The Settings shell itself carries the window's live region: one polite node, whose name is
/// the last notice. Fails if the shell stops drawing it.
#[test]
fn the_settings_shell_carries_the_live_region() {
    use super::announce;
    super::store::test_config_dir();
    shell::preview_page(Page::Settings);
    let ctx = ctx_with_tree();
    let frame = |t: f64| live_words(&ctx, |ui| shell::frame(ui, |_| {}), t);
    assert_eq!(
        frame(1.0),
        vec![String::new()],
        "one polite node, nothing said"
    );
    announce::notice_at(&ctx, shell::KEYBOARD_ON, 5.0);
    assert_eq!(frame(5.1), vec![shell::KEYBOARD_ON.to_string()]);
}

/// A state that changed while its page (or window) was out of sight is not announced when it
/// comes back: it was not seen in the frame before.
#[test]
fn a_change_made_out_of_sight_is_not_said_on_return() {
    use super::announce;
    let ctx = ctx_with_tree();
    let seen = |on: bool, t: f64| {
        live_words(
            &ctx,
            |ui| {
                announce::watch(ui.ctx(), "probe", on, |_, n| {
                    Some(if *n { "Back" } else { "Gone" }.to_string())
                });
                announce::app(ui);
            },
            t,
        )
    };
    assert_eq!(seen(true, 1.0), vec![String::new()]);
    // Seen in the next frame too: a change is said.
    assert_eq!(seen(false, 10.0), vec!["Gone".to_string()]);
    // Frames pass without the watch (another page, the window closed), the value changes, and
    // the watch comes back: silent.
    for t in [20.0, 21.0] {
        live_words(&ctx, announce::app, t);
    }
    live_words(&ctx, |_| {}, 22.0);
    // The region starts blank too (the window was not drawn in the frame before).
    assert_eq!(seen(true, 30.0), vec![String::new()], "nothing said");
}
