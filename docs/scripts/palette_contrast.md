# palette_contrast.py — contrast check for the shared palettes

`scripts/palette_contrast.py` checks the shared palettes (`foxglove-palette` v1, docs/palette.md) against WCAG contrast. It fills in the derived roles the way the Rust reader does (`palette_file.rs`: `deep` is the accent clamped to 3:1 on `paper`, `accent_light` to 4.5:1, `accent2_light` and `accent3_light` likewise), then checks every text pair at 4.5:1 and every graphics pair at 3:1. Bold-tier palettes (`"tier": "bold"`) are also checked on their `accent2`-tinted grounds (the wash: 12% on the dark grounds, 8% on `paper`), and their dark grounds must be deep shades (HSL lightness at most 14%). It reads the palette files (and `bandit.json` every time) and prints a per-pair report.

## How to run

```
python scripts/palette_contrast.py [--markdown] [file.json ...]
```

With no files it checks every `*.json` in `app/resources/palettes`. The script does not use argparse: `--markdown` is recognized anywhere in the command line, and every other argument that does not start with `--` is taken as a palette file (unknown `--` flags are ignored silently).

## Flags

| Flag | Default | Description |
|---|---|---|
| `--markdown` | plain text | print the report as a Markdown heading and a `Foreground` / `Ground` / `Ratio` / `Needs` table instead of one plain line per pair. |
| `file.json ...` | all files in `app/resources/palettes` | one or more palette files to check. |

## Exit codes

0 all pairs pass, 1 at least one pair fails (or a bold ground is too light). A malformed palette (a bad hex value, a `tier` other than `harmony` or `bold`, `accent3` without `accent2`, a bold palette without `accent2`) raises a `ValueError` instead of reporting a result.

## What it reads and writes

Reads: the given palette files (or every `*.json` in `app/resources/palettes`), plus `app/resources/palettes/bandit.json` for the fallback roles (`base`, `ink`, `paper`, `paper_ink`, `wall_ground`; a missing `app_ground` falls back to `base`). Each palette file must have `name` and `stops`; `roles` is optional, `tier` defaults to `harmony`.

Writes: stdout only.

## Example

`python scripts/palette_contrast.py --markdown app/resources/palettes/bandit-ember.json` — resolves the derived roles for that one palette and prints each promised foreground/ground pair with its ratio, the minimum it needs, and `ok` or `FAIL`.
