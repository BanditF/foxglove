// SPDX-License-Identifier: MIT
//! Whether a folder is kept in step with a cloud service (OneDrive, Dropbox and any other app
//! that registers a Windows Cloud Files *sync root*). Its files can then be rewritten from
//! another device or by the service, which is the same threat as a USB disk (trust model §4):
//! a dev folder vouches for whatever is in it, so such a folder never qualifies.
//!
//! No vendor is named here. The system is asked ([`SystemSync`]): the path sits under a sync
//! root, or carries a cloud placeholder attribute (a file or folder that is fetched on use, or
//! that the service pins or unpins). The question sits behind [`SyncProbe`] so tests use a fake.
//! Fails closed: an answer that can't be read is [`SyncAnswer::Unknown`], and Unknown refuses.

/// What the probe could tell about one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncAnswer {
    /// Not under a sync root and no placeholder attribute.
    Local,
    /// Under a sync root, or a cloud placeholder.
    Synced,
    /// The answer could not be read (the path, the API or the error was unexpected).
    Unknown,
}

pub trait SyncProbe: Send + Sync {
    /// Asks about a canonical path that exists now.
    fn check(&self, path: &str) -> SyncAnswer;
}

/// The real answers from Windows.
pub struct SystemSync;

/// `FILE_ATTRIBUTE_*` bits a cloud service sets: recall on open, recall on data access, pinned,
/// unpinned.
const CLOUD_ATTRIBUTES: u32 = 0x4_0000 | 0x40_0000 | 0x8_0000 | 0x10_0000;

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
const REPARSE_POINT: u32 = 0x400;

/// `HRESULT_FROM_WIN32` of the codes `CfGetSyncRootInfoByPath` gives for a path that is not under
/// a sync root: `ERROR_INVALID_FUNCTION` (1, what Windows 11 returned for a plain folder on an
/// NTFS drive when this was written), `ERROR_NOT_A_CLOUD_FILE` (376),
/// `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` (390), `ERROR_NOT_A_CLOUD_SYNC_ROOT` (405). Any other
/// code is Unknown, which refuses.
const NOT_UNDER_A_ROOT: [i32; 4] = [
    0x8007_0000_u32 as i32 | 1,
    0x8007_0000_u32 as i32 | 376,
    0x8007_0000_u32 as i32 | 390,
    0x8007_0000_u32 as i32 | 405,
];

/// The final decision from the two readings; every uncertain input is Unknown.
pub(crate) fn decide(attributes: Option<u32>, root: Option<i32>) -> SyncAnswer {
    match (attributes, root) {
        (Some(a), _) if a & CLOUD_ATTRIBUTES != 0 => SyncAnswer::Synced,
        // S_OK: the path is under a sync root.
        (_, Some(0)) => SyncAnswer::Synced,
        // "Invalid function" most likely means no sync filter on the volume. For a reparse
        // point (a junction, a link, a placeholder some provider left half-set-up) it proves
        // nothing: refuse. A canonical dev folder or a built program is never one.
        (Some(a), Some(h)) if h == NOT_UNDER_A_ROOT[0] && a & REPARSE_POINT != 0 => {
            SyncAnswer::Unknown
        }
        (Some(_), Some(h)) if NOT_UNDER_A_ROOT.contains(&h) => SyncAnswer::Local,
        _ => SyncAnswer::Unknown,
    }
}

#[cfg(windows)]
impl SyncProbe for SystemSync {
    fn check(&self, path: &str) -> SyncAnswer {
        use windows_sys::Win32::Storage::CloudFilters::{
            CfGetSyncRootInfoByPath, CF_SYNC_ROOT_INFO_BASIC,
        };
        use windows_sys::Win32::Storage::FileSystem::GetFileAttributesW;
        let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives both calls; the buffer
        // is 8 bytes (one `CF_SYNC_ROOT_BASIC_INFO`, an i64) and its size is passed.
        let (attrs, root) = unsafe {
            let a = GetFileAttributesW(wide.as_ptr());
            let mut buf = [0u8; 8];
            let mut got = 0u32;
            let h = CfGetSyncRootInfoByPath(
                wide.as_ptr(),
                CF_SYNC_ROOT_INFO_BASIC,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut got,
            );
            // INVALID_FILE_ATTRIBUTES: the path could not be read.
            ((a != u32::MAX).then_some(a), Some(h))
        };
        decide(attrs, root)
    }
}

#[cfg(not(windows))]
impl SyncProbe for SystemSync {
    fn check(&self, _: &str) -> SyncAnswer {
        SyncAnswer::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placeholder_attribute_or_a_sync_root_is_synced() {
        assert_eq!(
            decide(Some(0x40_0000), Some(NOT_UNDER_A_ROOT[0])),
            SyncAnswer::Synced
        );
        assert_eq!(decide(Some(0x4_0000), None), SyncAnswer::Synced);
        assert_eq!(decide(Some(0x8_0000 | 0x10), Some(1)), SyncAnswer::Synced);
        assert_eq!(decide(Some(0x10), Some(0)), SyncAnswer::Synced);
        assert_eq!(decide(None, Some(0)), SyncAnswer::Synced);
    }

    #[test]
    fn invalid_function_on_a_reparse_point_is_not_proof_of_local() {
        let invalid_function = NOT_UNDER_A_ROOT[0];
        assert_eq!(
            decide(Some(0x10 | 0x400), Some(invalid_function)),
            SyncAnswer::Unknown
        );
        assert_eq!(
            decide(Some(0x400), Some(invalid_function)),
            SyncAnswer::Unknown
        );
        // The same answer on a plain folder or file is Local.
        assert_eq!(
            decide(Some(0x10), Some(invalid_function)),
            SyncAnswer::Local
        );
        // The explicit "not a cloud file" codes still mean Local, reparse point or not.
        assert_eq!(
            decide(Some(0x10 | 0x400), Some(NOT_UNDER_A_ROOT[2])),
            SyncAnswer::Local
        );
        // A cloud attribute or a sync root still wins.
        assert_eq!(
            decide(Some(0x400 | 0x40_0000), Some(invalid_function)),
            SyncAnswer::Synced
        );
    }

    #[test]
    fn only_a_clean_not_under_a_root_answer_is_local() {
        for h in NOT_UNDER_A_ROOT {
            assert_eq!(decide(Some(0x10), Some(h)), SyncAnswer::Local);
            // Attributes that couldn't be read: not known to be local.
            assert_eq!(decide(None, Some(h)), SyncAnswer::Unknown);
        }
        // Any other error from the API, or no answer: unknown, which refuses.
        assert_eq!(
            decide(Some(0x10), Some(0x8007_0005_u32 as i32)),
            SyncAnswer::Unknown
        );
        assert_eq!(decide(Some(0x10), None), SyncAnswer::Unknown);
    }

    /// The real API on a folder that is not synced: a clear Local (so a fresh dev folder is not
    /// refused on a PC where the check works), printed for the log.
    #[cfg(windows)]
    #[test]
    fn the_real_probe_calls_a_plain_folder_local() {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let path = crate::keyboard::canonical_path(&dir).unwrap();
        let answer = SystemSync.check(&path);
        eprintln!("sync answer for {path}: {answer:?}");
        assert_eq!(answer, SyncAnswer::Local);
    }
}
