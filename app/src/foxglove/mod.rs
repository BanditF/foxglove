//! Foxglove's additions to the KeyPeek code. They live here, apart from upstream's files, so
//! pulling from srwi/keypeek stays easy (docs/keypeek-sync.md).

pub mod art_library;
mod art_preview;
pub mod art_service;
pub mod art_ui;
pub mod connection;
pub mod keyboard_actions;
pub mod keyboard_settings;
pub mod keymap_cache;
pub mod last_device;
pub mod layer_access;
pub mod layer_names;
pub mod link;
pub mod palm_host;
pub mod reconnect;
pub mod screens;
mod screens_settings;
pub mod screens_ui;
mod screens_watch;
pub mod services;
pub mod settings_ui;
pub mod startup;
pub mod tray;

#[cfg(target_os = "windows")]
pub(crate) mod device_watch;

pub mod update;
