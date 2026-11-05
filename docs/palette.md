# Shared palette (`foxglove-palette` v1)

One file of colour roles for the app theme, the scenes, the lights, the art previews and the
Bandit wallpapers (#40, piece c). Before it, the Bandit colours were copied by hand into each of
them and drifted.

- **The spec:** this page.
- **Bandit's palette:** `app/resources/palettes/bandit.json`. Bandit is the app's own theme and
  built-in scene palette, so it lives here; Foxglove still builds in nothing about any wallpaper.
- **The reader:** `app/src/foxglove/palette_file.rs`.
- **bandit-wallpapers** vendors a copy (`shared/palette/bandit.json`, with the foxglove commit
  it came from). Its build turns the copy into the wallpapers' colours and warns when the copy
  differs from foxglove main. Wallpapers fetch nothing at runtime.

## The file

```json
{
  "format": "foxglove-palette",
  "version": 1,
  "name": "Bandit",
  "roles": {
    "accent": "#f08a3c",
    "highlight": "#ffb070",
    "accent_light": "#b4540f",
    "base": "#060812",
    "ink": "#e8dcc4",
    "paper": "#e9e6df",
    "paper_ink": "#1b1b1c"
  },
  "stops": ["#f08a3c", "#ff5f3c", "#060812"]
}
```

| Field | Required | Meaning |
|---|---|---|
| `format` | yes | `"foxglove-palette"` |
| `version` | yes | `1`. A reader skips a file of any other version as a whole. |
| `name` | yes | The palette's name, as the Scenes page shows it. |
| `roles.accent` | yes | The palette's colour: links, focus and selection in the app, the lights' colour. |
| `stops` | yes | 2 to 8 colours, the named palette the scenes sweep through. |
| `roles.highlight` | no | A lighter accent (hover). Derived: the accent 0.08 lighter in Oklab. |
| `roles.deep` | no | A darker accent for graphics on light grounds. Derived: the accent clamped to 3:1 contrast on `paper`. |
| `roles.accent_light` | no | The accent for text on light grounds (the app's light theme). Derived: the accent clamped to 4.5:1 on `paper`. Set it to hand-pick the shade. |
| `roles.base` | no | The dark ground: the scenes' **Night** background. Falls back to Bandit's. |
| `roles.ink` | no | Ink on `base`. Falls back to Bandit's. |
| `roles.paper` | no | The light ground. Falls back to Bandit's. |
| `roles.paper_ink` | no | Ink on `paper`. Falls back to Bandit's. |

Every colour is sRGB `#rrggbb`: every consumer (egui, CSS, the manifest, the lights, the
previews) takes 8-bit sRGB. Oklab is the space the derivations work in, but it isn't stored, so
the file stays the one source of truth with no float drift.

"Clamped to N:1" is the rule of `FoxgloveLive.readable` in bandit-wallpapers: the same hue and
saturation (HSL), darkened or lightened only as far as it takes to reach a WCAG contrast of N
against the ground. The Rust reader (`palette_file::readable`) and the wallpapers'
`tools/palette.py` follow the same rule.

## Who uses which role

| Consumer | Roles |
|---|---|
| App theme (`theme.rs`) | dark: `accent`, `highlight` (hover); light: `accent_light`. The other theme tokens (grounds, text, status colours) stay the theme's own. |
| Scenes (`scene_looks.rs`) | `stops` is the **Bandit** palette, `base` the **Night** background, `accent` the demo scanline's colour. |
| Lights | `accent`. Only its hue and saturation reach a device; per-device correction stays in the drivers' calibration, not in the palette. |
| Art previews | Planned: `ink` on `base` (dark) or `paper_ink` on `paper` (light). Today `art_preview.rs` draws white on black, as the keyboard's 1-bit screen does. |
| Wallpapers (`shared/bandit.js`) | `orange` ← `accent`, `orangeLight` ← `highlight`, `orangeDark` ← `deep`, `cream` ← `ink`, `bg` ← `base`, `paper`, `paperInk` ← `paper_ink`; the `schemecolor` default ← `accent`. |

## Compatibility

- Changes inside v1 only add fields; readers skip roles and fields they don't know. A change
  that would break a reader is v2, which v1 readers skip as a whole. We avoid that.
- A wallpaper manifest (`foxglove-wallpaper` v1, docs/wallpaper-manifest.md) can carry the same
  `roles` in its `palette`. Older app builds skip it and keep reading `stops` and `accent`.
- Wallpapers built before the vendoring keep their own hard-coded colours. Nothing breaks; they
  just don't follow later changes.

## Who wins

- App chrome always uses the bundled palette; no wallpaper restyles the app.
- Lights and the scenes' **Wallpaper** palette: the manifest of the wallpaper on screen, then its
  picture, then the Windows accent or Bandit.
- Wallpaper drawing: the user's own setting, then the keyboard's light colour (live feed, behind
  its setting), then the wallpaper's built palette.
- The live feed doesn't carry the palette (#40 Q3). A stale vendored copy loses on purpose: the
  manifest describes what is on screen, and the wallpapers' build warns about the drift.

## Changing a Bandit colour

Edit `app/resources/palettes/bandit.json` only, then re-vendor it into bandit-wallpapers. For now
a test (`palette_file.rs`) checks that the theme and the scenes still equal the file; once they
read it (#40(c) step 5), the tests check that the theme's contrast rules still hold.
