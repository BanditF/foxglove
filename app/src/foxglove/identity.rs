//! Package identity for Dynamic Lighting's background control (#179, prototype).
//!
//! Windows lists an app under Settings > Dynamic Lighting > Background light control only if it
//! has package identity and declares the `com.microsoft.windows.lighting` app extension. The
//! MSI stays; next to `foxglove.exe` it installs `foxglove-identity.msix`, a signed sparse
//! package with no payload (`app/packaging/identity`). Foxglove registers it for the user on
//! its first unelevated launch, with the install folder as the external location; the identity
//! applies from the next launch. The uninstaller runs `foxglove.exe --unregister-identity`.
//!
//! All of it is off unless the exe was built with `FOXGLOVE_SPARSE_IDENTITY=1`, which also
//! embeds the `<msix>` manifest; without a Windows-trusted signature whose subject matches
//! [`PUBLISHER`], registration fails and Foxglove carries on without identity.

/// Must match `AppxManifest.xml` and `foxglove.exe.manifest`.
pub const NAME: &str = "BanditF.Foxglove";
pub const PUBLISHER: &str = "CN=Foxglove Placeholder";
/// The package file the installer puts next to the exe.
pub const PACKAGE_FILE: &str = "foxglove-identity.msix";

/// Whether this build carries the identity manifest.
pub const ENABLED: bool = cfg!(foxglove_sparse_identity);

/// `foxglove.exe --unregister-identity` (the uninstaller, elevated): removes the package for
/// every user and exits. True when it handled the command line.
#[cfg(windows)]
pub fn entry() -> bool {
    if std::env::args_os().nth(1).is_none_or(|a| a != "--unregister-identity") {
        return false;
    }
    if let Err(e) = unregister_all() {
        eprintln!("foxglove: unregistering the identity package: {e}");
    }
    true
}

#[cfg(not(windows))]
pub fn entry() -> bool {
    false
}

/// Registers the identity package in the background when this build wants it and the process
/// has no identity yet.
pub fn start() {
    #[cfg(windows)]
    if ENABLED && !super::repaint_trace::disabled("identity") {
        super::services::spawn_named("identity", || {
            if has_identity() || elevated() {
                return; // registered already, or elevated: per-user registration waits
            }
            match register() {
                Ok(true) => eprintln!("foxglove: identity package registered; it applies from the next start"),
                Ok(false) => {}
                Err(e) => eprintln!("foxglove: identity package not registered: {e}"),
            }
        });
    }
}

/// Whether this process runs with package identity.
#[cfg(windows)]
pub fn has_identity() -> bool {
    windows::ApplicationModel::Package::Current().is_ok()
}

#[cfg(not(windows))]
pub fn has_identity() -> bool {
    false
}

#[cfg(windows)]
fn elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut e).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

/// Registers `foxglove-identity.msix` from the exe's folder with that folder as the external
/// location. `Ok(false)` when the package file isn't there (a build without it).
#[cfg(windows)]
fn register() -> windows::core::Result<bool> {
    use windows::core::HSTRING;
    use windows::Foundation::Uri;
    use windows::Management::Deployment::{AddPackageOptions, PackageManager};
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf)) else {
        return Ok(false);
    };
    let package = dir.join(PACKAGE_FILE);
    if !package.is_file() {
        return Ok(false);
    }
    let options = AddPackageOptions::new()?;
    options.SetExternalLocationUri(&Uri::CreateUri(&HSTRING::from(dir.as_os_str()))?)?;
    let result = PackageManager::new()?
        .AddPackageByUriAsync(&Uri::CreateUri(&HSTRING::from(package.as_os_str()))?, &options)?
        .join()?;
    let code = result.ExtendedErrorCode()?;
    if code.is_err() {
        return Err(windows::core::Error::new(code, result.ErrorText()?.to_string()));
    }
    Ok(true)
}

/// Removes the identity package for every user (needs elevation; the uninstaller has it).
#[cfg(windows)]
fn unregister_all() -> windows::core::Result<()> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::{PackageManager, RemovalOptions};
    let manager = PackageManager::new()?;
    // An empty user SID means every user.
    for package in manager.FindPackagesByUserSecurityIdNamePublisher(&HSTRING::new(), &HSTRING::from(NAME), &HSTRING::from(PUBLISHER))? {
        let full_name = package.Id()?.FullName()?;
        manager.RemovePackageWithOptionsAsync(&full_name, RemovalOptions::RemoveForAllUsers)?.join()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three places that name the package agree.
    #[test]
    fn manifests_agree() {
        let appx = include_str!("../../packaging/identity/AppxManifest.xml");
        let exe = include_str!("../../packaging/identity/foxglove.exe.manifest");
        assert!(appx.contains(&format!("Name=\"{NAME}\" Publisher=\"{PUBLISHER}\"")));
        assert!(exe.contains(&format!("publisher=\"{PUBLISHER}\"")));
        assert!(exe.contains(&format!("packageName=\"{NAME}\"")));
        assert!(appx.contains("Application Id=\"Foxglove\"") && exe.contains("applicationId=\"Foxglove\""));
        assert!(appx.contains("com.microsoft.windows.lighting"));
    }
}
