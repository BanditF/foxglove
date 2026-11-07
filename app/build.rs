/// Foxglove (#371): the commit, for "Copy diagnostics".
fn git_sha() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if let Some(sha) = sha.filter(|s| !s.is_empty()) {
        println!("cargo:rustc-env=FOXGLOVE_GIT_SHA={sha}");
    }
}

#[cfg(windows)]
#[path = "src/foxglove/identity_build.rs"]
mod identity_build;

/// This machine's tools/local.toml and its text, from where `scripts/local_config.py` looks:
/// `FOXGLOVE_LOCAL_TOML`, this checkout's `tools\`, then the workspace's (the folder holding the
/// main checkout). Only a file that exists is watched: one created later needs a build.rs
/// rerun (`touch app/build.rs`), as a watched missing file would rebuild the app every time.
#[cfg(windows)]
fn local_toml() -> Option<(std::path::PathBuf, String)> {
    use std::path::{Path, PathBuf};
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("FOXGLOVE_LOCAL_TOML").filter(|v| !v.is_empty()) {
        paths.push(p.into());
    }
    let app = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    if let Some(checkout) = app.parent() {
        paths.push(checkout.join("tools").join("local.toml"));
    }
    let common = std::process::Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(&app)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(ws) = common
        .as_deref()
        .and_then(|c| Path::new(c).parent()?.parent())
    {
        paths.push(ws.join("tools").join("local.toml"));
    }
    paths
        .into_iter()
        .find_map(|p| std::fs::read_to_string(&p).ok().map(|text| (p, text)))
}

/// Where sparse-identity builds put the `<msix>` element (#179).
#[cfg(windows)]
const MSIX_MARKER: &str = "<!--FOXGLOVE_MSIX-->";

/// For an XML attribute value.
#[cfg(windows)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(windows)]
fn main() {
    git_sha();
    let mut res = winresource::WindowsResource::new();
    res.set_icon("resources/icon.ico");
    // Foxglove (#447): every build embeds the application manifest (per-monitor v2 DPI, long
    // paths, Windows 10/11, Common Controls v6). The sparse identity prototype (#179) stays
    // opt-in at build time (the environment or this machine's tools/local.toml): only then does
    // the manifest get the <msix> element.
    println!("cargo:rustc-check-cfg=cfg(foxglove_sparse_identity)");
    println!("cargo:rerun-if-env-changed=FOXGLOVE_SPARSE_IDENTITY");
    println!("cargo:rerun-if-env-changed=FOXGLOVE_IDENTITY_PUBLISHER");
    println!("cargo:rerun-if-env-changed=FOXGLOVE_LOCAL_TOML");
    println!("cargo:rerun-if-changed=packaging/foxglove.exe.manifest");
    println!("cargo:rerun-if-changed=packaging/identity/msix.xml");
    let manifest = std::fs::read_to_string("packaging/foxglove.exe.manifest")
        .expect("Failed to read packaging/foxglove.exe.manifest.");
    // The environment, else this machine's tools/local.toml (#179: a PC with a registered dev
    // identity keeps it across rebuilds), else off.
    let local = local_toml();
    if let Some((path, _)) = &local {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let identity = identity_build::resolve(
        |v| std::env::var(v).ok(),
        local.as_ref().map(|(_, text)| text.as_str()),
    );
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if let Some(warning) = identity_build::release_warning(&identity, &profile) {
        println!("cargo:warning={warning}");
    }
    let msix = if identity.sparse {
        println!("cargo:rustc-cfg=foxglove_sparse_identity");
        let msix = std::fs::read_to_string("packaging/identity/msix.xml")
            .expect("Failed to read packaging/identity/msix.xml.");
        // A dev certificate's subject (scripts/dev-identity.ps1); identity.rs reads the same
        // variable, and the package is packed with it (pack-identity.ps1 -Publisher).
        match &identity.publisher {
            Some(p) => msix.replace(
                "publisher=\"CN=Foxglove Placeholder\"",
                &format!("publisher=\"{}\"", xml_escape(p)),
            ),
            None => msix,
        }
    } else {
        String::new()
    };
    // identity::PUBLISHER reads it with option_env!, so a local.toml publisher reaches it too.
    if let Some(p) = &identity.publisher {
        println!("cargo:rustc-env=FOXGLOVE_IDENTITY_PUBLISHER={p}");
    }
    assert!(
        manifest.contains(MSIX_MARKER),
        "manifest lost its {MSIX_MARKER} marker"
    );
    res.set_manifest(&manifest.replace(MSIX_MARKER, msix.trim()));
    res.compile().expect("Failed to embed Windows resources.");
}

#[cfg(not(windows))]
fn main() {
    git_sha();
}
