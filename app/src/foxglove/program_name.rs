//! A watched program's own name for the Plugins tab's permission lines (#991): "see when OBS
//! Studio opens or closes" rather than "obs64". The name comes from the program file's version
//! resource (FileDescription, as Task Manager shows it, then ProductName), read from the running
//! process's file, else from where Windows' App Paths says the program is installed. Without
//! either, the line keeps the plugin's own spelling or the bare file name
//! ([`Manifest::describe_permission`]).
//!
//! Cheap by design: one lookup per program per visit to the Plugins page, on the first frame
//! that shows its line, kept (a miss too) until the page changes ([`page_left`]). No timer.

use std::collections::HashMap;
use std::sync::Mutex;

use foxglove_plugin_api::manifest::Manifest;
use foxglove_plugin_api::permission::Permission;

/// Names by lower-case file name (`None`: no name found), kept for one visit to the page.
static NAMES: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// A version resource's value fit for a permission line: trimmed, not empty, short, and naming
/// no file (a FileDescription like "obs64.exe" doesn't count).
fn fit(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= 40 && !text.to_lowercase().contains(".exe"))
        .then(|| text.to_string())
}

/// The program's name from its version strings: FileDescription first, then ProductName.
pub fn pick(description: Option<&str>, product: Option<&str>) -> Option<String> {
    description.and_then(fit).or_else(|| product.and_then(fit))
}

/// `p`'s line with an app watch's app named by `lookup` (the program's own name for a file
/// name) when it gives one; otherwise the manifest's line.
pub fn describe_with(
    m: &Manifest,
    p: &Permission,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> String {
    if let Permission::AppsWatch(exe) = p {
        if let Some(name) = lookup(exe) {
            return format!("see when {name} opens or closes");
        }
    }
    m.describe_permission(p)
}

/// `p`'s line for the install dialog and Plugins tab, with a watched program's own name when it
/// runs or is installed.
pub fn describe(m: &Manifest, p: &Permission) -> String {
    describe_with(m, p, name)
}

/// A program's own name for its file name (`obs64.exe`), looked up once per page visit.
fn name(exe: &str) -> Option<String> {
    let key = exe.trim().to_lowercase();
    let mut guard = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let kept = guard.get_or_insert_with(Default::default);
    kept.entry(key)
        .or_insert_with_key(|k| {
            let file = if k.ends_with(".exe") {
                k.clone()
            } else {
                format!("{k}.exe")
            };
            let path = super::app_icon::running_paths(&[file.as_str()])
                .into_iter()
                .next()
                .or_else(|| installed_path(&file))?;
            let (description, product) = version_strings(&path);
            pick(description.as_deref(), product.as_deref())
        })
        .clone()
}

/// The Plugins page was left: the next visit looks again (a program installed meanwhile).
pub fn page_left() {
    *NAMES.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Where Windows' App Paths says `file` is installed (the user's entry, then the machine's).
#[cfg(windows)]
fn installed_path(file: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };
    let sub = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{file}");
    let sub: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
        .into_iter()
        .find_map(|root| {
            let mut buf = [0u16; 1024];
            let mut bytes = (buf.len() * 2) as u32;
            // SAFETY: reads the key's default string into `buf`, whose size in bytes is `bytes`;
            // a REG_EXPAND_SZ value comes back expanded.
            let got = unsafe {
                RegGetValueW(
                    root,
                    PCWSTR(sub.as_ptr()),
                    PCWSTR::null(),
                    RRF_RT_REG_SZ,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    Some(&mut bytes),
                )
            };
            if got != ERROR_SUCCESS {
                return None;
            }
            let len = (bytes as usize / 2).min(buf.len());
            let text = String::from_utf16_lossy(&buf[..len]);
            let path = text
                .trim_end_matches('\0')
                .trim()
                .trim_matches('"')
                .to_string();
            (!path.is_empty()).then_some(path)
        })
}

#[cfg(not(windows))]
fn installed_path(_file: &str) -> Option<String> {
    None
}

/// The program file's FileDescription and ProductName, in the file's own language first, else
/// US English (as the Knob's sessions read FileDescription in `info-screen`).
#[cfg(windows)]
fn version_strings(path: &str) -> (Option<String>, Option<String>) {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let file = PCWSTR(wide.as_ptr());
    // SAFETY: Win32 calls on a buffer of the size Windows asked for; every pointer
    // VerQueryValueW returns points into `data` and is read only while `data` lives.
    unsafe {
        let size = GetFileVersionInfoSizeW(file, None);
        if size == 0 {
            return (None, None);
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(file, None, size, data.as_mut_ptr().cast()).is_err() {
            return (None, None);
        }
        let block = data.as_ptr().cast();
        // A value as u16s: `len` counts bytes for binary values and characters for strings.
        let query = |key: &str, bytes: bool| -> Option<Vec<u16>> {
            let key: Vec<u16> = key.encode_utf16().chain(std::iter::once(0)).collect();
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            let found = VerQueryValueW(block, PCWSTR(key.as_ptr()), &mut ptr, &mut len).as_bool();
            let len = if bytes { len / 2 } else { len } as usize;
            (found && !ptr.is_null() && len > 0)
                .then(|| std::slice::from_raw_parts(ptr as *const u16, len).to_vec())
        };
        // The translations: (language, code page) pairs of u16s.
        let mut langs: Vec<(u16, u16)> = query("\\VarFileInfo\\Translation", true)
            .map(|t| t.as_chunks::<2>().0.iter().map(|&[l, c]| (l, c)).collect())
            .unwrap_or_default();
        langs.extend([(0x0409, 0x04B0), (0x0409, 0x04E4)]);
        let string = |name: &str| {
            langs.iter().find_map(|(lang, cp)| {
                let text = query(
                    &format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{name}"),
                    false,
                )?;
                let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
                let text = String::from_utf16_lossy(&text[..end]).trim().to_string();
                (!text.is_empty()).then_some(text)
            })
        };
        (string("FileDescription"), string("ProductName"))
    }
}

#[cfg(not(windows))]
fn version_strings(_path: &str) -> (Option<String>, Option<String>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_then_product_then_none() {
        assert_eq!(
            pick(Some("OBS Studio"), Some("OBS")).as_deref(),
            Some("OBS Studio")
        );
        assert_eq!(
            pick(Some("  "), Some("OBS Studio")).as_deref(),
            Some("OBS Studio")
        );
        assert_eq!(
            pick(Some("obs64.EXE"), Some("OBS Studio")).as_deref(),
            Some("OBS Studio")
        );
        let long = "x".repeat(41);
        assert_eq!(pick(Some(&long), None), None);
        assert_eq!(pick(None, None), None);
    }

    #[test]
    fn app_watch_line_takes_the_programs_name_else_the_manifests() {
        let m = Manifest::parse(
            "id = \"com.example.obs\"\nname = \"Obs64 Status\"\nversion = \"1.0.0\"\napi = \"1.0\"\n\
             run = [\"s.exe\"]\npermissions = [\"apps.watch:obs64.exe\", \"devices.read\"]\n",
            foxglove_plugin_api::API_VERSION,
        )
        .unwrap();
        let watch = &m.permissions[0];
        let named = |_: &str| Some("OBS Studio".to_string());
        assert_eq!(
            describe_with(&m, watch, named),
            "see when OBS Studio opens or closes"
        );
        // No program found: the plugin's spelling, with no file name.
        assert_eq!(
            describe_with(&m, watch, |_| None),
            "see when Obs64 opens or closes"
        );
        // Other permissions keep their lines and never look a program up.
        let devices = &m.permissions[1];
        assert_eq!(
            describe_with(&m, devices, |_| panic!("looked up")),
            m.describe_permission(devices)
        );
    }

    /// The live lookup on this PC: whatever it finds names no file.
    #[test]
    fn live_lookup_names_no_file() {
        for exe in ["explorer.exe", "notepad.exe", "no-such-program-991.exe"] {
            if let Some(n) = name(exe) {
                assert!(!n.to_lowercase().contains(".exe"), "{exe}: {n}");
            }
        }
        assert_eq!(name("no-such-program-991.exe"), None);
        page_left();
    }
}
