//! Foxglove's additions to the KeyPeek code. They live here, apart from upstream's files, so
//! pulling from srwi/keypeek stays easy (docs/keypeek-sync.md).

pub mod accent;
pub mod api;
// Foxglove (#981): an app's icon from its program file.
pub mod app_icon;
pub mod app_log;
#[cfg(target_os = "windows")]
mod app_manifest;
pub mod app_windows;
pub mod art_builtins;
pub mod art_cycle;
mod art_encode;
mod art_health;
pub mod art_keys_health;
pub mod art_library;
pub mod art_manifest;
pub mod art_pieces;
mod art_playlist;
mod art_polarity;
mod art_preview;
mod art_seen;
pub mod art_service;
mod art_sources;
mod art_triggers;
pub mod art_ui;
pub mod art_words;
pub mod autostart;
pub mod away;
pub mod battery;
pub mod battery_dim_ui;
pub mod behavior_catalog;
pub mod boards;
pub mod calibrate;
pub mod calibrate_ui;
pub mod catalog_refresh;
pub mod connection;
pub mod device_thumb;
pub mod devices;
pub mod diagnostics;
pub mod effect_controls;
pub mod firmware_update;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod focus;
// Foxglove (#801): the overlay and the wallpaper feed follow the focus, or a pin.
pub mod focus_pins;
pub mod focus_ui;
// Foxglove (#802): "Follow the keyboard I type on", Raw Input device handles only.
pub mod follow_typing;
pub mod foreground;
pub mod gpu_reset;
pub mod health;
pub mod health_notice;
pub mod hold_lease;
pub mod identity;
pub mod key_layout;
pub mod keyboard_actions;
#[cfg(test)]
mod keyboard_direction_vectors;
pub mod keyboard_facts;
// Foxglove (#799): the one focused keyboard the app shows.
pub mod keyboard_focus;
pub mod keyboard_page;
pub mod keyboard_settings;
#[cfg(test)]
mod keyboard_zone_vectors;
pub mod keymap_cache;
pub mod keymap_health;
pub mod keymap_layers;
pub mod keymap_session;
pub mod layout_files;
// Foxglove (#719): the knob points at master or an app's volume.
pub mod knob;
pub mod knob_health;
pub mod knob_hide;
pub mod knob_popup;
pub mod lamp_health;
pub mod lamp_hold;
pub mod lamp_loss;
pub mod last_device;
pub mod layer_access;
pub mod layer_names;
pub mod led_map;
pub mod led_map_confirm;
pub mod led_map_ui;
pub mod light_zones;
pub mod lighting_leases;
pub mod lights_owner;
pub mod lights_page;
pub mod lights_palette;
pub mod link;
pub mod live_art;
pub mod live_feed;
pub mod live_wire;
pub mod mic_broker;
pub mod notice;
pub mod notifications;
pub mod notifications_ui;
mod notify_windows;
pub mod other_keyboards;
pub mod overlay_grab;
pub mod overlay_opacity;
pub mod overlay_place;
pub mod overlay_profile;
pub mod overlay_restore;
pub mod overlay_shown;
mod palette_file;
mod panel_polarity_ui;
pub mod parts;
pub mod pause;
pub mod pc_lights;
pub mod pc_lights_add;
pub mod pc_lights_ui;
pub mod pc_state;
pub mod peripherals;
pub mod plugin_actions;
pub mod plugin_icons;
pub mod plugin_install;
pub mod plugin_runner;
pub mod plugin_settings;
pub mod plugin_trust;
pub mod plugins_ui;
pub mod profiles_ui;
pub mod program_name;
pub mod quit;
pub mod reassert;
pub mod reconnect;
pub mod repaint_trace;
pub mod scene_card;
pub mod scene_lamps;
pub mod scene_looks;
pub mod scene_output;
pub mod scene_room;
pub mod scene_run;
pub mod scene_triggers;
pub mod scenes_ui;
pub mod screen_roles;
pub mod screens;
mod screens_settings;
pub mod screens_ui;
mod screens_watch;
pub mod services;
pub mod settings_open;
pub mod settings_ui;
pub mod settings_window;
pub mod shell;
pub mod single_instance;
pub mod startup;
pub mod store;
pub mod store_split;
#[cfg(test)]
pub(crate) mod studio_fake;
pub mod studio_session;
pub mod taskbar;
pub mod theme;
#[cfg(test)]
mod theme_preview;
pub mod tray;
pub mod tray_panel;
pub mod troubleshooting;
pub mod typing_art;

#[cfg(target_os = "windows")]
pub(crate) mod device_watch;

pub mod update;
pub mod via_host;
pub mod via_ui;
pub mod wallpaper;
pub mod wallpaper_engine;
pub mod wallpaper_image;
pub mod wallpaper_manifest;
pub mod wallpaper_sync;
pub mod wallpaper_sync_health;
pub mod windows_theme;
pub mod zones_ui;
