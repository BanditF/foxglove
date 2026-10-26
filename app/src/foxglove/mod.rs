//! Foxglove's additions to the KeyPeek code. They live here, apart from upstream's files, so
//! pulling from srwi/keypeek stays easy (docs/keypeek-sync.md).

pub mod accent;
pub mod api;
pub mod app_log;
pub mod app_windows;
pub mod art_cycle;
mod art_encode;
pub mod art_library;
mod art_library_view;
pub mod art_manifest;
mod art_preview;
pub mod art_service;
mod art_sources;
mod art_triggers;
pub mod art_ui;
pub mod autostart;
pub mod away;
pub mod battery;
pub mod boards;
pub mod catalog_refresh;
pub mod connection;
pub mod devices;
pub mod diagnostics;
pub mod firmware_update;
pub mod foreground;
pub mod gpu_reset;
pub mod identity;
pub mod keyboard_actions;
pub mod keyboard_facts;
pub mod keyboard_page;
pub mod keyboard_settings;
pub mod keymap_cache;
pub mod last_device;
pub mod layer_access;
pub mod layer_names;
pub mod lighting_leases;
pub mod lights_page;
pub mod link;
pub mod mic_broker;
pub mod overlay_profile;
pub mod pause;
pub mod plugin_actions;
pub mod plugin_icons;
pub mod plugin_install;
pub mod plugin_runner;
pub mod plugin_settings;
pub mod plugin_trust;
pub mod plugins_ui;
pub mod profiles_ui;
pub mod reconnect;
pub mod repaint_trace;
pub mod screens;
mod screens_settings;
pub mod screens_ui;
mod screens_watch;
pub mod services;
pub mod settings_ui;
pub mod shell;
pub mod single_instance;
pub mod startup;
pub mod store;
pub mod taskbar;
pub mod theme;
#[cfg(test)]
mod theme_preview;
pub mod tray;
pub mod tray_panel;

#[cfg(target_os = "windows")]
pub(crate) mod device_watch;

pub mod update;
pub mod via_host;
pub mod via_ui;
pub mod wallpaper;
