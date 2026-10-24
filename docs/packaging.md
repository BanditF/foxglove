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

## Auto-update

The app checks once shortly after startup (unless disabled) and on **Check now**
in Settings. It selects the highest stable `app-v` SemVer release, ignoring
firmware tags, drafts, prereleases and versions at or below the installed version.
There is no periodic update check. Windows offers installation; other platforms
link to the release. A placeholder public key disables network checks.

Generate an Ed25519 key pair in a trusted environment:

```powershell
cargo run -p foxglove-update --bin foxglove-update-sign -- keygen
```

Store the printed private seed as the repository Actions secret
`FOXGLOVE_UPDATE_KEY`. Put only the printed base64 public key in
`app/resources/update-key.pub` before building the app. Never commit the private
key. Existing installations trust their embedded key; changing it requires a
release signed by the previous key and a coordinated key transition.

The release job requires the secret and produces `foxglove.exe`,
`foxglove-x64.msi`, `latest.json`, and `latest.json.sig`. The latter is a base64
Ed25519 signature over the exact JSON bytes (including whitespace). The manifest
contains:

```json
{"version":"1.2.3","tag":"app-v1.2.3","msi":{"name":"foxglove-x64.msi","url":"https://github.com/BanditF/foxglove/releases/download/app-v1.2.3/foxglove-x64.msi","sha256":"64 hexadecimal characters","size":12345},"notes":"Release notes","published":"2026-10-23T00:00:00Z"}
```

Unknown JSON fields are ignored. To build and sign manually, run the signing
binary with `manifest --version 1.2.3 --msi <path> --url <https-url>
--notes-file <path>`, then `sign latest.json` with the secret in the environment.
The manifest command calculates SHA-256, size and the current RFC3339 timestamp.
The app verifies the signature before trusting metadata, then verifies the MSI
size and hash in `%LOCALAPPDATA%\Foxglove\updates`. A copied, non-elevated app
helper waits for the app to exit and requests UAC for `msiexec /i ... /passive
/norestart`. It waits for installation and restarts the installed app without
elevation. Cancellation or installation failure leaves `error.txt` for the next
startup and restarts the old install. Windows Installer handles upgrades;
settings under `%APPDATA%\Foxglove\config` are preserved. The startup preference
is `[updates] check_at_startup = true` in `updates.ini` beside the other settings.

Before shipping, exercise UAC acceptance/cancellation, installer failure,
interrupted download, successful upgrade/relaunch, and settings preservation on
a Windows test installation. The development sandbox cannot validate a real MSI
upgrade.
