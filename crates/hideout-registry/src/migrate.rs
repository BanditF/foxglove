// SPDX-License-Identifier: MIT
//! The copy engine for the Foxglove to Hideout data move (H5.2a, `design-h5-rebrand.md` §3, §4).
//!
//! **Dead code on purpose.** Nothing in the hub, the app or the legacy module's path
//! resolution calls [`migrate_foxglove`] yet; it is reachable only from the tests below. Wiring
//! it into a start path is H5.2b/c and needs the user's go.
//!
//! What it does: copies `foxglove.toml`, `config\` and `plugins\` from the old root into
//! `.staging` (a sibling of the target), writes `migrated.json` (source path, a sha256 per file,
//! time, version) inside it last, then finishes with one atomic rename onto the target.
//!
//! - The source is only ever read. The one exception is the `MOVED-TO-HIDEOUT.txt` note left in
//!   it after the rename succeeds.
//! - A crash leaves either no target or a complete one: a leftover `.staging` is discarded and
//!   the copy is retried on the next call. A target without `migrated.json` is refused, never
//!   overwritten.
//! - A second call with a finished target is a no-op.
//! - Symlinks and junctions (reparse points) inside the tree are skipped and listed; a reparse
//!   point as the source root or as one of the three entries is refused, so a planted link
//!   cannot pull files from elsewhere into the copy.
//! - Both roots are parameters: no environment or registry is read here. Refusing a redirected
//!   or packaged context is the caller's job (the redirect crate), with the roots it passes.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The marker file inside the target; its presence means the copy is complete.
pub const MARKER: &str = "migrated.json";
/// The staging folder, a sibling of the target (same volume, so the rename is atomic).
pub const STAGING: &str = ".staging";
/// The note left beside the old tree.
pub const NOTE: &str = "MOVED-TO-HIDEOUT.txt";
/// What is copied from the old root. `logs\` and anything else is left alone.
pub const ENTRIES: [&str; 3] = ["foxglove.toml", "config", "plugins"];

/// The content of `migrated.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The old root the files came from.
    pub source: String,
    /// Seconds since the Unix epoch when the copy finished.
    pub time: u64,
    /// The version of the program that copied.
    pub version: String,
    /// Relative path (forward slashes) to the sha256 of the file, hex.
    pub files: BTreeMap<String, String>,
    /// Reparse points left out, as relative paths.
    #[serde(default)]
    pub skipped: Vec<String>,
}

/// What a call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The tree was copied now.
    Copied(Manifest),
    /// A finished copy was already there; nothing was touched.
    AlreadyDone,
    /// The old root has none of the entries; nothing was created.
    NothingToCopy,
}

/// Copies the Foxglove data under `source_root` to `target_root` (see the module docs).
pub fn migrate_foxglove(
    source_root: &Path,
    target_root: &Path,
    version: &str,
) -> io::Result<Outcome> {
    if target_root.join(MARKER).is_file() {
        return Ok(Outcome::AlreadyDone);
    }
    if target_root.exists() {
        return Err(other(format!(
            "{} exists without {MARKER}; not overwriting",
            target_root.display()
        )));
    }
    let src_meta = match fs::symlink_metadata(long(source_root)) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Outcome::NothingToCopy),
        Err(e) => return Err(e),
    };
    if is_reparse(&src_meta) || !src_meta.is_dir() {
        return Err(other(format!(
            "{} is not a plain folder",
            source_root.display()
        )));
    }
    let parent = target_root
        .parent()
        .ok_or_else(|| other("target has no parent".into()))?;
    let staging = parent.join(STAGING);
    if fs::symlink_metadata(long(&staging)).is_ok() {
        // A crash left this; it is only ever our own copy, so discard it and start over.
        fs::remove_dir_all(long(&staging))?;
    }
    fs::create_dir_all(long(&staging))?;
    let mut manifest = Manifest {
        source: source_root.display().to_string(),
        time: 0,
        version: version.to_string(),
        files: BTreeMap::new(),
        skipped: Vec::new(),
    };
    let mut any = false;
    for name in ENTRIES {
        let from = source_root.join(name);
        let meta = match fs::symlink_metadata(long(&from)) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if is_reparse(&meta) {
            return Err(other(format!("{} is a link; refusing", from.display())));
        }
        any = true;
        if meta.is_dir() {
            copy_dir(&from, &staging.join(name), name, &mut manifest)?;
        } else {
            manifest
                .files
                .insert(name.to_string(), copy_file(&from, &staging.join(name))?);
        }
    }
    if !any {
        fs::remove_dir_all(long(&staging))?;
        return Ok(Outcome::NothingToCopy);
    }
    manifest.time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Written last: without it the old tree stays the one in use.
    let json = serde_json::to_vec_pretty(&manifest).map_err(|e| other(e.to_string()))?;
    fs::write(long(&staging.join(MARKER)), json)?;
    fs::rename(long(&staging), long(target_root))?;
    // Best effort and after the commit point: a failure here changes nothing about the copy.
    let _ = fs::write(
        long(&source_root.join(NOTE)),
        format!(
            "This data was copied to {} when Foxglove became Hideout. Nothing here was changed or deleted.\r\n",
            target_root.display()
        ),
    );
    Ok(Outcome::Copied(manifest))
}

fn copy_dir(from: &Path, to: &Path, rel: &str, m: &mut Manifest) -> io::Result<()> {
    fs::create_dir_all(long(to))?;
    let mut entries: Vec<_> = fs::read_dir(long(from))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let rel_child = format!("{rel}/{}", name.to_string_lossy());
        let meta = fs::symlink_metadata(long(&e.path()))?;
        if is_reparse(&meta) {
            m.skipped.push(rel_child);
        } else if meta.is_dir() {
            copy_dir(&e.path(), &to.join(&name), &rel_child, m)?;
        } else if meta.is_file() {
            m.files
                .insert(rel_child, copy_file(&e.path(), &to.join(&name))?);
        } else {
            m.skipped.push(rel_child);
        }
    }
    Ok(())
}

/// Copies one file and returns the sha256 of what was read, hex.
fn copy_file(from: &Path, to: &Path) -> io::Result<String> {
    let mut input = fs::File::open(long(from))?;
    let mut output = fs::File::create(long(to))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n])?;
    }
    output.sync_all()?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(windows)]
fn is_reparse(m: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & 0x400 != 0 || m.file_type().is_symlink()
}
#[cfg(not(windows))]
fn is_reparse(m: &fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

/// On Windows an absolute path gets the `\\?\` prefix so paths past 260 characters work.
#[cfg(windows)]
fn long(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with(r"\\?\") || !p.is_absolute() {
        return p.to_path_buf();
    }
    match s.strip_prefix(r"\\") {
        Some(unc) => PathBuf::from(format!(r"\\?\UNC\{}", unc.replace('/', r"\"))),
        None => PathBuf::from(format!(r"\\?\{}", s.replace('/', r"\"))),
    }
}
#[cfg(not(windows))]
fn long(p: &Path) -> PathBuf {
    p.to_path_buf()
}

fn other(m: String) -> io::Error {
    io::Error::other(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(long(&d));
        fs::create_dir_all(long(&d)).unwrap();
        d
    }
    fn put(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(long(p.parent().unwrap())).unwrap();
        fs::write(long(&p), data).unwrap();
    }
    fn old_tree(base: &Path) -> PathBuf {
        let s = base.join("Foxglove");
        put(&s, "foxglove.toml", b"a = 1\n");
        put(&s, "config/keymap.json", b"{\"k\":1}");
        put(&s, "config/art/one.bin", &[0u8, 1, 2, 255]);
        put(&s, "config/settings.json.migrated", b"old");
        put(&s, "plugins/p/plugin.toml", b"id = \"p\"");
        put(&s, "logs/foxglove.log", b"skip me");
        s
    }
    fn target(base: &Path) -> PathBuf {
        base.join("Hideout")
            .join("modules")
            .join("foxglove")
            .join("settings")
    }
    fn staging_of(dst: &Path) -> PathBuf {
        dst.parent().unwrap().join(STAGING)
    }
    fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(d: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in fs::read_dir(long(d)).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, root, out);
                } else {
                    let rel = p
                        .strip_prefix(long(root))
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.insert(rel, fs::read(long(&p)).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn copies_everything_byte_identical_with_hashes_and_skips_logs() {
        let base = temp("full");
        let src = old_tree(&base);
        let before = snapshot(&src);
        let dst = target(&base);
        let Outcome::Copied(m) = migrate_foxglove(&src, &dst, "0.4.0").unwrap() else {
            panic!()
        };
        assert_eq!(m.version, "0.4.0");
        assert_eq!(m.files.len(), 5);
        assert!(m.files.contains_key("config/settings.json.migrated"));
        assert!(!dst.join("logs").exists());
        for (rel, hash) in &m.files {
            let got = fs::read(dst.join(rel)).unwrap();
            assert_eq!(got, before[rel], "{rel}");
            let want: String = Sha256::digest(&got)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(&want, hash, "{rel}");
        }
        let on_disk: Manifest =
            serde_json::from_slice(&fs::read(dst.join(MARKER)).unwrap()).unwrap();
        assert_eq!(on_disk, m);
        assert!(!staging_of(&dst).exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn source_is_untouched_apart_from_the_note() {
        let base = temp("src");
        let src = old_tree(&base);
        let before = snapshot(&src);
        migrate_foxglove(&src, &target(&base), "0.4.0").unwrap();
        let mut after = snapshot(&src);
        assert!(after.remove(NOTE).is_some());
        assert_eq!(before, after);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn second_run_is_a_no_op() {
        let base = temp("idem");
        let src = old_tree(&base);
        let dst = target(&base);
        migrate_foxglove(&src, &dst, "0.4.0").unwrap();
        let first = snapshot(&dst);
        // Even a changed source is not copied again (that is H5.2c's reconcile).
        put(&src, "foxglove.toml", b"a = 2\n");
        assert_eq!(
            migrate_foxglove(&src, &dst, "0.4.1").unwrap(),
            Outcome::AlreadyDone
        );
        assert_eq!(first, snapshot(&dst));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn interrupted_staging_is_discarded_and_retried() {
        let base = temp("crash");
        let src = old_tree(&base);
        let dst = target(&base);
        put(&staging_of(&dst), "config/half.json", b"{");
        put(&staging_of(&dst), "stale.txt", b"x");
        assert!(!dst.exists());
        let Outcome::Copied(_) = migrate_foxglove(&src, &dst, "0.4.0").unwrap() else {
            panic!()
        };
        assert!(dst.join(MARKER).is_file());
        assert!(!dst.join("stale.txt").exists());
        assert!(!dst.join("config/half.json").exists());
        assert!(!staging_of(&dst).exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_failed_copy_leaves_no_target() {
        let base = temp("fail");
        let dst = target(&base);
        // The source root is a file, not a folder: refused before anything is created.
        let broken = base.join("Foxglove");
        fs::write(&broken, b"a file, not a folder").unwrap();
        assert!(migrate_foxglove(&broken, &dst, "0.4.0").is_err());
        assert!(!dst.exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn target_without_marker_is_refused_not_overwritten() {
        let base = temp("nomarker");
        let src = old_tree(&base);
        let dst = target(&base);
        put(&dst, "user.txt", b"mine");
        assert!(migrate_foxglove(&src, &dst, "0.4.0").is_err());
        assert_eq!(fs::read(dst.join("user.txt")).unwrap(), b"mine");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_or_empty_source_copies_nothing() {
        let base = temp("none");
        let dst = target(&base);
        assert_eq!(
            migrate_foxglove(&base.join("absent"), &dst, "0.4.0").unwrap(),
            Outcome::NothingToCopy
        );
        let empty = base.join("Foxglove");
        put(&empty, "logs/x.log", b"x");
        assert_eq!(
            migrate_foxglove(&empty, &dst, "0.4.0").unwrap(),
            Outcome::NothingToCopy
        );
        assert!(!dst.exists() && !staging_of(&dst).exists());
        let _ = fs::remove_dir_all(&base);
    }

    /// Makes `link` point at `real` as a junction (Windows, no privilege) or symlink (elsewhere).
    fn link_dir(real: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(real)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(real, link).is_ok()
        }
    }

    #[test]
    fn inner_reparse_points_are_skipped_and_listed() {
        let base = temp("junction");
        let src = old_tree(&base);
        let outside = base.join("outside");
        put(&outside, "secret.txt", b"not ours");
        assert!(
            link_dir(&outside, &src.join("config").join("link")),
            "cannot create a link"
        );
        let dst = target(&base);
        let Outcome::Copied(m) = migrate_foxglove(&src, &dst, "0.4.0").unwrap() else {
            panic!()
        };
        assert_eq!(m.skipped, vec!["config/link".to_string()]);
        assert!(!dst.join("config").join("link").exists());
        assert!(outside.join("secret.txt").is_file());
        // Removing the junction must not touch its target.
        let _ = fs::remove_dir(src.join("config").join("link"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_link_as_the_root_or_as_an_entry_is_refused() {
        let base = temp("rootlink");
        let real = old_tree(&base);
        let dst = target(&base);
        let root_link = base.join("RootLink");
        assert!(link_dir(&real, &root_link), "cannot create a link");
        assert!(migrate_foxglove(&root_link, &dst, "0.4.0").is_err());
        assert!(!dst.exists());

        let src2 = base.join("Foxglove2");
        put(&src2, "foxglove.toml", b"a");
        let elsewhere = base.join("elsewhere");
        put(&elsewhere, "x.json", b"{}");
        assert!(link_dir(&elsewhere, &src2.join("plugins")));
        assert!(migrate_foxglove(&src2, &dst, "0.4.0").is_err());
        assert!(!dst.exists());
        // The failed run left a staging folder; the retry discards it.
        let _ = fs::remove_dir(&root_link);
        let _ = fs::remove_dir(src2.join("plugins"));
        let Outcome::Copied(_) = migrate_foxglove(&src2, &dst, "0.4.0").unwrap() else {
            panic!()
        };
        assert!(elsewhere.join("x.json").is_file());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn long_paths_copy() {
        let base = temp("long");
        let src = base.join("Foxglove");
        put(&src, "foxglove.toml", b"a");
        let deep = format!(
            "config/{}/{}/{}/leaf.json",
            "d".repeat(90),
            "e".repeat(90),
            "f".repeat(90)
        );
        put(&src, &deep, b"deep");
        let dst = target(&base);
        let Outcome::Copied(m) = migrate_foxglove(&src, &dst, "0.4.0").unwrap() else {
            panic!()
        };
        assert!(m.files.contains_key(&deep));
        assert_eq!(fs::read(long(&dst.join(&deep))).unwrap(), b"deep");
        let _ = fs::remove_dir_all(long(&base));
    }
}
