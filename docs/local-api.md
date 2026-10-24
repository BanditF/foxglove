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
- **AppContainer:** plugins that declare neither `net` nor `files` run in one, so they have no network of their own and no user files.
- **Network, scoped (#214, API 1.2):**
  - **Why a broker:** Windows can't limit an AppContainer to particular hosts without admin-only, machine-wide settings, and it blocks loopback outright. So Foxglove opens the connections a plugin's permissions name, and the plugin stays in the AppContainer.
  - **Permissions:**
    - `net.connect:<host>:<port>`: TCP. `*.example.com` covers subdomains only, and the port may be `*`.
    - `net.listen:udp:127.0.0.1:<port>`: datagrams to a loopback port.
  - **Methods:** `net.open {kind: tcp|udp, host, port}` returns a `stream`. `net.send {stream, data}` sends bytes (base64), `net.close` ends a stream, and the events are `net.data {stream, data, from?}` and `net.closed {stream, reason}`.
  - **SDK:** `listen_udp(port)`, and `connect_tcp(host, port)`, which gives a `Read + Write` stream to wrap in TLS or a WebSocket.
  - **Limits:** 16 streams per plugin, and all of them close when the plugin's connection ends.
- **Network, unrestricted:** bare `net` still works for older plugins. Such a plugin runs outside the AppContainer, still in its Job Object, and the install dialog warns about it ("use any network, without the sandbox").
- **Key actions (#212, API 1.2):**
  - **Declaring:** a plugin lists what a key can trigger in `plugin.toml`, `[[actions]]` with `id` and `name`. Ids are unique, using `a-z`, `0-9` and `-`.
  - **Binding:** in the Plugins tab, the user binds each action to one of F13–F24. Each keyboard can send those keys, and they type nothing; the user puts one on a key in ZMK Studio or VIA. A key triggers one action, and an action has one key.
  - **Delivery:** Foxglove registers the bound keys as global hotkeys and sends `action.invoked {action}` to that plugin only. The SDK delivers them on `Plugin::actions()`. No plugin needs `key.events` for this.
  - **Storage and conflicts:** bindings live in `[plugins.bindings]`. A key another program already holds is flagged in the tab. `FOXGLOVE_DISABLE=actions` turns the hotkeys off.
- **Lighting leases (#236, API 1.2):**
  - **Taking and giving back:** `lighting.lease {device, color, effect?, priority: low|normal|high}` takes the lights and returns `{lease}`; `lighting.release {lease}` gives them back. In the SDK, `Plugin::lease()` returns a guard that releases when dropped. Both need `lighting.write`.
  - **Who shows:** the highest-priority lease, and the newest among equals. Use high for alerts (muted), normal for status (recording), and low for ambient light (a colour sync).
  - **Ending:** a plugin's leases end when its connection closes (stopped or crashed). When the last lease on a device ends, its lights go back to how they were before the first one.
  - **Confirmation:** every change is read back and sent again (up to 3 times) if the keyboard didn't take it.
  - **`lighting.set`:** still changes the lights directly, without any of this.
- **Secrets (#213, API 1.2):**
  - **Permission and access:** a plugin with the `secrets` permission keeps passwords and tokens with `secrets.get {name}` and `secrets.set {name, value}`, or `secret()` / `set_secret()` in the SDK. They go in Windows Credential Manager, under `Foxglove/plugin/<id>/<name>`, protected by the user's login and kept out of manifests, command lines, facts, logs and profile exports.
  - **Scope:** a plugin reaches only its own secrets, and outside tools get none.
  - **Entering them:** a plugin can declare what the user enters with `[[secrets]] name, label` in `plugin.toml`, and the Plugins tab then shows a password field for each. Values are never shown back, only "saved" or "not set".
  - **Limits:** names are 1–64 of a-z, 0-9, `-`, `_` and `.`, and values are at most 2048 bytes.
  - **Uninstall:** uninstalling a plugin deletes its secrets.
- **Windows APIs in the sandbox (#215).** `crates/plugin-sandbox-probe` runs each API under the real runner, sandboxed and not. Results on Windows 11:
  - **Media sessions (SMTC):** denied in the AppContainer ("Access is denied"). Plugins read them as facts instead: the app's one media reader publishes `media.title`, `media.artist` and `media.playing`, plus `volume.level` and `volume.muted` (`facts.read:media.*`, `facts.read:volume.*`). They update while the panel service runs.
  - **Microphone mute (Core Audio endpoint volume):** works in the AppContainer with no capability, so the sandbox can't limit it. So Foxglove brokers it instead (#237): it watches the default communications microphone through Core Audio's callbacks, with no polling, and publishes `mic.state` (`muted`, `unmuted`, `unavailable` or `unknown`; a failed read is never shown as muted), `mic.endpoint_id`, `mic.endpoint_name`, `mic.error` and `mic.app_state` (`unknown`; no app reports its own mute yet). `audio.inputs.read` lets a plugin read those facts, and `audio.inputs.mute` lets it call `audio.inputs.set_mute {muted}`. Endpoint mute is what Windows' own mute toggles. It isn't a privacy guarantee: apps can mute themselves, and exclusive-mode streams bypass it. `FOXGLOVE_DISABLE=mic` turns the watcher off.
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

**Catalog v2 (#192).** Every new field is optional, so v1 files still read.
- **Epoch:** `root.json` gains `"epoch"`, and `catalog.json` must carry the same one. Only the offline root can start a new epoch, and a new epoch restarts the sequence.
- **Rollback state:** the root `version`, the epoch and the sequence never go down. The app keeps them in `[plugins.trust]`, and if it can't save them, it trusts nothing.
- **Entries** gain `"channel"` (`stable` or `beta`), `"url"` (an immutable GitHub Release asset) and `"size"`.
- **Revocations:** `"revocations": [{"id", "versions": [...] (empty = all), "action": "yank"|"revoke", "reason"}]`. A yank stops new installs; a revoke also stops installed copies from launching. A pin never overrides a revocation.

**Where the catalog comes from:**
- The app ships one built-in.
- It downloads newer ones from `https://banditf.github.io/foxglove/plugins/v2/` (`root.json`, `catalog.json` and their `.minisig` files).
- It keeps the last download that verified in `plugin-catalog\`, next to the settings store, and uses whichever verified copy is newest.

**When it checks** (`catalog_refresh.rs`; there's no daily check and no polling, per the user's decision):
- At start, if the last success is more than 24 h old, after 5–60 s of jitter.
- When the Plugins tab opens, at most every 10 minutes.
- On **Check now**.

Each check:
- sends a conditional GET (`If-None-Match`) for each file;
- after a failure, backs off for 15 min, then 1 h, 6 h and 24 h, persisted in `[plugins.refresh]`. **Check now** ignores the backoff.

`FOXGLOVE_PLUGIN_CATALOG_URL` points the download elsewhere (HTTPS, or a localhost test server), and `FOXGLOVE_DISABLE=catalog` turns the start-up check off.

**What's checked:**
- **Install:** the archive's SHA-256 must be in the catalog and not yanked or revoked, its manifest must match the entry's id and version, and the unpacked files must match exactly.
- **Before every launch:** the version must still be in the catalog and not revoked (removing it from the catalog also stops it), and the folder must hold exactly the signed files. `plugin.log` is ignored.

**Making the keys (maintainer, once; tracked in #90):**
1. `minisign -G -p root.pub -s root.key`, then keep `root.key` offline.
2. `minisign -G -p catalog.pub -s catalog.key`.
3. Put the root public key (the base64 line of `root.pub`) in `app/resources/plugin-trust/root.pub`.
4. Write `root.json` listing `catalog.pub`'s key and sign it: `minisign -S -s root.key -m root.json`.
5. For each release, write `catalog.json` with a higher `sequence` and sign it with `catalog.key`.
6. Ship both files and their `.minisig` signatures in `app/resources/plugin-trust/`.

**Publishing (#190):** `scripts/publish-plugins.ps1`, with the catalog edits done by `plugin-catalog` (in `crates/plugin-trust`). The order is: zips first, checked, then the signed catalog.
1. Run `powershell -File scripts\publish-plugins.ps1 -Crate plugin-focus-timer[,…] [-Channel beta]`. This dry run:
   - packs each plugin;
   - names its release `plugin-<slug>-vX.Y.Z`, with the asset `<id>-<version>-windows-x86_64.zip`;
   - updates `target\plugin-site\plugins\v2\catalog.json`: it adds the entry with its URL, size and hashes, keeps one previous version per channel, then bumps the sequence and sets a 30-day expiry.
2. Run the same command with `-Release`. It creates the GitHub releases (never marked latest; beta ones as prereleases), then downloads each asset back and checks its SHA-256. A release that already exists is only checked, since releases are immutable.
3. **Maintainer step:** `minisign -S -s <catalog.key> -m target\plugin-site\plugins\v2\catalog.json`. The key never passes through an agent.
4. Run `-Pages`. It pushes `plugins/v2/` to the `gh-pages` branch, and refuses an unsigned or since-changed catalog.

To withdraw a version: `plugin-catalog yank|revoke <catalog.json> <id> [version…] --reason …`, then `-Resign`, sign, and `-Pages`. The weekly re-sign is `-Resign`, sign, `-Pages`. GitHub Pages must be turned on for the `gh-pages` branch once, in the repository settings.

**Developer mode:** `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1` allows unsigned plugins, and the Plugins tab shows a warning.

**Test hook:** with developer mode and a test `FOXGLOVE_CONFIG_DIR` (never the real settings folder), `FOXGLOVE_PLUGINS_AUTOINSTALL=<zip or folder>` installs and enables that plugin at start with all its declared permissions, without the approval dialog. It writes a warning to stderr and to `autoinstall.log` in the config dir. It is meant for the Test lab only.

## Writing a plugin (Rust)

The `foxglove-plugin` crate (`crates/plugin-sdk`, MIT):
- **Connect:** `Plugin::connect(name, version)` says hello and pings every 10 s for you.
- **Calls:** `set_fact`, `fact`, `subscribe` plus `events()`, `devices`, `set_lighting`, and a generic `call`.

The example is `crates/plugin-focus-timer`, a focus/rest timer that publishes `focus.state` and `focus.remaining` and colours the underglow. It asks only for `devices.read`, `lighting.write` and `facts.write:focus.*`, so it runs in the AppContainer.

The second example, `crates/plugin-openrgb-e131`, receives network data through Foxglove (`net.listen:udp:127.0.0.1:5568`) and stays sandboxed; see [openrgb-plugin.md](openrgb-plugin.md).

**Pack a plugin:** `powershell -File scripts\pack-plugin.ps1 plugin-focus-timer` builds it, zips `plugin.toml` and the program into `target\plugins\`, and prints the catalog entry (archive and file SHA-256s) to add to `catalog.json` before signing. Until the signing keys exist, install it with `FOXGLOVE_DEV_UNSIGNED_PLUGINS=1`.
