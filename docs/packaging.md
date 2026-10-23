# Windows installer (first pass)

Foxglove uses cargo-wix and WiX Toolset **3.14** (WiX 4+ is not compatible).
On Windows, install the Rust MSVC toolchain, Visual Studio C++ build tools and
WiX 3.14, put WiX's `bin` directory on `PATH`, then:

```powershell
cargo install cargo-wix --version 0.3.8 --locked
cd app
cargo build --release --locked
cargo wix --no-build --output target/wix/foxglove-x64.msi
```

The definition is `app/wix/main.wxs`; cargo-wix discovers it automatically.
This package targets x64 Windows. Its version defaults to `app/Cargo.toml`;
override it with `cargo wix --no-build --version 0.1.1`. Use increasing MSI
versions (major/minor <= 255, patch <= 65535). Keep the UpgradeCode fixed.
The installer tools could not be installed or exercised in the offline development
environment; an actual MSI build and Windows install/uninstall smoke test remain required.

## Installed files and startup

The elevated, per-machine MSI installs `foxglove.exe` and `LICENSE` to
`%ProgramFiles%\Foxglove`, with an all-users Start menu entry. It reuses
`app/resources/icon.ico`, also embedded in the exe by `app/build.rs` (#31 will
replace the artwork). There are no bundled firmware or art assets.

In the feature selection dialog, **Start with Windows** is optional and off by
default. It writes a quoted executable path to the installing user's
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Foxglove` value.
For a silent install, add `ADDLOCAL=App,StartWithWindows` to `msiexec /i ... /qn`.
This option affects only the account running installation/maintenance; when
elevating with another account's credentials, it applies to that account.
It does not configure startup for every user. Maintain/uninstall with the same
account to remove its Run entry. Quit the running app before upgrading/uninstalling.

Uninstall removes the installed files, shortcut and installer-owned registry
values. It never removes user settings or cached keymaps. Major upgrades replace
the previous package and reject downgrades.

## Current settings paths

These paths are resolved by `Settings::config_file_path()` and its callers;
installation does not change them:

| Data | Windows path |
| --- | --- |
| Overlay/KeyPeek-derived settings | `%APPDATA%\Foxglove\config\settings.ini` |
| Last device / connection | `%APPDATA%\Foxglove\config\connection.ini` |
| Screens settings | `%APPDATA%\Foxglove\config\screens.ini` |
| Saved layer names | `%APPDATA%\Foxglove\config\layers.ini` |
| Saved Studio keymap cache | `%APPDATA%\Foxglove\config\keymap-VID-PID.studio` (four-digit uppercase hex IDs) |

All current Foxglove writes above share `%APPDATA%\Foxglove\config` (the Windows
`directories` crate appends `config`). Settings
loading also falls back to KeyPeek's `%APPDATA%\srwi\KeyPeek\config\settings.ini`, then
`settings.ini` in the process working directory. Saving writes to Foxglove's
folder. Screens settings can import `%APPDATA%\hostpanel\config.toml` when
`screens.ini` is absent. These legacy files remain in place; no migration is
added here. A follow-up should move these files up one level, with a one-time import and
legacy reads, so new writes go directly to
`%APPDATA%\Foxglove`.

## Releases and validation

`.github/workflows/app.yml` runs only by manual dispatch or an `app-v*` tag.
Tags must use `app-vMAJOR.MINOR.PATCH`; CI uses that version for the MSI. Both
Windows and Linux checks must pass before the release job creates/reuses the
GitHub release and uploads the MSI and exe. Manual runs retain both as workflow
artifacts without publishing a release. Firmware workflows are unchanged.

Before release, run `powershell -ExecutionPolicy Bypass -File scripts/check.ps1`.
On a Windows test account, also verify default install, optional startup and sign-in,
silent install, upgrade/downgrade handling, and uninstall with settings/cache files
present. Check that the Run entry and shortcut disappear while those files survive.
