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

`--product FILTER`, `--allow-app`, and `--keymap PATH` apply to all commands.
`--only CASE` selects names containing CASE; an empty match is an error.
`--list` validates the whole suite, prints selected cases, and opens no devices.

The runner refuses hardware access while `Local\Foxglove.Panel` exists. Quit the
desktop app with **tray → Quit**, or deliberately override with `--allow-app`.
The runner keeps this mutex open throughout the run, excluding another runner
and preventing the desktop app from starting during the test.

## Behavior IDs

IDs are **not constants**: they belong to the flashed keyboard's Studio build.
The runner reads the same FXK1 exchange recording used by Foxglove's
`app/src/foxglove/keymap_cache.rs` in `%APPDATA%\Foxglove\config\keymap-*.studio`.
It independently decodes only Studio's behavior details (ID and display name),
using the published Studio protobuf field tags. This follows the app's pairing
of resolved behavior meaning with raw local IDs in `keyboard_actions.rs`, without
depending on the app crate or copying its GPL implementation. Common Studio
names become `kp`, `mo`, `tog`, `to`, `sl`, `rgb_ug`, etc. `behaviors` prints every
cached name, its ID, parameters, and injection eligibility.

Use `--keymap PATH` if there are multiple caches. There is no live Studio RPC,
unlock operation, hardcoded ID fallback, or automatic choice among multiple
devices' caches. **Ensure the cache matches the flashed firmware.** HELLO cannot
authenticate a cache or provide a firmware SHA. The report records the cache path
and this limitation. An explicit `local_id` is accepted but must also resolve to
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
| `expect_rgb` | `right_age_max_s` (inclusive), optional `left_on` / `right_on` (RGB_STATE's on/off, #603), optional `within_ms` |
| `type_into_sink` | `text` (window hint), `via` (array of `invoke` steps), `expect` (received text) |
| `prompt` | `ask`; answer is recorded verbatim, including the newline; only `y`/`yes` confirms a pass |
| `screen` | `half = "left"` / `"right"`, `golden = "golden/left-base.png"`, `mask = [{ x = 0, y = 0, width = 20, height = 10 }]` |
| `press_position` | `pos` (0–47), `state = "down"` / `"up"` |
| `expect_screen_still` | `half`, optional `region` (portrait 68×160), `ms`: captures for `ms` and fails on any change there (a paused animation, #603); its last capture is the baseline for a following `expect_screen_change` |

INVOKE status names: `ok` (0), `unknown_id` (1), `busy` (2), `failed` (3),
`not_built_in` (4), `bad_length` (5), `refused` (6). Layer indices in the bundled
keymap are QWERTY=0, NUMBER=1, SYMBOL=2, GAME=3, GAME+=4, ADMIN=5; momentary
bindings target 1, 2 and 4. Layer reports combine active and default masks when
computing the effective top layer. Observations arriving during INVOKE are kept;
reports left over from an earlier invocation are discarded before the next.

Requirements currently understood: `host_link>=N`, `invoke_refusal`,
`screen_readback`, `position_press`, `activity_relay` (HELLO bit 23: the left
relays its activity to the right, #603). Unsupported requirements block the **whole
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
- `art_select = { set = 1 }`: selects a built-in art set on the right through the relay, waits for the right to report it, and puts the previous set back. Pushed art lives in RAM and can't be restored, so the step reports BLOCKED when pushed art is showing, unless `replace_pushed = true`.
- `idle_cpu = { seconds = 20, max_cpu_ms = 60 }`: CPU used by `foxglove` (or `process = "..."`) over the window. Run with `--allow-app`; it's the regression test for #100.

`tests/hil/app-idle.toml` holds the app checks; `release.toml` includes it and `smoke.toml`.

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
