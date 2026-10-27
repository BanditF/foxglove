# What Foxglove is for (scope charter)

Every feature request is measured against this page before design (the research & scope gate in
[`agents.md`](agents.md)). User rule, 2026-10-26.

## In scope

Foxglove is a Windows app for the **keyboard and the lights and screens around it**:
- **The keyboard:** its screens and art, its lights, the layer overlay (KeyPeek), its keymap,
  battery and connection, flashing and firmware updates. Any ZMK or QMK board, with our own boards
  first in line.
- **PC lighting and screens:** case lights, peripherals' lights (through Windows Dynamic Lighting
  where it can), and scenes that tie them to the keyboard and the wallpaper.
- **Device management on Windows:** finding devices, their state, and one place to see them.
- **Optional integrations:** Wallpaper Engine, OBS, SignalRGB, OpenRGB, iCUE and others. Foxglove
  works fully without each of them and is richer with them.

## Out of scope

- **Hardware safety and performance controls:** fan curves, pump speed, overclocking, power
  limits. Leave them to the vendor's tool (OMEN Command Center and the like).
- **Generic automation and macro tooling,** unless it is about the keyboard (a key's action, a
  layer, a screen). Foxglove is not AutoHotkey.
- **Anything that needs a cloud account** to work. Optional downloads (updates, the plugin
  catalog) are fine when they can be turned off.
- **One-PC tuning:** a feature that only works after someone tunes it on one machine. It has to
  work on any user's PC from Windows' own mechanisms.

## Deciding an edge case

Ask, in order:
1. **Is it about the keyboard, or the lights and screens the user sees with it?** If not, it is out.
2. **Does it need a third-party app?** Then it is an optional integration: Foxglove must work
   without it, and the feature hides or shows one hint when it is missing.
3. **Does it run on its own on any Windows PC,** event-driven, with no free-running polling and no
   manual tuning? If it can't, redesign it or leave it out.
4. **Does a vendor tool already do it well, and is it safety-critical?** Then leave it to them.
5. **Still unclear?** Label the issue `needs-decision` and ask the user through the Hub, with a
   recommendation.
