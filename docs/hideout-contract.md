# Hideout module contract, v1

Status: spec, contract version **1.0** (#905). Nothing in it is built yet beyond what Foxglove's
plugin system already does; the phases that build it are in the Hideout design
(`review/hideout-modular-design.md`, §10). Test vectors: `tests/hideout-vectors/` (#906).

Hideout is the local composition layer for two peer cores, **Foxglove** (the keyboard and the
lights and screens around it) and **Vulpes** (the GPU and local AI; today Arete). This document
is the contract both cores and every module follow: what a module is, how it describes itself,
what it may do, how it is installed and run, how the cores talk, how agents reach them, and how
modules are trusted.

It grows out of Foxglove's plugin system ([local-api.md](local-api.md),
`crates/plugin-api`, `crates/plugin-trust`) and Arete's component installer
(`src/arete/components.py`, `footprint.py`, `selfupdate.py`). Where this spec and the plugin
system agree, the plugin system's code is the reference.

**Words.** MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119. A **reader** is any
program that parses manifests, catalogs or the inventory: the `hideout` CLI, the
`hideout-contract` crate, a core, or Vulpes's pydantic models. A **core** is Foxglove or Vulpes.
An **outside tool** is any client of a core's pipe that is neither the other core nor a module the
core started (scripts, Stream Deck, `foxglove-cli`, an MCP adapter).

## 1. The decided picks

Decided on 11/09 (design §9). Each pick shapes a rule below.

| # | Pick | Where this spec follows it |
|---|---|---|
| 1 | Hideout is a library, a CLI and an inventory file. **No daemon.** | §5.1: nothing in Hideout stays running; the cores supervise. §6.4: discovery is pipe names and run files. |
| 2 | Foxglove features become **built-in modules** that can be switched off. Only integrations and new optional features run as separate processes. | §2: kind `builtin`; §5.5: the core owns a built-in module's on/off switch. |
| 3 | **One Hideout root key** delegates a catalog key per family. | §8: `hideout-root-v1` with family-scoped catalog keys. |
| 4 | **One tray.** Foxglove's tray gets a Vulpes section; Vulpes keeps its own tray only when it runs alone. | §6.4: a core learns that the other runs from its run file and pipe, never by polling. Hideout has no UI. |
| 5 | Arete becomes Vulpes in phase 6, `arete` stays an alias, `C:\AI` is unchanged. | §3.3: the family and provider are named `vulpes` from v1; `arete` is accepted as an alias where names are read. |
| 6 | Lagopus, Zerda and Velox are **module sets** (profiles) of one Vulpes build. | §3.7: an edition is a `content` module that lists modules; switching only enables and disables. |

## 2. Module kinds

Every module has exactly one kind.

| Kind | Runs as | Started by | Has `run` | Examples |
|---|---|---|---|---|
| `core` | its own app | the user, autostart | no | `foxglove`, `vulpes` |
| `builtin` | code compiled into a core, switched on and off by that core | its core | no | `foxglove.overlay`, `foxglove.wallpaper`, `vulpes.image` |
| `service` | a separate process its core supervises; speaks JSON-RPC on stdio | its core | **yes** | `foxglove.mic-mute`, today's plugins, a future `vulpes.sionnach` |
| `runtime` | a pinned third-party program a broker launches; has no API of its own | a broker in its core | no | `vulpes.runtime.llama-cpp`, `vulpes.runtime.comfyui` |
| `content` | data only, read by a core | never runs | no | a board profile, an art pack, an agent pack, an edition |
| `firmware` | images flashed to a device, under Digitalis's own flashing rules | never runs on the PC | no | `digitalis.purpurea` |

Rules:
- A `core` module is shipped by its core's installer, not by `hideout modules install`. It is
  listed so other modules can `require` it and so cross-core grants (§4.4) have a home.
- A `builtin` module's manifest is embedded in its core's binary (§3.8). It is never installed,
  updated or removed on its own; it ships and updates with its core.
- A `firmware` module is installed and verified like `content`, but it is never enabled or
  started. Flashing stays with Digitalis (`crates/flash`, the scripts) and its confirmations.
- Only `service` modules are separate processes the Hideout lifecycle starts and stops.
  `runtime` processes are owned by a broker (the GPU broker in Vulpes), never by Hideout.

## 3. The manifest: `module.toml`

### 3.1 File and syntax

- A module's manifest is a file named `module.toml` at the top of its folder (for `builtin`
  modules: embedded text, §3.8).
- It MUST be TOML 1.0, UTF-8, readable by Rust's `toml` crate and Python's `tomllib`. Readers
  MUST NOT need any other TOML extension.
- **Unknown keys are refused**, at every level (`deny_unknown_fields`, as `Manifest::parse` does
  today). A reader that meets a key it doesn't know MUST reject the manifest with
  `manifest.unknown-key`. New keys come only with a new contract minor (§6.5), and a module that
  uses them sets `contract` to that minor, so an older reader refuses it with
  `manifest.contract` rather than `manifest.unknown-key`.
- Strings are compared exactly (case-sensitive) unless a rule below says otherwise.

### 3.2 Keys

| Key | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | The module's identity and folder name (§3.3). |
| `name` | string | yes | Shown to the user. 1 to 64 characters, not blank. |
| `version` | string | yes | The module's own version, SemVer `MAJOR.MINOR.PATCH` with an optional `-prerelease`. No build metadata (`+…`). |
| `kind` | string | yes | One of §2's kinds. |
| `family` | string | yes | `hideout`, `foxglove`, `vulpes`, `digitalis` or `third-party` (§3.3). |
| `contract` | string | yes | The Hideout contract version it was written for, `MAJOR.MINOR` (§6.5). v1 readers accept `1.0`. |
| `description` | string | no | Plain words for the install card. Default empty. |
| `api` | table, or string | no | Provider APIs it calls, each with the minimum `MAJOR.MINOR` it needs: `api = { foxglove = "1.4", vulpes = "1.0" }`. A plain string means `{ foxglove = "<string>" }` (plugin.toml's form). Keys are providers (§4.1). Default empty. |
| `requires` | array of strings | no | Modules it needs (§3.4). Default empty. |
| `enriches` | array of strings | no | Third-party programs it works with, as `<name>.exe` (same rule as `apps.watch`). When one is missing, the core shows one hint; it never blocks install or enable. Default empty. |
| `run` | array of strings | `service` only | The program and its arguments, relative to the module's folder. MUST be present and non-empty for `service`; MUST be absent for every other kind (`manifest.run`). |
| `provides` | array of strings | no | What it offers (§3.5). Default empty. |
| `permissions` | array of strings | no | What it may touch (§4). Default empty. |
| `[lifecycle]` | table | no | `start` and `data` (§3.6). |
| `[[pin]]` | array of tables | `runtime` only | The upstream archives it is made of (§3.6). At least one for `runtime`; MUST be absent for other kinds. |
| `[edition]` | table | edition content only | The module set an edition switches to (§3.7). |
| `[[actions]]`, `[[secrets]]`, `[[settings]]` | arrays of tables | no | Exactly as in plugin.toml (local-api.md, `crates/plugin-api/src/manifest.rs`): the same keys, the same checks. |

Checks every reader MUST make, each with the error id it reports (§10):

- `id`, `family` and `kind` agree (§3.3): `manifest.id`, `manifest.family`.
- `version` is SemVer as above: `manifest.version`.
- `contract` is `MAJOR.MINOR` and served (§6.5): `manifest.contract`.
- Every `api` value is `MAJOR.MINOR`; every key is a known provider: `manifest.api`. Whether the
  installed provider serves it is checked at install and enable, not at parse (`lifecycle.api`).
- `run[0]` stays inside the module's folder: every path component is a normal name or `.`; no
  `..`, no root, no drive, no prefix (`manifest.run`). This is today's rule.
- `[[secrets]]` needs `foxglove:secrets` (or the bare `secrets`) in `permissions`:
  `manifest.secrets`.
- `[[actions]]` ids and `[[settings]]` names are unique and use `a-z`, `0-9`, `-`; each setting's
  default fits its kind: `manifest.actions`, `manifest.settings`.

Example, a service module:

```toml
id = "foxglove.wallpaper"
name = "Wallpaper Engine sync"
version = "0.3.0"
kind = "service"
family = "foxglove"
contract = "1.0"
description = "Sends your keyboard's layer and lights to Wallpaper Engine wallpapers."
api = { foxglove = "1.4" }
requires = ["foxglove>=0.3"]
enriches = ["wallpaper64.exe"]
run = ["foxglove-wallpaper.exe"]
provides = ["facts:wallpaper.*"]
permissions = [
  "foxglove:facts.read:keyboard.*",
  "foxglove:net.listen:tcp:127.0.0.1:47911",
  "foxglove:apps.watch:wallpaper64.exe",
]

[lifecycle]
start = "on-fact:keyboard.connected"
data = "ask"
```

### 3.3 Ids and families

- An `id` is two or more labels joined by `.`. In `module.toml`, each label MUST match
  `[a-z0-9][a-z0-9_-]*`, and the whole id is at most 128 characters.
- **Reserved first labels:** `hideout`, `foxglove`, `vulpes`, `digitalis`, and `arete` (the alias
  of `vulpes`, pick 5). A module whose id starts with a reserved label MUST have the matching
  `family` (`arete.*` ids are read as family `vulpes`). A `third-party` module MUST NOT use a
  reserved first label, and its id SHOULD be reverse-DNS (`com.example.weather`).
- **First-party ids** are `<family>.<name>` (`foxglove.overlay`), with more labels allowed
  (`vulpes.runtime.llama-cpp`).
- **Core ids** are the single exception to "two or more labels": the `core` modules are exactly
  `foxglove` and `vulpes`, with `family` equal to the id. No other module may have a one-label id.
- **The legacy first-party prefix** `org.foxglove.` (today's plugins, `org.foxglove.mic-mute`) is
  family `foxglove`. It is accepted in `module.toml` only with `family = "foxglove"`.
- Before the rename (phase 6) the Vulpes side is Arete. The contract still says `vulpes`
  everywhere (family, provider, pipe, catalog), so nothing changes when the rename lands.
  Readers MUST accept `arete` as an alias of `vulpes` in `family`, in `api` keys and as a
  permission provider, and MUST write `vulpes`.

### 3.4 `requires`

Each entry is `<id>` or `<id>>=<version>`:
- `<id>` alone: that module is installed (or is a running core, or is one of a core's built-in
  modules) and enabled.
- `<id>>=<version>`: the same, at that version or newer. `<version>` is SemVer, and may be
  shortened to `MAJOR` or `MAJOR.MINOR` (missing parts are 0).
- No other operators in v1 (`<`, `=`, `~`, `^` are refused, `manifest.requires`).
- Checked at install and at enable (`lifecycle.requires`). Disabling or removing a module that an
  enabled module requires MUST be refused with `lifecycle.required-by`, naming the dependents,
  unless the user disables or removes them too.
- Cycles are refused at install (`lifecycle.requires`).

### 3.5 `provides`

Each entry is `<what>` or `<what>:<argument>`. Unknown `<what>` values are refused
(`manifest.provides`).

| Entry | Means | Kinds |
|---|---|---|
| `facts`, `facts:<pattern>` | It publishes facts (the pattern names which; pattern grammar as `facts.read`). | service, builtin |
| `panel-layout` | It draws on the info screen. | service, builtin |
| `lighting-effect` | It drives lights through leases. | service, builtin |
| `action` | It has key actions (`[[actions]]`). | service, builtin |
| `backend` | It is a device backend. | service, builtin |
| `methods:<pattern>` | It serves these pipe methods for its core (`vulpes.image.*`). The pattern is a method name or ends in `.*`. | builtin, service |
| `content:<type>` | It holds content of a type its core reads. v1 types: `board-profile`, `art-pack`, `lighting-preset`, `lighting-catalog`, `wallpaper-manifest`, `agent-pack`, `style-pack`, `eval-pack`, `template-pack`, `edition`. | content |
| `firmware:<board>` | It holds firmware images for a board (`firmware:purpurea`). | firmware |

A `content` module MUST provide at least one `content:` entry; a `firmware` module at least one
`firmware:` entry (`manifest.provides`). The first five entries are plugin.toml's `Provides`
values, unchanged.

### 3.6 `[lifecycle]` and `[[pin]]`

`[lifecycle]` keys:

| Key | Values | Default | Meaning |
|---|---|---|---|
| `start` | `on-enable`, `on-demand`, `on-fact:<pattern>` | `on-enable` | When the owning core starts a `service`, or activates a `builtin`. `on-demand`: when a call first needs it. `on-fact:<pattern>`: when a fact matching the pattern is first set (an event, never a timer). Ignored for `core`, `runtime`, `content` and `firmware`. |
| `data` | `keep`, `remove`, `ask` | `ask` | What `remove` does with the module's `data\` folder (§5.2). |

`[[pin]]` (runtime modules; Arete's component pins, now signed):

| Key | Type | Required | Meaning |
|---|---|---|---|
| `url` | string | yes | An `https://` URL of an immutable upstream file. |
| `sha256` | string | yes | Its SHA-256: 64 lowercase hex characters. |
| `size` | integer | no | Its size in bytes; the download limit. |
| `unpack` | string | no | `zip` (default) or `none`. |
| `strip_top` | bool | no | Drop the archive's top folder when unpacking. Default `true` (Arete's default). |

A runtime may have several pins (llama.cpp needs the server zip and the CUDA runtime zip). The
design sketch showed one `[pin]` table; v1 uses an array so every Arete component fits.

### 3.7 Editions

An edition (Lagopus, Zerda, Velox; pick 6) is a `content` module with
`provides = ["content:edition"]` and an `[edition]` table:

```toml
id = "vulpes.edition.zerda"
name = "Zerda (light)"
version = "1.0.0"
kind = "content"
family = "vulpes"
contract = "1.0"
requires = ["vulpes>=1.0"]
provides = ["content:edition"]

[edition]
core = "vulpes"
modules = ["vulpes.chat", "vulpes.search", "vulpes.mcp"]
```

- `core` names the core the edition belongs to; `modules` lists that core's modules the edition
  has **enabled**. Every other module of that core is disabled when the edition is chosen.
- Choosing an edition MUST only enable and disable modules. It MUST NOT install, update or remove
  anything; a listed module that isn't installed is reported as missing (`lifecycle.edition`),
  and the switch still applies to the rest.
- Edition settings (beyond the module list) are reserved for a later contract minor.

### 3.8 Built-in manifests

- A core embeds one manifest per built-in module (for example with `include_str!`), and its own
  `core` manifest. They are full `module.toml` texts and pass the same parser.
- A running core lists them in its run file (§6.4), so `hideout modules list` shows them
  without asking the core.
- Built-in modules have no folder, no files to verify and no catalog entry. Their trust is the
  core's own installer and update signature.

### 3.9 Legacy manifests: `plugin.toml` and `kit.toml`

They keep working unchanged. A reader looks in a module's folder for `module.toml`, then
`plugin.toml`, then `kit.toml`, and uses the first one found. A legacy file:

- is parsed exactly as `Manifest::parse` does today: the same required keys (`id`, `name`,
  `version`, `api`, `run`), the same id rule (two or more labels of ASCII letters, digits, `-`,
  `_`; upper case allowed), unknown keys refused;
- MUST NOT contain `kind`, `family`, `contract`, `requires`, `enriches`, `[lifecycle]`, `[[pin]]`
  or `[edition]` (they are unknown keys there, `manifest.unknown-key`);
- then reads as if it said:
  - `kind = "service"`;
  - `family = "foxglove"` when its id starts with `org.foxglove.` or `foxglove.`, otherwise
    `family = "third-party"`;
  - `contract = "1.0"`;
  - `api = { foxglove = "<its api>" }`;
  - every permission with the `foxglove:` provider (§4.1);
  - `[lifecycle] start = "on-enable"`, `data = "remove"` (today's uninstall deletes the folder,
    `data\` included).

A legacy file's `api` must be served by the installed Foxglove, as today (`manifest.api`).

## 4. Capabilities and permissions

### 4.1 Grammar

```
permission = [ provider ":" ] body
provider   = "foxglove" | "vulpes"            ; "arete" is read as "vulpes"
```

- **Parsing:** split at the first `:`. If the part before it is a provider, the rest is the body
  for that provider. Otherwise the whole string is a Foxglove body (legacy, unprefixed). No
  Foxglove body begins with a provider name, so this is unambiguous.
- **Canonical form** is always prefixed: `foxglove:lighting.write`. Readers MUST store and compare
  permissions in canonical form (the inventory, `added_permissions`). `module.toml` SHOULD use the
  prefixed form; the unprefixed form is accepted for Foxglove bodies.
- An unknown provider or an unknown body is refused (`permission.unknown`); a known body with a
  bad argument is refused (`permission.argument`).
- `hideout` and `digitalis` are not providers in v1: they serve no permissions.

**Foxglove bodies** are Foxglove's v1 grammar, unchanged (`crates/plugin-api/src/permission.rs`
is the reference): `facts.read:<pattern>`, `facts.write:<pattern>` (never `device.*` or `*`),
`devices.read`, `lighting.write`, `behaviors.invoke`, `screens.write`, `art.write`,
`key.events`, `hid.raw`, `profiles.write`, `focus.write`, `net`,
`net.connect:<host>:<port|*>`, `net.listen:udp:127.0.0.1:<port>`,
`net.listen:tcp:127.0.0.1:<port>`, `files`, `audio.inputs.read`, `audio.inputs.mute`, `secrets`,
`app.windows`, `apps.watch:<name>.exe`. New Foxglove bodies come only with a Foxglove API minor.

**Vulpes bodies** (new in v1):

| Body | Lets the holder | Argument |
|---|---|---|
| `gpu.status` | read the GPU broker's state and hear `vulpes.gpu.changed` | — |
| `gpu.request:<class>` | ask for the GPU as a tenant of that class, and release it | `chat`, `image` or `any` (`any` covers both) |
| `ask:<agent>` | ask a named Vulpes agent a question | an agent name: `[a-z0-9][a-z0-9-]{0,63}`, or `*` for any agent |
| `image.generate` | start an image job (Vulpes's safety and privacy rules still apply) | — |
| `secrets.ref:<scheme>` | pass secret *references* of that scheme for Vulpes's vault to resolve; never the values | a scheme: `[a-z][a-z0-9+.-]{0,31}` (`op`, `env`) |

**Sensitive** (the install dialog warns): Foxglove's `key.events`, `hid.raw` and `net` as today,
plus `vulpes:ask:*` and `vulpes:secrets.ref:*` (any scheme).

### 4.2 Granted at install

- The user approves a module's whole `permissions` list when installing it. Nothing is granted
  that isn't listed.
- An update whose manifest adds permissions (canonical form, compared as sets; today's
  `added_permissions`) MUST wait for the user's approval of the added ones. Until then the old
  version stays installed and running.
- Outside tools ask for permissions in `initialize`, and the user approves them once per tool, as
  today (local-api.md).
- A built-in module's permissions are granted with its core. It still MUST list them, so the
  same checks and the same lists apply to it.

### 4.3 Signing is provenance, not permission

A signed module gets only what the user granted. A catalog signature proves where the files came
from; it never grants, widens or skips a permission prompt.

### 4.4 Cross-core grants

- A core's own `core` manifest lists what it may call on the other core, in canonical form. For
  example Vulpes's lists `foxglove:lighting.write` and `foxglove:facts.read:keyboard.*`, and
  Foxglove's lists `vulpes:gpu.status`.
- The user approves them once, when the second core is installed (or when an update adds some).
  Hideout records the approval in the inventory (§5.3), so neither core shows a second dialog.
- A core applies a cross-core grant only to a connection whose client process is the other core:
  the pipe client's process id (Windows `GetNamedPipeClientProcessId`) equals the `pid` in that
  core's run file and the process's image is that run file's `exe` (§6.4). Any other client is an
  outside tool.

### 4.5 The never-list

These are **never grantable to an MCP adapter, to another core or through any cross-core grant**.
A manifest of kind `core` that lists one of them for the other provider is invalid
(`permission.never`). An outside tool or MCP adapter that asks for one in `initialize` gets it
refused (it is left out of `granted`), and calls that need it answer error 5 (DENIED).

| Never | Why |
|---|---|
| `foxglove:key.events` | a keylogger |
| `foxglove:hid.raw` | raw device access |
| Actions that need confirmation (`behaviors.invoke` of a `Confirm` behaviour: bootloader, reset, BT clear) | only a user's click in Foxglove may do them; `behaviors.invoke` already refuses them for every client |
| Another module's secrets (`foxglove:secrets` reaches only the caller's own; outside tools get none; `vulpes:secrets.ref` passes references, never values) | secrets stay with their owner |
| Starting or ending a gaming reservation (`vulpes.gpu.reserve`, `vulpes.gpu.unreserve`) | Arete's rule: only an explicit user action changes it (§6.7) |

`service` modules may still hold `key.events` and `hid.raw` from their own core, granted by the
user at install with the warning, as plugins can today. The never-list is about the doors between
cores and to agents.

## 5. Lifecycle

### 5.1 One implementation, no daemon

- The lifecycle is implemented once, in the `hideout-core` crate and the `hideout` CLI built on
  it. Foxglove links the crate; Vulpes runs the CLI (through `proc`) with `--json`. Foxglove's
  Plugins tab and `arete components` are front ends to the same lifecycle.
- **Nothing in Hideout stays running** (pick 1). Every `hideout` command does its work and exits.
  It MUST NOT leave a process, a scheduled task, a service or a timer behind.
- **The owning core runs modules.** Foxglove starts its `service` modules under
  `spawn_supervised` (backoff 1 s to 60 s; five crashes in ten minutes disables the module, with a
  notice). Vulpes's broker starts runtimes. A core notices inventory changes made by the CLI by
  a file-change notification on the inventory, not by polling.
- **Status never starts anything**, in either core or in the CLI: `list`, `verify` and every
  status call read only.

### 5.2 Folders

```
%LOCALAPPDATA%\Hideout\
  inventory.json          the record of installed modules and grants (§5.3)
  modules\<id>\           an installed module; its manifest at the top
  modules\<id>\data\      the module's own state; writable; not hashed
  modules\<id>.new\       an install or update being staged
  modules\<id>.prev\      the previous version, kept for rollback (one only)
  catalogs\<family>\      the last accepted root.json, catalog.json and signatures
  run\<core>.json         run files (§6.4)
  bin\hideout.exe         shipped in both cores' installers; the newer copy wins
```

- `HIDEOUT_HOME`, when set, replaces `%LOCALAPPDATA%\Hideout` (for tests, as
  `FOXGLOVE_CONFIG_DIR` does for Foxglove's settings).
- Until phase 3 moves them, Foxglove's plugins stay in `%APPDATA%\Foxglove\plugins\<id>\` and
  Arete's components in `C:\AI\runtimes\<name>\`; readers read them in place. The move keeps the
  old folder readable, as `kits\` became `plugins\`.
- A module's files outside `data\` are written only by the lifecycle. The module MUST NOT write
  them.

### 5.3 The inventory

`inventory.json`, UTF-8 JSON, written atomically (write `inventory.json.tmp`, then rename):

```json
{
  "format": "hideout-inventory-v1",
  "modules": {
    "foxglove.wallpaper": {
      "version": "0.3.0",
      "kind": "service",
      "family": "foxglove",
      "source": "catalog",
      "channel": "stable",
      "enabled": true,
      "installed": "2026-11-09T12:00:00Z",
      "granted": ["foxglove:apps.watch:wallpaper64.exe", "foxglove:facts.read:keyboard.*",
                  "foxglove:net.listen:tcp:127.0.0.1:47911"],
      "files": { "foxglove-wallpaper.exe": "<sha256>", "module.toml": "<sha256>" },
      "footprint": [],
      "previous": "0.2.1"
    }
  },
  "cross_core": {
    "vulpes": ["foxglove:facts.read:keyboard.*", "foxglove:lighting.write"]
  },
  "seen": { "root_version": 1, "epoch": 0, "families": { "foxglove": 12, "vulpes": 3 } }
}
```

- `source` is `catalog` or `sideload` (§8.6). `files` maps every file outside `data\` (path with
  `/`) to its SHA-256, as Arete's record does. `footprint` lists OS changes the module made that
  `remove` must undo (Arete's `footprint.py` items: `{what, kind, path}` with `kind`
  `path | credential | shortcut | setting`). `previous` is the version in `<id>.prev`, if any.
- `granted` and `cross_core` are in canonical form and sorted.
- `seen` is the anti-rollback state (§8.4). It never goes down.
- Built-in modules are not in `modules`; their on/off state belongs to their core (§5.5).
- A reader that finds a `format` it doesn't know MUST NOT write the file.

### 5.4 Steps

Every step can be run as `hideout modules <step> <id>` (and `--json` for a machine-readable
answer). Steps check in the order written and stop at the first failure, with its error id.

| Step | What happens |
|---|---|
| **install** | (1) Fetch the archive from the signed catalog (§8), or take a local zip in developer mode (§8.6). (2) Its SHA-256 MUST equal the catalog entry's; the version MUST NOT be yanked or revoked (`trust.*`). (3) Unpack to `modules\<id>.new\`; refuse entries with `..`, absolute paths or Windows-reserved names. (4) The folder MUST hold exactly the entry's files with their hashes (`trust.files`). (5) Parse the manifest; its `id` and `version` MUST equal the entry's (`trust.mismatch`). (6) Check `contract`, `api` and `requires` (`lifecycle.*`). (7) For a runtime: download every pin, check `sha256` (and `size`), unpack (Arete's `download` and `extract`). (8) Show the permissions; stop if the user declines (`lifecycle.declined`). (9) Rename `<id>.new` to `<id>` atomically. (10) Record it in the inventory, `enabled: false`, with every file's hash. |
| **verify** | **Shallow, before every launch** of a `service` (and before a broker starts a runtime): the installed version is still in the accepted catalog and not revoked (a yanked version still runs), and the folder holds exactly the signed files (Foxglove's `check_files`, `data\` skipped). **Deep, on demand** (`hideout modules verify --deep`): every file's hash against the inventory, and every runtime's unpacked files against their install record (Arete's `deep_verify`). Verify never changes anything. A failed shallow verify MUST stop the launch. |
| **enable** | Check `requires` again. Grant the approved permissions. Set `enabled: true`. The owning core starts it per `start`. |
| **disable** | Set `enabled: false`. The core stops it: its connection closes, so its leases, panel holds, watches and streams end (§6.3). Files, data, secrets and grants stay. |
| **update** | Install the new version as in install, steps 1–8, into `<id>.new`. If it adds permissions, wait for approval (§4.2). Stop the old one, move `<id>` to `<id>.prev` (replacing an older `.prev`), rename `<id>.new` to `<id>`, start it if enabled, and record `previous`. |
| **rollback** | Only when `<id>.prev` exists: stop the module, swap `<id>` and `<id>.prev`, verify (shallow), start it if enabled. A rollback to a revoked version is refused (`trust.revoked`). |
| **remove** | Stop it and kill its Job Object tree. Delete its secrets (Credential Manager `Foxglove/plugin/<id>/*` and `vulpes/<id>/*`), undo its `footprint`, delete its files and `<id>.prev`. Then `data\` per `[lifecycle] data`: `keep` leaves `data\` in place (a later install of the same id finds it), `remove` deletes it, `ask` asks the user (the CLI without a terminal treats `ask` as `keep`). Remove the inventory record. |

- `hideout modules list` lists every installed module, every running core and its built-in
  modules (from run files), with `id`, `version`, `kind`, `family`, `enabled`, `source` and any
  problem. It changes nothing.
- A step interrupted at any point leaves either the old state or the new one: the rename is the
  commit point, and a leftover `<id>.new` is deleted by the next step that touches `<id>`.

**CLI exit codes:** 0 done; 1 refused (a check failed; the error id is printed, and is
`error.id` in `--json`); 2 usage; 3 needs approval (permissions or `data = "ask"` without a
terminal); 4 busy (another `hideout` holds the lock file `inventory.lock`).

### 5.5 Built-in modules

- A built-in module's on/off switch lives in its core's settings (Foxglove's store; Vulpes's
  config), and the existing switches keep working (`FOXGLOVE_DISABLE=<name>` turns a built-in
  off for one run).
- Turning a built-in module off MUST leave typing, the core's brokers and every other module
  working (the keyboard-first rule). With every built-in off, Foxglove is the tray, the device
  layer and the brokers.
- `hideout modules enable|disable` on a built-in module answers `lifecycle.builtin` and names the
  core's setting. The CLI never edits a core's settings.

## 6. IPC

### 6.1 Wire

- **JSON-RPC 2.0**, one UTF-8 message per line (`\n`-terminated, no embedded newlines), at most
  256 KiB per line. This is Foxglove's v1 wire, unchanged (`crates/plugin-api/src/rpc.rs`).
- Requests carry an `id` and get exactly one response. Notifications (events) have no `id`.
- **`initialize` comes first** on every connection; anything else before it answers error 5.
- **Error codes:** JSON-RPC's own (−32700, −32600, −32601, −32602, −32603), then the shared codes
  1 unsupported, 2 disconnected, 3 busy, 4 timeout, 5 denied, 6 backend, 7 version,
  8 rate-limited. Both cores use them with these meanings. New shared codes come only with a
  contract minor; a core MUST NOT give its own meaning to codes 1–99.

### 6.2 Transports

| Transport | Who | Permissions |
|---|---|---|
| **stdio** | `service` modules, which their core starts | the module's granted permissions |
| **`\\.\pipe\foxglove-<user>`** (exists) | outside tools, `foxglove-cli`, the Foxglove MCP adapter, Vulpes | outside tools: approved per tool; Vulpes: the cross-core grant |
| **`\\.\pipe\vulpes-<user>`** (new) | outside tools, the Vulpes MCP adapter, Foxglove | the same |

- `<user>` is `%USERNAME%` keeping only ASCII letters, digits, `-` and `_` (Foxglove's
  `pipe::name()` today).
- Pipes MUST be created with a current-user-only ACL and `PIPE_REJECT_REMOTE_CLIENTS`.
- Vulpes serves its pipe while its UI or tray process runs.
- **No network listener.** Loopback WebSockets stay opt-in adapters with a per-install token
  (Foxglove's live feed, Arete's web UI). Tailnet exposure stays a Vulpes-only opt-in and MUST
  NOT forward to Foxglove's pipe.

### 6.3 Connections and leases

- Anything a connection holds ends when the connection ends: lighting leases, panel holds, GPU
  tenancy, subscriptions, app watches, streams. A crash or a kill ends it the same way.
- Liveness is the connection, not a heartbeat (#281): a core sees a module exit from its process
  handle; a module sees its core go when stdin ends; a pipe client sees the pipe close.
- A call with no answer fails after 30 s on the caller's side (error 4).

### 6.4 Discovery

No polling. A client finds a core in three ways, all event-driven or one-shot:

1. **Fixed pipe names.** Try to open the core's pipe. If it isn't there, the core isn't running.
2. **Run files.** A running core writes `%LOCALAPPDATA%\Hideout\run\<core>.json` after its pipe is
   listening and deletes it on a clean exit:
   ```json
   {
     "format": "hideout-run-v1",
     "core": "vulpes",
     "version": "0.9.0",
     "pid": 4242,
     "exe": "C:\\Users\\me\\AppData\\Local\\Programs\\Arete\\.venv\\Scripts\\pythonw.exe",
     "pipe": "\\\\.\\pipe\\vulpes-me",
     "started": "2026-11-09T12:00:00Z",
     "contract": "1.0",
     "apis": { "vulpes": "1.0" },
     "builtins": [ { "id": "vulpes.chat", "version": "0.9.0", "enabled": true } ]
   }
   ```
   A reader MUST treat a run file as stale (and ignore it) unless a process with `pid` is alive,
   its image path is `exe` (case-insensitive) and it started no later than `started` plus 60 s.
   A stale file is never an error; the next run of that core overwrites it.
3. **Retries are event-gated:** on a user action, or when the other core's window appears (the
   `apps.watch` hook), or when a run file is created (a directory change notification on `run\`).
   Never on a timer.

A core with the other core's run file and pipe present shows that core's section in its own
tray (pick 4: Foxglove's tray shows Vulpes; Vulpes shows its own tray only when Foxglove isn't
running). Hideout itself has no UI.

### 6.5 Versioning

Two version numbers, each `MAJOR.MINOR`:
- **The contract version** (this document): `1.0`. It covers manifests, the inventory, run files,
  catalogs and the `initialize` fields below.
- **Each provider's API version:** Foxglove's local API (1.13 today, `API_VERSION`), and Vulpes's
  pipe API (1.0 when H4.1 ships).

Rules for both:
- **A minor only adds:** new methods, new optional fields, new events, new manifest keys, new
  permission bodies. Both sides ignore unknown fields in messages (not in manifests, §3.1).
- **A major breaks.** A core serves the current major and the one before it for at least one
  release cycle.
- **"Served"** means: same major, and the server's minor is at least the client's (`Version::
  served_by`). A reader at contract 1.x accepts manifests whose `contract` is `1.0` to `1.x`.
- **Schemas are generated from the types** and checked in: Foxglove's `schema/v1.json` today,
  `schema/hideout-v1.json` from H1.2, and pydantic models for Vulpes generated from it. A golden
  test fails when the types and the schema drift.

**`initialize`** keeps Foxglove's v1 shape and adds two optional fields:

```json
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{
     "api":"1.0","contract":"1.0",
     "client":{"name":"vulpes","version":"0.9.0"},
     "permissions":["foxglove:facts.read:keyboard.*"]}}
← {"jsonrpc":"2.0","id":1,"result":{
     "api":"1.13","app":"0.3.0","contract":"1.0",
     "granted":["facts.read:keyboard.*"]}}
```

- `api` is the version of **the serving core's** API the client was built for (Foxglove's on
  Foxglove's pipe, Vulpes's on Vulpes's). Unchanged meaning on Foxglove's pipe.
- `contract` (new, optional; absent means `1.0`): the contract the client speaks. The result's
  `contract` is the one the core speaks. A client whose contract major isn't served gets error 7
  and the connection closes, as for `api`.
- `permissions` MAY be written prefixed or unprefixed. In the result, `granted` uses the form
  each core used before this contract (Foxglove: unprefixed) on API 1.x, so existing clients keep
  working; a client compares in canonical form.
- A cross-core connection (§4.4) gets its cross-core grant; anything it asks for beyond that is
  left out of `granted`.

### 6.6 Method names

- Foxglove's v1 method and event names stay unprefixed on Foxglove's pipe for good (`facts.get`,
  `lighting.lease`).
- Every new cross-core method and event is named `<family>.<area>.<verb>`
  (`vulpes.gpu.request`, `foxglove.wallpaper.pause`). Every method on Vulpes's pipe starts with
  `vulpes.`, except `initialize` and `ping`.
- A method name says which provider's permission it needs; the core's method table is the
  reference (local-api.md for Foxglove).

### 6.7 Vulpes pipe methods (reserved by this contract; built in H4.1)

| Method | Kind | Needs |
|---|---|---|
| `vulpes.gpu.status` | call | `vulpes:gpu.status` |
| `vulpes.gpu.request {class, why}` | call | `vulpes:gpu.request:<class>` (or `:any`); refused with error 3 while the GPU is reserved for gaming |
| `vulpes.gpu.release` | call | the tenancy it releases; also released when the connection ends |
| `vulpes.gpu.changed` | event | `vulpes:gpu.status` |
| `vulpes.gpu.reserve` | call | never grantable (§4.5). Accepted only from Foxglove's core connection (§4.4), which MUST send it only in direct answer to the user pressing a key bound to Foxglove's "Free GPU" action. |
| `vulpes.gpu.unreserve` | — | not served on the pipe in v1. Only Vulpes's own UI or CLI, on an explicit user request, ends a reservation. |

Gaming is a shared signal and neither side acts on the other's alone: Vulpes publishes
`vulpes.gpu.reserved`, Foxglove publishes `keyboard.game_mode`, and Foxglove's game mode never
reserves the GPU by itself.

## 7. The MCP bridge

- **MCP is the door for agents, not for modules.** Modules use the pipes. Agents use MCP.
- **Each core ships one stdio MCP adapter, which is a client of its own core's pipe** and nothing
  else. Permissions, rate limits and leases apply to it as to any outside tool.
  - **Vulpes:** `arete mcp` today, `vulpes mcp` after the rename (with `arete mcp` kept). Its tools
    don't change.
  - **Foxglove:** `foxglove-cli mcp` (new, H4.2). It connects as the outside tool
    `mcp:<client>`, where `<client>` is the MCP client's `clientInfo.name` keeping only ASCII
    letters, digits, `-`, `_` and `.` (at most 64), so the user approves each agent client once.
- **There is no aggregate Hideout MCP server.** Each adapter keeps its core's rules in its core's
  code.
- **Vulpes agents reach Foxglove through Arete's `mcp_client`** pointed at `foxglove-cli mcp`, so
  Vulpes needs no Foxglove-specific code. Foxglove modules reach Vulpes through its pipe with
  `vulpes:` permissions.

**Foxglove adapter tools (v1):**

| Tool | Calls | Needs |
|---|---|---|
| `status` | `initialize` result, `devices.list` | `devices.read` |
| `devices` | `devices.list` | `devices.read` |
| `keyboard_facts` | `facts.get` for `keyboard.layer`, `keyboard.layers`, `keyboard.right.battery`, `keyboard.link`, `keyboard.connected` | `facts.read:keyboard.*` |
| `screens` | `screens.list` | `devices.read` |
| `lights` | `lighting.state` | `devices.read` |
| `lighting_lease {device, color, effect?, priority?, ttl_s}` | `lighting.lease`, then `lighting.release` | `lighting.write` |
| `panel_show {device, bitmap, ttl_ms, screen?}` | `screens.panel_override` | `screens.write` |
| `art_select {device, art}` | `art.select` | `art.write` |
| `profile_activate {profile}` | `profiles.activate` | `profiles.write` |

- **Write tools are time-limited.** `lighting_lease` takes a required `ttl_s` from 1 to 600; the
  adapter releases the lease at the TTL, when the MCP session ends, or when it exits, whichever
  is first. `panel_show` is at most 60 s and `ttl_ms` 0 (hold) is refused.
- **The adapter MUST NOT expose** anything on the never-list (§4.5): no tool for `key.events`,
  `hid.raw`, `behaviors.invoke`, secrets or a GPU reservation. It MUST NOT ask for those
  permissions in `initialize`, and the core refuses them if it does.
- **Module lifecycle and grants are not reachable through MCP** in v1: no tool installs, enables,
  disables, updates or removes a module, or approves a permission. Those stay with the user.

## 8. Trust

### 8.1 Three kinds of trust, kept apart

1. **Module provenance** (this section): a minisign chain from one Hideout root. Works without
   Authenticode.
2. **App updates:** each core keeps its own update key and channel (Foxglove's Ed25519
   `latest.json`; Vulpes's gated source update until it ships a package). Not covered here.
3. **Code signing** (Authenticode, #383): stops SmartScreen warnings and unblocks Foxglove's sparse
   identity. **No part of the module system waits on it.**

### 8.2 The chain

```
Hideout root key (offline, minisign; public key pinned in both cores and hideout.exe)
  └─ signs root.json  (format hideout-root-v1)
       └─ names catalog keys, each bound to ONE family and a list of id scopes
            └─ each family's key signs that family's catalog.json
                 └─ pins each module version: archive SHA-256 and every file's SHA-256
                      └─ a runtime's pins (url + sha256), inside its signed module.toml
```

- **One root** (pick 3). The plugin root keys were never made (#90), so the Hideout root is the
  first root and no rotation is needed. Foxglove's `foxglove-plugin-root` format stays readable
  (as a root with one family, `foxglove`, scoped to every id) so existing test catalogs still
  verify; it MUST NOT be pinned in a release once the Hideout root exists.
- **Signatures** are minisign (Ed25519) over the exact bytes of the file, in a `.minisig` file
  next to it (`root.json.minisig`, `catalog.json.minisig`), checked with `minisign-verify` in
  Rust and an equivalent Ed25519 minisign check in Python (or by running `hideout`).
- **Times** are RFC 3339 UTC in exactly the form `YYYY-MM-DDTHH:MM:SSZ`, compared as text.

### 8.3 `root.json`

```json
{
  "format": "hideout-root-v1",
  "version": 1,
  "epoch": 0,
  "expires": "2027-11-09T00:00:00Z",
  "families": {
    "foxglove":  { "keys": ["RWQ...base64..."], "scope": ["foxglove.*", "org.foxglove.*", "com.example.*"] },
    "vulpes":    { "keys": ["RWQ...base64..."], "scope": ["vulpes.*", "arete.*"] },
    "digitalis": { "keys": ["RWQ...base64..."], "scope": ["digitalis.*"] }
  }
}
```

- `families` keys are the families of §3.3 (not `third-party`). Each has one or more minisign
  public keys (base64) and a `scope`: a list of id patterns, each an exact id or a prefix ending
  in `.*` (`com.example.*` covers `com.example.weather`, not `com.example`). `*` alone is refused.
- **Third-party modules** are listed in a first-party family's catalog, under a scope the root
  grants that family (`com.example.*` above).
- **Scopes MUST NOT overlap** between families: no id may match scopes of two families. A root
  where they do is invalid (`trust.scope-overlap`).
- A key MAY appear in only one family (`trust.key-reuse`).
- The root is checked as Foxglove's is today: signature by the pinned root key, not expired,
  `version` not below `seen.root_version`, `epoch` not below `seen.epoch` (`trust.root-*`).

### 8.4 A family's `catalog.json`

Foxglove's v2 catalog format (`foxglove-plugin-catalog-v2`, `crates/plugin-trust`) with these
additions, under the new format name `hideout-catalog-v1`:

```json
{
  "format": "hideout-catalog-v1",
  "family": "vulpes",
  "epoch": 0,
  "sequence": 3,
  "expires": "2026-12-09T00:00:00Z",
  "modules": [
    {
      "id": "vulpes.runtime.llama-cpp",
      "version": "11476.0.0",
      "kind": "runtime",
      "channel": "stable",
      "sha256": "<archive sha256>",
      "url": "https://github.com/BanditF/vulpes/releases/download/.../vulpes.runtime.llama-cpp-11476.0.0.zip",
      "size": 2048,
      "files": { "module.toml": "<sha256>" }
    }
  ],
  "revocations": [
    { "id": "vulpes.search", "versions": ["0.1.0"], "action": "yank", "reason": "broken search" }
  ]
}
```

A catalog is accepted only if all of these hold:
- its signature verifies with a key the root lists **for the catalog's `family`**
  (`trust.signature`);
- every entry's `id` (and every revocation's `id`) matches that family's scope
  (`trust.scope`); one entry out of scope rejects the whole catalog;
- `epoch` equals the root's (`trust.epoch`); it isn't expired (`trust.expired`);
- `sequence` isn't below `seen.families[<family>]` in the same epoch (`trust.rollback`); a new
  epoch restarts every family's sequence.

Entries:
- `kind` is required in `hideout-catalog-v1`; it MUST equal the manifest's (`trust.mismatch`).
- `modules` is the entry list (`plugins` and `kits` are read as aliases, as today).
- `channel`, `url`, `size`, `revocations` (yank: no new installs; revoke: also stops running)
  keep their v2 meaning.
- A runtime's pins are in its `module.toml`, which is one of the signed `files`; so the pins are
  signed through the catalog, and Arete's source no longer holds them.

After accepting a root and a catalog, the reader stores the new `seen` (§5.3) before using the
catalog; if it can't store it, it MUST NOT go on.

### 8.5 What verification proves

- An archive installs only if its SHA-256 is in an accepted catalog of the family whose scope
  covers its id, and that version isn't yanked or revoked.
- A module runs only if its folder holds exactly the signed files and its version isn't revoked
  (checked before every launch, §5.4).
- None of this grants a permission (§4.3).

### 8.6 Sideloading (developer mode)

- Outside the catalog, a module installs only in developer mode (Foxglove's existing
  `FOXGLOVE_DEV_UNSIGNED_PLUGINS`, and a `HIDEOUT_DEV_UNSIGNED=1` equivalent for the CLI).
- A sideloaded module is recorded with `source: "sideload"`; its shallow verify compares the
  folder against the inventory's hashes instead of a catalog. It still gets only what the user
  granted.
- A sideloaded module MUST NOT use an id inside a first-party scope of the pinned root
  (`trust.sideload-scope`), so it can't pose as a signed one.

## 9. Scope

- Hideout adds **local composition only:** the manifest, trust, the inventory, the lifecycle and
  discovery. Settings, domain logic and UI stay in the cores.
- Hideout has no daemon, no settings and no UI of its own (pick 1). A change that adds one is a
  `needs-decision`.
- **Systems, not products:** brokers and modules are written against a class of hardware or
  software (any HID LampArray, any Raw HID keyboard speaking the link, any GPU tenant, any MCP
  client). Keyboard models and vendor quirks are content modules and catalog data, never code.
- **Licences:** the contract, the SDKs and every service module talk over IPC and stay MIT.
  Built-in modules are part of their core (Foxglove is GPL).

## 10. Error ids

Readers report a failed check with one of these ids (in `--json` output as `error.id`, in Rust as
the error's `id()`, in Python as the exception's `id`). The message text is free; the id is the
contract, and the test vectors check it.

| Id | When |
|---|---|
| `manifest.syntax` | not valid TOML, or a key has the wrong type |
| `manifest.unknown-key` | a key the reader doesn't know, at any level |
| `manifest.missing-key` | a required key is missing |
| `manifest.id` | the id breaks §3.3's grammar |
| `manifest.family` | `family` is unknown or doesn't match the id's reserved first label |
| `manifest.kind` | `kind` is unknown |
| `manifest.version` | `version` isn't SemVer as §3.2 says |
| `manifest.contract` | `contract` isn't `MAJOR.MINOR`, or isn't served |
| `manifest.api` | an `api` value isn't `MAJOR.MINOR`, its key isn't a provider, or (legacy files) it isn't served |
| `manifest.run` | `run` is missing for a service, present for another kind, empty, or leaves the folder |
| `manifest.requires` | a `requires` entry breaks §3.4's grammar |
| `manifest.enriches` | an `enriches` entry isn't `<name>.exe` |
| `manifest.provides` | an unknown `provides` entry, or a content or firmware module without its entry |
| `manifest.lifecycle` | an unknown `start` or `data` value, or a bad `on-fact` pattern |
| `manifest.pin` | `[[pin]]` on a non-runtime, missing on a runtime, or a bad `url`, `sha256` or `unpack` |
| `manifest.edition` | `[edition]` without `content:edition`, or the reverse, or a bad `modules` list |
| `manifest.actions` | as plugin.toml |
| `manifest.settings` | as plugin.toml |
| `manifest.secrets` | `[[secrets]]` without the `secrets` permission |
| `permission.unknown` | an unknown provider or body |
| `permission.argument` | a known body with a bad argument |
| `permission.never` | a core manifest grants itself a never-list permission on the other core |
| `lifecycle.api` | a provider API it needs isn't served by what is installed |
| `lifecycle.requires` | a required module is missing, too old or disabled, or there is a cycle |
| `lifecycle.required-by` | disabling or removing it would break an enabled module |
| `lifecycle.declined` | the user declined the permissions |
| `lifecycle.builtin` | a lifecycle step the CLI doesn't do for built-in modules |
| `lifecycle.edition` | an edition lists a module that isn't installed (reported, not fatal) |
| `trust.signature` | a signature doesn't verify with the key it must |
| `trust.root-expired`, `trust.root-rollback` | the root expired, or is older than `seen` |
| `trust.scope` | a catalog entry or revocation is outside its family's scope |
| `trust.scope-overlap` | two families' scopes cover the same id |
| `trust.key-reuse` | one key is listed for two families |
| `trust.epoch` | a catalog's epoch isn't its root's |
| `trust.expired` | a catalog expired |
| `trust.rollback` | a catalog's sequence is below `seen` |
| `trust.unknown-archive` | the archive's hash isn't in any accepted catalog |
| `trust.yanked`, `trust.revoked` | the version was withdrawn |
| `trust.files` | the folder doesn't hold exactly the signed files |
| `trust.mismatch` | the manifest's id, version or kind differs from the catalog entry |
| `trust.sideload-scope` | a sideloaded module uses an id in a first-party scope |
| `rpc.*` | JSON-RPC transcripts use the numeric codes of §6.1 instead |

## 11. Test vectors

`tests/hideout-vectors/` holds the shared vectors (#906): valid and invalid `module.toml` files,
legacy `plugin.toml` and `kit.toml` files, permission strings, roots and catalogs, and JSON-RPC
transcripts. Its `index.json` gives each file's expected result: `valid` (with the normalized
fields a reader must produce) or `invalid` (with the error id of §10). Both the Rust crate (H1.1)
and Vulpes's models (H1.3) MUST pass every vector; a change to this spec that changes an
expected result changes the vector in the same PR.
