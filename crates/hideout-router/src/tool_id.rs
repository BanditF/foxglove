// SPDX-License-Identifier: MIT
//! Which program an outside tool is (S1, contract §6.4): the image file of the process that
//! opened the connection and that file's SHA-256. The name the tool gives itself is display
//! text and is never part of this. The process handle is opened when the connection is
//! accepted ([`PeerLookup::open`]) and the answer is read from that handle, so a pid reused
//! later cannot change who the connection was.

use std::path::{Path, PathBuf};

use hideout_contract::permission::tool_runs_scripts;
use sha2::{Digest, Sha256};

/// The program behind a connection: its canonical image path and the SHA-256 of that file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolId {
    pub image_path: String,
    pub image_sha256: String,
}

impl ToolId {
    /// Whether the hub may remember what this program was allowed: not a script runner.
    pub fn remembered(&self) -> bool {
        Path::new(&self.image_path)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| !tool_runs_scripts(n))
    }
}

/// Opens a handle on a connecting process, at connect time.
pub trait PeerLookup: Send + Sync {
    fn open(&self, pid: u32) -> Box<dyn PeerHold>;
}

/// What was opened at connect time. Reads the identity from that one handle; `None` is the
/// valid "unknown" state (no handle, no image, no hash).
pub trait PeerHold: Send {
    fn identity(&mut self) -> Option<ToolId>;
}

/// The system lookup: a Windows process handle; unknown anywhere else.
pub struct SystemLookup;

impl PeerLookup for SystemLookup {
    fn open(&self, pid: u32) -> Box<dyn PeerHold> {
        Box::new(SystemHold(foxglove_plugin::owner::PeerProcess::open(pid)))
    }
}

struct SystemHold(Option<foxglove_plugin::owner::PeerProcess>);

impl PeerHold for SystemHold {
    fn identity(&mut self) -> Option<ToolId> {
        let proc = self.0.as_ref()?;
        let image = canonical(&proc.image()?)?;
        let hash = sha256_of(&image)?;
        // The process must have run through the hash: its image is then still the one named.
        proc.alive().then(|| ToolId {
            image_path: image.to_string_lossy().into_owned(),
            image_sha256: hash,
        })
    }
}

/// The canonical path without the `\?\` prefix; `None` for a network path.
fn canonical(p: &Path) -> Option<PathBuf> {
    let c = std::fs::canonicalize(p).ok()?;
    let s = c.to_string_lossy();
    if s.starts_with(r"\?\UNC\") {
        return None;
    }
    Some(match s.strip_prefix(r"\?\") {
        Some(rest) => PathBuf::from(rest),
        None => c,
    })
}

/// The SHA-256 of the file, read while the file is open for reading only (on Windows that
/// keeps writers out while it is read).
fn sha256_of(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(1); // FILE_SHARE_READ
    }
    let mut f = opts.open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(path: &str) -> ToolId {
        ToolId {
            image_path: path.into(),
            image_sha256: "00".into(),
        }
    }

    #[test]
    fn runners_are_not_remembered_by_file_name() {
        for p in [
            r"C:\Python\python.exe",
            r"C:\x\PowerShell.EXE",
            r"C:\x\node.exe",
            r"C:\Windows\System32\cmd.exe",
        ] {
            assert!(!id(p).remembered(), "{p}");
        }
        assert!(id(r"C:\Tools\mytool.exe").remembered());
    }

    #[cfg(windows)]
    #[test]
    fn the_system_lookup_names_this_test_process() {
        let mut hold = SystemLookup.open(std::process::id());
        let me = hold.identity().expect("own identity");
        let exe = canonical(&std::env::current_exe().unwrap()).unwrap();
        assert_eq!(me.image_path, exe.to_string_lossy());
        assert_eq!(me.image_sha256, sha256_of(&exe).unwrap());
        assert_eq!(me.image_sha256.len(), 64);
    }
}
