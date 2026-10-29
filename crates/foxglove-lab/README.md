# Foxglove lab

MIT-licensed hardware-in-the-loop runner for Foxglove's Raw HID host link.
Run commands from the repository root. Listing, cached behavior lookup, and all
unit tests are offline and never enumerate a keyboard.

```powershell
cargo run -p foxglove-lab -- run --suite smoke --list
cargo run -p foxglove-lab -- behaviors
# These commands access hardware; run only when hardware testing is intended:
cargo run -p foxglove-lab -- status --product "Eyelash"
cargo run -p foxglove-lab -- run --suite smoke --non-interactive
cargo run -p foxglove-lab -- run --suite release --out lab-reports/my-run
cargo run -p foxglove-lab -- run --suite tests/hil/smoke.toml --only momentary
```

`--product FILTER`, `--allow-app`, `--keymap PATH` and `--studio-port COMn` apply to all commands.
`--only CASE` selects names containing CASE; an empty match is an error.
`--list` validates the whole suite, prints selected cases, and opens no devices.

The runner refuses hardware access while `Local\Foxglove.Panel` exists. Quit the
desktop app with **tray → Quit**, or deliberately override with `--allow-app`.
The runner keeps this mutex open throughout the run, excluding another runner
and preventing the desktop app from starting during the test.

## Behavior IDs

IDs are **not constants**: they belong to the flashed keyboard's Studio build.
The runner reads the app's saved exchange recording (FXK1, or FXK2 since #636, which
also names the firmware build it was read from), as Foxglove's
`app/src/foxglove/keymap_cache.rs` writes it in `%APPDATA%\Foxglove\config\keymap-*.studio`.
It independently decodes only Studio's behavior details (ID and display name),
using the published Studio protobuf field tags. This follows the app's pairing
of resolved behavior meaning with raw local IDs in `keyboard_actions.rs`, without
depending on the app crate or copying its GPL implementation. Common Studio
names become `kp`, `mo`, `tog`, `to`, `sl`, `rgb_ug`, etc. `behaviors` prints every
cached name, its ID, parameters, and injection eligibility (from the saved copy only;
it opens no device).

`run` and `rgb-latency` use the saved copy **only when it names the build the left
runs** (D7 09, `major.minor.patch-profile-sha`, as the app keys it) and holds ids
(#618). An FXK1 copy, a copy of another build, an empty one, a missing one, or a left
whose build can't be read: the runner reads the ids itself over Studio on the left's
USB serial port (`list_all_behaviors`, then `get_behavior_details` each; 3 s per call),
found by the keyboard's USB ids and the left's serial, or `--studio-port COMn`. The
report's `local_ids` says which source was used and why. If that read fails too (the
left on Bluetooth, the port held by the app's keymap editor or ZMK Studio), named
invokes fail safely and `local_ids` gives both reasons. Use `--keymap PATH` if there
are multiple caches; it is held to the same build rule. There is no unlock operation
and no hardcoded ID fallback.
An explicit `local_id` is accepted but must also resolve to
a recognized safe behavior in the cache: an unclassified numeric ID could type,
reboot, or power off on an older build, so it cannot bypass safety checks.

## Suite format

Suites use `schema = 1`, an optional `include = ["smoke.toml"]` (relative paths),
and `[[cases]]`. Unknown fields, unsupported schema versions, include cycles,
duplicate case names, invalid bounds and invalid step parameters are errors.

```toml
schema = 1
[[cases]]
name = "number-layer"
description = "NUMBER is held and returns to QWERTY"
tags = ["release"]
manual = false
requires = ["host_link>=1"]
steps = [
  { invoke = { behavior = "mo", param1 = 1, hold_ms = 800, expect_status = "ok" } },
  { expect_layer = { active_contains = [1], within_ms = 500 } },
  { expect_layer = { top = 0, within_ms = 1500 } },
]
```

Steps are single-key tables in an ordered `steps` array:

| Step | Fields |
| --- | --- |
| `invoke` | Exactly one of `behavior` (optional `&` prefix) / `local_id`; `param1`, `param2`, `hold_ms` default 0; `expect_status` defaults `ok` |
| `expect_layer` | `active_contains = [N, ...]` and/or `top = N`; `within_ms` defaults 500 |
| `expect_conn` | Optional `usb`, `ble_connected`, `right_link`; `usb_or_ble = true` tests either typing transport; `within_ms` defaults 500 |
| `expect_rgb` | `right_age_max_s` (inclusive), optional `left_on` / `right_on` (RGB_STATE's on/off, #603: what the LEDs show when the firmware has HELLO bit 24, #605), `shown = true` (both halves must report what their LEDs show), `left_look = { hue, sat, brightness, effect }` (the left's stored look) and `right_same_look = true` (the right's stored hue, saturation, brightness and effect equal the left's, #624), optional `within_ms` (up to 120000) |
| `type_into_sink` | `text` (window hint), `via` (array of `invoke` steps), `expect` (received text) |
| `prompt` | `ask`; answer is recorded verbatim, including the newline; only `y`/`yes` confirms a pass |
| `screen` | `half = "left"` / `"right"`, `golden = "golden/left-base.png"`, `mask = [{ x = 0, y = 0, width = 20, height = 10 }]` |
| `press_position` | `pos` (0–47), `state = "down"` / `"up"` |
| `expect_screen_still` | `half`, optional `region` (portrait 68×160), `ms`: captures for `ms` and fails on any change there (a paused animation, #603); its last capture is the baseline for a following `expect_screen_change` |
| `note_right_redraws` | `{}`: notes the right's whole-screen rewrites since its power-on (D7 8E bytes 16-20, HELLO bit 25, #618) once the count holds for 1.2 s |
| `expect_right_redraws` | optional `at_least` (1), `within_ms` (5000): the count grew that much since the note; BLOCKED when the right doesn't report it |

INVOKE status names: `ok` (0), `unknown_id` (1), `busy` (2), `failed` (3),
`not_built_in` (4), `bad_length` (5), `refused` (6). Layer indices in the bundled
keymap are QWERTY=0, NUMBER=1, SYMBOL=2, GAME=3, GAME+=4, ADMIN=5; momentary
bindings target 1, 2 and 4. Layer reports combine active and default masks when
computing the effective top layer. Observations arriving during INVOKE are kept;
reports left over from an earlier invocation are discarded before the next.

Requirements currently understood: `host_link>=N`, `invoke_refusal`,
`screen_readback`, `position_press`, `activity_relay` (HELLO bit 23: the left
relays its activity to the right, #603), `rgb_shown` (HELLO bit 24: RGB_STATE says what
each half's LEDs show, #605), `right_redraws` (HELLO bit 25: D7 8E carries the
right's redraw count, #618; `foxglove-lab status` prints it as `RIGHT_REDRAWS`). `split_guard_test` takes an optional `while_down` invoke,
sent once while HELLO shows the split link down (right link 0), so only the left takes
it (#624); the step is BLOCKED if the link never reads as down. Unsupported requirements block the **whole
case before its first step**. `screen` and `press_position` also imply their
requirements, so omitting `requires` cannot accidentally send a future command.
Screen/position cases are BLOCKED with issues #67/#78. Current HELLO has no
advertised wire contract for either feature. No guessed future capability bits
or command payloads are used. The portable `screen::Capture` trait and fake tests
already implement 1360-byte, 160×68 MSB-first snapshots, 1=ink, exact comparison,
rectangular ignored regions, ×4 PNGs, and red difference images. Goldens can be
native 160×68 or uniform ×4 exports. Hardware capture and seed goldens await #67.

`invoke_refusal` is deliberately **unknown on all current firmware**. The current
HELLO version is 1 both before and after the firmware's refusal patch; none of its
five capability bits proves refusal support. Consequently the bootloader test
is BLOCKED and **never sent**, even with an explicit ID or `expect_status`.
Future support must use a firmware-owned advertised contract, not an arbitrary
version assumption. `CONN_STATE` and `RGB_STATE` also require their HELLO bits.

## Host and app checks

These steps don't press anything. They use Windows' device list and process times.

- `usb_halves = {}` (also `foxglove-lab usb`): classifies each half by its USB serial.
  - Left running with its interfaces → ok.
  - Right absent, or present with **no** interfaces (peripheral firmware on USB power; Windows shows problem 28) → info, expected.
  - Right **with** interfaces → FAIL: left or settings_reset firmware is on it.
  - A half in the UF2 bootloader → warn (`strict = true` makes warnings fail).
- `art_select = { set = 1 }`: selects a built-in art set on the right through the relay, waits for the right to report it, and puts the previous set back. While the app runs (`--allow-app`, its `Local\Foxglove.Panel` mutex exists) the select goes through the app's local API `art.select` (by the set's manifest label) and is read back with one fresh D6 STATUS pair and `art.status`, since the app puts its own choice back after a direct SELECT (#622); with the app quit it is the direct D6 SELECT. `direct = true` always sends the D6 SELECT, even while the app runs: a select made behind the app's back, as another tool would, for cases that check the app puts its pinned set back (#622); its result starts with "direct (forced)". The step's result starts with the path it took, and the report's `art_select_path` says it too ("direct (forced)" when every select in the suite is `direct = true`). Pushed art lives in RAM and can't be restored, so the step reports BLOCKED when pushed art is showing, unless `replace_pushed = true`. When the left's relay is busy with another client's job first (state pushing or receiving, as after a re-link while the app re-applies the right's art, #618), the step waits for that job to end on the art reports that arrive (no timed reads), for at most `idle_within_ms` (30000).
- `idle_cpu = { seconds = 20, max_cpu_ms = 60, settle_s = 120 }`: CPU used by `foxglove` (or `process = "..."`) over the window. Run with `--allow-app`; it's the regression test for #100. `settle_s` (0-600, default 0) measures only once the process has run that long: one timed wait for what's left, counted from its start (#618). The result names the left-screen layout from the app's settings store (`FOXGLOVE_CONFIG_DIR` or `%APPDATA%\Foxglove\foxglove.toml`, with the active profile's override).

`tests/hil/app-idle.toml` holds the app checks; `release.toml` includes it first (its idle case must not measure the other cases' traffic), then `smoke.toml`.

## Safety and manual tests

Outside the sink, only cached, recognized non-typing behaviors (`mo`, `tog`,
`to`, `sl`, `rgb_ug`, `bl`, `ext_power`, `none`, `trans`) can be invoked. Typing,
mouse behaviors, unclassified custom macros, and disruptive behaviors are
rejected. Specs that toggle persistent state should restore it explicitly.

The Windows sink uses a small raw Win32 window via the same `windows` crate
version as the app. It owns and closes the window and pumps `WM_CHAR` messages;
it never synthesizes OS keyboard input. Only verified `kp` taps are accepted in
`via`. The runner checks foreground ownership before each injection and while
collecting characters, and fails immediately if focus is lost. Focus acquisition
has a 5-second deadline; character collection has bounded settling windows.
Windows focus can change concurrently with any USB transaction; keep the test
desktop unattended during typing. Native focus behavior needs hardware testing.
The sink is unavailable on other operating systems.

Every device request is bounded. Poll deadlines are 1–30000 ms (a request already
in flight may finish up to its own timeout after the poll deadline). INVOKE holds
are limited to 10000 ms and the suites request only the 800 ms needed. Firmware
automatically releases the binding; on failure or Ctrl-C the runner keeps the
connection alive through the outstanding release deadline, even if the reply was
lost. A reconnect is treated as an error, so execution cannot continue on a new
connection. Later steps of a failed case are SKIPPED. Subsequent cases still
produce results; cancellation prevents further injection.

Key presses through the keyboard (positions, INVOKE, the sink's taps) are input
to Windows like the user's. `foxglove-lab run` records each one in
`%LOCALAPPDATA%\Foxglove\lab-injections.log` (`FOXGLOVE_LAB_INJECTIONS`
overrides), as `scripts/lab/labkit.py` does for its tray clicks and window posts
(#618): `<start> <end> <the user's last input before it, or -> <what>`. Input from
an injection's start to 2 s past its end is the harness's; the "user is working"
gate (`python scripts/lab/labkit.py idle --min 120`) leaves it out, and any other
input still blocks. See `src/injections.rs`.

Manual prompts require a TTY and have a five-minute deadline. `--non-interactive`
blocks manual cases before injection and lists all prompt text in each report.
No firmware power-off/reset/bootloader command is used for the manual tests.

## Reports and exit status

Default output: `lab-reports/<UTC timestamp>-<git short sha>/` (gitignored).

* `report.json`: environment, runner/app version information, cache source,
  complete HELLO fields, cases and steps, statuses, timings, expected/actual
  values, manual answers, messages, and relative attachment paths.
* `junit.xml`: one test per case; BLOCKED and SKIPPED become `<skipped>` with
  reasons, FAIL becomes `<failure>`; step details are in `system-out`.
* `report.html`: self-contained light/dark page, counts and overall result first,
  case summary, details, expected/actual values, manual answers, and embedded PNGs.

Reports are checkpointed after each completed case. Connection errors produce a
report with failed selected cases. Missing firmware SHAs are explicitly null;
the runner does not invent firmware provenance or UF2 headroom. UF2 preflight
and user-authorized flashing remain separate tools (design P0); this runner does
not flash, implement simulator P2, or invoke external flashing scripts.

Overall PASS means no FAIL, and no BLOCKED case tagged `release`. Exit status is
stricter: zero requires every selected non-manual case to PASS as well. Filtered
cases are SKIPPED and excluded from that rule. A blocked release-tagged manual
case still prevents overall PASS and a successful exit. The bundled smoke suite
therefore cannot currently certify a release: refusal support is unknown.

Dependencies: `foxglove-link` for the existing link; `serde`, `serde_json`, `toml` for
specs/reports; `clap` for CLI validation; `chrono` for UTC report paths; `prost`
for minimal offline Studio metadata; PNG-only `image` for tested bitmap/diffs;
`base64` for self-contained HTML; `windows` for the native sink, mutex and Ctrl-C
(avoids another window/event-loop dependency); `libc` for Unix SIGINT handling.

```powershell
cargo test -p foxglove-lab
powershell -ExecutionPolicy Bypass -File scripts/check.ps1
```

All automated tests use scripted transports, fake layer reports, and fake screen
capture. They do not open the HID interface or a typing window.
