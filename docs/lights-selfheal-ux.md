# Keyboard lights self-heal: UX plan

UX plan for the host self-heal of the design note `review/design-lights-resilience.md` (h-1159),
sections 0 and (d): a scene runs, Foxglove holds the keyboard's LampArray, and the keyboard
shows no light (the 11/13 incident). Plan only, no production code. Review image (drawn with
Pillow from `theme.rs`'s tokens, not an egui render): `review/ux-lights-selfheal.png`, made by
`python review/ux-lights-selfheal.py`, which also prints the contrast table below.

## Research and scope

- **Reuse:** the Keyboard health card (`health.rs` `draw`: "OK" / "Warning" / "Failed" + `item:
  detail`, worst first), a feature row through `health::feature_rows` like
  `lamp_health::loss_row`, and its notification through `health_notice` with the row's own
  `notice` title and body (#814, once a day per item). The Scenes page status line
  (`scenes_ui::status`, `Tone::Ok`/`Warn`) and its Stop button. foxglove-cli's existing output
  style (`<thing>: <state>` lines, errors as `foxglove-cli: <msg>` on stderr, exit 1).
- **Emulate:** Windows' network troubleshooter pattern (it notices, tries the fix itself, and
  only asks the user when that fails); `systemctl status`/`is-active` style exit codes that a
  script can branch on without parsing text.
- **In scope:** the health row and its states, the Scenes page status line and buttons, the
  notification, a11y, contrast, `foxglove-cli scene [status|stop|repaint]` text and exit codes.
- **Out of scope:** the heal logic itself (design note (d)), the firmware lease (b), firmware
  scenes (a), a new tray panel row, the tray icon tooltip (stays battery only), a settings
  switch for the heal (none: it only rewrites what the scene already draws).

## Words

- The user-facing noun is **keyboard lights**, the verb **put back** (automatic) and **Repaint**
  (the user's button). Never "LampArray", "hold", "re-take", "owner", "frames", "Dynamic
  Lighting controller" in the app. The CLI's last line (`detail:`) may use them: it is On-call's.
- "Dark" means: a scene runs, Foxglove holds the lights, the keyboard reports them off, and it
  isn't unpowered or dimmed for battery. A scene paused for another app is not dark.
- Times are local `hh:mm` (24 h, as the rest of the app). Counts are digits ("2 times").

## Where it sits

1. **App page, Keyboard health card:** one feature row, item **"Keyboard lights during scenes"**,
   subsystem `lighting.heal`. Rows in that card have no buttons, so the Failed row names the one
   action and where it is.
2. **Scenes page, scene status:** the status line under the look gains a dark suffix, and
   **Repaint now** appears next to **Stop** only while the keyboard is dark. This is where the
   user already starts and stops the scene, so both ways out sit together.
3. **Notification:** only for Failed, through the existing health notice (at most once a day).
4. **Tray:** nothing new. The tray panel's lights card and the tooltip stay as they are; the
   notification is the tray-level signal.

## States

| State | When | Health row (level · detail) | Scenes status line (tone) | Buttons |
|---|---|---|---|---|
| No row | no scene has run since Foxglove started, or no keyboard LampArray is held | none | as today | Stop (as today) |
| Healthy | a scene ran, never dark | OK · "never went dark since Foxglove started" | "Running: <look>" (ok) | Stop |
| Dark, waiting | dark under 2 min (`N_DARK`) | unchanged | unchanged | unchanged |
| Putting back | dark 2 min; repaint, then re-take, running | OK · "went dark, putting the lights back now…" | "Running: <look> · keyboard went dark, putting the lights back…" (warn) | Stop · **Repainting…** (disabled) |
| Recovered | put back by repaint or re-take | OK · "went dark <n> times since start, put back each time (last at hh:mm)" (n = 1: "went dark once since start, put back (at hh:mm)") | "Running: <look>" (ok) | Stop |
| Keeps going dark | 3 or more recoveries in the last 60 min | Warning · "went dark <n> times in the last hour, put back each time (last at hh:mm)" | "Running: <look>" (ok) | Stop |
| Not recovered | still dark after the re-take; backoff armed | Failed · "dark since hh:mm and Foxglove couldn't put the lights back. Click Repaint now on the Scenes page." | "Running: <look> · keyboard lights dark since hh:mm" (warn), then a muted line: "Repaint now sends the scene's colours again. Stop gives the keyboard its own lights back." | **Repaint now** (primary) · Stop |
| Repaint now pressed | user click or `scene repaint` | as Putting back | as Putting back | Stop · **Repainting…** (disabled) |
| Repaint now didn't help | still dark 10 s after the click | Failed · "dark since hh:mm; Repaint now at hh:mm didn't help. Click Stop on the Scenes page to get the keyboard's own lights back." | "Running: <look> · keyboard lights still dark after Repaint now" (warn) + the same muted line | **Stop** (primary) · Repaint now |
| Stopped | user Stop | row keeps its counts (OK/Warning by history); "dark since" is cleared | "Stopping: putting the lights back…", then as today | Start |

Notes:
- Putting back stays OK on purpose: it is Foxglove doing its job for at most about a minute,
  and a Warning would raise a notification for something already being fixed.
- After a Failed state recovers (backoff try, Repaint now, or the keyboard coming back by
  itself), the row goes to Recovered or Keeps going dark; the notification is not withdrawn
  (Windows toasts aren't), and the next one waits a day as for every health item.
- The counts are since Foxglove started (restart parity: derived, they start at 0 after a
  restart and the row says "since start"; "dark since" is re-derived from the next lit report).

## Notification (Failed only)

`HealthRow.notice`:
- Title: **"Keyboard lights stayed dark"**
- Body: **"A scene is running but the keyboard shows no light. Open the Scenes page and click
  Repaint now."**

## Buttons

| Button | Shown | Does | Hover text | Accessible name |
|---|---|---|---|---|
| Repaint now | Scenes page, while Not recovered (primary) or Repaint now didn't help (plain) | `scene.repaint`: clears what Foxglove thinks is shown and sends the scene's colours again (design note (d) step 2) | "Sends the scene's colours to the keyboard again" | "Repaint the keyboard lights now" |
| Repainting… | while a repaint or re-take runs (disabled) | nothing | "Foxglove is putting the lights back" | "Repainting the keyboard lights" (disabled) |
| Stop | as today | as today (`scene.stop`) | as today: "Stop the scene and put the lights back" | "Stop the scene" |

Only one primary button at a time: Repaint now first; after it failed once, Stop becomes the
primary, since the user's next step is to get light back at all.

## Keyboard and screen reader

- Health row: as every row, one label "Keyboard lights during scenes: <detail>" after the level
  word; the level is always a word, never colour only.
- Scenes status line: a polite live region, announced once per state change ("Keyboard went
  dark, putting the lights back", "Keyboard lights dark since 14:05"); it takes no focus.
- Tab order on the Scenes page: Repaint now (when shown), then Stop, then the rest as today.
  Repaint now appearing does not move focus.
- After a Repaint now click the button turns into Repainting… and keeps focus; when the
  result comes, focus stays on the same spot (the button), whatever its new label.

## Contrast

WCAG 2.x, computed from `theme.rs` `dark()`/`light()` and the bundled accent (`bandit.json`),
not measured on a render. Every pair the plan uses is at least 4.5:1:

| Pair | Dark | Light |
|---|---|---|
| text / surface (row detail) | 12.9 | 13.6 |
| muted / surface (hint line, "Checked …") | 5.9 | 6.0 |
| ok / surface ("OK", "Running") | 8.0 | 5.3 |
| warn / surface ("Warning", dark status line) | 9.4 | 5.4 |
| danger / surface ("Failed") | 5.0 | 5.9 |
| on_accent / accent (primary button) | 7.7 | 5.0 |
| text / control (plain button) | 10.8 | 11.5 |
| text / raised, muted / raised (notification) | 11.8 / 5.4 | 11.5 / 5.1 |

## foxglove-cli scene

New command `scene` with `status` (the default), `stop` and `repaint`; API methods
`scene.status`, `scene.stop`, `scene.repaint` (design note (d)). `lighting.write` is already
in the CLI's permissions; `status` needs only `devices.read`. Output goes to stdout; errors to
stderr as `foxglove-cli: <msg>`, as today. Help: the unknown-command list gains
`scene [status|stop|repaint]`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | done, or the lights are lit, or no scene runs (nothing to heal) |
| 1 | error: Foxglove not running, API error, bad argument, nothing to act on (as every command today) |
| 2 | `status` only: dark now, and Foxglove is putting it back (Dark, waiting or Putting back) |
| 3 | dark and not put back: `status` in Not recovered, or `repaint` that left it dark |

So On-call branches on 3 only, and 2 means "check again in a minute".

### `foxglove-cli scene` / `foxglove-cli scene status`

Lines, in this order; `went dark:` only after a scene ran since start; `next:` only with exit 3;
`detail:` always when a keyboard LampArray is held.

```
scene: Bandit scanline (running)
keyboard lights: lit
went dark: 2 times since start, put back each time (last at 14:05)
detail: held over USB, owner USB, takes 14, hand-backs 13, frames drawn 5120 / sent 5123
```
exit 0.

```
scene: Bandit scanline (running)
keyboard lights: DARK since 14:03, putting them back
went dark: 3 times since start, 2 put back
detail: held over USB, owner USB, takes 15, hand-backs 14, frames drawn 0 / sent 5402
```
exit 2 (Dark, waiting says `DARK since 14:03, putting them back at 14:05`).

```
scene: Bandit scanline (running)
keyboard lights: DARK since 14:05, next try at 14:35
went dark: 3 times since start, 2 put back
next: foxglove-cli scene repaint, or foxglove-cli scene stop
detail: held over USB, owner USB, takes 15, hand-backs 14, frames drawn 0 / sent 5402
```
exit 3.

Other first lines, exit 0: `scene: none running`; `scene: Bandit scanline (paused for <who>)`
(no `keyboard lights:` verdict, Foxglove doesn't draw then); `scene: Bandit scanline (stopping)`.
Without a keyboard: `keyboard lights: no keyboard connected`. Unpowered: `keyboard lights: off
(the keyboard turned them off: <reason>)`, reason as the firmware gives it ("USB power off",
"battery low"), exit 0, not counted as dark.

### `foxglove-cli scene repaint`

Waits for the result (the next lit report, or one bounded read 10 s later, event-gated, as the
heal does), then prints:

```
scene: repainted Bandit scanline
keyboard lights: lit
```
exit 0.

```
scene: repainted Bandit scanline, but the keyboard is still dark
next: foxglove-cli scene stop
```
exit 3.

Errors, exit 1: `foxglove-cli: scene repaint: no scene is running`;
`foxglove-cli: scene repaint: the scene is paused for <who>; Foxglove doesn't draw over it`;
`foxglove-cli: scene repaint: already putting the lights back` (a repaint or re-take runs).

### `foxglove-cli scene stop`

As the Scenes page's Stop; waits until the stop is done (bounded 10 s):

```
scene: stopped Bandit scanline; the keyboard's own lights are back
```
exit 0. Nothing running: `scene: none running; nothing to stop`, exit 0 (stop is idempotent).
Not done in 10 s: `foxglove-cli: scene stop: still stopping after 10 s`, exit 1.

## Hand-off acceptance (app-engineer)

- Window-free tests: one per state row above, checking the health row (level and exact detail),
  `scenes_ui::status` text and tone, and which buttons show (`click(&view, "Repaint now")` →
  `Intent::Repaint`), in the style of `scenes_ui` and `lamp_health` tests.
- Snapshot tests for the CLI text and exit code of each block above (pure functions like
  `art_text`, from `scene.status` fixtures).
- A render test writes review PNGs of the Scenes status in Putting back and Not recovered and
  the health card with the Failed row, in both themes (replaces this drawn mockup).
- No new timer for the UI: the row and the status line change on the heal tracker's events only.

## Open questions for the Hub (pick first)

1. **Recovered by itself: OK or Warning?** **(A, pick)** OK with counts; Warning only at 3 or
   more in an hour (no notification for a fix that worked). / (B) Warning from the first
   recovery, as the design note's sketch says (one notification a day while the §0 fix beds in).
2. **Repaint now visibility.** **(A, pick)** Only while dark. / (B) Always while a scene runs, as a
   plain button (a manual "my lights look wrong" fix, more clutter).
3. **Exit code 2 for "putting back".** **(A, pick)** Separate code 2, so scripts can wait. / (B) 0
   until Foxglove gives up (simpler; only 3 matters).

## Not verified

- Drawn mockup: Segoe UI, not egui's font; widths estimated.
- The firmware's unpowered reasons' exact words (D7 83 flag 5, battery dim) were not traced.
