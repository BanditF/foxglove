# P8 Legacy pages as panels (UX plan)

Phase 2 row P8 (v3 H3.2, `review/design-phase2-plan.md`). In module mode the Hub (hideout.exe)
shows the keyboard's state from the facts foxglove.exe publishes, and one way into Foxglove's own
settings window. Plan only, no code. Review image:
`C:\Users\fuchs\splitkeeb\worktrees\h-p8-ux\review\ux-p8-legacy-panels-proposed.svg` (drawn
mockup with the hub's theme tokens, not an egui render).

## Research and scope

- **Reuse:** the panel host as it is (`crates/hideout-panels`): `tile` and `page` slots, the
  hub-drawn tile frame (icon, title, ··· menu, module state, greyed cached view), `tone`, the
  panel cache (§2.4) and the supervisor's state words (`model::state_words`, `tray::TILE_*`,
  docs/hideout-supervisor-ux.md). The facts P5 already mirrors (`keyboard_facts.rs`,
  `hub_link.rs` `FACTS = "keyboard.*"`). Foxglove's own tray copy ("not connected", "The lights
  show here once the keyboard reports them.", "Art…" opens Screens) and P1's guard notice.
- **Emulate:** Windows 11 quick settings and the Bluetooth flyout (one glance line, one way to
  full settings); Foxglove's tray "Open Foxglove →".
- **In scope:** read-only summaries (three tiles, one page with three sections); one action per
  summary that opens Foxglove's settings window at the matching page; all module states.
- **Out of scope:** changing anything from the Hub (profile, effect, colour, brightness, art
  Next/‹, layer): that stays in Foxglove's window until each feature is its own module (P9, H4).
  No new controls in Foxglove, no tray in module mode (P5 keeps it off), no firmware change.
  Nothing hard-coded to one keyboard: names come from facts (systems, not products).

## Decisions

### 1. What the Hub shows

- **Three tiles** in the tray panel, in this order: **Keyboard**, **Keyboard lights**,
  **Keyboard art** (icons `keyboard`, `lights`, `image`). Declared in Foxglove's
  `module.toml` `[[ui.panels]]`, so the Hub draws them from the cache before the module runs.
- **One page**, sidebar name **Foxglove** (icon `keyboard`): a line "Your keyboard, its lights
  and its art, as the Foxglove app sees them." and three sections with the same three titles. A
  click on a tile outside its button opens this page (§5.1).
- Titles say "Keyboard lights" and "Keyboard art", not "Lights" and "Art": the Hub already has
  its own Lights page (PC lights) and a first-party Art module is planned; two "Lights" in one
  sidebar is the one confusion this phase can cause.

### 2. The one action

- Every tile and every page section has one button, **Open Foxglove settings**. It opens
  Foxglove's settings window at the matching page: Keyboard, Lights, Screens (art lives there).
  Accessible name adds the page: "Open Foxglove settings, Lights page".
- The button sends a plain `ui.event` to the module, which shows its own window (no contract
  change, works on hub 1.0). It is disabled while the module isn't live (the hub greys the body),
  so a window never pops up later from a queued click.
- When the module isn't running, the frame's own button replaces it (Start again, Turn on).

### 3. Facts: there today vs. to add

All new facts stay under `keyboard.*`, so the existing grant (`facts.write:keyboard.*`) covers
them and no first-party area (`art.*`, `lighting.*`, §6.3) is touched. Event-driven like the rest:
set on change, removed on disconnect (except `keyboard.connected`).

| Fact | Type | Today | Used for |
|---|---|---|---|
| `keyboard.connected` | Bool | yes | Keyboard tile line 1, every "Not connected" |
| `keyboard.link` | Text `usb`/`ble` | yes | "Connected over USB / Bluetooth" |
| `keyboard.profile`, `keyboard.profile_connected` | Int, Bool | yes | page "Bluetooth, profile 2" |
| `keyboard.<part>.battery` | Int % | yes | "Left 77 % · Right 91 %", page meters |
| `keyboard.layer`, `keyboard.layers` | Int, Bytes | yes | fallback "Layer 3" |
| `keyboard.game_mode` | Bool | yes | not shown (not a summary item) |
| `keyboard.lighting.on/effect/brightness/rainbow` | Bool, Text, Int, Bool | yes | Lights tile "Solid · 80 %", "Off" |
| `keyboard.lighting.color` | Text `#rrggbb` | yes | the swatch only (never shown as hex) |
| **`keyboard.name`** | Text | **add** | page heading tooltip, accessible name ("Eyelash Corne") |
| **`keyboard.layer_name`** | Text | **add** | page "Layer GAME" (the names Foxglove already remembers) |
| **`keyboard.lighting.color_name`** | Text | **add** | the swatch's words ("Orange", "Custom colour"): colour never stands alone |
| **`keyboard.lighting.source`** | Text `keyboard`/`windows`/`scene` | **add** | sub line "Windows Dynamic Lighting is drawing them" / "<scene> is running" |
| **`keyboard.art.name`** | Text | **add** | Art tile line 1 (`art_pieces::showing_name`) |
| **`keyboard.art.from`** | Text `built_in`/`kept`/`sent` | **add** | "Built-in set" / "Kept on the keyboard" / "Sent from this PC" |

Not a fact: **a Foxglove page stopped by its guard (P1)** is the module's own state, not the
keyboard's. The module patches its own tile and section (`ui.patch`, a `label` with tone
`attention`); no fact, no new grant.

### 4. States

The frame states are the Hub's, with the supervisor's existing words; the module draws only the
body. "Stale" means one thing: values from the cache while the module isn't live, always greyed
under a state line. There is no time-based staleness (nothing heartbeats, nothing polls).

| State | Who draws | Tile | Page section | Button |
|---|---|---|---|---|
| Module mode off (default) or Foxglove not adopted | nobody | no tiles | no page | n/a: Foxglove has its own tray and window |
| `off` (adopted, turned off) | hub | "Off" / "Foxglove runs on its own." | page "Foxglove: Off." | **Turn on** |
| `starting` | hub | "Starting…" over cached values, greyed | same | disabled |
| Running, first facts not in yet | module | "Waiting for the keyboard" / "<item> shows here once the keyboard reports it." | same | Open Foxglove settings |
| Connected | module | as image 1–3 | as the page in the image | Open Foxglove settings |
| Keyboard not connected | module | "Not connected" (tone `off`) / "Turn it on or plug it in." on all three | rows go, one line "Not connected" | Open Foxglove settings |
| Lights off | module | "Off" / "The keyboard's lights are off." | "Lights: Off" | Open Foxglove settings |
| Lights held by Windows or a scene | module | "Solid · 80 %" / "Windows Dynamic Lighting is drawing them" | same line under Colour | Open Foxglove settings |
| `restarting` | hub | "Restarting in 4 s (2 of 5)" over cached values, greyed | crash bar "Foxglove stopped unexpectedly. Restarting in 4 s (2 of 5)." | disabled |
| `crash-stopped` | hub | "Stopped after 5 crashes" / "Foxglove isn't running." | stopped card, recovery ladder | **Start again** (then the ladder) |
| `quarantined` / broken panel | hub | "Foxglove's page couldn't be shown." | same | **Start again** |
| A Foxglove page stopped (P1 guard) | module | normal values + "Foxglove's page stopped working." (`attention`) | same line in that section | Open Foxglove settings (Try again is there) |
| Hub restart / grace period | hub | cached values, "Starting…" | same | disabled |

### 5. Wording (exact)

| Where | Text |
|---|---|
| Keyboard tile | "Connected over Bluetooth" or "Connected over USB" / "Left 77 % · Right 91 %" (only the parts reported) |
| Keyboard tile, no battery yet | "Connected over USB" / "Battery shows once the keyboard reports it." |
| Lights tile | "<Effect> · <n> %" with swatch / "<colour name>"; rainbow: "<Effect> · <n> %" / "Makes its own colours" |
| Art tile | "<piece name>" / "Built-in set" · "Kept on the keyboard" · "Sent from this PC" |
| Not connected (all three) | "Not connected" / "Turn it on or plug it in." |
| Guard | "Foxglove's page stopped working." |
| Button | "Open Foxglove settings" |
| Page intro | "Your keyboard, its lights and its art, as the Foxglove app sees them." |

Sentence case, no ids, no "module", "fact", "module mode" or hex in the UI. Percent as Foxglove's
knob line has it ("80 %"). A long piece or colour name is cut on a whole character with "…"; the
full text is in the accessible name.

### 6. Keyboard and screen reader

- Tray: tiles in grid order Keyboard, Keyboard lights, Keyboard art (row-major, after the
  Hub's own tiles unless the user moved them); arrows move between tiles, Space presses the
  tile's button, the ··· menu as today. Each tile is one node: "Keyboard, Connected over
  Bluetooth, Left 77 %, Right 91 %"; the frame's state words come first when not live
  ("Keyboard art, Restarting in 4 s, 2 of 5").
- Page: heading "Foxglove", then each section heading, its rows (label then value), its button.
  Tab order follows. A state change is announced once per change (the frame's existing rule).
- Contrast: every text pair is ≥ 4.5:1 on its surface in both themes, computed (WCAG formula,
  as `hideout-shell::theme::contrast`) from the `LIGHT`/`DARK` tokens, not measured on a render:

  | Pair | Light | Dark |
  |---|---|---|
  | text / surface | 17.1 | 13.0 |
  | muted / surface | 7.3 | 5.8 |
  | accent (button text) / surface | 5.2 | 6.4 |
  | on_accent / accent (sidebar, Start again) | 5.2 | 8.1 |
  | danger / surface | 6.9 | 5.4 |
  | attention / surface | 6.2 | 9.2 |
  | ok (meter fill, not text) / surface | 6.3 | 8.4 |
  | warn / surface | **3.8** (not used for text) | 9.2 |

  The amber `warn` token is 3.8:1 on light surfaces, so the restarting line uses the
  `attention` tone (6.2:1 light, 9.2:1 dark), not `warn`. Greyed cached values are inactive
  content; their state line is full contrast.

## Hand-off acceptance (app-engineer)

- `module.toml` declares three `tile` panels and one `page`; a test keeps it equal to what
  `ui.register` sends (like the `SERVED` test). Standalone (no `--module`): nothing registered,
  nothing changes.
- The six new facts are set on change and removed on disconnect, with window-free tests over the
  fake board, plus a restart-parity case (facts come back the same after a module restart).
- Panel vectors under `panels/valid/` for each body state in §4; strings exactly as §5; a
  hideout-panels render test per state in both themes writes review sheets.
- The button is disabled while the module isn't live; a click while live opens the window at
  the right page (lab, On-call; never on the user's desktop during a sitting).

## Open questions for the Hub (recommendation first)

1. **How the button opens the window.** **(A, pick) plain `ui.event`** to the module: works on
   hub 1.0, no contract change. / (B) a contract minor: `opens: "window:<id>"` with three
   `[[ui.windows]]` entries; cleaner, but needs `ui.window` in the manifest, which is a new
   permission the user approves on the next adopt.
2. **Titles.** **(A, pick) "Keyboard", "Keyboard lights", "Keyboard art"** / (B) "Keyboard",
   "Lights", "Art" as the phase-2 table says; collides with Hideout's Lights page.
3. **One page or three.** **(A, pick) one "Foxglove" page** with three sections: one sidebar
   item while it is a legacy module. / (B) three pages; more sidebar for a transitional module.
4. **Art Next/‹ in the tile** (Foxglove's tray has them, and the tray is off in module mode).
   **(A, pick) not in P8**: read-only phase; the button opens Screens. / (B) add later as an
   `[[actions]]` entry, after P9.
