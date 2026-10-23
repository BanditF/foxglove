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
    os_layout::init();
    let settings = Settings::load().unwrap_or_default();
    let available_devices = discover_devices();
    foxglove::screens::start();
    foxglove::startup::finish(platform::run(settings, available_devices))
}
