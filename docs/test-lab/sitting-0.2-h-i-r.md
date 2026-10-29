# 0.2 sitting, resumed: batches H, I, R and Z re-cut (#618)

The rest of the 0.2 sitting after the stability pass (#618 exit criterion 4). Every step an agent
can do through the local API, the CLI, foxglove-lab, a Win32 query or a log moved to On-call. The
user's steps are one action each and only what eyes (or hands) must do: the wallpaper flicker, a
colour match, a click reaching the desktop, a screen's look.

`tests/sitting-parity.toml` maps every step below (`batch = "<id>"`), and
`scripts/test_sitting_parity.py` fails when one has no entry, when a user step has no needs-eyes
reason, or when an agent step names a check that doesn't exist.

Step ids: `HA1` is an agent step of batch H (On-call runs it), `H1` a user step.

**User steps: 12, was 25** (H 9 → 5, I 4 → 2, R 9 → 5, Z 3 → 0). Two fallbacks add steps back:
- `I3`–`I4` (the tray panel) stay only when `IA2`'s focus cases come back BLOCKED (Windows'
  foreground lock), not FAIL.
- On firmware without the right's redraw count (HELLO bit 25; 13ff04b, on the board since
  10/29 00:35Z, has none), `RA1`–`RA3` are BLOCKED and R stays at its old nine steps.

## What the re-cut found (read-only, 2026-10-29)

- **The old H7–H8 (#70) couldn't pass on this PC.**
  - "Lights follow the wallpaper" is off in the user's store (`[lighting]` has only
    `follow_accent = true`).
  - With it on, the accent comes from the first monitor, in Wallpaper Engine's key order, whose
    wallpaper has a palette. That is FURY Constellations on the Dell (`#d9a441`, hue/sat 39/70),
    whatever the HUD monitor shows. FURY Forest and the HUD have no palette.
  - The Windows accent Foxglove follows is UISettings' Accent, `#9c6f05` (42/97): the base of the
    accent palette Windows derives from the colour picked in Settings (`#d39f3f`, 39/70, DWM's
    AccentColor, which Foxglove doesn't read). The 10/29 pre-sitting run expected the pick and
    failed `HA4` on a correct 42/97 (fixed in the check; `wallpaper_check.py state` prints both).
  - On the board the user's lights read about 23/100, not 42/97: RGB_STATE is the colour after
    the keyboard's calibration (gamma 2.5 in the user's store), which Foxglove reads back as 42/97.
  - The case follows only with "Match keyboard" on, and that's off (`match_keyboard = false`).
  - `HA4` now checks the mechanism with the wallpaper test hook. `H5` keeps one look at the
    gold.
- **The HUD is already on the Samsung monitor**, so the old H1 pick is gone. Wallpaper Engine has
  Stop under full-screen and maximized programs (`playbackfullscreen: "stop"`), the likely cause
  of #586's flicker: each Win key reloads the wallpaper.
- **The accent push failed three times today** in the user's log: "Foxglove accent: not
  supported by this keyboard or keymap" at 01:34Z and 02:06Z, and "no answer" at 07:01Z. `HA0`
  reads RGB_STATE before `H5` relies on it.
- The Presets card's wallpapers (#571), from the installed manifests: Bandit Radar, Bandit
  Skyline, Bandit Courier, FURY Chase, FURY Forest, FURY Pattern, FURY Print, FURY
  Constellations, Cauldron Corner, Moon Garden, Wheel of the Year, Bandit HUD
  (`wallpaper_check.py state` prints the current list).

## Batch H: wallpaper (#586, #527, #70, #571)

Agent, before the sitting (On-call's run list):
- `HA0` `wallpaper_check.py state`: Wallpaper Engine runs, the HUD is on a monitor, the playback settings, the Presets card's expected names, "Lights follow the wallpaper", the Windows accent as Foxglove reads it, and what the lights follow now; `foxglove-lab --allow-app status` records RGB_STATE (the colour after the keyboard's calibration, about 23/100 for the accent's 42/97). `H5` runs only when the lights follow the stars wallpaper (`#d9a441`, 39/70, "Lights follow the wallpaper" on); with the Windows accent they don't, and `H5` is dropped.
- `HA4` `wallpaper_follow.py` (a test app, temp config, wallpaper test hook): switching to FURY Pattern (3/78), then Cauldron Corner (29/82), then FURY Forest (no palette: the Windows accent as accent.rs reads it, 42/97 on this PC) puts each colour on both halves within the bounds, one "Foxglove accent: applied" line each; each expectation names its source (the fixture palette or UISettings' Accent). The temp store has no calibration, so RGB_STATE reads the asked hue/sat. The lights are put back.

Agent, in the sitting, right before `H1`:
- `HA1` the devlog listener (bounded), the HUD's Diagnostics on through `-control applyProperties` (`wallpaper_check.py debug on --monitor SAM`), and the GAME layer on (`sitting/game-layer-on`): the log says "game mode on".

User:
- `H1` Start any full-screen game on the Samsung monitor (the one with the Bandit HUD).
- `H2` Press the Windows key once. Tell me: did the HUD flicker?
- `H3` Close the game.

Agent, right after `H3`:
- `HA2` `wallpaper_check.py game --since <HA1's time>`: Foxglove sent game mode on, Wallpaper Engine dropped no `-control` command, the DXGI resets, and from the devlog whether the reloaded HUD kept game mode (the old H5's "does it still show GAME MODE", #586's open question). Then Diagnostics off and `sitting/game-layer-off`.

User (the Foxglove window is already open: On-call's last step before the sitting):
- `H4` In the Foxglove window's left column, click "Scenes". Tell me the names under "Wallpapers" on the "Presets" card.
- `H5` Look at the keyboard's underglow, then at the stars wallpaper on the Dell monitor. Tell me: is it the same gold?

Agent:
- `HA3` Compare `H4`'s names with `HA0`'s list.

## Batch I: overlay and tray (#549, #405/#406)

Agent, before the sitting:
- `IA1` `overlay_tray.py` (a test app, with the user's remembered keyboard seeded into its store): with Settings not focused (the test app opens it behind) the overlay is WS_EX_TRANSPARENT and WS_EX_LAYERED, and WindowFromPoint on it gives the window underneath; with the tray panel open (it draws inside the overlay window) the window region is the panel, the overlay drops WS_EX_TRANSPARENT, a point on the panel hits the overlay, and a point beside it goes underneath. Each is waited for (bounded): the passthrough lands after the frame that set the region.
- `IA2` `overlay_tray.py --focus` (the same run): Settings (a second launch) focused arms the overlay over the keys (once its keymap session connected), Settings minimized keeps the app running and makes the overlay click-through again (#549's report), and the tray panel closes when the focus goes to Settings (#406). BLOCKED, not FAIL, when Windows keeps the foreground where it was, or when the overlay never connected (no keys to take the mouse over). A test app that ends mid-run is one FAIL with its exit code and log, and the rest is not run.

User:
- `I1` Click the minimize button (–) at the top right of the Foxglove window.
- `I2` Click on the desktop right where the faded keyboard picture is. Tell me: did the click reach the desktop, with nothing blocked?

Only when `IA2` was BLOCKED for the panel:
- `I3` Left-click the fox tray icon.
- `I4` Click any browser tab. Tell me: did the tab switch and the panel close?

## Batch R: right screen after #621 (#613 acceptance)

Needs main firmware with HELLO bit 25 on both halves (1ebdf9d or later). On 13ff04b, run R as in
On-call's old note.

Agent, before the sitting (Foxglove quit):
- `RA1` `sitting/right-redraw-after-select-while-idle`: a SELECT while the right is dark is drawn when a LEFT tap wakes it: RGB_STATE right on, the redraw count up, readback ZMK_2 (old R4–R6, #621 check 3).
- `RA2` `right_redraws.py`: three take-and-hand-back cycles of the LEDs (SET_FRAME, as the window's preview does) each rewrite the right panel (old R7–R8, #621 check 4).
- `RA3` `resilience/right-redraw-after-wake`: an idle-off and a LEFT tap rewrite the panel (the count behind `R2`).

User:
- `R1` Leave the keyboard alone until the right half's lights go off (about 30 s). Tell me: what does the right screen show: blank, the picture frozen, or static?
- `R2` Press one key on the LEFT half. Tell me: within about a second, does the right screen show its picture cleanly, with no static?
- `R3` Unplug the right half's cable.
- `R4` Leave the keyboard alone until the right's lights go off. Tell me: what does the right screen show?
- `R5` Press one key on the LEFT half. Tell me: does the right show its picture cleanly?

## Z: case zone 1 back to its colour

Agent:
- `ZA1` `foxglove-cli lights` for the OMEN 45L's key and zone 1, then `foxglove-cli light <key> <zone> #a57500`: the answer lists zone 1 as `#a57500` (old Z1–Z3, the Lights page's colour box).
