# Right-screen art cycling (#60)

Status: spec for review. Today's hotlist (`app/src/foxglove/art_cycle.rs`, `[art.cycle]` in the
store, profiled) has four modes: **Don't cycle**, **Every N minutes**, **When the lights wake**,
and **Match the wallpaper**. This adds three event-driven triggers (**on a key**, **on a
layer**, **on a preset**) and sets the rules all modes share.

## Rule: events, not timers

Every trigger is an event the app or the keyboard already raises. Nothing polls, and nothing
wakes up to check. The one exception is **Every N minutes**, which the user asked for by name
(#60). It's a single deadline timer, armed only while a keyboard is connected and a hotlist
exists, and never a polling loop. A mode that has no event source on the running firmware is
shown disabled with the reason, as **When the lights wake** is today.

## The triggers

| mode (UI label) | event | what happens |
|---|---|---|
| Don't cycle | none | the pinned choice stays |
| Every N minutes | one deadline, re-armed on each change | next entry |
| When the lights wake | firmware RGB wake event (`wake_events`) | next entry |
| Match the wallpaper | Wallpaper Engine `config.json` change, or without it the Windows desktop wallpaper's change (file watchers) | the wallpaper's art |
| **On a key** (new) | the `&art_next` / `&art_prev` behaviour, pressed | next / previous entry |
| **On a layer** (new) | a layer change from the layer notifier | the layer's art while it's active |
| **On a preset** (new) | a lighting preset or profile being applied | the preset's art |

The new modes aren't exclusive with each other: a hotlist can step **on a key** and also show
**layer art** for GAME. So the store gets a set of triggers, not one mode:

```toml
[art.cycle]
items = [...]
position = 2
step = ["key", "lights_wake"]            # what advances the hotlist: key, lights_wake, minutes
minutes = 5                              # only with "minutes"
follow = "wallpaper"                     # or "preset", or absent: what picks the art outright
layers = { 3 = { builtin = "Bandit_Radar" } }   # layer index -> art while that layer is active
```

`Mode` keeps deserialising, and the old values migrate one to one:
- `Minutes(m)` becomes `step = ["minutes"]` with `minutes = m`;
- `LightsWake` becomes `step = ["lights_wake"]`;
- `Wallpaper` becomes `follow = "wallpaper"`.

### On a key

- **Firmware:** a new shield behaviour, `&art_next` (and `&art_prev`), bound in the keymap.
  SYSTEM + **P** / **O** (top row, right) are free.
- **With the app connected,** the key sends a host-link event (a new D7 notify, like the RGB
  wake one), and the app steps the hotlist, which can include library files.
- **Without the app,** the firmware steps through what it holds itself: the built-ins in the
  hotlist and the saved slots. So the key always does something.
- No polling, and `key.events` isn't needed: the behaviour is the event.

### On a layer

- **Shows a layer's art while the layer is active,** then puts back whatever was there. GAME
  could show `Bandit_Radar`, for example. Holding NUMBER shouldn't swap the art, so only layers
  given art in `layers` react.
- **The source** is the layer notifier the overlay already uses (`zmk-keypeek-layer-notifier`).
- **Built-ins and saved slots** switch at once. **Files** are pushed when the layer is first
  entered and cached in a slot afterwards (see the art notes), so the next time is instant.
- **Momentary layers:** show the art only once the layer has been active for 400 ms, so tapping
  a thumb key doesn't flash the screen. That's a one-shot deadline from the event, not a poll.

### On a preset

- **When a lighting preset is applied** (#265, #273: Settings, tray, or a profile switch), show
  the preset's `art`. If **Change the right screen with the preset** is off, nothing changes.
- **Clearing the preset** puts back the hotlist position or the pinned art.
- **The hotlist doesn't step on a preset;** a preset picks its own art. A profile without a
  preset but with its own `[art.cycle]` just uses that hotlist.

## When triggers overlap

Highest first:
1. **The user's own pick,** from the tray or Settings. It pins and pauses the triggers until
   the next trigger event.
2. **Layer art,** while that layer is active.
3. **Preset art,** while a preset is active.
4. **The wallpaper match** (`follow = "wallpaper"`).
5. **The hotlist position,** moved by `step` triggers.

When a higher source ends (layer left, preset cleared), the next one down shows again. The
store keeps `position` across restarts, as now.

## Art notes

- **Instant versus pushed:**
  - built-ins and saved slots switch at once (an index);
  - a file is a push over the split link, about 1–3 s for a 4–12 KiB set.
  - For key and layer triggers, prefer built-ins and slots. The UI marks hotlist files with a
    small "push" badge, and offers **Save to keyboard** to turn a file into a slot.
- **Size:** keep hotlist art under 12 KiB (the library target) so pushes stay short. The
  built-ins and the four preset sets are all under that.
- **Feedback:** when a key steps the hotlist, the left panel shows a one-second toast (`ART 3/7`
  and the set's label in the 3x5 font) over the clock area. That's a panel override with a 1 s
  TTL, not a timer. Layer and preset changes need no toast; the art itself is the feedback.
- **No extra transition frames:** the firmware swaps sets on the next frame. A glitch-wipe
  transition would need firmware work and RAM. Not now.
- **Art that pairs well with triggers:**
  - `Bandit_Radar` for GAME (it reads as "scanning");
  - `Bandit_Lock` for a locked or idle PC (a later trigger);
  - `Bandit_Signal` for the Signal preset;
  - the FURY and Hearth sets for their presets.

## UI (Settings → Right screen art → Hotlist)

- **Step the list:** checkboxes for **On the art key** (disabled without firmware support,
  with the reason), **When the lights wake**, and **Every [5] min**.
- **Follow:**
  - a radio: **Nothing** / **The wallpaper** / **The lighting preset**;
  - **Follow** keeps its monitor picker for the wallpaper.
- **Layer art:** a small table with one row per layer (from the keymap), an art picker per row,
  and "none" by default.

## Work this needs

- **Firmware:** `&art_next`, `&art_prev` and the host-link notify, plus on-device stepping over
  built-ins and slots. HELLO gets a capability bit, so the app can enable the key option.
- **App:**
  - the store change and migration;
  - the priority stack in the art service;
  - layer and preset hooks;
  - the toast;
  - the UI above.
- **Test lab:**
  - the key step with and without the app;
  - layer art on GAME, including a quick tap that mustn't flash;
  - preset art appearing and being restored;
  - priority order after a manual pick.

## As built (first app PR)

- **Store:** `art_cycle.rs` has `step` (key, lights_wake, minutes), `minutes`, `follow` (nothing,
  wallpaper) and `layers`. The old `mode` is read once and migrated, and never written back.
- **Logic:** `art_triggers.rs` holds the layer and preset triggers and their priority. The art
  service gets the layer changes from the device event bus (`DeviceEvent::LayerChanged`), so
  `keyboard.rs` is untouched.
  - Its only clock is the one-shot `LAYER_DELAY` deadline, merged with the minutes deadline into
    one `recv_timeout`.
  - Trigger art is shown without pinning (`Command::Show`). The hotlist and the wallpaper match
    wait while trigger art is on top.
- **Presets:** the preset code calls `Handle::preset_art(Some(item) / None)`, and only when
  **Change the right screen with the preset** is on. That toggle belongs to the presets, so
  there's no "follow the preset" option here.
- **Layer art** offers built-ins and saved slots (instant). Files wait for the slot cache.
- **The art key:** the checkbox is there but disabled until firmware ships `&art_next` and its
  HELLO capability bit.
