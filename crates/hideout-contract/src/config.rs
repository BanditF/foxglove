//! `hideout.toml`'s `[[sources]]` and `[[trust.anchors]]` (§13.4, §13.6), and (spec v1.6) its
//! `[windows]` table of pop-out placement (§2.5), read on its own by [`Windows::parse`] so a bad
//! placement never blocks the sources. Other tables of the file are the hub's and are passed over.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{fail, Error, ErrorId};
use crate::module::{id_grammar_ok, item_id_ok, Visibility};
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

/// Places kept per page, one per screen layout (§2.5, spec v1.6). A fifth drops the oldest.
pub const PLACES_PER_PAGE: usize = 4;

/// A pop-out's window state when it was saved (§2.5, spec v1.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    Normal,
    Maximized,
    /// Snapped by Windows; the place is the snapped rectangle.
    Snapped,
}

/// One saved place of a pop-out: where it was in one screen layout (§2.5, spec v1.6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// The screen layout's id: 16 lowercase hex digits (§2.5).
    pub layout: String,
    /// The screen the window was on, by its display target path (§2.5).
    pub screen: String,
    /// Position from that screen's top-left, in device-independent units.
    pub x: f64,
    pub y: f64,
    /// Size in device-independent units, above 0.
    pub width: f64,
    pub height: f64,
    pub state: WindowState,
    /// When it was saved, `YYYY-MM-DDTHH:MM:SSZ` (UTC); the oldest goes first.
    pub saved: String,
}

/// What the hub keeps for one page (`[windows."<module id>/<panel id>"]`, spec v1.6).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageWindow {
    /// Open in its own window (restored as a pop-out, §2.5).
    #[serde(default)]
    pub popped_out: bool,
    #[serde(default)]
    pub keep_on_top: bool,
    /// At most [`PLACES_PER_PAGE`], one per layout.
    #[serde(default)]
    pub places: Vec<Place>,
}

/// `hideout.toml`'s `[windows]` table (§2.5, §7.3, spec v1.6): keys are `<module id>/<panel id>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Windows {
    #[serde(default)]
    pub windows: BTreeMap<String, PageWindow>,
}

fn utc_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

fn page_key_ok(key: &str) -> bool {
    matches!(key.split_once('/'), Some((m, p)) if !m.is_empty() && id_grammar_ok(m) && item_id_ok(p))
}

impl Windows {
    /// Reads only the `[windows]` table of a `hideout.toml`; every error is `config.windows`.
    pub fn parse(text: &str) -> Result<Windows, Error> {
        crate::check_toml_nesting(text)
            .map_err(|e| Error::new(ErrorId::ConfigWindows, e.message))?;
        let table: toml::Table =
            toml::from_str(text).map_err(|e| Error::new(ErrorId::ConfigWindows, e.to_string()))?;
        let Some(w) = table.get("windows") else {
            return Ok(Windows::default());
        };
        let windows = w
            .clone()
            .try_into()
            .map_err(|e: toml::de::Error| Error::new(ErrorId::ConfigWindows, e.to_string()))?;
        Ok(Windows { windows })
    }

    /// The §2.5 shape checks, per page. The hub drops a page that fails and keeps the rest.
    pub fn validate_page(key: &str, page: &PageWindow) -> Result<(), Error> {
        let bad = |why: String| fail(ErrorId::ConfigWindows, format!("windows.{key:?}: {why}"));
        if !page_key_ok(key) {
            return bad("the key is not <module id>/<panel id>".into());
        }
        if page.places.len() > PLACES_PER_PAGE {
            return bad(format!(
                "{} places, at most {PLACES_PER_PAGE}",
                page.places.len()
            ));
        }
        for (i, p) in page.places.iter().enumerate() {
            let hex = p.layout.len() == 16
                && p.layout
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
            if !hex {
                return bad(format!("place {i}: layout is not 16 lowercase hex digits"));
            }
            if page.places[..i].iter().any(|q| q.layout == p.layout) {
                return bad(format!("place {i}: layout {} twice", p.layout));
            }
            if p.screen.is_empty() {
                return bad(format!("place {i}: no screen"));
            }
            if !(p.x.is_finite() && p.y.is_finite()) {
                return bad(format!("place {i}: position is not a finite number"));
            }
            if !(p.width.is_finite() && p.height.is_finite() && p.width > 0.0 && p.height > 0.0) {
                return bad(format!("place {i}: size is not above 0"));
            }
            if !utc_ok(&p.saved) {
                return bad(format!("place {i}: saved is not YYYY-MM-DDTHH:MM:SSZ"));
            }
        }
        Ok(())
    }

    /// Checks every page; the first failure is returned.
    pub fn validate(&self) -> Result<(), Error> {
        for (key, page) in &self.windows {
            Windows::validate_page(key, page)?;
        }
        Ok(())
    }
}
