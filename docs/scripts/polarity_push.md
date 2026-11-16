# polarity_push.py — push checks on hardware (#666)

`scripts/lab/polarity_push.py` runs the #666 push checks on hardware (On-call). With the app under test on a temp config (`foxglove-polarity-push-` prefix, via `labkit`'s `App` and `temp_config`), it pushes the light-background and dark-background versions of one drawing as encoded files (`tests/art/ink-fixture.bin`, `tests/art/dark-fixture.bin`, the dark one stored as is) and checks that a pushed piece flips with the panel (Light, then back to Dark), that the light- and dark-background files look the same on the right (auto normalises the dark one), and that `ink-fixture.bin` with Ink: Invert is the inverse. Panel polarity is set on the left's host link over HID (D7 12, 0 dark, 1 light) and read back from the D7 92 reply. Captures are the right's readback through `foxglove-lab` (`capture_screen`) with the status strip masked; each check takes the best pair over 4 captures and passes at 98% or more, so a capture mid-redraw doesn't fail it.

## How to run

    python scripts/lab/polarity_push.py --app <foxglove.exe> --lab <foxglove-lab.exe> --out <dir> [--serial <left's>]

## Flags

| Flag | Default | Description |
|---|---|---|
| `--app` (required) | — | The Foxglove app executable; the value is passed through `labkit`'s `tool_arg`. |
| `--lab` (required) | — | The `foxglove-lab.exe` executable; the value is passed through `labkit`'s `tool_arg`. |
| `--out` (required) | — | Output directory for the lab case runs; created if missing. |
| `--serial` | `''` | The left's USB serial, when more than one host-link device is attached. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | PASS — all three checks passed; prints `RESULT PASS`. |
| 1 | FAIL — one or more checks failed; prints `RESULT FAIL`. |
| 2 | BLOCKED — no keyboard: `devices.list` returned nothing after up to 40 × 0.5 s of waiting; prints `RESULT BLOCKED: no keyboard`. |

## Files read and written

- **Read:** `tests/art/ink-fixture.bin` and `tests/art/dark-fixture.bin` (pushed to the keyboard via `art.select`); the left's host-link HID device (vendor 0x1D50, product 0x615E, usage page 0xFF60, usage 0x61, filtered by `--serial` when set); `<out>/<tag>/report.json` and the `.png` attachments it lists (written by `foxglove-lab`).
- **Written:** the `--out` directory; a per-capture suite file `<out>/<tag>.toml` and case output directory `<out>/<tag>` each (created by the `foxglove-lab` subprocess, run as `--allow-app run --suite <suite> --non-interactive --out <out>/<tag>` with a 120 s timeout); the D7 12 polarity writes to the host-link device; art pushes through the app's local API (`art.status`, `art.select` for the light file, the dark file, the light file with `polarity: invert`, then back to the built-in if `art.status` reported one); and to stdout: `art.status before:`, the push results, one `PASS`/`FAIL` line per check, and the final `RESULT PASS`/`RESULT FAIL`.

## Example

    python scripts/lab/polarity_push.py --app "C:\foxglove\foxglove.exe" --lab "C:\foxglove\foxglove-lab.exe" --out "C:\lab\polarity-push"

This pushes the two fixtures in turn, captures the right at each panel polarity and for the inverted light file, and exits 0 only if all three checks reach 98%.
