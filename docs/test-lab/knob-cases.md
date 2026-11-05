# Lab cases: the knob (#719)

The PC's hold of the keymap's `&knob` / `&knob_press` (KNOB, D7 14 / D7 94, HELLO bit 31,
#720) and the knob's part of the self-report (D7 13, block version 2, bytes 96-103), proven on
the keyboard after a batch flash. Code: `crates/foxglove-lab/src/knob.rs`.

## Run

- `foxglove-lab health` runs them after the lights cases, on a direct read (the app not
  running). Their rows are in the group `knob`. No bit 31 or no bit 28 is a
  `WARN knob Not tested: ...` row, so a build without the knob still passes; a failed case fails
  health (exit 1). `--no-knob` leaves them out. While the app runs, health asks the app and the
  knob is one `Not tested` warning.
- `foxglove-lab knob [--json]` runs them alone. Exit 0 pass, 1 a case failed, 2 blocked: no bit
  31, no keyboard, or the app running (never beside the app, `--allow-app` or not: it holds the
  knob after each HELLO and would take the case's hold). A failure outranks a block.

The set takes well under a second. Each request waits at most 500 ms. The knob is on an app only
for one self-report read. The release runs at the end whatever happened (a drop guard). A lab
killed mid-run leaves the hold until the next USB disconnect or endpoint change, or until the
user's Foxglove holds it again on its next HELLO.

## Cases

Every KNOB reply is D7 94 00: status, held, target, the keymap's parts. A knob event in its place
(D7 94 01: a hand on the knob) is skipped and the hold read again, at most 3 times.

| Row | Does | Pass |
|---|---|---|
| Read | D7 14 FF | status 0, a part named; held at the start (no app running) is a WARN: a hold a dead app left, handed back at the end |
| Hold | D7 14 01 00 | held, on master |
| Target app | D7 14 01 01 | held, on an app |
| Health while held | the left's block | a knob part; its parts as the reply's; held, on an app |
| Back to master | D7 14 01 00 | held, on master |
| Bad request refused | D7 14 02 00 | status 5; still held, on master |
| Release | D7 14 00 00, then D7 14 FF | not held in the reply and in the fresh read |
| Health row | the left's block | not held; no hold dropped since the first read (a PC's release isn't a drop); the health view's `Knob` row, as it reads |

The `HUMAN knob` line (`WARN knob Not tested: by hand: ...` in health) is what the lab can't do:
turning the knob and its click, double click and hold. They belong to a sitting row.

Unit tests (`cargo test -p foxglove-lab knob`) run every row against a fake keyboard that behaves
as host_link.c's `knob_command` and health.c do, with knobs for the pass, fail and blocked paths.

## Not here

- Turns, clicks and holds, and what Windows does with them: a person (the human line).
- The firmware's click, double click and hold timing, held vs fallback: the zmk-sim `knob` case
  (tests/zmk-sim/knob).
- The app's sessions, its target cycle and the popup: the app's own tests.
