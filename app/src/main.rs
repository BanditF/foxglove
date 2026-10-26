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
    if foxglove::update::helper_entry() { return; }
    if foxglove::identity::entry() { return; }
    // Foxglove (#345): one per user; a second launch shows the running one and exits 0.
    if !foxglove::single_instance::claim() { return; }
    foxglove::update::start();
    os_layout::init();
    foxglove::away::start(); // Foxglove (#410): locked or display off stops background clocks
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
    foxglove::keyboard_facts::start();
    foxglove::via_host::start();
    foxglove::plugin_install::autoinstall();
    foxglove::plugin_runner::sync();
    foxglove::plugin_actions::start();
    foxglove::plugin_settings::start();
    foxglove::app_windows::start();
    foxglove::mic_broker::start();
    foxglove::catalog_refresh::start();
    foxglove::identity::start();
    foxglove::startup::finish(platform::run(settings, available_devices))
}
