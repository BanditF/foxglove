//! Start with Windows (user decision, 2026-10-26; #741): a switch in Settings, on by default, kept
//! in step with Windows' own startup list both ways.
//!
//! - The entry is a shortcut in the user's Startup folder, `Foxglove.lnk`, with Windows' on/off
//!   state for it in `HKCU\...\Explorer\StartupApproved\StartupFolder` (value `Foxglove.lnk`). So
//!   it shows in Settings › Apps › Startup and Task Manager's Startup apps, and their toggle works.
//!   Until 0.2 it was the `HKCU\...\Run` value `Foxglove`; Windows skipped it at sign-in on
//!   2026-11-04 (Shell-Core 9707: the Run values listed before a disabled one never started, #741),
//!   so startup moves a Run value starting this exe to the shortcut, with its on/off state.
//! - The switch shows Windows' live state: on when the shortcut is there and Windows doesn't have
//!   it disabled. It's read again whenever the Settings window gets the focus (the user may have
//!   changed it in Windows meanwhile); nothing watches the registry.
//! - Turning the switch on or off writes what Windows' own toggle writes: the StartupApproved
//!   value enabled (02) or disabled (03 and the time). On also makes the shortcut when it's missing
//!   or starts another exe (the user asked).
//! - Startup never turns it back on (#684): disabled in Windows, the switch shows off; a shortcut
//!   Foxglove made and someone removed stays removed. With the switch on, the shortcut is made
//!   only when Foxglove never made one, or repaired when the file it starts no longer exists. A
//!   shortcut starting another exe that exists (a second install) is left alone.
//! - Uninstall runs `foxglove.exe --uninstall-cleanup` (`app/wix/main.wxs`): it removes the
//!   shortcut and the old Run value only when they start this exe.
//! - Dev builds (debug, or an exe in a cargo `target` dir or under `worktrees`) never write at
//!   startup: those files get cleaned, and an entry naming a deleted lab build starts nothing.
//! - Test isolation runs (`FOXGLOVE_CONFIG_DIR` set) never touch the user's entry or registry.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The old Run value's name, the same as 0.1's MSI.
pub const VALUE: &str = "Foxglove";
/// `HKCU\` + this.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The shortcut in the Startup folder.
pub const SHORTCUT: &str = "Foxglove.lnk";
/// Windows' on/off state for Startup-folder entries (`HKCU\` + this, value [`SHORTCUT`]).
pub const APPROVED_FOLDER: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";
/// The same for Run values (value [`VALUE`]).
pub const APPROVED_RUN: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

/// The `[autostart]` section of the settings store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutostartSettings {
    /// The switch, as Windows had it when last read.
    pub start_with_windows: bool,
    /// Foxglove has made its shortcut once: a missing one was removed by someone, not new.
    pub made: bool,
}

impl Default for AutostartSettings {
    fn default() -> Self {
        Self {
            start_with_windows: true,
            made: false,
        }
    }
}

/// The old Run value, behind a trait so tests never touch the registry.
pub trait RunKey {
    fn get(&self) -> std::io::Result<Option<String>>;
    fn remove(&self) -> std::io::Result<()>;
}

/// The Startup-folder shortcut.
pub trait Entry {
    /// The exe the shortcut starts; `None` when there is no shortcut.
    fn target(&self) -> std::io::Result<Option<PathBuf>>;
    /// Makes (or replaces) the shortcut, starting `exe`.
    fn create(&self, exe: &Path) -> std::io::Result<()>;
    fn remove(&self) -> std::io::Result<()>;
}

/// One StartupApproved value: Windows' on/off toggle for an entry.
pub trait Approved {
    /// `Some(true)` enabled, `Some(false)` disabled, `None` no value (Windows counts it enabled).
    fn get(&self) -> std::io::Result<Option<bool>>;
    fn set(&self, enabled: bool) -> std::io::Result<()>;
    fn clear(&self) -> std::io::Result<()>;
}

/// A StartupApproved value as Windows writes it: 12 bytes, the first 02 (enabled) or 03
/// (disabled), then the FILETIME of the disabling (0 while enabled).
pub fn approved_bytes(enabled: bool, filetime: u64) -> [u8; 12] {
    let mut b = [0u8; 12];
    if enabled {
        b[0] = 2;
    } else {
        b[0] = 3;
        b[4..].copy_from_slice(&filetime.to_le_bytes());
    }
    b
}

/// Whether a StartupApproved value says enabled: Windows writes 02 or 06 for on and 01, 03 or 07
/// for off, so an even first byte is on.
pub fn approved_enabled(bytes: &[u8]) -> bool {
    bytes.first().is_none_or(|b| b & 1 == 0)
}

/// Where Windows keeps Foxglove's startup entry: the shortcut and its toggle, and the old Run
/// value and its toggle (moved at startup).
pub struct Places<'a> {
    pub entry: &'a dyn Entry,
    pub approved: &'a dyn Approved,
    pub run: &'a dyn RunKey,
    pub run_approved: &'a dyn Approved,
}

/// Windows' live state of the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Live {
    /// The shortcut is there, starting this file, and Windows has it on.
    On(PathBuf),
    /// The shortcut is there, but turned off in Windows' startup list.
    Disabled(PathBuf),
    /// No shortcut.
    Missing,
}

pub fn live(p: &Places) -> std::io::Result<Live> {
    Ok(match p.entry.target()? {
        None => Live::Missing,
        Some(t) if p.approved.get()? == Some(false) => Live::Disabled(t),
        Some(t) => Live::On(t),
    })
}

/// The Run value for `exe`: its path in quotes, as the MSI wrote it.
pub fn command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Whether two exe paths (or a Run value and a path) name the same file: ignoring case, quotes and
/// surrounding space.
fn same(value: &str, command: &str) -> bool {
    let bare = |s: &str| s.trim().trim_matches('"').to_lowercase();
    bare(value) == bare(command)
}

fn same_path(a: &Path, b: &Path) -> bool {
    same(&a.to_string_lossy(), &b.to_string_lossy())
}

/// The file a Run value starts: the quoted path, or the whole value when it isn't quoted.
fn target(value: &str) -> PathBuf {
    let v = value.trim();
    match v.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest).into(),
        None => v.into(),
    }
}

/// Why this build must not write the entry at startup, if it mustn't: a debug build, or an exe in
/// a cargo `target` dir or under `worktrees` (lab and agent builds, cleaned without notice).
pub fn dev_build(exe: &Path, debug: bool) -> Option<&'static str> {
    if debug {
        return Some("a debug build");
    }
    // Split by hand: a Windows path read on Linux (the tests) has no `\` components.
    let path = exe.to_string_lossy().to_lowercase();
    let mut dirs = path.split(['/', '\\']).rev().skip(1); // the folders, not the file name
    let dev = dirs.any(|c| matches!(c, "target" | "_targets" | "worktrees"));
    dev.then_some("a dev build (cargo target or worktree path)")
}

/// What startup did.
#[derive(Debug, PartialEq, Eq)]
pub enum Startup {
    /// The switch is off: nothing written.
    Off,
    /// Turned off in Windows' startup list: the switch shows off; nothing written.
    DisabledInWindows,
    /// The shortcut Foxglove made was removed: the switch shows off; not made again.
    RemovedOutside,
    /// The shortcut already starts this exe.
    Unchanged,
    /// There was none and Foxglove never made one: it now starts this exe.
    Written,
    /// The shortcut started a file that no longer exists (shown); it now starts this exe.
    Replaced(String),
    /// The shortcut starts another exe that exists (shown): left alone.
    KeptOther(String),
    /// The Run value starting this exe moved to the shortcut (`true`: it was off in Windows and
    /// stays off).
    Moved(bool),
    /// The entry is missing or stale, but this is a dev build (why).
    Skipped(&'static str),
}

/// At startup: moves an old Run value starting this exe (or a file that's gone) to the shortcut,
/// then takes Windows' live state into `s` and writes only what [`Startup`] says. Never turns the
/// entry back on. `dev` from [`dev_build`].
pub fn reconcile(
    s: &mut AutostartSettings,
    exe: &Path,
    dev: Option<&'static str>,
    p: &Places,
    exists: &dyn Fn(&Path) -> bool,
) -> std::io::Result<Startup> {
    let me = command(exe);
    // The old Run value: this exe's, or one whose file is gone, moves; another exe's stays, and
    // with no shortcut it is that install's entry (no second one is made beside it).
    if let Some(v) = p.run.get()? {
        if !(same(&v, &me) || !exists(&target(&v))) && p.entry.target()?.is_none() {
            return Ok(Startup::KeptOther(v));
        }
        let ours = same(&v, &me) || !exists(&target(&v));
        if ours {
            if let Some(why) = dev {
                return Ok(Startup::Skipped(why));
            }
            let off = p.run_approved.get()? == Some(false);
            p.entry.create(exe)?;
            p.approved.set(!off)?;
            p.run.remove()?;
            p.run_approved.clear()?;
            s.start_with_windows = !off;
            s.made = true;
            return Ok(Startup::Moved(off));
        }
    }
    match live(p)? {
        Live::Disabled(_) => {
            s.start_with_windows = false;
            Ok(Startup::DisabledInWindows)
        }
        Live::On(t) => {
            s.start_with_windows = true;
            if same_path(&t, exe) {
                return Ok(Startup::Unchanged);
            }
            if exists(&t) {
                return Ok(Startup::KeptOther(t.display().to_string()));
            }
            if let Some(why) = dev {
                return Ok(Startup::Skipped(why));
            }
            p.entry.create(exe)?;
            s.made = true;
            Ok(Startup::Replaced(t.display().to_string()))
        }
        Live::Missing if !s.start_with_windows => Ok(Startup::Off),
        Live::Missing if s.made => {
            s.start_with_windows = false;
            Ok(Startup::RemovedOutside)
        }
        Live::Missing => {
            if let Some(why) = dev {
                return Ok(Startup::Skipped(why));
            }
            p.entry.create(exe)?;
            p.approved.set(true)?;
            s.made = true;
            Ok(Startup::Written)
        }
    }
}

/// The one log line for what startup did, and why.
pub fn describe(result: &std::io::Result<Startup>, exe: &Path) -> String {
    let me = exe.display();
    let what = match result {
        Ok(Startup::Off) => "off in Settings; nothing written".into(),
        Ok(Startup::DisabledInWindows) => {
            "turned off in Windows' startup list; the switch shows off".into()
        }
        Ok(Startup::RemovedOutside) => {
            "the Startup shortcut was removed; the switch shows off and it isn't made again".into()
        }
        Ok(Startup::Unchanged) => format!("the Startup shortcut starts {me}; nothing to do"),
        Ok(Startup::Written) => format!("made the Startup shortcut; it starts {me}"),
        Ok(Startup::Replaced(old)) => {
            format!(
                "the Startup shortcut started {old}, which no longer exists; it now starts {me}"
            )
        }
        Ok(Startup::KeptOther(other)) => format!(
            "the Startup shortcut starts {other}, which exists; left alone (turn the switch off \
             and on in Settings to start {me} instead)"
        ),
        Ok(Startup::Moved(off)) => format!(
            "moved from the Run value to a Startup shortcut starting {me}{}",
            if *off {
                " (off in Windows, as it was)"
            } else {
                ""
            }
        ),
        Ok(Startup::Skipped(why)) => {
            format!("the entry is missing or stale, but this is {why}; not written")
        }
        Err(e) => format!("couldn't read or write the startup entry: {e}"),
    };
    format!("Foxglove: Start with Windows: {what}")
}

/// The switch: on makes the shortcut if it doesn't start this exe and sets Windows' toggle on; off
/// sets Windows' toggle off, as Windows' own switch does (the shortcut stays).
pub fn apply(on: bool, exe: &Path, p: &Places) -> std::io::Result<()> {
    if on {
        match p.entry.target()? {
            Some(t) if same_path(&t, exe) => {}
            _ => p.entry.create(exe)?,
        }
    }
    p.approved.set(on)
}

/// Uninstall: removes the shortcut and the old Run value where they start this exe. True when
/// something was removed.
pub fn cleanup(exe: &Path, p: &Places) -> std::io::Result<bool> {
    let mut removed = false;
    if p.entry.target()?.is_some_and(|t| same_path(&t, exe)) {
        p.entry.remove()?;
        p.approved.clear()?;
        removed = true;
    }
    if p.run.get()?.is_some_and(|v| same(&v, &command(exe))) {
        p.run.remove()?;
        p.run_approved.clear()?;
        removed = true;
    }
    Ok(removed)
}

#[cfg(windows)]
mod windows_places {
    use super::{Approved, Entry, RunKey};
    use std::path::{Path, PathBuf};
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE, REG_BINARY};
    use winreg::{RegKey, RegValue};

    fn not_found<T>(r: std::io::Result<T>) -> std::io::Result<Option<T>> {
        match r {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A string value under `HKCU\<key>` (the Run value).
    pub struct RegString {
        pub key: String,
        pub name: String,
    }

    impl RunKey for RegString {
        fn get(&self) -> std::io::Result<Option<String>> {
            let Some(key) = not_found(RegKey::predef(HKEY_CURRENT_USER).open_subkey(&self.key))?
            else {
                return Ok(None);
            };
            not_found(key.get_value::<String, _>(&self.name))
        }
        fn remove(&self) -> std::io::Result<()> {
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let Some(key) = not_found(hkcu.open_subkey_with_flags(&self.key, KEY_SET_VALUE))?
            else {
                return Ok(());
            };
            not_found(key.delete_value(&self.name)).map(|_| ())
        }
    }

    impl RegString {
        /// Seeds a Run value (tests only: Foxglove no longer writes one).
        #[cfg(test)]
        pub fn set(&self, command: &str) -> std::io::Result<()> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&self.key)?;
            key.set_value(&self.name, &command)
        }
    }

    /// A StartupApproved value under `HKCU\<key>`.
    pub struct RegApproved {
        pub key: String,
        pub name: String,
    }

    impl Approved for RegApproved {
        fn get(&self) -> std::io::Result<Option<bool>> {
            let Some(key) = not_found(RegKey::predef(HKEY_CURRENT_USER).open_subkey(&self.key))?
            else {
                return Ok(None);
            };
            Ok(
                not_found(key.get_raw_value(&self.name))?
                    .map(|v| super::approved_enabled(&v.bytes)),
            )
        }
        fn set(&self, enabled: bool) -> std::io::Result<()> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&self.key)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64 / 100 + 116_444_736_000_000_000);
            let bytes = super::approved_bytes(enabled, now).to_vec();
            key.set_raw_value(
                &self.name,
                &RegValue {
                    bytes: bytes.into(),
                    vtype: REG_BINARY,
                },
            )
        }
        fn clear(&self) -> std::io::Result<()> {
            RegString {
                key: self.key.clone(),
                name: self.name.clone(),
            }
            .remove()
        }
    }

    /// `<dir>\Foxglove.lnk`.
    pub struct Shortcut {
        pub dir: PathBuf,
    }

    impl Shortcut {
        fn path(&self) -> PathBuf {
            self.dir.join(super::SHORTCUT)
        }
    }

    /// COM on this thread for one call: initialised here if it wasn't (the UI thread's is).
    fn with_com<T>(f: impl FnOnce() -> windows::core::Result<T>) -> std::io::Result<T> {
        use windows::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
        };
        // SAFETY: balanced below; S_FALSE (already initialised) also needs its CoUninitialize, and
        // RPC_E_CHANGED_MODE (another model already) needs none.
        let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let r = f();
        if init.is_ok() {
            // SAFETY: pairs the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
        r.map_err(|e| std::io::Error::other(e.to_string()))
    }

    impl Entry for Shortcut {
        fn target(&self) -> std::io::Result<Option<PathBuf>> {
            use windows::core::{Interface, HSTRING};
            use windows::Win32::System::Com::{
                CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
            };
            use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
            let lnk = self.path();
            if !lnk.is_file() {
                return Ok(None);
            }
            with_com(|| {
                // SAFETY: a ShellLink object loads the file and copies its path into our buffer.
                unsafe {
                    let link: IShellLinkW =
                        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                    link.cast::<IPersistFile>()?
                        .Load(&HSTRING::from(lnk.as_os_str()), STGM_READ)?;
                    let mut buf = [0u16; 1024];
                    link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
                    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    Ok(Some(PathBuf::from(String::from_utf16_lossy(&buf[..n]))))
                }
            })
        }

        fn create(&self, exe: &Path) -> std::io::Result<()> {
            use windows::core::{Interface, HSTRING};
            use windows::Win32::System::Com::{
                CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER,
            };
            use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
            std::fs::create_dir_all(&self.dir)?;
            let lnk = self.path();
            with_com(|| {
                // SAFETY: a ShellLink object with this exe, saved to the Startup folder.
                unsafe {
                    let link: IShellLinkW =
                        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                    link.SetPath(&HSTRING::from(exe.as_os_str()))?;
                    if let Some(dir) = exe.parent() {
                        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
                    }
                    link.SetDescription(&HSTRING::from("Foxglove"))?;
                    link.cast::<IPersistFile>()?
                        .Save(&HSTRING::from(lnk.as_os_str()), true)
                }
            })
        }

        fn remove(&self) -> std::io::Result<()> {
            match std::fs::remove_file(self.path()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }

    /// The user's Startup folder (FOLDERID_Startup).
    pub fn startup_dir() -> std::io::Result<PathBuf> {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Startup, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
        // SAFETY: the returned string is copied, then freed with CoTaskMemFree as documented.
        unsafe {
            let p = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let dir = PathBuf::from(p.to_string().unwrap_or_default());
            CoTaskMemFree(Some(p.0 as *const _));
            Ok(dir)
        }
    }

    /// The real places, or a test root's: `root` names a folder for the shortcut and an `HKCU`
    /// subkey standing in for `Software\Microsoft\Windows\CurrentVersion` (tests only).
    pub struct Owned {
        pub entry: Shortcut,
        pub approved: RegApproved,
        pub run: RegString,
        pub run_approved: RegApproved,
    }

    impl Owned {
        pub fn user() -> std::io::Result<Self> {
            Ok(Self::at(startup_dir()?, ""))
        }

        /// `reg_base` empty: the real keys; else every key moves under `HKCU\<reg_base>`.
        pub fn at(dir: PathBuf, reg_base: &str) -> Self {
            let key = |k: &str| match reg_base {
                "" => k.to_string(),
                base => format!(
                    r"{base}\{}",
                    k.rsplit_once(r"CurrentVersion\").map_or(k, |(_, r)| r)
                ),
            };
            Self {
                entry: Shortcut { dir },
                approved: RegApproved {
                    key: key(super::APPROVED_FOLDER),
                    name: super::SHORTCUT.into(),
                },
                run: RegString {
                    key: key(super::RUN_KEY),
                    name: super::VALUE.into(),
                },
                run_approved: RegApproved {
                    key: key(super::APPROVED_RUN),
                    name: super::VALUE.into(),
                },
            }
        }

        pub fn places(&self) -> super::Places<'_> {
            super::Places {
                entry: &self.entry,
                approved: &self.approved,
                run: &self.run,
                run_approved: &self.run_approved,
            }
        }
    }
}

/// A test isolation run: the user's entry is never touched.
#[cfg(windows)]
fn isolated() -> bool {
    std::env::var_os("FOXGLOVE_CONFIG_DIR").is_some()
}

/// `foxglove.exe --uninstall-cleanup`, run by the MSI's uninstall as the user: removes this
/// exe's startup entry and exits. Opens no window. True when this was that run.
pub fn uninstall_entry() -> bool {
    if std::env::args_os()
        .nth(1)
        .is_none_or(|a| a != "--uninstall-cleanup")
    {
        return false;
    }
    #[cfg(windows)]
    if let (Ok(exe), Ok(owned)) = (std::env::current_exe(), windows_places::Owned::user()) {
        if let Err(e) = cleanup(&exe, &owned.places()) {
            eprintln!("Foxglove: uninstall: Start with Windows: {e}");
        }
    }
    true
}

/// The switch as last read from Windows: 0 = not read yet, 1 = off, 2 = on.
static ON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn settings() -> AutostartSettings {
    super::store::try_load::<AutostartSettings>("autostart").unwrap_or_default()
}

fn remember(s: &AutostartSettings, before: &AutostartSettings) {
    ON.store(
        if s.start_with_windows { 2 } else { 1 },
        std::sync::atomic::Ordering::Relaxed,
    );
    if s != before {
        if let Err(e) = super::store::save("autostart", s) {
            eprintln!("Foxglove: Start with Windows: couldn't save the switch: {e}");
        }
    }
}

pub fn enabled() -> bool {
    match ON.load(std::sync::atomic::Ordering::Relaxed) {
        0 => {
            refresh();
            ON.load(std::sync::atomic::Ordering::Relaxed) == 2
        }
        v => v == 2,
    }
}

/// Reads Windows' live state into the switch (the Settings window got the focus; first use).
/// Writes nothing to Windows.
pub fn refresh() {
    let before = settings();
    let mut s = before.clone();
    #[cfg(windows)]
    if !isolated() {
        if let Ok(owned) = windows_places::Owned::user() {
            match live(&owned.places()) {
                Ok(Live::On(_)) => s.start_with_windows = true,
                Ok(Live::Disabled(_)) => s.start_with_windows = false,
                Ok(Live::Missing) if s.made => s.start_with_windows = false,
                Ok(Live::Missing) | Err(_) => {}
            }
        }
    }
    remember(&s, &before);
}

/// At app start, once.
pub fn start() {
    #[cfg(windows)]
    {
        if isolated() {
            eprintln!(
                "Foxglove: Start with Windows: a test run (FOXGLOVE_CONFIG_DIR set); the startup \
                 entry is not touched"
            );
            return;
        }
        let (Ok(exe), Ok(owned)) = (std::env::current_exe(), windows_places::Owned::user()) else {
            return;
        };
        let before = settings();
        let mut s = before.clone();
        let dev = dev_build(&exe, cfg!(debug_assertions));
        let result = reconcile(&mut s, &exe, dev, &owned.places(), &|p| p.is_file());
        eprintln!("{}", describe(&result, &exe));
        remember(&s, &before);
    }
}

/// The Settings switch turned on or off: writes Windows' toggle (and the shortcut, on) and saves it.
pub fn set_enabled(on: bool) -> Result<(), String> {
    let before = settings();
    let mut s = before.clone();
    #[cfg(windows)]
    if !isolated() {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let owned = windows_places::Owned::user().map_err(|e| e.to_string())?;
        apply(on, &exe, &owned.places()).map_err(|e| format!("Start with Windows: {e}"))?;
        s.made = true;
        match on {
            true => eprintln!(
                "Foxglove: Start with Windows: switched on in Settings; the Startup shortcut starts {}",
                exe.display()
            ),
            false => eprintln!("Foxglove: Start with Windows: switched off in Settings"),
        }
    }
    s.start_with_windows = on;
    remember(&s, &before);
    Ok(())
}

thread_local! {
    /// Why the last change of the switch failed, shown under it.
    static MESSAGE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// Whether the window was focused last frame: its gaining focus reads Windows' state again.
    static FOCUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The switch, for the Settings page.
pub fn checkbox(ui: &mut egui::Ui) {
    let focused = ui.ctx().input(|i| i.viewport().focused).unwrap_or(false);
    if focused && !FOCUSED.replace(focused) {
        refresh();
    } else if !focused {
        FOCUSED.set(false);
    }
    let mut on = enabled();
    if ui
        .checkbox(&mut on, "Start with Windows")
        .on_hover_text(
            "Start Foxglove when you sign in to Windows. The same switch as Foxglove's in \
             Windows Settings › Apps › Startup and Task Manager's Startup apps.",
        )
        .changed()
    {
        MESSAGE.set(set_enabled(on).err());
    }
    if let Some(m) = MESSAGE.with_borrow(|m| m.clone()) {
        ui.colored_label(super::theme::palette(ui).danger, m);
    }
}

#[cfg(test)]
mod tests;
