// SPDX-License-Identifier: MIT
//! `run\instances.json` (§7.9): the instances the hub started, so a restarted hub can take them
//! back. One record per module; written on every launch and end.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub instance: u64,
    pub pid: u32,
    /// The launch token (`HIDEOUT_TOKEN`), reused when it reconnects (§7.9).
    pub token: String,
    pub exe: PathBuf,
    /// SHA-256 of `exe` at launch, lowercase hex.
    pub sha256: String,
    /// The Job Object's name, to open it again.
    pub job: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instances {
    pub modules: BTreeMap<String, Record>,
}

impl Instances {
    pub fn path(home: &Path) -> PathBuf {
        home.join("run").join("instances.json")
    }

    /// The file's records; none when it is missing or unreadable (the hub then starts fresh).
    pub fn load(home: &Path) -> Instances {
        std::fs::read_to_string(Self::path(home))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Writes the file atomically (temp file, then rename).
    pub fn save(&self, home: &Path) -> io::Result<()> {
        let path = Self::path(home);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("hideout-inst-{}", std::process::id()));
        let mut i = Instances::default();
        i.modules.insert(
            "a.b".into(),
            Record {
                instance: 3,
                pid: 42,
                token: "t".into(),
                exe: "C:/m/a.exe".into(),
                sha256: "00".into(),
                job: "Local\\hideout-job".into(),
            },
        );
        i.save(&dir).unwrap();
        assert_eq!(Instances::load(&dir), i);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Instances::load(&dir), Instances::default());
    }
}
