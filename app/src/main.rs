#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod connection;
mod device_discovery;
mod foxglove;
mod hid_labels;
mod key_action;
mod key_matrix;
mod key_paint;
mod keyboard;
mod keymap_editor;
mod layout_key;
mod monitors;
mod os_layout;
mod overlay_window;
mod platform;
mod protocols;
mod qmk_keycode_labels;
mod settings;
mod tray;
mod ui_wake;
mod ui_widgets;
mod zmk_keycode_labels;

use device_discovery::discover_devices;
use settings::Settings;

fn main() {
    // Foxglove (#370): stderr and panics to logs\foxglove.log, first, so an early exit is seen.
    foxglove::app_log::start();
    if foxglove::update::helper_entry() { return; }
    if foxglove::identity::entry() { return; }
    if foxglove::autostart::uninstall_entry() { return; } // Foxglove (#444): MSI uninstall
    // Foxglove (#345): one per user; a second launch shows the running one and exits 0.
    if !foxglove::single_instance::claim() { return; }
    foxglove::update::start();
    os_layout::init();
    foxglove::gpu_reset::start(); // Foxglove (#346): a fresh copy after a GPU reset
    foxglove::away::start(); // Foxglove (#410): locked or display off stops background clocks
    foxglove::calibrate::load(); // Foxglove: each device's colour calibration, before any write
    let settings = foxglove::overlay_profile::effective(Settings::load().unwrap_or_default());
    let available_devices = discover_devices();
    if !foxglove::repaint_trace::disabled("screens") {
        foxglove::screens::start();
    }
    if !foxglove::repaint_trace::disabled("art") {
        foxglove::art_service::start();
    }
    foxglove::store::init_profiles();
    foxglove::foreground::start();
    foxglove::accent::start();
    foxglove::wallpaper::start();
    foxglove::api::start();
    foxglove::reassert::start(); // Foxglove (#618): re-apply device state after events that lose it
    foxglove::pc_state::start(); // Foxglove (#609, #604): the PC awake or away, for the keyboard's idle
    foxglove::devices::start(); // Foxglove: find the host-link keyboard whenever a link connects
    foxglove::keyboard_facts::start();
    foxglove::battery::start(); // Foxglove (#372, #373)
    foxglove::peripherals::start(); // Foxglove (#504): Bluetooth and controller batteries
    foxglove::live_feed::start(); // Foxglove (#499): the live wallpaper feed, when turned on
    foxglove::via_host::start();
    foxglove::pc_lights::start(); // Foxglove (#423): case and peripheral lights
    foxglove::scene_run::start_from_env(); // Foxglove (#485): the live check's demo scene
    foxglove::scene_triggers::start(); // Foxglove (#489): layer sweeps
    foxglove::scenes_ui::startup(); // Foxglove (#489): "Start with this profile"
    foxglove::plugin_install::autoinstall();
    foxglove::plugin_runner::sync();
    foxglove::plugin_actions::start();
    foxglove::plugin_settings::start();
    foxglove::app_windows::start();
    foxglove::mic_broker::start();
    foxglove::notifications::start(); // Foxglove (#507): notifications on the left screen
    foxglove::catalog_refresh::start();
    foxglove::identity::start();
    foxglove::autostart::start(); // Foxglove: Start with Windows
    let result = platform::run(settings, available_devices);
    // Foxglove (#618): an exit through the window loop (not the tray's Quit) says so, so a
    // test app that ended is told apart from one that crashed.
    if result.is_ok() {
        foxglove::app_log::early("Foxglove: the overlay window closed; quitting");
    }
    // Foxglove (#618): the panel, lights, art and wallpapers go back, within 4 s (quit.rs).
    foxglove::quit::hand_back("the window closed");
    foxglove::startup::finish(result)
}
