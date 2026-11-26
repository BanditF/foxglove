# Foxglove local API and plugins, v1

Status: built (#69). The contract is in code (`crates/plugin-api`, API 1.9, schema in
`crates/plugin-api/schema/v1.json`); the app serves it on the named pipe
`\\.\pipe\foxglove-<user>` (`crates/plugin-host`), and the plugins and `foxglove-cli` use it. This
builds on [plugins-and-devices.md](plugins-and-devices.md) (the device layer, plugins, the
Plugins tab).

## One protocol, three doors

| Who | Door | Permissions |
|---|---|---|
| **Plugins** (plugins Foxglove starts) | the plugin's stdin/stdout | what its `plugin.toml` declares, approved by the user at install |
| **Outside tools** (Stream Deck, Wallpaper Engine #39, scripts) | named pipe `\\.\pipe\foxglove-<user>`, current user only | what `initialize` asks for. No prompt yet (#1142): everything not sensitive is granted when the tool connects, the sensitive ones (key events, raw HID) never; the per-tool dialog will ask once. The pipe admits this user's processes only, not remote or low-integrity (sandboxed) ones |
| **foxglove-cli** | the same pipe | as an outside tool |

What an outside tool is granted is decided in one place, by the run: a normal run or a lab run (#1449).
A lab run is a test app started with `FOXGLOVE_APP_OK=1` and a `FOXGLOVE_CONFIG_DIR` that is not your real
settings folder, which is how the lab harness (`scripts/lab`) starts one; Foxglove says so once in its log at start.
Only how Foxglove was started decides it, never anything a client sends, and a lab run never grants a
sensitive permission. Today both runs grant the same.

There is no network listener. A localhost WebSocket for browser-based tools (Wallpaper Engine
web wallpapers) can come later behind an opt-in toggle and a token file. It would speak the
same messages.

## Wire

- **JSON-RPC 2.0**, one UTF-8 message per line, at most 256 KiB per line.
- **Requests** carry an `id` and get exactly one response; **events** are notifications from
  the app (no `id`).
- **`initialize` comes first**, and nothing else is accepted before it:
  ```json
  → {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"api":"1.0","client":{"name":"weather","version":"1.2.0"}}}
  ← {"jsonrpc":"2.0","id":1,"result":{"api":"1.0","app":"0.3.0","granted":["facts.write:weather.*","net"]}}
  ```
  A client whose API major isn't served gets error 7 (VERSION) and the connection closes.
- **Errors:** JSON-RPC's own codes, plus the Foxglove codes, which mirror the device layer's errors:
  1 unsupported, 2 disconnected, 3 busy, 4 timeout, 5 denied, 6 backend, 7 version, 8 rate-limited.

## Methods (v1)

| Method | Kind | Needs |
|---|---|---|
| `initialize`, `ping` | call | — |
| `facts.get`, `facts.list`, `facts.subscribe`, `facts.unsubscribe` | call | `facts.read:<pattern>` covering the key/pattern |
| `facts.set` | call | `facts.write:<pattern>`; `device.*` keys are never writable |
| `facts.changed` | event | the subscription |
| `devices.list` / `devices.changed` | call / event | `devices.read` |
| `lighting.state` / `lighting.set` | call | `devices.read` / `lighting.write` |
| `behaviors.list` / `behaviors.invoke` | call | `devices.read` / `behaviors.invoke` |
| `screens.list` | call | `devices.read` |
| `screens.panel_override` | call | `screens.write` |
| `art.status` / `art.select` | call | `devices.read` / `art.write` |
| `key.event` | event | `key.events` (warned: a keylogger) |
| `profiles.list` / `profiles.activate` | call | — / `profiles.write` (API 1.1) |
| `lighting.sync` / `lighting.set_sync` | call | `devices.read` / `lighting.write` (API 1.5) |
| `pc_lighting.list` / `pc_lighting.set_zone` | call | `devices.read` / `lighting.write` (API 1.5) |
| `tray.panel` / `tray.set_panel` | call | `devices.read` / `app.windows` (API 1.6) |
| `overlay.window` / `overlay.restore` | call | `devices.read` / `app.windows` (API 1.6) |
| `lights.page` | call | `devices.read` (API 1.7) |
| `scenes.page` | call | `devices.read` (API 1.10) |
| `scene.status` / `scene.repaint` / `scene.stop` | call | `devices.read` / `lighting.write` / `lighting.write` (API 1.16): see below |
| `lights.status` / `scene.start` | call | `devices.read` / `lighting.write` (API 1.18): see below |
| `focus.get` / `focus.set` | call | `devices.read` / `focus.write` (API 1.12) |
| `app.hand_over` | call | `app.windows` (API 1.17): only the hub's own `hideout.exe` is obeyed, see below |
| `overlay.shown` / `overlay.set_shown` | call | `devices.read` / `app.windows` (API 1.13) |
| `wallpaper-engine.pause` / `.release` / `.status` | call | `wallpaper.control` (API 1.15, #1120): see below. Never granted over MCP. Grants nothing a process of this user can't do itself (Wallpaper Engine's own `-control pause`); the `reason` refuses control, direction and zero-width characters (#1142). |
| `screens.info_override` / `screens.info_release` | call | `screens.write` (API 1.14): the Info screen names of `screens.panel_override` / `screens.panel_release`, which stay as deprecated aliases. SDK: `info_screen_*`, the `panel_*` methods are deprecated. |

The params and results are in the schema. Notes:

- **Facts are the main surface.** Device state appears under `device.<id>.*` (connected, layers,
  battery.right, link.*, lighting.*). The keyboard with Foxglove's link is also under short names
  (#17): `keyboard.connected`, `keyboard.layer` (the highest active layer), `keyboard.layers`,
  `keyboard.game_mode` (a GAME or GAME+ layer is active, by the layer names; #1121),
  `keyboard.right.battery`, `keyboard.link` (`usb`/`ble`), `keyboard.profile` (1-based),
  `keyboard.profile_connected`, and the keyboard's lights (#40): `keyboard.lighting.on`,
  `keyboard.lighting.color` (`#rrggbb`, the chosen colour at full brightness),
  `keyboard.lighting.brightness` (0-100), `keyboard.lighting.effect` (its name) and
  `keyboard.lighting.rainbow` (the effect makes its own colours), all from the firmware's own
  events; on a disconnect all but
  `keyboard.connected` go. Plugins publish their own facts (`weather.temp`), and the panel and
  other plugins read them.
- **`behaviors.invoke` refuses actions that need confirmation** (error 5). A Plugin can never press
  one; only a user's click in Foxglove can. The firmware already refuses &bt, &out, reset and
  soft-off from the PC altogether.
- **`behaviors.list` / `behaviors.invoke` answer from the keymap read on the firmware the keyboard
  runs** (#639). Until it's read (a new or unknown build, #636) both say the behaviors aren't
  known yet (error 3, try again). The list is the app's allowlist: anything not on it (&bt, &out,
  &bootloader, &sys_reset, &soft_off) is unsupported (error 1). API 1.9 adds each entry's
  `behavior`, the keymap's name for it without the `&` (`rgb_ug`, `tog`, `to`, `sl`, `bl`,
  `ext_power`, `caps_word`, `studio_unlock`), so a caller finds an id by kind rather than by its
  label; foxglove-lab takes its ids from here while the app runs.
- **`lighting.set` and `lighting.lease` wait for the keyboard's keymap (#655):** right after
  Foxglove starts, or after a firmware change, the keymap's lighting behavior (`&rgb_ug`) isn't
  known yet. The call then waits for the keymap read, at most 5 s (one log line), and answers
  error 4 "the keyboard's lighting isn't known yet (keymap not read)" if it still isn't; error 1
  "not supported" comes only from a keymap without `&rgb_ug`, at once. Other devices don't wait.
- **`art.select` takes over Foxglove's own art job** (a re-apply, Rotation's timer) instead of
  answering "another art job is running" (#618); that error now means a job a user or another
  caller started is still running.
- **`screens.panel_override`** lasts at most 60 s; the panel then returns to Foxglove's own drawing.
- **`art.select` names built-ins by their manifest name** (stable across firmware), plus slots,
  random, or a file. API 1.6 (#618) adds `"next"` and `"previous"`: the tray panel's Next › and
  ‹, the same command. The call answers once the art job ended (with its error, if it failed),
  and logs one `Foxglove: art … (api): <before> → <after>` line. `art.status` also gives `name`,
  the piece as the tray panel names it. API 1.8 (#666) adds `polarity` for a `file`: `"auto"`,
  `"as_is"` or `"invert"`, kept for that file as the art picker's **Ink** choice is (left out:
  the file's choice so far, auto unless one was made). Every file is sent normalised, so its
  drawing is the ink; the keyboard's dark or light screens decide the look.
- **The tray panel and the overlay window (#612, #618, API 1.6), for agents:** `tray.panel` says
  whether the panel is open and where it was last drawn (screen pixels). `tray.set_panel {open}`
  opens or closes it as a tray click does (an open first un-minimizes the overlay) and answers
  once the overlay's frame did it; one `Foxglove: tray panel … (api asked …)` line each.
  `overlay.window` is the #612 window probe: minimized, shown, the outer rect, the display it
  covers most, and on screen or not. `overlay.restore` runs the overlay's restore check now and
  answers with the probe; it logs the restore line, or `… nothing to restore (api)`. The writes
  need `app.windows`. `foxglove-cli art`, `panel` and `overlay` use them.
- **Keyboard health (#705, API 1.11):** `health.read {device}` reads the keyboard's self-report
  now (both halves' D7 13 blocks, the right's over the relay, up to about 8 s; docs/health.md)
  and answers `{level, rows}`: one row per feature, `{subsystem, item, level, detail}` with
  `level` `ok`, `warn` or `fail` and the reason in plain words, the keyboard's first, then the
  rows Foxglove's own features add. It needs `devices.read`. `foxglove-lab health` uses it while
  the app holds the keyboard.
- **The focused keyboard (#799, API 1.12):** Foxglove shows one keyboard at a time, the
  focus, picked from every keyboard it found this run: the host-link keyboard (`host_link`),
  VIA/Vial keyboards (`via`, read-only) and Dynamic Lighting keyboards (`lamp_array`, lights
  only). `focus.get` answers `{keyboard, keyboards, pins, follow_typing}`: the focus's key (null
  before any keyboard was found), each keyboard's `{key, name, source, connected, read_only,
  device}` (a gone one stays listed with `connected` false until the app restarts), the overlay's
  and the wallpaper feed's pinned keys (null follows the focus), and "follow the keyboard I type
  on" (off by default; App › Keyboard shown, #802: with it on, a few keys on another known
  keyboard move the focus there, at most once a second, and Foxglove reads only which keyboard
  typed, never the key). `focus.set {keyboard}` picks the focus, remembered in the settings;
  `focus.set {keyboard, view}` with `view` `overlay` or `feed` pins that view instead (`keyboard`
  null: follow the focus again). An unknown key is error -32602. Both answer with `focus.get`'s
  result. Every change also sets the `focus.*` facts (`focus.keyboard`, `focus.name`,
  `focus.source`, `focus.connected`, `focus.keyboards`, `focus.connected_keyboards`,
  `focus.overlay`, `focus.feed`), so `facts.subscribe` with `focus.*` hears it.
- **Wallpaper Engine's pause (#1120, API 1.15), one owner:** Foxglove's wallpaper sync is the
  only program that sends Wallpaper Engine its command line, so a pause never runs beside a
  push. `wallpaper-engine.pause {reason}` (at most 64 characters) takes a lease and answers
  `{lease, paused}` (`lease` is a string, such as `we-3-9f1c...`, to give back as it is; the
  hub's lease rules route it): the first lease sends `-control pause`, further ones only join. `paused:
  false` means Wallpaper Engine was already paused (a paired wallpaper reported it: the user's
  pause), so this lease will not play it. `wallpaper-engine.release {lease}` answers `{played,
  why?}`: `play` goes out only when the last lease ends, never over the user's pause
  (`why: "user-paused"`) and never with a full-screen program in front (`"fullscreen"`; Wallpaper
  Engine's own full-screen setting brings the wallpaper back), and an earlier release answers
  `"held"`. If the `play` command itself fails (it can't start or gives no answer in 5 s), the
  answer is `why: "failed"`: the wallpapers are still paused, Foxglove's pause still holds them
  (the pause mark stays), and the play is owed. It goes again, never on a timer, at the next
  Resume, the next last release or end of a connection, quitting, or the next start; Wallpaper
  Engine quitting, a paired wallpaper reporting itself playing (the user pressed Play), or a lease
  taken over the user's own pause ends the debt. The fact `wallpaper_engine.play_owed`
  is `true` while a play is owed and no lease holds the wallpapers, and the health view gains
  `failed pushes N` and `failed plays N` once they happen. A lease ends with its connection; quitting Foxglove plays first (under the
  same rules), then tells the wallpapers it has gone. Pushes wait while paused; the newest state goes
  once, `RELOAD_QUIET` after the play. `wallpaper-engine.status {}` answers `{running, owner:
  "foxglove", leases, paused_by_lease}`. Errors: 1 Wallpaper Engine isn't installed, 2 it isn't
  running, 6 the command couldn't start or timed out (5 s) or ended with an exit code other than 0 (#1399), 8 more than 16 leases on one connection
  (through the hub: 16 per caller, `lease.limit`, and 64 for all its callers together),
  -32602 an unknown lease or one of another connection (or a reason over 64 characters), 5 no
  `wallpaper.control`. The fact `wallpaper_engine.paused_by` holds the live leases' reasons
  (absent when none), and the health view gains `paused N, played N, dropped plays N`. Wallpaper
  Engine has no "am I paused" query, so the user's pause is seen only through a paired wallpaper.
  (#1519) A lab or test run never touches the user's Wallpaper Engine. A Foxglove started with
  `FOXGLOVE_CONFIG_DIR` set to anything but the real settings folder decides this once at start
  and says so in one log line. To it Wallpaper Engine is not installed and not running: these
  calls answer error 1, no command is sent, no wallpaper is handed a pairing ticket, and none
  of its folders is read. A lab case that needs the path sets `FOXGLOVE_WALLPAPER_ENGINE_DIR`
  to a stand-in folder (not an installed copy, or it is ignored); then only that folder is read
  and only a program inside it is run or counted as running.
  (#1142) A pause outlives nothing: while Foxglove's pause holds Wallpaper Engine the file
  `wallpaper-engine-paused` sits beside `foxglove.toml` (written before the `pause` goes out), and
  a Foxglove that starts and finds it (after a crash, a kill or a stopped sync worker) plays
  Wallpaper Engine under the rules above, or just removes it when Wallpaper Engine isn't running.
  If Wallpaper Engine quits while paused for a lease (seen at the next change or lease call), it
  comes back playing: pushes go on, the leases stay, and the last release answers `why:
  "restarted"` without a play. The health row adds `left by a crash N` and `Wallpaper Engine
  quit N` once they happen.
  (#1142) The user sees who holds it: Foxglove's hub page has a **Wallpaper Engine** section
  ("Paused by Arete", the callers' reasons in quotes after the name) and, while a caller holds
  it, **Resume**, which ends every lease (a later `release` answers −32602 "not a lease of this
  connection" here, `lease.unknown` through the hub) and plays under the rules above. Hideout's
  tray menu has **Resume wallpapers (paused by Arete)** while a caller through the hub holds it;
  it ends the hub's callers' leases only (the hub doesn't see this pipe's), so a pause held
  over this pipe ends with the page's Resume. There is no time limit on a pause.
- **Show layer overlay (API 1.13):** `overlay.shown` answers `{on}`, the Keyboard page's
  **Show layer overlay** checkbox; `overlay.set_shown {on}` sets it as the checkbox does (saved
  in the store's `[overlay_shown]`, one `Foxglove: layer overlay on|off` line when it changed)
  and answers the same. Off hides the overlay at once and keeps its other settings. The write
  needs `app.windows`.
- **What the Lights page shows (#674, API 1.7), for agents:** `lights.page {device}` reads the
  keyboard's lights (RGB_STATE) and answers with what the Lights page's Keyboard card shows, from
  the same view-model the page draws: the effect the keyboard reports (`effect`), the caption
  over the colour row (`colour_label`: "Colour", or "Looks like" over the rainbow), `rainbow`,
  which controls are visible (`controls`: swatches, custom, hue, sat, brightness, speed, accent),
  and the "The keyboard shows X; Foxglove saved Y" `note`. Only the host-link keyboard.
  `scripts/lab/lights_page.py` uses it.
- **What the Scenes page's Presets card lists (#618, API 1.10), for agents:** `scenes.page` answers
  with the card's rows from the same sources the card draws: `wallpapers` (each row's `name`, its
  wallpaper's `wallpaper` id from the installed manifests, and `on_screen`, the card's "On screen"
  tag), `effects` and `yours`, in the card's order. `scripts/lab/wallpaper_check.py presets`
  checks it against the installed manifests (the 0.2 sitting's old H4).
- **The scene and the keyboard's lights (API 1.16), for `foxglove-cli scene` and agents:**
  `scene.status` answers `state` (`none`, `running`, `paused`), `scene`, `paused_for`, `keyboard`
  (`lit`, `dark`, `off`, `none`), `dark_since` and `next_try` (local `hh:mm`), `putting_back`,
  `failed` (not put back; Repaint now is offered), the counts `darks` and `recovered` since
  Foxglove started, `last_recovery` and a `detail` line for On-call. "Dark" means a scene runs,
  Foxglove holds the lights, the keyboard reports them off for over 2 minutes, and it is not
  unpowered. `scene.repaint` sends the scene's colours again in full and waits (at most 12 s,
  woken by the keyboard's reports, no timer) for the result: `{scene, lit}`; it refuses with
  "no scene is running", "the scene is paused for ...", or "already putting the lights back".
  `scene.stop` stops the scene as the Scenes page's Stop does and is idempotent: `{scene}`, none
  when nothing ran. `foxglove-cli scene [status|stop|repaint]` prints these; its exit code is
  0 (lit, none running, done), 1 (error), 2 (dark and being put back) or 3 (dark and not put
  back, or still dark after `repaint`). Since API 1.18 `scene.status` (and `lights.status`,
  `scene.start`, below) also come through the hub's alias pipe `foxglove-<user>.<hash>`, served
  by Foxglove's module; `scene.repaint` and `scene.stop` stay with the standalone pipe.
- **Reading and starting the lights through the hub (API 1.18, #1377), for agents:**
  - `lights.status {}` (`devices.read`) answers `{state, scene, look, keyboard, temperature}`:
    `state`, `scene` and `keyboard` as `scene.status`, `look` the running look's kind
    (`temperature`, `scanline` ...; none when no scene runs), and `temperature`, present while the
    Temperature look runs: `{source, reading_state, temp_c, target, fallback}`. `reading_state` is
    `ok`, `waiting`, `missing`, `no-report`, `not-allowed`, `app-off`, `app-stopped` or `no-hub`;
    `temp_c` is a number only with `ok` (never 0 for a missing reading); `target` is the colour the
    lights were last told to show (`#RRGGBB`), none while the fallback shows. The same changes are
    one line each in foxglove.log: `Foxglove: lights: temperature 63 C -> #E82D1F (warm)` and
    `... temperature fallback (no reading)` (#1378).
  - `scene.start {look, params?}` (`lighting.write`) starts a look as the Scenes page's Start does
    and answers `{look, scene}`. `look` is `scanline`, `sweep`, `ripple`, `breathe`, `gradient`,
    `drift`, `two-tone`, `tide`, `temperature` or `sound` (Sound takes `palette` only; the caller never gets the level, and `lights.status` says only `look: sound`). `params` (all optional; one left out keeps what the user saved on the Scenes page for that look, or Foxglove's default when another look was chosen):
    `palette` (one of Foxglove's own names), `speed` 1-10, `direction` down/up/right/left,
    `width_mm` 20-400, `colour` `#RRGGBB`, `background` night/off, and for Temperature `source`
    (`gpu`; lower case letters, digits, `_`, `-`, up to 24), `colours` palette/cool-to-hot,
    `cool_c`, `hot_c` (20-100 in steps of 5 like the page, at least 10 apart). Errors: -32602 for an unknown look, an unknown
    or out-of-range or wrongly typed parameter, or one the look does not use (nothing is clamped
    silently), params over 1 KiB; BUSY (3) when Foxglove is paused, when Temperature has no
    sensors (standalone, the sensors app off, no grant) or for Screen glow, which the API never starts (the
    user's click in the app is the only way, even once allowed); RATE_LIMITED (8) for a start within 2 s of the last one. -32601 only
    for a name the module does not serve. The started look, with the params given, is saved as the chosen look, as with the Scenes
    page's Start. Through the hub the module must hold the user's `provide:` grants for the three
    names (a changed manifest asks again). The router's check of the caller's permission is the
    only permission check: the module is not told who called, so it cannot check again.
- **Case zones and the sync (#617, API 1.5):** `pc_lighting.list` gives each PC lighting
  device (by its key, `vvvv:pppp` plus a serial), its zones with `#rrggbb` colours, whether
  Foxglove writes it, the apps that may fight it, and the last write's status.
  `pc_lighting.set_zone {device, zone, color}` (device by key or a unique part of its name,
  zone by id or name) writes as the Lights page's Apply does, saved, and answers with the list
  once the write is done; an error carries the status. `lighting.sync {device}` and
  `lighting.set_sync {device, on}` read and change a split keyboard's "right half's lights
  follow the left", answered from the keyboard's readback. Both writes log one line, old → new,
  and are refused while Foxglove is paused. `foxglove-cli lights`, `light` and `sync` use them.
- **`hid.raw` is reserved:** it's a valid permission but has no method in v1.

## Versioning

- **Minor (1.x):** only adds, i.e. new methods, new optional fields, new events. Both sides
  ignore what they don't know.
- **Major:** breaking. The app serves the current major and the one before it for at least one
  release cycle.
- **Enforcement:** `schema/v1.json` is generated from the Rust types, and a test fails when the
  types change without the file. Reviewers check that a schema diff matches the version bump.
  Regenerate with `UPDATE_SCHEMA=1 cargo test -p foxglove-plugin-api`.
- **Releases:** Foxglove 0.1.0 served API 1.1; Foxglove 0.2.0 serves API 1.10;
  main (0.3, in progress) serves API 1.18 (`crates/plugin-api`, `API_VERSION`). The app's own version (`initialize`'s `app`) and the API
  version move separately.
- **SDKs:** the Rust types *are* the Rust SDK (MIT). Python/TypeScript clients get generated from
  the schema later.

## Plugins

**Renamed in #87** (Kits → plugins, Den → the Plugins tab and the plugin host, Palm → the device layer). Names that shipped keep working:
- **Manifest:** `kit.toml` is still read when there is no `plugin.toml`.
- **Catalogs:** the formats `foxglove-kit-catalog` (with `kits`) and `foxglove-kit-root` are still accepted.
- **Settings:** `[kits]` (and `[kits.trust]`) are read as `[plugins]`, and the next save writes the new name.
- **Folder:** a `kits\` folder becomes `plugins\` on first use.
- **Environment:** `FOXGLOVE_DEV_UNSIGNED_KITS` and `FOXGLOVE_DISABLE=kits` still work.
- **Unchanged:** the API (methods, permission names, schema) had no kit names in it, so it is the same.

```toml
# %APPDATA%\Foxglove\plugins\com.example.weather\plugin.toml
id = "com.example.weather"
name = "Weather"
version = "1.2.0"
api = "1.0"
run = ["weather.exe"]
permissions = ["facts.write:weather.*", "net"]
provides = ["facts"]
description = "Puts the temperature on the left screen."
```

**Manifest rules** (checked by `Manifest::parse`):
- `id` is reverse-DNS;
- `api` must be served;
- `run` stays inside the plugin's folder;
- unknown keys and permissions are refused.

**Updates:** an update that adds permissions needs the user's OK for those (`added_permissions`).

**Lifecycle** (next PR):
- **Supervision:** Foxglove starts each enabled plugin under `services::spawn_supervised`, with backoff from 1 s to 60 s.
- **Liveness (#281):** no heartbeat. Foxglove sees a plugin exit from its process handle, and a plugin sees Foxglove go when its stdin ends; the SDK then fails waiting calls and ends every stream. A call that gets no answer fails after 30 s (`PluginError::Timeout`). Five crashes in ten minutes disables the plugin, with a notice in the Plugins tab.
- **Rate limits:** per plugin (calls/s, lighting frames/s, panel overrides); over the limit gets error 8.

**Containment on Windows:**
- **Job Object:** every plugin runs in one, killed with Foxglove, with memory and CPU caps and no breakaway.
- **AppContainer:** plugins that declare neither `net` nor `files` run in one, so they have no network of their own and no user files.
- **Network, scoped (#214, API 1.2):**
  - **Why a broker:** Windows can't limit an AppContainer to particular hosts without admin-only, machine-wide settings, and it blocks loopback outright. So Foxglove opens the connections a plugin's permissions name, and the plugin stays in the AppContainer.
  - **Permissions:**
    - `net.connect:<host>:<port>`: TCP. `*.example.com` covers subdomains only, and the port may be `*`.
    - `net.listen:udp:127.0.0.1:<port>`: datagrams to a loopback port.
    - `net.listen:tcp:127.0.0.1:<port>` (API 1.3, #298): TCP connections to a loopback port, so a local page or tool (a Wallpaper Engine wallpaper) can reach the plugin.
  - **Methods:** `net.open {kind: tcp|udp|tcp-listen, host, port}` returns a `stream`. With `tcp-listen`, each connection arrives as `net.accepted {listener, stream, from}`, then works like any other stream; closing the listener's stream stops accepting. `net.send {stream, data}` sends bytes (base64), `net.close` ends a stream, and the events are `net.data {stream, data, from?}` and `net.closed {stream, reason}`.
  - **SDK:** `listen_udp(port)`, and `connect_tcp(host, port)`, which gives a `Read + Write` stream to wrap in TLS or a WebSocket.
    - `listen_tcp(port)` gives a `TcpListener`. Its `accept()` blocks until a connection comes.
    - The SDK's `ws` module is the WebSocket server side: `read_handshake`, `Handshake::check(origins, token)`, `accept` / `reject`, `Reader` for messages, and `write_text` / `write_frame`.
    - The checks are the plugin's choice, since a `file://` page sends `Origin: null`. A per-install token (`?token=` or the sub-protocol) is the real check.
  - **Limits:** 16 streams per plugin, and all of them close when the plugin's connection ends.
- **Network, unrestricted:** bare `net` still works for older plugins. Such a plugin runs outside the AppContainer, still in its Job Object, and the install dialog warns about it ("use any network, without the sandbox").
- **Key actions (#212, API 1.2):**
  - **Declaring:** a plugin lists what a key can trigger in `plugin.toml`, `[[actions]]` with `id` and `name`. Ids are unique, using `a-z`, `0-9` and `-`.
  - **Binding:** in the Plugins tab, the user binds each action to one of F13–F24. Each keyboard can send those keys, and they type nothing; the user puts one on a key in ZMK Studio or VIA. A key triggers one action, and an action has one key.
  - **Delivery:** Foxglove registers the bound keys as global hotkeys and sends `action.invoked {action}` to that plugin only. The SDK delivers them on `Plugin::actions()`. No plugin needs `key.events` for this.
  - **Storage and conflicts:** bindings live in `[plugins.bindings]`. A key another program already holds is flagged in the tab. `FOXGLOVE_DISABLE=actions` turns the hotkeys off.
- **Info screen (#209):**
  - **Screens (API 1.4, #560):** `screens.list {device}`, with `devices.read`, returns each screen as `{id, role, name, width, height, format, rotation}`. `role` is `info` (the panel the PC draws, where plugins draw), `art` or `display`. Screens are named by role ("Info screen", "Art screen") or by the board profile, never by side. SDK: `Plugin::screens()`, `Plugin::panel_target()` (64x52 on an app before 1.4), `PanelTarget::fit()`.
  - **Drawing:** `screens.panel_override {device, width, height, bitmap, ttl_ms, screen?}`, with `screens.write`, shows a plugin's 1 bpp frame (rows MSB first, padded to whole bytes, base64) over an info screen for up to 60 s. `screen` is an id from `screens.list` (API 1.4); without it the frame goes to the first info screen, as API 1.0-1.3 plugins expect. SDK: `Plugin::panel_show()`.
  - **Holding it (API 1.3, #281):**
    - `ttl_ms` 0 holds the frame until the next one replaces it, `screens.panel_release {device}` gives the panel back, or the plugin's connection closes.
    - A steady screen is sent once, never re-sent. SDK: `Plugin::panel_hold()`, `Plugin::panel_release()`.
    - A timed frame goes away by itself, and at once when the connection closes.
  - **Errors:** a frame of the wrong size is refused. SDK: `Plugin::panel_override()`.
- **Waiting for an app (#277, API 1.3):**
  - **Why:** a plugin that needs another app (OBS) waits to be told its windows opened instead of polling for it.
  - **Asking:** `apps.watch {exe}` with `apps.watch:<name>.exe` (such as `apps.watch:obs64.exe`) returns `{exe, open}`: how many of its visible top-level windows are open now. Plugins only, and only the apps they declare. SDK: `Plugin::watch_app()`, then `Plugin::app_windows()`.
  - **Telling:** Foxglove follows windows being shown and destroyed with an out-of-context WinEvent hook (no admin, nothing injected).
    - `app.window.appeared {exe, open}` and `app.window.closed {exe, open}` go to the plugins waiting for that app.
    - `open` is the count afterwards. A close with `open` 0 means the app has probably quit.
    - The watch ends when the plugin's connection closes.
    - An elevated app's windows may not be seen.
  - **Off switch:** `FOXGLOVE_DISABLE=apps-watch`.
  - **Rule:** a retry is started by an event and stopped by one (connected, the last window closed), with an attempt limit only as the fallback. No free-running timers.
- **Settings (#69, API 1.3):** a plugin declares options in `plugin.toml`:
  - `[[settings]]` with `name`, `label`, `kind` (`bool`, `int` or `text`) and `default`, plus `min`/`max` for `int`.
  - The Plugins tab shows a control for each, and the values live in Foxglove's settings.
  - The plugin reads them with `settings.get` and gets them all again as `settings.changed` when the user changes one. SDK: `Plugin::settings()`, `Plugin::settings_changes()`.
  - A stored value that no longer fits its declaration falls back to the default.
- **Plugin data (#209):** each plugin gets `data/` in its folder (`Plugin::data_dir()`) for state between runs. It's writable even in the sandbox, and left out of the signed-file check.
- **Mic mute (`crates/plugin-mic-mute`, #210):**
  - **How it works:** it follows `mic.state` (`audio.inputs.read`) and asks Foxglove to mute or unmute (`audio.inputs.mute`), so it stays sandboxed.
  - **Muted:** a red, high-priority lease, and Art's inverted MUTED screen, held while muted (sent once). When the microphone goes away, the panel is given back.
  - **Unmuted:** a brief white flash, then the lights go back, and LIVE shows for 3 s.
  - **Unknown:** amber, and the `?` screen. A failed read is never shown as muted.
  - **Unavailable:** nothing.
  - **Keys:** `toggle`, `mute` and `unmute`.
  - **Caveat:** this is Windows' endpoint mute, not a privacy guarantee.
- **Broadcast Status (`crates/plugin-broadcast-status`, #211):**
  - **How it works:** it connects to OBS's WebSocket server (obs-websocket v5, port 4455, set up in OBS under Tools → WebSocket Server Settings) through Foxglove (`net.connect:127.0.0.1:4455`), so it stays sandboxed. The password is the plugin's `obs-password` secret, set in the Plugins tab.
  - **Listening only:** it asks for a snapshot (`GetStreamStatus`, `GetRecordStatus`, `GetCurrentProgramScene`) when it connects, then follows OBS's events. It never starts, stops or switches anything.
  - **Streaming:** a red, high-priority lease and the bright LIVE screen, held (sent once). With a recording too, the LIVE + REC screen.
  - **Recording:** red at normal priority and REC, held; amber and the pause bars while paused.
  - **Nothing running:** READY for 3 s, and the lights are left alone.
  - **Connecting:** tries begin when an `obs64.exe` window appears (`apps.watch:obs64.exe`, #277), at start if OBS is open, and when a connection drops.
    - They stop when it connects or OBS's last window closes, and after 10 tries 2 s apart at most.
    - There's no timer while OBS is away.
  - **OBS gone:** NOT CONNECTED for 3 s when a connection is lost (not while OBS simply isn't running). A missing or wrong password is logged.
  - **Scene:** its name goes on the panel under the symbol, up to 15 characters.
  - **Facts:** `broadcast.connected`, `broadcast.streaming`, `broadcast.recording`, `broadcast.paused`, `broadcast.scene`.
- **Sensors (`crates/sensors` is the engine, `crates/sensors-host` builds `sensors.exe`; read-only):**
  - **Facts:** `sensor.cpu.load_pct`, `sensor.gpu.load_pct`, `sensor.ram.used_pct` (whole percent, 0-100), `sensor.gpu.temp_c` (degrees C), `sensor.gpu.clock_mhz` (graphics clock, MHz), `sensor.gpu.power_w` (watts), `sensor.gpu.fan_rpm` (RPM), `sensor.gpu.fan_pct` (the fan's duty, whole percent; a percent is never published as RPM), `sensor.system.temp_c` (the hottest board or case thermal zone Windows reports, whole degrees C; never the processor), and `sensor.cpu.state`, `sensor.gpu.state`, `sensor.ram.state`, `sensor.system.state` (`ok`, `unavailable` or `unknown`). There is no CPU package temperature and no chassis fan RPM: they are not reported.
  - **Sources:** load and memory from Windows' PDH counters (the info screen's own sampler, now shared); GPU temperature, clock, power and fan from NVML, loaded at run time from System32 (`nvml.dll`); the system temperature from the PDH counter `\Thermal Zone Information(*)\Temperature` (Kelvin, its own query), which needs no admin, driver or elevation. Each zone is rounded to whole degrees C; the hottest is the fact and every zone is listed on the page only.
  - **Unavailable:** a value that can't be read (no NVIDIA driver, no GPU, the load denied in the sandbox, no RPM call in the driver) is absent and its group's `state` is `unavailable`. It is never 0.
  - **Idle:** `sensors.exe` is a native Hideout module (hub 1.5) that registers `sensor.*`. Nothing is sampled, no timer exists and no fact is published until the hub's `facts.demand` (contract §8.8, v1.20) says something subscribes to `sensor.*`, or while its own page or tray tile is on screen (a hub `bind` creates no demand, so the module counts its panels from `ui.visibility`, contract §5.9). Sampling is 1 Hz and stops when the demand returns to 0 and neither panel is on screen, and slows to one read every 10 s while game mode is on (it does not stop: the last values stay published). A group's `state` is `ok` while any of its values is present.
  - **Page and tile:** the module registers a "PC sensors" page (processor, graphics card and memory load, and the graphics card's temperature, speed, power and fan; a "System temperature" section with the hottest zone and the zones behind it, numbered hottest first (never the firmware names); a value that isn't reported reads "Not reported", never 0) and a tray tile (the busiest of processor, graphics card and memory, with the other two beside it). The tile is hidden until the user adds it from Edit tiles. A panel that is not on screen holds "Reading…" everywhere, so no number outlives a release. It adds no fact and changes no contract.
  - **Permission:** `facts.write:sensor.*`, and `facts.read:keyboard.game_mode` only to slow down during a game (without that grant it reads at the normal rate). No devices, no key events, no control.
- **Pomodoro (`crates/plugin-focus-timer`, #209):**
  - **Cycle:** focus 25 min, rest 5 min, and a long rest of 15 min every 4 rounds.
  - **Restarts:** the cycle is kept as deadlines in `data/state.json`, so it survives restarts and sleep. After more than a whole cycle away, it starts fresh.
  - **Showing it:** it draws Art's countdown screen (docs/plugin-screens.md) every second while running, and holds the underglow with a lease: red to focus, green to rest, given back while paused.
  - **Paused:** nothing ticks; the paused screen stays held until a key (#281).
  - **Keys:** `pause` and `reset`.
- **Lighting leases (#236, API 1.2):**
  - **Taking and giving back:** `lighting.lease {device, color, effect?, priority: low|normal|high}` takes the lights and returns `{lease}`; `lighting.release {lease}` gives them back. In the SDK, `Plugin::lease()` returns a guard that releases when dropped. Both need `lighting.write`.
  - **Who shows:** the highest-priority lease, and the newest among equals. Use high for alerts (muted), normal for status (recording), and low for ambient light (a colour sync).
  - **Ending:** a plugin's leases end when its connection closes (stopped or crashed). When the last lease on a device ends, its lights go back to how they were before the first one.
  - **Confirmation:** every change is read back and sent again (up to 3 times) if the keyboard didn't take it.
  - **Changing a lease (API 1.3):** `lighting.update {lease, color, effect?}` changes a held lease in place, so a colour stream (OpenRGB, SignalRGB) keeps one lease. SDK: `Lease::set(color)`.
  - **`lighting.set`:** still changes the lights directly, without any of this.
- **Secrets (#213, API 1.2):**
  - **Permission and access:** a plugin with the `secrets` permission keeps passwords and tokens with `secrets.get {name}` and `secrets.set {name, value}`, or `secret()` / `set_secret()` in the SDK. They go in Windows Credential Manager, under `Foxglove/plugin/<id>/<name>`, protected by the user's login and kept out of manifests, command lines, facts, logs and profile exports.
  - **Scope:** a plugin reaches only its own secrets, and outside tools get none.
  - **Entering them:** a plugin can declare what the user enters with `[[secrets]] name, label` in `plugin.toml`, and the Plugins tab then shows a password field for each. Values are never shown back, only "saved" or "not set".
  - **Limits:** names are 1–64 of a-z, 0-9, `-`, `_` and `.`, and values are at most 2048 bytes.
  - **Uninstall:** uninstalling a plugin deletes its secrets.
- **Windows APIs in the sandbox (#215).** `crates/plugin-sandbox-probe` runs each API under the real runner, sandboxed and not. Results on Windows 11:
  - **Media sessions (SMTC):** denied in the AppContainer ("Access is denied"). Plugins read them as facts instead: the app's one media reader publishes `media.title`, `media.artist` and `media.playing`, plus `volume.level` and `volume.muted` (`facts.read:media.*`, `facts.read:volume.*`). They update while the panel service runs.
  - **Microphone mute (Core Audio endpoint volume):** works in the AppContainer with no capability, so the sandbox can't limit it. So Foxglove brokers it instead (#237): it watches the default communications microphone through Core Audio's callbacks, with no polling, and publishes `mic.state` (`muted`, `unmuted`, `unavailable` or `unknown`; a failed read is never shown as muted), `mic.endpoint_id`, `mic.endpoint_name`, `mic.error` and `mic.app_state` (`unknown`; no app reports its own mute yet). `audio.inputs.read` lets a plugin read those facts, and `audio.inputs.mute` lets it call `audio.inputs.set_mute {muted}`. Endpoint mute is what Windows' own mute toggles. It isn't a privacy guarantee: apps can mute themselves, and exclusive-mode streams bypass it. `FOXGLOVE_DISABLE=mic` turns the watcher off. (#1526) A lab or test run never changes a real volume or mute: a Foxglove started with `FOXGLOVE_CONFIG_DIR` set to anything but the real settings folder decides this once at start and says so in one log line ("audio: off for this run"). In it `audio.inputs.set_mute` answers a backend error with those words, `mic.state` is `unavailable`, the knob isn't held, the apps' audio sessions aren't followed and master volume is never engaged, so no app's or device's level is touched at start, while running or at exit. The guard sits at the Core Audio calls themselves. The lab cases that mute the real microphone on purpose (`scripts/lab/mic_mute.py`, the microphone part of `scripts/lab/api_tools.py`) need On-call's separate switch `FOXGLOVE_LAB_REAL_MIC=1`; with it a lab run may read the microphone and set its mute, and still no output volume.
- **"Trusted" Plugins:** the others (vendor SDK Plugins need their vendor service) are marked trusted in the Plugins tab.

**Plugins catalog:** curated, first-party, à la carte. Each plugin has an install button, and nothing
vendor-specific is installed by default. Third-party plugins are a later decision.

**License:** Plugins are separate programs talking over IPC, so they needn't be GPL. The API crate
is MIT. (A reading of the licenses, not legal advice.)

## Phase 3 plan (M6)

1. **This PR:** `crates/plugin-api` (types, permissions, manifest, schema), and this doc.
2. **Host:**
   - a connection loop over any `Read + Write` (stdio and pipe share it);
   - an `initialize` gate, per-call permission checks, facts get/set/subscribe on the device layer's store, and rate limits;
   - tests driven by golden transcripts.
3. **Named pipe** with a current-user ACL, plus `foxglove-cli status` as its first client.
4. **Plugin runner:** spawn, Job Object/AppContainer, supervision, ping.
5. **Plugins tab:** installed plugins (status, permissions, log, enable/disable), the catalog, install/uninstall.
6. **Device methods** (lighting, behaviors, panel override, art) wired through the device layer, and an
   example weather plugin.

## Signing (#159)

Only signed plugins install and run, checked by `crates/plugin-trust`.

**The chain:**
- **Root key:** an **offline root key**, whose public half is pinned in the app at `app/resources/plugin-trust/root.pub`, signs `root.json`:
  `{"format":"foxglove-plugin-root","version":1,"expires":"2027-…Z","catalog_keys":["<minisign pubkey>"]}`.
- **Catalog:** a catalog key named there signs `catalog.json`:
  `{"format":"foxglove-plugin-catalog","sequence":N,"expires":"…Z","plugins":[{"id","version","sha256","files":{"path":"sha256"}}]}`.
- **Rollback:** the app refuses a catalog whose `sequence` is lower than the highest one it has seen.

**Catalog v2 (#192).** Every new field is optional, so v1 files still read.
- **Epoch:** `root.json` gains `"epoch"`, and `catalog.json` must carry the same one. Only the offline root can start a new epoch, and a new epoch restarts the sequence.
- **Rollback state:** the root `version`, the epoch and the sequence never go down. The app keeps them in `[plugins.trust]`, and if it can't save them, it trusts nothing.
- **Entries** gain `"channel"` (`stable` or `beta`), `"url"` (an immutable GitHub Release asset) and `"size"`.
- **Revocations:** `"revocations": [{"id", "versions": [...] (empty = all), "action": "yank"|"revoke", "reason"}]`. A yank stops new installs; a revoke also stops installed copies from launching. A pin never overrides a revocation.

**Where the catalog comes from:**
- The app ships one built-in.
- It downloads newer ones from `https://banditf.github.io/foxglove/plugins/v2/` (`root.json`, `catalog.json` and their `.minisig` files).
- It keeps the last download that verified in `plugin-catalog\`, next to the settings store, and uses whichever verified copy is newest.

**When it checks** (`catalog_refresh.rs`; there's no daily check and no polling, per the user's decision):
- At start, if the last success is more than 24 h old, after 5–60 s of jitter.
- When the Plugins tab opens, at most every 10 minutes.
- On **Check now**.

Each check:
- sends a conditional GET (`If-None-Match`) for each file;
- after a failure, backs off for 15 min, then 1 h, 6 h and 24 h, persisted in `[plugins.refresh]`. **Check now** ignores the backoff.

`FOXGLOVE_PLUGIN_CATALOG_URL` points the download elsewhere (HTTPS, or a localhost test server), and `FOXGLOVE_DISABLE=catalog` turns the start-up check off.

**What's checked:**
- **Install:** the archive's SHA-256 must be in the catalog and not yanked or revoked, its manifest must match the entry's id and version, and the unpacked files must match exactly.
- **Before every launch:** the version must still be in the catalog and not revoked (removing it from the catalog also stops it), and the folder must hold exactly the signed files. `plugin.log` is ignored.

**Making the keys (maintainer, once; tracked in #90):**
1. `minisign -G -p root.pub -s root.key`, then keep `root.key` offline.
2. `minisign -G -p catalog.pub -s catalog.key`.
3. Put the root public key (the base64 line of `root.pub`) in `app/resources/plugin-trust/root.pub`.
4. Write `root.json` listing `catalog.pub`'s key and sign it: `minisign -S -s root.key -m root.json`.
5. For each release, write `catalog.json` with a higher `sequence` and sign it with `catalog.key`.
6. Ship both files and their `.minisig` signatures in `app/resources/plugin-trust/`.

**Launch (#785):** the keys, the signing secret and the Pages workflow that publishes `plugins/v2/` are in [plugin-catalog-launch.md](plugin-catalog-launch.md). `plugin-sign` (in `crates/plugin-trust`, `--features sign`) makes minisign-format keys and signs `root.json` and `catalog.json`, so the `minisign` tool isn't needed.

**Publishing (#190):** `scripts/publish-plugins.ps1`, with the catalog edits done by `plugin-catalog` (in `crates/plugin-trust`). The order is: zips first, checked, then the signed catalog.
1. Run `powershell -File scripts\publish-plugins.ps1 -Crate plugin-focus-timer[,…] [-Channel beta]`. This dry run:
   - packs each plugin;
   - names its release `plugin-<slug>-vX.Y.Z`, with the asset `<id>-<version>-windows-x86_64.zip`;
   - updates `target\plugin-site\plugins\v2\catalog.json`: it adds the entry with its URL, size and hashes, keeps one previous version per channel, then bumps the sequence and sets a 30-day expiry.
2. Run the same command with `-Release`. It creates the GitHub releases (never marked latest; beta ones as prereleases), then downloads each asset back and checks its SHA-256. A release that already exists is only checked, since releases are immutable.
3. **Maintainer step:** `minisign -S -s <catalog.key> -m target\plugin-site\plugins\v2\catalog.json`. The key never passes through an agent.
4. Run `-Pages`. It pushes `plugins/v2/` to the `gh-pages` branch, and refuses an unsigned or since-changed catalog.

To withdraw a version: `plugin-catalog yank|revoke <catalog.json> <id> [version…] --reason …`, then `-Resign`, sign, and `-Pages`. The weekly re-sign is `-Resign`, sign, `-Pages`. GitHub Pages must be turned on for the `gh-pages` branch once, in the repository settings.

**Developer mode (#381):** a switch at the foot of the Plugins page, off by default, kept in `[plugins.dev]`. Its warning: "Lets Foxglove load plugins nobody has signed. Only turn this on for plugins you build yourself."
- **Off:** an unsigned archive doesn't install, and an unsigned plugin that is already installed doesn't run. It stays installed, and its card says why ("Off: nobody has signed it, and unsigned plugins only run in developer mode.").
- **On:** each unsigned build still needs a one-time OK, ticked on the install card. The OK is kept in `[plugins.dev.allowed]` by the build's package hash: the SHA-256 of its files' paths and SHA-256s (the list the catalog would sign), one `path<TAB>sha256` line per file, sorted. A changed build is a new hash and asks again ("Allow this build" on its card). Removing the plugin forgets its OKs.
- **Signed plugins** are unaffected either way. A revoked version never runs, and a yanked one never installs, in developer mode too.
- **Badges:** an unsigned plugin is SIDELOADED, even under a catalog plugin's id, with UNSIGNED under its name.
- **Ids:** installing an unsigned file under a catalog plugin's id warns that it takes that plugin's place. Installing a signed plugin over an unsigned copy warns that it replaces it.
- **Updates:** nothing updates a sideloaded plugin from the catalog; the catalog refresh only fetches the signed files.
- `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` turns developer mode on for one run (the Test lab), and the switch shows it held on.
- **Before the signing key exists:** while `root.pub` is still the placeholder, the plugins that were installed when this policy first ran (`[plugins.grandfather]`, by id and version) keep running as before, and a health row warns "plugin signing key not set; installed plugins allowed". New installs from a file get the unsigned flow anyway, and installing one of them again from a file ends its grandfathering. Once a build ships the real key, the normal policy applies, and the Plugins page says once: "Plugin signing is on now: plugins nobody has signed only run in developer mode." That it was shown is kept in `[plugins.grandfather]`.

**Test hook:** with `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` and a test `FOXGLOVE_CONFIG_DIR` (never the real settings folder), `FOXGLOVE_PLUGINS_AUTOINSTALL=<zip or folder>` installs and enables that plugin at start with all its declared permissions, without the approval dialog. It writes a warning to stderr and to `autoinstall.log` in the config dir. It is meant for the Test lab only.

## Writing a plugin (Rust)

The `foxglove-plugin` crate (`crates/plugin-sdk`, MIT):
- **Connect:** `Plugin::connect(name, version)` says hello and pings every 10 s for you.
- **Calls:** `set_fact`, `fact`, `subscribe` plus `events()`, `devices`, `set_lighting`, and a generic `call`.

The example is `crates/plugin-focus-timer`, a focus/rest timer that publishes `focus.state` and `focus.remaining` and colours the underglow. It asks only for `devices.read`, `lighting.write` and `facts.write:focus.*`, so it runs in the AppContainer.

The second example, `crates/plugin-openrgb-e131`, receives network data through Foxglove (`net.listen:udp:127.0.0.1:5568`) and stays sandboxed; see [openrgb-plugin.md](openrgb-plugin.md).

**Pack a plugin:** `powershell -File scripts\pack-plugin.ps1 plugin-focus-timer` builds it, zips `plugin.toml` and the program into `target\plugins\`, and prints the catalog entry (archive and file SHA-256s) to add to `catalog.json` before signing. Until the signing keys exist, install it in developer mode.

## Publishing your own plugin

Anyone can write a plugin and share its archive. Users install it with **Install from file…**. Until it's in the signed catalog, it's unsigned, so it installs and runs only in developer mode, after the user's one-time OK (see Signing).

**Package layout:** a `.zip` with `plugin.toml` at its top, next to the program and anything it needs:
```
com.example.weather-1.2.0.zip
├── plugin.toml
├── weather.exe
└── icon.svg        (optional, see plugin-icons.md)
```
- The folder it unpacks to is named after `id`, so the id must be yours: reverse-DNS on a domain or account you control. A catalog plugin's id is warned about, and taking one replaces that plugin.
- `run` names the program inside the package. Nothing outside the package is run.
- The runner writes `plugin.log` and a data folder itself, so don't ship them.
- At most 64 MiB unpacked and 2000 files.

**Packing:** `powershell -File scripts\pack-plugin.ps1 <crate>` builds a crate in this repository and zips it into `target\plugins\<id>-<version>.zip`, with the catalog entry (archive and file SHA-256s). For a plugin built elsewhere, zip the files yourself so that `plugin.toml` is at the top.

**Permissions:** ask for the least that works. Users see each one on the install card, and the sensitive ones are marked. The plugin gets only what it declared and the user approved. A call needing anything else fails with "not permitted" (error 5).
- `facts.write:<your prefix>.*` rather than broad patterns, and read-only permissions where they do.
- A scoped `net.connect:<host>:<port>` or `net.listen:…` keeps the plugin in the AppContainer. Bare `net` or `files` runs it outside the sandbox, and its card says so.
- An update that adds permissions asks the user again.

**Getting into the catalog:** open an issue on BanditF/foxglove with the source and a release asset. A maintainer packs and signs it (see Publishing above). Signed, it installs and runs without developer mode.


## Through the Hideout hub (module mode, #1143 P6)

Off by default: only `foxglove.exe --module` (started by `hideout.exe`, registered with `--adopt`) does
this. The app then serves no `foxglove-<user>` pipe; the hub serves that alias and routes calls.

- The router checks the caller's permission (the table above) before the app is asked; the app
  answers only the names in `SERVED` (`app/src/foxglove/hub_link.rs`) and treats everything else as
  -32601. The answers are the standalone handlers' answers (a test compares them).
- Served: `health.read`, `lights.page`, `scenes.page`, `lights.status`, `scene.status`, `scene.start`
  (API 1.18, #1377; `scene.repaint` and `scene.stop` stay with the standalone pipe),
  `overlay.window|restore|shown|set_shown`,
  `tray.panel|set_panel`, `pc_lighting.list|set_zone`, `wallpaper-engine.pause|release|status`
  (#1120). The user grants each as `provide:<method>`.
- `wallpaper-engine.*` through the hub follows the contract's lease rules (section 8.5): only the
  caller holding a lease can name it, the hub's `lease.closed` (any reason) ends that lease and
  plays when it was the last, and the link to the hub dropping ends every routed lease. A
  caller of the hub uses the same three methods with `wallpaper.control`.
- Not served, because the contract gives an adopted module no first-party area (§6.3): `devices.*`
  (so `foxglove-cli status` still needs the standalone pipe), `lighting.*`, `art.*`, `screens.*`,
  `behaviors.*`, `profiles.*`, `focus.*`. Facts come from the hub's cache (`keyboard.*` is
  published by the module).

- **The hub asks a standalone copy to quit (API 1.17, first-run FR5):** `app.hand_over {}` asks a
  standalone Foxglove to hand back what it holds and exit cleanly, as the tray's Quit does, so the
  hub can start it as an app inside Hideout. The kernel's answer for the caller's process id
  decides, never anything in the request: it must be a `hideout.exe` that passes the same identity
  checks as a pipe's server (same user, Medium integrity or above, no AppContainer, its install
  folder). Any other caller gets error 5 (denied) and nothing happens. Calling it on an
  app that runs inside Hideout does nothing (that app serves no pipe).
  Limits that are accepted: the caller's process id is read once per connection, so a client that
  exits and whose id is reused by a verified `hideout.exe` before the check would pass (the only
  effect is a clean quit, which `taskkill` already allows); a standalone copy that starts between
  the hub's last look and its own start can take the name, as it can today. After a copy has
  closed, the hub serves the `foxglove-<user>` alias itself (a probe first), so CLI and SDK tools
  on the alias keep working without a hub restart. Only Foxglove is wired for this so far; the
  pipe name and image should later come from the manifest.
