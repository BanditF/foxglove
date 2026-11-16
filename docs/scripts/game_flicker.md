# game_flicker.py — the sitting's H1-H3 (#586), the game side

`scripts/lab/game_flicker.py` answers the sitting's #586 from the game's side: does the Bandit HUD flicker when the Win key is pressed in a full-screen game? It stands in for a full-screen game on the HUD's monitor with a borderless, topmost Probe window that, like a real game, minimizes when it loses focus, then taps the real Win key and captures that monitor (GDI, about `--fps` times a second) for `--watch-s` seconds; a frame is a flicker when it goes dark (the wallpaper unloaded or reloading) or jumps (a flash) against the captures taken before the game. It also checks Foxglove's side: no wallpaper push may land inside Wallpaper Engine's reload quiet window (`RELOAD_QUIET_S`, 2.0 s) after the Win key. It takes the foreground and makes real input, so the PC must be unlocked and the user away (2 minutes idle), and it runs only under On-call with `FOXGLOVE_APP_OK=1`. It launches nothing. It sits between HA1 (Diagnostics on, the GAME layer on) and HA2 (`wallpaper_check.py game`, which reads the devlog's reloads after the fact).

## How to run

```
FOXGLOVE_APP_OK=1 python scripts/lab/game_flicker.py [--monitor SAM] [--out <dir>]
```

## Flags

| Flag | Type | Default | Description |
|---|---|---|---|
| `--monitor` | string | `SAM` | a piece of the HUD monitor's device name (Wallpaper Engine's key). |
| `--watch-s` | float | 6.0 | how long to capture after the Win key. |
| `--settle-s` | float | 5.0 | how long the game runs before the Win key. |
| `--band` | float | 0.4 | the top share of the monitor measured (so the Start menu itself isn't a flicker). |
| `--fps` | float | 10.0 | capture frame rate (frames per second). |
| `--out` | string | `lab-reports/game-flicker-<UTC time>` under the repo root | folder for the frames (PNGs, kept for a later look). |

## Exit codes

0 PASS, 1 FAIL, 2 BLOCKED (`FOXGLOVE_APP_OK` unset, or the PC had user input within the last 2 minutes).

## What it reads and writes

Reads: Foxglove's log `%APPDATA%\Foxglove\logs\foxglove.log` (the "wallpaper sync" push lines), the enumerated monitors, and the screen of the selected monitor (GDI capture, the top `--band` of it).

Writes: PNG frames to the `--out` folder (default `lab-reports/game-flicker-<time>`), and one line per check to stdout (flicker times, the push lines, the reload re-push). It also takes the foreground and makes real input: it creates and focuses the Probe stand-in window and taps the Win and Esc keys. It launches no other process.

## Example

`FOXGLOVE_APP_OK=1 python scripts/lab/game_flicker.py --monitor SAM` — stands in for a full-screen game on the monitor whose device name contains `SAM`, taps the Win key, captures the top 40 % of that monitor for 6 s, and reports whether the HUD flickered and whether Foxglove pushed a wallpaper into the reload.
