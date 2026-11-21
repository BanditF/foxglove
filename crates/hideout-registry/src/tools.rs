// SPDX-License-Identifier: MIT
//! What the user allowed programs outside Hideout (S1, contract section 6.4): one record per
//! program file, kept in `tools.json` beside the inventory. A record is bound to the file's
//! path and SHA-256; when the file's bytes change the record is void and the program is asked
//! about again. The name a program gives itself is never stored here and never decides
//! anything. A script runner, a program with no known file and a contained program never get
//! a record (the router does not offer them).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FORMAT: &str = "hideout-tools/1";
/// The most records kept; the oldest is dropped first.
pub const MAX_TOOLS: usize = 64;

/// What was allowed to one program file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRecord {
    pub image_path: String,
    pub image_sha256: String,
    /// Permission strings, as the router parses them.
    pub granted: Vec<String>,
    /// When the user allowed it (`YYYY-MM-DDTHH:MM:SSZ`). `None` in a record written before
    /// the time was kept: the page then says the date is not known, never a made-up one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_at: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    #[serde(default)]
    tools: Vec<ToolRecord>,
}

/// One program's kept answer as the router holds it: its path, its hash and the granted
/// strings.
pub type Kept = (String, String, Vec<String>);

/// `tools.json` under a registry home.
#[derive(Clone, Debug)]
pub struct ToolStore {
    path: PathBuf,
}

impl ToolStore {
    pub fn at(home: &Path) -> ToolStore {
        ToolStore {
            path: home.join("tools.json"),
        }
    }

    /// The records on disk. A missing, unreadable or unknown-format file is no records (the
    /// programs are asked again; nothing is guessed).
    pub fn load(&self) -> Vec<ToolRecord> {
        read(&self.path).map(|f| f.tools).unwrap_or_default()
    }

    /// The records on disk, or why they could not be read. Only a missing file is no records.
    pub fn try_load(&self) -> io::Result<Vec<ToolRecord>> {
        match read(&self.path) {
            Ok(f) => Ok(f.tools),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// The hub's one read at its start. A read that fails for a moment (another program has
    /// the file open) is tried again for about a second. If the file still can't be read, the
    /// error is returned and this store is marked [`Self::unread`] for the rest of the run:
    /// the router then holds none of the records, so nothing may be written from what it
    /// holds. A file that is not valid, or of an unknown format, is not waited for: it reads
    /// as no records, as before.
    pub fn load_at_start(&self) -> io::Result<Vec<ToolRecord>> {
        let mut tries = 0;
        loop {
            match self.try_load() {
                Ok(tools) => return Ok(tools),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::InvalidData | io::ErrorKind::Unsupported
                    ) =>
                {
                    return Ok(Vec::new())
                }
                Err(e) if tries >= 20 => {
                    let mut unread = UNREAD.lock().unwrap_or_else(|e| e.into_inner());
                    if !unread.contains(&self.path) {
                        unread.push(self.path.clone());
                    }
                    return Err(e);
                }
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }

    /// The hub could not read this file at its start ([`Self::load_at_start`]): for this run
    /// the router does not hold what the file says, so the hub changes one record at a time
    /// ([`Self::remember`], [`Self::forget`]) and never calls [`Self::sync`].
    pub fn unread(&self) -> bool {
        UNREAD
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&self.path)
    }

    /// Keeps `granted` for this program file. A record for the same path with another hash is
    /// replaced (the old file's answer is void), and the list is capped at [`MAX_TOOLS`].
    /// `allowed_at` is when the user answered.
    pub fn remember(
        &self,
        image_path: &str,
        image_sha256: &str,
        granted: &[String],
        allowed_at: Option<&str>,
    ) -> io::Result<()> {
        self.update(false, |tools| {
            tools.retain(|t| !same_path(&t.image_path, image_path));
            tools.push(ToolRecord {
                image_path: image_path.to_string(),
                image_sha256: image_sha256.to_string(),
                granted: granted.to_vec(),
                allowed_at: allowed_at.map(str::to_string),
            });
            let over = tools.len().saturating_sub(MAX_TOOLS);
            tools.drain(..over);
        })
    }

    /// Drops every record for this program file: its bytes changed.
    pub fn void_path(&self, image_path: &str) -> io::Result<()> {
        self.forget(image_path)
    }

    /// Drops every record for this program file, whatever its hash: the user pressed Forget on
    /// the Apps page. The program is asked about again the next time it connects.
    pub fn forget(&self, image_path: &str) -> io::Result<()> {
        self.update(false, |tools| {
            tools.retain(|t| !same_path(&t.image_path, image_path))
        })
    }

    /// Makes the file say what the router holds: one record per answer `memory` returns, in
    /// its order (oldest first), capped at [`MAX_TOOLS`]. Every write of the hub goes through
    /// here, so the file never holds an answer the router dropped: an Allow that is written
    /// late cannot bring back a program the user forgot in between. The one exception is a run
    /// whose start could not read the file ([`Self::unread`]): the router then does not hold
    /// the records, and the caller changes one record instead. `memory` is called while
    /// the store's lock is held and must only read. A record keeps the time it had; `stamp`
    /// (path, hash, time) sets the time of the answer the user just gave; any other new record
    /// has none. Nothing is written when the file already says the same.
    pub fn sync(
        &self,
        memory: impl FnOnce() -> Vec<Kept>,
        stamp: Option<(&str, &str, &str)>,
    ) -> io::Result<()> {
        // What is in the file only gives the times, so a file that is not valid JSON is
        // replaced; one that can't be read at all is an error.
        self.update(true, |tools| {
            let mut new: Vec<ToolRecord> = Vec::new();
            for (image_path, image_sha256, granted) in memory() {
                if granted.is_empty() {
                    continue;
                }
                let stamped = stamp
                    .filter(|(p, h, _)| same_path(p, &image_path) && *h == image_sha256)
                    .map(|(_, _, at)| at.to_string());
                let allowed_at = stamped.or_else(|| {
                    tools
                        .iter()
                        .find(|t| {
                            same_path(&t.image_path, &image_path) && t.image_sha256 == image_sha256
                        })
                        .and_then(|t| t.allowed_at.clone())
                });
                new.push(ToolRecord {
                    image_path,
                    image_sha256,
                    granted,
                    allowed_at,
                });
            }
            let over = new.len().saturating_sub(MAX_TOOLS);
            new.drain(..over);
            *tools = new;
        })
    }

    /// Holds the store's lock until the guard is dropped: no other write of the file happens
    /// meanwhile, in this process or another (a write waits a moment, then fails).
    pub fn lock(&self) -> io::Result<impl Drop> {
        if let Some(dir) = self.path.parent() {
            hideout_acl::create_private_dir_all(dir)?;
        }
        take_lock(&self.path.with_file_name("tools.lock"))
    }

    /// Reads, changes and writes the file under the lock. Only a missing file reads as no
    /// records (and, when `replace_bad`, one that is not valid JSON): any other read error is
    /// returned and nothing is written, so a file that could not be read for a moment is
    /// never replaced by an empty list.
    fn update(&self, replace_bad: bool, f: impl FnOnce(&mut Vec<ToolRecord>)) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            hideout_acl::create_private_dir_all(dir)?;
        }
        let lock = self.path.with_file_name("tools.lock");
        let _guard = take_lock(&lock)?;
        let (mut tools, was) = match read(&self.path) {
            Ok(f) => (f.tools.clone(), Some(f.tools)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), None),
            Err(e) if replace_bad && e.kind() == io::ErrorKind::InvalidData => (Vec::new(), None),
            // A file this hub does not understand, or could not read, is left as it is.
            Err(e) => return Err(e),
        };
        f(&mut tools);
        if was.as_ref() == Some(&tools) {
            return Ok(());
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(&File {
            format: FORMAT.into(),
            tools,
        })
        .map_err(io::Error::other)?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// The files the hub could not read at its start, for this run ([`ToolStore::unread`]).
static UNREAD: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// What a record says for exactly this file: its granted strings, or `None` when no record
/// has this path and this hash (a changed file is a different program).
pub fn granted_for<'a>(
    records: &'a [ToolRecord],
    image_path: &str,
    image_sha256: &str,
) -> Option<&'a [String]> {
    records
        .iter()
        .find(|t| same_path(&t.image_path, image_path) && t.image_sha256 == image_sha256)
        .map(|t| t.granted.as_slice())
}

/// Windows paths compare without regard to case. Only ASCII letters are folded, and the hash is
/// not compared: in a folder set to tell case apart, two files whose names differ only in case
/// count as one path here, so Forget for one also forgets the other. That only ever takes an
/// answer away.
pub fn same_path(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn read(path: &Path) -> io::Result<File> {
    let bytes = std::fs::read(path)?;
    let f: File = serde_json::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if f.format != FORMAT {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unknown tools format",
        ));
    }
    Ok(f)
}

/// The held lock: the open handle is the lock, so it ends with the process that holds it.
struct Guard(Option<std::fs::File>, PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        // The handle first: the file can't be removed while it is open. If another writer
        // opened it in between, the removal fails and that writer keeps its lock.
        self.0.take();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// `tools.lock`, opened so that nobody else can open it while the handle lives. The system
/// closes the handle when the process ends, however it ends, so a lock file left on disk by a
/// crash is just a file: the next writer opens it and holds the lock. A lock that is held is
/// waited for a moment, not forever.
#[cfg(windows)]
fn take_lock(path: &Path) -> io::Result<Guard> {
    use std::os::windows::fs::OpenOptionsExt;
    /// `ERROR_SHARING_VIOLATION`: another handle holds the file.
    const HELD: i32 = 32;
    /// `ERROR_ACCESS_DENIED`: what a lock file answers while its removal is under way.
    const GOING: i32 = 5;
    for _ in 0..50 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(path)
        {
            Ok(f) => return Ok(Guard(Some(f), path.to_path_buf())),
            Err(e) if matches!(e.raw_os_error(), Some(HELD | GOING)) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "tools.lock is held",
    ))
}

/// `tools.lock`, created exclusively; a held lock is waited for a moment, not forever.
#[cfg(not(windows))]
fn take_lock(path: &Path) -> io::Result<Guard> {
    for _ in 0..50 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(f) => return Ok(Guard(Some(f), path.to_path_buf())),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "tools.lock is held",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-tools-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn tools_record_round_trip_and_void() {
        let d = home("rt");
        let s = ToolStore::at(&d);
        assert!(s.load().is_empty());
        let g = vec!["lighting.write".to_string()];
        s.remember(r"C:\Tools\a.exe", "aa", &g, Some("2026-11-17T09:00:00Z"))
            .unwrap();
        let recs = s.load();
        assert_eq!(granted_for(&recs, r"C:\Tools\a.exe", "aa"), Some(&g[..]));
        // Case of the path does not matter; a changed hash is no record.
        assert!(granted_for(&recs, r"c:\tools\A.EXE", "aa").is_some());
        assert_eq!(granted_for(&recs, r"C:\Tools\a.exe", "bb"), None);
        // New bytes at the same path replace the old record.
        s.remember(r"C:\Tools\a.exe", "bb", &[], None).unwrap();
        let recs = s.load();
        assert_eq!(recs.len(), 1);
        assert_eq!(granted_for(&recs, r"C:\Tools\a.exe", "aa"), None);
        s.void_path(r"C:\Tools\a.exe").unwrap();
        assert!(s.load().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn records_are_capped_and_the_oldest_goes() {
        let d = home("cap");
        let s = ToolStore::at(&d);
        for i in 0..MAX_TOOLS + 3 {
            s.remember(&format!(r"C:\T\{i}.exe"), "h", &[], None)
                .unwrap();
        }
        let recs = s.load();
        assert_eq!(recs.len(), MAX_TOOLS);
        assert_eq!(recs[0].image_path, r"C:\T\3.exe");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_record_without_a_time_loads_and_the_time_round_trips() {
        let d = home("old");
        // A file written before the time was kept.
        std::fs::write(
            d.join("tools.json"),
            r#"{"format":"hideout-tools/1","tools":[{"image_path":"C:\\Tools\\old.exe","image_sha256":"aa","granted":["lighting.write"]}]}"#,
        )
        .unwrap();
        let s = ToolStore::at(&d);
        let recs = s.load();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].allowed_at, None);
        s.remember(r"C:\Tools\new.exe", "bb", &[], Some("2026-11-17T09:00:00Z"))
            .unwrap();
        let recs = s.load();
        assert_eq!(recs[0].allowed_at, None);
        assert_eq!(recs[1].allowed_at.as_deref(), Some("2026-11-17T09:00:00Z"));
        // The old record is written back without a time, never with a made-up one.
        let text = std::fs::read_to_string(d.join("tools.json")).unwrap();
        assert_eq!(text.matches("allowed_at").count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn forget_drops_only_that_program_file() {
        let d = home("forget");
        let s = ToolStore::at(&d);
        let g = vec!["lighting.write".to_string()];
        s.remember(r"C:\Tools\a.exe", "aa", &g, None).unwrap();
        s.remember(r"C:\Tools\b.exe", "bb", &g, None).unwrap();
        s.forget(r"c:\tools\A.EXE").unwrap();
        let recs = s.load();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].image_path, r"C:\Tools\b.exe");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn kept(path: &str, sha: &str) -> Kept {
        (path.into(), sha.into(), vec!["lighting.write".to_string()])
    }

    #[test]
    fn sync_writes_what_memory_holds_and_keeps_the_times() {
        let d = home("sync");
        let s = ToolStore::at(&d);
        let a = r"C:\Tools\a.exe";
        let b = r"C:\Tools\b.exe";
        s.sync(
            || vec![kept(a, "aa")],
            Some((a, "aa", "2026-11-17T09:00:00Z")),
        )
        .unwrap();
        // A second answer: the first keeps its time, the new one gets its own, and a record
        // nobody stamped has none.
        s.sync(
            || vec![kept(a, "aa"), kept(b, "bb"), kept(r"C:\Tools\c.exe", "cc")],
            Some((b, "bb", "2026-11-18T09:00:00Z")),
        )
        .unwrap();
        let recs = s.load();
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].allowed_at.as_deref(), Some("2026-11-17T09:00:00Z"));
        assert_eq!(recs[1].allowed_at.as_deref(), Some("2026-11-18T09:00:00Z"));
        assert_eq!(recs[2].allowed_at, None);
        // An answer written late, after the program was forgotten, does not bring it back:
        // memory no longer holds it, whatever the write was for.
        s.sync(
            || vec![kept(b, "bb")],
            Some((a, "aa", "2026-11-19T09:00:00Z")),
        )
        .unwrap();
        let recs = s.load();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].image_path, b);
        assert_eq!(recs[0].allowed_at.as_deref(), Some("2026-11-18T09:00:00Z"));
        // More than the cap: the oldest go, as in memory.
        s.sync(
            || {
                (0..MAX_TOOLS + 2)
                    .map(|i| kept(&format!(r"C:\T\{i}.exe"), "h"))
                    .collect()
            },
            None,
        )
        .unwrap();
        let recs = s.load();
        assert_eq!(recs.len(), MAX_TOOLS);
        assert_eq!(recs[0].image_path, r"C:\T\2.exe");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A lock file left by a hub that was killed does not stop the next write; a lock another
    /// writer holds does, for a moment and then with an error.
    #[test]
    fn a_lock_left_behind_is_taken_over_and_a_held_one_is_not() {
        let d = home("lock");
        let s = ToolStore::at(&d);
        std::fs::write(d.join("tools.lock"), b"").unwrap();
        s.remember(r"C:\Tools\a.exe", "aa", &[], None).unwrap();
        assert_eq!(s.load().len(), 1);
        let held = s.lock().unwrap();
        let e = s.forget(r"C:\Tools\a.exe").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(s.load().len(), 1);
        drop(held);
        s.forget(r"C:\Tools\a.exe").unwrap();
        assert!(s.load().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file that can't be read is not replaced by an empty list: the other programs'
    /// records stay.
    #[test]
    fn a_file_that_cannot_be_read_is_not_overwritten() {
        let d = home("unread");
        std::fs::write(d.join("tools.json"), b"{ not json").unwrap();
        let s = ToolStore::at(&d);
        assert!(s.remember("x", "y", &[], None).is_err());
        assert!(s.forget("x").is_err());
        assert_eq!(std::fs::read(d.join("tools.json")).unwrap(), b"{ not json");
        // The hub's own write goes by what the router holds, so it replaces the bad file.
        s.sync(|| vec![kept(r"C:\Tools\a.exe", "aa")], None)
            .unwrap();
        assert_eq!(s.load().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file the hub's start could not read is waited for, then marked for the run; one that
    /// is only missing or not valid is not.
    #[cfg(windows)]
    #[test]
    fn a_start_that_cannot_read_the_file_is_marked() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = home("start");
        let s = ToolStore::at(&d);
        assert!(s.load_at_start().unwrap().is_empty());
        s.remember(r"C:\Tools\a.exe", "aa", &[], None).unwrap();
        assert_eq!(s.load_at_start().unwrap().len(), 1);
        assert!(!s.unread());
        {
            // Another program has the file open and shares it with nobody.
            let _held = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(d.join("tools.json"))
                .unwrap();
            assert!(s.load_at_start().is_err());
        }
        assert!(s.unread());
        assert!(!ToolStore::at(&home("start-other")).unread());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unknown_format_is_not_overwritten() {
        let d = home("fmt");
        std::fs::write(
            d.join("tools.json"),
            r#"{"format":"hideout-tools/9","tools":[]}"#,
        )
        .unwrap();
        let s = ToolStore::at(&d);
        assert!(s.load().is_empty());
        assert!(s.remember("x", "y", &[], None).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
