# plugin_panels.py — first-party plugin left-panel assets

`scripts/plugin_panels.py` draws the left-panel assets for the first-party plugins (spec: `docs/plugin-screens.md`). The host panel is 64x52, 1 bit, and sprites are stored the way `screens.panel_override` takes its bitmap: rows MSB first, each row padded to whole bytes, 1 = ink, base64. A plugin builds a frame by ORing sprites at the "layout" positions, or sends a ready-made "frames" entry as it is. The script generates every bitmap in code (seven-segment digits, the Pomodoro icons, the 3x5 font, and the mic-mute and broadcast-status states) and, for each of the three plugins, writes a `panel.json` carrying the plugin id, layout and packed bitmaps plus PNG previews of every state at 4x scale.

## How to run

```
python -I scripts/plugin_panels.py
python -I scripts/plugin_panels.py --sheet sheet.png
```

## Flags

| Flag | Default | Description |
|---|---|---|
| `--sheet PATH` | none | Also write a review sheet of every preview frame (named, 5 per row, 4x scale) to `PATH`. |

## Exit codes

Needs Pillow (`from PIL import Image`). The script sets no explicit exit code except argparse: a bad flag exits 2 and `-h` exits 0. On success it exits normally (0) after printing `wrote panel assets to assets/plugins/`; with `--sheet` it also prints `sheet <path>`. On failure the exception propagates.

## Files read and written

- **Reads:** no data files — every bitmap is generated in code (the `--sheet` path is written relative to the current directory).
- **Writes:** under `assets/plugins/` (directories created if missing), for each plugin a `panel.json` (JSON, one-space indent, trailing newline) and `preview/<state>.png`:
  - `org.foxglove.focus-timer/`: previews `focus`, `focus-late`, `rest`, `paused`
  - `org.foxglove.mic-mute/`: previews `muted`, `live`, `unknown`, `muted-label`, `live-label`
  - `org.foxglove.broadcast-status/`: previews `idle`, `recording`, `recording-paused`, `streaming`, `both`, `disconnected`, `streaming-label`, `both-label`
  - the sheet image at the `--sheet` path, if given.

## Example

```
python -I scripts/plugin_panels.py --sheet sheet.png
wrote panel assets to assets/plugins/
sheet sheet.png
```
