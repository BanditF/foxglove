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
| 18 | G / SYSTEM GAME toggle | 1, 5 |
| 29 | Z | 2, 1 |
| 37 | M / SYSTEM soft-off | 2, 7 |
| 41 | Escape | 2, 11 |
| 43 | left middle thumb | 3, 4 |
| 46 | right middle thumb | 3, 7 |

## Coverage and expected traces

`keycode_events.snapshot` files are the expected traces, compared strictly by CI
(`zmk-sim.yml`). Write a new one by hand from the case's intent, then check it against the
CI run's filtered output; never copy the output in unread.
The filtered output follows upstream's `hid_listener_keycode` → `kp` format.
The extra `SIM_SOFT_OFF` marker observes the power-off boundary.

| Case | Assertions |
| --- | --- |
| `base` | Tab Q A Z on left; Y P H M Enter on right |
| `thumbs` | Both press orders × both release orders; position 1 gives N1 on NUMBER, shifted N1 (`!`) on SYMBOL, F1 on FUNCTION, Q after release; Tab verifies base |
| `thumbs-game` | Same four sequences after real SYSTEM+G `&tog 3` (FUNCTION, then Tab's `&mo 6`); Esc verifies GAME, Tab after toggling off; GAME supplies `&mo 4` on the left thumb in either order, so GAME+ plus SYMBOL activates FUNCTION |
| `combos` | The only current combo (12+41, default 50 ms) toggles GAME on/off in both press orders; 120 ms separation emits Backspace/Esc and leaves Tab unchanged |
| `soft-off` | M emits M on base, nothing for NUMBER RGB command, Plus on SYMBOL; SYSTEM+M 30/1900 ms emits no key and no power-off, 2100 ms calls power-off once; interleaved F1 taps distinguish these intervals. Q+S+Z pressed together emits Q/S/Z down and up, then Q still works |
| `sleep` | Deep sleep at the **production** idle timeout (`prod.keys` copies `CONFIG_ZMK_IDLE_SLEEP_TIMEOUT` from `config/eyelash_corne.conf`, #223): after one Q, idle at 30 s, still idle (not asleep) 29 min 50 s later, then sleep and `SIM_SLEEP` (the stubbed `sys_poweroff` ends the run). Simulated time runs unslowed; `module/sleep_sim.c` watches the clock, and there's no exit-after, so a board that never sleeps fails on the timeout |
| `rgb-state` | RGB_STATE's truth (#605, #624): the real `host_link.c` and `rgb_idle.c`, with ZMK's underglow, the strip and the relay faked (`module/rgb_state_sim.c`). The fake underglow draws every change into a fake strip the way `dim_strip.c` hands frames to `rgb_idle.c`; that strip is the truth. Each D7 83 read, parsed as the app does (`on` from the shown block, bytes 17-22), must match it, and the shown value must match the brightest LED: at start, idle (dark), wake, the PC's frames through idle (lit while ZMK's flag is off), a black frame (dark while the on/off is on), hand-back, a breathe at its darkest (still on), sync off and on. Each change's D7 83 event and its lit are pinned. Then #624: a look change (as after an invoke) puts the left's hue, saturation, brightness and effect in the RGB op the right is sent (a key that changes nothing sends nothing), and as the right, a half with an older saved look (hue 4, 9 %, Spectrum) takes the left's from that op, writing only what differs. #618: the left's side of the right's doorbell (`art_relay_doorbell.h`, as `art_relay_central.c` calls it): the ring that answers the subscribe on a new link comes before the first status read and its shown block is kept, so RGB_STATE reports the right's shown state once that read is in (before the fix it stayed empty until the right's LEDs next went lit or dark); a ring that changes nothing is no D7 83, a one-byte ring (a right before #605) takes nothing, the link going drops it. Then as the right following the left: the left idles while the PC's traffic runs (status reads, the PC's colour, the left's look, the RGB op again), the LEDs go dark and the shown block says dark and following (its idle bit is the right's own activity, idle all along: the left's keys don't count there), and a key on the left lights them again. Not simulated: the Bluetooth relay, the right's doorbell and a real re-link (HIL `rgb-state-follows-the-leds`, `right-relink-keeps-the-lefts-look`) |
| `activity-relay` | The left's activity relay (#603): the real `activity_relay.c` on ZMK's real activity, with the relay replaced by a print of the op the right would be sent (`module/activity_sim.c`, built by the board's own `art_relay_activity_op()`). Idle at 3 s and sleep at 20 s (`case.conf`): Q while active since boot sends nothing; idle sends `10 01`; Q wakes (`10 00`); Q while active sends nothing; idle; a cable going in, played as `usb_wake.c` does it (a call into ZMK's activity listener), sends `10 00`; idle; going to sleep sends `10 02` before the power-off (`SIM_SLEEP` ends the run). One message per change, none between. The Bluetooth send, the lane/request pair and the right's handling aren't simulated (no split transport here) |
| `battery-dim` | The real `dim_strip.c` and `battery_dim.c` (#222) in front of a fake strip with fake USB and charge readings (`module/dim_sim.c`), at the shield's shipped `NICE_VIEW_RGB_DIM_*` defaults: 80% before any event and on battery, 100% on USB, 12% below 20% charge and back to 80% only at 22% (hysteresis); switched off is 100% everywhere, a level of 0 is refused, the app's own levels apply; 255,128,10 / 100,0,200 scale by integer division with a lit channel kept at 1 or more, and the caller's buffer is never written (ZMK's frames pass `colour_effects.c` as Solid here) |
| `colour-swirl` | The colour Swirl (#626): ZMK v0.3.0's own Swirl (its HSB conversion and rainbow, 360/21 degrees between LEDs, speed × 2 a frame, faked in `module/colour_swirl_sim.c`) drawn through the real `dim_strip.c` and `colour_effects.c`, with the real `rgb_idle.c` and `host_link.c`, at the shield's shipped band and depth (`Kconfig.defconfig`). 60 frames each (one turn) of the user's amber (#d39f3f), red across 0, white, amber after RGB_HUI and amber on battery (80 %): every LED of every frame within the band of the chosen hue (+2 degrees of 8-bit rounding, +4 dimmed) while ZMK's own hues span the wheel; the band reaches both edges; a brightness wave; every frame moves; ZMK's buffer is never written; white stays white and still moves. Spectrum, Solid and Breathe pass through as ZMK drew them. HELLO bit 29, D7 83 byte 29 = 0x0B and its shown hue within the band. As the right: an older saved look takes the left's from the relay's RGB op (#624) and its 60 frames equal the left's LED for LED. The lighting health counters (`colour_effects.h`: effect, colour/recoloured/built flags, band, hue, widest offset, frames, last apply) before any frame, after the swirls, on Spectrum and Solid (no frame counted), and at the end. Not simulated: the WS2812s, the Bluetooth relay, two boards running at once (each half turns its own swirl, as before) |
| `panel-stream` | The plugin screens' left-panel stream (#238), played by `module/panel_stream_sim.c` against the real `host_link.c`: 10 whole frames (FILL, 18 ROWS, PRESENT) at 2 Hz with a HELLO and a CONN_STATE after each, then 30 frames back to back and a HELLO. Every request is answered (no other reply appears), the shown frame is the last one sent in each phase, and four Q taps meanwhile type normally. Simulated time stands still while code runs, so this checks ordering and loss, not CPU time |
| `host-frames` | SET_FRAME (#272) against the real `host_link.c` and `host_frames.c`, with the strip and the art relay faked in `module/frames_sim.c`: HELLO bits 14 and 22; LAMP_PARTS (D7 10, #561) answers the left's chain and the right's (2 parts of 21); a chunk without SHOW doesn't reply or draw; a whole frame takes both halves and draws the left; the right gets only the lamps that changed (one lamp, nothing for the same frame, a fill for one colour per half, the span from `art_relay.h`); a burst of 5 frames draws the left each time and sends the right the first and the newest only (100 ms gap); 10 lamps, lamps past 41 and a short report answer 5; RELEASE, a 3 s timeout and an endpoint change hand both halves back, the last two with an unsolicited `D7 8C` (event 1) |
| `host-zones` | A per-key scene on a zone board (#611): the SET_FRAME reports the app sends for four looks (a gradient each way, a sweep at two moments) on the Eyelash Corne as its profile ships (6 column zones per half, 15 lamps per half that light nothing, #608), from `app/src/foxglove/keyboard_zone_vectors.rs` (`module/zone_vectors.inc`), played through the real `host_link.c` and `host_frames.c` with `module/frames_sim.c`'s fake strip and relay (`module/zones_sim.c`). Both halves show every zone's lamp in its zone's colour (worked out from the profile's map: the look at the centre of the zone's keys) and every dead lamp black; RELEASE hands both halves back |
| `lamparray` | The HID LampArray (#88) as a transport onto SET_FRAME's core: the real `lamparray.c` (its six feature reports, no USB or BLE transport) on the real `host_frames.c` and `host_link.c`, with `module/frames_sim.c`'s fake strip and relay, Windows and Foxglove played by `module/lamparray_sim.c`. Before any host traffic a take-over and a hand-back draw but send no D7 8C; the attributes (42 lamps, SET_EFFECT's 328 x 86 mm room, Keyboard, 33 ms on USB, 100 ms on BLE); the lamp walk (each read moves on, the right's lamp 21 mirrored, ids past the end clamp, the walk wraps); an update before autonomous mode is off draws nothing; autonomous mode off takes both halves from black with `D7 8C 00 01 .. 01`; a multi update (intensity scales the colour) and a range update land on the left strip and the right's relay (only what changed); no LampUpdateComplete, no draw; Foxglove meanwhile: SHOW busy (1, owner 1), a chunk dropped, RELEASE answers 0 and leaves the lights; autonomous mode on hands back with owner 0, event 1, and again does nothing; BLE takes (owner 2), USB takes from it (the last host draws), BLE's autonomous on and an endpoint change leave USB's lights, an endpoint change hands BLE's back; Foxglove's frames (owner 3) give way to Windows; short, unknown and empty reports refused; the D7 90 tail (bytes 12-23) counts take-overs, hand-backs, frames drawn, Foxglove refusals and bad reports |
| `host-effects` | SET_EFFECT (#490) against the real `host_link.c`, `host_effects.c`, `effect_run.c`, `lamp_effect.c` and `host_frames.c`, with the real `rgb_idle.c`, and the strip, the art relay and ZMK's underglow faked in `module/effects_sim.c`: every vector `crates/scene` writes to `module/effect_vectors.inc` within 2 per channel; every Scenes-page direction (scanline and sweep, down/up/right/left) as the app sends it, from `app/src/foxglove/keyboard_direction_vectors.rs` (`module/direction_vectors.inc`), rendered on both halves with the firmware's lamp table and the right's mirror, each LED within 8 per channel of the app's colour; HELLO bit 19; PLACE refuses a half without area; a still effect takes both halves and draws once; a moving scanline draws a frame every 33 ms on the clock it was sent with, and keeps drawing through an endpoint change; PLACE moves it on the same clock; SET_FRAME takes over (the right told by the frames) and an effect takes back without a `D7 8C` event; bad kinds, colour counts, angles, lengths and ops answer 5; LAMPS (HELLO bit 21, #550) refuses a third half, LEDs past 20, values past 1000 and a short report, draws the left's LEDs where it puts them, hands the right its own points (not mirrored) with the effect, and count 0 puts a half back on the built-in table; RELEASE hands both halves back. The scene owns the lights (#542): it keeps drawing through idle (3 s here) and the user's off, RGB_SYNC's left on, the PC's D7 83 events and what the underglow relay tells the right follow the real on/off meanwhile, RELEASE puts that on/off back, and as the right it keeps drawing while the left's link drops and comes back saying off, then follows that off on RELEASE |
| `key-ripple` | SET_EFFECT's key rings (#490, kind 5, D5 88 capability 37): the host-effects case's real files and fakes with its own driver (`key_ripple_driver` in `module/effects_sim.c`), on a physical layout with the Eyelash Corne's key places (the case's keymap chooses it over the bare matrix transform, same positions), keys raised as ZMK's position events. Capability 37; where ZMK's layout puts Q, Backspace, the middle cluster and the two middle thumbs (turned 12 degrees) for a one-piece board, the left half and the right half (`lamp_effect_key_point`; a key past its side's keys held to the edge); kind 5 without a period or with three colours answers 5; kind 5 takes both halves and draws the background once, no frames without a key; Q starts a ring that lights LED 0 (the nearest, 2.8 mm) first, draws only while it grows, then the background and no more draws; the other half's key (source 0, as relayed) and a release alone start none; five keys at once (four rings, the fifth taking the oldest's place) end on the background; the same effect sent again keeps the ring in flight; no ring under a still breathe, under the PC's frames (SET_FRAME takes the lights) or after RELEASE. Not simulated: two boards at once (the right's side of the layout is checked through the function), the WS2812s |
| `pc-state` | The PC state (#609, #604): the real `pc_state.c`, `host_link.c` (D7 11 / D7 91), `rgb_idle.c` and `activity_relay.c` on ZMK's real activity, with ZMK's underglow, the cable and the relay to the right faked (`module/pc_state_sim.c`), at the shipped times: ZMK's 30 s idle, the shield's 600 s USB idle (read from `Kconfig.defconfig`) and the production 30-minute sleep (`prod.keys`). HELLO bit 26; on battery idle 30 s after a key, as before; plugged in without the app lit until 10 minutes after the last key (decision 2), then dark; a lease of 0 and an unknown state answer 5 and change nothing; awake with the wake lights it at once and holds it past 30 s; the ask (D7 91 event 1, lease 60) a minute before the end, answered with a renewal; away dark at once, also 5 s after a key; back lit at once (#604); a key ends away; the app gone: both asks (60, 20) unanswered, the lease lapses and the light lasts until 10 minutes after the last key; an unplug ends the hold at once. Each step's op to the right: the left's keys (`10 xx`, never the hold), the RGB op's on/off with the left's own (byte 13) and the PC state (`11 xx`). As the right (decision 3): held only on its own USB power, showing the left's own on/off; the left's away and awake follow at once; unplugged dark again. Then on battery ZMK's own sleep at 30 minutes (`10 02`, `SIM_SLEEP` ends the run). Not simulated: the key backlight (`backlight_idle.c`), the right's art and screen, the Bluetooth relay |
| `rgb-idle-race` | rgb_idle's key-then-away race (#651 follow-up, a key then Win+L): the pc-state case's real files and fakes with its own driver (`rgb_idle_race_sim_driver` in `module/pc_state_sim.c`). On USB with the PC awake, a real key (position 1) and then away at once, before the key's 50 ms check: the LEDs go dark and the user's on/off (the shown wanted bit) stays on, still on past the 10 s save delay, so nothing saves off; back lights them again. The user's own off and on still count, and the user's off just before away (away landing before its check) is still the user's: dark after back until the user's on. Only its own `race:` lines are pinned |
| `panel-polarity` | The panel polarity (#666): the real `panel_polarity.c` and `host_link.c` (D7 12 / D7 92), as the left with the relay, the drawers, the settings backend and the relay faked (`module/polarity_sim.c`). HELLO bit 27; the Kconfig default (dark); light from the PC redraws this half, sends the right `12 01` and saves once 2 s later; the same value again changes nothing; a bad value answers 5; a short report reads; two flips within the save delay write nothing; as the right the left's value is drawn and saved, a bad one ignored; a reboot's settings load (through `settings_runtime_set`, which calls the real handler as `settings_load` does) brings the stored value back and redraws, and a stored value of the wrong size or range is ignored. Not simulated: the panels themselves, the flash write, the Bluetooth relay (relay-sim `polarity-relink`) |
| `hostlink` | The host-link protocol against the real `host_link.c`, with `module/hostlink_sim.c` playing the PC: every request and reply is printed, so the snapshot pins the wire format and the refusal, gating and BOOTLOADER nonce rules |
| `hostlink-ga` | `hostlink` with the GA profile (`FOXGLOVE_TEST_HOOKS=n`, #81): POSITION and BOOTLOADER answer 4, HELLO bits 5 and 9 are clear, and INVOKE still refuses with 6 |; both also pin the Corne's art pace (#842): its right art target (`screens_corne.c`) is fast with no limit, so every rate plays at 1000 / fps as before, with the PC's e-paper animate off or on
| `art-epaper` | The firmware's own art pace on e-paper (#842): the `art-local` board with its art target e-paper (partial refresh, 1 s shortest frame, v2 descriptor) and the real `art_store.c`, `art_local.c`, `art_pace.c` and `host_link.c`. Word 2 has ART_PACE; built-in, pushed and kept (flash slot) sets show their first frame only until D6 0A ANIMATE 01, then play held to 1 s (8 fps and 4 fps both every 1000 ms); a still set stays still; a repeated ANIMATE re-times nothing; ANIMATE 00 stills the set on screen; ANIMATE during a push leaves the receive alone. The drawer is faked: it prints the pace it would play at |
| `art-keys` | `&art_next` / `&art_prev` (#60, `module/art_keys_sim.c`): to the board without a subscription, to the PC (D7 8D 01 dir) with one, and to the board again after D7 0D 00 or an endpoint change |
| `health` | The keyboard's self-report (#705): the real `health.c` and `host_link.c` (D7 13 / D7 93) as the left of a one-piece keyboard, with the Raw HID module's counters faked (`module/health_sim.c`, `module/include/raw_hid/stats.h`). HELLO bit 28; one block read in four chunks of one copy, parsed field by field (version, half, length, flags, build, the boot self-test: settings pass, display/relay/LEDs not applicable here, Raw HID sent); six requests while the system queue can't run: four replies come and two are dropped, and the next block counts the two; bad half and chunk answer 5, the right's chunk before its copy 6, the right without a relay 2, a report cut short reads chunk 0. The relay side, the right's block and the relay self-test are relay-sim's `health` |
| `knob` | The knob (#719): the real `knob.c` (`&knob`, `&knob_press` as the production keymap binds them on the base layer) and `host_link.c` (D7 14 KNOB / D7 94), with `module/knob_sim.c` playing the PC and the encoder (turns through the behaviour's sensor API, the click at position 34 through ZMK's keymap). HELLO bit 31; not held: the turns are the volume keys and the click is mute; held with master as the target the turns stay the keys, a click (after the 300 ms double-click time) and a hold (at 500 ms, before the release) are events, and a double click is an event plus mute tapped on the board; held on an app the turns are events, three detents one event, and no key goes; master volume (#855, D7 16 / D7 96, word 2's bit 35 in D5 88): held on master the turns are events (target 0) and a double click taps no mute, also with the knob handed back (its click is then mute again); handed back, master's turns are the keys; 2 answers 5; an endpoint change drops the hold with event 5, drops master volume with D7 96 01, and the keys are back; a bad request answers 5; handing back is the keys again. The USB unplug path (same listener, no event) isn't simulated (no USB here) |
| `hold-lease` | The host-held modes' lease (#847): the real `host_link.c` (D7 15 HOLD_LEASE / D7 95), `art_keys.c` and `knob.c` with the production keymap's `&art_next` and `&knob`, `module/hold_lease_sim.c` playing the PC, the art key (through the behaviour) and the encoder (its sensor API), simulated time unslowed. Word 2's bit 34; an app before #847 (no lease) keeps both holds through a minute of silence; a 30 s lease, 2 and 121 answer 5 and change nothing; a live app answering each ask (a third of the lease after its last report, D7 95 01) keeps both; other reports every 8 s renew with no ask; a killed app gets asks at 10 and 20 s and both holds lapse at 30 s (D7 95 02, mask 03): the art key steps on the board and the turn is the volume key; the PC's next report hears of the lapse again and the holds come back; nothing held asks nothing for 40 s; with the lease off (D7 15 00) a hold stays through 40 s of silence; master volume alone (#855, D7 16) sends master's turn to the PC, is asked for at 10 and 20 s and lapses at 30 s (mask 04), its turn then the volume key, the next report hears of it and it comes back; an endpoint change ends the holds and the lease. The USB unplug path (same listener) isn't simulated (no USB here) |
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
