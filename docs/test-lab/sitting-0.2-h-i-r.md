# 0.2 sitting, resumed: batches H, I, R and Z re-cut (#618), and P (#609)

The rest of the 0.2 sitting after the stability pass (#618 exit criterion 4). The user said human
tests cost far too much, so every step is now an agent step: a lab case On-call runs hands-free,
with the PC unlocked and the user away. What a person looked at or did is read back (the local
API, foxglove-lab, a Win32 query, a screen capture, a log) or done with real input (SendInput) on
lab-owned windows.

`tests/sitting-parity.toml` maps every step below (`batch = "<id>"`) and every former user step to
the agent step that replaced it (`replaces`) or to the short residual list (`[[residual]]`);
`scripts/test_sitting_parity.py` fails when a step has no entry, a former step is unmapped, a user
step has no needs-eyes reason, or a step names a check that doesn't exist.

Step ids: `HA1` is an agent step of batch H (On-call runs it); `H1` would be a user step.

**User steps: 0, was 18** (H 5 → 0, I 2 → 0, R 5 → 0, P 6 → 0, Z 0; the I3–I4 fallback is gone
too). What can't be automated is in [What's left](#whats-left): none of it is a step of this
sitting.

## Run it

On-call, from a checkout of the PR's head, the PC unlocked and the user away (about 25 minutes:
H 3, I 3, R 4, P 14; the P stage keeps the keyboard untouched about 13 minutes):

```powershell
$env:FOXGLOVE_APP_OK = '1'
powershell -ExecutionPolicy Bypass -File scripts\lab\sitting.ps1
```

`-App <foxglove.exe>` (default `app\target\release\foxglove.exe`, built from this tree), `-Skip
H,I,R,P` to leave batches out, `-Monitor SAM` for the HUD's monitor. After the release gate:
`scripts\release-test.ps1 -Firmware <dir> -AllowApp -NonInteractive -Sitting`. Exit 0 PASS, 1
FAIL, 2 BLOCKED; `RA5` alone BLOCKED (the right on USB power) is the expected gate, not a hole.
The report folder keeps every stage's output and HA5's frames (PNG) for a later look.

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
- **The HUD is already on the Samsung monitor**, so the old H1 pick is gone. Wallpaper Engine has
  Stop under full-screen and maximized programs (`playbackfullscreen: "stop"`), the likely cause
  of #586's flicker: each Win key reloads the wallpaper.
- **The accent push failed three times that day** in the user's log ("not supported by this
  keyboard or keymap" at 01:34Z and 02:06Z, "no answer" at 07:01Z), all on builds from before
  #634/#636 or during a flash. `HA6` reads what the app keeps now.
- The Presets card's wallpapers (#571), from the installed manifests: Bandit Radar, Bandit
  Skyline, Bandit Courier, FURY Chase, FURY Forest, FURY Pattern, FURY Print, FURY
  Constellations, Cauldron Corner, Moon Garden, Wheel of the Year, Bandit HUD
  (`wallpaper_check.py state` prints the current list).

## Batch H: wallpaper (#586, #527, #70, #571)

- `HA0` `wallpaper_check.py state`: Wallpaper Engine runs, the HUD is on a monitor, the playback settings, the Presets card's expected names, "Lights follow the wallpaper", the Windows accent as Foxglove reads it, and what the lights follow now; `foxglove-lab --allow-app status` records RGB_STATE (the colour after the keyboard's calibration).
- `HA3` `wallpaper_check.py presets --app <exe>` (old H4): the Presets card of a test app of this build, through the local API's `scenes.page` (API 1.10, the card's own rows), lists every installed wallpaper look and nothing else under "Wallpapers", and tags "On screen" exactly the wallpapers Wallpaper Engine shows.
- `HA6` `wallpaper_check.py gold` (old H5): the running Foxglove's `lighting.state` (read back through the keyboard's calibration) is the hue/sat of what it follows now, the stars' `#d9a441` with "Lights follow the wallpaper" on, else the Windows accent: the user's calibration is their own match of the keyboard against the screen, so the same colour through it is the same gold to the eye.
- `HA4` `wallpaper_follow.py` (a test app, temp config, wallpaper test hook): switching to FURY Pattern (3/78), then Cauldron Corner (29/82), then FURY Forest (no palette: the Windows accent as accent.rs reads it, 42/97 on this PC) puts each colour on both halves within the bounds, one "Foxglove accent: applied" line each; each expectation names its source. The lights are put back.
- `HA1` the devlog listener (bounded), the HUD's Diagnostics on through `-control applyProperties` (`wallpaper_check.py debug on --monitor SAM`), and the GAME layer on (`sitting/game-layer-on`): the log says "game mode on".
- `HA5` `game_flicker.py --monitor SAM` (old H1–H3): a lab window stands in for a full-screen game on the HUD's monitor (borderless, topmost, black; it takes the foreground with a real click and minimizes itself on focus loss, as an exclusive full-screen game does), a real Win key tap (SendInput), and the monitor captured about 10 times a second for 6 s (GDI: what DWM composes, the wallpaper too): a dark or flashing frame against the HUD's own captures before the game is a flicker (#586), with its time for `HA2`. Then a real Esc closes the Start menu and the window closes; the HUD must show again. The frames are kept for a later look. A capture that sees only black is BLOCKED. Since #586's fix it also reads Foxglove's log: no wallpaper push may land in the 2 s reload after the Win key (`wallpaper_sync.rs` `RELOAD_QUIET`), and the re-push after it ("after a full-screen program", Wallpaper Engine on Stop) is printed.
- `HA2` `wallpaper_check.py game --since <HA1's time>`: Foxglove sent game mode on, Wallpaper Engine dropped no `-control` command, the DXGI resets, and from the devlog whether the reloaded HUD kept game mode (#586's open question). Then Diagnostics off and `sitting/game-layer-off`.

## Batch I: overlay and tray (#549, #405/#406)

- `IA1` `overlay_tray.py` (a test app, with the user's remembered keyboard seeded into its store): with Settings not focused the overlay is WS_EX_TRANSPARENT and WS_EX_LAYERED, and WindowFromPoint on it gives the window underneath; with the tray panel open the window region is the panel, the overlay drops WS_EX_TRANSPARENT, a point on the panel hits the overlay, and a point beside it goes underneath. Each is waited for (bounded).
- `IA2` `overlay_tray.py --focus` (the same run): Settings (a second launch) focused arms the overlay over the keys, Settings minimized keeps the app running and makes the overlay click-through again (#549), and the tray panel closes when the focus goes to Settings (#406). PASS on 10/29 (#655 rerun).
- `IA3` `overlay_tray.py --real-clicks` (the same run; old I1–I4): real mouse clicks (SendInput, routed by Windows as a person's): Settings' minimize button, found by Windows' own hit test (WM_NCHITTEST), minimizes it and the app keeps running with the overlay click-through (I1); a click on the faded keyboard reaches a lab window under the overlay (I2); a click on the tray icon (Shell_NotifyIconGetRect) opens the panel (I3); a click on another lab window ("any browser tab") gets the focus and the panel closes (I4). Each click puts the mouse back.

## Batch R: right screen after #621 (#613 acceptance)

Needs firmware with HELLO bits 24–25 on both halves (main since 1ebdf9d). Foxglove quit (the
runner stops it by its exact name and starts it again by its path).

- `RA1` `sitting/right-redraw-after-select-while-idle`: a SELECT while the right is dark is drawn when a LEFT tap wakes it: RGB_STATE right on, the redraw count up, readback Bandit_Emblem (#621 check 3).
- `RA2` `right_redraws.py`: three take-and-hand-back cycles of the LEDs (SET_FRAME, as the window's preview does) each rewrite the right panel (#621 check 4).
- `RA3` `resilience/right-redraw-after-wake`: an idle-off and a LEFT tap rewrite the panel.
- `RA4` `sitting/right-clean-wake-usb` (old R1–R2): on USB power, the keyboard idles (the PC told away), one LEFT tap: the right lit within 2 s, its whole-screen rewrite count up (the panel rewritten after its power came back: no noise survives a full rewrite), and its readback a frame of Bandit_Emblem. BLOCKED while the right is on its battery.
- `RA5` `sitting/right-clean-wake-battery` (old R3–R5): the same on the right's battery. No step unplugs the cable: BLOCKED while the right is on USB power, it runs the next time the right is found on its battery (`foxglove-lab usb`: "right is not on USB").

## Batch P: the PC state (#609, #604, PR #651)

On this PC the display going off starts Modern Standby, so nothing turns it off: the app's away
test hook (`FOXGLOVE_AWAY_OVERRIDE`, away.rs) stands in for Windows' display notification, and
everything after the edge is the app's real path. The hook arms only with `FOXGLOVE_APP_OK=1` (a
lab launch), so a stray variable in a user's run is ignored. Needs both halves on #651's firmware (HELLO bit
26) and the left on USB. `pc_away.py` runs PA1–PA4 in one go.

- `PA1` `pc_away.py` (a test app with the hook): on connect the app says "awake (keyboard connected)"; `foxglove-lab status`: `PC_STATE` held, reason 0 (the lease). With both halves' underglow off by the user's choice, the run turns it on for the run and puts it back.
- `PA2` (old P1–P3) the hook says "display off": "PC state to the keyboard: away (display off)" and `foxglove-lab rgb-watch` sees both halves dark within 2 s; then "display on": "awake, wake (display on)", both lit within 2 s, and the right's readback changes within 5 s (its art moves). Then "locked" alone tells the keyboard nothing and both stay lit (#609 decision 1), and "unlocked" sends "awake, wake (session unlocked)".
- `PA3` `foxglove-lab pc-state`: `state` Awake, `reason` 0, and the app's last PC-state line is awake, before the quit (#651's first try quit while the last state sent was away).
- `PA4` (old P4–P6) the test app is stopped (Quit sends nothing more), no key is pressed, and `rgb-watch` segments with `pc-state` reads between them (about every 2 minutes, up to 700 s): reason 0 (the lease) first, then the keyboard goes dark on its own and the hold ends (reason 4). Then the user's Foxglove starts again by its path. Input meanwhile makes the dark step BLOCKED, not FAIL. `--history` also lists the real display edges of the user's own nights from their foxglove.log (measured, never caused).

## Z: case zone 1 back to its colour

- `ZA1` `foxglove-cli lights` for the OMEN 45L's key and zone 1, then `foxglove-cli light <key> <zone> #a57500`: the answer lists zone 1 as `#a57500`. Done and read back on 10/29 (#90); not repeated.

## What's left

None of these is a step of this sitting, and none blocks 0.2. Each is mapped in
`tests/sitting-parity.toml` (`[[residual]]`).

1. **What the right's panel shows while it has no power** (old R1, R4: blank, frozen or noise).
   The ls0xx panel is write-only and its power is cut with the LEDs, so no readback can exist,
   and the PC has no camera on the keyboard. It isn't a pass/fail: it feeds #621's ext-power
   decision (keep the rail on while the LEDs are off). Whoever next glances at an idle keyboard
   can answer it on #90; nothing waits on it.
2. **One unplug of the right's cable, only if `RA5` never ran.** No lab can cut USB power. `RA5`
   runs by itself whenever the right is found on its battery (it was on 10/29); if it hasn't by
   the 0.2 cut, one unplug is the only hands step, and the case does the rest.
3. **Exclusive full-screen games (#586).** `HA5`'s stand-in is a borderless window; a game in
   exclusive DXGI full-screen can't be captured with GDI. If the user still sees the flicker in a
   real game after `HA5` passes, that report reopens it; it is not a step.
