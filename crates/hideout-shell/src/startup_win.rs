// SPDX-License-Identifier: MIT
//! The real Startup folder and StartupApproved registry value for [`crate::startup_switch`]
//! (Windows only). Only `hideout.exe` builds these (`real`); no test does: in a test build every
//! constructor refuses, so a test can never reach the user's Startup folder or registry.

use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Storage::FileSystem::GetDriveTypeW;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_OPTION_NON_VOLATILE,
    REG_SZ, REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Startup, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
};

use crate::autostart::{Approval, Link, StartupFolder, LINK_NAME};
use crate::startup_switch::{approved_bytes, approved_enabled};

/// The key under HKCU that holds the StartupApproved values, watched for changes.
pub const APPROVED_PARENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";
const INSTALLER_KEY: &str = r"Software\Hideout\Installer";

fn refuse_in_tests() -> Result<(), String> {
    if cfg!(test) {
        return Err("a test build never touches the real Startup folder or registry".into());
    }
    // A lab or test hub (its own folder) never writes the user's Startup folder either.
    for k in ["HIDEOUT_CONFIG_DIR", "HIDEOUT_HOME"] {
        if std::env::var_os(k).is_some_and(|v| !v.is_empty()) {
            return Err(format!("{k} is set: a test run writes nothing"));
        }
    }
    Ok(())
}

fn io(e: impl ToString) -> String {
    e.to_string()
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`: a symlink, a junction, a mount point or any other reparse tag.
const REPARSE: u32 = 0x400;

/// Refuses (fails closed) when `p` is a reparse point or cannot be inspected. A path that does not
/// exist is fine: the write creates it.
fn refuse_reparse(p: &Path) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.file_attributes() & REPARSE != 0 || m.file_type().is_symlink() => Err(format!(
            "{} is a link or junction; Hideout will not write through it",
            p.display()
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("cannot check {}: {e}", p.display())),
    }
}

/// The Startup folder and the shortcut in it must both be plain (no reparse point).
fn check_plain(dir: &Path, lnk: &Path) -> Result<(), String> {
    refuse_reparse(dir)?;
    refuse_reparse(lnk)
}

/// `<Startup folder>\Hideout.lnk`.
pub struct RealStartup {
    dir: PathBuf,
}

impl RealStartup {
    pub fn new() -> Result<Self, String> {
        refuse_in_tests()?;
        // SAFETY: the returned string is copied, then freed with CoTaskMemFree as documented.
        let dir = unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None).map_err(io)?;
            let dir = PathBuf::from(p.to_string().map_err(io)?);
            CoTaskMemFree(Some(p.0 as *const _));
            dir
        };
        Ok(Self { dir })
    }

    fn path(&self) -> PathBuf {
        self.dir.join(LINK_NAME)
    }

    /// The Startup folder (for a change watch; reads nothing).
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

/// COM on this thread for one call.
fn with_com<T>(f: impl FnOnce() -> windows::core::Result<T>) -> Result<T, String> {
    // SAFETY: balanced below; RPC_E_CHANGED_MODE needs no CoUninitialize.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let r = f();
    if init.is_ok() {
        // SAFETY: pairs the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    r.map_err(io)
}

impl StartupFolder for RealStartup {
    fn read(&self) -> Option<Link> {
        let lnk = self.path();
        if check_plain(&self.dir, &lnk).is_err() || !lnk.is_file() {
            return None;
        }
        with_com(|| {
            // SAFETY: a ShellLink object loads the file and copies its strings into our buffers.
            unsafe {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.cast::<IPersistFile>()?
                    .Load(&HSTRING::from(lnk.as_os_str()), STGM_READ)?;
                let mut path = [0u16; 1024];
                link.GetPath(&mut path, std::ptr::null_mut(), 0)?;
                let mut args = [0u16; 1024];
                link.GetArguments(&mut args)?;
                let s = |b: &[u16]| {
                    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                    String::from_utf16_lossy(&b[..n])
                };
                Ok(Link {
                    target: PathBuf::from(s(&path)),
                    args: s(&args).trim().to_string(),
                })
            }
        })
        .ok()
    }

    fn write(&mut self, link: &Link) -> Result<(), String> {
        refuse_in_tests()?;
        let lnk = self.path();
        check_plain(&self.dir, &lnk)?;
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        check_plain(&self.dir, &lnk)?;
        with_com(|| {
            // SAFETY: a ShellLink object with this exe, saved into the Startup folder.
            unsafe {
                let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                sl.SetPath(&HSTRING::from(link.target.as_os_str()))?;
                sl.SetArguments(&HSTRING::from(link.args.as_str()))?;
                if let Some(dir) = link.target.parent() {
                    sl.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
                }
                sl.SetDescription(&HSTRING::from("Hideout"))?;
                sl.cast::<IPersistFile>()?
                    .Save(&HSTRING::from(lnk.as_os_str()), true)
            }
        })
    }

    fn delete(&mut self) -> Result<(), String> {
        refuse_in_tests()?;
        check_plain(&self.dir, &self.path())?;
        match std::fs::remove_file(self.path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
            _ => Ok(()),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        refuse_reparse(path).is_ok() && path.is_file()
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The `Hideout.lnk` value under StartupApproved\StartupFolder.
pub struct RealApproval;

impl RealApproval {
    pub fn new() -> Result<Self, String> {
        refuse_in_tests()?;
        Ok(Self)
    }
}

/// Reads `HKCU\<key>` value `name` as raw bytes with its type; `None` when absent.
fn query(key: &str, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let key = wide(key);
    let name = wide(name);
    let mut h = HKEY::default();
    // SAFETY: valid NUL-terminated strings; the handle is closed below; the buffer is sized by
    // the first call's answer.
    unsafe {
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            None,
            KEY_QUERY_VALUE,
            &mut h,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let mut ty = REG_VALUE_TYPE::default();
        let mut len = 0u32;
        let mut out = None;
        if RegQueryValueExW(
            h,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut ty),
            None,
            Some(&mut len),
        ) == ERROR_SUCCESS
        {
            let mut buf = vec![0u8; len as usize];
            if RegQueryValueExW(
                h,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            ) == ERROR_SUCCESS
            {
                buf.truncate(len as usize);
                out = Some((ty, buf));
            }
        }
        let _ = RegCloseKey(h);
        out
    }
}

impl Approval for RealApproval {
    fn read(&self) -> Option<bool> {
        query(APPROVED_KEY, LINK_NAME).map(|(_, b)| approved_enabled(&b))
    }

    fn write(&mut self, enabled: bool) -> Result<(), String> {
        refuse_in_tests()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64 / 100 + 116_444_736_000_000_000);
        let bytes = approved_bytes(enabled, now);
        let key = wide(APPROVED_KEY);
        let name = wide(LINK_NAME);
        let mut h = HKEY::default();
        // SAFETY: valid NUL-terminated strings; the handle is closed below.
        unsafe {
            let e = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut h,
                None,
            );
            if e != ERROR_SUCCESS {
                return Err(format!("registry: {}", e.0));
            }
            let e = RegSetValueExW(h, PCWSTR(name.as_ptr()), None, REG_BINARY, Some(&bytes));
            let _ = RegCloseKey(h);
            if e != ERROR_SUCCESS {
                return Err(format!("registry: {}", e.0));
            }
        }
        Ok(())
    }

    fn delete(&mut self) -> Result<(), String> {
        refuse_in_tests()?;
        let key = wide(APPROVED_KEY);
        let name = wide(LINK_NAME);
        let mut h = HKEY::default();
        // SAFETY: valid NUL-terminated strings; the handle is closed below.
        unsafe {
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                KEY_SET_VALUE,
                &mut h,
            ) != ERROR_SUCCESS
            {
                return Ok(()); // no key, no value
            }
            let _ = RegDeleteValueW(h, PCWSTR(name.as_ptr())); // absent is success
            let _ = RegCloseKey(h);
        }
        Ok(())
    }
}

/// A REG_SZ value's text, up to the first NUL. Any other type (EXPAND_SZ, BINARY, ...) is `None`.
fn reg_sz(ty: REG_VALUE_TYPE, bytes: &[u8]) -> Option<String> {
    if ty != REG_SZ {
        return None;
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let u16s: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let n = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
    let s = String::from_utf16_lossy(&u16s[..n]);
    (!s.is_empty()).then_some(s)
}

/// `X:\...` where `X:\` is a fixed drive per `fixed`: not a mapped network drive, a removable or
/// optical drive. UNC and `\\?\` forms are not drive paths here.
fn on_fixed_drive(path: &str, fixed: impl Fn(&str) -> bool) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' && fixed(&path[..3])
}

fn drive_is_fixed(root: &str) -> bool {
    const DRIVE_FIXED: u32 = 3;
    let w = wide(root);
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) == DRIVE_FIXED }
}

/// The install folder the Hideout installer recorded (`HKCU\Software\Hideout\Installer`, value
/// `App`), the same record `find_installed` reads for the other products. `None` (so "not
/// installed", nothing written) unless the value is REG_SZ and names a folder on a fixed local
/// drive.
pub fn installed_folder() -> Option<PathBuf> {
    let (ty, bytes) = query(INSTALLER_KEY, "App")?;
    let s = reg_sz(ty, &bytes)?;
    on_fixed_drive(&s, drive_is_fixed).then(|| PathBuf::from(s))
}

/// The switch over the real Startup folder and registry, for this exe. `HubPaths::overridden`
/// makes it a test-run switch that writes nothing.
pub fn real(
    exe: PathBuf,
    overridden: bool,
) -> Result<crate::startup_switch::Switch<RealStartup, RealApproval>, String> {
    Ok(crate::startup_switch::Switch::new(
        crate::autostart::Context { exe, overridden },
        installed_folder(),
        RealStartup::new()?,
        RealApproval::new()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hideout-h52-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn junction(link: &Path, target: &Path) {
        let ok = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J failed");
    }

    fn unjunction(link: &Path) {
        let _ = std::fs::remove_dir(link); // removes the junction, not its target
    }

    #[test]
    fn plain_dir_with_or_without_lnk_passes() {
        let d = temp("plain");
        assert!(check_plain(&d, &d.join(LINK_NAME)).is_ok());
        std::fs::write(d.join(LINK_NAME), b"x").unwrap();
        assert!(check_plain(&d, &d.join(LINK_NAME)).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn junction_as_the_startup_folder_is_refused() {
        let d = temp("jdir");
        let real = d.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = d.join("Startup");
        junction(&link, &real);
        let r = check_plain(&link, &link.join(LINK_NAME));
        assert!(r.unwrap_err().contains("link or junction"));
        unjunction(&link);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn junction_as_the_shortcut_path_is_refused() {
        let d = temp("jlnk");
        let real = d.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = d.join(LINK_NAME);
        junction(&link, &real);
        assert!(check_plain(&d, &link).is_err());
        assert!(!RealStartup { dir: d.clone() }.exists(&link));
        assert!(RealStartup { dir: d.clone() }.read().is_none());
        unjunction(&link);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A file symlink needs Developer Mode or elevation; where it cannot be made, the junction
    /// tests above still cover the reparse check.
    #[test]
    fn file_symlink_as_the_shortcut_is_refused() {
        let d = temp("slnk");
        let target = d.join("other.lnk");
        std::fs::write(&target, b"x").unwrap();
        let link = d.join(LINK_NAME);
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            eprintln!("skipped: no symlink privilege");
        } else {
            assert!(check_plain(&d, &link).is_err());
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn uninspectable_path_fails_closed() {
        // A NUL in the path cannot be queried: an error other than NotFound is a refusal.
        assert!(refuse_reparse(Path::new("C:\\bad\0name")).is_err());
    }

    #[test]
    fn test_builds_never_reach_the_real_folder_or_registry() {
        assert!(RealStartup::new().is_err());
        assert!(RealApproval::new().is_err());
    }

    #[test]
    fn registry_value_must_be_reg_sz() {
        let sz: Vec<u8> = "C:\\Hideout\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(reg_sz(REG_SZ, &sz).as_deref(), Some("C:\\Hideout"));
        assert_eq!(reg_sz(REG_BINARY, &sz), None);
        assert_eq!(reg_sz(REG_VALUE_TYPE(2), &sz), None); // REG_EXPAND_SZ
        assert_eq!(reg_sz(REG_SZ, &[0, 0]), None); // empty
    }

    #[test]
    fn only_fixed_drive_letters_pass() {
        let fixed = |root: &str| root.eq_ignore_ascii_case("C:\\");
        assert!(on_fixed_drive("C:\\Program Files\\Hideout", fixed));
        assert!(!on_fixed_drive("Z:\\Hideout", fixed)); // mapped network drive
        assert!(!on_fixed_drive("\\\\server\\share\\Hideout", fixed));
        assert!(!on_fixed_drive("\\\\?\\C:\\Hideout", fixed));
        assert!(!on_fixed_drive("C:Hideout", fixed));
        assert!(!on_fixed_drive("", fixed));
    }

    #[test]
    fn a_nonexistent_drive_is_not_fixed() {
        assert!(!drive_is_fixed("?:\\"));
    }
}
