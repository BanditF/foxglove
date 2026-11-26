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
pub fn one_run_rule(path: &str, windows_folder: Option<&str>) -> Option<OneRun> {
    if path.is_empty() {
        return None;
    }
    if windows_folder.is_some_and(|w| !w.is_empty() && crate::dev_folder::path_within(w, path)) {
        return Some(OneRun::Windows);
    }
    let file = path.rsplit(['\\', '/']).next().unwrap_or("");
    hideout_contract::permission::tool_runs_scripts(file).then_some(OneRun::Scripts)
}

/// The Windows folder of this PC (`C:\Windows`), canonical. Asked of the system, not read from
/// the environment (a process's `SystemRoot` is whatever its parent set). `None` when the
/// system gives no answer: the Windows half of the rule then matches nothing, the script-runner
/// half still holds.
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
    if n == 0 || n >= buf.len() {
        return None;
    }
    let raw = String::from_utf16(&buf[..n]).ok()?;
    // The same form the program's path has, so the two compare component by component.
    Some(crate::keyboard::canonical_path(std::path::Path::new(&raw)).unwrap_or(raw))
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
        // No answer from the system: nothing is taken for the Windows folder.
        assert_eq!(one_run_rule(r"C:\Windows\notepad.exe", None), None);
        assert_eq!(one_run_rule(r"C:\Windows\notepad.exe", Some("")), None);
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
