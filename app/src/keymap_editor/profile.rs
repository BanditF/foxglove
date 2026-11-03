//! Editor profiles (#579): which sidebar sections a firmware family's editor has, and whether
//! each one is always there, there only when the connected board reports what it needs, or
//! never there. The board's capabilities decide, not its model (the systems rule).
//!
//! The idea comes from upstream KeyPeek's `EditorProfile`, `FeatureManifest` and
//! `SupportLevel` (srwi/keypeek 16a047e and cae44c8, `keypeek-core/src/keymap_editor/
//! profile.rs`, GPL-3.0 like this app). #579 chose to port the idea rather than pull upstream's
//! crates: `app/` is a hard fork of KeyPeek d7f5a89. Only the ZMK profile exists so far; the QMK
//! editor keeps its own sections until it gets one.

use super::{SidebarItem, SidebarSection};
use crate::keyboard::Keyboard;

/// Whether a section is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
// Every ZMK section depends on the board; `Supported` and `Unsupported` are for the QMK profile.
#[allow(dead_code)]
pub enum SupportLevel {
    /// Always.
    Supported,
    /// When the connected board reports what it needs (a behaviour role, its own behaviours).
    Conditional,
    /// Never.
    Unsupported,
}

/// One section's support level.
pub struct FeatureManifest<T: 'static> {
    pub section: T,
    pub support: SupportLevel,
}

/// A firmware family's editor: its sidebar and what decides each section.
pub trait EditorProfile {
    type Section: SidebarItem + 'static;

    /// The sidebar, grouped.
    fn sidebar(&self) -> &'static [SidebarSection<Self::Section>];

    /// Each section's support level. A section with no entry isn't offered.
    fn manifest(&self) -> &'static [FeatureManifest<Self::Section>];

    /// What a `Conditional` section needs from the board.
    fn board_has(&self, section: Self::Section, keyboard: &Keyboard) -> bool;

    /// `section`'s support level, if the manifest lists it.
    fn support(&self, section: Self::Section) -> Option<SupportLevel> {
        self.manifest()
            .iter()
            .find(|m| m.section == section)
            .map(|m| m.support)
    }

    /// Whether `section` is offered for `keyboard`.
    fn available(&self, section: Self::Section, keyboard: &Keyboard) -> bool {
        offered(self.support(section), || self.board_has(section, keyboard))
    }
}

/// Whether a section with this support level is offered; `board_has` is asked only for a
/// `Conditional` one. A section the manifest doesn't list isn't offered.
pub fn offered(support: Option<SupportLevel>, board_has: impl FnOnce() -> bool) -> bool {
    match support {
        Some(SupportLevel::Supported) => true,
        Some(SupportLevel::Conditional) => board_has(),
        Some(SupportLevel::Unsupported) | None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, PartialEq, Debug)]
    enum S {
        Always,
        IfBoard,
        Never,
        Unlisted,
    }

    impl SidebarItem for S {
        fn label(self) -> &'static str {
            "s"
        }
        fn is_supported(self, _: &Keyboard) -> bool {
            true
        }
    }

    struct P(bool);

    impl EditorProfile for P {
        type Section = S;
        fn sidebar(&self) -> &'static [SidebarSection<S>] {
            &[]
        }
        fn manifest(&self) -> &'static [FeatureManifest<S>] {
            &[
                FeatureManifest {
                    section: S::Always,
                    support: SupportLevel::Supported,
                },
                FeatureManifest {
                    section: S::IfBoard,
                    support: SupportLevel::Conditional,
                },
                FeatureManifest {
                    section: S::Never,
                    support: SupportLevel::Unsupported,
                },
            ]
        }
        fn board_has(&self, _: S, _: &Keyboard) -> bool {
            self.0
        }
    }

    /// The manifest decides: Conditional follows the board; Unsupported and unlisted never show.
    #[test]
    fn the_manifest_decides_each_section() {
        for board in [true, false] {
            let p = P(board);
            let decide = |s| offered(p.support(s), || p.0);
            assert!(decide(S::Always));
            assert_eq!(decide(S::IfBoard), board);
            assert!(!decide(S::Never));
            assert!(!decide(S::Unlisted));
        }
    }
}
