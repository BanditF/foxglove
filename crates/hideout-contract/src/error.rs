//! Error ids (spec §15). The id is the contract; message text is free.

use std::fmt;

macro_rules! error_ids {
    ($($variant:ident => $id:literal,)*) => {
        /// A §15 error id. [`ErrorId::id`] is the string readers report (`error.id` in `--json`).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum ErrorId {
            $($variant,)*
        }

        impl ErrorId {
            /// Every id, in spec order.
            pub const ALL: &'static [ErrorId] = &[$(ErrorId::$variant,)*];

            pub fn id(self) -> &'static str {
                match self {
                    $(ErrorId::$variant => $id,)*
                }
            }

            pub fn from_id(id: &str) -> Option<ErrorId> {
                match id {
                    $($id => Some(ErrorId::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

error_ids! {
    ManifestSyntax => "manifest.syntax",
    ManifestUnknownKey => "manifest.unknown-key",
    ManifestMissingKey => "manifest.missing-key",
    ManifestId => "manifest.id",
    ManifestFamily => "manifest.family",
    ManifestKind => "manifest.kind",
    ManifestVersion => "manifest.version",
    ManifestHub => "manifest.hub",
    ManifestApi => "manifest.api",
    ManifestRun => "manifest.run",
    ManifestVisibility => "manifest.visibility",
    ManifestRequires => "manifest.requires",
    ManifestEnriches => "manifest.enriches",
    ManifestProvides => "manifest.provides",
    ManifestActivation => "manifest.activation",
    ManifestUi => "manifest.ui",
    ManifestWindowMissing => "manifest.window-missing",
    ManifestMcp => "manifest.mcp",
    ManifestMcpMethod => "manifest.mcp-method",
    ManifestPin => "manifest.pin",
    ManifestEdition => "manifest.edition",
    ManifestIdentity => "manifest.identity",
    ManifestActions => "manifest.actions",
    ManifestSettings => "manifest.settings",
    ManifestSecrets => "manifest.secrets",
    ManifestLimits => "manifest.limits",
    PermissionUnknown => "permission.unknown",
    PermissionArgument => "permission.argument",
    PermissionNever => "permission.never",
    PermissionProvider => "permission.provider",
    UiUndeclared => "ui.undeclared",
    UiLimits => "ui.limits",
    UiProp => "ui.prop",
    UiAction => "ui.action",
    LifecycleApi => "lifecycle.api",
    LifecycleRequires => "lifecycle.requires",
    LifecycleRequiredBy => "lifecycle.required-by",
    LifecycleDeclined => "lifecycle.declined",
    LifecycleEdition => "lifecycle.edition",
    LifecycleNoHub => "lifecycle.no-hub",
    InstallDevModeRequired => "install.dev-mode-required",
    ConfigNoPublicSource => "config.no-public-source",
    ConfigFolderUrl => "config.folder-url",
    ConfigSourceId => "config.source-id",
    ConfigUnknownAnchor => "config.unknown-anchor",
    ConfigWindows => "config.windows",
    TrustSignature => "trust.signature",
    TrustRootExpired => "trust.root-expired",
    TrustRootRollback => "trust.root-rollback",
    TrustScope => "trust.scope",
    TrustScopeOverlap => "trust.scope-overlap",
    TrustKeyReuse => "trust.key-reuse",
    TrustEpoch => "trust.epoch",
    TrustExpired => "trust.expired",
    TrustRollback => "trust.rollback",
    TrustUnknownArchive => "trust.unknown-archive",
    TrustYanked => "trust.yanked",
    TrustRevoked => "trust.revoked",
    TrustFiles => "trust.files",
    TrustMismatch => "trust.mismatch",
    TrustVisibility => "trust.visibility",
    TrustSideloadScope => "trust.sideload-scope",
}

impl fmt::Display for ErrorId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// A failed check: its §15 id and a message for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub id: ErrorId,
    pub message: String,
}

impl Error {
    pub fn new(id: ErrorId, message: impl Into<String>) -> Error {
        Error {
            id,
            message: message.into(),
        }
    }

    pub fn id(&self) -> &'static str {
        self.id.id()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.message)
    }
}

impl std::error::Error for Error {}

/// Shorthand for building an `Err`.
pub(crate) fn fail<T>(id: ErrorId, message: impl Into<String>) -> Result<T, Error> {
    Err(Error::new(id, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for e in ErrorId::ALL {
            assert_eq!(ErrorId::from_id(e.id()), Some(*e));
            assert!(seen.insert(e.id()));
        }
    }
}
