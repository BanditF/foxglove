//! The store split (11/07 diagnosis): Foxglove started by a packaged app (the Claude desktop
//! app's shells, say) runs inside that app's file-system container. Windows then redirects its
//! `%APPDATA%\Foxglove` (settings and log) into that app's private copy,
//! `%LOCALAPPDATA%\Packages\<that app>\LocalCache\Roaming\Foxglove`, so it reads and writes a
//! second store the user's own Foxglove never sees: another pairing token, no saved look.
//!
//! The detector (`GetFinalPathNameByHandleW` of an opened file against the path asked for) is
//! shared with the lab and the flasher (#894): `crates/redirect`.
//!
//! [`start`] checks once at launch, after the log opened: one log line, a health WARN row
//! ("Settings folder") and its notice, through `health_notice` (at most once a day).

use std::path::Path;
use std::sync::OnceLock;

pub use foxglove_redirect::Redirect;

use super::health::{HealthRow, Level};

/// The notice's title and body.
pub const TITLE: &str = "Settings in another app's copy";
pub const BODY: &str = "Foxglove is using another app's private copy of its settings — start it \
                        from the Start menu or the tray";

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
        .find_map(|p| foxglove_redirect::check(p));
    let _ = FOUND.set(found.clone());
    super::health::register("store_split", health_rows);
    if let Some(r) = found {
        let folder = store.parent().unwrap_or(&store);
        eprintln!("{}", log_line(&r, folder));
        super::health_notice::check(&[row(&r)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxglove_redirect::redirected;
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

    /// The shared detector (crates/redirect) skips Foxglove's own package by this name.
    #[test]
    fn the_detector_knows_foxglove_by_its_identity() {
        assert_eq!(foxglove_redirect::OWN_PACKAGE, super::super::identity::NAME);
    }

    #[test]
    fn no_redirect_no_rows() {
        // FOUND is never set in tests (start skips the sandbox).
        assert!(health_rows().is_empty());
    }
}
