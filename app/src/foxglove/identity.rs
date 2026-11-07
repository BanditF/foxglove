//! Package identity for Dynamic Lighting's background control (#179).
//!
//! Windows lists an app under Settings > Dynamic Lighting > Background light control only if it
//! has package identity and declares the `com.microsoft.windows.lighting` app extension. The
//! MSI stays; next to `foxglove.exe` it installs `foxglove-identity.msix`, a signed sparse
//! package with no payload (`app/packaging/identity`), and the package's logos in `Assets\`
//! (the sparse manifest's logo paths resolve in the install folder, the external location).
//!
//! On its first unelevated start without identity, Foxglove registers the package for the user
//! with its own folder as the external location. Identity applies only to processes started
//! after that, so Foxglove then starts itself again, once ([`after_register`]): the copy is told
//! by [`RELAUNCHED`] and never relaunches, so a registration that doesn't give identity (another
//! folder, a build without the `<msix>` element) can't loop. The uninstaller runs
//! `foxglove.exe --unregister-identity` as the user.
//!
//! All of it is off unless the exe was built with `FOXGLOVE_SPARSE_IDENTITY=1`, which also
//! embeds the `<msix>` manifest; without a Windows-trusted signature whose subject matches
//! [`PUBLISHER`], registration fails and Foxglove carries on without identity. A run with
//! `FOXGLOVE_CONFIG_DIR` (tests, the Test lab) never registers.
//!
//! **Version rule.** Windows refuses to register the same package version again with other
//! contents (0x80073CF9). Any change to `AppxManifest.xml` or the logos bumps [`VERSION`] (and
//! the manifest's `Version`; a test checks they agree).
#![cfg_attr(not(windows), allow(dead_code))]

/// Must match `AppxManifest.xml` and `msix.xml`.
pub const NAME: &str = "BanditF.Foxglove";
pub const PUBLISHER: &str = "CN=Foxglove Placeholder";
/// The package version in `AppxManifest.xml`: bumped with every change to the package.
pub const VERSION: &str = "1.0.1.0";
/// The package file the installer puts next to the exe.
pub const PACKAGE_FILE: &str = "foxglove-identity.msix";
/// The logos the manifest names, shipped in `Assets\` next to the exe.
pub const ASSETS: [&str; 3] = [
    r"Assets\logo50.png",
    r"Assets\logo150.png",
    r"Assets\logo44.png",
];
/// Set in the copy Foxglove starts after registering: it never relaunches again.
pub const RELAUNCHED: &str = "FOXGLOVE_IDENTITY_RELAUNCHED";

/// Whether this build carries the identity manifest.
pub const ENABLED: bool = cfg!(foxglove_sparse_identity);

/// `foxglove.exe --unregister-identity` (the per-user uninstaller, as the user): removes the
/// package for that user and exits. True when it handled the command line.
#[cfg(windows)]
pub fn entry() -> bool {
    if std::env::args_os()
        .nth(1)
        .is_none_or(|a| a != "--unregister-identity")
    {
        return false;
    }
    if let Err(e) = unregister_for_user() {
        eprintln!("Foxglove: unregistering the identity package: {e}");
    }
    true
}

#[cfg(not(windows))]
pub fn entry() -> bool {
    false
}

/// What to do once registration has run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Registered just now: start a fresh copy, which gets the identity.
    Relaunch,
    /// Registered, but this process is already the relaunched copy (and still has none): say so
    /// and carry on without identity.
    CarryOn,
    /// Nothing registered (no package next to the exe, or it failed).
    Nothing,
}

/// The step after [`register`]: `registered` its result, `relaunched` whether this process is
/// the copy started after an earlier registration ([`RELAUNCHED`]).
pub fn after_register(registered: bool, relaunched: bool) -> Next {
    match (registered, relaunched) {
        (true, false) => Next::Relaunch,
        (true, true) => Next::CarryOn,
        (false, _) => Next::Nothing,
    }
}

/// Whether this start may register at all: a build with the identity manifest, no identity
/// yet, not elevated (registration is per user), and not a sandboxed run.
pub fn may_register(enabled: bool, has_identity: bool, elevated: bool, sandboxed: bool) -> bool {
    enabled && !has_identity && !elevated && !sandboxed
}

/// The logos missing from `dir` (the install folder): Settings shows a blank icon without them.
pub fn missing_assets(dir: &std::path::Path) -> Vec<&'static str> {
    ASSETS
        .iter()
        .copied()
        .filter(|a| !dir.join(a).is_file())
        .collect()
}

/// Registers the identity package in the background when this build wants it and the process
/// has no identity yet, then starts a fresh copy once so the identity applies.
pub fn start() {
    #[cfg(windows)]
    if ENABLED && !super::repaint_trace::disabled("identity") {
        super::services::spawn_named("identity", || {
            let sandboxed = std::env::var_os("FOXGLOVE_CONFIG_DIR").is_some()
                || super::store::test_sandbox().is_some();
            if !may_register(ENABLED, has_identity(), elevated(), sandboxed) {
                return;
            }
            let registered = match register() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Foxglove: identity package not registered: {e}");
                    false
                }
            };
            match after_register(registered, std::env::var_os(RELAUNCHED).is_some()) {
                Next::Relaunch => relaunch(),
                Next::CarryOn => eprintln!(
                    "Foxglove: identity package registered, but this copy still has no identity \
                     (registered for another folder?); carrying on without it"
                ),
                Next::Nothing => {}
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
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
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
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
    else {
        return Ok(false);
    };
    let package = dir.join(PACKAGE_FILE);
    if !package.is_file() {
        return Ok(false);
    }
    let missing = missing_assets(&dir);
    if !missing.is_empty() {
        eprintln!(
            "Foxglove: identity package: {} missing next to the exe; Settings may show a blank icon",
            missing.join(", ")
        );
    }
    let options = AddPackageOptions::new()?;
    options.SetExternalLocationUri(&Uri::CreateUri(&HSTRING::from(dir.as_os_str()))?)?;
    let result = PackageManager::new()?
        .AddPackageByUriAsync(
            &Uri::CreateUri(&HSTRING::from(package.as_os_str()))?,
            &options,
        )?
        .join()?;
    let code = result.ExtendedErrorCode()?;
    if code.is_err() {
        return Err(windows::core::Error::new(
            code,
            result.ErrorText()?.to_string(),
        ));
    }
    Ok(true)
}

/// Starts a fresh copy, which runs with the identity, and ends this one: the lights, panel and
/// art go back as on Quit (bounded), the single-instance guard is let go first so the copy takes
/// it, and the copy carries [`RELAUNCHED`].
#[cfg(windows)]
fn relaunch() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("Foxglove: identity package registered; can't start again: {e}");
            return;
        }
    };
    super::quit::hand_back("the identity package was registered");
    super::single_instance::release();
    let null = std::process::Stdio::null;
    match std::process::Command::new(exe)
        .env(RELAUNCHED, "1")
        .stdin(null())
        .stdout(null())
        .stderr(null())
        .spawn()
    {
        Ok(_) => {
            super::app_log::early(&format!(
                "Foxglove: identity package {VERSION} registered; started a fresh copy with it"
            ));
            std::process::exit(0);
        }
        // The guard is gone, but this copy goes on (its window and tray are still up): the
        // identity applies from the next start instead.
        Err(e) => eprintln!(
            "Foxglove: identity package registered; can't start again ({e}); it applies from \
             the next start"
        ),
    }
}

/// Removes the identity package for this user: the per-user MSI's uninstall runs as the user
/// (#444), and the package was registered per user.
#[cfg(windows)]
fn unregister_for_user() -> windows::core::Result<()> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::{PackageManager, RemovalOptions};
    let manager = PackageManager::new()?;
    // An empty user SID means the current user.
    for package in manager.FindPackagesByUserSecurityIdNamePublisher(
        &HSTRING::new(),
        &HSTRING::from(NAME),
        &HSTRING::from(PUBLISHER),
    )? {
        let full_name = package.Id()?.FullName()?;
        manager
            .RemovePackageWithOptionsAsync(&full_name, RemovalOptions::None)?
            .join()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APPX: &str = include_str!("../../packaging/identity/AppxManifest.xml");
    const EXE: &str = include_str!("../../packaging/identity/msix.xml");

    /// The three places that name the package agree.
    #[test]
    fn manifests_agree() {
        assert!(APPX.contains(&format!(
            "Name=\"{NAME}\" Publisher=\"{PUBLISHER}\" Version=\"{VERSION}\""
        )));
        assert!(EXE.contains(&format!("publisher=\"{PUBLISHER}\"")));
        assert!(EXE.contains(&format!("packageName=\"{NAME}\"")));
        assert!(
            APPX.contains("Application Id=\"Foxglove\"")
                && EXE.contains("applicationId=\"Foxglove\"")
        );
        assert!(APPX.contains("com.microsoft.windows.lighting"));
    }

    /// Microsoft's sparse-package template: architecture neutral, the exe's own trust level.
    #[test]
    fn the_package_is_neutral_and_declares_what_identity_needs() {
        assert!(APPX.contains("ProcessorArchitecture=\"neutral\""));
        assert!(APPX.contains("<uap10:AllowExternalContent>true</uap10:AllowExternalContent>"));
        assert!(APPX.contains("rescap:Capability Name=\"runFullTrust\""));
        // With identity Windows asks per app before it lets notifications be read (#507).
        assert!(APPX.contains("uap3:Capability Name=\"userNotificationListener\""));
    }

    /// Every logo the manifest names is in the package folder and in [`ASSETS`], so the MSI
    /// and the dev script ship each next to the exe.
    #[test]
    fn every_logo_is_shipped() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/identity");
        for (i, _) in APPX.match_indices("Assets\\") {
            let name: String = APPX[i..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '\\' | '.'))
                .collect();
            if !name.ends_with(".png") {
                continue; // the comment's "Assets\"
            }
            assert!(ASSETS.contains(&name.as_str()), "{name} not in ASSETS");
        }
        assert_eq!(missing_assets(&dir), Vec::<&str>::new());
        let empty = std::env::temp_dir().join("foxglove-identity-no-assets");
        assert_eq!(missing_assets(&empty), ASSETS.to_vec());
    }

    #[test]
    fn registering_relaunches_once() {
        assert_eq!(after_register(true, false), Next::Relaunch);
        assert_eq!(
            after_register(true, true),
            Next::CarryOn,
            "the relaunched copy never starts another"
        );
        assert_eq!(after_register(false, false), Next::Nothing);
        assert_eq!(after_register(false, true), Next::Nothing);
    }

    #[test]
    fn only_a_plain_unelevated_identity_build_registers() {
        assert!(may_register(true, false, false, false));
        assert!(!may_register(false, false, false, false), "flag off");
        assert!(!may_register(true, true, false, false), "has identity");
        assert!(!may_register(true, false, true, false), "elevated");
        assert!(!may_register(true, false, false, true), "a test or lab run");
    }
}
