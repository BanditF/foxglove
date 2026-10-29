> **Since #516** this view lives on the Screens page (Showing now · On the keyboard · On this PC);
> the separate window is gone. Below is the original design, for history.

# Art library view (#59)

Status: design spec for review. Today **Settings → Right screen art** shows the status, the
pinned choice, a built-in picker, **Saved on the keyboard** (slots) and the hotlist
(`art_ui.rs`). The library folder is scanned with previews (`art_library.rs`), but there's no
single place that lists every piece and shows where it lives. This view is that place.

## What it lists

One grid of every piece of right-screen art Foxglove knows about, from five sources:

| source | where | id |
|---|---|---|
| **Built-in** | compiled into the right half's firmware (`custom_art.json` manifest; one set since #671, `Bandit_Emblem`) | manifest name, e.g. `Bandit_Gem` |
| **Saved on the keyboard** | flash slots on the right half (`art_slots`, count from the firmware, 20 KiB each) | slot number + the name in its header |
| **Library** | `%APPDATA%\Foxglove\config\art\*.bin`, including the wallpaper sets and `wallpapers.json` | file name |
| **Bundled** | sets that ship with the app: the lighting presets' art (`app/resources/lighting/art/`), the plugin art (`assets/plugins/*/`) | resource path |
| **Sources** (optional) | folders the user adds, e.g. `art-src\bandit\out` (Flipper-style frame folders) | folder path; built on demand with `flipper_art.py`'s converter |

The same art in two places is shown **once**, with every location as a badge, for example
Bandit Radar as built-in and also as `bandit-radar.bin` in the library. Pieces are matched by the
NVA1 blob's bytes (a hash). The wallpaper build already checks built-in matches the same way
(`BUILTIN` in keyboard.py).

## A card

```
┌──────────────┐
│  ▶ preview   │   animated thumbnail (art_preview), 2x, light-on-dark as on the keyboard
│   (68x140)   │
├──────────────┤
│ Bandit Radar │   label (manifest label / slot header / file name)
│ ● ON SCREEN  │   status line, see flags
│ [B] [S2] [L] │   location badges: Built-in, Slot 2, Library, Bundled, Source
│ 3.6 KiB 12f  │   size and frames (fps on hover)
│ ☆ hotlist    │   star toggles hotlist membership
└──────────────┘
```

Click a card to show it now (pin). Right-click (or the ⋯ button) for the actions below.

## Flags: what's on the device

- **● On screen:** the set the right half is showing now, from the art status event
  (`HOST_LINK_CAP_ART_STATUS_EVENT`, `D6` status). Exactly one card.
- **Built-in (B):** in this firmware's manifest. If the manifest isn't known for the running
  build, built-ins show as "Set N" with no preview, as today.
- **Saved (S n):** in flash slot n. It survives power cycles.
- **Pushed (RAM):** the set was pushed but not saved, so it's gone after a power cycle. Only the
  on-screen piece can be in this state.
- **Pinned 📌:** re-applied after power cycles (the store's pin).
- **In hotlist ☆** / **layer art** / **preset art:** used by a cycling trigger (#60). The badge
  says which.
- **Too big:** over the 16 KiB right-slot limit. It can't be pushed, and the card says why.

**Filters** across the top: All · On the keyboard (B + S + on screen) · Library · Hotlist ·
Sources. Also a search box over the labels.

## Actions

| action | for | does |
|---|---|---|
| Show now | any | pin and show it (a built-in or slot is instant; anything else is pushed) |
| Add to / remove from hotlist | any | toggles ☆ |
| Save to keyboard… | library, bundled, source, on-screen RAM | pick a slot (shows what it overwrites), save |
| Clear slot | saved | after confirmation, frees the slot |
| Use for layer… / preset… | any | sets layer or preset art (#60) |
| Show in folder | library, source | opens Explorer |
| Rebuild | source | re-runs the converter |
| Remove from library | library | moves the file to the Recycle Bin, never a hard delete |
| Ink: Auto / As is / Invert | library, source | which way round the piece is drawn (#666): Auto finds its background from the edge pixels of every frame (a tie: all of them), so its drawing is the ink; As is takes its dark pixels as the drawing, Invert its light ones. Kept per file or folder (`[art.polarity]`); a folder's copy in the library goes as its encode made it. The keyboard's Screens: Dark / Light then decides the look |

Risky actions (clear a slot, overwrite a slot) take a second click, like **Actions** in the
Keyboard section.

## Editing the built-ins

Built-ins are compiled into the firmware today, so "edit" needs the firmware's help. Two stages:

1. **Now, no firmware change: "replace" a built-in with a slot.** A slot can be marked as
   standing in for built-in N. The app then shows the slot wherever N was chosen: the pin,
   the hotlist, layer and preset art. The card shows the built-in struck through with
   "replaced by S2". It's an app-side alias, and undoing it is one click.
2. **Later, a custom firmware build: "Built-ins for my next firmware".** The user arranges
   the built-in list:
   - drag cards into order;
   - add from the library or sources;
   - drop unused sets;
   - a budget bar shows the image size against the bootloader limit (`0xAD000`, 16 KiB per set).

   Foxglove writes the list as the `flipper_art.py` input line set (the format in
   `ART_CREDITS.md`), plus the art files, and starts a build. Today that's a GitHub Actions
   firmware build. The result is flashed with `foxglove_flash.py` (with the user's go, as for
   any flash). The manifest (`custom_art.json`) keeps names stable, so pins and hotlists
   survive the reflash.

   The licensing register (`art-licensing.md`) is checked first: anything flagged
   **DO NOT SHIP** or **UNCLEAR** (bip3r, community-test) can't go in a built-in list, and the
   card says why.

## Layout and performance

- **Grid:** a grid of 2x thumbnails (136x280 px), wrapping, in its own scroll area. Settings
  gets an **Open art library** button; the existing compact controls stay in place for quick
  picks.
- **Thumbnails:**
  - decode on the art library's loader thread, as now;
  - animate only while visible and the window has focus;
  - cache decoded frames by hash.
- **Rescans** happen on a folder-watcher event (the `screens_watch` pattern) and on firmware
  connect (new manifest or slot list). They never poll.

## Work this needs

- **App:**
  - the view;
  - hash matching across sources;
  - slot replace/alias;
  - Recycle-Bin delete;
  - source folders with an on-demand converter run;
  - the built-in list editor (stage 2).
- **Firmware:** none for stage 1. Stage 2 uses the existing build and flash path.
- **Art:** nothing new. The cards use each piece's own frames, and the empty state reuses the
  Plugins tab's sleeping-Bandit illustration with "No art yet: add .bin files or a source
  folder".
