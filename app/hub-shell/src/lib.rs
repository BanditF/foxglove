//! The hub shell, shared by Foxglove's app and Hideout's hub (`hideout.exe`, #932): the parts of
//! Foxglove's shell that are not about keyboards. Extracted, not copied, so both binaries keep
//! one copy (design: review/hideout-modular-design.md, Phase 2, H2.1).
//!
//! GPL-3.0-only like the app it came from (LICENSING.md).

pub mod aumid;
pub mod monitors;
pub mod placement;
pub mod single_instance;
pub mod taskbar;
pub mod theme;
pub mod tray_glyph;
