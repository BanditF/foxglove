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
//!   (`canvas`, `image`, `text` with a `stream`) draw as stubs until H2.9.
//!
//! Clean-room: written from `docs/hideout-contract.md`, the Hideout design doc and the MIT crates
//! in `crates/`. It knows no module ids and no device code.

pub mod assets;
pub mod feed;
pub mod model;
pub mod render;
pub mod tree;
pub mod view;

pub use egui;
pub use feed::{Inbound, Outbound, PanelFeed};
pub use hideout_contract::module::Slot;
pub use model::{state_words, HostedPanel, Outgoing, PanelHost, PanelKey, PanelState};
pub use render::{panel_ui, DrawReport};
pub use view::ViewState;
