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
override it with `cargo wix --no-build --install-version 0.1.1`. Use increasing MSI
versions (major/minor <= 255, patch <= 65535). Keep the UpgradeCode fixed.
The installer tools could not be installed or exercised in the offline development
environment; an actual MSI build and Windows install/uninstall smoke test remain required.

## Installed files and startup

The MSI installs **per user** (#444): `foxglove.exe` and `LICENSE` go to
`%LOCALAPPDATA%\Programs\Foxglove`, with a Start menu entry for the installing user. Everything
it writes is under `HKCU` (`Software\Foxglove\Installer`). It needs no UAC, so installs and
updates run without elevation. Another Windows user installs their own copy. It reuses
`app/resources/icon.ico`, which `app/build.rs` also embeds in the exe. No firmware or art is
bundled.

**Start with Windows** belongs to the app alone. The switch is in **Settings -> Theme and
updates -> Start with Windows** and is on by default.
- It writes and removes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Foxglove`, the
  quoted exe path. Windows' Settings › Apps › Startup lists it with its own toggle.
- At startup, with the switch on, it fixes a missing value or one naming another exe. Turned off,
  the value is removed and never added back.
- The MSI writes no Run value. On uninstall (not on an upgrade), it runs
  `foxglove.exe --uninstall-cleanup` as the user, which removes the value if it starts that exe.

Uninstall removes the installed files, the shortcut, the installer's registry values and the
app's Run value. It never removes user settings or cached keymaps. Major upgrades replace the
previous package and reject downgrades. Quit the running app before upgrading or uninstalling;
the in-app updater does this for you.

### Moving from 0.1 (per machine) to the per-user MSI

0.1.0 was a per-machine MSI in `%ProgramFiles%\Foxglove`. Windows Installer can't upgrade a
product from one install scope to the other: a per-user MSI doesn't see the per-machine one, so
it would install next to it (two apps, two Start menu entries, both competing for the keyboard).
So the per-user MSI checks for `%ProgramFiles%\Foxglove\foxglove.exe`. When it finds it, it stops
before changing anything, with this message:

> Foxglove 0.1 is installed for all users. Uninstall it first in Settings > Apps > Installed apps
> (your settings and keymaps are kept), then run this installer again.

The clean migration:
1. Quit Foxglove from the tray.
2. Uninstall **Foxglove** in Settings › Apps › Installed apps. This takes one UAC prompt, as it
   is the old per-machine package. Settings and keymaps in `%APPDATA%\Foxglove\config` stay.
   If 0.1's "Start with Windows" option was on, its Run value goes with it.
3. Run the new MSI. No UAC. Start Foxglove; with the switch on (the default, or as you left it),
   it writes the Run value for its new path.

0.1's in-app updater runs the new MSI elevated. That run stops at the same message, and 0.1
shows "Update failed" and keeps running. Nothing is half-installed. After step 2 every update is
per user and silent.

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
On a Windows test account, also verify:
- a default install with no UAC prompt, the Start menu entry, and startup at sign-in (the app's
  switch);
- a silent install (`msiexec /i foxglove-x64.msi /qn`, no elevation);
- upgrade and downgrade handling;
- the 0.1 per-machine guard message;
- uninstall with settings and cache files present: the Run entry and the shortcut go, and those
  files stay.

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
helper waits for the app to exit and runs `msiexec /i ... /passive /norestart`
without elevation (the MSI is per user, #444). It waits for installation and restarts the installed app without
elevation. Cancellation or installation failure leaves `error.txt` for the next
startup and restarts the old install. Windows Installer handles upgrades;
settings under `%APPDATA%\Foxglove\config` are preserved. The startup preference
is `[updates] check_at_startup = true` in `updates.ini` beside the other settings.

Before shipping, exercise UAC acceptance/cancellation, installer failure,
interrupted download, successful upgrade/relaunch, and settings preservation on
a Windows test installation. The development sandbox cannot validate a real MSI
upgrade.

## Sparse identity for Dynamic Lighting (#179, prototype)

Windows lists an app under **Settings → Personalization → Dynamic Lighting → Background light control** only if it has package identity and declares the `com.microsoft.windows.lighting` app extension. Scout's #178 settled the approach: keep the MSI and add a signed sparse package with no payload.

None of this is on by default. Ordinary builds and MSIs don't change.

**Pieces**
- **`app/packaging/identity/`:**
  - `AppxManifest.xml`: `AllowExternalContent`, mediumIL, `win32App`, the lighting app extension, `AppListEntry="none"`.
  - Logos.
  - `msix.xml`: the `<msix>` element tying the exe to the package.
- **Build:** `FOXGLOVE_SPARSE_IDENTITY=1 cargo build --release --manifest-path app/Cargo.toml` adds that element to the application manifest (`app/build.rs`) and turns on registration (`app/src/foxglove/identity.rs`).
- **Package:** `scripts\pack-identity.ps1` builds `target\identity\foxglove-identity.msix`. `-Thumbprint <sha1>` signs it.
- **Installer:** `cargo wix -C -dSparseIdentity=<path to signed msix>` installs it next to the exe. It also adds an uninstall step, `foxglove.exe --unregister-identity`, which runs elevated and removes the package for every user. A major upgrade doesn't run that step, so the registration survives. In-place auto-updates keep the same folder and the same embedded manifest, so they keep the identity.
- **Registration:**
  - On an unelevated start without identity, Foxglove registers the msix for the user (`AddPackageByUriAsync` with the install folder as `ExternalLocationUri`).
  - The identity applies from the next start.
  - Failures are logged and Foxglove carries on without identity. Foreground-only Dynamic Lighting control needs no identity.

**Blocked on signing (#90):** Windows registers the package only with a trusted signature whose subject equals `Publisher` (now the placeholder `CN=Foxglove Placeholder`, in both manifests and `identity.rs`; a test checks they agree). The plan is Azure Artifact Signing (ties to #158/#159). The proof the issue asks for (Foxglove listed under Background light control, and keeping control from the tray) also needs the LampArray controller (#88), so it waits for both. The WiX additions are behind the `SparseIdentity` define and haven't been built yet; no WiX locally, and the app CI is manual.
