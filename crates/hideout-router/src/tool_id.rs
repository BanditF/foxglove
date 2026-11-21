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

    /// The peer's token facts read from this same handle (integrity, AppContainer), when the
    /// hold can: the transport's admission check and the identity then name one process
    /// handle, and a pid reused in between cannot change who was checked. `None`: not
    /// available here.
    #[cfg(windows)]
    fn token(&self) -> Option<std::io::Result<hideout_acl::Peer>> {
        None
    }
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
        let named = canonical(&proc.image()?)?;
        // One handle on the file serves its path and its hash, and the process is asked again
        // for its image while that handle is open: the file read is the file it runs.
        let mut file = open_for_hash(&named)?;
        let held = plain_local(&final_path(&file)?.to_string_lossy())?.to_string();
        if !held.eq_ignore_ascii_case(&named.to_string_lossy()) {
            return None;
        }
        let hash = hash_bounded(&mut file, HASH_MAX_BYTES, HASH_MAX_TIME)?;
        let again = canonical(&proc.image()?)?;
        if !again.to_string_lossy().eq_ignore_ascii_case(&held) {
            return None;
        }
        Some(ToolId {
            image_path: held,
            image_sha256: hash,
        })
    }

    #[cfg(windows)]
    fn token(&self) -> Option<std::io::Result<hideout_acl::Peer>> {
        self.0.as_ref().map(|p| p.peer())
    }
}

/// The largest program file the hub will read to tell a program: a bigger one is not told.
pub const HASH_MAX_BYTES: u64 = 512 * 1024 * 1024;
/// The longest the hub spends reading one program file.
pub const HASH_MAX_TIME: std::time::Duration = std::time::Duration::from_secs(10);

/// The canonical path without the `\\?\` prefix; `None` for a network path.
fn canonical(p: &Path) -> Option<PathBuf> {
    let c = std::fs::canonicalize(p).ok()?;
    plain_local(&c.to_string_lossy()).map(PathBuf::from)
}

/// A canonical path as plain text: the `\\?\` prefix dropped. `None` for a network path, in
/// either spelling (`\\?\UNC\server\share` or `\\server\share`).
fn plain_local(s: &str) -> Option<&str> {
    if s.starts_with(r"\\?\UNC\") {
        return None;
    }
    let plain = s.strip_prefix(r"\\?\").unwrap_or(s);
    (!plain.starts_with(r"\\")).then_some(plain)
}

/// The file opened for reading only: on Windows no one can write to it while it is open.
fn open_for_hash(p: &Path) -> Option<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        opts.share_mode(1); // FILE_SHARE_READ
    }
    opts.open(p).ok()
}

/// The path the open handle names (the file itself, not what was asked for).
#[cfg(windows)]
fn final_path(f: &std::fs::File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
    let mut buf = vec![0u16; 1024];
    // SAFETY: a valid file handle and a buffer of the length given.
    let n = unsafe {
        GetFinalPathNameByHandleW(
            f.as_raw_handle() as _,
            buf.as_mut_ptr(),
            buf.len() as u32,
            0,
        )
    } as usize;
    (n != 0 && n <= buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])))
}

#[cfg(not(windows))]
fn final_path(_: &std::fs::File) -> Option<PathBuf> {
    None
}

/// The SHA-256 of what `f` holds, read from where it is. `None` when the file is longer than
/// `max_bytes` or reading it takes longer than `max_time`: a program whose file is too big or
/// too slow to read is not told (it may only read).
fn hash_bounded(
    f: &mut std::fs::File,
    max_bytes: u64,
    max_time: std::time::Duration,
) -> Option<String> {
    use std::io::Read;
    if f.metadata().ok()?.len() > max_bytes {
        return None;
    }
    let start = std::time::Instant::now();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max_bytes || start.elapsed() > max_time {
            return None;
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

    /// A network image has no identity, and a local one loses the `\\?\` prefix.
    #[test]
    fn a_network_image_has_no_identity() {
        assert_eq!(plain_local(r"\\?\UNC\server\share\tool.exe"), None);
        assert_eq!(plain_local(r"\\server\share\tool.exe"), None);
        assert_eq!(
            plain_local(r"\\?\C:\Tools\tool.exe"),
            Some(r"C:\Tools\tool.exe")
        );
        assert_eq!(
            plain_local(r"C:\Tools\tool.exe"),
            Some(r"C:\Tools\tool.exe")
        );
    }

    #[cfg(windows)]
    #[test]
    fn the_system_lookup_names_this_test_process() {
        let own = canonical(&std::env::current_exe().unwrap()).unwrap();
        assert!(!own.to_string_lossy().starts_with(r"\\"), "{own:?}");
        let mut hold = SystemLookup.open(std::process::id());
        let me = hold.identity().expect("own identity");
        let exe = canonical(&std::env::current_exe().unwrap()).unwrap();
        assert_eq!(me.image_path, exe.to_string_lossy());
        let mut f = open_for_hash(&exe).unwrap();
        assert_eq!(
            me.image_sha256,
            hash_bounded(&mut f, HASH_MAX_BYTES, HASH_MAX_TIME).unwrap()
        );
        assert_eq!(me.image_sha256.len(), 64);
    }

    /// A file over the size cap, or one that takes longer than the time bound, is not hashed.
    #[test]
    fn hashing_is_bounded_in_size_and_time() {
        let p = std::env::temp_dir().join(format!("hideout-hash-{}.bin", std::process::id()));
        std::fs::write(&p, vec![7u8; 200_000]).unwrap();
        let big = std::time::Duration::from_secs(60);
        let mut f = open_for_hash(&p).unwrap();
        assert_eq!(hash_bounded(&mut f, 199_999, big), None);
        let mut f = open_for_hash(&p).unwrap();
        assert_eq!(hash_bounded(&mut f, 200_000, big).unwrap().len(), 64);
        let mut f = open_for_hash(&p).unwrap();
        assert_eq!(
            hash_bounded(&mut f, 200_000, std::time::Duration::ZERO),
            None
        );
        let _ = std::fs::remove_file(&p);
    }
}
