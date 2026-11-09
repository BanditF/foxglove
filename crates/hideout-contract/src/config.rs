//! `hideout.toml`'s `[[sources]]` and `[[trust.anchors]]` (§13.4, §13.6). Other tables of the
//! file are the hub's and are passed over here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{fail, Error, ErrorId};
use crate::module::Visibility;
use crate::scopes_overlap;

/// The anchor name of the Hideout root chain (§12.2).
pub const ROOT_ANCHOR: &str = "hideout-root";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Catalog,
    Git,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: SourceKind,
    pub visibility: Visibility,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    /// The anchor that signs it; none for a developer-mode folder.
    #[serde(default)]
    pub anchor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub id: String,
    pub kind: String,
    pub key: String,
    pub scope: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Trust {
    #[serde(default)]
    pub anchors: Vec<Anchor>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Config {
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub trust: Trust,
}

fn source_id_ok(id: &str) -> bool {
    let mut c = id.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase() || f.is_ascii_digit())
        && c.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, Error> {
        crate::check_toml_nesting(text)?;
        toml::from_str(text).map_err(|e| Error::new(ErrorId::ManifestSyntax, e.to_string()))
    }

    /// Checks sources and anchors. `root_scopes` maps each root family to its scope patterns.
    pub fn validate(&self, root_scopes: &BTreeMap<String, Vec<String>>) -> Result<(), Error> {
        for s in &self.sources {
            if !source_id_ok(&s.id) {
                return fail(ErrorId::ConfigSourceId, format!("bad source id {:?}", s.id));
            }
            if s.kind == SourceKind::Folder && s.url.is_some() {
                return fail(
                    ErrorId::ConfigFolderUrl,
                    format!("{}: a folder source has a path, not a url", s.id),
                );
            }
            if let Some(a) = &s.anchor {
                if a != ROOT_ANCHOR && !self.trust.anchors.iter().any(|t| &t.id == a) {
                    return fail(
                        ErrorId::ConfigUnknownAnchor,
                        format!("{}: unknown anchor {a}", s.id),
                    );
                }
            }
        }
        if !self.sources.is_empty()
            && !self
                .sources
                .iter()
                .any(|s| s.visibility == Visibility::Public)
        {
            return fail(ErrorId::ConfigNoPublicSource, "no public source");
        }
        for anchor in &self.trust.anchors {
            for pattern in &anchor.scope {
                for (family, scopes) in root_scopes {
                    if scopes.iter().any(|r| scopes_overlap(pattern, r)) {
                        return fail(
                            ErrorId::TrustScopeOverlap,
                            format!(
                                "anchor {}'s scope {pattern} overlaps the {family} root scope",
                                anchor.id
                            ),
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
