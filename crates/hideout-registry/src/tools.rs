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
        self.update(|tools| {
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
        self.update(|tools| tools.retain(|t| !same_path(&t.image_path, image_path)))
    }

    fn update(&self, f: impl FnOnce(&mut Vec<ToolRecord>)) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            hideout_acl::create_private_dir_all(dir)?;
        }
        let lock = self.path.with_file_name("tools.lock");
        let _guard = take_lock(&lock)?;
        let mut tools = match read(&self.path) {
            Ok(f) => f.tools,
            // A file this hub does not understand is left as it is.
            Err(e) if e.kind() == io::ErrorKind::Unsupported => return Err(e),
            Err(_) => Vec::new(),
        };
        f(&mut tools);
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

/// Windows paths compare without regard to case.
fn same_path(a: &str, b: &str) -> bool {
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

struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `tools.lock`, created exclusively; a held lock is waited for a moment, not forever.
fn take_lock(path: &Path) -> io::Result<Guard> {
    for _ in 0..50 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(_) => return Ok(Guard(path.to_path_buf())),
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
