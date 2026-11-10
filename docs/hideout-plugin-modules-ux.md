# Hideout: the existing plugins as modules (UX plan)

Issue #975 (H2.8), UX gate step. Mic mute, the focus timer, broadcast status (OBS), the iCUE
mirror and OpenRGB E1.31 become Hideout modules: each declares a tray **tile** and a **page**
through the contract (§5) and the hub draws them with no code that names any of them. This plan is
one tile pattern and one page pattern for every module of this kind, then each plugin's data:
its words, its states and its one primary action. Clean-room: written from
[hideout-contract.md](hideout-contract.md) §4–§5 and §7.8, [hideout-tabs-ux.md](hideout-tabs-ux.md),
[hideout-supervisor-ux.md](hideout-supervisor-ux.md), the plugins' `crates/plugin-*/plugin.toml`
and `src/`, and public docs; no code under `app/` was read.

Mockups (Python + Pillow, [`design/mockups/hideout_plugin_modules_mockups.py`](design/mockups/hideout_plugin_modules_mockups.py),
the tab and supervisor mockups' helpers and `hideout-shell` DARK tokens; one `tile()` function
draws every tile from data; the script fails under 4.5:1, lowest 4.9:1):
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h28\ux-h28-proposed-tiles.png`: the tray panel with
  all five tiles, the tile's anatomy (hub frame and module body), the six tones
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h28\ux-h28-proposed-tile-states.png`: every state
  per module: normal, first run, outside app missing, error
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h28\ux-h28-proposed-supervisor.png`: the tile and the
  page while starting, restarting (amber crash bar), stopped, needing a newer Hideout, quarantined
- `C:\Users\fuchs\splitkeeb\worktrees\_review\h28\ux-h28-proposed-pages.png`: the page pattern for
  Mic mute (working), OBS (can't reach) and Corsair iCUE (first run)

## Research

- **Reuse:** the §5.2 widget set (`value`, `label`, `meter`, `button`, `image`, `markdown`,
  `section`), `bind` to facts the plugins already publish (`focus.*`, `broadcast.*`), the
  generated settings form (`[[settings]]`, `[[secrets]]`, §4.7), `[[actions]]` with key binding
  (UX3), the §7.8 state list, the supervisor's crash bar and stopped card, the tab strip's marks,
  §5.3 optimistic input with "refused" revert. Each plugin's info-screen art
  (`assets/plugins/*/panel.json`) is reused as the page's "On your keyboard" preview through
  `ui.asset.put`. Those files are **info screen art**, not hub panels; docs should say so (H1.4
  renamed the keyboard's screen).
- **Emulate:**
  - **Windows quick settings / Action Center tiles:** a fixed tile frame, one glance and one
    toggle-like action, the whole tile opens the full page; the user hides and orders tiles.
  - **macOS Control Center modules and widget families:** the system draws the frame and sizes; the
    app only fills a small, capped body. Stale data shows as such, never as current.
  - **Stream Deck / OBS docks:** a status word in a strong colour (LIVE, REC) is the glance; the
    colour repeats the word, never replaces it.
  - **Home Assistant entity cards:** "Unavailable" and "Needs setup" are card states with a fix
    link, not error dialogs; integrations stay loaded while their device is away.
  - **VS Code walkthroughs:** first-run setup is numbered steps inside the page, one action each.
- **Not taken:** per-plugin tile layouts (breaks "systems, not products"), a hub widget per domain
  (a "mic" or "timer" widget), modals for "OBS isn't open", Start/Stop buttons for OBS (the plugin
  only listens), a countdown animation stream for the timer (a once-a-second patch is enough).

## Scope

In: the tray panel tiles and the sidebar pages of these five modules, their empty, unavailable and
error states, their words, and how they look under the supervisor (`docs/scope.md`: one place to
see the keyboard, lights and screens and know they work). Out: the status strip (no status items
in this issue; Mic mute's "Muted" item is a later pick under UX15's budget), `hideout.mic`
(H4.2; when it lands, Mic mute's tile stays the same and only its source changes), the lights
module's own ownership UI (which effect owns the lights), new plugin features.

## The tile pattern (every module)

A tile is a `[[ui.panels]]` entry with `slot = "tile"` (§4.7, §5.1). Two parts:

**Hub frame (data from the manifest, drawn by the hub, same for everyone):**
- Icon and title from the panel's `icon` and `title`.
- The ··· menu: **Open <Module>**, Hide tile, Move left, Move right.
- The state mark (orange ring, red warning mark) and the amber restart strip (below), from
  `module.<id>.state`.
- The stale view: greyed body plus "Last seen <age>" while the module isn't running.
- A key chip next to a button tied to a declared action (spec S3).
- A click anywhere outside the button opens the module's page (as a tab, hideout-tabs-ux.md).

**Module body (the `ui.register` tree, capped, spec S4):**

| Node | Count | Rule |
|---|---|---|
| `value` | exactly 1 | The glance: one word or short phrase that says the state ("Muted", "18:42", "Live"). Long values (over 12 characters) draw smaller. Carries a `tone` (spec S1). |
| `label` | 0–1 | One sub line: what it is about ("Scene: Gameplay", "K70 RGB PRO") or what to do ("Turn on SDK in iCUE settings"). |
| `meter` | 0–1 | Progress, for the timer only today. |
| `button` | 0–1 | The primary action. Tied to a declared `[[actions]]` id where one exists (spec S3), or opens a section of the module's own page (spec S2), or sends `ui.event` (Try again). |

Tones (spec S1): the module picks a word, the hub picks the colour from its theme:
`neutral` (text), `active` (orange: following, counting), `live` (red: muted, recording, live),
`ok` (green: rest), `attention` (amber: needs something from you), `off` (muted grey: idle,
outside app closed). Colour never stands alone: the value word always says the state.

Rules shared by all tiles:
- **One primary action at most.** No action is fine: the tile then only shows state and opens the
  page (OBS, iCUE and OpenRGB while working).
- **Optimistic input** (§5.3): Mute flips to "Muted" at once; a refusal reverts it and the tile
  shows the "refused" state with `data.reason`.
- **Off modules have no tile.** Turning a module off removes its tile from the tray panel; the
  Modules page has Turn on. Hidden tiles are listed under Edit tiles.
- **Outside problems are tile states, not crashes** (spec S6). OBS closed, iCUE closed, a busy
  port: the module stays running and says what it needs. Only a real crash reaches the supervisor.

## The page pattern (every module)

A `[[ui.panels]]` entry with `slot = "page"`. One column, sections in this order, every section
optional except Now; a section with nothing to say is left out:

1. **Now:** the tile's glance at page size, its sub line in full, the primary button (and at most
   one more, e.g. Focus timer's "Start a new focus period").
2. **How to connect:** numbered steps, one action each (`markdown`), with any field the step needs
   inline. Shown only while the module needs setup or can't reach its outside app; collapsed into
   About once it works. Tile buttons "How to", "Set up" and "How to connect" land here (spec S2).
3. **On your keyboard:** what the keyboard shows for this module: an `image` of the current info
   screen frame (from the plugin's info-screen art) and one line about the lights.
4. **Keys:** generated by the hub from `[[actions]]` with the bound key or "Set key" (spec S5).
5. **Settings:** the generated form from `[[settings]]` and `[[secrets]]` (§4.7).
6. **About:** the manifest `description`, as plain sentences.

Header, tab and state lines come from the hub (§5.9), never from the module's tree.

## Per module

Names are the manifest `name`; the proposed renames are question 1. Icons are names from the hub's
icon list (spec S10). Text in quotes is exactly what shows.

### Mic mute (`org.foxglove.mic-mute`, icon `mic`)

| State | Tile value (tone) | Sub line | Button |
|---|---|---|---|
| On | "Mic on" (neutral) | "Default communications mic" | Mute (secondary look), action `toggle`, key chip |
| Muted | "Muted" (live) | same | **Unmute**, action `toggle`, key chip |
| No microphone | "No microphone" (off) | "Windows has no default mic" | none |
| Refused | "Couldn't mute" (attention) | "Windows said no. Try again." | **Try again** |

Page: Now ("Muted" / "Default communications mic: <device name>" / Unmute); On your keyboard
("Info screen: MUTED", "Lights: red while muted"); Keys (Mute or unmute, Mute, Unmute); About:
"This is Windows' mute for the default communications mic. Apps like Discord can still mute
themselves." No setup, so no empty state: it works on first start. No microphone, page line:
"Plug in a microphone, or pick one in Windows Sound settings, Recording tab."

### Focus timer (`org.foxglove.focus-timer`, icon `timer`)

| State | Tile value (tone) | Sub line | Meter | Button |
|---|---|---|---|---|
| Focus | "18:42" (active) | "Focus · round 2 of 4" | time used | **Pause**, action `pause`, key chip |
| Rest | "3:10" (ok) | "Rest · then round 3" | time used | **Pause** |
| Paused | "18:42" (off) | "Paused" | none | **Resume** |
| Keyboard away | "18:42" (active) | "Keyboard not connected" | time used | **Pause** |

- The value binds to `focus.remaining` and `focus.state` (facts it already publishes) or is
  patched once a second; either way the patches stop while no one can see the tile (spec S7).
- Page: Now (big time, phase, Pause, and "Start a new focus period", action `reset`); On your
  keyboard ("Lights: red to focus, green to rest", info-screen preview); Keys; About.
- Empty state: none. It starts its first focus period when turned on (round 1, 25:00).

### OBS (`org.foxglove.broadcast-status`, icon `video`)

| State | Tile value (tone) | Sub line | Button |
|---|---|---|---|
| Off air | "Off air" (neutral) | "Scene: <scene>" | none |
| Recording | "Recording" (live) | "Scene: <scene>" | none |
| Recording paused | "Recording paused" (attention) | "Scene: <scene>" | none |
| Live | "Live" (live) | "Scene: <scene>" | none |
| Live and recording | "Live · Recording" (live) | "Scene: <scene>" | none |
| OBS closed | "OBS isn't open" (off) | "Shows here when you open it" | none |
| Can't reach | "Can't reach OBS" (attention) | "Turn on its WebSocket server" | How to (opens How to connect) |
| Needs password | "Needs password" (attention) | "OBS asks for one" | **Enter password** (opens Settings) |

- It only listens: no Start, Stop or scene buttons anywhere. About says: "Hideout only listens to
  OBS. It never starts, stops or switches anything."
- How to connect: 1. "In OBS, open Tools > WebSocket Server Settings." 2. "Tick Enable WebSocket
  server." 3. "Leave Server Port at 4455." 4. "If Enable Authentication is ticked, copy the
  password, then click Enter password below." Then "Hideout connects by itself." (it reconnects on
  the `apps.watch` event, no polling).
- "OBS closed" is the empty state too: nothing to set up until OBS opens.
- The module must tell "OBS closed" (app watch) from "Can't reach" (open, connect refused) from
  "Needs password" (auth refused); today all three are one "disconnected" frame.

### Corsair iCUE (`org.foxglove.icue`, icon `palette`)

| State | Tile value (tone) | Sub line | Button |
|---|---|---|---|
| Following | "Following" (active) | "<device model>" | none |
| First run (no SDK file) | "Needs setup" (attention) | "Corsair's iCUE SDK file" | **Set up** (opens How to connect) |
| iCUE closed | "iCUE isn't open" (off) | "Follows again when it opens" | none |
| SDK access off | "iCUE said no" (attention) | "Turn on SDK in iCUE settings" | How to |
| No device | "No device found" (attention) | "Nothing matches "<filter>"" or "No Corsair device with lights" | Change device (opens Settings) |

- How to connect: 1. "Download the iCUE SDK from Corsair." (link "Download page", "opens
  corsair.com in your browser") 2. "Unzip it." 3. "Click Choose file and pick
  iCUESDK.x64_2019.dll." (file field, spec S8) 4. "In iCUE, open Settings and turn on SDK."
- Choosing the file takes effect at once (the module handles `settings.set`, §8.8). The words
  "turn the plugin off and on" (today's log line) never show.
- About: "Mirroring only: Hideout reads colours and never takes control of Corsair lighting.
  Foxglove ships no Corsair files."

### OpenRGB (`org.foxglove.openrgb`, icon `lights`)

| State | Tile value (tone) | Sub line | Button |
|---|---|---|---|
| Following | "Following" (active) | "OpenRGB on this PC" | none |
| First run (nothing received yet) | "Waiting for OpenRGB" (attention) | "Add Foxglove in OpenRGB" | **How to connect** |
| Quiet (received before) | "Waiting for OpenRGB" (off) | "Last colours <age> ago" | none |
| Port busy | "Port 5568 is busy" (attention) | "Another app uses it" | **Try again** |

- How to connect: 1. "In OpenRGB, open Settings > E1.31 Devices." 2. "Click Add." 3. "Set IP to
  127.0.0.1 and Universe to 1." 4. "Save, then click Rescan Devices." (app-engineer: check the
  menu names against the current OpenRGB release before shipping.)
- About: "Lets OpenRGB colour your keyboard's lights. Listens on this PC only; nothing leaves your
  PC." "E1.31" appears only in the steps, where OpenRGB uses the word.
- Today a busy port exits with code 1 (`plugin-openrgb-e131/src/main.rs`), so five tries become
  "Stopped after 5 crashes" for an outside problem. Spec S6 makes it the "Port busy" state.

## Under the supervisor (§7.7, §7.8)

Same tile, same page; the hub adds the state. Words and the ladder are exactly
hideout-supervisor-ux.md's.

| State | Tile | Page |
|---|---|---|
| Starting… | cached tile, orange ring on the icon, button enabled (key presses queue, UX3) | cached page, header "Starting…" |
| Restarting | **amber strip** under the title: "Restarting in 4 s (2 of 5)"; body greyed, sub line becomes "Last seen 12 s ago"; button greyed with tooltip "<Module> is restarting. A key press waits up to 2 s." | the **amber crash bar**: "<Module> stopped unexpectedly. Restarting in 4 s (2 of 5)." / "Your place on this page is kept." · Open log; page below greyed with "Last seen …" |
| Stopped after 5 crashes | red warning mark for the icon, red outline, "Stopped after 5 crashes", **Open** | the stopped card and its recovery ladder |
| Needs a newer Hideout | red mark, "Needs a newer Hideout", **Update Hideout** | the §7.8 card |
| Quarantined (§5.5) | red mark, "Its tile couldn't be shown", **Open** | "<Module>'s page couldn't be shown" card |
| Off | no tile | "Off" + **Turn on** |

- **A stale state never looks current.** A greyed "Muted" with "Last seen" matters most for Mic
  mute: Windows keeps the mic as it was while the module is down, and the page says so ("Windows
  keeps the mic as it was."). For lighting modules the page adds one line while stopped: "Your
  keyboard is back on its own lighting." (lights handed back, §7.9).
- The tray icon and menu follow the supervisor plan's quiet rule (no dot for restarts). The tile
  shows the restart because the user opened the tray panel to look.
- Each tile draws in its own guarded scope (§5.5): one bad tile greys itself, not the tray panel.

## Wording

| Where | Text |
|---|---|
| Tile ··· menu | Open <Module> · Hide tile · Move left · Move right |
| Tray panel footer | Open Hideout · Edit tiles |
| Stale caption | "Last seen <n> s ago" / "Last seen <n> min ago" |
| Restart strip (tile) | "Restarting in <n> s (<k> of 5)" |
| Disabled button tooltip | "<Module> is restarting. A key press waits up to 2 s." |
| Stopped tile | "Stopped after 5 crashes" · Open |
| Lights handed back | "Your keyboard is back on its own lighting." |
| Section titles | Now · How to connect · On your keyboard · Keys · Settings · About |
| Unbound action | Set key |

Never shown: plugin ids (`org.foxglove.*`), panel ids, tone names, `ui.*` and error ids, port
numbers except where the user must type or recognise one (OBS's 4455 in its own dialog, OpenRGB's
busy port), "E1.31" outside the OpenRGB steps, "plugin" (the UI word is module, pick H).

## Keyboard and screen reader

- The tray panel is a grid: arrow keys move between tiles, Enter opens the page, Space presses
  the tile's button, Shift+F10 opens the ··· menu.
- A tile's accessible name is title + value + sub: "Mic mute, Muted, default communications mic".
  The button's name includes its key: "Unmute, F13".
- Value changes are announced politely, at most once per 5 s per tile; the timer's seconds are
  never announced (only phase changes: "Focus timer, rest").
  - A phase is the frame's state, the glance's `tone`, and the glance's word when it has no
    digit; the sub line and meter never count. The words are the title, then the glance's word,
    or else the sub line's first part ("Rest · then round 3" gives "Rest").
  - **For module authors:** send a phase change's glance and sub line in **one** `ui.patch`
    (the SDK's `Panels::show` does). The hub speaks on the draw that shows the new tone, so a
    sub line sent in a later patch would be missing from what is said.
  - A change held back by the 5 s window is said when the window ends (one wake, no polling).
- Contrast: every tone on the tile surface is at least 4.5:1 (the mockup script checks it; the
  shell's `theme.rs` contrast test should cover the tone tokens).

## Spec gaps (for the contract owner, not the user)

The plan stays inside §5's schema except for these items. None names a plugin; each is generic.

| # | Gap | Proposal | Needed for H2.8? |
|---|---|---|---|
| S1 | No way to say a value's meaning, so the hub can't colour "Muted" or "Live" without knowing the domain | `tone` prop on `value` and `label`: `neutral`, `active`, `live`, `ok`, `attention`, `off`; the hub maps it to theme tokens; unknown draws `neutral` | yes |
| S2 | A `button` can only send `ui.event`; it can't take the user to the module's own page, a section of it, or its settings | `opens` prop on `button`: `"page"`, `"page#<node id>"` or `"settings"`, own panels only; the hub navigates without a round trip | yes |
| S3 | A button can't say it is a declared action, so the hub can't show its bound key or route click and key the same way | `action` prop on `button` naming an `[[actions]]` id; the hub sends `action.invoke`, shows the key chip, queues during restarts (UX3) | yes |
| S4 | §5.5's caps are page-sized; a tile needs a small, fixed body | Tile caps: at most 12 nodes, depth 3, one `button`, 64 bytes per text; over is `ui.limits` for that tile; the frame (icon, title, menu, state) is hub-drawn | yes |
| S5 | §4.7 says the page "shows each bound key next to its action", but not who draws it | The hub appends a generated Keys section to every page whose module has `[[actions]]`, like the settings form | yes |
| S6 | Nothing tells a module that an absent outside app or a busy port is not a crash | Contract note under §7.7: a module SHOULD stay running and show the problem in its panels, retrying on an event or a user's Try again; it exits non-zero only on a real fault. Plugin changes: OpenRGB's busy port, iCUE's missing file | yes (plugin side) |
| S7 | `ui.visibility` (§5.9) covers page panels only, so a tile can't know nobody is looking | Send `ui.visibility` for `tile` panels too: `visible` while the tray panel is open | yes (timer) |
| S8 | No file picker: a path setting is a free `text` field | `[[settings]] kind = "file"` with optional `filter` (e.g. `"iCUESDK*.dll"`); the hub shows Choose file (native picker) and stores the path | yes (iCUE) |
| S9 | Legacy `plugin.toml` can't carry `[[ui.panels]]` (§4.10: no new keys), and stdio modules need the hub's bridge to carry `ui.*` | Each plugin ships a `module.toml` (the superset) with its panels; confirm the stdio bridge forwards `ui.register`, `ui.patch`, `ui.event` and `ui.visibility` | yes |
| S10 | `icon` is a free string with no published list | Publish the icon names with the schema (`mic`, `timer`, `video`, `palette`, `lights`, …); unknown draws a generic module icon | yes |
| S11 | The default-tile rule (question 3) needs to know whether an `enriches` app was ever seen | A fact `apps.seen.<name>` from `hideout.appwatch`, usable in `when` | only with question 3's pick |

Hub behaviour (no contract change): the "Last seen" stale caption under any greyed tile or page,
from the cached tree's timestamp.

## Hand-off to app-engineer (H2.8)

Acceptance:
- Each module's `module.toml` declares one `tile` and one `page` panel; the lab hub's tray panel
  shows all five with the states in the tile-states image reachable by fixtures (OBS closed, a
  wrong password, OpenRGB with nothing sending, a busy port 5568, iCUE without the SDK file).
- `grep -ri "mic-mute\|focus\|broadcast\|icue\|openrgb" crates/hideout-*` finds nothing.
- Window-free tests over the panel model: each module's registration parses, stays within the
  tile caps (S4), uses only known tones and icons, and every drawn string avoids the "Never shown"
  list.
- Killing a module in the lab shows the amber strip on its tile and the crash bar on its page;
  five kills show the stopped tile with Open. A busy port never reaches the supervisor.

## Decisions (11/10, Hub, designer's picks)

1. Names (manifest `name` only): **Focus timer, OBS, Corsair iCUE, OpenRGB**.
2. Default tiles: **Mic mute and Focus timer**; OBS, Corsair iCUE and OpenRGB show once their app
   has been seen on this PC (needs S11).
3. A stopped module's tile offers **Open** only (to the page's recovery ladder). The "stopped"
   notification is unchanged from docs/hideout-supervisor-ux.md (#968): **Start again · Open
   <Module>**. Start again lives on the notification and the stopped card, never on the tile.
