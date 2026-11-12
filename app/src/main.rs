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
    // Foxglove (#1143 P5): `--module` (the hub starts the app so) keeps the hub's launch values in
    // memory and clears them from the environment, before any thread exists. Without the flag
    // this does nothing.
    let module = foxglove::hub_link::enter();
    // Foxglove (#370): stderr and panics to logs\foxglove.log, first, so an early exit is seen.
    foxglove::app_log::start();
    if let Err(why) = module {
        // Never a fallback to the user's own pipe or a tray: say why and stop.
        foxglove::app_log::early(&why);
        std::process::exit(2);
    }
    if foxglove::update::helper_entry() {
        return;
    }
    if foxglove::identity::entry() {
        return;
    }
    if foxglove::autostart::uninstall_entry() {
        return;
    } // Foxglove (#444): MSI uninstall
      // Foxglove (#345): one per user; a second launch shows the running one and exits 0.
    if !foxglove::single_instance::claim(!foxglove::settings_open::tray_start()) {
        return;
    }
    // Foxglove: settings and log redirected into another app's package (started from it)?
    foxglove::store_split::start();
    foxglove::update::start();
    os_layout::init();
    foxglove::gpu_reset::start(); // Foxglove (#346): a fresh copy after a GPU reset
    foxglove::away::start(); // Foxglove (#410): locked or display off stops background clocks
    foxglove::calibrate::load(); // Foxglove: each device's colour calibration, before any write
    let settings = foxglove::overlay_profile::effective(Settings::load().unwrap_or_default());
    let available_devices = discover_devices();
    foxglove::features::start_all(); // Foxglove (#1143): every feature's start, in the table's order
    foxglove::hub_link::start(); // Foxglove (#1143 P5): with --module, the keyboard's facts to the hub (a no-op otherwise; after the table, whose order is unchanged)
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
