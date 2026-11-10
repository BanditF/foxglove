# Hideout lab hub: loading the first-party plugins (#1001)

A lab hub can't install the first-party plugins (`org.foxglove.*`, family `foxglove`) the normal
way: they need a signed catalog entry, and developer-mode sideload refuses first-party ids
(`trust.sideload-scope`, contract §12.6). Renaming them to `lab.foxglove.*` doesn't help and
isn't needed. **Lab mode** loads them as they ship, only in a lab hub.

## What lab mode does

- It is on only with `HIDEOUT_LAB=1` (or `hideout.exe --lab`) **and** `HIDEOUT_CONFIG_DIR` set
  to the hub's folder, and that folder is not `%LOCALAPPDATA%\Hideout` or `%APPDATA%\Hideout` or
  anything inside them. Otherwise `hideout.log` says
  `lab: lab mode refused: <why>; no lab modules loaded` and the hub runs as a normal hub.
  `HIDEOUT_HOME` alone is not enough.
- When it's on, `hideout.log` says `lab: lab mode on; loading lab modules from <config>\lab-modules`,
  then one `lab:` line per folder: `<id>: loaded from …` or `…: not loaded: <why>`.
- Each `<config>\lab-modules\<id>\` whose manifest (`module.toml`, else `plugin.toml`) has that
  `id` joins the registry **in memory only**, as source `lab`:
  - its files are hashed when the hub reads the registry, and checked again before each launch:
    a file changed in between stops the launch (`trust.files`);
  - its permissions are granted as for a developer-mode sideload (`Source::Sideload`), never as
    a first-party catalog module, so never-list items are left out (§6.6);
  - it is enabled unless the lab's `hideout.toml` has `[modules.<id>] enabled = false`;
  - an id that is installed for real (in `inventory.json`) keeps the installed one.

## What it does not change

- **Trust.** No root, catalog, anchor or `seen` is read or written for a lab module. Lab
  entries are never written to `inventory.json`, and one written there by hand is dropped when
  the registry opens. A real install of the same id never inherits a lab entry's grants.
  Developer-mode sideload still refuses first-party ids, in lab mode too.
- **The launch check.** The supervisor starts the exe from the lab folder in its own Job Object
  and records its pid and exe hash. A module on the router pipe is still checked at
  `initialize` (token, pid, exe hash, §7.5). A bridged stdio plugin (all five first-party ones)
  is the child the supervisor spawned on its own stdin and stdout, so its identity is the bridge
  (§8.2).
- **The user's hub.** Lab mode is refused on the default folder, so the user's Hideout never
  loads `lab-modules`.

## Also fixed in #1001: bridged plugins were ended every 30 s

The contract lets a bridged plugin skip `module.ready` (§8.2), and the Foxglove plugins speak
the v1 wire and never send it. The router never told the supervisor they were ready, so the
30 s ready timer (§7.5) ended every bridged plugin as a crash (`ended with 1`, restarted with
backoff). Renaming the ids had nothing to do with it. The bridge now reports `module.ready` for
a bridged plugin once its `initialize` is answered. The bridge also used to throw away a
bridged plugin's stderr, so `logs\<id>.log` stayed empty. It now goes to that log, as for any
module (§7.7).

## On-call steps

Use a hub build from its own target dir and a lab folder outside `%LOCALAPPDATA%`. Never point
`HIDEOUT_CONFIG_DIR` at the user's hub. In PowerShell, from the checkout:

```powershell
$lab = 'C:\Users\fuchs\splitkeeb\worktrees\_lab\hub-1001'   # any folder outside %LOCALAPPDATA%
$env:CARGO_TARGET_DIR = 'C:\Users\fuchs\splitkeeb\worktrees\_targets\lab-1001'
cargo build -p hideout -p plugin-focus-timer -p plugin-mic-mute -p plugin-icue-mirror `
  -p plugin-openrgb-e131 -p plugin-broadcast-status
$bin = "$env:CARGO_TARGET_DIR\debug"
$plugins = @{
  'org.foxglove.focus-timer'      = @('plugin-focus-timer',      'focus-timer.exe')
  'org.foxglove.mic-mute'         = @('plugin-mic-mute',         'mic-mute.exe')
  'org.foxglove.icue'             = @('plugin-icue-mirror',      'icue-mirror.exe')
  'org.foxglove.openrgb'          = @('plugin-openrgb-e131',     'openrgb-e131.exe')
  'org.foxglove.broadcast-status' = @('plugin-broadcast-status', 'broadcast-status.exe')
}
foreach ($id in $plugins.Keys) {
  $crate, $exe = $plugins[$id]
  $dir = "$lab\lab-modules\$id"
  New-Item -ItemType Directory -Force $dir | Out-Null
  Copy-Item "crates\$crate\module.toml", "crates\$crate\plugin.toml" $dir
  Copy-Item "$bin\$exe" $dir
}
$env:HIDEOUT_CONFIG_DIR = $lab
& "$bin\hideout.exe" --lab
```

1. Remove the old renamed `lab.foxglove.*` copies from the lab's `modules\` and
   `inventory.json`, or start from a fresh lab folder.
2. Check `"$lab\logs\hideout.log"`: `lab: lab mode on`, five `lab: org.foxglove.… loaded` lines,
   then `supervisor: started org.foxglove.… (pid …)` for each.
3. Wait more than 30 s. No `supervisor: org.foxglove.… ended with 1` lines should appear, and
   each module's state is `running`.
4. Each module's stderr is in `"$lab\logs\<id>.log"`.
5. To rebuild one plugin: stop the lab hub, copy the new exe over the one in its lab folder,
   start the hub again. (Hot swap of a dev build without a hub restart is H2.7.)

Then run #995's steps 2 to 5 on the module pages.

Clean up: stop the lab hub (only that process, by its pid), then delete `$lab` and the target dir.
