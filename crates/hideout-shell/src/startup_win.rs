// SPDX-License-Identifier: MIT
//! The real Startup folder and StartupApproved registry value for [`crate::startup_switch`]
//! (Windows only). Only `hideout.exe` builds these (`real`); no test does: in a test build every
//! constructor refuses, so a test can never reach the user's Startup folder or registry.

use std::path::{Path, PathBuf};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_BINARY, REG_OPTION_NON_VOLATILE,
    REG_VALUE_TYPE,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Startup, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT,
};

use crate::autostart::{Approval, Link, StartupFolder, LINK_NAME};
use crate::startup_switch::{approved_bytes, approved_enabled};

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
        if !lnk.is_file() {
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
        std::fs::create_dir_all(&self.dir).map_err(io)?;
        let lnk = self.path();
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
        match std::fs::remove_file(self.path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
            _ => Ok(()),
        }
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file()
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

/// The install folder the Hideout installer recorded (`HKCU\Software\Hideout\Installer`, value
/// `App`), the same record `find_installed` reads for the other products.
pub fn installed_folder() -> Option<PathBuf> {
    let (_, bytes) = query(INSTALLER_KEY, "App")?;
    let (pairs, _) = bytes.as_chunks::<2>();
    let u16s: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let n = u16s.iter().position(|&c| c == 0).unwrap_or(u16s.len());
    let s = String::from_utf16_lossy(&u16s[..n]);
    (!s.is_empty()).then(|| PathBuf::from(s))
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
