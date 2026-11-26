// SPDX-License-Identifier: MIT
//! Apps whose OK holds for one run only (#1452).
//!
//! An adopted app's OK is bound to its program file and its arguments. That says what runs only
//! when the program file is the app. A script runner, or a program that is part of Windows, runs
//! whatever it is handed: a script named by an argument, a module name, a command string, or
//! whatever that script loads later. Hashing the program file and comparing the arguments does
//! not cover those, so for such an app nothing is remembered: the user's Allow holds for the
//! next start, Hideout asks again at every start after it, and "Keep my OK" is never offered.
//!
//! The rule is judged here from the program's canonical path, and the registry enforces it
//! wherever an OK is stored ([`crate::Registry::approve_listed`]), so no caller (the window, the
//! control pipe) can store a remembered OK for such an app.

pub use hideout_contract::permission::OneRun;

/// Why the app whose program is at `path` gets a one-run OK, or `None` when its OK is bound to
/// the program file as usual. `path` is canonical (links resolved, long names), as
/// [`crate::keyboard::canonical_path`] gives it. `windows_folder` is the Windows folder as the
/// system reports it; containment is by whole path components, letter case ignored, so
/// `C:\WindowsApps\x.exe` is not inside `C:\Windows`.
///
/// A program inside the Windows folder answers [`OneRun::Windows`] whatever its name; any other
/// answers [`OneRun::Scripts`] when its file name is a script runner's
/// ([`hideout_contract::permission::tool_runs_scripts`]). A renamed copy of a runner outside the
/// Windows folder is not caught by its name: its arguments are still part of what was allowed.
///
/// With no Windows folder to compare with (`None` or empty) no program can be shown to be
/// outside it, so every program gets a one-run OK ([`OneRun::Scripts`]): the rule fails closed.
pub fn one_run_rule(path: &str, windows_folder: Option<&str>) -> Option<OneRun> {
    if path.is_empty() {
        return None;
    }
    let Some(windows) = windows_folder.filter(|w| !w.is_empty()) else {
        return Some(OneRun::Scripts);
    };
    if crate::dev_folder::path_within(windows, path) {
        return Some(OneRun::Windows);
    }
    let file = path.rsplit(['\\', '/']).next().unwrap_or("");
    hideout_contract::permission::tool_runs_scripts(file).then_some(OneRun::Scripts)
}

/// The Windows folder of this PC (`C:\Windows`), canonical. Asked of the system first; the
/// environment's `SystemRoot` (whatever the parent process set) is used only when the system
/// gives no answer, and only when it names a folder that is there. `None` when neither does:
/// [`one_run_rule`] then gives every program a one-run OK.
pub fn system_windows_folder() -> Option<String> {
    static FOLDER: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FOLDER.get_or_init(read_windows_folder).clone()
}

#[cfg(windows)]
fn read_windows_folder() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    let mut buf = vec![0u16; 512];
    // SAFETY: `buf` is a writable buffer of `buf.len()` UTF-16 units, and that length is passed.
    let n = unsafe { GetSystemWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let asked = if n == 0 || n >= buf.len() {
        None
    } else {
        String::from_utf16(&buf[..n]).ok()
    };
    let raw = asked.or_else(|| folder_from(std::env::var_os("SystemRoot")))?;
    // The same form the program's path has, so the two compare component by component.
    Some(crate::keyboard::canonical_path(std::path::Path::new(&raw)).unwrap_or(raw))
}

/// `value` (the environment's `SystemRoot`) when it is an absolute path to a folder that is
/// there; anything else is no answer.
#[cfg(windows)]
fn folder_from(value: Option<std::ffi::OsString>) -> Option<String> {
    let raw = value?.into_string().ok()?;
    let path = std::path::Path::new(&raw);
    (path.is_absolute() && path.is_dir()).then_some(raw)
}

#[cfg(not(windows))]
fn read_windows_folder() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: Option<&str> = Some(r"C:\Windows");

    #[test]
    fn a_program_in_the_windows_folder_gets_a_one_run_ok_by_whole_components() {
        for inside in [
            r"C:\Windows\System32\rundll32.exe",
            r"c:\windows\system32\RUNDLL32.EXE",
            r"C:\Windows\notepad.exe",
            // A script runner there is "part of Windows" first.
            r"C:\Windows\System32\cmd.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ] {
            assert_eq!(one_run_rule(inside, WIN), Some(OneRun::Windows), "{inside}");
        }
        for outside in [
            // A name that only starts like the folder is another folder.
            r"C:\WindowsApps\tool.exe",
            r"C:\Windows.old\System32\tool.exe",
            r"C:\Windows2\tool.exe",
            r"D:\Windows\tool.exe",
            r"C:\Apps\Windows\tool.exe",
            r"C:\Apps\Foxglove\foxglove.exe",
            // The folder itself is not a program inside it.
            r"C:\Windows",
            "",
        ] {
            assert_eq!(one_run_rule(outside, WIN), None, "{outside}");
        }
        // The folder is the one handed in, in any letter case.
        assert_eq!(
            one_run_rule(r"E:\WinNT\system32\tool.exe", Some(r"e:\winnt")),
            Some(OneRun::Windows)
        );
    }

    #[test]
    fn with_no_windows_folder_known_every_program_gets_a_one_run_ok() {
        for path in [
            r"C:\Windows\notepad.exe",
            r"C:\Apps\Foxglove\foxglove.exe",
            r"D:\Tools\tool.exe",
        ] {
            assert_eq!(one_run_rule(path, None), Some(OneRun::Scripts), "{path}");
            assert_eq!(
                one_run_rule(path, Some("")),
                Some(OneRun::Scripts),
                "{path}"
            );
        }
        assert_eq!(one_run_rule("", None), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_fallback_takes_only_an_absolute_folder_that_is_there() {
        let here = std::env::temp_dir();
        assert_eq!(
            folder_from(Some(here.clone().into_os_string())),
            Some(here.to_string_lossy().into_owned())
        );
        assert_eq!(folder_from(None), None);
        assert_eq!(folder_from(Some("Windows".into())), None);
        assert_eq!(
            folder_from(Some(here.join("hideout-no-such-folder").into_os_string())),
            None
        );
    }

    #[test]
    fn a_script_runner_gets_a_one_run_ok_wherever_it_is() {
        for runner in [
            r"C:\Python312\python.exe",
            r"C:\Python312\PythonW.EXE",
            r"C:\Program Files\nodejs\node.exe",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ] {
            assert_eq!(one_run_rule(runner, WIN), Some(OneRun::Scripts), "{runner}");
            assert_eq!(
                one_run_rule(runner, None),
                Some(OneRun::Scripts),
                "{runner}"
            );
        }
        // A folder named like a runner does not make the program one.
        assert_eq!(one_run_rule(r"C:\python\foxglove.exe", WIN), None);
    }

    #[cfg(windows)]
    #[test]
    fn the_system_names_its_windows_folder() {
        let folder = system_windows_folder().expect("the system names its Windows folder");
        assert!(std::path::Path::new(&folder).is_dir(), "{folder}");
        let inside = format!(r"{folder}\System32\kernel32.dll");
        assert_eq!(one_run_rule(&inside, Some(&folder)), Some(OneRun::Windows));
    }
}
