# Knob: hide an app from the knob (UX plan)

Issue #952, UX gate step. The knob's click cycles apps with sound (#719, #854). Some apps the user
never wants there (a vendor control centre holding a stream) still come up. This plan is the
"Hide from knob" action, where the hidden list lives and how to undo it. The filters themselves
(part 1 and 3 of the issue) have no UI beyond one sentence in Settings.

Mockups (Python + Pillow, [`design/mockups/knob_exclude_mockups.py`](design/mockups/knob_exclude_mockups.py),
Foxglove's DARK tokens; the script fails under 4.5:1, lowest 5.4:1):
- `C:\Users\fuchs\splitkeeb\review\knob-exclude\ux-knob-exclude-proposed-tray-hide.png`: the
  card today, the tray panel's new Knob row with Hide from knob, and what both say after
- `C:\Users\fuchs\splitkeeb\review\knob-exclude\ux-knob-exclude-proposed-settings-undo.png`: the
  Knob card in Settings (Keyboard page) with the hidden list, Show again, and the empty state

## Research

What exists (read from `app/src/foxglove/knob.rs`, `knob_popup.rs`, `tray_panel.rs`,
`crates/info-screen/src/sessions.rs`):
- The knob's **card** (`knob_popup.rs`, "Flyout twin") is drawn on the overlay window,
  **click-through and never activating**, by design: it must not take a click or focus mid-game.
  So it can't carry a button without undoing #719's choice.
- **#854 already skips silent sessions:** the knob offers only sessions heard (peak above 0) during
  this run (`Heard`). Part 1a of #952 is therefore mostly done; what's left is "background
  process with no window" and the user's own list.
- Sessions carry the program's path (`icon`, else the exe) and pid, so a match by program file is
  possible and survives the app restarting (session ids don't).
- The **tray panel** (`tray_panel.rs`) is the interactive daily surface ("both halves, profile,
  lights …") with parts like Batteries, Profile, Screens; a Knob part fits its pattern.
- There is no Knob card in Settings today; the Keyboard page is where the knob belongs.

- **Reuse:** the tray panel's `part` rows and pills, the Settings card style (store-as-you-go),
  the card's existing "<app> stopped · knob: master" shape for the "hidden" note.
- **Emulate:**
  - **Windows notifications:** "Turn off all notifications for <app>" from the toast itself, undone
    in Settings > Notifications' per-app list. Act where you are annoyed, undo in one known place.
  - **Windows Volume Mixer:** apps by name and icon, matched by program, never by internal id.
  - **Gmail / Android "Undo" snackbar:** a reversible action gets Undo right there, briefly.
- **Not taken:** making the card clickable (steals clicks in games; a 2-second target is also hard
  to hit), a keyboard gesture on the knob (no free gesture: click, turn, double, hold are taken),
  a per-app "allow list" mode (one list is enough).

## Scope

In: the knob (`docs/scope.md`: the keyboard's controls doing the right thing on this PC). Generic:
any app, matched by its program file; the shipped list is data. Out: hiding apps from Windows'
own Volume Mixer or flyout, per-device lists, the left screen's volume line (it follows the knob's
target and needs no change), any app-specific code.

## Flow: hide

Pre-condition: the knob is on an app (a click found Logi Options+). On master the Knob row isn't
shown: there is nothing to hide.

1. User opens the **tray panel** (left click on the tray icon). The new **Knob** row shows the
   target: icon, name, "1 of 3 · 40 %", and a **Hide from knob** button.
2. User clicks **Hide from knob**. At once:
   - the knob moves to the next app (or master if none), as if clicked, and the card shows
     "Logi Options+ hidden" / "knob: Spotify · 2 of 2" (or "knob: master") for 3 s;
   - the Knob row turns into "Logi Options+ is hidden from the knob." with **Undo** and
     "Change in Settings". Undo stays until the panel closes.

Done. One click. Hidden takes effect for this session and every later one.

## Flow: undo

- Right after: **Undo** in the tray panel (one click). The knob does not jump back; the app is
  simply in the cycle again at its place.
- Later: Foxglove > Settings > Keyboard page > **Knob** card > the app's row > **Show again**. The
  row leaves the list and one line replaces it: "Discord is back in the knob's cycle." / Undo, until
  the page changes. No Save button; the next knob click uses it.

## The Knob card in Settings (Keyboard page)

- Title "Knob". Two lines: "A click on the knob goes to the next app that's playing sound." /
  "It skips apps that haven't played anything and apps on this list."
- Caption "HIDDEN FROM THE KNOB", then one row per program: icon, the app's name, a second line
  "Hidden by you · <date>" or "On Foxglove's list", and **Show again**. Tooltip: name, the file name
  ("LogiOptionsMgr.exe · matched by this file"), so two apps with one name can be told apart.
- **Add an app…** lists the apps with a session right now (the Volume Mixer's list), for hiding an
  app before the knob ever lands on it. One pick adds it.
- Empty: "No apps hidden. Use Hide from knob in the tray panel while the knob is on one."
- Foxglove's shipped entries (the seed, part 3) show in the same list with "On Foxglove's list" and
  can be shown again too; the user's choice wins and is stored as an override, so a later seed
  update doesn't hide it again.

## Data (for app-engineer)

- User list in Foxglove's settings: program files (normalised full path, plus file name as the
  fallback match), each with "hidden" or "shown" (an override of the seed) and the date.
- Seed: a data file shipped with the app, read only, keyed the same way. No app names in code.
- The "background, no window" filter: count hidden and tool windows as windows, so music players
  minimised to the tray (which keep a hidden window) aren't skipped. If the filter can't tell, the
  session is offered: a wrong skip is worse than one extra app (the user can hide it in one click).

## Wording

| Where | Text |
|---|---|
| Tray panel row | "KNOB" / <app> / "<n> of <m> · <level> %" / Hide from knob |
| After hide (tray) | "<app> is hidden from the knob." / Undo / Change in Settings |
| After hide (card, 3 s) | "<app> hidden" / "knob: <next app> · <n> of <m>" or "knob: master" |
| Settings card | "Knob" / "A click on the knob goes to the next app that's playing sound." / "It skips apps that haven't played anything and apps on this list." |
| List caption, row | "HIDDEN FROM THE KNOB" / "Hidden by you · <date>" or "On Foxglove's list" / Show again |
| After Show again | "<app> is back in the knob's cycle." / Undo |
| Empty | "No apps hidden. Use Hide from knob in the tray panel while the knob is on one." |

No session ids, pids or "session" in the UI. "Hide", never "exclude" or "block".

## Hand-off acceptance

- Window-free tests: the filter over a captured session fixture (heard, no window, user list,
  seed, seed override) gives the expected cycle.
- Hide from the tray model: the next click skips the app; Undo restores it; it survives a restart
  of the app (new session id) and of Foxglove.
- Strings test: no session id or pid in any drawn string; labels ≥ 4.5:1 in both themes.

## Open questions for the user (via the Hub)

1. Where "Hide from knob" lives: **tray panel Knob row** / a clickable knob card (hit-test only
   over the card for its 2–3 s, like the tray panel's hit region) / Settings only.
2. The "background, no window" filter: **ship it, but count hidden windows** (tray apps stay) /
   skip it and rely on "never played" plus the hidden list.
