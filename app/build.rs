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
fn main() {
    git_sha();
    let mut res = winresource::WindowsResource::new();
    res.set_icon("resources/icon.ico");
    // Foxglove: the sparse identity prototype (#179) is opt-in at build time, so ordinary
    // builds keep no manifest and no package identity.
    println!("cargo:rustc-check-cfg=cfg(foxglove_sparse_identity)");
    println!("cargo:rerun-if-env-changed=FOXGLOVE_SPARSE_IDENTITY");
    if std::env::var_os("FOXGLOVE_SPARSE_IDENTITY").is_some_and(|v| v == "1") {
        println!("cargo:rustc-cfg=foxglove_sparse_identity");
        println!("cargo:rerun-if-changed=packaging/identity/foxglove.exe.manifest");
        res.set_manifest_file("packaging/identity/foxglove.exe.manifest");
    }
    res.compile().expect("Failed to embed Windows resources.");
}

#[cfg(not(windows))]
fn main() {
    git_sha();
}
