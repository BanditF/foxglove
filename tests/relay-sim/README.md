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
  every 7.5 ms, with ZMK's slave latency 30: the right listens right after an event that carried
  data, when it has data of its own, and at least every 31st event (#662). The right half is a model of `art_relay_peripheral.c`'s service, with Zephyr's
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
| `stall` | `wake`, but at the second tap the left's TX stalls: nothing it sends goes on the air until the link drops, while the right's PDUs still arrive. HELLO's relay debug then reads "timeouts 1, in flight 1" with the bearer alive, as in the #618 1ebdf9d soak, the 5568e5a gate and the 6a2632f gate | the relay comes back on its own within 45 s, then the pushes, SELECT and snapshot work |
| `stall-app` | `stall` with the app's traffic | as `stall` |
| `late` | one request answered 4.5 s late (past the 3 s GATT timeout), user commands meanwhile | the SELECT, a push and a snapshot right after the timeout work |
| `late-ignored` | a status read answered 4.5 s late with an ATT error; a SELECT goes while it is out | the left let it go (timeouts 1, in flight 1), the SELECT completes with its own answer (a left that took the late error for it fails here), nothing is held afterwards, a push works, and the link never dropped |
| `select-snap` | right-art-select-golden with the app running (#662): select set 0, a snapshot, set 1 (a kept slot since #671), a snapshot; the right's ATT server follows Zephyr 3.5's response-buffer rule and its system queue is busy 20 ms after each redraw | both snapshots OK within the 5 s budget, no request left unanswered, no re-link |
| `snap-latency` | the snapshot under the link's real timing (#662's d265853 hardware failure): latency 30, the response-buffer rule, the right's system queue busy 20 ms after each redraw and 20 ms on each art frame (10 a second), the app's traffic; select 0, snapshot, select 1, snapshot, two more snapshots while it animates, two art pushes, a SELECT | every snapshot OK and the slowest under 2 s (40% of the 5 s budget), nothing unanswered, no re-link; prints each snapshot's and push's simulated time |
| `snap-latency-old` (diagnostic) | `snap-latency` against a right before the push: it refuses the push byte, the left reads | not gating: the reads time out at the 5 s budget (the reply still comes inside the lab's 6 s) |
| `polarity-relink` | the panel polarity (#666): the left's value on link-up, a flip, a burst of flips with a SELECT among them (select-snap's response-buffer rule and busy right, the app's traffic), a flip while the link is down, a re-link with no change | the right has the newest value each time and the left reports it acknowledged (unknown while the link is down); one write per re-link, then quiet; no request unanswered, no re-link |
| `playlist-store` | the art playlist (#522) with two places lent to it, under `snap-latency`'s link (latency 30, the response-buffer rule, the right's busy system queue, the app's traffic): STORE (D6 09) stages a piece into a place ahead, then five steps, each a SELECT of the staged place and the next piece STOREd into the other place; then a STORE into the place on screen, one after it, and a SLOT read back | every STORE lands (the place holds those bytes, name and length) with the screen untouched (same set, no push played), slowest stage under 4 s and slowest step under 1.5 s, the store into the shown place refused with the screen and link left alone, the place reads back, no re-link, nothing unanswered; prints each stage's and step's simulated time and the places' write counts |
| `playlist-store-old` | `playlist-store`'s STORE against a right before it (status version 9) | the left fails it with error 7 (right rejected) before sending anything, within 1 s, the screen untouched; the keep path that shows the piece (push, SAVE) works, as Foxglove falls back to there |
| `restore-update` | art across a firmware update (#521): three places kept with STORE, then the right restarts with new firmware twice: one update keeps the places, one wipes them (a settings reset); Foxglove STOREs them back, and the link drops in the middle of the second | the places survive the first update (SLOT reads, used bits); after the wipe the first pass restores 2/3, the retry once the relay is back 3/3 (`art restored N/M`, from the reads back), the screen stays on the right's own art throughout, nothing unanswered |
| `health` | the self-report (#705, `health.c` built with the relay): the right switched on 70 s after the left; the right's block read (D7 13 half 1) under the response-buffer rule and a busy right; a status read answered 4.5 s late; a push the right refuses, then one it takes; a right without the block; a re-link | the relay self-test reads pending, then failed with no link (code 2), then failed late (code 1); the right's block arrives within 5 s with nothing unanswered and no re-link; the left counts the timeout, the failed push (last error: the right refused) and the good one, the right's block counts its refusal and its blob; a right without the block answers 4 and counts no refusal; the re-link counts a drop and a second find. `stall` also checks the timeouts, unstick, drop and re-find since boot |
| `art-step-toast` | the on-board art step's toast (#852): the right holds its built-in and kept places 0 and 2 (step list: sets 0, 1, 3); seven `art_relay_step` presses (as `&art_next` / `&art_prev` without the app holding the keys), forward, back and round the end, the last three with the app's traffic; then a press while a SELECT is still going, and one with the link down | every press that steps shows the toast (`art_toast_show`, stubbed in `sim_stubs.c`) with its place and the list's length (n/3), the way it stepped and built-in or kept, and the right lands on the piece it names; the busy and the no-link press step nothing and show no toast |
| `lost` (diagnostic) | one request never answered | not gating: the SELECT queued behind it can't be answered either; the push after the unstick's re-link works |

## How the stall is handled (art_relay_central.c, #618)

`stall` and `stall-app` were known gaps that CI required to fail: one request that never got its
answer kept a single in-flight flag set, and every relay command after it failed until a
re-link. Now:

- **Request slots.** Each request's params live in one of two slots, which Zephyr owns from the
  call until the request's callback. A request with no callback after 3 s is let go and the
  relay goes on with the other slot. The late callback only frees its slot (nothing is copied,
  nothing woken), so it can never be taken for the next request's answer (`late-ignored`). Only
  while both slots are held does a request get -EBUSY. HELLO's byte 6 counts the held slots
  (0-2).
- **Unstick.** A let-go request (or screen readback) that is still unanswered 10 s after it went
  out means nothing the left sends gets through. The left then drops the split link once, as
  the #122 guard does, from its own small queue (started on first use). Zephyr ends every
  request, ZMK reconnects and the relay is back after discovery (about 19 s in `stall`). At most
  3 drops in a row until a request is answered in time again. It starts at the let-go and stops
  at the late callback or when the link goes.
- **Paced write commands.** At most 3 of the relay's write commands (art data, frames, effects)
  wait for the radio at a time; Zephyr's complete callback counts them down. So they never take
  every ATT buffer and TX context from the requests and from ZMK. A command that finds no room
  for 1 s fails instead of sleeping inside Zephyr.

`sim_bt.c` models `bt_gatt_write_without_response_cb` as Zephyr 3.5 has it: the callback runs on
the system work queue once the command is sent, and is dropped for a command still queued when
the link goes. `sim_bt_faults.late_err` gives a late answer an ATT error.

- **Spaced requests (#662).** No request goes to the right sooner than 25 ms after the right's
  last answer (`REQ_GAP_MS`), and the screen readback reads one chunk per request from relay_q
  instead of chaining Read Blobs from the RX callback. Zephyr 3.5's ATT server on the right holds
  its single response buffer until the sent callback has run on the right's system queue; a
  request arriving before that gets no response at all, and the left's bearer then waits on it
  until a re-link. The 5a50426 gate showed exactly that after a SELECT: "right half timed out",
  "still busy after retries", then the unstick's link drop. The readback's budget is 5 s
  (`SCREEN_BUDGET_MS`), and a readback let go at its deadline arms the unstick from that
  request's own send time, so a slow but answered read never drops the link.

- **Screen push (#662, d265853's hardware failure).** On hardware every right snapshot timed out
  on d265853, with no relay timeout. The split link has ZMK's slave latency (30 x 7.5 ms): the
  right listens only after an event that carried data, when it has data, or every 31st event.
  The spaced Read Blob requests each found it asleep and waited up to ~230 ms; ~63 of them ran
  past the 5 s budget. relay-sim didn't model latency, so `select-snap` passed. Now it does
  (`snap-latency` fails on d265853: 5.1 s, timed out, twice per snapshot), and the right pushes
  the image: the SNAP write carries `ART_RELAY_SNAP_PUSH`, the right sends the image as
  notifications of the status value (`ART_RELAY_RANG_SCREEN`, offset u16, up to 17 bytes) once
  captured, two unsent at a time, and the left reads only what didn't come, spaced as before.
  Notifications take no response buffer, and a right with data to send skips its latency:
  0.4-1.2 s per snapshot in `snap-latency`. A right before the push refuses the two-byte SNAP
  (ATT 0x0d); the left then snaps and reads as before for that link (`snap-latency-old`). Art
  pushes under latency: 3599 bytes in ~0.9 s from COMMIT, 3190 in ~1.1 s; a request after a
  request (SELECT's status read) waits for the right's next listen, 0.3-0.8 s per SELECT.

## STORE (#522, #521)

The right's flash places (`art_slots.c`, 8 of 20 KiB at 0xAD000) took a piece only with SAVE,
which saves the set on screen: keeping a piece meant showing it first. STORE (relay op 19, D6 09
from the PC, right status version 10, HELLO bit 30) ends a push instead of COMMIT: the right
checks the set it received and writes it into a place without showing it, so the playlist can
stage its next piece ahead of the step and a firmware update's lost places come back without the
screen flickering through them. The fake right models SAVE, STORE and the places (each write
takes `flash_write_us`, 650 ms by default, on the right's own queue; status SAVING meanwhile, a
BEGIN or SELECT then gets 0x82), and `sim_bt_right_update` restarts it with new firmware.

## Slot CRCs (#863)

D6 87 carries the CRC each place keeps for its set (crc_version 1 at byte 27, the CRC after it),
so the app matches kept places to pieces and to each other by content. The right's slot CRC
characteristic sits right after the screen one (value handle screen + 2), and the left reads it
only from a right with status version 11, so discovery still ends at the screen. The fake right
has it at version 11 and answers a read of that handle with an invalid-handle error below it.
`playlist-store` reads a staged place's CRC and an empty place's 0; `playlist-store-old` (a right
before STORE) reads its place back with version 0 and no CRC read on the link.

## Known gaps (run in CI, must still fail)

None at the moment. `run.sh` and `build.ps1` still take a list (`KNOWN_GAPS`,
`$RelaySimKnownGaps`) and fail if a listed gap starts passing, so a new gap is documented the
same way.

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
