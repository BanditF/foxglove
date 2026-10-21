# Foxglove: one app for the keyboard (KeyPeek + hostpanel + firmware)

**Names.** *Foxglove* is the project, the repo (BanditF/foxglove, formerly zmk-new_corne) and the
desktop app. *Digitalis*, the plant's Latin name, which means "of the finger" and was coined in
1542 by Leonhart Fuchs, is the link between the PC and the keyboard: the `digitalis` crate
(formerly `hostlink`) and the 0xD5/0xD6 protocol. The firmware headers keep their `host_link`
file names.

Status: proposal (2026-10-20). Tracked in this repo's issues.

## Where things stand

| | KeyPeek | hostpanel |
|---|---|---|
| UI | egui overlay + settings window, tray (Settings, Quit) | tray only (status, Pause, Open config, Quit), CLI |
| HID | ZMK: `ZmkProtocol` opens 0xFF60 and reads with a 200 ms timeout (`hid_read`); the keymap comes over ZMK Studio RPC (serial/BLE). VIA/Vial: `qmk-via-api` plus a 0xC0 keepalive | opens 0xFF60/0x61, picks the first interface that answers HELLO; `reply()` waits for `proto, 0x80\|cmd` and drops everything else |
| Reports | in: 0xFF layer state, 0xF1 key events. Never writes to HID for ZMK | out: 0xD5 panel, 0xD6 art. in: their replies |
| Threads | one reader thread per `Keyboard`; keymap commands queued over mpsc and run after each read | one worker thread (render at 1-10 fps, HELLO every 60 s, reconnect every 3 s) |
| Settings | `settings.ini` in `%APPDATA%\srwi\KeyPeek` (rust-ini) | `%APPDATA%\hostpanel\config.toml` |
| Platforms | Windows, macOS, Linux (X11 + Wayland) | Windows (providers stubbed elsewhere) |

On Windows each process's HID handle gets its own copy of every input report, which is why the
two apps can run side by side today. Two problems with that: two handles, and nothing stops two
writers fighting over the panel.

## Recommendation

Build one app, **Foxglove**. It takes over everything KeyPeek does and adds the screens. Its link
to the keyboard is a library, **Digitalis**.

Repo layout (decided 2026-10-20, BanditF/foxglove-tracker#18): **zmk-new_corne grows into Foxglove.**
The firmware stays at the root, so the ZMK build, artifact names and flashing are untouched. The
repo was renamed to `foxglove` (GitHub redirects the old URL). It stays public because the art
requires it.

```
foxglove/ (formerly zmk-new_corne, public)
  boards/ config/ zephyr/ build.yaml   firmware (Digitalis on the keyboard side: host_link, art_relay)
  crates/digitalis/           (MIT) the PC end of the link: protocol, HidLink, art blobs
  app/                        (GPL-3.0) the Foxglove app, KeyPeek absorbed (crate `foxglove`)
  crates/hostpanel/           (MIT) old tray app until cutover, then only the CLI (`foxglove-cli`)
  scripts/ (tools)  docs/     flipper_art.py, uf2end.py, proposal
```

### Names inside the code

| Name | What it is |
|---|---|
| `foxglove` (crate, exe, settings dir, tray) | The app: overlay, keymap editor, settings, tray, screens, providers, local API. All of KeyPeek's features carry over unchanged: VIA/Vial/ZMK, the overlay with all its settings, the keymap editor, monitor choice, and Windows/macOS/Linux support |
| `digitalis` (crate) | Everything that talks to the keyboard: `protocol` (0xD5/0xD6 builders and parsers), `HidLink` (one handle, report dispatch), `art` (NVA1 blobs: encode, decode, push), `panel` (1-bit canvas). It has no UI and no OS providers, so the CLI and later tools can use it too |
| `foxglove::screens`, `foxglove::facts`, `foxglove::providers`, `foxglove::api` | App modules, ported from hostpanel's worker, render and providers |
| Firmware `host_link` / `art_relay` | The keyboard side of Digitalis. File names stay, and the docs call the protocol "Digitalis v1" |
| Runtime names | Settings `%APPDATA%\Foxglove\settings.ini`, mutex `Local\Foxglove.Panel`, pipe `\.\pipe\foxglove` |

### Absorbing KeyPeek

- **How it comes in:** `git subtree add --prefix=app` from BanditF/keypeek, keeping its history,
  then rename the crate to `foxglove` and its window, tray and settings to Foxglove. This replaces
  the earlier submodule plan, because a submodule can't be renamed and restructured freely.
- **Staying in sync with srwi:** `git subtree pull --prefix=app` from srwi/keypeek, at points of
  our choosing. The renames will cause some conflicts, so keep them shallow (crate name, strings,
  the settings path) and leave upstream's file layout alone. New code goes in new modules.
- **Upstreaming** goes through `git subtree split`, then a PR from BanditF/keypeek. It's harder
  than from a plain fork, but it's only for the generic pieces.
- **Credit and license:** the app stays GPL-3.0 with srwi's copyright and LICENSE, and the About
  box says "based on KeyPeek by Stephan Rumswinkel". `digitalis` stays MIT.
- **Settings:** on first run, KeyPeek's `settings.ini` and hostpanel's `config.toml` are imported.

### Keyboard first

The keyboard must never depend on the PC app. Losing the app costs features, never typing.

| Situation | Keyboard does |
|---|---|
| App not running, crashed, or PC asleep | Types normally on both halves. The left panel keeps the **last frame** the PC sent (no timeout back to WPM; a Kconfig sets the timeout, default "never"). The right half keeps playing whatever set it shows |
| Cable unplugged mid-frame | Shows the last *presented* frame. A half-written back buffer is never shown |
| App dies during an art push | The left drops the half-received blob after 10 s with no DATA. The right keeps playing its current set until a new blob passes its CRC |
| Split link drops during a relay | The push fails cleanly (GATT timeout already exists). The right keeps its current set, and the left reports failure next time anyone asks |
| Overlay app gone | Layer and key reports are fire-and-forget. Nothing waits for the PC |

The firmware already gets much of this right: the relay has its own work queue, the HID receive
path uses `K_NO_WAIT` message queues, and GATT calls have timeouts. The gaps are the 65 s fallback
to WPM, the missing timeout for an abandoned left-side receive, and an unverified right-side
"keep current until valid" rule. Each gets a firmware issue plus an ugly-cases test pass.
Lesson from the GATT stall: anything that talks to the other half must have a timeout and must
never block ZMK's own split traffic.

On the app side, each service (screens, art, providers, API) runs on its own thread with its own
panic boundary (`catch_unwind`, logged, restart with backoff). A broken provider or API client
can't take down the overlay.

Rejected: keep two apps and add IPC between them. That still leaves two handles, two trays and
two settings stores. Also rejected: hostpanel as a daemon with KeyPeek as its client.

## Architecture

### One HID connection: `HidLink`

- Owns the single `HidDevice` for usage page 0xFF60, usage 0x61, on one VID/PID. It runs one
  thread that loops `read_timeout(10 ms)`, dispatches the report, then drains a write queue. The
  short timeout matters: KeyPeek's current 200 ms read would add 200 ms to every request.
- Dispatch on byte 0:
  - `0xFF`, `0xF1` go to the overlay sink. `Keyboard`'s reader thread consumes a channel
    instead of calling `protocol.hid_read()`.
  - `0xD5`/`0xD6` with `byte1 & 0x80` go to the pending-request table, keyed `(proto, cmd)`,
    and complete a oneshot. The firmware has no sequence numbers, so allow only one request in
    flight per key. hostpanel's `drain()` goes away: it would eat KeyPeek's reports on a shared
    handle.
  - Everything else goes to a broadcast tap for later integrations and debugging.
- API: `send(report)`, `request(report, expect=(proto,cmd), timeout) -> [u8; 32]`,
  `subscribe(filter) -> Receiver`, `state() -> Connected{product, hello} | Waiting(reason)`.
- Lifecycle: the link does not depend on the ZMK Studio connection. Today a locked Studio
  connection means no `Keyboard`, and so no HID. The panel has to keep running regardless. The
  link reconnects on its own (hostpanel's 3 s retry). `Keyboard::is_alive` follows the link,
  and the ZMK `Reopener` reuses it instead of opening a new handle.
- Matching: by default the link takes the VID/PID of the device chosen in KeyPeek's Device
  dropdown. If none is chosen, it falls back to hostpanel's rule: the first interface that
  answers HELLO, optionally filtered by product name. A HELLO answer is the capability check.
  Without one, the Screens UI stays hidden, so other boards see plain KeyPeek.
- The VIA and Vial paths are left alone in the first pass, since they don't run on this board.
  They could move to `HidLink` later, but that is an upstream question.

### Where hostpanel's modules go

| hostpanel | goes to | change |
|---|---|---|
| `device.rs` constants, `Hello`, `ArtStatus`, tables | `digitalis::protocol` | pure builders/parsers (`hello() -> [u8;32]`, `parse_hello(&[u8])`), no `HidDevice` |
| `device.rs` `show`, `push_art`, `select_art` | `digitalis::{panel_tx, art}` + `foxglove::art_service` | written against a small `Transport` trait (send/request). `HidLink` implements it, and so does a mock for tests |
| `render.rs` | `digitalis::render` | unchanged; `View` becomes a read of the facts store |
| `providers.rs` | `digitalis::providers` behind the `Provider` trait (below) | Windows-only impls stay `cfg(windows)` |
| `config.rs` | KeyPeek `Settings`, new `[screens]` and `[screens.games]` INI sections | one-time import of `config.toml` |
| `main.rs` worker | `foxglove::panel_service` thread | same loop, sends through `HidLink`; keeps the 20 s resend and 60 s HELLO |
| `main.rs` tray | KeyPeek `tray.rs` | new menu items (below) |
| battery CSV | `foxglove::battery_log` | moves under `%APPDATA%\Foxglove` |
| CLI `status/push-art/select/preview` | `hostpanel-cli` | talks to the running app over the local API; falls back to direct HID when the app is not running |

New code goes in new modules, and upstream's files are only touched where they must be. That
keeps `git subtree pull` from srwi cheap. The screens features turn on only when the keyboard
answers HELLO, so on other boards Foxglove is exactly KeyPeek.

## UI

**Settings window:** a new "Screens" section, shown only when the link answered HELLO. It
follows the existing `ui.label` + control rows in `ui_settings.rs`.

- Status: link (product, host-link version), right half (connected/relay), right battery,
  relay discovery result. The relay debug counters sit in a collapsed "Details".
- Left panel: "Drive left screen" toggle, fps (1-10), Pause, and a live preview of the 64x52
  bitmap scaled 3x (copied from `Panel.bits`, the same pixels sent to the board).
- Game hints: an editable exe -> text table, with an "Add foreground app" button that fills in
  the program that was in front before the window got focus.
- Right art:
  - Current state: built-in set n, pushed set, or receiving.
  - Built-in picker: Random, plus 0..`set_count`-1. The firmware only reports a count, so names
    come from a sidecar manifest written by `flipper_art.py` if it exists, otherwise "Set n".
  - Library: a folder of `.bin` blobs (`%APPDATA%\...\art`) with first-frame thumbnails. The NVA1
    decoder is a few dozen lines of XOR+RLE. Each entry gets Push and Delete.
  - "Push file..." through `egui-file-dialog` (already a dependency), with a progress bar from
    `pushed/length` and a cancel that stops sending.
  - "Re-push after power cycle": the art is RAM-only. When STATUS shows `right_state = built-in`
    after a link-up, the app re-pushes the pinned set.

**Tray** (same icon, left click still opens Settings):
`status line (disabled)` / Settings... / Pause left panel (check) / Right art ▸ {Random,
Built-in 1..N, library entries, Push file...} / Quit. hostpanel's "Open config" goes away now
that the settings live in the UI.

## Licensing

- Combining the two is fine. MIT is GPL-3.0-compatible, so MIT code can go into a GPL-3.0
  program, and the combined binary is distributed under GPL-3.0. Keep the MIT notice on the
  `digitalis` files.
- Because the user owns hostpanel, they can relicense it anyway. Keeping `digitalis` as its own
  MIT crate has a payoff: it stays reusable outside KeyPeek (another app, a QMK port), and it
  matches the MIT firmware side (`host_link.h`).
- What GPL-3.0 requires: the repo is public on GitHub, so `app/` is already distributed. Any binary
  handed out needs its source available, which a public repo covers. Private use carries no
  obligations.
- New dependencies (`embedded-graphics`, `crc32fast`, `chrono`, `toml`, `windows`) are
  MIT/Apache-2.0, so they're compatible. `clap` is only needed in the CLI crate.
- Art is a separate question: pushed blobs are data, not code. If the app ever bundles art sets,
  it needs the `ART_CREDITS.md` attributions. The Hammerbeam and crystal art rest on the
  artists' consent, not a license, so leave them out of anything bundled.
- This is a reading of the licenses, not legal advice.

## Upstreaming to srwi/keypeek

Candidates, each as its own PR. Open an issue first to ask whether the author wants it.

1. **`HidLink`/report dispatch refactor.** One Raw HID owner with a pluggable report
   dispatch, plus the 10 ms read with a write queue. It's useful upstream on its own: faster
   commands, and no "Device is locked means no layer events" coupling. This is the PR that makes
   everything else rebase-friendly.
2. **Report tap / extension hook.** A small trait that registers handlers for unclaimed byte-0
   values. It is generic, and lets Foxglove add 0xD5/0xD6 without touching upstream files.
3. **Local API exposing layer state** (read-only). Others could use it for stream overlays or
   lighting. Pitch it only after 1 lands.
4. Monitor selection is already upstream (#49, which replaced our #51), so `app/` starts from
   upstream `master` and drops `monitor-select`.

What stays Foxglove-only: the name, the 0xD5/0xD6 protocol and everything firmware-specific to this board,
the Screens settings and tray items, the Windows providers, the art library, and the hostpanel CLI.

## Migration (both keep working throughout)

1. **Freeze.** Tag `hostpanel v0.1` and the current KeyPeek build, and keep both exes
   (`keypeek-monitor\`, `hostpanel\target\release`).
2. **Panel lock in hostpanel.** A named mutex `Local\Foxglove.Panel`: only the holder
   drives the left panel, and the other shows "panel driven elsewhere". Ship this in hostpanel
   first, so the old and new apps never fight.
3. **Extract `digitalis`.** Make hostpanel a workspace (lib + bin) with the bin using the lib.
   Add golden tests for every report builder/parser against today's `device.rs` bytes. hostpanel
   behaves exactly as before.
4. **Absorb KeyPeek into `app/`** (subtree from upstream master + our fork's commits; rename to
   Foxglove). Add `HidLink` (in `digitalis`) and route ZMK `hid_read` through it.
   No new features yet. Verify the overlay (layers, key presses, reconnect after sleep) while
   hostpanel still runs beside it.
5. **Panel service + Screens tab** (feature `foxglove`). "Drive left screen" is off by default
   and takes the mutex. Run both, switch the toggle, and the panel moves over cleanly.
6. **Art service + UI.** Built-in select, push, library, re-push. Make it match `hostpanel
   push-art` timing and results.
7. **Retire the hostpanel tray.** `hostpanel-cli` talks to the local API, `config.toml` is
   imported once, and the autostart entry moves to the merged exe. Keep the old exe one week
   as a fallback.
8. **Local API and providers** (next section).

Each step leaves both executables runnable. Rolling back means quitting the new one and starting
the old one.

## Extension points

**Facts store.** A typed key-value map with change notifications: `clock.time`, `audio.volume`,
`media.title`, `app.foreground`, `keyboard.layer`, `keyboard.right.battery`, `keyboard.link`.
Everything reads and writes through it. The panel renders from it, so the panel can show the
active layer for free. The local API and plugins read and write it too.

**Provider plugins.**
- In-process: `trait Provider { fn id(&self) -> &str; fn interval(&self) -> Duration; fn poll(&mut self, facts: &mut Facts); }`
  The current clock, volume, media and foreground-exe providers become the first four.
- Out-of-process: an executable listed in settings, started by the app, speaking JSON lines on
  stdin/stdout (`{"set":{"weather.temp":"14°"}}`). It works in any language, crashes stay
  isolated, and there's no Rust ABI problem. No dylib loading.
- Panel layouts: today's `render::draw` becomes the "default" layout. Later layouts are just
  more renderers over the facts store. A declarative layout file is possible but not needed yet.

**Local API.**
- Transport: JSON-RPC over a named pipe (`\\.\pipeoxglove`, Unix socket
  elsewhere, current-user ACL). No network listener by default. A localhost WebSocket for
  browser tools can be added behind an opt-in toggle with a token file.
- Methods: `status`, `facts.get/set/subscribe`, `panel.override(bitmap, ttl)`,
  `art.push(path|bytes)`, `art.select(n)`, `overlay.show/hide`.
- Events: `layer.changed`, `link.changed`.
- `key.event` exists but is off unless the user turns it on: a per-key stream is a keylogger.
- `hid.raw` (send arbitrary reports) is opt-in and limited to unassigned byte-0 values.

**Firmware side.** Reserve 0xD7-0xDF for future host-link protocols and add a capability bitmask
to the HELLO reply, so the app can enable features per firmware instead of per version.

## GitHub issue breakdown (draft)

Each migration step becomes one issue. Labels: `link`, `panel`, `art`, `ui`, `api`, `upstream`,
`hostpanel`, `firmware`.

1. hostpanel: named-mutex panel lock (`hostpanel`)
2. Extract `digitalis` crate with golden report tests (`hostpanel`)
3. Absorb KeyPeek into `app/` as the `foxglove` crate (`upstream`)
4. `HidLink`: single handle, 10 ms reader, dispatch, request/oneshot (`link`)
5. Route ZMK `hid_read` and reconnect through `HidLink` (`link`)
6. Upstream proposal issue + PR: report dispatch refactor (`upstream`)
7. Settings: `[screens]` INI sections + `config.toml` import (`ui`)
8. Panel service (render loop, resend, HELLO, battery log) behind feature `foxglove` (`panel`)
9. Screens tab: status + left-panel controls + live preview + game hints (`ui`, `panel`)
10. Art service: select/push/status with progress and cancel (`art`)
11. Art UI: built-in picker, library with thumbnails, push file, re-push after power cycle (`ui`, `art`)
12. Tray: status, pause, Right art submenu (`ui`)
13. Facts store + Provider trait; port the 4 providers (`api`)
14. Local API (named pipe, JSON-RPC) + `hostpanel-cli` as a client (`api`)
15. Out-of-process provider plugins (JSON lines) (`api`)
16. Firmware: HELLO capability bits, reserve 0xD7-0xDF (`firmware`)
17. Retire hostpanel tray; autostart switch (`hostpanel`)

## Open questions for the user

1. ~~Where do the issues live?~~ Decided: in BanditF/foxglove (moved 2026-10-20; the old
   tracker is archived).
2. ~~Name~~ Decided: Foxglove for the app (crate, exe, settings), and Digitalis for the link
   (crate and protocol). KeyPeek is absorbed whole, via subtree into `app/`.
3. **Fork base.** Rebase onto upstream `master` and drop `monitor-select`, since #49 covers it?
4. **Upstream appetite.** Do you want to pitch the `HidLink` refactor to srwi at all, or keep
   everything in the fork?
5. **digitalis license.** Keep it a separate MIT crate (reusable) or fold it into the GPL tree
   (simpler)?
6. **Art import.** Port `flipper_art.py`'s encoder to Rust so the UI can import PNG/GIF
   directly, or keep Python for making blobs and have the UI push `.bin` only?
7. **Platforms.** Windows-only for the Screens features, or should the providers get
   macOS/Linux implementations too?
8. **Local API scope.** Named pipe only, or a localhost WebSocket from the start (needed for
   browser or Stream Deck-style tools)?
9. **Key-event stream** on the API: never, or opt-in?
10. ~~Repo layout~~ Decided: zmk-new_corne grows into Foxglove (#18).
