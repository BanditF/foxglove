# overlay_tray.py — app-level stability cases for the overlay, tray panel, and agent controls

`scripts/lab/overlay_tray.py` runs the app-level cases of the stability pass (#618) for the overlay, the tray panel, the agent controls and the re-apply hooks, read back through the local API (API 1.6) and the test app's log. It launches the build under test on a temp `FOXGLOVE_CONFIG_DIR` (the user's Foxglove is stopped by its exact name and started again by its path), then checks the tray click opening the panel (#612/#620), the restore after a posted display change, the agent controls (`tray.set_panel` open/close, `overlay.restore`, the tray's Next › and ‹ through `art.select`), the re-apply after a display change and after relay-ready (#601/#622), and the overlay's click-through (#549, #405/#406). With `--focus` it also runs the cases that move the focus (Settings shown by a second launch, minimized, the panel closing on focus loss), and with `--real-clicks` the sitting's I1-I4 with real mouse input through SendInput. The keyboard cases need the keyboard (BLOCKED without it) and the split-link drop case a dev build.

## How to run

```
FOXGLOVE_APP_OK=1 python scripts/lab/overlay_tray.py --app <foxglove.exe> [--focus] [--real-clicks]
```

**On-call only.** It launches the app under test, so it runs only with `FOXGLOVE_APP_OK=1`. `--focus` and `--real-clicks` additionally require the user to be idle for 120 s (the PC unlocked and the user away).

## Flags

| Flag | Default | Description |
|---|---|---|
| `--app` (required) | — | The Foxglove build under test. |
| `--lab` | `<TARGET>/release/foxglove-lab.exe` | Path to `foxglove-lab.exe`. |
| `--skip-guard` | off | Leave out the split-link drop (GA build). |
| `--focus` | off | Also the cases that move the focus (Settings shown by a second launch): PC unlocked, user away. |
| `--real-clicks` | off | The sitting's I1-I4 with real mouse clicks (SendInput) on lab probe windows: PC unlocked, user away. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | PASS |
| 1 | FAIL |
| 2 | BLOCKED |

## Files read and written

- **Read:** `--app` (the build under test); `app/resources/lighting/art/fury.bin` (shown first through `art.select`); `app/resources/lighting/art/hearth.bin` (the blob `foxglove-lab` pushes behind the app's back); the test app's log (checked for the restore, re-apply and api lines); the user's store (the remembered keyboard, seeded into the temp store); `lab-reports/overlay-tray-<case>-<ns>/report.json` after each `foxglove-lab` case.
- **Written:** the temp config dir for the test app (`foxglove-overlay-tray-` prefix), and `lab-reports/overlay-tray-<case>-<ns>/` for the two lab cases `lose-art` and `relink` (the generated `.toml` case suite plus the `foxglove-lab` report).

## Example

```
FOXGLOVE_APP_OK=1 python scripts/lab/overlay_tray.py --app "C:\foxglove\foxglove.exe" --lab "C:\foxglove\foxglove-lab.exe"
```

This runs the tray click, display change, agent controls, re-apply and click-through cases against the named build, then exits 0 (PASS), 1 (FAIL) or 2 (BLOCKED).
