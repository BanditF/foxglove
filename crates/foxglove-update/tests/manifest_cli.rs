//! The release workflow's `manifest` call (#1185): latest.json carries `exe_sha256`.
use std::process::Command;

fn run(name: &str, extra: &[&str]) -> (bool, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "foxglove-manifest-cli-{}-{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.msi"), b"msi").unwrap();
    std::fs::write(dir.join("notes.txt"), b"notes").unwrap();
    let ok = Command::new(env!("CARGO_BIN_EXE_foxglove-update-sign"))
        .current_dir(&dir)
        .args([
            "manifest",
            "--version",
            "1.2.3",
            "--msi",
            "a.msi",
            "--url",
            "https://x/foxglove-x64.msi",
            "--notes-file",
            "notes.txt",
        ])
        .args(extra)
        .status()
        .unwrap()
        .success();
    (ok, dir)
}

#[test]
fn the_manifest_carries_the_exe_hash_the_release_passes() {
    let hash = "AB".repeat(32);
    let (ok, dir) = run("good", &["--exe-sha256", &hash]);
    assert!(ok);
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("latest.json")).unwrap()).unwrap();
    assert_eq!(v["exe_sha256"], "ab".repeat(32));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_malformed_exe_hash_is_refused_and_nothing_is_written() {
    let (ok, dir) = run("bad", &["--exe-sha256", "not-a-hash"]);
    assert!(!ok);
    assert!(!dir.join("latest.json").exists());
    let _ = std::fs::remove_dir_all(dir);
}
