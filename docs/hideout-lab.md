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

## A withdrawn version in the lab (#1026)

Lab modules have no catalog, so a catalog can't revoke one. Instead a lab hub reads
`<config>\lab-withdrawn.json` (lab mode only, never in the user's hub):

```json
{ "org.foxglove.focus-timer": { "version": "0.1.0", "reason": "Please update to 0.1.1." } }
```

A lab module whose loaded version it names reads as withdrawn by its publisher (contract §7.8
`revoked`, v1.9): the hub ends its leases with `"revoked"`, publishes `module.<id>.revoked`,
stops it and shows the withdrawn card, the red marks, the tile and the tray line, with one
notification per version (remembered in `<config>\run\withdrawn-notified.json`; delete that
file to see the notification again). `hideout.log` says `lab: <id>: <version> reads as withdrawn
(lab-withdrawn.json)`. **Check for updates** in a lab hub reads the file again: still named, it
says "No new version yet"; taken out of the file (or the version changed), the module starts
again. A normal hub has no catalog fetch yet, so its Check for updates says "Couldn't check".

## Pipe names: one set per hub folder (#1010)

A lab hub and the user's hub can run side by side: every pipe and kernel object a hub names
carries its folder (contract §8.2). The default folder keeps the old names; any other folder
adds `<suffix>`, a `.` and 16 hex digits hashed from the folder's canonical path, the same for
every spelling of that folder.

| What | Default folder | Lab folder |
|---|---|---|
| Router pipe | `\\.\pipe\hideout-<user>` | `\\.\pipe\hideout-<user><suffix>` |
| Alias | `\\.\pipe\foxglove-<user>` | `\\.\pipe\foxglove-<user><suffix>` |
| Capture (#1008) | not served | `\\.\pipe\hideout-capture-<user><suffix>` |
| Lab control (#1014) | not served | `\\.\pipe\hideout-lab-<user><suffix>` |
| Module jobs | `Local\hideout-job-<user>-<id>-<n>` | `Local\hideout-job-<user><suffix>-<id>-<n>` |

- Modules the lab hub starts get its own pipe in `HIDEOUT_PIPE`, so they connect to the lab hub,
  never the user's. The SDKs' `connect_env` (Rust and Python) opens exactly that name.
- `hideout.log` says the name: `router: serving \\.\pipe\hideout-<user><suffix>`.
- The `hideout.exe --capture` client works the name out from `HIDEOUT_CONFIG_DIR` itself; run
  it with the same `HIDEOUT_CONFIG_DIR` as the lab hub.
- An outside tool for the lab hub (for example `foxglove-cli` with `FOXGLOVE_PIPE`) takes the
  name from the log line.
- `HIDEOUT_CONFIG_DIR` set to `%LOCALAPPDATA%\Hideout` is the user's hub, not a lab hub: same
  names, and lab mode stays refused.

## Also fixed in #1001: bridged plugins were ended every 30 s

The contract lets a bridged plugin skip `module.ready` (§8.2), and the Foxglove plugins speak
the v1 wire and never send it. The router never told the supervisor they were ready, so the
30 s ready timer (§7.5) ended every bridged plugin as a crash (`ended with 1`, restarted with
backoff). Renaming the ids had nothing to do with it. The bridge now reports `module.ready` for
a bridged plugin once its `initialize` is answered. The bridge also used to throw away a
bridged plugin's stderr, so `logs\<id>.log` stayed empty. It now goes to that log, as for any
module (§7.7).

## Also fixed in #1013: three bugs from the first lab run

- **A page button on a bridged plugin timed out** ("action pause: timeout"). The hub sends
  `action.invoke`, a call; the plugin SDK knew only Foxglove's `action.invoked` notification, so
  the call was never answered and the action never reached the plugin. The SDK now queues the
  action and answers the call itself, at once.
- **A module's own channel was refused** ("Room map has no channel called map"). Only the grant
  `channel.offer:<name>` let a module offer a channel, and nothing turned the manifest's
  `provides = ["channel:<name>"]` into one, so `module.register` refused `map`. The manifest's
  declaration now offers it (§8.4); a module with a catalog entry is still held to the catalog.
- **The crash bar and stopped card said the module id.** The hub's panel feed knows only the id;
  the panel host now names a module's panels by its manifest's `name`, as the toast does.

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

## Lab control (#1003)

A lab hub can be driven from a script, so the tray, keyboard and flicker checks don't need hands.
`hideout.exe --lab-do <verb> [args]` sends one line to the lab hub's control pipe
(`\\.\pipe\hideout-lab-<user><folder suffix>`, current user only, no remote clients) and prints
the hub's answer, one JSON object with `"ok"` (exit 0 when ok). The client opens no window and
starts nothing. Code: `crates/hideout-shell/src/lab_control.rs`.

**Only a lab hub serves it.** Lab mode must be on (`HIDEOUT_LAB=1` or `--lab`, with a
`HIDEOUT_CONFIG_DIR` that is not the user's hub folder, as above). The user's hub (default
folder) never serves it, whatever the environment says: it starts no thread, no pipe and no
plugin. A lab hub has one thread blocked in `ConnectNamedPipe` and an egui plugin that counts
passes. The accessible tree is built only after the first request. `hideout.log` says
`lab control: on, \\.\pipe\hideout-lab-…`, then one `lab control: <verb>` line per request.

| Verb | What it does |
|---|---|
| `state` | JSON: `tabs` and `active_page`; per window (`main`, `flyout`, other viewports as `popouts`) the accessible nodes as last drawn (role, name or text, selected, disabled, focused), `focus`, `on_screen` / `open` / `visible`, `passes`, and the flyout's `geometry` (window, monitor, work area); `foreground` (main, flyout, menu, tray, other); `tray.events` and `tray.last` (tray events that reached the hub's handler) and `tray.menu` (the open tray menu's items: text, enabled, checked, highlighted); `loop.wakes` (event-loop wakes since start) and `loop.root_passes`. |
| `frames <ms> [<from_t>]` | The frame log from `from_t` (default: now) for `ms` (at most 10 s): one row per pass of each window, with whether the main window was on screen and the flyout window visible; passes per window; the first and last time the flyout was visible. |
| `tray-click` | Posts `WM_LBUTTONDOWN`, `WM_LBUTTONUP` as tray-icon's callback message to its window. |
| `tray-double-click [<gap_ms>]` | Down, up, a gap (default 150 ms), `WM_LBUTTONDBLCLK`, up: Windows' order for a double click. |
| `tray-menu` | `WM_RBUTTONDOWN`, `WM_RBUTTONUP`: tray-icon opens the tray menu. |
| `tray-key-select` | What the shell sends for Enter or Space on the icon (Win+B, arrows). With notify version 0, which tray-icon 0.24 uses, that is the right-button pair, so it opens the menu. |
| `menu-key <down\|up\|left\|right\|enter\|esc\|home\|end>` | Posts the key to the open tray menu's window (`#32768`), for the menu's own keyboard loop. |
| `menu-pick <text>` | Closes the open tray menu and posts the item's `WM_COMMAND` to the tray window, which muda turns into the menu event. |
| `key <chord> [main\|flyout]` | A chord (`ctrl+k`, `shift+tab`, `esc`, `enter`, `down`, `f6`) as key press and release in the window's next egui pass. Without a window named: the foreground one, else an open flyout, else the main window. |
| `text <text>` | A text event, for typing into the focused field. |
| `click <name>[#n] \| @<i> [main\|flyout]` | AccessKit Focus and Click on the node with that accessible name (or text), on the `n`-th node with it, or on node `i` of `state`. This is what a screen reader or UI Automation sends. |
| `focus <tab or page>` | The same on a tab (or sidebar entry) of the main window: makes it the active page. |
| `open-flyout` / `close-flyout` | A tray click if the flyout is closed / Esc into the flyout if it is open. |
| `close-window` | Posts `WM_CLOSE` to the main window, which closes it to the tray. |
| `set-setting <module> <name> <value>` | Changes a module setting in the main window's next frame through the Settings form's own path (`settings.set` to the module), for a field a script can't type into (a file picker). The value is JSON when it parses (`10`, `true`), else text (a path). |

An action answers after the pass that took it and the frame after that (at most 2 s + 0.5 s; an
action no pass took is dropped). A window that isn't on screen is refused at once.

**Same code path.** The tray verbs post the messages Windows sends a notification-area icon to
tray-icon's own window. tray-icon's window procedure makes its `TrayIconEvent` (or opens the
menu), and the hub's handler runs as for a real click. Keys and clicks enter at egui's input,
before the hub's frame reads it.

**What it does not prove** (each stays one hand check, listed below):
- that Windows delivers a real click, double click or key to the icon: the shell's own messages,
  its double-click timing and the keyboard focus in the notification area;
- that the keyboard reaches the hub window through Windows and winit (keys enter at egui);
- foreground rights: `SetForegroundWindow` after a posted message can be refused where a real
  click is allowed, so closing on focus loss isn't tested;
- how anything looks: the `--capture` PNGs are for a person or the designer to look at;
- toasts, the browser that Update Hideout opens, taskbar edges and a second monitor.

### `scripts/lab/hideout_lab_checks.py`

Runs the steps below against a lab hub and prints PASS, FAIL or BLOCKED per step, with
`--capture` PNGs in `<lab>\lab-shots`. It needs `FOXGLOVE_APP_OK=1` (it shows windows) and
refuses the user's hub folder.

```powershell
$env:FOXGLOVE_APP_OK = '1'
python scripts\lab\hideout_lab_checks.py --app "$bin\hideout.exe" --lab $lab --start
# Against a lab hub that already runs (HIDEOUT_LAB=1, HIDEOUT_LAB_CAPTURE=1): leave out --start.
# One group only: --only flyout, --only pages or --only supervisor.
```

**Automated now** (step numbers from the PRs' lab steps):
- #997 1: a tray click opens the flyout with the default tiles (Focus timer, Mic mute), against
  the taskbar and inside the work area, with a capture.
- #997 1: a double click opens the main window, and the frame log says whether the flyout was on
  screen first. A FAIL gives how long it was on screen: that is the flicker to file.
- #997 2: a second click closes the flyout and it stays closed; Esc closes it.
- #997 4: ··· > Hide tile, then Edit tiles shows the tile unticked; tick it, Done;
  `hideout.toml` has `[tray]`.
- #997 5: Open Hideout opens the main window and closes the flyout; a tile's "Open <Module>"
  opens its page; keyboard select on the icon opens the tray menu, and arrows and Enter pick
  Open Hideout.
- #991 keyboard: the flyout opens with the first tile focused; Right and Left move between
  tiles; Space presses the Focus timer's Pause (and again for Resume); Shift+F10 opens the
  tile's ··· menu with "Open <Module>" focused; Esc closes the menu, not the flyout, and the
  focus is back on the tile (capture `991-flyout-keyboard`); Enter opens the tile's page.
- #997 8: OBS Change, type, Cancel: back to "Saved" (BLOCKED unless a password is saved).
- #1019 (hard gate, runs first): main window hidden, a tray click shows and draws the flyout
  within 1 s, and after it closes the hidden hub wakes 2 times a second or fewer.
- #997 9: the hidden hub's loop wakes stay at 2 a second or fewer, before and after the flyout.
- #995 1: the sidebar lists every lab module, and `hideout.log` has the manifest line.
- #995 2: Focus timer page: Keys lists its two actions, and Pause is clicked.
- #995 3: Corsair iCUE: Settings shows the SDK file field, a path set through it (`set-setting`,
  the form's path) lands, and `settings.json` appears.
- #995 4: OBS How to opens How to connect (BLOCKED when the page has no How to).
- #995 5 and #1004: one kill shows the crash bar (two lines, Open log, no Restart now) and
  leaves the tray menu's status line alone, then the page comes back live. Five kills show the
  stopped card with Start again focused and Copy diagnostics, and the tray menu names the module.
  Esc changes nothing; Enter starts it again.

**Still needs a human** (one check each; these stay in #90):
1. A real click on the tray icon opens the flyout, and a real double click opens the window
   (Windows delivers them). Note any flash.
2. With the flyout open, click on the desktop: it closes (focus loss).
3. Win+B, arrow to the Hideout icon, Enter, then Shift+F10 or the Menu key: the tray menu opens.
4. The taskbar on the top, left and right, or a second monitor at another scale: the flyout
   follows.
5. The flyout has no taskbar button.
6. Update Hideout opens the releases page in the browser (needs a module that needs a newer hub).
7. The stopped toast shows, and its Start again and Open <Module> work.
8. #996's visual steps: the room map moves smoothly, pauses when minimised or on another tab,
   and follows "animation effects" off.
9. A look at the `lab-shots` PNGs (the designer's taste check) when a step's drawing changed.
10. With the flyout open, the Menu key (the key with a menu on it) opens the focused tile's ···
    menu, as Shift+F10 does (the lab's `key` can't send it: egui has no Menu key).
