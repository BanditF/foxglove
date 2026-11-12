# Master volume: "Reset app levels" (UX plan)

Escape hatch for software master volume (#1064, `crates/info-screen/src/master.rs`,
`app/src/foxglove/knob_master.rs`, plan `docs/knob-exclude-ux.md` "Master volume on a device that
ignores Windows volume"). Plan only, no code. Review image:
`C:\Users\fuchs\splitkeeb\worktrees\h-ux-reset\review\ux-reset-levels-proposed.svg` (drawn
mockup, not an egui render).

## Problem

With "Foxglove sets this device's volume" on, every app is turned down by master. When something
goes wrong (Vivaldi at 9 %, a game with no sound) the user's only way out today is to turn the
setting off, or Windows' Volume mixer Reset, which the next scale overwrites. They need one obvious
action that fixes every app at once and leaves master on.

Why apps go wrong (from the engine): an app's own level is learned from what Windows reports. If
Windows starts a closed app at a scaled level the record doesn't match, or an app writes a low
level, that low level becomes the app's "own" and is scaled again. A stale master-mute mark or a
self-managed mark can leave a game silent. So "put each app back to its recorded own level" does
not fix the worst case: the recorded level is what drifted.

## Research and scope

- **Reuse:** the engine's `rescale` and journal (`Soft::rescale`, `journal_on`); the tray Knob
  part and its Hide/Undo pattern (`tray_panel.rs`, #952); the knob popup's master card
  (`knob_popup.rs`); the knob's Hold event, which on master today does nothing ("already master").
- **Emulate:**
  - **Windows 11 Volume mixer, Reset** ("Reset sound devices and volumes for all apps to the
    recommended defaults"): every app back to 100 %. Same meaning, same word.
  - **EarTrumpet:** per-app levels are the app's own; master sits above them.
  - **Foxglove's own Hide from knob:** a one-click action with Undo in the same tray row.
- **Scope:** in: apps on the handled device (live and remembered). Out: other devices, exclusive-mode
  streams (Foxglove can't see them), changing any Windows setting, a new firmware gesture.

## Decisions

### 1. What it does: master stays on and scales again at once

- Every app on the handled device gets **own level 100 %**, then is scaled by master at once (one
  `rescale`, journal first). Volume mixer shows master's amount for each app; the knob shows 100 %.
- Remembered apps (closed, in the journal) get own 100 % too, so they come back right.
- **Self-managed apps:** the mark and its write count are cleared, and they are reset like the rest.
  If the app sets its own volume again, the existing fight rule marks it again. A game whose own
  write left it silent gets sound back; one that fights wins as before.
- **Master mute:** unchanged. A muted master stays muted (its marks stay). When master is not muted,
  stale master-mute marks are cleared and those apps unmuted. The user's own app mutes stay.
- **Master's level and the device hold:** unchanged.
- **No "pause scaling" state.** On a device that ignores its volume, apps at their own level with
  no scaling play at full volume. Every app ends at master's amount at most: never louder than
  master says, the engine's rule.
- **Undo** (tray only): each app's previous own level and marks come back, then scaled. Offered
  until the next change to master or any app's level, or 10 minutes, whichever comes first.
- One log line: `Foxglove: master volume: app levels reset: <n> apps to 100 %, master <m> %`
  (`, <k> couldn't be set` when some failed). The health view counts resets.

### 2. Where it lives: the knob and the tray (two places)

- **Knob (first):** **hold on master, then hold again within 3 s.** Hold on master is free today
  (no firmware change, HELLO as is). The first hold only asks; nothing changes until the second.
  So holding to get back to master, then holding again by habit, still needs a deliberate second
  hold while the card says what it will do. Only while the device is handled; otherwise hold on
  master does nothing, as today.
- **Tray panel, Knob part:**
  - On master while the device is handled, the Knob part shows (today it is hidden on master):
    "Master volume" / "<m> % · Foxglove sets this device's volume" with a **Reset app levels**
    button (image A).
  - On an app, today's row stays; a text button **Reset app levels** sits under it (image B), so
    the action is there whatever the knob points at.
  - Keyboard: the button is in the panel's tab order after Hide from knob; Enter or Space presses it.
    No confirm step in the tray: Undo covers it.
- **Settings > Keyboard > Knob card:** no button (two places is enough). The while-on lines gain
  one line: "If an app is too quiet, hold the knob on master twice, or use Reset app levels in the
  tray."

### 3. Wording (exact)

| Where | Text |
|---|---|
| Tray button | "Reset app levels" |
| Tray button tooltip | "Every app back to 100 %, then turned down by master." |
| Tray Knob part on master, line 1 / 2 | "Master volume" / "<m> % · Foxglove sets this device's volume" |
| Tray after reset | "App levels reset. All apps follow master (<m> %)." + **Undo** |
| Tray after reset, some failed | "App levels reset, except <k> app(s). All others follow master." + **Undo** |
| Tray after Undo | "App levels put back." (until the panel closes) |
| Knob card, first hold (3 s) | "Master volume" + level, third row "Hold again to reset app levels" (accent) |
| Knob card, done (3 s) | "App levels reset" / "All apps follow master · <m> %" |
| Knob card, done, master muted | "App levels reset" / "Master volume muted" |
| Knob card, some failed | "App levels reset, except <k>" / "All others follow master · <m> %" |
| Settings, while-on line 4 | "If an app is too quiet, hold the knob on master twice, or use Reset app levels in the tray." |

The confirm row reuses the master card's third-row layout from #1064 §1 (64 pt card, 10 pt row,
at most 212 pt wide); "Hold again to reset app levels" fits.

### 4. States (every state has a way out)

| State | Knob | Tray |
|---|---|---|
| Device not handled | hold on master: nothing (today) | no master line, no button |
| Handled, idle | first hold asks | A or B |
| Asked (3 s) | second hold resets; anything else or the timeout cancels, nothing changed | unchanged |
| Just reset | card 3 s | C (or D) with Undo |
| Undo used | nothing | "App levels put back." |
| Undo expired (next change, 10 min) | nothing | back to A or B |
| Journal unreadable (paused, #1070) | no confirm card; hold does nothing | no button (the setting's paused line covers it) |

## Hand-off acceptance (app-engineer)

- Window-free engine tests over the fake audio: after a reset every live and remembered app has own
  1.0 and applied `amp(master)`; self-managed is cleared; master's level, mute and hold are
  unchanged; a muted master's marks stay; stale marks clear when master is not muted; no app ever
  goes above `amp(master)` during the writes (journal first).
- Undo restores the previous own levels and marks exactly, and is gone after the next master or app
  level change or 10 minutes.
- Knob: hold on master with the device handled shows the confirm card; a second hold within 3 s
  resets; a turn, click, double click or the timeout cancels. Device not handled: no card.
- Tray: the Knob part shows on master only while handled; strings exactly as in §3; 4.5:1 contrast
  both themes; the button is reachable by keyboard.
- Restart parity: an Undo snapshot is in memory only and is not offered after a restart (n/a by
  design); the reset itself is in the journal before any write.

## Open question for the Hub (one)

1. **What "my levels" means.** **(A, pick) Every app to 100 %, then master**, with Undo: fixes the
   drifted case (Vivaldi at 9 %), matches Windows' own Reset; trims the user set by hand go, Undo
   brings them back. / (B) Every app to its recorded own level, then master: keeps trims, but
   doesn't fix a drifted own level, which is the case that needs the escape hatch.
   Image: `C:\Users\fuchs\splitkeeb\worktrees\h-ux-reset\review\ux-reset-levels-proposed.svg`.
