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

## Addendum (11/10): sound mirrors (#1060) and one row per app (#1061)

Quick designer pass, docs only, built on the 11/10 research pass (session dump, meter reads and
logs on the user's PC). Nothing new is drawn and no mockups this round: the tray Knob row and the
Settings card keep their layout, and only the words below change.

### Research

- **#1060's cause:** LightStudio-background (audio-reactive lighting) and Wallpaper Engine's
  `wallpaper32.exe` (audio-reactive wallpaper) hold a **loopback** session: they listen to the
  speaker mix. Their session meter equals the device's mix on every read, so `Heard` counts them
  the first time any real app plays. Both own hidden windows, so the window test passes too.
- **#1061's cause:** on this occurrence Vivaldi had one session, in its audio-service child. The
  knob targeted it and the 8 % level was measured in the mix, so the turn worked. One row per app
  is still worth doing as hardening (several sessions of one app would be several rows today).
- **Emulate:** **EarTrumpet** (MIT) shows one row per app, keyed by the exe path or the package.
  A change goes to every session of the app, and the row shows the loudest. Its name comes from the
  session's display name, then the app's name, then the exe. **Windows' Volume Mixer** shows
  "active or recently active" apps by name and icon. Its grouping parameter is no use: every
  session here had a different one. Nobody filters mirrors. EarTrumpet and the mixer show
  everything because they are lists, but the knob is a cycle, so the mirror rule is Foxglove's own.
- **Reuse:** `program_name` (#1045) for the friendly name, `display_name`'s fallbacks, the hidden
  list's match by file name, and the peak read the knob already does per click (plus one read of
  the device's own meter).

### 1. Who is in the cycle (the rule as the user sees it)

**One sentence:** a click goes to the apps that have played sound, not to apps that only listen to
it.

- **Unchanged:** an app is in the cycle once it has played sound this run, as #854 does. The
  window test stays as built (hidden windows count), so a music player minimised to the tray stays.
  The earlier provisional "visible window only" pick is dropped: it would drop tray players.
- **New, a mirror:** an app whose sound level only follows what the speakers already play
  (lighting or wallpaper that reacts to music, a visualiser, a recorder) **never counts as
  played**, so it never joins the cycle. Foxglove tells it apart on the reads it already does: its
  level equals the device's mix while another app accounts for that sound.
- **What the user sees of a mirror: nothing. It is left out silently.** There is no "Not in the
  knob cycle" list, no card and no tray line. The research's optional tray line ("listens to the
  mix") is not taken, for three reasons. A mirror is never the knob's target, so the tray Knob row
  never shows it. A list of left-out helpers would change by the second and name things nobody
  asked to see. And Settings' **Add an app…** already lists every app with a session now, mirrors
  included, so the user can still hide one by hand if the rule ever misses it.
- **When a left-out app starts playing for real** (its level stops following the mix, say a
  visualiser that also plays its own sound), it counts as played on that read. It joins at the end
  of the cycle, and the next click reaches it. Nothing pops up when it joins.
- **The tie:** when a tray player and a mirror are the only things with sound, Foxglove can't tell
  them apart, so both stay offered. That's no worse than today, and Hide from knob handles it.
- **The hidden list wins:** an app the user hid never joins, playing or not.
- **A muted app or one at 0 %** still counts as played: its own meter isn't the mix.

### 2. One row per app

- **What a row is:** every session of one app is one row. An app is its package (Store apps), else
  its program's full path (any case). If Foxglove can't read the path (a protected game), the path
  comes from the session's own identifier. This covers Chromium browsers, Electron apps and
  Firefox, whose sound comes from a child of the same program.
- **A helper with a different program** (a WebView2 host under an app) joins its parent's row
  only when it has no app window of its own and the parent has one. **A child with its own
  window stays its own row:** a game started by Steam is the game, not Steam.
- **Row name and icon:** taken from the app's windowed (or top-most) process, **never the
  helper's**. The name is that session's display name if it set one, else the program's
  FileDescription, then ProductName (`program_name`, #1045), else the file name without `.exe`.
  For example "Vivaldi", never "vivaldi" or "Vivaldi (2 sessions)". **No count of sessions
  anywhere, and "session" stays out of the UI.**
- **Same name twice:** only if two rows in the cycle get one name (two installs, say Vivaldi and
  a Vivaldi snapshot in different folders), each adds its program's folder name:
  "Vivaldi · Vivaldi Snapshot".
- **The level it shows:** the loudest of its sessions at the last read (the one playing), else
  the first. **Muted** shows when the row's shown session is muted. The double-click mutes or
  unmutes all of them together.
- **A turn** moves **every** session of the row by the same 2 % detent, each from its own level,
  so the app's own balance is kept (EarTrumpet's way). The row reads the loudest, so what the user
  sees is what they hear.
- **Hide from knob on a grouped row** stores the app's windowed program file (vivaldi.exe), never
  a helper's: hiding msedgewebview2.exe would hide every app that uses it. A protected game whose
  path came from the session identifier can be hidden by that file name too.
- **The row lives** while any of its sessions lives, so the knob's target is the app, not one
  session. "n of m" counts rows (apps).

### 3. Wording (exact)

| Where | Text |
|---|---|
| Settings card, intro | "A click on the knob goes to the next app that's playing sound." / "It skips apps that haven't played anything, apps that only listen to the speakers, and apps on this list." |
| Row name, same name twice | "<app> · <folder>" |
| Tray row tooltip | "<app> · <file> · matched by this file" |

Everything else in the earlier Wording table is unchanged: "<n> of <m> · <level> %", "<app>
hidden", Undo, Show again and the empty state. No "session", "loopback", "mirror", "helper" or
counts in any drawn string. "Mirror" is a word for this doc and the code only.

### Hand-off acceptance (adds to the list above)

- Captured fixture from 11/10 (window-free; per-read peaks plus the device's peak): LightStudio-
  background and wallpaper32 are never played; Vivaldi and the game are; the cycle is Vivaldi,
  then the game.
- A muted or 0 % app playing while another app makes the mix is played. A tray player alone with
  a mirror: both offered.
- One app with a silent parent session and a playing child session gives one row. A step changes
  both and keeps their offset, and the row shows the playing one's level. The row lives on when
  one of the sessions ends.
- A windowless helper with a different program under a windowed parent joins it. A game under a
  launcher keeps its own row. Hide from knob stores the windowed program's file name.
- Strings test: no "session", "mirror" or "(2" in any drawn string; a same-name pair gets
  "· <folder>".
