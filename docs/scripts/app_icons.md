# app_icons.py

Draws Foxglove's app icon and tray glyphs from the Bandit fox motif (#31, #403). It produces a dark tile with the low-poly fox as the app icon (SVG master, PNG, ICO), every tray state for Windows taskbars (white and black variants at 16/20/24/32 px) plus macOS templates at 44 px, and the sparse-package identity logos. The file names match KeyPeek's, so upstream syncs do not conflict.

## Requirements

Pillow (`PIL`) is required. Without it the script fails on import with an unhandled `ImportError`. Everything else is the standard library.

## Usage

    python -I scripts/app_icons.py [--tray] [--sheet x.png]

## Flags

| flag | behaviour |
|---|---|
| (none) | write the app icon (`icon.svg`, `icon.png`, `icon.ico`), all tray glyphs and the identity logos |
| `--tray` | write the tray glyphs only; this skips the app icon and also the identity logos |
| `--sheet <path>` | after writing, also render a review sheet to `<path>` (relative paths resolve against the current directory) showing every tray glyph at real size and at 6x zoom on a dark and a light taskbar, plus the macOS templates; works with or without `--tray` |

## Exit codes

0 on success. The script does not call `sys.exit()`; failures (a missing Pillow, a missing output folder) raise unhandled exceptions.

## Files read

The fox polygons are hard-coded in the script, so drawing needs no input files. The exception is `--sheet`: it re-opens the tray PNGs that were just written in `app/resources/` to compose the sheet, and it tries the font `segoeui.ttf`, falling back to Pillow's default font if that is not available.

## Files written

| file(s) | location | sizes / variants |
|---|---|---|
| `icon.svg` | app/resources/ | 256x256 master, vector |
| `icon.png` | app/resources/ | 256x256, dark tile with fox |
| `icon.ico` | app/resources/ | multi-size: 16, 20, 24, 32, 48, 64, 256 px (sizes <= 24 use a simplified silhouette) |
| `tray-icon-white*.png`, `tray-icon-black*.png` | app/resources/ | states: plain (on), `-off`, `-paused`, `-screen` (the fox plus an orange eye, an almond outline with a pupil and no disc, bottom right; 16 and 20 px are hand-placed pixel maps, `EYE_MAPS`, the rest is drawn from the same proportions); sizes 16, 20, 24, 32 px |
| `tray-icon-template*.png` | app/resources/ | 44 px, black + alpha for macOS templates, same four states |
| `logo44.png`, `logo50.png`, `logo150.png` | app/packaging/identity/Assets/ | sparse-package identity logos (not written with `--tray`) |

## Tray file names

Name pattern: `tray-icon-<variant><state><size>.png`, where variant is `white` (for dark taskbars), `black` (for light taskbars) or `template`; state suffix is empty (on), `-off`, `-paused` or `-screen`. The size suffix is omitted for 32 px and 44 px (the plain name is the 32 px glyph, and the 44 px template); the other sizes add `-16`, `-20` or `-24`. For example `tray-icon-white-paused-16.png` and `tray-icon-template-off.png`.

## Example

    python -I scripts/app_icons.py --tray --sheet tray-sheet.png
