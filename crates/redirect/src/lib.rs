//! Windows' file-system redirection into another app's package (#896, #894).
//!
//! A process started from a packaged app's shell (the Claude desktop app's, say) runs inside
//! that app's file-system container: Windows redirects its writes under `%APPDATA%` and
//! `%LOCALAPPDATA%` into that app's private copy,
//! `%LOCALAPPDATA%\Packages\<that app>\LocalCache\{Roaming,Local}\...`, which nothing outside
//! the container sees. The app's settings (#896), the lab's injection log and the flasher's
//! history (#894) all landed there.
//!
//! The signal is the one Windows gives for any redirected file: `GetFinalPathNameByHandleW` on
//! an open handle names the backing file, which then lies under another package's `LocalCache`.
//! `GetCurrentPackageFamilyName` doesn't tell: a packaged app's shell (and so a process started
//! from it) has no package identity of its own, yet its writes are redirected (measured 11/07 in
//! a Claude shell). `scripts/appdata_redirect.py` is the Python twin of this crate.

use std::path::Path;

/// Foxglove's own package family's name part: its own container (if Windows ever gave it one)
/// is not another app's. The app checks it against its `identity::NAME`.
pub const OWN_PACKAGE: &str = "BanditF.Foxglove";

/// A file redirected into another app's package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// That app's package family name (`Claude_pzs8sxrjxfjjc`).
    pub package: String,
    /// The file Windows really opened.
    pub path: String,
}

/// Whether `requested` (the path opened) was redirected: `real` is what
/// `GetFinalPathNameByHandleW` says it opened. Another app's package's `LocalCache` gives
/// `Some`; the same file (any case, with or without the `\\?\` prefix), a link elsewhere, or
/// Foxglove's own package gives `None`.
pub fn redirected(requested: &Path, real: &str) -> Option<Redirect> {
    let plain = |p: &str| {
        let p = p.strip_prefix(r"\\?\UNC\").map_or_else(
            || p.strip_prefix(r"\\?\").unwrap_or(p).to_string(),
            |rest| format!(r"\\{rest}"),
        );
        p.replace('/', "\\").to_ascii_lowercase()
    };
    let (asked, got) = (plain(&requested.to_string_lossy()), plain(real));
    if asked == got {
        return None;
    }
    let at = got.find(r"\appdata\local\packages\")? + r"\appdata\local\packages\".len();
    let rest = &got[at..];
    let end = rest.find('\\')?;
    if !rest[end..].starts_with(r"\localcache\") {
        return None;
    }
    // The family name as Windows spells it (the lower-cased copy has the same length).
    let real_plain = real
        .strip_prefix(r"\\?\")
        .unwrap_or(real)
        .replace('/', "\\");
    let offset = real_plain.len() - got.len();
    let package = real_plain[offset + at..offset + at + end].to_string();
    if package
        .to_ascii_lowercase()
        .starts_with(&format!("{}_", OWN_PACKAGE.to_ascii_lowercase()))
    {
        return None;
    }
    Some(Redirect {
        package,
        path: real_plain,
    })
}

/// Whether the existing file `file` is really another app's copy (None: it isn't, it doesn't
/// exist, or Windows couldn't say).
pub fn check(file: &Path) -> Option<Redirect> {
    let f = std::fs::File::open(file).ok()?;
    redirected(file, &final_path(&f)?)
}

/// Whether new files in `dir` (which must exist) go to another app's copy: creates a probe file
/// there, asks where it went, and deletes it.
pub fn probe(dir: &Path) -> Option<Redirect> {
    let file = dir.join(format!(".foxglove-redirect-probe-{}", std::process::id()));
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .ok()?;
    let found = final_path(&f).and_then(|real| redirected(&file, &real));
    drop(f);
    let _ = std::fs::remove_file(&file);
    found
}

/// One line for a tool that wrote `what` to `requested` but Windows put it in `r.path`.
pub fn note(what: &str, requested: &Path, r: &Redirect) -> String {
    format!(
        "{what} is in another app's private copy: {} (asked for {}). This shell runs inside {}'s \
         package, so Windows redirects AppData; run from a normal terminal (Windows Terminal, \
         Start menu) to write the real file",
        r.path,
        requested.display(),
        r.package
    )
}

/// The path Windows really opened for `f`.
#[cfg(windows)]
pub fn final_path(f: &std::fs::File) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let handle = HANDLE(f.as_raw_handle());
    let mut buf = vec![0u16; 1024];
    // SAFETY: the handle is open for the call; the buffer's length is passed with it.
    let n = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, FILE_NAME_NORMALIZED) } as usize;
    if n == 0 || n > buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n]))
}

#[cfg(not(windows))]
pub fn final_path(_: &std::fs::File) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ASKED: &str = r"C:\Users\fuchs\AppData\Roaming\Foxglove\logs\foxglove.log";

    /// Measured 11/07 in a Claude shell: the real file under Claude's LocalCache.
    #[test]
    fn another_apps_local_cache_is_a_redirect() {
        let real = r"\\?\C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Foxglove\logs\foxglove.log";
        let r = redirected(&PathBuf::from(ASKED), real).expect("redirected");
        assert_eq!(r.package, "Claude_pzs8sxrjxfjjc");
        assert_eq!(
            r.path,
            r"C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Foxglove\logs\foxglove.log"
        );
    }

    /// #894: the flasher's history and the lab's injection log, under %LOCALAPPDATA%.
    #[test]
    fn local_app_data_redirects_too() {
        let asked = PathBuf::from(r"C:\Users\fuchs\AppData\Local\Foxglove\lab-injections.log");
        let r = redirected(
            &asked,
            r"\\?\C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\Foxglove\lab-injections.log",
        )
        .expect("redirected");
        assert_eq!(r.package, "Claude_pzs8sxrjxfjjc");
        let line = note("the lab's injection log", &asked, &r);
        assert!(
            line.contains(r"Claude_pzs8sxrjxfjjc\LocalCache\Local\Foxglove\lab-injections.log"),
            "{line}"
        );
        assert!(
            line.contains("inside Claude_pzs8sxrjxfjjc's package"),
            "{line}"
        );
    }

    #[test]
    fn the_same_file_or_another_place_is_not() {
        let asked = PathBuf::from(ASKED);
        assert_eq!(redirected(&asked, &format!(r"\\?\{ASKED}")), None);
        assert_eq!(
            redirected(&asked, &format!(r"\\?\{}", ASKED.to_ascii_uppercase())),
            None,
            "case doesn't matter"
        );
        // A folder moved by a link (OneDrive, a junction): not a package's copy.
        assert_eq!(
            redirected(&asked, r"\\?\D:\Roaming\Foxglove\logs\foxglove.log"),
            None
        );
        // A package folder but not its LocalCache.
        assert_eq!(
            redirected(
                &asked,
                r"\\?\C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\RoamingState\foxglove.log"
            ),
            None
        );
        // Foxglove's own package.
        assert_eq!(
            redirected(
                &asked,
                r"\\?\C:\Users\fuchs\AppData\Local\Packages\BanditF.Foxglove_abc123\LocalCache\Roaming\Foxglove\logs\foxglove.log"
            ),
            None
        );
        // A folder under the package (FOXGLOVE_CONFIG_DIR in %TEMP%) still counts.
        let temp = PathBuf::from(r"C:\Users\fuchs\AppData\Local\Temp\fg\foxglove.toml");
        assert!(redirected(
            &temp,
            r"\\?\C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\Temp\fg\foxglove.toml"
        )
        .is_some());
    }

    /// The probe leaves nothing behind, and a missing file or folder is no redirect.
    #[test]
    fn probe_and_check_clean_up_and_tolerate_missing_paths() {
        let dir = std::env::temp_dir().join(format!("fg-redirect-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = probe(&dir);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert_eq!(probe(&dir.join("missing")), None);
        assert_eq!(check(&dir.join("missing.log")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
