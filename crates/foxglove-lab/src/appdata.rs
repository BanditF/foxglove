//! Redirected AppData (#894). Started from a packaged app's shell (a Claude Code session, say),
//! the lab's writes under `%LOCALAPPDATA%\Foxglove` (the injection log) land in that app's
//! private copy, which a lab started anywhere else never reads. `health` shows it as a WARN row
//! and a suite run prints where its injections go. The detector is `foxglove_redirect`, shared
//! with the app's store-split check (#896).

use foxglove_link::health::{HealthRow, Level};
use foxglove_redirect::Redirect;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `%LOCALAPPDATA%\Foxglove`, or `%LOCALAPPDATA%` while that folder doesn't exist yet.
fn probe_dir() -> Option<PathBuf> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    let ours = local.join("Foxglove");
    Some(if ours.is_dir() { ours } else { local })
}

/// Whether this process's `%LOCALAPPDATA%` writes are redirected: one probe file, once a run.
pub fn redirect() -> Option<Redirect> {
    static FOUND: OnceLock<Option<Redirect>> = OnceLock::new();
    FOUND
        .get_or_init(|| foxglove_redirect::probe(&probe_dir()?))
        .clone()
}

/// The health row for a redirect: a WARN (it never fails the run).
pub fn row(r: &Redirect) -> HealthRow {
    HealthRow::new(
        "lab",
        "AppData",
        Level::Warn,
        format!(
            "this shell runs inside {}'s package, so the lab's AppData files (the injection log) \
             go to its private copy ({}); run from a normal terminal",
            r.package,
            Path::new(&r.path)
                .parent()
                .map_or_else(|| r.path.clone(), |p| p.display().to_string())
        ),
    )
}

/// The rows for `health`: the WARN while redirected, else none.
pub fn rows() -> Vec<HealthRow> {
    redirect().iter().map(row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_is_a_warn_naming_the_copy() {
        let r = Redirect {
            package: "Claude_pzs8sxrjxfjjc".into(),
            path: r"C:\Users\u\AppData\Local\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\Foxglove\.probe".into(),
        };
        let row = row(&r);
        assert_eq!(row.level, Level::Warn);
        assert_eq!(
            (row.subsystem.as_str(), row.item.as_str()),
            ("lab", "AppData")
        );
        assert!(
            row.detail
                .contains(r"Claude_pzs8sxrjxfjjc\LocalCache\Local\Foxglove)"),
            "{}",
            row.detail
        );
        assert!(crate::health::row_line(&row).starts_with("WARN lab "));
    }
}
