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

The app keeps that Start menu entry too (#856): at startup it makes `Foxglove.lnk` in the user's
Start menu Programs folder when it's missing, or repairs it when the file it starts is gone, so
a copy run outside the MSI can be found in Start as well. One starting another exe that exists
is left alone; dev builds and test runs never write it. `--uninstall-cleanup` removes it when it
starts this exe.

**Start with Windows** belongs to the app alone. The switch is in **App -> Startup and
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

If the message still appears right after uninstalling 0.1, the old exe was still running and
Windows removes it at the next restart: restart, then run the MSI again.

0.1 never installs an update by itself: its build has the placeholder update key, so it only
says automatic updates are off and links to the releases page. So users meet this message only
when they run the new MSI themselves. Nothing is half-installed. After step 2 every update is
per user and silent.

## Current settings paths

Installation does not change these. With `FOXGLOVE_CONFIG_DIR` set, all of them go to that
folder instead (tests and lab runs).

| Data | Windows path |
| --- | --- |
| Foxglove's settings and profiles (`foxglove::store`) | `%APPDATA%\Foxglove\foxglove.toml` |
| Overlay settings (KeyPeek's, upstream format) | `%APPDATA%\Foxglove\config\settings.ini` |
| Saved Studio keymap cache | `%APPDATA%\Foxglove\config\keymap-VID-PID.studio` (four-digit uppercase hex IDs) |
| Art library | `%APPDATA%\Foxglove\config\art\` |
| Installed plugins | `%APPDATA%\Foxglove\plugins\` |
| Log | `%APPDATA%\Foxglove\logs\` |

The older per-feature files in `config\` (`connection.ini`, `screens.ini`, `layers.ini`,
`lighting.ini`, `updates.ini`, `art.json`, `art-cycle.json`) are imported into `foxglove.toml`
the first time each feature loads, then renamed `*.migrated` and kept. `screens` can also
import hostpanel's `%APPDATA%\hostpanel\config.toml`. KeyPeek's `settings.ini` stays where it
is ([profiles.md](profiles.md), Decision 1); loading it falls back to KeyPeek's
`%APPDATA%\srwi\KeyPeek\config\settings.ini`.

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

## Sparse identity for Dynamic Lighting (#179)

Windows lists an app under **Settings → Personalization → Dynamic Lighting → Background light control** only if it has package identity and declares the `com.microsoft.windows.lighting` app extension. Scout's #178 settled the approach: keep the MSI and add a signed sparse package with no payload.

None of this is on by default. Ordinary builds and MSIs don't change: `FOXGLOVE_SPARSE_IDENTITY` stays off until the signing identity exists.

**Pieces**
- **`app/packaging/identity/`:**
  - `AppxManifest.xml`: `AllowExternalContent`, mediumIL, `win32App`, the lighting app extension, `AppListEntry="none"`, architecture `neutral` (Microsoft's sparse template). Capabilities: `runFullTrust`, `unvirtualizedResources` and `userNotificationListener` (below).
  - Logos in `Assets\`. The manifest's logo paths resolve in the **external location**, the exe's folder, so the logos ship there too: the MSI's `IdentityLogos` component, or `pack-identity.ps1 -Stage <exe folder>` for a dev build. Without them Settings may show a blank icon (Foxglove logs which are missing when it registers).
  - `msix.xml`: the `<msix>` element tying the exe to the package.
- **Build:** `FOXGLOVE_SPARSE_IDENTITY=1 cargo build --release --manifest-path app/Cargo.toml` adds that element to the application manifest (`app/build.rs`) and turns on registration (`app/src/foxglove/identity.rs`).
- **Package:** `scripts\pack-identity.ps1` builds `target\identity\foxglove-identity.msix` (`-Out <folder>` elsewhere). `-Thumbprint <sha1>` signs it. `-Stage <exe folder>` copies the msix and `Assets\` next to the exe. The gate packs it unsigned when the Windows SDK is installed (`scripts/test_identity_package.py`; schema only, nothing signed or registered).
- **Installer:** `cargo wix -C -dSparseIdentity=<path to signed msix>` installs it and the logos next to the exe. It also adds an uninstall step, `foxglove.exe --unregister-identity`, which runs **as the user** (`Impersonate="yes"`, per-user MSI, #444) and removes the package for that user only; the package was registered per user. A major upgrade doesn't run that step, so the registration survives. In-place auto-updates keep the same folder and the same embedded manifest, so they keep the identity.
- **Registration:**
  - On an unelevated start without identity, Foxglove registers the msix for the user (`AddPackageByUriAsync` with the install folder as `ExternalLocationUri`). A run with `FOXGLOVE_CONFIG_DIR` (tests, the Test lab) never registers.
  - Identity applies only to processes started after that, so Foxglove then **starts itself again once**: the lights, panel and art go back as on Quit, the single-instance guard is let go, and the copy (marked `FOXGLOVE_IDENTITY_RELAUNCHED=1`) takes over. The copy never relaunches, so a registration that gives no identity (another folder, a build without the flag) can't loop; it logs and carries on.
  - Failures are logged and Foxglove carries on without identity. Foreground-only Dynamic Lighting control needs no identity.
- **Version rule:** Windows refuses to register the same version with other contents (0x80073CF9). Every change to `AppxManifest.xml` or the logos bumps its `Version` and `identity::VERSION` (a test checks they agree). A dev re-registration of the same version needs the old one removed first (`scripts\dev-identity.ps1 undo`).
- **Notifications with identity (#507):** a packaged process reads notifications only with the `userNotificationListener` capability, and Windows then decides per app (it starts at "not asked"; the desktop-apps switch no longer applies). So the manifest declares it, Foxglove appears under Settings → Privacy & security → Notifications, and the Notifications card offers **Allow Foxglove**, which asks Windows on that click only. The arrival event works with identity; any refusal still falls back to the folder watch. Both paths have tests (`notify_windows`).
- **Dev builds (app-main rebuilds):** a dev registration points the external location at the build's folder, e.g. `foxglove\app\target\release`. Once it is registered, **every rebuild of that exe must set `FOXGLOVE_SPARSE_IDENTITY=1`**, or the new exe has no `<msix>` element and silently runs without identity (Windows' background order then skips it). The identity health row says so.

**Dev identity (`scripts\dev-identity.ps1`, #179):** the interim route on the user's own PC, run **only by the user, on their go**. Without arguments it prints the plan and changes nothing. `apply` (asks first unless `-Yes`) does, recording each finished step in `%LOCALAPPDATA%\Foxglove\dev-identity.json`:
1. **cert:** `New-SelfSignedCertificate` in `Cert:\CurrentUser\My`, subject `CN=Foxglove Dev, O=BanditF` (`-Publisher`), code signing only, key **not exportable**, one year.
2. **trust:** its public part into `Cert:\CurrentUser\TrustedPeople` (this user only, no admin).
3. **sign:** `pack-identity.ps1 -Publisher -Thumbprint -NoTimestamp -Stage <exe folder>`.
4. **register:** `Add-AppxPackage -Path <msix> -ExternalLocation <exe folder>` (default `app	arget
elease` of the main checkout).
5. **verify:** `Get-AppxPackage BanditF.Foxglove` with that publisher.

`undo` reverses exactly the recorded steps, newest first (Remove-AppxPackage, the staged msix and `Assets\`, the TrustedPeople copy, the certificate and its key), also after a failure part way; without a record it removes any package and dev certificate this script makes, by name and subject. The tests (`scripts/test_dev_identity.py`) run plan, apply and undo with fake operations only.

The exe must name the same publisher: build it with `FOXGLOVE_SPARSE_IDENTITY=1` and `FOXGLOVE_IDENTITY_PUBLISHER='CN=Foxglove Dev, O=BanditF'` (`build.rs` writes it into the `<msix>` element; `identity::PUBLISHER` reads it). `apply` checks the exe for both before changing anything. **Every app-main rebuild of that exe must set both variables again**, or it silently loses identity.

**Signing (#90, #383):** Windows registers the package only with a trusted signature whose subject equals `Publisher` (now the placeholder `CN=Foxglove Placeholder`, in both manifests and `identity.rs`; a test checks they agree). Public builds wait for SignPath Foundation (#383); a dev cert made on the user's PC is the interim route (`scripts\dev-identity.ps1`, run only on the user's go). A publisher change is a new provider to Windows, so the user re-orders Background light control once.

### Dynamic Lighting controller with identity (#179)

What Foxglove does once it runs with identity (none of it runs unpackaged):
- **The keyboard's own LampArrays are held** (`app/src/foxglove/lamp_hold.rs`): over USB and over Bluetooth, which Windows lists as two devices ("Eyelash Corne 1" and "2"), for as long as Foxglove runs, pause and away included, and never written. A device no listed controller holds goes to Windows' own controller, which takes the keyboard's lights and the firmware then refuses the host link. Quit lets them go, so Windows hands each to the next controller. The holds follow events only: a keyboard connect and the Lights worker's scan reopen them, and `AvailabilityChanged` updates who has each.
- **H1, to be measured:** a hold without writes, by the controller ranked first, keeps Windows out and leaves the keyboard on its host link. The lab case `dl-hold-keeps-windows-out` (`tests/hil/dl-hold.toml`, manual, On-call) reads the firmware's LampArray take-over counter and owner byte (LAMP_PARTS bytes 12-23) for two minutes while Foxglove holds them from the background.
- **Fallback if H1 fails** (the case FAILs): Foxglove draws the keyboard through Windows, the #749 b path (`lights_owner::through_windows`, the scene output's keyboard LampArray), and those writes go **only to the transport the host link uses now** (`lamp_hold::draw_on`); the other transport stays held, unwritten. This is already wired: when the firmware reports a LampArray owner, the existing path takes over.
- **Foxglove recognises its own hold:** an owner byte naming a LampArray transport Foxglove holds, with Windows' leave to drive it, is "Foxglove, through Windows Dynamic Lighting" (or the running scene), never the amber "Windows holds the keyboard's lights".
- **What the user sees:** while Windows Dynamic Lighting (not Foxglove's own hold) has the keyboard's lights and nothing waits for them, the Lights page's Keyboard card shows a quiet bar, "Windows Dynamic Lighting is controlling this keyboard", with **Open Dynamic Lighting settings** (`ms-settings:personalization-lighting`); with identity its sentence says to drag Foxglove to the top of Background light control and check both keyboard device pages. A waiting scene keeps the amber bar. The Keyboard health view has a "Dynamic Lighting identity" row (OK with identity; WARN when the package is next to the exe but this copy has none, or the exe was rebuilt without the flag), one row per keyboard LampArray transport Foxglove holds (WARN when another controller ranks first for it), and after a flash a WARN "Lights owner after the flash" when Windows took the lights that it didn't have before (with a notification). **The app never writes the Windows setting.** On-call: after each batched flash, read the health view (or `foxglove-lab health`) for that row.
- **Every other LampArray (the G502, any Dynamic Lighting device)** follows the running scene as before; between scenes the Lights worker gives a device Windows hands to Foxglove its colours, and as the background controller one it never set gets its resting look (saved colours, else white), so none is left dark or as another controller left it.
