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

/// Where sparse-identity builds put the `<msix>` element (#179).
#[cfg(windows)]
const MSIX_MARKER: &str = "<!--FOXGLOVE_MSIX-->";

#[cfg(windows)]
fn main() {
    git_sha();
    let mut res = winresource::WindowsResource::new();
    res.set_icon("resources/icon.ico");
    // Foxglove (#447): every build embeds the application manifest (per-monitor v2 DPI, long
    // paths, Windows 10/11, Common Controls v6). The sparse identity prototype (#179) stays
    // opt-in at build time: only then does the manifest get the <msix> element.
    println!("cargo:rustc-check-cfg=cfg(foxglove_sparse_identity)");
    println!("cargo:rerun-if-env-changed=FOXGLOVE_SPARSE_IDENTITY");
    println!("cargo:rerun-if-changed=packaging/foxglove.exe.manifest");
    println!("cargo:rerun-if-changed=packaging/identity/msix.xml");
    let manifest = std::fs::read_to_string("packaging/foxglove.exe.manifest")
        .expect("Failed to read packaging/foxglove.exe.manifest.");
    let msix = if std::env::var_os("FOXGLOVE_SPARSE_IDENTITY").is_some_and(|v| v == "1") {
        println!("cargo:rustc-cfg=foxglove_sparse_identity");
        std::fs::read_to_string("packaging/identity/msix.xml")
            .expect("Failed to read packaging/identity/msix.xml.")
    } else {
        String::new()
    };
    assert!(manifest.contains(MSIX_MARKER), "manifest lost its {MSIX_MARKER} marker");
    res.set_manifest(&manifest.replace(MSIX_MARKER, msix.trim()));
    res.compile().expect("Failed to embed Windows resources.");
}

#[cfg(not(windows))]
fn main() {
    git_sha();
}
