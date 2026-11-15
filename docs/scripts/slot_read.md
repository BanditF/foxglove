# slot_read.py — SLOT_READ flash-place readback test

`scripts/lab/slot_read.py` runs the SLOT_READ test (hardware ticket #911; readback tickets #878/#908). It borrows a KEPT flash place on the connected keyboard, backs up its original bytes to disk, writes an ink-fixture art piece into that place, checks that a live-feed client receives frames and that `art/kept-<crc>.bin` lands in the config folder with a matching CRC, then restores the original bytes and verifies byte-identical recovery. On any failure or exception it still tries to restore (the restore step is retried up to 3 times). If the restore cannot be verified it prints the backup file path and manual restore steps, and exits 1.

## How to run

```
FOXGLOVE_APP_OK=1 python scripts/lab/slot_read.py --app <foxglove.exe> [--lab <foxglove-lab.exe>] [--slot N] [--dry-run]
```

**On-call only.** Outside `--dry-run` this launches Foxglove and rewrites a real flash place on the connected keyboard. Run it only with `FOXGLOVE_APP_OK=1`, which On-call sets once the Hub agrees; the session guard hook (`scripts/hooks/guard.py`) refuses the command without it, and the lab cases exit 2 (BLOCKED) when the app may not be launched. Note that `slot_read.py` itself does not read the variable. The test app runs on a temporary FOXGLOVE_CONFIG_DIR with an empty art library and the live feed enabled on port 8768.

## Flags

| Flag | Default | Description |
|---|---|---|
| `--app` (required) | — | Path to the Foxglove app executable. |
| `--lab` | `<TARGET>/release/foxglove-lab.exe` | Path to `foxglove-lab.exe`. |
| `--within <seconds>` | 60.0 | Seconds allowed for frames to arrive from the feed client. |
| `--slot <N>` | First kept flash place | The KEPT flash place to borrow. |
| `--dry-run` | off | Only back up the chosen place and verify it reads back identical; nothing is written to the keyboard or app launched. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | PASS — all checks passed, original bytes restored and verified. |
| 1 | FAIL — one or more checks failed, or restoration could not be verified (manual restore steps are printed). |
| 2 | BLOCKED — no kept flash place available, the device right is not connected, or a lab step was blocked (for example the app may not be launched without the OK). |

## Files read and written

- **Read:** `tests/art/ink-fixture.bin` (the test fixture), `tests/hil/slot-read.toml` (lab case suite).
- **Written:** `<temp>/foxglove-slot-backups/<timestamp>/slot<slot>.bin` and `.json` (backup of the borrowed place); `<temp>/foxglove-slot-backups/<timestamp>/pick.list.json` (list of kept places, if auto-selecting a slot); `art/kept-<crc:08x>.bin` in the app's config folder (the piece received from the live feed). Backups are never deleted by this script.

## Example

```
FOXGLOVE_APP_OK=1 python scripts/lab/slot_read.py --app "C:\foxglove\foxglove.exe" --slot 3 --dry-run
```

This backs up flash place 3, verifies it reads back identical to the backup on disk, and exits without writing anything to the keyboard.