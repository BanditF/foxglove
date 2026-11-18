# Hideout demo: hub with Foxglove and Vulpes as modules

Goal: the Hideout hub starts and shows Foxglove (keyboard tiles and page) and Vulpes (Arete's
"AI" tile and page) as modules, both live. Everything runs in an isolated demo folder; the real
Hideout and Foxglove data are never read or written.

**Status: steps 1 to 3 are verified by the window-free tests (`scripts/ops/test-demo-up.ps1`, on a
fake `hideout.exe`). Steps 4 to 6 have never been run on a real PC and are marked UNVERIFIED.**
Nothing here was launched when this page was written.

## What you need

| Thing | Where | Note |
|---|---|---|
| `hideout.exe` | `cargo build -p hideout` in this checkout (own `CARGO_TARGET_DIR`) | Windows only |
| `foxglove.exe` of this build | `cargo build -p foxglove` (the `app/` crate) | Its version (`app/Cargo.toml`, 0.3.0) goes into the manifest; the hub stops a module whose `initialize` version differs (contract 7.5) |
| Arete's module folder | a folder with Arete's `module.toml` (its repo: `src/arete/hideout/module.toml`) | `run[0]` is a placeholder in the vendored copy, so pass `-AreteExe` |
| `arete.exe` | this PC's Arete install | Its version MUST equal the manifest `version` (0.1.0 in the vendored copy) |

## Steps

Run from the checkout in PowerShell. `$exe` values are your paths.

1. **Dry run (default, safe).** Writes nothing, starts nothing:
   ```powershell
   powershell -File scripts\ops\demo-up.ps1 -AreteModuleDir <arete-module-folder> -AreteExe <arete.exe> `
     -HideoutExe <hideout.exe> -FoxgloveExe <foxglove.exe>
   ```
   Shows: the demo folder (default `%TEMP%\hideout-demo`), both `run[0]` paths and versions, the
   isolation values, and `plan:` lines (register or already registered; hub start "no"). Any
   `PROBLEM:` line (placeholder path, missing exe) exits 1.
2. **Register only.** Same command plus `-Adopt`. Runs `hideout.exe --adopt` for each module that
   is not yet in `<demo>\hub\inventory.json`, with `HIDEOUT_CONFIG_DIR=<demo>\hub` and
   `FOXGLOVE_CONFIG_DIR=<demo>\foxglove-data`. Opens no window and starts nothing.
3. **Check the registration.** The script prints `inventory has vulpes: True` and `foxglove: True`
   (the parse of `inventory.json`). A refused manifest prints the hub's sentence (for example "not
   registered: ...") and exits 1.
4. **Start the demo hub (UNVERIFIED).** Same command plus `-Launch` (implies `-Adopt`). Starts
   `hideout.exe` with the demo environment and records its pid in `<demo>\hub.pid`. This is the
   only switch that starts anything. Expect the hub window with the Modules page.
5. **See the modules (UNVERIFIED).** Expected: a Foxglove page and three tiles (Keyboard, Keyboard
   lights, Keyboard art), and an "AI" page and tile (Vulpes). Vulpes starts when its page is
   opened or a method it provides is called (`activation on = open, call`); a cold tile reads "Not
   running". Foxglove has no `[activation]`, so the default `enable` applies and the hub may start
   it at hub start (contract 4.6). Both modules ask the user for `provide:` grants (permission
   dialog, #1178): the tiles stay empty until those are allowed. The log is
   `<demo>\hub\logs\hideout.log`.
6. **What each shows (UNVERIFIED live; the content is pinned by tests).**
   - Foxglove (`app/src/foxglove/hub_panels.rs`): read-only tiles drawn from the `keyboard.*`
     facts; one button, "Open Foxglove settings".
   - Vulpes (`crates/hideout-panels/tests/vulpes.rs` and `vectors/vulpes.json`): GPU holder and
     the gaming reservation, with the actions "Free GPU" and "Release GPU reservation".

## One fixed app folder (rebuilds keep the same exe path)

Foxglove is never adopted from the build's own path (a cargo target rotates and is deleted).
`-Adopt`/`-Launch` copy the build to `<demo>\apps\foxglove\foxglove.exe` (skipped when the bytes
are identical) and adopt THAT path. Every rebuild (yours or On-call's) overwrites the same file,
so the path in the hub's record never changes and the Keep-my-OK switch, once the user turns it
on, stays valid (0 clicks per rebuild). A dry run copies nothing. Rollback still starts the
`-FoxgloveExe` you pass, as before. Grants and the trust default are unchanged.

If a still-running Foxglove holds the fixed exe, the copy fails with a PROBLEM line: stop it, then
run again.

**Already registered at another path (the old trap).** The script reads the registered `run[0]`
(from `<demo>\hub\modules\foxglove\<version>\module.toml`). If it is not the fixed path (or cannot be
read) it prints `WARNING: foxglove is registered at ...` and
- hub not running: re-adopts at the fixed path (`hideout.exe --adopt` again; grants stay, the
  binding to the exe is re-asked once, as any moved exe);
- hub running: adopts nothing and exits 1 with the manual step: run `demo-up.ps1 -DemoHome <demo> -Down`,
  then run again with `-Adopt`.
It never leaves the hub quietly on an older build. Vulpes (Arete's own exe) is unchanged: skipped
when registered.

## Rollback to standalone Foxglove (one step)

```powershell
powershell -File scripts\ops\demo-up.ps1 -DemoHome <the same folder> -Rollback -Launch -FoxgloveExe <foxglove.exe>
```
Stops the demo hub (by `hub.pid`, only if it is named `hideout`), then starts `foxglove.exe` with
no arguments and no demo environment: Foxglove on its own, as today, on its normal data. No data
is touched and the demo folder stays. Without `-Launch` it only stops the hub and prints what it
would start. Foxglove in module mode ends by itself after the hub goes (30 s grace); wait for it
before the standalone start so two copies do not hold the keyboard (UNVERIFIED on a real PC).
If the user's Foxglove autostarts, nothing else is needed.

## Undo

```powershell
powershell -File scripts\ops\demo-up.ps1 -DemoHome <the same folder> -Down -Clean
```
`-Down` stops only the process whose pid is in `hub.pid` and only if it is named `hideout`;
`-Clean` deletes the demo folder. Modules the hub started end with it (contract 7.9: Foxglove
hands its devices back and exits after the 30-second grace period; stop it by pid if it lingers,
never by name). Nothing outside `-DemoHome` was written.

## Rules and warnings

- **Game mode and desktop:** `-Launch` opens a window and Foxglove in module mode takes the
  keyboard's devices. Do not run it during a game or a sitting without a go.
- **Keyboard:** Foxglove in module mode drives the real keyboard over HID like the normal app.
  The data folder is isolated, the device is not. Stop the user's own Foxglove first or expect
  both to contend for it (UNVERIFIED which wins).
- The script refuses `%LOCALAPPDATA%\Hideout`, `%APPDATA%\Hideout` and the Foxglove folders (and
  anything under them) as `-DemoHome`.

## Not yet verified

- A real `hideout.exe` `--adopt` of both manifests from this script (the Rust test
  `adopt::tests` covers the verb on a synthetic manifest; the script's tests use a fake exe).
- That the demo hub shows both modules, that the grants flow works, and that both go live (the
  Vulpes `initialize` version equal to the manifest's, Foxglove's equal to `app/Cargo.toml`).
- That the module child inherits `FOXGLOVE_CONFIG_DIR` from the hub (the hub's launch values are
  fixed by contract 7.5; whether the rest of the environment passes through is not checked).
- Whether the hub starts Foxglove at hub start (default activation `enable`). There is no hub
  mode that suppresses this; the only guarantee is that the script starts nothing without
  `-Launch`.
