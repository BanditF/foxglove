//! Whether a build carries the sparse package identity (#179), and with which publisher.
//! Shared with `app/build.rs` (`#[path]`), which reads the environment and the machine-local
//! `tools/local.toml`; compiled into the app only for its tests.
//!
//! Precedence, per value: the environment variable when it is set and not empty
//! (`FOXGLOVE_SPARSE_IDENTITY=1` on, any other value off, e.g. `0`), then `[identity]` in
//! `tools/local.toml` (`sparse = true`, `publisher = "..."`), then off and the placeholder
//! publisher. Without a local.toml (the repo, CI, other PCs) identity stays off.

/// The sparse identity flag.
pub const SPARSE_VAR: &str = "FOXGLOVE_SPARSE_IDENTITY";
/// A dev certificate's subject, for the `<msix>` element and `identity::PUBLISHER`.
pub const PUBLISHER_VAR: &str = "FOXGLOVE_IDENTITY_PUBLISHER";

/// What a build gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Embed the `<msix>` element and turn registration on.
    pub sparse: bool,
    /// The publisher to name, or `None` for the checked-in placeholder.
    pub publisher: Option<String>,
    /// Where `sparse` came from: "env", "local.toml" or "default".
    pub source: &'static str,
}

/// Resolves the identity from `env` (a variable's value, if set) and the text of a
/// tools/local.toml, if one was found. A local.toml that doesn't parse counts as absent.
pub fn resolve(env: impl Fn(&str) -> Option<String>, local_toml: Option<&str>) -> BuildIdentity {
    let table: Option<toml::Table> = local_toml.and_then(|t| t.parse().ok());
    let local = |key: &str| table.as_ref()?.get("identity")?.get(key).cloned();
    let set = |var: &str| {
        env(var)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };

    let (sparse, source) = match set(SPARSE_VAR) {
        Some(v) => (v == "1", "env"),
        None => match local("sparse").and_then(|v| v.as_bool()) {
            Some(b) => (b, "local.toml"),
            None => (false, "default"),
        },
    };
    let publisher = set(PUBLISHER_VAR).or_else(|| {
        local("publisher")
            .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
    });
    BuildIdentity {
        sparse,
        publisher,
        source,
    }
}

/// The one-line `cargo:warning` for a release build without identity, or `None`.
pub fn release_warning(identity: &BuildIdentity, profile: &str) -> Option<String> {
    (profile == "release" && !identity.sparse).then(|| {
        format!(
            "Foxglove: sparse package identity is OFF in this release build (from {}); a \
             registered dev identity would be lost. Turn it on with {SPARSE_VAR}=1 or \
             [identity] sparse = true in tools/local.toml.",
            identity.source
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_ON: &str =
        "[identity]\nsparse = true\npublisher = \"CN=Foxglove Dev, O=BanditF\"\n";
    const LOCAL_OFF: &str = "[identity]\nsparse = false\n";

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn nothing_set_is_off() {
        let id = resolve(env(&[]), None);
        assert_eq!(
            id,
            BuildIdentity {
                sparse: false,
                publisher: None,
                source: "default"
            }
        );
        let other = resolve(env(&[]), Some("[keyboard]\nleft_serial = \"A1\"\n"));
        assert!(!other.sparse, "a local.toml without [identity] is off");
    }

    #[test]
    fn local_toml_turns_it_on_with_its_publisher() {
        let id = resolve(env(&[]), Some(LOCAL_ON));
        assert!(id.sparse);
        assert_eq!(id.source, "local.toml");
        assert_eq!(id.publisher.as_deref(), Some("CN=Foxglove Dev, O=BanditF"));
        assert!(!resolve(env(&[]), Some(LOCAL_OFF)).sparse);
    }

    #[test]
    fn the_environment_wins_both_ways() {
        let on = resolve(env(&[(SPARSE_VAR, "1")]), Some(LOCAL_OFF));
        assert!(on.sparse);
        assert_eq!(on.source, "env");
        let off = resolve(env(&[(SPARSE_VAR, "0")]), Some(LOCAL_ON));
        assert!(!off.sparse, "an explicit 0 opts out despite local.toml");
        assert_eq!(off.source, "env");
        let on_alone = resolve(env(&[(SPARSE_VAR, "1")]), None);
        assert!(on_alone.sparse);
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let id = resolve(
            env(&[(SPARSE_VAR, " "), (PUBLISHER_VAR, "")]),
            Some(LOCAL_ON),
        );
        assert!(id.sparse);
        assert_eq!(id.publisher.as_deref(), Some("CN=Foxglove Dev, O=BanditF"));
    }

    #[test]
    fn the_publisher_variable_beats_local_toml() {
        let id = resolve(env(&[(PUBLISHER_VAR, " CN=Other ")]), Some(LOCAL_ON));
        assert_eq!(id.publisher.as_deref(), Some("CN=Other"));
    }

    #[test]
    fn a_broken_or_mistyped_local_toml_is_off() {
        assert!(!resolve(env(&[]), Some("[identity\nsparse = true")).sparse);
        assert!(!resolve(env(&[]), Some("[identity]\nsparse = \"yes\"\n")).sparse);
    }

    #[test]
    fn release_builds_without_identity_warn() {
        let off = resolve(env(&[]), None);
        let warning = release_warning(&off, "release").unwrap();
        assert!(warning.contains("OFF"), "{warning}");
        assert!(!warning.contains('\n'), "one line: {warning}");
        assert_eq!(release_warning(&off, "debug"), None);
        assert_eq!(
            release_warning(&resolve(env(&[]), Some(LOCAL_ON)), "release"),
            None
        );
    }
}
