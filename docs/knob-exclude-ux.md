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

## Master volume on a device that ignores Windows volume (#1064)

Some output devices take Windows' master volume into their own hardware, and sometimes don't apply
it. The first one we know of is a JBL headset's Xbox-protocol USB dongle, "Headphones (Xbox
Controller)". In that state the knob, the volume keys and Windows' slider all move the number, but
the sound stays the same. App volumes still work. The user wants master to work, with Windows' own
controls and no hooks.

Engineering design (research 11/10, Hub 11/10):
- While on, Foxglove holds the device's own volume near the top (one key step below it, §2). Master is a level Foxglove keeps,
  applied by turning every app on that device down by master's amount.
- Each app's own level is stored and restored exactly; a write-ahead journal covers crashes.
- **One master:**
  - Foxglove listens for the device's volume-change notification (event-driven, no polling).
  - A flyout drag or a volume key press is taken as the new master. The device is put back near the top
    in the same callback.
  - Foxglove's own writes are recognised by their event context and ignored.
  - So the knob, Windows' slider and the volume keys all move the same master.
- Master mute mutes the apps and marks them; unmute unmutes only the marked ones.
  - Turned off or quit while master is muted (#1070): the marked apps stay muted, and the device
    stays muted. A device that ignores its volume may ignore its mute too, so unmuting the apps
    would be louder; keeping them muted is the quieter choice. When Foxglove sees the device
    unmuted (at once while it runs, else at its next start) it unmutes the marked apps, as
    master's unmute would have. The user's own mutes stay.

This replaces the earlier hint-only plan: when master works, there is nothing to hint.

- **Reuse:**
  - the master card as it is (`knob_popup.rs`), and its tab place above Windows' flyout;
  - the Knob card in Settings (`knob_hide.rs`, Keyboard page) and its store-as-you-go rows;
  - the tray panel's Knob row (`tray_panel.rs`);
  - the seed-file pattern of `app/resources/knob-hidden.toml`.
- **Emulate:**
  - **WinSoftVol:** the same scaling, with "force software volume" per device.
  - **EarTrumpet:** per-app levels are the app's own.
  - **Windows Sound settings:** devices by their Windows name, and the wording of "Allow
    applications to take exclusive control of this device".
- **Scope:**
  - In: master volume for any output device whose volume Windows hands to the hardware (systems, not
    products; the dongle is the first entry of a data list).
  - Out: drivers, audio effect plug-ins (APOs), virtual devices, hooks, Bluetooth's registry switch,
    changing any Windows setting for the user, and detecting the fault by measurement (not possible:
    the research, §1).

### 1. The master card: the same as on any device

- **Every knob master card on a device Foxglove handles looks exactly like today's:** "Master volume",
  the level, the bar, and "Master volume muted". There is no extra line: it works, so it says nothing.
- **A change from Windows' flyout or the volume keys** (another keyboard, or the knob on firmware
  without #855): the sound follows at once, and Foxglove shows its master card with the real level for
  2 s, **directly above Windows' flyout** (where the "Knob · Master" tab sits). For mute, it shows
  "Master volume muted".
  - The card gets a third row: it grows from 48 to 64 pt upwards, 10 pt, `DIM`, at most 212 pt wide:
    **"Windows' slider goes back near the top."**
  - This row shows on the first outside change of each Foxglove run only. After that, the card alone
    shows the level: the user has learnt why the slider is near the top.

### 2. What Windows' flyout shows after a drag or a key press

The device's volume has to stay near the top so that it never turns the sound down twice, so Windows'
slider cannot show the real master. The Hub's minimum is that it respects the drag:
- **Drag:** the slider stays where the user drops it while the mouse is held. On release, the sound
  takes that level, and the slider moves back near the top within a moment. Foxglove's card above it
  shows the level the user chose (§1), so the drag visibly "took".
- **Volume key:** Windows' flyout jumps one step and back near the top. The sound and Foxglove's card
  move one step.
- **Requirement for app-engineer:**
  - Keys step master **both ways** by Windows' own step. With the device at exactly 100 %, "volume
    up" sends no change event, and "volume down" reads as "98 %", not "one step down".
  - So either hold the device one step below 100 % (and check that the step costs nothing audible
    on the listed devices), or read a key-sized change from the held level as a step and anything
    else as a drag to that level.
  - Picked (#1069): the device is held one key step below the top, and whole key steps from the
    level Windows last reported are key presses. A drag is applied once it settles, and a drag
    down never raises master. The words say "near the top", with no number: the step depends on
    the device.
  - Follow-ups (#1070):
    - A key press is applied once no other change follows it for 120 ms (shorter than Windows'
      shortest key-repeat delay, longer than a drag's sample gap), with no polling: the
      notification starts a one-shot deadline. A drag whose first sample is one whole step is a
      drag, not a key press. Further whole steps the same way before then add up (a key repeat).
    - A device that reports a step coarser than Windows' default 2 % (10 %, say) while its keys
      move it less: a smaller whole-percent change from the hold is one key press, and master
      steps by what the key moved (within the 1-10 % clamp). It becomes the device's step only
      after two presses in a row of the same size, so one slider click is never learnt. The hold stays where
      it is, so no change makes the device louder. Up and down stay alike.
  - Engineering picks one and states it in the PR. The user sees only: keys work both ways, and a
    drag sets the level where it's dropped.
- Explained in three places: the card's row (§1), the setting's first while-on line (§3), and one
  user-guide paragraph.

### 3. The per-device setting: Settings > Keyboard > Knob card

There is a new part under the hidden-apps list, for the **current default output device**. Other
devices keep their own choice and show it when they are the default.

| Where | Text |
|---|---|
| Caption | "MASTER VOLUME" |
| Row title | the device's Windows name, e.g. "Headphones (Xbox Controller)" |
| Toggle | "Foxglove sets this device's volume" |
| Second line, listed device | "On by default: this device often ignores Windows volume." |
| Second line, other eligible device | "Turn on if the volume number moves but the sound stays the same." |
| While on, line 1 | "Windows' slider goes back near the top for this device; the sound follows the knob, the slider and the volume keys." |
| While on, line 2 | "Foxglove turns each app down instead. Volume mixer shows those lower levels; the knob shows each app's own." |
| While on, line 3 (exclusive control, §5) | "Apps that take exclusive control of this device play at full volume." |
| Not eligible (Windows sets its volume in software) | "Windows sets this device's volume itself." (no toggle) |
| After turning off (until the page changes) | "Windows sets this device's volume again." |
| After turning off while master was muted (#1070) | "Windows sets this device's volume again. Apps stay muted until you unmute the device." |
| On, but the saved levels can't be read (#1070), instead of the while-on lines | "Master volume is paused: Foxglove couldn't read its saved levels. It tries again when you change the volume." |

- **Default:** on for a listed device, and only when Windows hands its volume to the hardware. Off for
  every other eligible device. The user's choice is stored per device and wins over the list.
- **Accessible text:** the toggle's name is the toggle label. Its description is the row title plus
  the second line, e.g. "Headphones (Xbox Controller). On by default: this device often ignores
  Windows volume."
- It takes effect at once (no Save). Turning it off puts the device's volume back to the master level
  and every app back to its own level.

### 4. Per-app levels: the app's own

- The knob's app cards and the tray Knob row show and step **the app's own level** (Tarkov 84 %).
  Windows' Volume mixer shows the applied one (29 % with master at 35 %).
- Explained in the setting's line 2, and in the **tray Knob row's tooltip** while the device is
  handled: **"<app> <own> % · Volume mixer shows <applied> % with master at <master> %."**
- An app that keeps setting its own volume (a game's slider: the self-managed cut-off) stops following
  master. Its tray tooltip says **"<app> sets its own volume, so master doesn't change it."** There is
  nothing on the card: a 2 s card is no place for it.

### 5. Exclusive-control apps

Apps that take exclusive control bypass app levels and play at the device's full volume. That is the
one way this gets loud. Foxglove can't see those streams without polling, so it doesn't try:
- Line 3 of the setting says so, in Windows' own words.
- If Windows' "Allow applications to take exclusive control of this device" is on for the device, the
  line gets **Open Sound settings**. Foxglove reads that once when the page draws. The button opens
  Windows' Sound panel so the user can change it there; Foxglove never changes it. (This is the
  question below.)

### 6. After a crash

- **While Foxglove is down:** the sound stays where it was (apps turned down, device near the top). Nothing
  gets louder. Changes from the flyout or the keys are not followed until Foxglove is back.
- **Next start:** Foxglove reads the journal and puts each app back as it appears.
  - An app still at Foxglove's applied level gets its own level from the journal.
  - An app the user or the app changed meanwhile keeps that new level as its own.
  - Then Foxglove takes over again or, if the setting is now off, restores and stops.
- **What the user sees:** nothing. There is no toast and no tray line, because the sound never jumps.
  The log says "Foxglove: master volume: restored <n> apps from the last run's journal", and the
  health view counts it.
- **Apps that closed while Foxglove was down** may start quieter (Windows remembers the lower level).
  Foxglove fixes each one the first time it plays, while its journal entry is kept (30 days).
- **Uninstall, or Foxglove not started again:** nothing restores from the journal. Foxglove's
  next start restores it; otherwise the user guide's fallback is Windows' **Settings > System >
  Sound > Volume mixer > Reset**. An uninstaller step was looked at in #1070 and left out: the
  MSI's `--uninstall-cleanup` hook would have to open the audio sessions in a one-shot process
  without starting master volume, and race a running Foxglove; and a normal uninstall closes
  Foxglove first, which already puts everything back. Only a crash followed by an uninstall is
  left to the Volume mixer reset.
- **A journal that can't be read** (a read error, not only a broken file, #1070) is kept and
  never written over: master volume doesn't start until it can be read, the setting shows the
  paused line (§3), and the device's next volume change or the page's next visit tries again. A
  broken one is moved aside as before.

### 7. Data (for app-engineer)

`app/resources/volume-quirks.toml` is read-only and shipped with the app, with no device names in code:

```toml
# Output devices that take Windows' master volume into hardware and sometimes don't apply it (#1064).
# Matched as a case-insensitive prefix against the default output device's parent hardware ids.
# Listed devices get "Foxglove sets this device's volume" on by default (only if Windows reports
# hardware volume for them); the user's choice per device wins.

[[device]]
ids = ["USB\\VID_0ECB&PID_2148"]
name = "JBL headset dongle (Xbox protocol)"  # log and this file only; the UI uses Windows' name
note = "Ignores endpoint volume after some reboots; the headset's wheel still works (2026-10-30)."
```

The per-device choice lives in Foxglove's settings, keyed by the endpoint id, with the device's Windows
name for display.

### Hand-off acceptance (adds to the list above)

- On a handled device, the knob's master card draws the same as on any device. An outside change shows
  the card above Windows' flyout, with the third row on the first change of the run only.
- Flyout drag (fake endpoint event, no own context): master takes the dropped level, and the device
  goes back to its held level in the same callback. Own writes are ignored.
- Volume keys step master both ways from any level (window-free test over the fake endpoint).
- Setting: eligible and listed is on by default; eligible and unlisted is off; not eligible shows the
  line with no toggle. The choice survives a restart and a list update.
- Tray tooltip strings for handled and self-managed apps; no session ids in any string. All strings
  are at least 4.5:1 in both themes, and the card's third row fits 212 pt.

### Open question for the user (via the Hub)

1. Exclusive-control apps play at full volume while this is on. **Show "Open Sound settings" in the
   setting when Windows allows exclusive control, so you can turn that off yourself** (pick) /
   mention it only in the user guide.
