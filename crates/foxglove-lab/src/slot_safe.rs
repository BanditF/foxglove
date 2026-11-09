//! Data-safe flash-slot steps for the SLOT_READ lab case (#911). The keyboard has no empty flash
//! place, so the case borrows a kept one: it backs the bytes up to a file first, runs the test in
//! that place, and puts the original bytes back and verifies them. These are the pieces the
//! `art_slot` step runs; scripts/lab/slot_read.py sequences them (and does the restore in a
//! `finally`). Everything goes through [`SlotDevice`] so the unit tests use a fake keyboard.
//!
//! A backup is two files: `<base>.bin` (the exact bytes) and `<base>.json` (slot, name, CRC,
//! length), both written before anything in the place is touched.

use anyhow::{ensure, Context, Result};
use foxglove_link::{art, protocol, protocol::SlotInfo, Transport};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What the flow needs from the keyboard.
pub trait SlotDevice {
    /// What flash place `n` holds, or an error past the last place.
    fn info(&self, n: u8) -> Result<SlotInfo>;
    /// The bytes kept in place `n` (`len` of them, from D6 87), read back with SLOT_READ.
    fn read(&self, n: u8, len: u16) -> Result<Vec<u8>>;
    /// Replaces place `n` with `blob` under `name`.
    fn store(&self, n: u8, blob: &[u8], name: &str) -> Result<()>;
}

/// The real keyboard.
pub struct Real<'a, T: Transport>(pub &'a T);

impl<T: Transport> SlotDevice for Real<'_, T> {
    fn info(&self, n: u8) -> Result<SlotInfo> {
        art::slot(self.0, n)
    }
    fn read(&self, n: u8, len: u16) -> Result<Vec<u8>> {
        art::read_slot(self.0, n, len, || false)
    }
    fn store(&self, n: u8, blob: &[u8], name: &str) -> Result<()> {
        art::store(self.0, blob, n, name, |_| {}, || false).map(|_| ())
    }
}

/// The sidecar of a backup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub slot: u8,
    pub name: String,
    pub crc: u32,
    pub len: u16,
}

pub fn bin_path(base: &str) -> PathBuf {
    PathBuf::from(format!("{base}.bin"))
}
pub fn meta_path(base: &str) -> PathBuf {
    PathBuf::from(format!("{base}.json"))
}

/// The kept places, in order (the scan stops at the first place the keyboard doesn't answer for).
pub fn kept(dev: &impl SlotDevice) -> Vec<SlotInfo> {
    (0..16u8)
        .map_while(|n| dev.info(n).ok())
        .filter(|i| i.used)
        .collect()
}

/// The first kept place, or `want` if it is kept (an empty place is refused: it has nothing to back up).
pub fn pick(dev: &impl SlotDevice, want: Option<u8>) -> Result<SlotInfo> {
    let list = kept(dev);
    match want {
        Some(n) => list
            .into_iter()
            .find(|i| i.slot == n)
            .with_context(|| format!("place {n} keeps nothing (nothing to back up)")),
        None => list.into_iter().next().context("no kept place"),
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    ensure!(
        std::fs::read(path)? == bytes,
        "{} did not read back as written",
        path.display()
    );
    Ok(())
}

/// Reads place `n` and checks the bytes against its own CRC and length. Never writes the keyboard.
fn read_checked(dev: &impl SlotDevice, n: u8) -> Result<(SlotInfo, Vec<u8>)> {
    let info = dev.info(n)?;
    ensure!(info.used, "place {n} keeps nothing");
    let bytes = dev.read(n, info.len)?;
    ensure!(
        bytes.len() == usize::from(info.len),
        "place {n}: read {} of {} bytes",
        bytes.len(),
        info.len
    );
    if let Some(crc) = info.crc {
        let got = protocol::blob_crc(&bytes);
        ensure!(
            got == crc,
            "place {n}: the bytes read back have CRC {got:08x}, the place says {crc:08x}"
        );
    }
    Ok((info, bytes))
}

/// Step 2: reads place `n` and writes `<base>.bin` and `<base>.json` (read back from disk), with
/// the CRC the bytes really have. Never writes the keyboard.
pub fn backup(dev: &impl SlotDevice, n: u8, base: &str) -> Result<String> {
    let (info, bytes) = read_checked(dev, n)?;
    let meta = Meta {
        slot: n,
        name: info.name.clone(),
        crc: protocol::blob_crc(&bytes),
        len: info.len,
    };
    write_file(&bin_path(base), &bytes)?;
    write_file(
        &meta_path(base),
        serde_json::to_string_pretty(&meta)?.as_bytes(),
    )?;
    Ok(format!(
        "place {n} ({:?}) backed up: {} bytes, crc {:08x}, {}",
        meta.name,
        meta.len,
        meta.crc,
        bin_path(base).display()
    ))
}

/// A backup's sidecar alone.
pub fn load_meta(base: &str) -> Result<Meta> {
    Ok(serde_json::from_slice(
        &std::fs::read(meta_path(base))
            .with_context(|| format!("no backup {}", meta_path(base).display()))?,
    )?)
}

/// Loads a backup and checks the file against its own sidecar.
pub fn load(base: &str, n: u8) -> Result<(Meta, Vec<u8>)> {
    let meta = load_meta(base)?;
    let bytes = std::fs::read(bin_path(base))
        .with_context(|| format!("no backup {}", bin_path(base).display()))?;
    ensure!(
        meta.slot == n,
        "the backup is of place {}, not {n}",
        meta.slot
    );
    ensure!(
        bytes.len() == usize::from(meta.len) && protocol::blob_crc(&bytes) == meta.crc,
        "the backup file {} does not match its own record",
        bin_path(base).display()
    );
    Ok((meta, bytes))
}

/// Compares place `n` with the backup: CRC, name, length and every byte. Never writes the keyboard.
pub fn verify(dev: &impl SlotDevice, n: u8, base: &str) -> Result<String> {
    let (meta, want) = load(base, n)?;
    let (info, got) = read_checked(dev, n)?;
    ensure!(
        info.crc.is_none_or(|c| c == meta.crc),
        "place {n} keeps crc {:08x}, the backup is {:08x}",
        info.crc.unwrap_or(0),
        meta.crc
    );
    ensure!(
        info.name == meta.name,
        "place {n} is named {:?}, the backup {:?}",
        info.name,
        meta.name
    );
    ensure!(
        got == want,
        "place {n}: bytes differ from the backup (crc {:08x} against {:08x})",
        protocol::blob_crc(&got),
        meta.crc
    );
    Ok(format!(
        "place {n} is byte-identical to the backup (crc {:08x}, {} bytes)",
        meta.crc, meta.len
    ))
}

/// Step 3's write: puts the fixture in place `n`, but only if the backup exists on disk and place
/// `n` still equals it now (so the original is safe before it is replaced).
pub fn write_fixture(
    dev: &impl SlotDevice,
    n: u8,
    base: &str,
    blob: &[u8],
    name: &str,
) -> Result<String> {
    ensure!(art::is_blob(blob), "the fixture is not an art blob");
    verify(dev, n, base).context("not writing: the place is not backed up")?;
    dev.store(n, blob, name)?;
    Ok(format!(
        "fixture ({:08x}) written to place {n}",
        protocol::blob_crc(blob)
    ))
}

/// Step 4: writes the backup bytes back into place `n` and verifies them (a second `verify`, from
/// a fresh read). When the verify fails the error says how to restore by hand.
pub fn restore(dev: &impl SlotDevice, n: u8, base: &str) -> Result<String> {
    let (meta, bytes) = load(base, n).context("the backup cannot be loaded")?;
    // Already original (nothing was written, or the write failed first): leave the flash alone.
    if let Ok(msg) = verify(dev, n, base) {
        return Ok(format!("{msg} (nothing to restore)"));
    }
    dev.store(n, &bytes, &meta.name)?;
    verify(dev, n, base).map(|m| format!("restored; {m}"))
}

/// The manual way back, for when [`restore`] could not be verified.
pub fn manual_instructions(n: u8, base: &str) -> String {
    format!(
        "Restore place {n} by hand. The original bytes are {bin} (details {meta}). Run: \
         foxglove-lab --allow-app run --suite tests/hil/slot-read.toml --only slot-read-restore \
         with FOXGLOVE_SLOT_READ_SLOT={n} and FOXGLOVE_SLOT_READ_BACKUP={base}; or push {bin} to \
         place {n} in the Foxglove app (Art, STORE to place {n}, name from the json) and compare \
         the place's CRC with the json.",
        bin = bin_path(base).display(),
        meta = meta_path(base).display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::cell::RefCell;

    type Kept = (String, Vec<u8>);

    /// A keyboard with places; `fail_store` makes the next store(s) fail, `corrupt_store` makes
    /// it keep wrong bytes.
    struct Fake {
        slots: RefCell<Vec<Option<Kept>>>,
        fail_stores: RefCell<u32>,
        corrupt_store: RefCell<bool>,
        stores: RefCell<Vec<u8>>,
    }
    impl Fake {
        fn new(slots: Vec<Option<(&str, Vec<u8>)>>) -> Self {
            Self {
                slots: RefCell::new(
                    slots
                        .into_iter()
                        .map(|s| s.map(|(n, b)| (n.to_string(), b)))
                        .collect(),
                ),
                fail_stores: RefCell::new(0),
                corrupt_store: RefCell::new(false),
                stores: RefCell::new(vec![]),
            }
        }
    }
    impl SlotDevice for Fake {
        fn info(&self, n: u8) -> Result<SlotInfo> {
            let s = self.slots.borrow();
            let slot = s.get(usize::from(n)).context("past the last place")?;
            Ok(match slot {
                Some((name, b)) => SlotInfo {
                    slot: n,
                    used: true,
                    writes: 1,
                    len: b.len() as u16,
                    name: name.clone(),
                    crc: Some(protocol::blob_crc(b)),
                },
                None => SlotInfo {
                    slot: n,
                    used: false,
                    writes: 0,
                    len: 0,
                    name: String::new(),
                    crc: None,
                },
            })
        }
        fn read(&self, n: u8, _len: u16) -> Result<Vec<u8>> {
            Ok(self.slots.borrow()[usize::from(n)]
                .as_ref()
                .context("empty")?
                .1
                .clone())
        }
        fn store(&self, n: u8, blob: &[u8], name: &str) -> Result<()> {
            if *self.fail_stores.borrow() > 0 {
                *self.fail_stores.borrow_mut() -= 1;
                bail!("store failed");
            }
            let mut b = blob.to_vec();
            if *self.corrupt_store.borrow() {
                b[0] ^= 1;
            }
            self.stores.borrow_mut().push(n);
            self.slots.borrow_mut()[usize::from(n)] = Some((name.to_string(), b));
            Ok(())
        }
    }

    fn base(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!("slot-safe-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("b").to_string_lossy().into_owned()
    }
    fn original() -> Vec<u8> {
        (0..300u32).map(|i| (i * 7) as u8).collect()
    }
    fn fixture() -> Vec<u8> {
        let mut b = b"NVA1".to_vec();
        b.extend(0..60u8);
        b
    }
    fn kb() -> Fake {
        Fake::new(vec![
            None,
            Some(("mine", original())),
            Some(("other", vec![9; 20])),
        ])
    }

    #[test]
    fn picks_first_kept_or_the_asked_one() {
        let d = kb();
        assert_eq!(pick(&d, None).unwrap().slot, 1);
        assert_eq!(pick(&d, Some(2)).unwrap().slot, 2);
        assert!(pick(&d, Some(0)).is_err(), "an empty place is refused");
        assert!(pick(&Fake::new(vec![None]), None).is_err());
    }

    #[test]
    fn backup_writes_files_first_and_never_writes_the_keyboard() {
        let d = kb();
        let b = base("backup");
        backup(&d, 1, &b).unwrap();
        assert_eq!(std::fs::read(bin_path(&b)).unwrap(), original());
        let m: Meta = serde_json::from_slice(&std::fs::read(meta_path(&b)).unwrap()).unwrap();
        assert_eq!(
            (m.slot, m.name.as_str(), m.crc),
            (1, "mine", protocol::blob_crc(&original()))
        );
        assert!(d.stores.borrow().is_empty());
        verify(&d, 1, &b).unwrap();
    }

    #[test]
    fn write_refuses_without_a_backup_and_leaves_the_place_alone() {
        let d = kb();
        assert!(write_fixture(&d, 1, &base("none"), &fixture(), "t").is_err());
        assert!(d.stores.borrow().is_empty());
        // A backup of another state (the place changed since) is refused too.
        let b = base("stale");
        backup(&d, 1, &b).unwrap();
        d.slots.borrow_mut()[1] = Some(("mine".into(), vec![1, 2, 3]));
        assert!(write_fixture(&d, 1, &b, &fixture(), "t").is_err());
        assert!(d.stores.borrow().is_empty());
    }

    #[test]
    fn full_flow_restores_byte_identical() {
        let d = kb();
        let b = base("flow");
        backup(&d, 1, &b).unwrap();
        write_fixture(&d, 1, &b, &fixture(), "t").unwrap();
        assert_eq!(d.slots.borrow()[1].as_ref().unwrap().1, fixture());
        assert!(
            verify(&d, 1, &b).is_err(),
            "the fixture is not the original"
        );
        restore(&d, 1, &b).unwrap();
        let s = d.slots.borrow();
        assert_eq!(s[1].as_ref().unwrap(), &("mine".to_string(), original()));
        assert_eq!(
            s[2].as_ref().unwrap().1,
            vec![9; 20],
            "other places untouched"
        );
    }

    #[test]
    fn restore_after_failed_write_stores_nothing() {
        let d = kb();
        let b = base("nowrite");
        backup(&d, 1, &b).unwrap();
        *d.fail_stores.borrow_mut() = 1;
        assert!(write_fixture(&d, 1, &b, &fixture(), "t").is_err());
        let msg = restore(&d, 1, &b).unwrap();
        assert!(msg.contains("nothing to restore"));
        assert!(d.stores.borrow().is_empty());
    }

    #[test]
    fn restore_that_does_not_verify_is_an_error() {
        let d = kb();
        let b = base("bad");
        backup(&d, 1, &b).unwrap();
        write_fixture(&d, 1, &b, &fixture(), "t").unwrap();
        *d.corrupt_store.borrow_mut() = true;
        assert!(restore(&d, 1, &b).is_err());
        // Failing stores likewise.
        *d.corrupt_store.borrow_mut() = false;
        *d.fail_stores.borrow_mut() = 1;
        assert!(restore(&d, 1, &b).is_err());
        // The next try works (the backup is still on disk).
        restore(&d, 1, &b).unwrap();
        assert_eq!(d.slots.borrow()[1].as_ref().unwrap().1, original());
    }

    #[test]
    fn a_damaged_backup_file_is_refused() {
        let d = kb();
        let b = base("damaged");
        backup(&d, 1, &b).unwrap();
        std::fs::write(bin_path(&b), b"nope").unwrap();
        assert!(load(&b, 1).is_err());
        assert!(restore(&d, 1, &b).is_err());
        assert!(load(&b, 2).is_err(), "wrong place");
    }

    #[test]
    fn manual_instructions_name_the_files() {
        let t = manual_instructions(3, "C:/x/b");
        assert!(t.contains("b.bin") && t.contains("b.json") && t.contains("place 3"));
    }
}
