// SPDX-License-Identifier: MIT
//! The Hideout release a module version needs (spec §12.4, v1.11): a catalog entry's optional
//! `min_hub_release`, and the page **Update Hideout** opens (§7.8). Only a link on the
//! allow-listed releases page is ever opened; anything else falls back to [`RELEASES_URL`].
//!
//! A `min_hub_release` that can't be read (not an object, a bad `version`, a link off the
//! allow-list) is ignored, never an error: it only picks a link, so it must never cost the
//! catalog (and the revocations it carries) its acceptance.

use serde::{Deserialize, Serialize};

/// Hideout's releases page: what Update Hideout opens when no usable `min_hub_release` names a
/// release (the hub has no updater of its own yet).
pub const RELEASES_URL: &str = "https://github.com/BanditF/foxglove/releases";

/// A `url` longer than this is not opened.
pub const MAX_URL_LEN: usize = 512;

/// A `version` longer than this is not read.
pub const MAX_VERSION_LEN: usize = 32;

/// A catalog entry's `min_hub_release` (v1.11): the first Hideout release that serves this
/// module version, and the page it is on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HubRelease {
    /// The Hideout release, e.g. `0.4.0` (digits and dots, an optional `-` suffix).
    pub version: String,
    /// The release's page. Opened only when [`link_allowed`]; otherwise [`RELEASES_URL`].
    pub url: String,
}

impl HubRelease {
    /// Reads a `min_hub_release` value. `None` for anything that isn't an object with a good
    /// `version` and a string `url` (unknown keys are ignored, as in any catalog entry). The
    /// `url` is kept as written; [`HubRelease::link`] decides whether it is opened.
    pub fn read(v: &serde_json::Value) -> Option<HubRelease> {
        let o = v.as_object()?;
        let version = o.get("version")?.as_str()?;
        let url = o.get("url")?.as_str()?;
        if !version_ok(version) {
            return None;
        }
        Some(HubRelease {
            version: version.to_string(),
            url: url.to_string(),
        })
    }

    /// The page Update Hideout opens for this release: its `url` when allowed, else the
    /// releases page.
    pub fn link(&self) -> &str {
        if link_allowed(&self.url) {
            &self.url
        } else {
            RELEASES_URL
        }
    }
}

/// The page **Update Hideout** opens: the release's own page when there is one and it is
/// allowed, else [`RELEASES_URL`].
pub fn update_url(release: Option<&HubRelease>) -> &str {
    release.map_or(RELEASES_URL, HubRelease::link)
}

/// Whether Update Hideout may open `url`: `https`, host `github.com` exactly, and the path
/// [`RELEASES_URL`]'s or under it (`/releases/tag/<tag>`, `/releases/latest`). After the
/// prefix only `/` and `A–Z a–z 0–9 . _ ~ + -` may follow, with no `..`, no empty segment and
/// no query or fragment, so no other host, user part, port or escape gets through.
pub fn link_allowed(url: &str) -> bool {
    if url.len() > MAX_URL_LEN {
        return false;
    }
    let Some(rest) = url.strip_prefix(RELEASES_URL) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(path) = rest.strip_prefix('/') else {
        return false;
    };
    if path.is_empty() {
        return true;
    }
    path.split('/').all(|seg| {
        !seg.is_empty()
            && seg != "."
            && !seg.contains("..")
            && seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'+' | b'-'))
    })
}

/// `MAJOR.MINOR` or `MAJOR.MINOR.PATCH` in digits, optionally followed by `-` and
/// `A–Z a–z 0–9 .`, at most [`MAX_VERSION_LEN`] bytes.
fn version_ok(v: &str) -> bool {
    if v.is_empty() || v.len() > MAX_VERSION_LEN {
        return false;
    }
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let core_ok = (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let pre_ok = pre
        .is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'));
    core_ok && pre_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allowed_links() {
        for ok in [
            RELEASES_URL,
            "https://github.com/BanditF/foxglove/releases/",
            "https://github.com/BanditF/foxglove/releases/tag/hideout-v0.4.0",
            "https://github.com/BanditF/foxglove/releases/tag/v0.4.0-rc.1",
            "https://github.com/BanditF/foxglove/releases/latest",
        ] {
            assert!(link_allowed(ok), "{ok}");
        }
        for bad in [
            "http://github.com/BanditF/foxglove/releases",
            "https://github.com.evil.example/BanditF/foxglove/releases",
            "https://github.com@evil.example/BanditF/foxglove/releases",
            "https://evil.example/https://github.com/BanditF/foxglove/releases",
            "https://github.com:8443/BanditF/foxglove/releases",
            "https://GitHub.com/BanditF/foxglove/releases",
            "https://github.com/BanditF/foxglove/releasesx",
            "https://github.com/BanditF/foxglove/releases.evil",
            "https://github.com/BanditF/foxglove/releases/../../other/repo",
            "https://github.com/BanditF/foxglove/releases/tag//x",
            "https://github.com/BanditF/foxglove/releases/tag/./x",
            "https://github.com/BanditF/foxglove/releases?x=1",
            "https://github.com/BanditF/foxglove/releases/tag/v1#top",
            "https://github.com/BanditF/foxglove/releases/tag/v%2e1",
            "https://github.com/BanditF/foxglove/releases/tag/v1 x",
            "https://github.com/BanditF/foxglove/releases/tag/v1\\x",
            "https://github.com/BanditF/foxglove/releases/tag/\u{202e}1",
            "https://github.com/BanditF/other/releases",
            "file:///C:/Windows/System32/calc.exe",
            "",
        ] {
            assert!(!link_allowed(bad), "{bad:?}");
        }
        let long = format!("{RELEASES_URL}/tag/{}", "a".repeat(MAX_URL_LEN));
        assert!(!link_allowed(&long));
    }

    #[test]
    fn reads_the_field_and_falls_back() {
        let tag = "https://github.com/BanditF/foxglove/releases/tag/hideout-v0.4.0";
        let r = HubRelease::read(&json!({"version": "0.4.0", "url": tag, "later": 1})).unwrap();
        assert_eq!(r.version, "0.4.0");
        assert_eq!(r.link(), tag);
        assert_eq!(update_url(Some(&r)), tag);
        let off =
            HubRelease::read(&json!({"version": "0.4.0", "url": "https://evil.example/"})).unwrap();
        assert_eq!(off.link(), RELEASES_URL);
        assert_eq!(update_url(None), RELEASES_URL);
        for bad in [
            json!(null),
            json!("0.4.0"),
            json!({"url": tag}),
            json!({"version": "0.4.0"}),
            json!({"version": 4, "url": tag}),
            json!({"version": "0.4.0", "url": 1}),
            json!({"version": "four", "url": tag}),
            json!({"version": "1", "url": tag}),
            json!({"version": "1.2.3.4", "url": tag}),
            json!({"version": "0.4.0-", "url": tag}),
            json!({"version": "0.4.0-rc 1", "url": tag}),
        ] {
            assert_eq!(HubRelease::read(&bad), None, "{bad}");
        }
        assert!(HubRelease::read(&json!({"version": "0.4", "url": tag})).is_some());
        assert!(HubRelease::read(&json!({"version": "0.4.0-rc.1", "url": tag})).is_some());
    }
}
