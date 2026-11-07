//! The store split (11/07 diagnosis): Foxglove started by a packaged app (the Claude desktop
//! app's shells, say) runs inside that app's file-system container. Windows then redirects its
//! `%APPDATA%\Foxglove` (settings and log) into that app's private copy,
//! `%LOCALAPPDATA%\Packages\<that app>\LocalCache\Roaming\Foxglove`, so it reads and writes a
//! second store the user's own Foxglove never sees: another pairing token, no saved look.
//!
//! The signal is the one Windows gives for any redirected file: `GetFinalPathNameByHandleW` on
//! an open handle names the backing file, which here lies under another package's `LocalCache`.
//! `GetCurrentPackageFamilyName` doesn't tell: a shell of a packaged app (and so Foxglove started
//! from it) has no package identity of its own (`APPMODEL_ERROR_NO_PACKAGE`), yet its writes are
//! redirected (measured 11/07 in a Claude shell).
//!
//! [`start`] checks once at launch, after the log opened: one log line, a health WARN row
//! ("Settings folder") and its notice, through `health_notice` (at most once a day).

use std::path::Path;
use std::sync::OnceLock;

use super::health::{HealthRow, Level};

/// Foxglove's own package family's name part: its own container (if Windows ever gave it one)
/// is not another app's.
const OWN: &str = super::identity::NAME;

/// The notice's title and body.
pub const TITLE: &str = "Settings in another app's copy";
pub const BODY: &str = "Foxglove is using another app's private copy of its settings — start it \
                        from the Start menu or the tray";

/// Foxglove's files redirected into another app's package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirect {
    /// That app's package family name (`Claude_pzs8sxrjxfjjc`).
    pub package: String,
    /// The file Windows really opened.
    pub path: String,
}

/// Whether `requested` (the path Foxglove opened) was redirected: `real` is what
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
        .starts_with(&format!("{}_", OWN.to_ascii_lowercase()))
    {
        return None;
    }
    Some(Redirect {
        package,
        path: real_plain,
    })
}

/// The log line for a redirect.
pub fn log_line(r: &Redirect, folder: &Path) -> String {
    format!(
        "Foxglove: started inside another app's package ({}); settings and log in {} are that \
         app's private copy at {}",
        r.package,
        folder.display(),
        r.path
    )
}

/// The health row for a redirect (subsystem "app", "Settings folder"), with the notice's words.
pub fn row(r: &Redirect) -> HealthRow {
    HealthRow::new(
        "app",
        "Settings folder",
        Level::Warn,
        format!(
            "Foxglove was started from another app ({}), so Windows keeps its settings and log in \
             that app's private copy ({}). The Foxglove you start from the Start menu or the tray \
             uses the real ones",
            r.package, r.path
        ),
    )
    .with_notice(TITLE, BODY)
}

static FOUND: OnceLock<Option<Redirect>> = OnceLock::new();

/// The health rows (registered as "store_split"): the WARN while redirected.
pub fn health_rows() -> Vec<HealthRow> {
    FOUND.get().cloned().flatten().iter().map(row).collect()
}

/// Checks once, after `app_log::start`: the log file (open by then) or the store file.
pub fn start() {
    if super::store::test_sandbox().is_some() {
        return;
    }
    let Some(store) = super::store::path() else {
        return;
    };
    let log = super::app_log::dir().map(|d| d.join("foxglove.log"));
    let found = log
        .iter()
        .chain(std::iter::once(&store))
        .filter(|p| p.is_file())
        .find_map(|p| final_path(p).and_then(|real| redirected(p, &real)));
    let _ = FOUND.set(found.clone());
    super::health::register("store_split", health_rows);
    if let Some(r) = found {
        let folder = store.parent().unwrap_or(&store);
        eprintln!("{}", log_line(&r, folder));
        super::health_notice::check(&[row(&r)]);
    }
}

/// The path Windows really opened for `file`.
#[cfg(windows)]
fn final_path(file: &Path) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let f = std::fs::File::open(file).ok()?;
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
fn final_path(_: &Path) -> Option<String> {
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
        let line = log_line(&r, Path::new(r"C:\Users\fuchs\AppData\Roaming\Foxglove"));
        assert!(
            line.contains("another app's package (Claude_pzs8sxrjxfjjc)"),
            "{line}"
        );
        let row = row(&r);
        assert_eq!(
            (row.subsystem.as_str(), row.item.as_str()),
            ("app", "Settings folder")
        );
        assert_eq!(row.level, Level::Warn);
        let (title, body) = super::super::health_notice::text(&row);
        assert_eq!(title, TITLE);
        assert!(
            body.contains("start it from the Start menu or the tray"),
            "{body}"
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
        // A test's own folder under the package (FOXGLOVE_CONFIG_DIR in %TEMP%) still counts.
        let temp = PathBuf::from(r"C:\Users\fuchs\AppData\Local\Temp\fg\foxglove.toml");
        assert!(redirected(
            &temp,
            r"\\?\C:\Users\fuchs\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\Temp\fg\foxglove.toml"
        )
        .is_some());
    }

    #[test]
    fn no_redirect_no_rows() {
        // FOUND is never set in tests (start skips the sandbox).
        assert!(health_rows().is_empty());
    }
}
