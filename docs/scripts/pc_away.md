# pc_away.py — the sitting's batch P without the user

`scripts/lab/pc_away.py` runs the sitting's batch P (#609, #604, PR #651) with the user absent: the PC asleep and awake, Foxglove quit, and the keyboard going dark on its own, with no real display-off (on this PC that starts Modern Standby, so the probes that did it are retired, #651). The build under test runs on a temp `FOXGLOVE_CONFIG_DIR` with the away test hook (`FOXGLOVE_AWAY_OVERRIDE`): a watched file stands in for Windows' display notification, so everything after the edge is the app's real path (`pc_state.rs` telling the left half, which tells the right). The keyboard is read with `foxglove-lab`: `rgb-watch` (each D7 83 event: both halves' lit, wanted, idle) and `pc-state` (D7 11 read: held, reason). It checks that on connect the app says awake and holds the lease (PA1), that display off/on edges turn both halves dark/lit within `--edge-s` and the right half's art moves (PA2), that a lock alone tells the keyboard nothing while an unlock wakes it, that the last PC state before the quit is awake (PA3), that after the quit the keyboard goes dark within `--dark-within-s` as the lease ends (PA4), and that the user's Foxglove runs again (P6).

## How to run

```
FOXGLOVE_APP_OK=1 python scripts/lab/pc_away.py --app <foxglove.exe> [--lab <foxglove-lab.exe>] [--history]
```

Needs both halves on firmware with PC_STATE (HELLO bit 26) and the left on USB. It launches the app (`FOXGLOVE_APP_OK=1`, On-call), requires the user to be idle, and the keyboard must stay untouched about 13 minutes (the user away 2 min first; a key pressed meanwhile makes the dark step BLOCKED, not FAIL).

## Flags

| Flag | Default | Description |
|---|---|---|
| `--app` (required) | — | Path to the Foxglove app executable. |
| `--lab` | `<TARGET>/release/foxglove-lab.exe` | Path to `foxglove-lab.exe`. |
| `--edge-s <seconds>` | 2.0 | Dark or lit within this after the edge. |
| `--dark-within-s <seconds>` | 700 | After the quit, dark within this. |
| `--segment-s <seconds>` | 120 | The `rgb-watch` segments between `pc-state` reads. |
| `--history` | off | Also read the user's foxglove.log for real display edges. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | PASS |
| 1 | FAIL |
| 2 | BLOCKED (for example, a key was pressed during the dark step) |

## Files read and written

- **Read:** `foxglove-lab` `status`, `rgb-watch`, `pc-state`, and `run` output; the test app's log in the temp config dir; `lab-reports/pc-away-right-<timestamp>/report.json` (the right-moves lab case); with `--history`, the user's own `foxglove.log` (the real display edges of normal nights, measured, never caused).
- **Written:** a temp `FOXGLOVE_CONFIG_DIR` and the `away.txt` hook file next to it (written `display off` / `display on` / `locked` / `unlocked` to drive the edges); `lab-reports/pc-away-right-<timestamp>.toml` (the right-moves lab suite) and its output directory. If both halves' underglow was off by the user's choice, it is turned on through the API for the run and put back at the end.

## Example

```
FOXGLOVE_APP_OK=1 python scripts/lab/pc_away.py --app "C:\foxglove\foxglove.exe" --history
```

That prints the last 5 real display edges from the user's `foxglove.log`, then runs PA1 through the dark step and the restart check, and exits 0 (PASS), 1 (FAIL), or 2 (BLOCKED).
