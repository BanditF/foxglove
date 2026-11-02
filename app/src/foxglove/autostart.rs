//! Start with Windows (user decision, 2026-10-26): a switch in Settings, on by default.
//!
//! - The app is the only owner of the Run value (#444):
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Foxglove`, the quoted exe path.
//!   The per-user MSI writes none; Windows' Settings › Apps › Startup shows it with its own toggle.
//!   (0.1's per-machine MSI could write the same value; it's kept as it is when it starts this exe.)
//! - Uninstall runs `foxglove.exe --uninstall-cleanup` (`app/wix/main.wxs`), which removes the
//!   value only when it starts this exe, so another install's entry is left alone.
//! - At startup, with the switch on, the value is written only when it is missing or the file it
//!   names no longer exists (#684). A value naming another exe that exists (a second install, a
//!   renamed copy) is left alone: startup never takes it over. With the switch off, startup leaves
//!   the value alone: turning it off removed it, and it is never added back.
//! - Dev builds (debug, or an exe in a cargo `target` dir or under `worktrees`) never write it at
//!   startup, even when it's missing: those files get cleaned, and a value naming a deleted lab
//!   build leaves Windows nothing to start.
//! - Turning the switch on writes the value for this exe, whatever it named: the user asked.
//!   Turning it off removes it.
//! - Test isolation runs (`FOXGLOVE_CONFIG_DIR` set) never touch the value, at startup or on the
//!   switch: the user's autostart isn't the test's.
//! - Only at startup and on the switch: nothing watches the registry.

use serde::{Deserialize, Serialize};

/// The Run value's name, the same as the MSI's.
pub const VALUE: &str = "Foxglove";
/// `HKCU\` + this.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The `[autostart]` section of the settings store.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutostartSettings {
    pub start_with_windows: bool,
}

impl Default for AutostartSettings {
    fn default() -> Self {
        Self {
            start_with_windows: true,
        }
    }
}

/// The Run value, behind a trait so tests never touch the registry.
pub trait RunKey {
    fn get(&self) -> std::io::Result<Option<String>>;
    fn set(&self, command: &str) -> std::io::Result<()>;
    fn remove(&self) -> std::io::Result<()>;
}

/// The current user's Run key in the registry.
#[cfg(windows)]
pub struct Registry;

#[cfg(windows)]
impl RunKey for Registry {
    fn get(&self) -> std::io::Result<Option<String>> {
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let key = match hkcu.open_subkey(RUN_KEY) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        match key.get_value::<String, _>(VALUE) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn set(&self, command: &str) -> std::io::Result<()> {
        let (key, _) =
            winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
        key.set_value(VALUE, &command)
    }

    fn remove(&self) -> std::io::Result<()> {
        let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let key = match hkcu.open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_SET_VALUE) {
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        match key.delete_value(VALUE) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// The Run value for `exe`: its path in quotes, as the MSI writes it.
pub fn command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Whether a Run value starts this exe: the same path, ignoring case and the quotes.
fn same(value: &str, command: &str) -> bool {
    let bare = |s: &str| s.trim().trim_matches('"').to_lowercase();
    bare(value) == bare(command)
}

/// The file a Run value starts: the quoted path, or the whole value when it isn't quoted.
fn target(value: &str) -> std::path::PathBuf {
    let v = value.trim();
    match v.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest).into(),
        None => v.into(),
    }
}

/// Why this build must not write the Run value at startup, if it mustn't: a debug build, or an
/// exe in a cargo `target` dir or under `worktrees` (lab and agent builds, cleaned without notice).
pub fn dev_build(exe: &std::path::Path, debug: bool) -> Option<&'static str> {
    if debug {
        return Some("a debug build");
    }
    // Split by hand: a Windows path read on Linux (the tests) has no `\` components.
    let path = exe.to_string_lossy().to_lowercase();
    let mut dirs = path.split(['/', '\\']).rev().skip(1); // the folders, not the file name
    let dev = dirs.any(|c| matches!(c, "target" | "_targets" | "worktrees"));
    dev.then_some("a dev build (cargo target or worktree path)")
}

/// What startup did to the Run value.
#[derive(Debug, PartialEq, Eq)]
pub enum Startup {
    /// The switch is off: the value was left as it is.
    Off,
    /// The value already starts this exe.
    Unchanged,
    /// The value was missing; it now starts this exe.
    Written,
    /// The value named a file that no longer exists (shown); it now starts this exe.
    Replaced(String),
    /// The value starts another exe that exists (shown): left alone.
    KeptOther(String),
    /// The value was missing (`None`) or names a missing file, but this is a dev build (why).
    Skipped(Option<String>, &'static str),
}

/// At startup, with the switch on: writes the Run value only when it is missing or names a file
/// that doesn't exist, and never for a dev build (`dev`, from [`dev_build`]). A value starting
/// another exe that exists is left alone. Off, it does nothing.
pub fn reconcile(
    on: bool,
    command: &str,
    dev: Option<&'static str>,
    key: &dyn RunKey,
    exists: &dyn Fn(&std::path::Path) -> bool,
) -> std::io::Result<Startup> {
    if !on {
        return Ok(Startup::Off);
    }
    let stale = match key.get()? {
        Some(v) if same(&v, command) => return Ok(Startup::Unchanged),
        Some(v) if exists(&target(&v)) => return Ok(Startup::KeptOther(v)),
        stale => stale,
    };
    if let Some(why) = dev {
        return Ok(Startup::Skipped(stale, why));
    }
    key.set(command)?;
    Ok(match stale {
        Some(v) => Startup::Replaced(v),
        None => Startup::Written,
    })
}

/// The one log line for what startup did, and why.
pub fn describe(result: &std::io::Result<Startup>, command: &str) -> String {
    let what = match result {
        Ok(Startup::Off) => "off in Settings; the Run value is left as it is".into(),
        Ok(Startup::Unchanged) => format!("already starts {command}; nothing to do"),
        Ok(Startup::Written) => format!("the Run value was missing; it now starts {command}"),
        Ok(Startup::Replaced(old)) => {
            format!("the Run value named {old}, which no longer exists; it now starts {command}")
        }
        Ok(Startup::KeptOther(other)) => format!(
            "the Run value starts {other}, which exists; left alone (turn the switch off and on \
             in Settings to start {command} instead)"
        ),
        Ok(Startup::Skipped(None, why)) => {
            format!("the Run value is missing, but this is {why}; not written")
        }
        Ok(Startup::Skipped(Some(old), why)) => format!(
            "the Run value names {old}, which no longer exists, but this is {why}; not written"
        ),
        Err(e) => format!("couldn't read or write the Run value: {e}"),
    };
    format!("Foxglove: Start with Windows: {what}")
}

/// A test isolation run: the user's Run value is never touched.
#[cfg(windows)]
fn isolated() -> bool {
    std::env::var_os("FOXGLOVE_CONFIG_DIR").is_some()
}

/// Uninstall: removes the Run value if it starts `command` (this exe). True when removed.
pub fn cleanup(command: &str, key: &dyn RunKey) -> std::io::Result<bool> {
    if key.get()?.is_some_and(|v| same(&v, command)) {
        key.remove()?;
        return Ok(true);
    }
    Ok(false)
}

/// `foxglove.exe --uninstall-cleanup`, run by the MSI's uninstall as the user: removes this
/// exe's Run value and exits. Opens no window. True when this was that run.
pub fn uninstall_entry() -> bool {
    if std::env::args_os()
        .nth(1)
        .is_none_or(|a| a != "--uninstall-cleanup")
    {
        return false;
    }
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        if let Err(e) = cleanup(&command(&exe), &Registry) {
            eprintln!("Foxglove: uninstall: Start with Windows: {e}");
        }
    }
    true
}

/// The switch: on writes the Run value, off removes it.
pub fn apply(on: bool, command: &str, key: &dyn RunKey) -> std::io::Result<()> {
    if on {
        key.set(command)
    } else {
        key.remove()
    }
}

/// The switch, read from the store once and then kept here: 0 = not read yet, 1 = off, 2 = on.
static ON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn enabled() -> bool {
    use std::sync::atomic::Ordering;
    match ON.load(Ordering::Relaxed) {
        0 => {
            let on = super::store::try_load::<AutostartSettings>("autostart")
                .unwrap_or_default()
                .start_with_windows;
            ON.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        v => v == 2,
    }
}

/// At app start, once.
pub fn start() {
    #[cfg(windows)]
    {
        if isolated() {
            eprintln!(
                "Foxglove: Start with Windows: a test run (FOXGLOVE_CONFIG_DIR set); the Run \
                 value is not touched"
            );
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let me = command(&exe);
        let dev = dev_build(&exe, cfg!(debug_assertions));
        let result = reconcile(enabled(), &me, dev, &Registry, &|p| p.is_file());
        eprintln!("{}", describe(&result, &me));
    }
}

/// The Settings switch turned on or off: saves it and updates the Run value.
pub fn set_enabled(on: bool) -> Result<(), String> {
    super::store::save(
        "autostart",
        &AutostartSettings {
            start_with_windows: on,
        },
    )
    .map_err(|e| e.to_string())?;
    ON.store(if on { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
    #[cfg(windows)]
    if !isolated() {
        let me = command(&std::env::current_exe().map_err(|e| e.to_string())?);
        apply(on, &me, &Registry).map_err(|e| format!("Start with Windows: {e}"))?;
        match on {
            true => eprintln!("Foxglove: Start with Windows: switched on in Settings; starts {me}"),
            false => eprintln!("Foxglove: Start with Windows: switched off in Settings; removed"),
        }
    }
    Ok(())
}

thread_local! {
    /// Why the last change of the switch failed, shown under it.
    static MESSAGE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The switch, for the Settings page.
pub fn checkbox(ui: &mut egui::Ui) {
    let mut on = enabled();
    if ui
        .checkbox(&mut on, "Start with Windows")
        .on_hover_text("Start Foxglove when you sign in to Windows")
        .changed()
    {
        MESSAGE.set(set_enabled(on).err());
    }
    if let Some(m) = MESSAGE.with_borrow(|m| m.clone()) {
        ui.colored_label(super::theme::palette(ui).danger, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The Run value in memory.
    #[derive(Default)]
    struct Fake(RefCell<Option<String>>, RefCell<u32>);

    impl RunKey for Fake {
        fn get(&self) -> std::io::Result<Option<String>> {
            Ok(self.0.borrow().clone())
        }
        fn set(&self, command: &str) -> std::io::Result<()> {
            *self.1.borrow_mut() += 1;
            *self.0.borrow_mut() = Some(command.into());
            Ok(())
        }
        fn remove(&self) -> std::io::Result<()> {
            *self.1.borrow_mut() += 1;
            *self.0.borrow_mut() = None;
            Ok(())
        }
    }

    const ME: &str = r#""C:\Program Files\Foxglove\foxglove.exe""#;

    #[test]
    fn on_by_default() {
        assert!(AutostartSettings::default().start_with_windows);
        let parsed: AutostartSettings = toml::from_str("").unwrap();
        assert!(
            parsed.start_with_windows,
            "a store without the section means on"
        );
    }

    /// Only these files exist, for `reconcile`.
    fn only(files: &'static [&'static str]) -> impl Fn(&std::path::Path) -> bool {
        move |p| files.iter().any(|f| std::path::Path::new(f) == p)
    }
    const ME_FILE: &str = r"C:\Program Files\Foxglove\foxglove.exe";
    const OTHER: &str = r#""C:\Users\fuchs\splitkeeb\app-main\foxglove-9f29911.exe""#;
    const OTHER_FILE: &str = r"C:\Users\fuchs\splitkeeb\app-main\foxglove-9f29911.exe";
    const GONE: &str = r#""C:\worktrees\_targets\oncall-673-app\release\foxglove.exe""#;
    const DEV: Option<&str> = Some("a dev build");

    fn holding(v: Option<&str>) -> Fake {
        Fake(RefCell::new(v.map(Into::into)), RefCell::new(0))
    }

    #[test]
    fn startup_writes_a_missing_value() {
        let key = Fake::default();
        let r = reconcile(true, ME, None, &key, &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::Written);
        assert_eq!(key.0.borrow().as_deref(), Some(ME));
    }

    #[test]
    fn startup_rewrites_a_value_whose_file_is_gone() {
        let key = holding(Some(GONE));
        let r = reconcile(true, ME, None, &key, &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::Replaced(GONE.into()));
        assert_eq!(key.0.borrow().as_deref(), Some(ME));
    }

    #[test]
    fn startup_never_takes_over_another_existing_exe() {
        // #684: a renamed copy or a second install that exists keeps the value, dev build or not.
        for dev in [None, DEV] {
            let key = holding(Some(OTHER));
            let r = reconcile(true, ME, dev, &key, &only(&[ME_FILE, OTHER_FILE])).unwrap();
            assert_eq!(r, Startup::KeptOther(OTHER.into()));
            assert_eq!(
                (key.0.borrow().as_deref(), *key.1.borrow()),
                (Some(OTHER), 0)
            );
        }
    }

    #[test]
    fn startup_with_the_same_exe_is_a_no_op() {
        let key = holding(Some(ME));
        let r = reconcile(true, ME, None, &key, &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::Unchanged);
        assert_eq!(*key.1.borrow(), 0);
    }

    #[test]
    fn a_dev_build_never_writes_at_startup() {
        let key = Fake::default();
        let r = reconcile(true, ME, DEV, &key, &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::Skipped(None, "a dev build"));
        assert_eq!((key.0.borrow().as_deref(), *key.1.borrow()), (None, 0));
        let key = holding(Some(GONE));
        let r = reconcile(true, ME, DEV, &key, &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::Skipped(Some(GONE.into()), "a dev build"));
        assert_eq!(
            (key.0.borrow().as_deref(), *key.1.borrow()),
            (Some(GONE), 0)
        );
    }

    #[test]
    fn the_settings_switch_claims_the_value_from_another_exe() {
        let key = holding(Some(OTHER));
        apply(true, ME, &key).unwrap();
        assert_eq!(key.0.borrow().as_deref(), Some(ME));
    }

    #[test]
    fn dev_paths_are_recognised() {
        use std::path::Path;
        let dev = |p: &str| dev_build(Path::new(p), false).is_some();
        assert!(dev(
            r"C:\Users\fuchs\splitkeeb\worktrees\_targets\oncall-673-app\release\foxglove.exe"
        ));
        assert!(dev(
            r"C:\Users\fuchs\splitkeeb\foxglove\app\target\release\foxglove.exe"
        ));
        assert!(dev(
            r"C:\Users\fuchs\splitkeeb\worktrees\autostart\app\Target\release\foxglove.exe"
        ));
        assert!(!dev(ME_FILE));
        assert!(!dev(OTHER_FILE));
        assert!(!dev(
            r"C:\Users\fuchs\AppData\Local\Programs\Foxglove\foxglove.exe"
        ));
        assert!(
            dev_build(Path::new(ME_FILE), true).is_some(),
            "debug builds are dev builds"
        );
    }

    #[test]
    fn the_target_is_the_quoted_path() {
        let me = std::path::PathBuf::from(ME_FILE);
        assert_eq!(target(ME), me);
        assert_eq!(target(&format!("{ME} --tray")), me);
        assert_eq!(target(&format!("  {ME_FILE} ")), me);
    }

    #[test]
    fn the_log_line_says_what_and_why() {
        let line = |r: Startup| describe(&Ok(r), ME);
        assert!(line(Startup::Written).contains("was missing"));
        assert!(line(Startup::Replaced(GONE.into())).contains("no longer exists"));
        assert!(line(Startup::KeptOther(OTHER.into())).contains("left alone"));
        assert!(line(Startup::Unchanged).contains("nothing to do"));
        assert!(line(Startup::Skipped(None, "a dev build")).contains("not written"));
        assert!(line(Startup::Off).starts_with("Foxglove: Start with Windows: off"));
    }

    #[test]
    fn uninstall_removes_only_this_exes_value() {
        let key = Fake(RefCell::new(Some(ME.into())), RefCell::new(0));
        assert!(cleanup(ME, &key).unwrap());
        assert_eq!(*key.0.borrow(), None);
        assert!(!cleanup(ME, &key).unwrap(), "nothing left to remove");
        // Another install's value (a second copy, a dev build) stays.
        let other = r#""D:\dev\foxglove.exe""#;
        *key.0.borrow_mut() = Some(other.into());
        assert!(!cleanup(ME, &key).unwrap());
        assert_eq!(key.0.borrow().as_deref(), Some(other));
    }

    #[test]
    fn the_msis_own_value_is_kept_as_it_is() {
        // 0.1's MSI wrote the same name and the quoted path; case and quoting don't matter.
        let key = Fake(
            RefCell::new(Some(r#""c:\program files\foxglove\FOXGLOVE.EXE""#.into())),
            RefCell::new(0),
        );
        let r = reconcile(true, ME, None, &key, &only(&[ME_FILE])).unwrap();
        assert_eq!(r, Startup::Unchanged);
        assert_eq!(
            *key.1.borrow(),
            0,
            "no write: one value, not a second entry"
        );
    }

    #[test]
    fn turned_off_it_is_removed_and_never_added_back() {
        let key = Fake::default();
        apply(true, ME, &key).unwrap();
        assert_eq!(key.0.borrow().as_deref(), Some(ME));
        apply(false, ME, &key).unwrap();
        assert_eq!(*key.0.borrow(), None);
        for _ in 0..3 {
            let r = reconcile(false, ME, None, &key, &only(&[])).unwrap();
            assert_eq!(r, Startup::Off);
        }
        assert_eq!(*key.0.borrow(), None, "startup never re-adds it");
        // Off leaves someone else's value (the MSI added later) alone, too.
        *key.0.borrow_mut() = Some(ME.into());
        let writes = *key.1.borrow();
        reconcile(false, ME, None, &key, &only(&[])).unwrap();
        assert_eq!(
            (key.0.borrow().as_deref(), *key.1.borrow()),
            (Some(ME), writes)
        );
    }

    #[test]
    fn the_value_is_the_quoted_exe_path() {
        assert_eq!(
            command(std::path::Path::new(
                r"C:\Program Files\Foxglove\foxglove.exe"
            )),
            ME
        );
    }
}
