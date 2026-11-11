# Lab cases: the PC's lights (#492)

Per-LED frames (SET_FRAME, D7 0C / D7 8C, HELLO bit 14, #272/#530) and the firmware's own
effects (SET_EFFECT, D7 0F / D7 8F, HELLO bit 19, #490/#541/#553), proven end to end on the
keyboard after a batch flash. Code: `crates/foxglove-lab/src/host_lights.rs`.

## Run

- `foxglove-lab health` runs the quick set after the self-report (D7 13), on a direct read
  (the app not running). Its rows are in the group `lights`. A set the firmware can't do (no bit
  14 or 19; SET_EFFECT is in dev builds only) is a `WARN lights Not tested: ...` row there, so a
  release build still passes; a failed case fails health (exit 1). `--no-lights` leaves them out.
  While the app runs, health asks the app and the lights are one `Not tested` warning.
- `foxglove-lab lights [--watchdog] [--stream N] [--json]` runs them alone. Exit 0 pass, 1 a case
  failed, 2 blocked: a bit missing, no keyboard, or the app running (never beside the app,
  `--allow-app` or not: its scenes would take the lights mid-case). A failure outranks a block.
  `--watchdog` adds the frame timeout case (about 30 s on the shipped firmware).

The quick set takes about 2 s. Each request waits at most 500 ms; the stream sends a fixed count
(30); the watchdog waits the reply's timeout plus 5 s. Both releases run at the end whatever
happened; a lab killed mid-run leaves the lights to the keyboard's own timeout.

## Cases

Each reply is checked for status 0 and owner 3 (the PC's). With HELLO bit 24 (RGB_SHOWN) the
left's LEDs are read back from RGB_STATE (D7 83) about 150 ms later: the PC-drawn flag, and the
brightest LED's hue (within 8 degrees, saturation 90+) while lit. Dark (idle, no ext power) is a
pass with "not confirmed"; no bit 24 is one `WARN Read-back` row and the replies decide.

| Row | Does | Pass |
|---|---|---|
| Frame | every LED red (LAMP_PARTS count, else 42) | status 0, owner 3, the right draws it (no link: WARN; can't: FAIL) |
| Frame read-back | RGB_STATE | PC-drawn, red |
| Lost last chunk | a green frame's reports without the SHOW one, then a whole blue frame | the blue frame is the PC's, shows blue |
| Frame stream | 30 frames one in flight, red/blue | every reply ok and owner 3; WARN past 400 ms a round trip |
| Frame release | RELEASE | owner 0; LEDs no longer PC-drawn |
| Watchdog (`--watchdog`) | one frame, then nothing | D7 8C event (byte 6 = 1) with owner 0, not before the timeout less 1 s, within timeout + 5 s; LEDs the keyboard's; timeout 0: WARN |
| Effect | a still red breathe | status 0, owner 3, kind 4, the right draws it; read-back red |
| Bad effect refused | the same with kind 0 | status 5; the breathe runs on (owner 3, kind 4) |
| Frame over effect | a blue frame over the breathe, then SET_EFFECT RELEASE | frame owner 3, blue; the effect no longer runs; frame RELEASE owner 0 |
| Effect again / Effect release | a blue breathe, then RELEASE | runs, blue; then owner 0, kind 0, LEDs the keyboard's |

Unit tests (`cargo test -p foxglove-lab host_lights`) run every row against a fake keyboard
that behaves as host_frames.c and host_effects.c do, with knobs for the pass, fail and blocked
paths.

## Key rings and the halves' phase (#490, after the batched flash)

Needs dev firmware on **both** halves (the right's relay status version 13; HELLO v2 word 2 bit 5,
capability 37 KEY_RINGS, on the left). Not automated in `foxglove-lab lights` yet (follow-up on
#490): until then On-call runs it by hand, with the user away (the key presses below go through
the D7 05 test hook and so reach the desktop; the two `&mo` thumbs type nothing).

| Step | Does | Pass |
|---|---|---|
| Rings sent | SET_EFFECT kind 5: ring red (FF0000), background dim blue (000010), period 3000 ms, width 30 mm (`D7 0F 02 05 02 00 00 00 B8 0B 00 00 00 2C 01 00 00 00 00 FF 00 00 00 00 10`) | D7 8F status 0, owner 3, kind 5, right 1 (2: the right's firmware is older, FAIL) |
| Background | RGB_STATE (D7 83) | PC-drawn, the brightest LED blue; both halves dim blue by eye |
| Left key | D7 05 43 1, then 43 0 (the left middle thumb, `&mo 1`); RGB_STATE 300 ms later | the brightest LED red; by eye a red ring grows from the left thumb over the left half only, about 3 s, then dim blue again |
| Right key | D7 05 46 1, then 46 0 (the right middle thumb, `&mo 2`, relayed to the right) | by eye the ring grows over the right half only, from its thumb; the left's RGB_STATE stays blue (each half rings only its own keys) |
| App dead | quit Foxglove, type on both halves | rings from the keys typed, on the half typed on; typing unaffected (#25) |
| Phase | SET_EFFECT a scanline (kind 1, period 2000 ms, angle 0, width 30 mm); film both halves at 120 fps or more; again after a wake from idle and after switching the right off and on | the band leaves the left half and enters the right with no jump: the right's band within one frame (33 ms, about 5 mm at this speed) of where the left's predicts it |
| Release | SET_EFFECT RELEASE (`D7 0F 00`) | owner 0, kind 0; both halves back to their own effect |

The relay side of the phase (each send, each wake's re-anchor and each re-link land the right
within one frame of the left under the link's latency) is relay-sim's `effect-phase` and
`effect-relink`; the rings themselves are zmk-sim's `key-ripple`.

## Not here

- Which key each LED sits under, and colours by eye: needs a person. That is #90 row G (the
  LampArray sitting, `foxglove-lab lamp` / `frame-dot`).
- The right half's 10 fps coalescing and its own drawing: the zmk-sim `host-frames` case
  (tests/zmk-sim/host-frames). The right's read-back (its RGB_STATE half) is as of its last
  report, so the cases judge the left's LEDs and the right only by the reply's right byte.
- The app's frame rate cap: the app's own tests.
- Typing while frames stream: the resilience suite's typing cases (they take focus, so On-call
  runs them with the user away).
