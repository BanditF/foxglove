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

## Addendum (11/10): background helpers (#1060) and one row per app (#1061)

**Status: provisional.** The causes in #1060 and #1061 are still hypotheses. These picks are
drafts until the research (how EarTrumpet and the Windows mixer group and hide sessions, and the
Vivaldi cause from logs and a session dump) is in.

Quick designer pass, docs only. No mockups this round: nothing new is drawn. The tray Knob row and
the Settings card keep their layout, and only the words below change.

### Research

- **Why LightStudioBackground may get in (hypothesis, from the code):** `sessions::has_window` counts *any* top-level window,
  hidden ones included (this plan's "Data" section asked for that so tray players stay). A helper
  that owns one hidden or message-sized window passes. `Heard` then keeps a session for the whole
  run once its peak has gone above -80 dB even once, so a single blip is enough.
- **Emulate:** Windows 11 **Settings > System > Sound > Volume mixer** and the quick-settings mixer
  show **one row per app**, named and iconed by the app, with one slider for all of its streams.
  macOS / Windows **media flyouts** show an app only while it is playing. Foxglove takes both: one
  row per app, and background apps only while they play.
- **Reuse:** `program_name` (#1045) for the friendly name (FileDescription, then ProductName, with
  the over-long and `.exe` names skipped), `display_name`'s fallbacks, the hidden list's match by
  file name, and the peak read the knob already does on each click.

### 1. Background helpers: the rule as the user sees it

**One sentence:** a click goes to apps you can see that have played sound, plus any background
app that is playing right now.

- **An app with a visible window** (on screen or minimised to the taskbar) that has played sound
  this run is in the cycle, as today.
- **A background app** (no visible window of its own: tray-only, hidden, tool, message-only or
  cloaked windows don't count) **is in the cycle only while it's playing**, judged on the peak
  read the click already does. A tray-only music player that is playing is in. A helper that holds
  a silent stream or once blipped is out.
- **When a background app starts playing,** it joins at the end of the cycle and the next click
  reaches it. There is no card or toast when it joins (mid-game noise). If it goes quiet while
  the knob is on it, the knob stays on it (no jump) until the next click, which skips it if it's
  still quiet.
- **An app on the hidden list never joins,** playing or not. Hide means hide.
- **Left-out apps don't show anywhere.** No "Not in the knob cycle" list: it would change every
  few seconds and be a list of helper names nobody asked to see. The way to see them is the one
  that already exists: **Add an app…** in the Settings Knob card lists every app with sound now,
  helpers included, so the user can still hide one that leaks in. The user doesn't need a "Show
  again" for these, because they come back on their own when they play.
- Counting the window: owned by the app's process or another process of the same program file
  (as `has_window` does now), and it must be visible (`IsWindowVisible`, not cloaked, not a tool
  window, not message-only). A minimised window counts as visible. If it can't be told, the app is
  offered, as before.

This replaces this plan's earlier Data line, "count hidden and tool windows as windows". Tray
players are now covered by "while it's playing" instead.

### 2. One row per app

- **What a row is:** every session of one program file (the same full path, any case) is one row.
  So is every session of a helper process started by that app (the process tree, for apps whose
  sound comes from a differently named helper). The row's name and icon come from the **program
  that has the visible window**. If none does, they come from the top-most process of the app in
  the tree, **never from the helper** (a WebView2 or audio-service process).
- **Row name:** the app's friendly name, picked in this order: the windowed session's own display
  name, if it set one; else the program's FileDescription, then ProductName (`program_name`, as in
  #1045); else the file name without `.exe`. For example, "Vivaldi", never "vivaldi" or
  "Vivaldi (2 sessions)". **No count of sessions anywhere.** The word "session" stays out of the
  UI.
- **Only when needed:** if two rows in the cycle end up with the same name (two installs, say
  Vivaldi and a Vivaldi snapshot in different folders), each adds its program's folder name:
  "Vivaldi · Vivaldi Snapshot". The tray row's tooltip keeps the file
  ("vivaldi.exe · matched by this file").
- **The level it shows:** the level of the session that is playing (the loudest peak at the read).
  If none is playing, it shows the highest level among them. **Muted** shows only when every
  session is muted.
- **A turn:** the first detent sets **every** session to the shown level ± 2 %, and later detents
  move them together. So what the row shows is what the whole app plays at, as in Windows 11's
  per-app slider. A double-click (mute) mutes all of them; if some were muted, it mutes the rest
  first.
- **Hide from knob on a grouped row** hides the app by the **windowed program's** file name
  (vivaldi.exe), never by a helper's (hiding msedgewebview2.exe would hide every app that uses it).
- The "n of m" count counts rows (apps), not sessions.

### 3. Wording (exact)

| Where | Text |
|---|---|
| Settings card, intro (now 3 lines) | "A click on the knob goes to the next app that's playing sound." / "Apps that run in the background join only while they're playing." / "It skips apps that haven't played anything and apps on this list." |
| Row name, same name twice | "<app> · <folder>" |
| Tray row tooltip | "<app> · <file> · matched by this file" |
| Tray row level, all sessions muted | "<n> of <m> · muted" |

Everything else in the earlier Wording table is unchanged: "<n> of <m> · <level> %", "<app>
hidden", Undo, Show again and the empty state. No "session", "helper", "background process" or
counts in any row.

### Hand-off acceptance (adds to the list above)

- Fixture replay (window-free): a helper with only a hidden window and a once-blipped peak is
  left out; the same helper playing at click time is in, at the end; a tray-only player that is
  playing is in; a hidden-list app that is playing stays out.
- Fixture replay: one app with a silent parent session and a playing child session gives one row,
  named after the windowed program; one step sets both to shown ± 2 %; "n of m" counts the row
  once; Hide from knob stores the windowed program's file name.
- Strings test: no "session" and no "(2" in any drawn row; a same-name pair gets "· <folder>".
