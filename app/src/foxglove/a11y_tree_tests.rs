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
