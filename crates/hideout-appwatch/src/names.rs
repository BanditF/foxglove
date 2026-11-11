// SPDX-License-Identifier: MIT
//! A watched app's friendly name for a permission line (#991): "see whether OBS Studio is
//! running" instead of "obs64". The name comes from the exe's version info, `FileDescription`
//! first, then `ProductName`; trademark marks are dropped, and a generic word ("Launcher",
//! "Setup") or a name with a comma counts as no name. The exe is found from a running process
//! of that name, else the app's App Paths entry; with neither, or no usable name, the caller
//! keeps the bare exe name. Written for the hub from the Win32 documentation (clean-room).

/// Descriptions that say what kind of file it is, not which app (compared without case).
const GENERIC: &[&str] = &[
    "bootstrappackagedgame",
    "launcher",
    "game launcher",
    "setup",
    "installer",
    "updater",
    "application",
    "app",
    "program",
    "executable",
];

/// The longest name kept; a longer description is a sentence, not a name.
const MAX_LEN: usize = 48;

/// A version-info string as a permission line's name, or `None` when it isn't one.
pub fn clean_program_name(raw: &str) -> Option<String> {
    let mut s = raw.replace(['\u{2122}', '\u{00AE}', '\u{00A9}'], " ");
    for mark in ["(tm)", "(r)", "(c)"] {
        while let Some(at) = s.to_ascii_lowercase().find(mark) {
            s.replace_range(at..at + mark.len(), " ");
        }
    }
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let bad = s.is_empty()
        || s.chars().count() > MAX_LEN
        || s.contains(',')
        || s.chars().any(char::is_control)
        || s.to_ascii_lowercase().contains(".exe")
        || GENERIC.iter().any(|g| s.eq_ignore_ascii_case(g));
    (!bad).then_some(s)
}

/// `FileDescription` when it is a name, else `ProductName` when that is one.
pub fn pick_program_name(description: Option<&str>, product: Option<&str>) -> Option<String> {
    description
        .and_then(clean_program_name)
        .or_else(|| product.and_then(clean_program_name))
}

/// The friendly name of the app `label` (an exe name without `.exe`, as the app watch uses),
/// from its running process or its App Paths entry. `None` off Windows, or when no exe is
/// found or it has no usable name: the caller then says the bare label.
pub fn program_name(label: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let path = crate::win::running_exe(label).or_else(|| crate::win::app_path(label))?;
        let (description, product) = crate::win::version_names(&path);
        pick_program_name(description.as_deref(), product.as_deref())
    }
    #[cfg(not(windows))]
    {
        let _ = label;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_go_and_generic_or_comma_names_are_no_name() {
        assert_eq!(
            clean_program_name("  OBS  Studio ").as_deref(),
            Some("OBS Studio")
        );
        assert_eq!(
            clean_program_name("Example\u{2122} Suite\u{00AE}").as_deref(),
            Some("Example Suite")
        );
        assert_eq!(
            clean_program_name("Example(TM) Tool (r)").as_deref(),
            Some("Example Tool")
        );
        for none in [
            "",
            "   ",
            "BootstrapPackagedGame",
            "launcher",
            "Setup",
            "Example, Inc. tool",
            "example.exe",
            "\u{2122}",
            "A description that goes on far too long to be the name of any app",
        ] {
            assert_eq!(clean_program_name(none), None, "{none:?}");
        }
    }

    #[test]
    fn the_description_comes_first_then_the_product() {
        assert_eq!(
            pick_program_name(Some("Example Studio"), Some("Example")).as_deref(),
            Some("Example Studio")
        );
        assert_eq!(
            pick_program_name(Some("Launcher"), Some("Example Game")).as_deref(),
            Some("Example Game")
        );
        assert_eq!(pick_program_name(Some("Setup"), Some("Updater")), None);
        assert_eq!(pick_program_name(None, None), None);
    }

    /// Window-free: reads the version info of Windows' own command processor.
    #[cfg(windows)]
    #[test]
    fn a_system_exe_has_a_name_and_an_unknown_app_none() {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let (d, p) = crate::win::version_names(&format!(r"{root}\System32\cmd.exe"));
        let name = pick_program_name(d.as_deref(), p.as_deref()).expect("cmd.exe has a name");
        assert!(!name.to_ascii_lowercase().contains(".exe"), "{name}");
        assert_eq!(program_name("hideout-no-such-app-991"), None);
    }
}
