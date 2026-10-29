# Real ZMK keymap simulator tests

These cases run ZMK **v0.3.0**, not the Python layer replay. The tag uses
Zephyr `v3.5.0+zmk-fixes`, board **`native_posix_64`**, executable
`zephyr/zmk.exe`, and container `docker.io/zmkfirmware/zmk-build-arm:3.5`.
The design note's `native_sim` / `stable` names do not apply to this tag.

Each case includes `common/keymap.dtsi`, which includes the production
`config/eyelash_corne.keymap` verbatim. Production keymap/config and widgets
are untouched. `ZMK_CONFIG` points at each case, so the production `.conf`
(Nordic GPIO, displays, BLE, LEDs, split transport, etc.) is intentionally
not loaded. This suite checks central keymap logic, not split transport,
Studio, host delivery, physical sensors, or actual power management.

## Matrix and physical layout

The vendor shield's `default_layout` references `default_transform` in
`eyelash_corne.dtsi`. `common/matrix.dtsi` reproduces that transform exactly:
48 populated positions in a sparse **5-row × 14-column** mock matrix.
It selects `zmk,matrix-transform`, the supported v0.3.0 compatibility path;
physical key coordinates/rotations used by Studio are unnecessary here.
The upstream native board overlay supplies `&kscan`, UART, and `exit-after`.
The vendor `main` branch is mutable; recheck the mapping when changing boards.

Some useful position → matrix coordinates:

| Position | Key | Row, column |
| --- | --- | --- |
| 1 | Q | 0, 1 |
| 12 | Backspace | 0, 11 |
| 13 | Shift/Caps tap dance | 1, 0 |
| 15 | S | 1, 2 |
| 18 | G / ADMIN GAME toggle | 1, 5 |
| 29 | Z | 2, 1 |
| 37 | M / ADMIN soft-off | 2, 7 |
| 41 | Escape | 2, 11 |
| 43 | left middle thumb | 3, 4 |
| 46 | right middle thumb | 3, 7 |

## Coverage and expected traces

`keycode_events.snapshot` files are **hand-authored, unverified expectations**.
None has been generated or validated by a simulator locally. A normal CI run
will compare them strictly; do not assume the suite passes until CI runs.
The filtered output follows upstream's `hid_listener_keycode` → `kp` format.
The extra `SIM_SOFT_OFF` marker observes the power-off boundary.

| Case | Assertions |
| --- | --- |
| `base` | Tab Q A Z on left; Y P H M Enter on right |
| `thumbs` | Both press orders × both release orders; position 1 gives N1 on NUMBER, shifted N1 (`!`) on SYMBOL, F1 on ADMIN, Q after release; Tab verifies base |
| `thumbs-game` | Same four sequences after real ADMIN+G `&tog 3`; Esc verifies GAME, Tab after toggling off; GAME supplies `&mo 4` on the left thumb in either order, so GAME+ plus SYMBOL activates ADMIN |
| `combos` | The only current combo (12+41, default 50 ms) toggles GAME on/off in both press orders; 120 ms separation emits Backspace/Esc and leaves Tab unchanged |
| `soft-off` | M emits M on base, nothing for NUMBER RGB command, Plus on SYMBOL; ADMIN+M 30/1900 ms emits no key and no power-off, 2100 ms calls power-off once; interleaved F1 taps distinguish these intervals. Q+S+Z pressed together emits Q/S/Z down and up, then Q still works |
| `sleep` | Deep sleep at the **production** idle timeout (`prod.keys` copies `CONFIG_ZMK_IDLE_SLEEP_TIMEOUT` from `config/eyelash_corne.conf`, #223): after one Q, idle at 30 s, still idle (not asleep) 29 min 50 s later, then sleep and `SIM_SLEEP` (the stubbed `sys_poweroff` ends the run). Simulated time runs unslowed; `module/sleep_sim.c` watches the clock, and there's no exit-after, so a board that never sleeps fails on the timeout |
| `activity-relay` | The left's activity relay (#603): the real `activity_relay.c` on ZMK's real activity, with the relay replaced by a print of the op the right would be sent (`module/activity_sim.c`, built by the board's own `art_relay_activity_op()`). Idle at 3 s and sleep at 20 s (`case.conf`): Q while active since boot sends nothing; idle sends `10 01`; Q wakes (`10 00`); Q while active sends nothing; idle; a cable going in, played as `usb_wake.c` does it (a call into ZMK's activity listener), sends `10 00`; idle; going to sleep sends `10 02` before the power-off (`SIM_SLEEP` ends the run). One message per change, none between. The Bluetooth send, the lane/request pair and the right's handling aren't simulated (no split transport here) |
| `battery-dim` | The real `dim_strip.c` and `battery_dim.c` (#222) in front of a fake strip with fake USB and charge readings (`module/dim_sim.c`), at the shield's shipped `NICE_VIEW_RGB_DIM_*` defaults: 80% before any event and on battery, 100% on USB, 12% below 20% charge and back to 80% only at 22% (hysteresis); switched off is 100% everywhere, a level of 0 is refused, the app's own levels apply; 255,128,10 / 100,0,200 scale by integer division with a lit channel kept at 1 or more, and the caller's buffer is never written |
| `panel-stream` | The plugin screens' left-panel stream (#238), played by `module/panel_stream_sim.c` against the real `host_link.c`: 10 whole frames (FILL, 18 ROWS, PRESENT) at 2 Hz with a HELLO and a CONN_STATE after each, then 30 frames back to back and a HELLO. Every request is answered (no other reply appears), the shown frame is the last one sent in each phase, and four Q taps meanwhile type normally. Simulated time stands still while code runs, so this checks ordering and loss, not CPU time |
| `host-frames` | SET_FRAME (#272) against the real `host_link.c` and `host_frames.c`, with the strip and the art relay faked in `module/frames_sim.c`: HELLO bits 14 and 22; LAMP_PARTS (D7 10, #561) answers the left's chain and the right's (2 parts of 21); a chunk without SHOW doesn't reply or draw; a whole frame takes both halves and draws the left; the right gets only the lamps that changed (one lamp, nothing for the same frame, a fill for one colour per half, the span from `art_relay.h`); a burst of 5 frames draws the left each time and sends the right the first and the newest only (100 ms gap); 10 lamps, lamps past 41 and a short report answer 5; RELEASE, a 3 s timeout and an endpoint change hand both halves back, the last two with an unsolicited `D7 8C` (event 1) |
| `host-effects` | SET_EFFECT (#490) against the real `host_link.c`, `host_effects.c`, `effect_run.c`, `lamp_effect.c` and `host_frames.c`, with the real `rgb_idle.c`, and the strip, the art relay and ZMK's underglow faked in `module/effects_sim.c`: every vector `crates/scene` writes to `module/effect_vectors.inc` within 2 per channel; every Scenes-page direction (scanline and sweep, down/up/right/left) as the app sends it, from `app/src/foxglove/keyboard_direction_vectors.rs` (`module/direction_vectors.inc`), rendered on both halves with the firmware's lamp table and the right's mirror, each LED within 8 per channel of the app's colour; HELLO bit 19; PLACE refuses a half without area; a still effect takes both halves and draws once; a moving scanline draws a frame every 33 ms on the clock it was sent with, and keeps drawing through an endpoint change; PLACE moves it on the same clock; SET_FRAME takes over (the right told by the frames) and an effect takes back without a `D7 8C` event; bad kinds, colour counts, angles, lengths and ops answer 5; LAMPS (HELLO bit 21, #550) refuses a third half, LEDs past 20, values past 1000 and a short report, draws the left's LEDs where it puts them, hands the right its own points (not mirrored) with the effect, and count 0 puts a half back on the built-in table; RELEASE hands both halves back. The scene owns the lights (#542): it keeps drawing through idle (3 s here) and the user's off, RGB_SYNC's left on, the PC's D7 83 events and what the underglow relay tells the right follow the real on/off meanwhile, RELEASE puts that on/off back, and as the right it keeps drawing while the left's link drops and comes back saying off, then follows that off on RELEASE |
| `tap-dance` | Real position 13: single tap gives Shift, 300 ms hold gives Shift, double tap gives Caps; final Q checks cleanup |

There is **no `&mt`, `&lt`, or custom hold-tap binding** in this keymap.
The final case tests the existing tap dance instead of adding artificial
hold-taps to production. Its default tapping term is 200 ms. If a hold-tap
is added later, add tap/hold/interruption cases for its actual position.

In v0.3.0 kscan-mock, the time in `ZMK_MOCK_PRESS/RELEASE` is the delay
**after that event** before the next event (also used for the first event's
initial delay). Put a long hold duration on the PRESS, not the RELEASE.
Cases finish with an unmapped `(4,0)` release and a 300 ms drain. It produces
an expected unfiltered warning, no key event, and gives work/log queues time
to finish before `exit-after` exits.

## Test-only substitutions

1. `&bt` and `&rgb_ug` use `foxglove,sim-noop-two`, preserving their existing
   two parameter cells. The module driver consumes press/release without
   emitting keycodes. BLE profile changes and LED effects are not tested.
2. `CONFIG_ZMK_PM_SOFT_OFF=n` avoids native board power-off dependencies.
   The module compiles **upstream `src/behaviors/behavior_soft_off.c`** directly,
   retaining its real `hold-time-ms` logic, binding, and release handling.
   Only `zmk_pm_soft_off()` is replaced: it prints `SIM_SOFT_OFF` and returns.
   This verifies a short hold is rejected and a >2000 ms hold reaches the
   power-off boundary. It does not verify shutdown, peripheral propagation,
   or wake. Upstream checks `held > hold_time_ms` on **release**, not at the
   instant the threshold is crossed.
3. The mock matrix replaces the GPIO scanner and split coordinate offsets.
   Both halves are injected centrally in the complete vendor coordinate space.

Mouse `&mkp`, `&mmv`, `&msc`, input listeners/processors, `&out`, reset/bootloader,
and sensor-rotate behaviors remain upstream. Pointing is enabled to compile
them; encoder hardware is omitted and sensor events are not injected.
Reset/bootloader are never pressed. `&ext_power` and Studio unlock are not
referenced by the production keymap; their hardware features remain disabled
and need no referenced behavior stubs. No custom nice!view module is loaded.

## Running and CI

Inside the upstream build container, with a v0.3.0 west workspace already
initialized, updated, and `west zephyr-export` run:

```sh
cd /path/to/workspace/zmk/app
export ZMK_SRC_DIR="$PWD"
bash /path/to/foxglove/tests/zmk-sim/run.sh
```

`ZMK_BUILD_DIR` optionally selects a different build/log directory. The runner
pristine-builds every case with `CONFIG_ASSERT=y`, the shared extra `.conf`,
and only the simulator module in `ZMK_EXTRA_MODULES`. It strips log prefixes
with `sed -e 's/.*> //'`, applies each case's `events.patterns`, and runs
upstream's `diff -auZ` comparison. It retains build/full/filtered/stderr logs
and diffs, continues through case failures, and fails on build errors,
process errors/timeouts, empty filtered output, or snapshot differences.

`.github/workflows/zmk-sim.yml` checks out this repo and upstream v0.3.0,
initializes west from **upstream `app/west.yml`**, caches the Zephyr modules,
and runs the suite with a 30 minute job timeout. The vendor and production
display/Raw HID modules are deliberately not imported: all keymap logic comes
from ZMK and all hardware substitutions are local and explicit. Logs and
generated DTS/Kconfig are uploaded on failure. Test-only changes are excluded
from the separate firmware workflow.

### Capturing/reviewing snapshots safely

First run normal CI against the hand-authored expectations. If traces differ,
inspect the build/full logs and diff before changing any expectation. To
capture actual traces, manually dispatch **ZMK simulator** on the intended
branch with `auto_accept: true`, or run in the container:

```sh
ZMK_TESTS_AUTO_ACCEPT=1 bash /path/to/foxglove/tests/zmk-sim/run.sh
```

Download `zmk-sim-candidate-snapshots`. CI only uploads files: it never commits,
pushes, or modifies the branch. A human must compare every candidate with the
case table above, the existing expected snapshot, and its full trace, especially
absence of combo fallback keycodes, F1 layer probes, and the single soft-off
marker between the final two F1 probes. Copy only reviewed snapshots into the
corresponding case directories and rerun **without auto-accept**. Auto-accept
can bless regressions and is not correctness evidence. Build/process/empty
trace failures still fail in auto-accept mode. Review candidates only from a
run in which every case built and ran successfully.

## Adding a case

Create a direct child directory with `native_posix_64.keymap` including
`../common/keymap.dtsi`, an `&kscan { events = <...>; };` sequence using the
vendor coordinates, `events.patterns` with a short coverage comment, and
`keycode_events.snapshot`. Do not copy or replace the production keymap.
Use stable keycode/behavior observations and a drain event at the end.
Add the intended output to the coverage table, capture/review the snapshot,
then run the full suite with auto-accept disabled. The runner discovers cases
automatically. Keep stubs confined to the test module and document additions.

## Upstream sources read

Mechanics are modeled on these exact v0.3.0 files (not current documentation):

- [app/run-test.sh](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/run-test.sh)
- [.github/workflows/test.yml](https://github.com/zmkfirmware/zmk/blob/v0.3.0/.github/workflows/test.yml)
- [app/west.yml](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/west.yml)
- [native board overlay](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/boards/native_posix_64.overlay) and [configuration](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/boards/native_posix_64.conf)
- [conditional-layer/mo-overlap keymap](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/tests/conditional-layer/mo-overlap/native_posix_64.keymap), [filter](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/tests/conditional-layer/mo-overlap/events.patterns), and [snapshot](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/tests/conditional-layer/mo-overlap/keycode_events.snapshot)
- [hold-tap/hold-preferred/2-dn-timer-up keymap](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/tests/hold-tap/hold-preferred/2-dn-timer-up/native_posix_64.keymap) and [filter](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/tests/hold-tap/hold-preferred/2-dn-timer-up/events.patterns)
- [tap-dance fixture](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/tests/tap-dance/behavior_keymap.dtsi)
- [mock scanner implementation](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/module/drivers/kscan/kscan_mock.c) and [event macros](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/module/include/dt-bindings/zmk/kscan_mock.h)
- [soft-off behavior](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/src/behaviors/behavior_soft_off.c), [Kconfig](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/Kconfig.behaviors), [app CMake](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/CMakeLists.txt), [HID listener](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/src/hid_listener.c), and [physical layout compatibility path](https://github.com/zmkfirmware/zmk/blob/v0.3.0/app/src/physical_layouts.c)

Vendor sources read at `main`: [shield/matrix](https://github.com/a741725193/zmk-new_corne/blob/main/boards/shields/eyelash_corne/eyelash_corne.dtsi)
(blob `e412ee06911363e6b0b5974d7a95ad4f65f4f480`) and
[physical layout](https://github.com/a741725193/zmk-new_corne/blob/main/boards/shields/eyelash_corne/eyelash_corne-layouts.dtsi)
(blob `316630a3e490f76f2d4107a928d4eecca5ad36af`).
