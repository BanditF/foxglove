//! Foxglove's additions to the KeyPeek code. They live here, apart from upstream's files, so
//! pulling from srwi/keypeek stays easy (docs/keypeek-sync.md).

pub mod art_service;
pub mod keyboard_actions;
pub mod keyboard_settings;
pub mod keymap_cache;
pub mod last_device;
pub mod layer_names;
pub mod link;
pub mod overlay_profile;
pub mod palm_host;
pub mod profiles_ui;
pub mod reconnect;
pub mod repaint_trace;
pub mod screens;
mod screens_watch;
mod screens_settings;
pub mod screens_ui;
pub mod services;
pub mod art_cycle;
pub mod accent;
pub mod store;
pub mod api;
pub mod art_library;
pub mod den;
pub mod den_ui;
pub mod kit_trust;
pub mod kits;
pub mod art_manifest;
mod art_preview;
pub mod art_ui;
pub mod settings_ui;
pub mod startup;
pub mod tray;
pub mod connection;
pub mod layer_access;
pub mod foreground;

#[cfg(target_os = "windows")]
pub(crate) mod device_watch;

pub mod update;
pub mod via_host;
pub mod via_ui;
