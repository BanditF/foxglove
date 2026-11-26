# wallpaper_check.py — batch H of the 0.2 sitting (#586, #527, #70, #571)

`scripts/lab/wallpaper_check.py` checks, from the files Wallpaper Engine, Foxglove and the Bandit wallpapers already write, the sitting's tickets #586, #527, #70
and #571 (full-screen game playback, the devlog reloads around it, the Presets card's wallpaper list, the lights following the wallpaper). It launches nothing
and changes nothing, except `debug on|off`, which sends one `-control applyProperties` to the running Wallpaper Engine to turn a wallpaper's Diagnostics on or off
(Wallpaper Engine does not save it).

## How to run

```
python scripts/lab/wallpaper_check.py state
python scripts/lab/wallpaper_check.py debug on|off --monitor SAM
python scripts/lab/wallpaper_check.py game --since 2026-10-29T09:00:00Z
python scripts/lab/wallpaper_check.py presets [--app <foxglove.exe>]
python scripts/lab/wallpaper_check.py gold
```

## Flags

| Flag | Subcommand | Default | Description |
|---|---|---|---|
| `--we PATH` | all | `C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine` | Wallpaper Engine's folder. |
| `on` | `debug` | required: `on` or `off` | Diagnostics on or off. |
| `--monitor KEY` | `debug` | required | a piece of Wallpaper Engine's monitor key, e.g. `SAM`. |
| `--since TIME` | `game` | required | UTC time, e.g. `2026-10-29T09:00:00Z`. |
| `--devlog PATH` | `game` | `C:\Users\fuchs\bandit-wallpapers\logs\devlog.jsonl` | the devlog snapshot file. |
| `--wallpaper NAME` | `game` | `Bandit HUD` | the snapshots' wallpaper title. |
| `--game-blocked` | `game` | off | HA5 (the game) was BLOCKED: no game to tell. |
| `--app PATH` | `presets` | the running Foxglove | a test app of this build (temp config, FOXGLOVE_APP_OK=1). |
| `--tolerance N` | `gold` | 4 | hue and saturation steps allowed (rounding). |

## Exit codes

0 PASS, 1 FAIL, 2 BLOCKED (a file, the devlog or the running Foxglove is missing).

## What it reads and writes

Reads: Wallpaper Engine's `config.json` and `log.txt` (under `--we`), each selected wallpaper's `foxglove.json` manifest next to its file,
`<we>\projects\myprojects\*\foxglove.json` (the looks' names), `%APPDATA%\Foxglove\foxglove.toml`, `%APPDATA%\Foxglove\logs\foxglove.log`,
the devlog JSONL (`--devlog`), and the Windows accent (UISettings' Accent via PowerShell, else `HKCU\...\Explorer\Accent`; the picked `HKCU\...\DWM`
AccentColor for the record). `presets` reads the running Foxglove's `scenes.page` (API 1.10); `gold` its `devices.list` and `lighting.state` (API 1.2).

Writes: to stdout only, one line per check. `debug on|off` additionally runs `wallpaper64.exe -control applyProperties` on the running Wallpaper
Engine for the one matching monitor; `presets --app` launches the given app on a temporary config.
A test app never reads the installed Wallpaper Engine folder or runs its program (#1519), so
`presets --app` gives it a stand-in folder (`FOXGLOVE_WALLPAPER_ENGINE_DIR`, a temp copy of the
manifests with no program): its card must list the same looks and tag none as on screen.

## Example

`python scripts/lab/wallpaper_check.py debug on --monitor SAM` — turns Diagnostics on for the monitor whose Wallpaper Engine key contains `SAM`; the
wallpaper then posts a snapshot every 2 s to the devlog listener.
