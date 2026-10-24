# Foxglove local API and Kits, v1

Status: design for #69, with the contract in code (`crates/kit-api`, schema in
`crates/kit-api/schema/v1.json`). This builds on [plugins-and-devices.md](plugins-and-devices.md)
(Palm, Kits, the Den). Nothing in the app speaks it yet. The next PRs add the host, the named
pipe and the Den tab.

## One protocol, three doors

| Who | Door | Permissions |
|---|---|---|
| **Kits** (plugins Foxglove starts) | the Kit's stdin/stdout | what its `kit.toml` declares, approved by the user at install |
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
- **Errors:** JSON-RPC's own codes, plus the Foxglove codes, which mirror Palm's errors:
  1 unsupported, 2 disconnected, 3 busy, 4 timeout, 5 denied, 6 backend, 7 version, 8 rate-limited.

## Methods (v1)

| Method | Kind | Needs |
|---|---|---|
| `initialize`, `ping` | call | — |
| `facts.get`, `facts.subscribe`, `facts.unsubscribe` | call | `facts.read:<pattern>` covering the key/pattern |
| `facts.set` | call | `facts.write:<pattern>`; `device.*` keys are never writable |
| `facts.changed` | event | the subscription |
| `devices.list` / `devices.changed` | call / event | `devices.read` |
| `lighting.state` / `lighting.set` | call | `devices.read` / `lighting.write` |
| `behaviors.list` / `behaviors.invoke` | call | `devices.read` / `behaviors.invoke` |
| `screens.panel_override` | call | `screens.write` |
| `art.status` / `art.select` | call | `devices.read` / `art.write` |
| `key.event` | event | `key.events` (warned: a keylogger) |

The params and results are in the schema. Notes:

- **Facts are the main surface.** Device state appears under `device.<id>.*` (connected, layers,
  battery.right, lighting.*). Kits publish their own facts (`weather.temp`), and the panel and
  other Kits read them.
- **`behaviors.invoke` refuses actions that need confirmation** (error 5). A Kit can never press
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
  Regenerate with `UPDATE_SCHEMA=1 cargo test -p foxglove-kit-api`.
- **SDKs:** the Rust types *are* the Rust SDK (MIT). Python/TypeScript clients get generated from
  the schema later.

## Kits

```toml
# %APPDATA%\Foxglove\kits\com.example.weather\kit.toml
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
- `run` stays inside the Kit's folder;
- unknown keys and permissions are refused.

**Updates:** an update that adds permissions needs the user's OK for those (`added_permissions`).

**Lifecycle** (next PR):
- **Supervision:** Foxglove starts each enabled Kit under `services::spawn_supervised`, with backoff from 1 s to 60 s.
- **Liveness:** a `ping` every 10 s; no answer in 30 s means a restart. Five crashes in ten minutes disables the Kit, with a notice in the Den.
- **Rate limits:** per Kit (calls/s, lighting frames/s, panel overrides); over the limit gets error 8.

**Containment on Windows:**
- **Job Object:** every Kit runs in one, killed with Foxglove, with memory and CPU caps and no breakaway.
- **AppContainer:** Kits that declare neither `net` nor `files` run in one, so no network and no user files.
- **"Trusted" Kits:** the others (vendor SDK Kits need their vendor service) are marked trusted in the Den.

**Den catalog:** curated, first-party, à la carte. Each Kit has an install button, and nothing
vendor-specific is installed by default. Third-party Kits are a later decision.

**License:** Kits are separate programs talking over IPC, so they needn't be GPL. The API crate
is MIT. (A reading of the licenses, not legal advice.)

## Phase 3 plan (M6)

1. **This PR:** `crates/kit-api` (types, permissions, manifest, schema), and this doc.
2. **Host:**
   - a connection loop over any `Read + Write` (stdio and pipe share it);
   - an `initialize` gate, per-call permission checks, facts get/set/subscribe on Palm's store, and rate limits;
   - tests driven by golden transcripts.
3. **Named pipe** with a current-user ACL, plus `foxglove-cli status` as its first client.
4. **Kit runner:** spawn, Job Object/AppContainer, supervision, ping.
5. **Den tab:** installed Kits (status, permissions, log, enable/disable), the catalog, install/uninstall.
6. **Device methods** (lighting, behaviors, panel override, art) wired through Palm, and an
   example weather Kit.
