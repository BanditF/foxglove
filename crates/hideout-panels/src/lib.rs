// SPDX-License-Identifier: MIT
//! The Hideout hub's panel host (contract §5, design §4.3).
//!
//! - [`model`]: the panels the hub holds. `ui.register` replaces a module's panels in one step,
//!   `ui.patch` changes nodes in place (validated and coalesced to ~20 a second), facts fill
//!   bindings, and input becomes `ui.event` with optimistic values that a refusal reverts.
//! - [`view`]: view state (scroll offsets, selected tabs, open sections, text being typed) keyed by
//!   stable node ids, so a re-registration keeps it (§5.6).
//! - [`tree`]: the patch operations on a tree.
//! - [`assets`]: `ui.asset.put` images keyed by hash (§5.3).
//! - [`feed`]: the trait the router implements to feed panels in and take events out, plus an
//!   in-process channel pair.
//! - [`render`]: draws a panel in egui, each one in its own guarded scope (§5.5). Live streams
//!   (`canvas`, `image`, `text`, `markdown` with a `stream`) draw from [`streams`].
//! - [`streams`]: live streams (§5.4, H2.9): visible only, at most 30 frames a second, reopen
//!   and text rules; [`stream_io`] reads the channels.
//! - [`tray`]: the tray panel's tile grid in the hub's frame (v1.4), with Edit tiles.
//! - [`sections`]: the Keys and Settings sections the hub adds to a module's page (§4.7).
//! - [`keys`]: key capture for the Keys section (Set key, #991): the bindable chords.
//!
//! Clean-room: written from `docs/hideout-contract.md`, the Hideout design doc and the MIT crates
//! in `crates/`. It knows no module ids and no device code.

pub mod assets;
pub mod feed;
pub mod keys;
pub mod model;
pub mod render;
pub mod sections;
#[cfg(windows)]
pub mod stream_io;
mod stream_view;
pub mod streams;
pub mod tray;
pub mod tree;
pub mod view;

pub use egui;
pub use feed::{Inbound, KeyInvoker, Outbound, PanelFeed};
pub use hideout_contract::messages::{Badge, Visibility};
pub use hideout_contract::module::Slot;
pub use model::{
    state_words, when_true, FilePicker, HostedPanel, HubCall, Outgoing, PanelHost, PanelKey,
    PanelState,
};
pub use render::{key_chip, panel_ui, panel_ui_quiet, DrawReport, Glance, Go};
pub use sections::page_sections_ui;
pub use streams::{Look, StreamAct, StreamCommand, StreamKey, Streams};
pub use tray::{
    tile_ui, tile_ui_at, tray_panel_ui, TileAnnouncer, TilePhase, TileReport, TrayAction,
    TrayLayout,
};
pub use view::ViewState;
