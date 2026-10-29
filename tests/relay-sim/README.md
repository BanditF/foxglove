# relay-sim: the left's art relay on a host

The real `boards/shields/nice_view_custom/widgets/art_relay_central.c` (with
`screen_snap_relay.inc`), compiled for the PC and run against a fake split link. zmk-sim has no
split transport, so the relay isn't built there; this is where its timing and failure handling
get tested.

- `sim_kernel.c`: a deterministic kernel. Threads are coroutines with Zephyr priorities
  (negative = cooperative), virtual time moves only when every thread is blocked. Work queues,
  delayable work, semaphores and `k_msleep` behave as Zephyr's.
- `sim_bt.c`: the left's Bluetooth host as Zephyr 3.5 (v3.5.0+zmk-fixes) has it: one ATT request
  on the bearer and the rest queued; TX meta data and connection TX contexts (5 each with ZMK's
  central) taken with K_FOREVER off the system work queue and K_NO_WAIT on it; a 30 s ATT
  timeout that kills the bearer; Read Blob continuation from the RX thread. A connection event
  every 7.5 ms. The right half is a model of `art_relay_peripheral.c`'s service, with Zephyr's
  CCC rules for a bonded peer.
- `sim_stubs.c`: what the relay calls into (host_link, rgb_idle, screen_snap, ZMK activity).
- `main.c`: scenarios, driven the way foxglove-lab and the app drive the left over USB.

## Scenarios

| Scenario | What runs | Passes when |
| --- | --- | --- |
| `wake` | resilience's right-follows-left-activity (SELECT, tap, idle, snapshots, tap, snapshots), then two pushes, a SELECT and a snapshot; Foxglove quit | every step works |
| `wake-app` | the same with the app's background traffic (STATUS, RGB_STATE about once a second, D7 0E) | every step works |
| `relink-shown` | a re-link, then RGB_STATE (rgb-state-follows-the-leds) | the right's shown block is back after the re-link |
| `push-relink-tap` | the resilience suite's order: a push, a push with the link dropped mid-relay (r5a), then rgb-state-follows-the-leds' tap | RGB_STATE has the right's shown block (lit) within the step's 3 s |
| `late` (diagnostic) | one request answered 4.5 s late (past the 3 s GATT timeout), user commands meanwhile | not gating |
| `lost` (diagnostic) | one request never answered | not gating |

## Known gaps (run in CI, must still fail)

| Scenario | What runs | Would pass when |
| --- | --- | --- |
| `stall` | `wake`, but at the second tap the left's TX stalls: nothing it sends goes on the air until the link drops, while the right's PDUs still arrive. HELLO's relay debug then reads "timeouts 1, in flight 1" with the bearer alive, as in the #618 1ebdf9d soak and the 5568e5a gate | the relay comes back on its own within 45 s, then the pushes, SELECT and snapshot work |
| `stall-app` | `stall` with the app's traffic | as `stall` |

No firmware since 13ff04b gets out of that state without a re-link. A left-side watchdog (drop the
link once a timed-out request has had no answer for 10 s) passed both in this harness, but it was
held back from #647 to keep that hardware gate to rgb-state; current main shows timeouts 0 on
hardware. `run.sh` and `build.ps1` fail if a known gap starts passing, so this list stays true.

`wake-busy` (diagnostic) turns on Zephyr's response-buffer rule on the right with a 40 ms busy
system queue per redraw; the right's display has its own queue (Kconfig.defconfig), so it isn't
the default.

## Running

- Windows (MSVC Build Tools): `powershell -File tests/relay-sim/build.ps1` (the gate runs it).
- Linux (CI, `zmk-sim.yml` job `relay-sim`): `bash tests/relay-sim/run.sh`.
- Across firmware commits: `powershell -File tests/relay-sim/bisect.ps1 -Commits 13ff04b,HEAD`
  builds each commit's widgets directory against this harness and prints one RESULT line per
  scenario.

Each run prints PASS/FAIL lines, the relay debug bytes as HELLO reports them (chrcs, ends, last
ret, ATT error, timeouts, answered, in flight, security, MTU), and the fake link's counters.
