# Wallpaper manifest (`foxglove.json`, `foxglove-wallpaper` v1)

Any Wallpaper Engine wallpaper can describe itself to Foxglove with a `foxglove.json` next to
its `project.json` (#562). Foxglove builds in nothing about any wallpaper. The Bandit wallpapers
(BanditF/bandit-wallpapers) ship a manifest the same way a third-party wallpaper does, and
Foxglove works fully with none (docs/scope.md, "Optional integrations").

The code is `app/src/foxglove/wallpaper_manifest.rs`.

## What Foxglove does with it

- **Scenes page, Presets:** each motif is a lighting look under a **Wallpapers** heading, with
  the wallpaper's title and pack in its hover. With no manifest found, that group and **Right
  screen too** don't show, and only the effects are offered.
- **On screen:** a look whose wallpaper Wallpaper Engine shows now is tagged **On screen**.
- **Right screen too:** while a motif runs, the right screen shows the wallpaper's paired art.
- **Wallpaper sync (#493):** Foxglove pushes each monitor's wallpaper only the properties its
  `supports` lists, through Wallpaper Engine's command line. A wallpaper that supports none
  gets nothing.
- **Live feed (#499):** the feed's port reaches the wallpapers that support `foxglove_live`.
  The feed itself is open to any paired client (docs/live-feed.md).

## Where Foxglove looks

When Wallpaper Engine is installed (found as in `wallpaper_engine.rs`):

- each folder in `<wallpaper_engine>\projects\myprojects` and `projects\defaultprojects`;
- each Workshop item in `<library>\steamapps\workshop\content\431960`;
- the folder of each wallpaper on screen (the `file` Wallpaper Engine records per monitor).

The wallpaper service reads them when it starts, on **Refresh** (Art page, Match the
wallpaper), and when Wallpaper Engine's `config.json` or the Windows wallpaper changes (the
folder watches it already has; nothing polls). On those events a root's folder list is read
again only when the root folder changed (a project added or removed); a manifest edited inside
an existing project is seen on Refresh, or as soon as that wallpaper is on screen. A scene that
started before its wallpaper was found takes its look when the wallpaper is. When two manifests
share an id, the first wins (own projects before the Workshop), so a prefixed id is safer.

## The file

```json
{
  "format": "foxglove-wallpaper",
  "version": 1,
  "id": "tidepool.reef",
  "title": "Reef",
  "pack": "Tidepool",
  "supports": ["clock", "layer", "gamemode", "foxglove_live"],
  "palette": { "name": "Reef", "stops": ["#0fb5ae", "#f2e8cf", "#0b2545"] },
  "motifs": [
    {
      "name": "Reef drift",
      "about": "Kelp swaying in teal and sand",
      "look": { "kind": "drift", "palette": "Reef", "speed": 3, "width_mm": 180 }
    }
  ],
  "art": { "file": "keyboard/right.bin", "library": "reef.bin", "fallback": "Bandit_Emblem" }
}
```

| Field | Required | Meaning |
|---|---|---|
| `format`, `version` | yes | `"foxglove-wallpaper"`, `1`. Anything else is skipped. |
| `id` | yes | A stable id: lower-case letters, digits, `.`, `_`, `-`, at most 64. A reverse-domain or pack prefix (`tidepool.reef`) keeps it unique. |
| `title` | no | The name the page shows (1–40 characters). Defaults to `project.json`'s `title` (cut to 40), then the id. |
| `pack` | no | The pack or author name, shown with the title (1–40 characters). |
| `supports` | no | The Foxglove properties the wallpaper reads (below). Names Foxglove doesn't know are skipped, so a newer list still loads. |
| `palette` | no | One named palette: `name` and two or more `#rrggbb` `stops`. |
| `motifs` | no | Up to 8 lighting looks: `name` (1–40 characters), `about` (the hover text), and `look`. |
| `art` | no | The right-screen art paired with the wallpaper (below). |

Unknown fields are skipped. A manifest that breaks a rule is skipped as a whole, with one log
line (`Foxglove: …\foxglove.json skipped: …`), and is read again when its folder list is next
read (or on Refresh). The file must be under 64 KiB.

A wallpaper is matched to a monitor by its folder. Only for the Windows desktop wallpaper and
the `FOXGLOVE_WALLPAPER_OVERRIDE` test hook is it also matched by id (the file's name).

### `supports`

| Name | Property pushed | Value |
|---|---|---|
| `layer` | `foxglove_layer` | The highest active layer's name (`L<n>` when unnamed), `""` with no keyboard |
| `gamemode` | `foxglove_game` | `true` while a layer named GAME… is active |
| `clock` | `foxglove_scene` | The running scene's clock and geometry for that monitor (`foxglove_scene::wallpaper`), `""` with no scene |
| `foxglove_live` | `foxglove_live` | The live feed's port while it listens, `0` when it doesn't (docs/live-feed.md) |

Declare each one you list as a user property in `project.json` too: Wallpaper Engine passes
them to `wallpaperPropertyListener.applyUserProperties`. Foxglove pushes once per change, never
on a timer. When Foxglove quits, it sends each the empty value. `shared/foxglove-sync.js` and
`shared/foxglove-live.js` in bandit-wallpapers are working clients.

### `motifs[].look`

One of the Scenes page's six effects, with only the parameters it uses:

| Key | Values | Default |
|---|---|---|
| `kind` | `scanline`, `sweep`, `ripple`, `breathe`, `gradient`, `drift` | (required) |
| `colour` | `#rrggbb` (scanline, ripple, breathe) | Foxglove's orange |
| `background` | `night`, `off` | `night` |
| `palette` | Foxglove's (`Bandit`, `Forest`, `Steel`, `Ember`, `Spectrum`) or the manifest's own `palette.name` (sweep, gradient, drift) | `Bandit` |
| `speed` | 1–10 | 5 |
| `direction` | `down`, `up`, `right`, `left` | `down` |
| `width_mm` | 20–400 (the band's width, or Drift's size) | 80 |

A palette whose name is already taken (by Foxglove, or by another wallpaper with other colours)
is shown as `Name (Title)`, and so is a motif named like a preset already offered.

### `art`

Tried in this order, the first that exists wins:

| Key | Meaning |
|---|---|
| `builtin` | A set built into the keyboard's firmware, by its stable name (`custom_art.json`). |
| `file` | An NVA1 art file inside the wallpaper's folder (a relative path, never `..`). |
| `library` | A file name in Foxglove's art folder (`%APPDATA%\Foxglove\config\art`). |
| `bundled` | A set that ships with Foxglove, copied into the art folder. |
| `fallback` | Another built-in set. |

## Checking a manifest

`python -I tools/foxglove_manifest.py` in bandit-wallpapers checks a whole repo's manifests,
including that `supports` matches the scripts each page loads. Its rules mirror this page. In
Foxglove, `wallpaper_manifest.rs`'s tests read a fake third-party pack (Tidepool) from a fake
Wallpaper Engine folder.

## Later

Foxglove-hosted wallpapers (#513) will read the same file, so one manifest serves both hosts.
