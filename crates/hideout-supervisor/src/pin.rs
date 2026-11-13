// SPDX-License-Identifier: MIT
//! The program pin of a started module (#1155-B): the exe is opened once with sharing for reads
//! only, so while the pin lives nobody can write, rename or delete it (a sharing violation), and
//! the hash and the canonical path both come from that one handle. The pin is held from the
//! hash to the process start, and for as long as the instance runs.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use sha2::{Digest, Sha256};

/// An open, write-locked program file with its canonical path and SHA-256.
#[derive(Debug)]
pub struct Pin {
    file: std::fs::File,
    /// The canonical full path (no `\?\`), as the registry compares it.
    pub path: String,
    /// The SHA-256 of the bytes read through the pinned handle.
    pub sha256: String,
}

impl Pin {
    /// Opens `exe` write-locked and hashes it through that handle.
    pub fn open(exe: &Path) -> io::Result<Pin> {
        let mut file = open_locked(exe)?;
        let path = hideout_registry::keyboard::canonical_path(exe)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        file.seek(SeekFrom::Start(0))?;
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        let sha256 = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Ok(Pin { file, path, sha256 })
    }

    /// Hashes the pinned handle again (the bytes now), for a re-check at `initialize`.
    pub fn rehash(&self) -> io::Result<String> {
        let mut f = &self.file;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        f.seek(SeekFrom::Start(0))?;
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
}

#[cfg(windows)]
fn open_locked(exe: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ only: no writer, no rename, no delete while this handle is open.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(exe)
}

#[cfg(not(windows))]
fn open_locked(exe: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(exe)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir()
            .join("hideout-pin-tests")
            .join(format!("{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_pinned_file_cannot_be_written_renamed_or_deleted() {
        let d = temp("lock");
        let exe = d.join("foxglove.exe");
        std::fs::write(&exe, b"one").unwrap();
        let pin = Pin::open(&exe).unwrap();
        assert_eq!(pin.sha256, crate::sha256_file(&exe).unwrap());
        assert!(std::fs::write(&exe, b"two").is_err(), "overwrite");
        assert!(
            std::fs::rename(&exe, d.join("moved.exe")).is_err(),
            "rename"
        );
        assert!(std::fs::remove_file(&exe).is_err(), "delete");
        assert_eq!(pin.rehash().unwrap(), pin.sha256);
        drop(pin);
        std::fs::write(&exe, b"two").unwrap();
    }

    #[test]
    fn aliases_of_the_path_pin_the_same_canonical_file() {
        let d = temp("alias");
        let exe = d.join("Foxglove.exe");
        std::fs::write(&exe, b"x").unwrap();
        let want = Pin::open(&exe).unwrap().path;
        // Case and slash variants.
        let upper = std::path::PathBuf::from(exe.display().to_string().to_uppercase());
        let p = Pin::open(&upper).unwrap();
        assert!(hideout_registry::keyboard::same_path(&p.path, &want));
        let slashes = exe.display().to_string().replace('\\', "/");
        let p = Pin::open(Path::new(&slashes)).unwrap();
        assert!(hideout_registry::keyboard::same_path(&p.path, &want));
        // A symlink (skipped when the OS refuses it).
        let link = d.join("link.exe");
        if std::os::windows::fs::symlink_file(&exe, &link).is_ok() {
            let p = Pin::open(&link).unwrap();
            assert!(hideout_registry::keyboard::same_path(&p.path, &want));
        }
        // A directory junction (mklink /J, skipped when it fails).
        let real = d.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("foxglove.exe"), b"x").unwrap();
        let junction = d.join("junction");
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            let p = Pin::open(&junction.join("foxglove.exe")).unwrap();
            let r = Pin::open(&real.join("foxglove.exe")).unwrap();
            assert!(hideout_registry::keyboard::same_path(&p.path, &r.path));
            let _ = std::fs::remove_dir(&junction);
        }
        // An 8.3 short name (skipped when the volume makes none).
        let long = d.join("a long file name.exe");
        std::fs::write(&long, b"x").unwrap();
        let short = short_name(&long);
        if let Some(s) = short {
            let a = Pin::open(&s).unwrap();
            let b = Pin::open(&long).unwrap();
            assert!(hideout_registry::keyboard::same_path(&a.path, &b.path));
        }
    }

    /// The 8.3 name of `p` through `cmd /c for %I in (..) do @echo %~sI`; `None` when it has none.
    fn short_name(p: &Path) -> Option<std::path::PathBuf> {
        let out = std::process::Command::new("cmd")
            .arg("/c")
            .arg(format!("for %I in (\"{}\") do @echo %~sI", p.display()))
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let short = std::path::PathBuf::from(s);
        (short.exists()
            && short.display().to_string().to_lowercase() != p.display().to_string().to_lowercase())
        .then_some(short)
    }
}
