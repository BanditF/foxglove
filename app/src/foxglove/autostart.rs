//! Start with Windows (user decision, 2026-10-26): a switch in Settings, on by default.
//!
//! - It owns the same Run value the MSI's optional "Start with Windows" feature writes:
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `Foxglove`, the quoted exe path
//!   (`app/wix/main.wxs`). One value, never a second entry.
//! - At startup, with the switch on, a missing value or one naming another exe is set to this
//!   one. With it off, startup leaves the value alone: turning it off removed it, and it is
//!   never added back.
//! - Turning it on writes the value; turning it off removes it.
//! - Only at startup and on the switch: nothing watches the registry.
//! - Test runs (`FOXGLOVE_CONFIG_DIR` set) and debug builds never touch the value at startup,
//!   so a test build can't take over the user's autostart.

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

/// What startup did to the Run value.
#[derive(Debug, PartialEq, Eq)]
pub enum Startup {
    /// The switch is off: the value was left as it is.
    Off,
    /// The value already starts this exe.
    Unchanged,
    /// The value was missing or named another exe; it now starts this one.
    Written,
}

/// At startup: with the switch on, makes the Run value start `command`. Off, it does nothing.
pub fn reconcile(on: bool, command: &str, key: &dyn RunKey) -> std::io::Result<Startup> {
    if !on {
        return Ok(Startup::Off);
    }
    if key.get()?.is_some_and(|v| same(&v, command)) {
        return Ok(Startup::Unchanged);
    }
    key.set(command)?;
    Ok(Startup::Written)
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
        if cfg!(debug_assertions) || std::env::var_os("FOXGLOVE_CONFIG_DIR").is_some() {
            return; // a test or development run: the user's autostart isn't ours to change
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        match reconcile(enabled(), &command(&exe), &Registry) {
            Ok(Startup::Written) => {
                eprintln!("Foxglove: Start with Windows now starts {}", exe.display())
            }
            Ok(_) => {}
            Err(e) => eprintln!("Foxglove: Start with Windows: {e}"),
        }
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
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        apply(on, &command(&exe), &Registry).map_err(|e| format!("Start with Windows: {e}"))?;
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

    #[test]
    fn startup_adds_a_missing_value_and_fixes_one_for_another_exe() {
        let key = Fake::default();
        assert_eq!(reconcile(true, ME, &key).unwrap(), Startup::Written);
        assert_eq!(key.0.borrow().as_deref(), Some(ME));
        *key.0.borrow_mut() = Some(r#""C:\old\foxglove.exe""#.into());
        assert_eq!(reconcile(true, ME, &key).unwrap(), Startup::Written);
        assert_eq!(key.0.borrow().as_deref(), Some(ME));
    }

    #[test]
    fn the_msis_own_value_is_kept_as_it_is() {
        // The MSI writes the same name and the quoted path; case and quoting don't matter.
        let key = Fake(
            RefCell::new(Some(r#""c:\program files\foxglove\FOXGLOVE.EXE""#.into())),
            RefCell::new(0),
        );
        assert_eq!(reconcile(true, ME, &key).unwrap(), Startup::Unchanged);
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
            assert_eq!(reconcile(false, ME, &key).unwrap(), Startup::Off);
        }
        assert_eq!(*key.0.borrow(), None, "startup never re-adds it");
        // Off leaves someone else's value (the MSI added later) alone, too.
        *key.0.borrow_mut() = Some(ME.into());
        let writes = *key.1.borrow();
        reconcile(false, ME, &key).unwrap();
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
