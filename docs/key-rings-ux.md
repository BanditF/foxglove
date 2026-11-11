# Key-press rings on the keyboard (UX plan)

Part of #490, UX gate step for the app side of PR #1109. The firmware can now draw a ring from
any pressed key by itself (SET_EFFECT kind 5, capability KEY_RINGS): Foxglove sends the ring
colour, background, width and time once, and each half does the rest with no PC involved.
Without the capability, Foxglove keeps #489's app-drawn ripple. The firmware side is dev-only
(off in GA), so for now almost every user gets the app-drawn path.

This plan decides where the feature lives, its controls and wording, what shows when the keyboard
can't draw the rings, and whether the user is told who draws them.

Mockups (Python + Pillow, [`design/mockups/key_rings_mockups.py`](design/mockups/key_rings_mockups.py),
the Hideout mockups' dark tokens; the script fails under 4.5:1, lowest 4.95:1):
- `C:\Users\fuchs\splitkeeb\review\key-rings\ux-key-rings-proposed-effect-card.png`: the Scenes
  Effect card with Ripple and the new "Rings start" row, the hovers, and the right-half line
- `C:\Users\fuchs\splitkeeb\review\key-rings\ux-key-rings-proposed-triggers.png`: the Triggers
  card when the keyboard draws the rings (A) and when Foxglove does (B)

## Research

What exists (read from `scenes_ui.rs`, `scene_looks.rs`, `scene_triggers.rs`, `scene_run.rs`,
`lights_palette.rs`, PR #1109, #489, #1095):
- **Scenes > Effect** has nine effect pills in one radio group (#1095); **Ripple** is one of them:
  "Rings of colour grow from the keyboard", a loop from the keyboard's middle, with Colour,
  Background (Night/Off), Speed (1 to 10, a ring's time 12 s to 1 s) and Width (20 to 400 mm).
- **Scenes > Triggers** has "Ripple on key press" (#489): a ring of the scene's colour from each
  key, over whatever look runs, drawn by the app from the key report, 1.2 s and 60 mm fixed.
- The built-in look **"Ripple from the keys"** is today's loop from the keyboard's middle. Its
  name already describes the key-press mode.
- **Precedent for "who draws":** since #542 a scene's effect runs on the keyboard (SET_EFFECT)
  when the firmware can, else Foxglove sends frames. The page never says which; the only place it
  surfaces is "Keep running on the keyboard" (its hover: "Needs keyboard firmware that draws
  effects") and the diagnostics line in `scene_run.rs`.
- **The Lights page** offers the keyboard's own effects plus a palette's slow effects, and those
  run as scenes (`scenes_ui::start_look`). Lights has no ripple.
- Kind 5 is a whole effect: background plus rings. It can't sit under a scanline the way #489's
  trigger does. So it is a look, not a trigger.
- On the keyboard, each half draws rings only from its own keys (no split traffic per press), so
  a ring from a left key stays on the left half. #489's app-drawn ring crosses the room.

- **Reuse:** the Ripple effect's controls as they are (swatches, Night/Off pills, Speed and Width
  sliders), `theme::pill_group` for the one new row, the built-in look list, the Triggers card.
- **Emulate:**
  - **Razer Synapse / Corsair iCUE:** a key-press ripple is an effect in the same list as the
    looping ones (Synapse "Ripple", iCUE "Type ripple"), with the same colour and speed controls.
    Neither tells the user whether the keyboard or the PC draws it.
  - **QMK RGB Matrix:** "reactive" effects are plain modes next to the cycling ones; the
    keyboard-side rendering is an implementation detail.
  - **Foxglove's own SET_EFFECT path (#542):** draw on the keyboard when it can, quietly.
- **Not taken:**
  - **A tenth effect pill "Key rings":** to most people a key ring holds keys. It would also add
    a second ring effect with the same controls as Ripple.
  - **An "on-board" badge on the pill:** nothing for the user to do with it, it would be missing
    for every GA keyboard, and no other effect carries one.
  - **Making #489's trigger draw on the keyboard:** kind 5 replaces the scene's effect, so the
    trigger would silently turn every look into a still background.
  - **A Lights-page entry:** key rings need a colour, background and room position, which are the
    Scenes page's; Lights' palette effects already open as scenes.

**Scope:** In (`docs/scope.md`: per-key lighting and scenes). No new settings section beyond one
field on the look.

## Picks

1. **Where:** Scenes > Effect, inside **Ripple**. A new radio group **"Rings start"** under the
   effect pills, shown only for Ripple: **"On a loop"** (default, today's Ripple, unchanged) and
   **"On each key press"** (the new mode). No new effect pill, no badge.
2. **Controls:** the ones Ripple already has, same order, same ranges: Colour (swatches) = the ring,
   Background (Night/Off) = the background, Speed (1 to 10) = how fast a ring grows and fades (the
   ring's time, `period_ms`), Width (mm) = the ring's width. No separate "duration" control: a
   ring's time is what Speed already sets for the loop.
3. **Built-in look:** "Ripple from the keys" becomes the key-press mode: speed 8 (1.6 s), width
   60 mm, Bandit orange on Night. A user's saved looks keep their mode ("On a loop" for every look
   saved before this).
4. **Who draws it:** the keyboard when it reports KEY_RINGS, else Foxglove (#489's path: rings
   from the key report over the still background). **Same look and same controls either way; the
   page doesn't say which**, with two exceptions below where it changes what the user gets.
5. **The rest of the room:** the scene's other lights show the background, and Foxglove draws the
   ring across them from the key report in both cases, as #489 does today. A ring from a left key
   doesn't cross to the right half when the keyboard draws it; accepted, not explained (see the
   question below).
6. **"Ripple on key press" (Triggers)** stays for every other look. While the look is Ripple on
   each key press it shows **ticked and greyed**, with a visible reason line; the saved value is
   untouched and comes back when the look changes.
7. **"Keep running on the keyboard"** (Triggers) is where "who draws" matters: after quitting,
   only a keyboard that draws the rings itself keeps them. When it can't, the box is greyed with
   a reason line (unlike Screen glow, which hides it: this one can work after a firmware update).
8. **Right half on older firmware** (#1109's reply says right = 2): the left draws rings, the
   right keeps its own lights. One line in the Effect card with a link to the firmware update.

## States

| State | When | Effect card | Triggers card |
|---|---|---|---|
| A | KEY_RINGS, both halves current | controls only | "Ripple on key press" greyed + reason; "Keep running" works as today |
| B | no KEY_RINGS (every GA build, older firmware, another keyboard) | controls only | as A, plus "Keep running" greyed + reason |
| C | KEY_RINGS, right half older (reply right = 2) | the right-half line + "Update firmware" | as A |
| D | no scene running | controls; the map preview shows a demo ring (below) | as A or B |
| E | look is not Ripple on each key press | no "Rings start" row (or row on "On a loop") | today's card, unchanged |

Paused, away, a game holding the lights, Windows Dynamic Lighting owning them: as for any scene
effect today (#542); nothing new.

**Map preview (state D and while running):** the room map can't wait for key presses. It shows
one demo ring from a home-row key (left index finger's) every ring's time plus 1 s, so the
controls' effect is visible. With Windows' animation effects off, it shows one still ring
half-grown and doesn't animate (the page's existing `next_frame` rule gains this case).

## Strings

| Where | String |
|---|---|
| Caption (caps label, radio group name) | `Rings start` |
| Pill 1 | `On a loop` |
| Pill 1 hover / description | `One ring after another from the keyboard's middle` |
| Pill 2 | `On each key press` |
| Pill 2 hover / description | `A ring grows from each key you press. Foxglove never sees what you type.` |
| Ripple pill hover (changed) | `Rings of colour grow from the keyboard, on a loop or from each key you press` |
| Triggers, greyed box reason | `The Ripple effect already starts a ring at each key.` |
| Triggers, "Keep running" reason (state B) | `This keyboard can't draw key-press rings by itself, so they stop when Foxglove quits.` |
| Effect card, state C | `Rings show on the left half only. The right half needs a firmware update.` |
| Effect card, state C link | `Update firmware` (opens the existing firmware update page) |
| Built-in look name | `Ripple from the keys` (kept) |
| Diagnostics line (Copy report) | `keyboard: key rings drawn on the keyboard` / `keyboard: key rings drawn by Foxglove (no KEY_RINGS)` |
| User guide, Effect list | add: `Ripple can start its rings on a loop or from each key you press.` |

"Never sees what you type" is true on both paths: the keyboard path sends Foxglove nothing per
press, and the app path gets the key's position only (#489). No string says "on-board", "KEY_RINGS"
or "SET_EFFECT" outside Diagnostics.

## Accessibility

- "Rings start" is a `theme::pill_group` (#1095): an AccessKit radio group named "Rings start",
  two radio buttons with checked state and position (n of 2); Tab enters at the picked pill,
  arrows move and pick, Home/End, Space/Enter.
- The pill hovers are also the radio buttons' descriptions (screen readers get them, not only
  mouse users).
- The greyed boxes' reasons are **visible lines** under the box (not hover only) and are set as
  the box's AccessKit description, so a screen reader says why it is unavailable.
- Sliders keep their visible label and spoken value ("8 of 10", "60 mm").
- Text pairs meet 4.5:1 (mockup check); the state C link uses the accent on the card surface,
  as other links do.
- Motion: the map preview follows Windows' animation-effects setting (still ring when off). The
  keyboard's own rings are what the user picked and start only from their own presses; nothing
  flashes on its own.
- Order in the Effect card follows reading order: effect, Rings start, Colour, Background, Speed,
  Width, save row.

## Hand-off to app-engineer (acceptance)

- `Look` gains `starts: Loop | KeyPress` (serde default `Loop`); old configs load unchanged.
- "Rings start" row only for Ripple, via `theme::pill_group`; tests for the tree (radio group,
  names, checked, n of 2) beside `theme_preview::pill_groups`.
- KeyPress + KEY_RINGS: `EffectKind::KeyRings` sent once on start and on any control change
  (re-sends keep growing rings, per #1109); the room's other lights get the background and the
  app-drawn ring from the key report.
- KeyPress without KEY_RINGS: still background plus #489's ring (with the look's colour, width,
  speed instead of 1.2 s / 60 mm) on every light the scene lights, keyboard included.
- Triggers card: the greyed "Ripple on key press" with its reason; "Keep running" greyed in
  state B; saved trigger values untouched.
- State C line from the D7 8F reply's right = 2, cleared when it changes.
- Window-free tests for states A to E (fake capability and reply), and the preview's
  reduced-motion still frame.
- User guide line; Diagnostics line.

## Question for the Hub

1. **Should Foxglove's own (fallback) rings also stay on the pressed half, so both paths look
   the same?** Options: (a) **no, keep #489's ring across the room** (recommended: looks better,
   and the keyboard path is dev-only today; the difference shows only on a split keyboard with
   KEY_RINGS); (b) yes, clip the app's ring on the keyboard to the pressed half.
   Image: `C:\Users\fuchs\splitkeeb\review\key-rings\ux-key-rings-proposed-effect-card.png`.

**Decided 11/11 (designer's pick, standing rule): (a)** — fallback rings keep #489's ring across the room.
