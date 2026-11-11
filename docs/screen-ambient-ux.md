# Screen glow: lights follow the screen's edges (UX plan, #496)

Issue #496, UX gate step. It covers the user-facing half of "screen ambient mode": the lights take
the colours at the edges of what's on screen, like a TV's ambient light. This plan says where the
mode lives, how the user allows it, what always shows while it runs, the gates the user sees, how
edges map to devices, smoothing, the default, every string, and accessibility. Capture itself
(DXGI Desktop Duplication, GPU downscale, the 20 updates a second cap) is the app-engineer's, as the
issue says.

Mockups (Python + Pillow,
[`design/mockups/screen_ambient_mockups.py`](design/mockups/screen_ambient_mockups.py), Foxglove's
DARK tokens; the script fails under 4.5:1, lowest 5.42:1). Copies are in
[`design/images/screen-ambient/`](design/images/screen-ambient/).
- `C:\Users\fuchs\splitkeeb\review\screen-ambient\ux-screen-ambient-proposed-1-consent.png`: the
  Scenes page with Screen glow picked, and the permission card after the first Start
- `C:\Users\fuchs\splitkeeb\review\screen-ambient\ux-screen-ambient-proposed-2-running.png`: the
  room map while it runs (edge strips, a line from each device to its edge), the Effect card with
  Smoothing and "Stop allowing"
- `C:\Users\fuchs\splitkeeb\review\screen-ambient\ux-screen-ambient-proposed-3-tray.png`: the tray
  icon's eye, the tooltip, the tray panel's Screen glow row, and that row while a gate holds it

## Research

What exists (read from `app/src/foxglove/scenes_ui.rs`, `scene_looks.rs`, `scene_room.rs`,
`tray.rs`, `tray_panel.rs`, `docs/user-guide.md` "Scenes", PRs #523 and #528):
- **Scenes page** (sidebar, after Lights): Room card (live room map in mm, drag a device, Reset
  positions), Looks card (Wallpapers / Effects / Yours), Effect card (each effect shows only the
  parameters it uses), Triggers card, Start / Stop, and one status line (`scenes_ui::status`:
  "Running: {name}", "Paused for {who}", "Stopping: putting the lights back…", "Not running. The
  map shows a preview.").
- **Effects are `f(position, t)`** over the room map's points (`crates/scene`). A screen-sampled
  colour per point fits that shape: the effect looks up the nearest edge sample instead of a formula.
- **Gates already in the scene scheduler:** away (locked, display off) and Foxglove paused; plus
  stepping aside for Windows Dynamic Lighting or another app (#491). Missing: a full-screen game.
  `live_feed.rs` already has a `fullscreen_now()` check to reuse.
- **Tray:** glyph variants (normal, off, paused), the tooltip's short status line ("Connected",
  "Paused"), the right-click menu with "Pause Foxglove" / "Resume Foxglove", and the tray panel's
  parts (Batteries, Profile, Lights, Knob).
- **"From wallpaper"** (palette) takes colours from the wallpaper's picture. It never reads the
  screen, so it needs no consent. Screen glow is the first feature that reads the whole screen.
- There is no Settings privacy section, and no other screen-reading feature to share a permission with.

**Prior art (described, not copied):**
- **Philips Ambilight:** LEDs on the TV's back take the colours at the matching edge of the
  picture. Settings: a style (follow video, audio or a colour), brightness and saturation.
  *Ambilight + hue* extends it to room lamps: the user says which side of the TV each lamp stands
  on, and the lamp takes that side's colour. **Emulated:** a device's place in the room picks its
  edge, with no per-LED setup.
- **Hyperion.ng** (MIT, open source): a grabber feeds edge regions mapped to LED positions from an
  LED layout editor (count per side, depth, gap, start corner). It has smoothing (settling time,
  update frequency) and black-border detection for letterboxed films. **Emulated:** edge regions to
  positions, smoothing, black-bar skipping. **Not taken:** its dozen layout and colour knobs.
  Foxglove already knows where every device is.
- **Govee DreamView / Govee Desktop:** a "sync to screen" mode for its PC light bars and TV strips,
  chosen like any other scene, with a whole-screen vs edge choice and a saturation control. Its
  TV kits use a camera. **Emulated:** it is chosen like any other light mode, not a separate app.
- **SignalRGB Screen Ambience:** an *effect* in its effect library. Each device takes the colour
  of the screen area under where it sits on SignalRGB's layout canvas. **Emulated:** the
  placement (an effect) and the mapping (the canvas = our room map). It is the closest pattern.
- **Windows Dynamic Lighting:** has no screen-following effect. Its pattern here is ownership: a
  foreground app can take the lights, and the background one steps aside. **Emulated:** Screen
  glow is a scene, so it yields like one ("Paused for Windows Dynamic Lighting").
- **Consent and indicator (not lighting):** macOS asks once for Screen Recording, revocable in
  Settings, and shows a menu-bar indicator while an app records. Windows Graphics Capture draws a
  yellow border around the captured screen. **Emulated:** ask once, take it back in one known
  place, and keep an always-visible indicator. **Not taken:** the yellow border, because an
  ambient light that frames the screen in yellow defeats itself. DXGI Desktop Duplication shows no
  OS indicator, so Foxglove must show its own.

## Scope

In (`docs/scope.md`: the lights around the keyboard and PC; the issue scopes it in as opt-in).
Generic: any light the scene lights today, placed by the room map, with any number of monitors.
Out: per-LED edge editors, per-app rules ("only in VLC"), audio sync, camera capture, recording
or streaming the screen, a separate "ambient" app or window. Reading only Wallpaper Engine's
window is also out for v1: the existing "From wallpaper" palette covers the wallpaper case (see
Questions).

## Placement: an effect on the Scenes page

**Screen glow** is one more entry in the Looks card's **Effects** list, last, after Tide effect.
It is not a separate mode, toggle or page.
- One owner of the lights: Start / Stop, the scheduler, leases, yielding, profiles ("Start with
  this profile"), "Save as look" and the status line all work unchanged. A separate ambient
  toggle would fight scenes for the same lights.
- SignalRGB and Govee make it a selectable mode or effect, too.
- **Not a palette source** ("From screen"): a palette feeds a moving effect, so Sweep coloured by
  the screen would neither move nor follow, and the consent would have to cover every effect.
- **Triggers** still apply: Ripple on key press and Sweep on layer change draw over the screen's
  colours. That is cheap, and it is the same rule as for every other effect.
- **Looks:** the user can save it ("Save as look" keeps Smoothing). Wallpaper looks never include
  it: a `foxglove.json` that names it is ignored for that look, so a wallpaper can't start screen
  reading.
- **"Keep running on the keyboard"** does not apply: with Foxglove closed, nothing reads the screen.
  The box is hidden while Screen glow is picked.

## Turning it on: the permission card

Default: **off and not allowed.** Picking Screen glow in the list reads nothing. The map shows a
grey still drawing, and the Effect card shows its one-line description.

1. User clicks **Screen glow** in Looks, Effects. The map greys its edge strips, and the Effect
   card says what it does.
2. User clicks **Start**. The first time only, the Effect card shows the **permission card** in
   place of its controls, and nothing starts.
3. User clicks **Allow and start**. The yes is stored (global, not per profile), the scene
   starts, the tray icon gets the eye, and the status line says it's reading the screen.
   - Or **Not now** (also Esc): the card closes, nothing runs, and nothing is stored. The next
     Start asks again.
4. Later Starts never ask.
5. **Take it back:** with Screen glow picked, the Effect card ends with "Foxglove may read your
   screen for Screen glow." and a **Stop allowing** link. Clicking it stops Screen glow if it runs
   and forgets the yes. The line then reads "Foxglove asks before reading your screen."

Rules:
- **Only a click on Allow and start says yes.** "Start with this profile", resuming the last
  look after a restart, the local API and plugins never show the card and never start Screen glow
  without a stored yes. Without one they don't start it, and the status line says
  "Screen glow didn't start: it needs your yes. Pick it and press Start."
- **No preview before the yes**, and none while stopped: the map's live strips are the running
  scene itself. Nothing reads the screen unless the eye is in the tray.
- **Focus** lands on **Not now**, so a stray Enter never agrees.

## Always visible: the eye

While frames are read (and only then), all three show:
- **Tray icon:** the fox glyph with a small eye badge, bottom right. Art hand-off: a `screen`
  variant next to `off` and `paused`, white and black, at 16, 20, 24 and 32 px. **Paused wins:**
  while Foxglove is paused the icon shows Paused, and nothing reads the screen.
- **Tooltip status line:** "Connected · Screen glow on" ("Disconnected · Screen glow on" without a
  keyboard). The battery line stays under it.
- **Tray panel:** a **Screen glow** row on top: the eye, "Reading your screen for the lights" and a
  **Stop** button. It is the Scenes page's Stop: the scene ends and the lights get their colours
  back. The panel's Lights part says "Scene: Screen glow".
- **Scenes page status line:** the eye mark plus "Running: Screen glow · reading your screen".

**Pause:** the tray menu's "Pause Foxglove" pauses Screen glow with everything else. No frames
are read while paused. "Resume Foxglove" picks it up again, and Screen glow gets no pause item of
its own.

**Notification:** none on a user Start (the user just clicked). When a profile switch or the
last-look resume starts it while Foxglove's window is closed, one Windows notification says
"Screen glow is on. Foxglove is reading your screen for the lights. Stop it from the tray."

## Gates the user sees

Each gate stops reading frames (not just sending them). The scene stays "running" and resumes by
itself when the gate lifts. While a gate holds it, the tray icon drops the eye, and the tray
panel's row stays with the gate's words.

| Gate | Lights meanwhile | Status line (Scenes page) and tray-panel row |
|---|---|---|
| A game is full screen | fade in 1 s to their own colours | Paused while a game is full screen |
| PC locked | as scenes today: nothing is sent | Paused while the PC is locked |
| Display off | as scenes today: nothing is sent | Paused while the display is off |
| Foxglove paused | as scenes today | Paused: Foxglove is paused. Resume it from the tray. |
| Another app or Windows has the lights | as scenes today (#491) | Paused for {who} (unchanged) |
| Capture fails (remote desktop, GPU or driver change) | hold, then fade in 3 s to their own colours | Can't read the screen right now. Foxglove tries again when the display changes. |

- **Full screen** means a full-screen game (Direct3D exclusive full screen, or Windows reporting
  "busy"/presentation mode). Full-screen video in a browser or player keeps glowing, because
  that is the point of an ambient light. If Windows can't tell the two apart, the app-engineer
  says so on the PR, and the Hub decides (see Questions). Borderless-window games are not
  detected; that is known and stated in the guide.
- **Protected video** (some streaming services) shows as black to other apps, so the lights go
  dim. That is a line in the guide, not a live status: Foxglove can't tell it from a dark scene.

## Mapping: the room map, nothing new

- Each lit point (a case zone, a keyboard half or LED, a LampArray lamp) takes the colour of **the
  nearest stretch of the nearest screen's edge** in the room map's millimetres. A keyboard below
  the main screen follows its bottom edge, the left half from its left part. A case beside the
  right screen follows its right edge, top to bottom. A device far from every screen still follows
  its nearest screen.
- The **sample depth** (how far in from the edge) is fixed, not a control: a strip of about 10 %
  of the screen's shorter side, as Ambilight and Hyperion's defaults do.
- **Black bars** (letterboxed films, 4:3 games) are skipped, so the lights follow the picture.
  This is always on, with no control.
- **To change which edge a device follows, drag it** on the Room card, as today. The running map
  draws the monitors' edge strips in their live colours, plus a thin line from each lit device to
  the stretch it follows. These lines show only while Screen glow runs.
- Monitors follow Windows' display layout and can't be dragged (unchanged). With no display read,
  Screen glow can't start: Start is disabled with the hover text "Foxglove can't see a screen yet."

## Smoothing

One control, three pills: **Quick · Even · Calm**. Default **Even**.
- Quick follows every cut (about 0.1 s), Even blends over about 0.3 s, and Calm drifts over about
  1 s. The exact constants are the app-engineer's, in this order.
- If Windows' "Animation effects" is off, a new Screen glow starts on **Calm**. That only sets the
  default: a user's pick wins.
- No other controls: no colour, palette, speed, direction, width, saturation or brightness (the
  lights' own brightness applies). The Effect card shows only what the effect uses, as #528 set.

## Accessibility

- **Flashes:** whatever the smoothing, a light never swings between dark and bright more than 3
  times a second (WCAG 2.3.1's three-flash limit). A strobing game or film becomes a steady pulse
  on the lights. This is always on.
- **Indicator not by colour alone:** the eye has a shape, and the tooltip and tray row say it in
  words. The gate rows differ in wording, not only in the eye's tint.
- **Screen readers (AccessKit):** the tray row reads "Screen glow, reading your screen for the
  lights, Stop button". The permission card is a group labelled by its title, with its buttons in
  reading order (Allow and start, Not now). The pills are a radio group named "Smoothing".
- **Keyboard:** everything is reachable by Tab, focus starts on Not now, Esc means Not now, and
  the pills move with the arrow keys.
- **Contrast:** every label is at least 4.5:1 on its ground (checked by the mockup script, lowest
  5.42:1).
- **Layout:** the permission card wraps at 720×420 and 880×600 at 100/125/150 %, under the existing
  layout guard (#464/#466).

## Wording (exact)

| Where | String |
|---|---|
| Looks, Effects list | Screen glow |
| Effect card, description | Your lights take the colours at the edges of your screens, like a TV's ambient light. |
| Permission card, title | Let Foxglove read your screen? |
| Permission card, body | To colour your lights, Foxglove looks at everything on your screens, up to 20 times a second, while Screen glow runs. |
| Permission card, point 1 | The picture never leaves Foxglove and is never saved. Only the edge colours go to the lights. |
| Permission card, point 2 | It pauses while a game is full screen, the PC is locked or the display is off. |
| Permission card, point 3 | While it runs, the tray icon shows an eye. Stop it there or here at any time. |
| Permission card, primary button | Allow and start |
| Permission card, secondary button | Not now |
| Permission card, footnote | Foxglove remembers your answer. You can take it back under this effect. |
| Effect card, caps label | Smoothing |
| Smoothing pills | Quick · Even · Calm |
| Smoothing hint | Quick follows every cut. Calm drifts between colours and never flickers. |
| Effect card, allowed | Foxglove may read your screen for Screen glow. |
| Effect card, link | Stop allowing |
| Effect card, not allowed (after Stop allowing) | Foxglove asks before reading your screen. |
| Room card, stopped | Not running. Screen glow shows your screen's colours here once it runs. |
| Room card, running hint | Drag a device to change which edge it follows. |
| Status, running | Running: Screen glow · reading your screen |
| Status, game | Paused while a game is full screen |
| Status, locked | Paused while the PC is locked |
| Status, display off | Paused while the display is off |
| Status, Foxglove paused | Paused: Foxglove is paused. Resume it from the tray. |
| Status, capture failed | Can't read the screen right now. Foxglove tries again when the display changes. |
| Status, no yes (profile or resume) | Screen glow didn't start: it needs your yes. Pick it and press Start. |
| Start, disabled hover (no display) | Foxglove can't see a screen yet. |
| Tray tooltip status | Connected · Screen glow on (or Disconnected · Screen glow on) |
| Tray panel row, title | Screen glow |
| Tray panel row, line | Reading your screen for the lights |
| Tray panel row, button | Stop |
| Tray panel row, held | the status strings above, from "Paused while…" to "Can't read the screen right now" |
| Tray panel, Lights part | Scene: Screen glow |
| Notification (started with the window closed) | Screen glow is on. Foxglove is reading your screen for the lights. Stop it from the tray. |
| Copy diagnostics line | Screen glow: allowed/not allowed · running/stopped/held ({gate}) · {n} updates/s |

The user-guide entry goes under Scenes, after Effect:

> **Screen glow** (Effects): your lights take the colours at the edges of your screens, like a
> TV's ambient light. Each device follows the screen edge nearest to where it stands on the Room
> map; drag it to change that. The first Start asks whether Foxglove may read your screen. The
> picture never leaves Foxglove and is never saved. While it reads, the tray icon shows an eye,
> and **Stop** is in the tray panel. **Stop allowing** (under the effect) takes the yes back.
> **Smoothing:** Quick, Even or Calm. It pauses during full-screen games (not borderless ones),
> while the PC is locked or the display is off, and with Pause Foxglove. Some streaming services
> show as black to other apps, so the lights stay dim during them.

## Hand-off

- **app-engineer (#496):** everything above, plus the issue's acceptance. Tests (window-free):
  - the permission gate: no stored yes means no capture from Start, profile, resume, API or plugin;
  - "Stop allowing" stops it and forgets the yes;
  - the eye on the tray only while frames are read;
  - each gate, with fake events;
  - the edge-to-point mapping on recorded frames, black-bar skipping and the flash limiter;
  - wallpaper looks can't start it;
  - the strings above (asserted, as `tray.rs` does).
- **art:** the tray `screen` variant (fox plus eye badge), white and black, at 16/20/24/32 px,
  matching the `paused` variant's badge placement.
- **UX review before merge:** `review\ui-scenes-screen-glow*.png` from the render harness (consent,
  running, each held state), plus the tray panel row.

## Questions (for the Hub; my pick first)

1. **Name:** **Screen glow** (pick: a noun like the other effects, plain words) / Ambient screen /
   Follow screen.
2. **Full-screen video:** **keeps glowing; only full-screen games pause it** (pick: the main use is
   films) / anything full screen pauses it (simpler and safer if Windows can't tell them apart).
3. **Permission:** **asked once, remembered until Stop allowing** (pick: like macOS's Screen
   Recording permission; the eye keeps it honest) / asked at every Start.
4. **Default:** **off and not allowed** (pick, as the issue recommends). Not a question unless the
   Hub wants it on by default; reading the whole screen should never start unasked.

Images for all four: `C:\Users\fuchs\splitkeeb\review\screen-ambient\ux-screen-ambient-proposed-1-consent.png`
(permission), `...-2-running.png` (map and smoothing), `...-3-tray.png` (indicator and gates), all in
`C:\Users\fuchs\splitkeeb\review\screen-ambient\`.
