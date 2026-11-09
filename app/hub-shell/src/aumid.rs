//! The process's AppUserModelID: the id Windows groups an app's windows under on the taskbar,
//! and the name its notifications carry. Hideout sets its own (`BanditF.Hideout`) at startup, so
//! it never groups with Foxglove (spec §2.5). Foxglove gets its id from its identity package.

/// Sets this process's AUMID. Call before any window exists. Errors are returned, not fatal:
/// without it, Windows groups by the exe path instead.
#[cfg(windows)]
pub fn set(id: &str) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    // SAFETY: passes a string that lives for the call; Windows copies it.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(id)) }
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn set(_id: &str) -> Result<(), String> {
    Ok(())
}
