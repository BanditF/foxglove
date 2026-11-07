# Shared palette (`foxglove-palette` v1)

One file of colour roles for the app theme, the scenes, the lights and the
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
    "paper_ink": "#1b1b1c",
    "app_ground": "#060812",
    "wall_ground": "#09090a"
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
| `roles.app_ground` | no | The night ground Foxglove draws on: the scenes' **Night** background. Bandit's file keeps it equal to `base`. |
| `roles.wall_ground` | no | The night ground the wallpapers draw on (`bg` in bandit-wallpapers' `bandit.js`). |
| `roles.accent2` | no | A second accent, for dark grounds (see [Harmony palettes](#harmony-palettes)). Without it a consumer uses `accent` alone, which is Bandit's look. |
| `roles.accent2_light` | no | `accent2` for text on light grounds. Derived: `accent2` clamped to 4.5:1 on `paper`. |
| `roles.accent3` | no | A third accent, used even more sparingly. Only read when `accent2` is set. |
| `roles.accent3_light` | no | `accent3` for text on light grounds. Derived: `accent3` clamped to 4.5:1 on `paper`. |
| `harmony` | no | A label for people: `complementary`, `split-complementary`, `triadic` or `analogous`. No reader acts on it. |

### The two night grounds

Bandit has two near-blacks on purpose: `app_ground` `#060812`, a blue night that the lights and
the app's scenes sit on, and `wall_ground` `#09090a`, the neutral black the wallpapers are drawn
on. They are different surfaces, so they stay different colours. Both are named so neither gets
"fixed" into the other by accident. Readers that don't know these roles skip them (v1 only adds
fields); `base` is still the role every reader uses for the dark ground.

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
| Art previews | None: `art_preview.rs` keeps white on black on purpose, because the previews mimic the keyboard's 1-bit nice!view screen (#610). The palette doesn't tint them. |
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

Edit `app/resources/palettes/bandit.json` only, then re-vendor it into bandit-wallpapers. The
theme (`theme.rs`) and the scenes (`scene_looks.rs`) read it when the app starts; the theme's
tests check that its contrast rules hold for the file, and for either Bandit orange with the
hand-picked `highlight` and `accent_light` taken out. Keep `name` as `Bandit`: saved looks and
manifests name the palette.

## Harmony palettes

Bandit's single orange stays the default and isn't changed. The harmony palettes add to the
library: each one keeps the fox and Bandit orange `#f08a3c` (HSL hue 26°) as the anchor. Then it
adds one or two colours from the colour wheel as accents. They are new files next to
`bandit.json`. No reader loads them yet: the app's reader skips `accent2` / `accent3` today
(they are additive v1 roles), and listing the files in the app is #871.

| File | Name | Harmony | Second accent (`accent2`) | Third accent (`accent3`) | Night (`base`) |
|---|---|---|---|---|---|
| `bandit-steel.json` | Bandit Steel | complementary | steel blue `#6ca5d0` (206°) | — | `#050a13` |
| `bandit-dusk.json` | Bandit Dusk | split-complementary | sea glass `#62bcb6` (176°) | periwinkle `#999ee6` (236°) | `#07081a` |
| `bandit-foxglove.json` | Bandit Foxglove | triadic, toned down | foxglove violet `#b9a1d9` (266°) | leaf jade `#6bb38a` (146°) | `#0a0814` |
| `bandit-ember.json` | Bandit Ember | analogous | gold `#edb545` (40°) | ember red `#ea6753` (8°) | `#0d0807` |

All four keep Bandit's `accent`, `highlight` and `accent_light` exactly
(`scripts/test_palette_contrast.py` checks this). Their grounds lean a little toward the palette's
colour, and `app_ground` equals `base` as in Bandit's file. The cool accents are pale and only
moderately saturated (HSL saturation 32–60 %) on purpose: next to a saturated orange they read as
cool light, not as a second brand colour.

### Usage rule (every harmony palette)

- **The fox is never recoloured.** It keeps its orange, cream and ink in every palette.
- **Orange keeps the most accent area.** `accent2` covers at most **15 %** of a surface's area,
  and `accent3` at most **3 %**. Orange always covers at least as much as `accent2`. (Raised
  from 10 % at the taste check, 11/07.)
- **Where `accent2` goes:** the second ring and the compass nodes of a HUD, one secondary
  value (for example the signal state), peak caps on audio bars, progress fills, the status dot,
  a scanline tint and an ambient glow behind the motif. The tint and the glow stay at 7 %
  opacity or less.
- **Where `accent3` goes:** one marker per surface (the HUD's triangle), or the homing keys.
- **Not for:** headers, the title, the tagline, the clock, warnings. Those keep orange and ink.
- **Lights:** orange on the alpha keys, `accent2` on the thumb keys (6 of 42), `accent3` on the
  homing keys F and J (2 of 42). Effects are slow and smooth: a two-tone breathe (orange and
  `accent2` in opposite phase, 8 s loop, never below 35 %) or a slow gradient sweep through the
  stops for Ember. Effects only may pass the area budget while they run (Ember's sweep); a still
  surface never does. No flashes and no strobing; nothing changes faster than once per 2 s.
- **Light grounds** use the `_light` roles for text. Bandit's hand-picked `accent_light` is
  4.98:1 on the app's white cards but 4.0:1 on `paper`: on `paper` use it for graphics and
  large text only. That's unchanged from Bandit.

### Contrast

`python scripts/palette_contrast.py` (add `--markdown` for full tables) checks every bundled
palette. Text roles need 4.5:1 on each dark ground (`base`, `app_ground`, `wall_ground`) or on
`paper`; graphics need 3:1. `app_surface` is the light theme's white card (`theme.rs`), not a
role. The lowest ratio per role:

| Role | Needs | Steel | Dusk | Foxglove | Ember |
|---|---|---|---|---|---|
| `ink` (dark grounds) | 4.5 | 14.60 | 14.61 | 14.62 | 14.74 |
| `accent` (dark grounds) | 4.5 | 7.94 | 7.94 | 7.95 | 7.96 |
| `highlight` (dark grounds) | 4.5 | 11.04 | 11.05 | 11.06 | 11.08 |
| `accent2` (dark grounds) | 4.5 | 7.49 | 8.87 | 8.66 | 10.70 |
| `accent3` (dark grounds) | 4.5 | — | 7.92 | 7.98 | 6.22 |
| `paper_ink` on `paper` | 4.5 | 13.86 | 13.78 | 13.84 | 13.86 |
| `accent2_light` on `paper` | 4.5 | 5.05 `#1c649c` | 4.88 `#0b6e68` | 5.60 `#6d45a1` | 4.87 `#8a570a` |
| `accent3_light` on `paper` | 4.5 | — | 6.66 `#3941ac` | 4.95 `#206f42` | 5.14 `#af301d` |
| `accent_light` on the app's cards | 4.5 | 4.98 | 4.98 | 4.98 | 4.98 |
| `accent_light` on `paper` (graphics) | 3 | 4.01 | 3.99 | 4.01 | 3.98 |
| `deep` (derived) on `paper` | 3 | 3.02 | 3.00 | 3.02 | 3.01 |

The script runs in `scripts/check.ps1` through its unit test, so a palette that breaks a
promise fails the gate.

### Research

- **Reuse:** the `foxglove-palette` v1 format and its derivations (`readable`, `deep`,
  `accent_light`). The new roles reuse the `accent` / `accent_light` pattern. The contrast
  script ports `palette_file::readable` as it is.
- **Emulate:** Material 3's secondary and tertiary colour roles (a second and a third accent
  with their own on-light shades, sized by usage, not by count). The 60-30-10 rule of interior
  and UI colour (ground, ink, accent), with the second accent taking part of the 10. Adobe
  Color's harmony rules (complementary, split-complementary, triadic, analogous) for picking the
  hues. Radix Colors' habit of publishing contrast with every step.
- **Design our own:** only the usage budget per surface (15 % / 3 %, the fox never recoloured)
  and the light mapping (thumbs, homing keys).
