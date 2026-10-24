# Foxglove local API and plugins, v1

Status: design for #69, with the contract in code (`crates/plugin-api`, schema in
`crates/plugin-api/schema/v1.json`). This builds on [plugins-and-devices.md](plugins-and-devices.md)
(the device layer, plugins, the Plugins tab). Nothing in the app speaks it yet. The next PRs add the host, the named
pipe and the Plugins tab.

## One protocol, three doors

| Who | Door | Permissions |
|---|---|---|
| **Plugins** (plugins Foxglove starts) | the plugin's stdin/stdout | what its `plugin.toml` declares, approved by the user at install |
| **Outside tools** (Stream Deck, Wallpaper Engine #39, scripts) | named pipe `\\.\pipe\foxglove-<user>`, current user only | what `initialize` asks for, approved by the user once per tool |
| **foxglove-cli** | the same pipe | as an outside tool |

There is no network listener. A localhost WebSocket for browser-based tools (Wallpaper Engine
web wallpapers) can come later behind an opt-in toggle and a token file. It would speak the
same messages.

## Wire

- **JSON-RPC 2.0**, one UTF-8 message per line, at most 256 KiB per line.
- **Requests** carry an `id` and get exactly one response; **events** are notifications from
  the app (no `id`).
- **`initialize` comes first**, and nothing else is accepted before it:
  ```json
  → {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"api":"1.0","client":{"name":"weather","version":"1.2.0"}}}
  ← {"jsonrpc":"2.0","id":1,"result":{"api":"1.0","app":"0.3.0","granted":["facts.write:weather.*","net"]}}
  ```
  A client whose API major isn't served gets error 7 (VERSION) and the connection closes.
- **Errors:** JSON-RPC's own codes, plus the Foxglove codes, which mirror the device layer's errors:
  1 unsupported, 2 disconnected, 3 busy, 4 timeout, 5 denied, 6 backend, 7 version, 8 rate-limited.

## Methods (v1)

| Method | Kind | Needs |
|---|---|---|
| `initialize`, `ping` | call | — |
| `facts.get`, `facts.list`, `facts.subscribe`, `facts.unsubscribe` | call | `facts.read:<pattern>` covering the key/pattern |
| `facts.set` | call | `facts.write:<pattern>`; `device.*` keys are never writable |
| `facts.changed` | event | the subscription |
| `devices.list` / `devices.changed` | call / event | `devices.read` |
| `lighting.state` / `lighting.set` | call | `devices.read` / `lighting.write` |
| `behaviors.list` / `behaviors.invoke` | call | `devices.read` / `behaviors.invoke` |
| `screens.panel_override` | call | `screens.write` |
| `art.status` / `art.select` | call | `devices.read` / `art.write` |
| `key.event` | event | `key.events` (warned: a keylogger) |
| `profiles.list` / `profiles.activate` | call | — / `profiles.write` (API 1.1) |

The params and results are in the schema. Notes:

- **Facts are the main surface.** Device state appears under `device.<id>.*` (connected, layers,
  battery.right, lighting.*). Plugins publish their own facts (`weather.temp`), and the panel and
  other plugins read them.
- **`behaviors.invoke` refuses actions that need confirmation** (error 5). A Plugin can never press
  one; only a user's click in Foxglove can. The firmware already refuses &bt, &out, reset and
  soft-off from the PC altogether.
- **`screens.panel_override`** lasts at most 60 s; the panel then returns to Foxglove's own drawing.
- **`art.select` names built-ins by their manifest name** (stable across firmware), plus slots,
  random, or a file.
- **`hid.raw` is reserved:** it's a valid permission but has no method in v1.

## Versioning

- **Minor (1.x):** only adds, i.e. new methods, new optional fields, new events. Both sides
  ignore what they don't know.
- **Major:** breaking. The app serves the current major and the one before it for at least one
  release cycle.
- **Enforcement:** `schema/v1.json` is generated from the Rust types, and a test fails when the
  types change without the file. Reviewers check that a schema diff matches the version bump.
  Regenerate with `UPDATE_SCHEMA=1 cargo test -p foxglove-plugin-api`.
- **SDKs:** the Rust types *are* the Rust SDK (MIT). Python/TypeScript clients get generated from
  the schema later.

## Plugins

**Renamed in #87** (Kits → plugins, Den → the Plugins tab and the plugin host, Palm → the device layer). Names that shipped keep working:
- **Manifest:** `kit.toml` is still read when there is no `plugin.toml`.
- **Catalogs:** the formats `foxglove-kit-catalog` (with `kits`) and `foxglove-kit-root` are still accepted.
- **Settings:** `[kits]` (and `[kits.trust]`) are read as `[plugins]`, and the next save writes the new name.
- **Folder:** a `kits\` folder becomes `plugins\` on first use.
- **Environment:** `FOXGLOVE_DEV_UNSIGNED_KITS` and `FOXGLOVE_DISABLE=kits` still work.
- **Unchanged:** the API (methods, permission names, schema) had no kit names in it, so it is the same.

```toml
# %APPDATA%\Foxglove\plugins\com.example.weather\plugin.toml
id = "com.example.weather"
name = "Weather"
version = "1.2.0"
api = "1.0"
run = ["weather.exe"]
permissions = ["facts.write:weather.*", "net"]
provides = ["facts"]
description = "Puts the temperature on the left screen."
```

**Manifest rules** (checked by `Manifest::parse`):
- `id` is reverse-DNS;
- `api` must be served;
- `run` stays inside the plugin's folder;
- unknown keys and permissions are refused.

**Updates:** an update that adds permissions needs the user's OK for those (`added_permissions`).

**Lifecycle** (next PR):
- **Supervision:** Foxglove starts each enabled plugin under `services::spawn_supervised`, with backoff from 1 s to 60 s.
- **Liveness:** a `ping` every 10 s; no answer in 30 s means a restart. Five crashes in ten minutes disables the plugin, with a notice in the Plugins tab.
- **Rate limits:** per plugin (calls/s, lighting frames/s, panel overrides); over the limit gets error 8.

**Containment on Windows:**
- **Job Object:** every plugin runs in one, killed with Foxglove, with memory and CPU caps and no breakaway.
- **AppContainer:** Plugins that declare neither `net` nor `files` run in one, so no network and no user files.
  A `net` plugin runs outside it, still in its Job Object. An AppContainer can't receive loopback traffic from another program without a machine-wide exemption (`CheckNetIsolation`, admin), and local listeners such as the OpenRGB Plugin need exactly that. So `net` stays a permission the user approves at install.
- **"Trusted" Plugins:** the others (vendor SDK Plugins need their vendor service) are marked trusted in the Plugins tab.

**Plugins catalog:** curated, first-party, à la carte. Each plugin has an install button, and nothing
vendor-specific is installed by default. Third-party plugins are a later decision.

**License:** Plugins are separate programs talking over IPC, so they needn't be GPL. The API crate
is MIT. (A reading of the licenses, not legal advice.)

## Phase 3 plan (M6)

1. **This PR:** `crates/plugin-api` (types, permissions, manifest, schema), and this doc.
2. **Host:**
   - a connection loop over any `Read + Write` (stdio and pipe share it);
   - an `initialize` gate, per-call permission checks, facts get/set/subscribe on the device layer's store, and rate limits;
   - tests driven by golden transcripts.
3. **Named pipe** with a current-user ACL, plus `foxglove-cli status` as its first client.
4. **Plugin runner:** spawn, Job Object/AppContainer, supervision, ping.
5. **Plugins tab:** installed plugins (status, permissions, log, enable/disable), the catalog, install/uninstall.
6. **Device methods** (lighting, behaviors, panel override, art) wired through the device layer, and an
   example weather plugin.

## Signing (#159)

Only signed plugins install and run, checked by `crates/plugin-trust`.

**The chain:**
- **Root key:** an **offline root key**, whose public half is pinned in the app at `app/resources/plugin-trust/root.pub`, signs `root.json`:
  `{"format":"foxglove-plugin-root","version":1,"expires":"2027-…Z","catalog_keys":["<minisign pubkey>"]}`.
- **Catalog:** a catalog key named there signs `catalog.json`:
  `{"format":"foxglove-plugin-catalog","sequence":N,"expires":"…Z","plugins":[{"id","version","sha256","files":{"path":"sha256"}}]}`.
- **Rollback:** the app refuses a catalog whose `sequence` is lower than the highest one it has seen.

**What's checked:**
- **Install:** the archive's SHA-256 must be in the catalog, its manifest must match the entry's id and version, and the unpacked files must match exactly.
- **Before every launch:** the version must still be in the catalog (removing it revokes it), and the folder must hold exactly the signed files. `plugin.log` is ignored.

**Making the keys (maintainer, once; tracked in #90):**
1. `minisign -G -p root.pub -s root.key`, then keep `root.key` offline.
2. `minisign -G -p catalog.pub -s catalog.key`.
3. Put the root public key (the base64 line of `root.pub`) in `app/resources/plugin-trust/root.pub`.
4. Write `root.json` listing `catalog.pub`'s key and sign it: `minisign -S -s root.key -m root.json`.
5. For each release, write `catalog.json` with a higher `sequence` and sign it with `catalog.key`.
6. Ship both files and their `.minisig` signatures in `app/resources/plugin-trust/`.

**Developer mode:** `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` allows unsigned plugins, and the Plugins tab shows a warning.

**Test hook:** with developer mode and a test `FOXGLOVE_CONFIG_DIR` (never the real settings folder), `FOXGLOVE_PLUGINS_AUTOINSTALL=<zip or folder>` installs and enables that plugin at start with all its declared permissions, without the approval dialog. It writes a warning to stderr and to `autoinstall.log` in the config dir. It is meant for the Test lab only.

## Writing a plugin (Rust)

The `foxglove-plugin` crate (`crates/plugin-sdk`, MIT):
- **Connect:** `Plugin::connect(name, version)` says hello and pings every 10 s for you.
- **Calls:** `set_fact`, `fact`, `subscribe` plus `events()`, `devices`, `set_lighting`, and a generic `call`.

The example is `crates/plugin-focus-timer`, a focus/rest timer that publishes `focus.state` and `focus.remaining` and colours the underglow. It asks only for `devices.read`, `lighting.write` and `facts.write:focus.*`, so it runs in the AppContainer.

The second example, `crates/plugin-openrgb-e131`, is a plugin with the `net` permission; see [openrgb-plugin.md](openrgb-plugin.md).

**Pack a plugin:** `powershell -File scripts\pack-plugin.ps1 plugin-focus-timer` builds it, zips `plugin.toml` and the program into `target\plugins\`, and prints the catalog entry (archive and file SHA-256s) to add to `catalog.json` before signing. Until the signing keys exist, install it with `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1`.
