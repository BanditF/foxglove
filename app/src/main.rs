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
    foxglove::devices::start(); // Foxglove: find the host-link keyboard whenever a link connects
    foxglove::keyboard_facts::start();
    foxglove::battery::start(); // Foxglove (#372, #373)
    foxglove::peripherals::start(); // Foxglove (#504): Bluetooth and controller batteries
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
    foxglove::catalog_refresh::start();
    foxglove::identity::start();
    foxglove::autostart::start(); // Foxglove: Start with Windows
    let result = platform::run(settings, available_devices);
    foxglove::calibrate_ui::cancel(); // Foxglove: quitting mid-calibration puts the colours back
    foxglove::scenes_ui::quitting(); // Foxglove (#489): no profile starts a scene now
    foxglove::scene_run::stop(); // Foxglove (#485): a running scene gives the lights back
    foxglove::wallpaper_sync::quit(); // Foxglove (#493): the wallpapers go back to their own clocks
    foxglove::startup::finish(result)
}
