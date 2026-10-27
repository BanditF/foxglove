# Settings and profiles

Status: design for #66 (phase 5 of [plugins-and-devices.md](plugins-and-devices.md), M6), for
approval. No code yet.

## Where settings live today

Everything sits in `%APPDATA%\Foxglove\config\`, one file per feature, each read and written
its own way:

| File | Owner | Format |
|---|---|---|
| `settings.ini` | upstream KeyPeek (overlay look, monitor, layers shown, timeouts) | INI, rewritten whole by upstream code |
| `connection.ini` | last keyboard | INI |
| `screens.ini` | left panel: drive, fps, game hints | INI, watched for changes |
| `layers.ini` | saved layer names | INI |
| `art.json`, `art-cycle.json` | pinned right art, hotlist | JSON |
| `lighting.ini`, `updates.ini` | accent colour, update checks | INI |
| `keymap-*.studio`, `right-battery.csv`, `art\` | caches and data, not settings | — |

It works, but there's no single place to version or migrate settings, no profiles, and no
export.

## Decision 1: one store for Foxglove's settings, KeyPeek's file left alone

- **New file:** `%APPDATA%\Foxglove\foxglove.toml`, with `schema = 1` at the top. Each Foxglove
  feature owns one table: `[screens]`, `[art]`, `[art.cycle]`, `[lighting]`, `[updates]`,
  `[connection]`, `[layers]`, `[plugins]`, `[profiles]`, and for Scenes `[scenes]` (the chosen
  look, which a profile may override), `[scene_presets]` and `[scene]` (the room map's drags).
- **Module:** `foxglove::store` loads the file once and hands each feature a typed section
  (serde, with defaults).
- **Writes:** atomic (temp file and rename). External edits are picked up by the existing file
  watcher (`screens_watch`, generalised), so there's no polling.
- **Migration:** on the first run with the store, the old files are imported once and renamed
  `*.migrated`. They aren't deleted, so rolling back to an older Foxglove still finds them by
  hand. Each later schema bump is one `migrate_n_to_n1` function, chained.
- **KeyPeek's `settings.ini` stays where it is, in its own format.** Upstream code rewrites it
  whole, and moving it would make every `git subtree pull` from srwi/keypeek painful. The store
  reads and writes the overlay settings *through* KeyPeek's `Settings` type. Profiles can still
  override overlay keys (Decision 2) without changing that file's format.
- **Not settings:** caches and data stay as files (`keymap-*.studio`, `right-battery.csv`,
  the art library).

## Decision 2: a profile is a sparse override

```toml
[profiles]
active = "Game"            # or absent = Base
auto = true                # switch by foreground program

[profiles.Game]
overlay = { visible_layers = 0b0110, timeout = 800 }
screens = { fps = 10 }
art = { pinned = { slot = 2 } }
lighting = { color = { hue = 0, saturation = 100 }, effect = "0" }

[[profiles.rules]]
program = "eldenring.exe"
profile = "Game"
```

- **Base and overrides:** the base settings are the store as above. A profile holds only the
  keys it changes, so editing the base still reaches every profile that doesn't override that
  key.
- **Effective settings** = base deep-merged with the active profile. Each feature reads its
  effective section, never the raw one.
- **What a profile can set:**
  - overlay (KeyPeek's look, layers shown, timeouts);
  - screens (drive, fps, game hints);
  - art (pinned choice, hotlist and its mode);
  - lighting (accent follow, or a fixed colour/effect applied on switch);
  - which plugins are enabled.
- **What a profile can't set:** the keyboard connection, update settings, layer names and
  caches, because those describe the machine, not a mood.

## Switching

- **By hand:** a Profiles submenu in the tray (Base, each profile, Auto), and a picker at the
  top of Settings.
- **By program:** `profiles.rules` map a foreground program to a profile. The app already
  publishes the `app.foreground` fact on Windows' foreground-change event, and that's the
  trigger. No polling.
  - **Matching:** the first rule that matches wins. No match means Base.
  - **Manual choice:** a manual pick holds until the next foreground change that matches a rule
    (or until Auto is chosen again), so the user is never fought.
- **Applying:** the switch sets the `profile.active` fact. Each feature subscribes to it and
  reloads its effective section. This is the same facts mechanism plugins and the local API use,
  so a Stream Deck button (through the API) can switch profiles too.
- **What the keyboard sees:** lighting and art changes go through the device layer, so a profile switch
  shows on the board within a second.

## Export and import

- **Export:** one `.foxprofile` (TOML) per profile, or all of them. It holds the overrides and
  the rules, never the connection or machine-specific keys.
- **Import:** validates the schema version (migrating older ones). If the name already exists,
  it asks before replacing.

## API

- **New facts:** `profile.active` and `profile.names`.
- **New method:** `profiles.activate {name}` with a new permission `profiles.write`. That's an
  API minor bump to 1.1, purely additive.

## Phases

1. **Store and migration:** `foxglove.toml`, typed sections, import of the old files. No
   visible change.
2. **Profiles:** the data model, effective-settings merge, tray and Settings picker, and the
   `profile.active` fact.
3. **Rules:** switching per foreground program, with the manual-override behaviour above.
4. **Export/import and the API method.**

Each phase leaves the app shippable. Phases 1 and 2 touch every feature's settings code, so
they go in as one feature at a time behind the store, starting with screens.

## Questions for the user

1. **Starter profiles:** ship with *Work* and *Game*, or start with only Base and let you
   create profiles?
2. **Lighting in a profile:** a fixed colour/effect, or only "follow accent on/off" plus the
   effect, leaving the colour to the board?
3. **Overlay in profiles:** should profiles override the overlay's look (colours, size,
   position), or only which layers it shows and when?
