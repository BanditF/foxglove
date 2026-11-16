# foxglove_flash.py — safely flash the nice!nano v2 halves

`scripts/flash/foxglove_flash.py` (Python 3.12, no dependencies) flashes the left and/or right half of a nice!nano v2 through its UF2 bootloader drive. It preflights each image (whole 512-byte blocks, nRF52840 family, addresses within 0x26000..0xAD000, no overlaps, and a half marker), then asks each half into its bootloader (hands-free via `foxglove-lab`, or by asking for a double-tap reset), waits for its drive, and writes the image to `flash.uf2`, reading the running build back to verify it. Real writes require an interactive terminal and typing `FLASH` (plus `ERASE SETTINGS` for a settings reset) unless `--yes` is given; then every automatic check still runs and a `foxglove-lab` typing check replaces the typed `TYPES`. A half left in its bootloader by an earlier run is written first; `--resume` writes only such halves and asks none in.

## How to run

```
python scripts/flash/foxglove_flash.py --firmware DIR --dry-run
python scripts/flash/foxglove_flash.py --left L.uf2 --right R.uf2
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--firmware DIR` | — | Folder with the UF2s; per half, `foxglove-fw-*-{half}-{profile}.uf2` when present, else the fixed names. |
| `--left FILE` / `--right FILE` | — | The UF2 for that half, in place of `--firmware`. |
| `--profile {dev,ga}` | `dev` | Which build in a `--firmware` folder with `foxglove-fw-*` names. |
| `--half {left,right,both}` | `both` | Which half to flash. |
| `--dry-run` | off | Preflight only; the flash step is skipped. |
| `--wait-seconds N` | `600` | Seconds to wait for a half's bootloader drive; must be positive and finite. |
| `--json` | off | Suppress the log lines and print one JSON report (`images`, `messages`, `exit_code`). |
| `--allow-unknown-half` | off | Accept an image whose half the markers and heuristics do not identify. |
| `--settings-reset` | off | Allow a SETTINGS RESET image (erases saved settings; needs a second confirmation). |
| `--force` | off | Flash a half even when it already runs the same build. |
| `--yes` | off | No prompts: the typed `FLASH`/`TYPES` are waived; every automatic check still runs, with a typing check in place of `TYPES`. |
| `--allow-profile-mix` | off | Allow leaving one half dev and the other GA. |
| `--pnp-rescan` | off | When a bootloader drive does not mount, run `pnputil /scan-devices` once before asking for a replug (needs an elevated prompt). |
| `--left-reboot-image FILE` | — | The left UF2 the left runs now (same SHA, checked); used to restart the left hands-free when the right must be asked in while the relay is down. |
| `--resume` | off | Write only the halves already in their bootloaders, each scoped to its USB serial, and never ask a half in. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The run finished (or there was nothing to flash). |
| 1 | Refused — a `Refused`, `NoDrive`, `OSError`, or `ValueError` (preflight, identity, lock, confirmation); also a run stopped by the user. |
| 2 | Timeout — a `TimeoutError` or a subprocess timeout; also a bad argument (argparse errors and `--wait-seconds` of 0 or less exit 2). |
| 3 | Flash failed — a `FlashFailed` or a subprocess error. |

## Files read and written

- **Read:** the UF2 for each half (`--left`/`--right`, or from `--firmware`: `foxglove-fw-*-{half}-{profile}.uf2` when present, else `eyelash_corne_studio_left.uf2` / `nice_view_custom-eyelash_corne_right-zmk.uf2`); `--left-reboot-image` when set; `INFO_UF2.TXT` on the bootloader drive (one `Board-ID:` line required); the serials map and the rollback folder from `local_config` (`tools/local.toml` or the environment override); the `foxglove-lab` executable in `target/{release,debug}` or `$CARGO_TARGET_DIR`; the typing check's `report.json` in `%TEMP%\foxglove-flash-typing-<half>`; Windows PnP devices, disk/volume mappings, and device-change events via PowerShell; `tasklist` for the lock owner's PID.
- **Written:** `flash.uf2` on the half's bootloader volume; the single-flasher lock `%TEMP%\foxglove-flash.lock` (or the `FOXGLOVE_FLASH_LOCK` env var), holding the owner's PID and removed on exit; one line per event (UTC time, half, image SHA, outcome, detail) appended to `%LOCALAPPDATA%\Foxglove\flash-history.log` (or the `FOXGLOVE_FLASH_HISTORY` env var), also mirrored to the real `%LOCALAPPDATA%` when running from a packaged app's shell; the log lines on stdout, or one JSON report with `--json`; a `pnputil /scan-devices` rescan with `--pnp-rescan` on an elevated prompt.

## Example

```
python scripts/flash/foxglove_flash.py --left L.uf2 --right R.uf2
```
