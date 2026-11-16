# release_assets.py — firmware release assets

`scripts/firmware/release_assets.py` assembles a firmware release's assets (ticket #378) from the UF2s of one CI run. It ships the GA left half, the GA right half, and the settings-reset UF2 under the release's own names, and writes a SHA256SUMS over them. The settings-reset UF2 is RESCUE.md's last resort: it erases the half's settings and Bluetooth pairings, then the half is flashed again. Standard library only.

## How to run

```
python scripts/firmware/release_assets.py --version 0.2.0 --src out --dest release
```

All three flags are required.

## Flags

| Flag | Description |
|---|---|
| `--version` (required) | The release version, used in the output names. |
| `--src` (required) | Directory holding the CI's named UF2s: `foxglove-fw-v<version>-<sha>-left-ga.uf2`, `...-right-ga.uf2`, `...-settings-reset.uf2`. |
| `--dest` (required) | Directory the assets are written into (created with parents if missing). |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All three UF2s were copied and SHA256SUMS was written. |
| 1 | A kind of UF2 is missing in `--src`, or more than one file matches a kind; the offending kinds are printed to stderr. |

## Files read and written

- **Reads:** the `*.uf2` files in `--src` whose names end in `-left-ga.uf2`, `-right-ga.uf2`, or `-settings-reset.uf2` (exactly one of each is required).
- **Writes:** into `--dest`: `foxglove-<version>-left.uf2`, `foxglove-<version>-right.uf2`, `foxglove-<version>-settings-reset.uf2`, and `SHA256SUMS` (sha256sum format, `<hex>  <name>`, one line per asset). Each written name is printed to stdout, SHA256SUMS last.

## Example

```
python scripts/firmware/release_assets.py --version 0.2.0 --src out --dest release
```

With the three CI UF2s in `out/`, this writes `foxglove-0.2.0-left.uf2`, `foxglove-0.2.0-right.uf2`, `foxglove-0.2.0-settings-reset.uf2`, and `SHA256SUMS` into `release/`.
