# Foxglove: plugins (Kits) and one device layer (Palm)

Status: approved 2026-10-24 (decisions at the end). Owner: App engineer (session 3). Builds on
[proposal.md](proposal.md) (facts store, provider plugins, local API) and issues #17, #39, #40, #44.

## Why

The app grew around one keyboard. Each feature talks to its own thing:

| Today | Talks to | Shape |
|---|---|---|
| Overlay + keymap editor | `app/src/protocols` (`KeyboardProtocol`, upstream KeyPeek) | keymap read/write, `hid_read()` |
| Screens, art, 0xD7 settings/behaviors | `digitalis::link` (`Link`, `Transport`) | one Raw HID owner, request/reply |
| RGB | `keyboard_settings` over 0xD7 | Corne only |
| Providers (clock, media, volume, app) | `crates/panel` | Windows-only, in-process |

The growth list (QMK, VIA/Vial, other keyboards, six RGB SDKs, plugins, profiles) multiplies
that. Without one seam, every new feature has to know every backend, and every backend every
feature. The proposal is two seams:

- **Palm**: the device layer. One interface every backend implements: ZMK/Digitalis, QMK Raw HID,
  VIA/Vial, and lighting SDKs. Features ask a device what it *can* do, never what it *is*.
- **Kits**: plugins. Out-of-process programs with a versioned JSON-RPC API, declared permissions
  and a supervisor. A Kit can add facts, panel layouts, lighting effects, actions, or a whole
  Palm backend.

Names (forest/fox/finger, open to change): the **Palm** is what all the fingers (devices) hang
off; a **Kit** is a young fox, and Kits live in the **Den** (the plugin host and its settings tab).

Keyboard first still holds: no Kit and no backend is ever needed for typing, and the app runs
with every Kit disabled.

## Palm: the device layer

### Shape: capabilities, not one fat trait

A keyboard, a mouse pad with LEDs, and an OpenRGB controller share almost nothing. So a device
is an identity plus a set of capabilities, each its own small trait. Callers ask
`device.lighting()` and get `Option<&dyn Lighting>`.

```rust
// crates/palm (MIT, no UI, no OS providers; like digitalis)
pub struct DeviceInfo {
    pub id: DeviceId,          // stable across reconnects: backend + vid:pid + serial/path
    pub backend: BackendId,    // "zmk", "qmk", "via", "vial", "openrgb", "dynamic-lighting", ...
    pub name: String,
    pub kind: DeviceKind,      // Keyboard, KeyboardHalf, Mouse, Strip, Other
    pub firmware: Option<String>,
}

pub trait Backend: Send + Sync {
    fn id(&self) -> BackendId;
    /// Cheap, repeatable; the app calls it on OS hotplug events and on demand (no timer).
    fn discover(&self) -> Result<Vec<DeviceInfo>, PalmError>;
    fn open(&self, info: &DeviceInfo, bus: EventSink) -> Result<Box<dyn Device>, PalmError>;
}

pub trait Device: Send {
    fn info(&self) -> &DeviceInfo;
    fn caps(&self) -> Caps;                          // bitset, for UI gating without calls
    fn keymap(&self)    -> Option<&dyn Keymap>      { None } // layout, read, write, save session
    fn layers(&self)    -> Option<&dyn Layers>      { None } // active layers, set/toggle
    fn behaviors(&self) -> Option<&dyn Behaviors>   { None } // list + invoke (#44)
    fn lighting(&self)  -> Option<&dyn Lighting>    { None } // zones/LEDs, color, effect, state
    fn screens(&self)   -> Option<&dyn Screens>     { None } // panel frames, art push/select
    fn power(&self)     -> Option<&dyn Power>       { None } // battery per half, output, BT profile
    fn settings(&self)  -> Option<&dyn DevSettings> { None } // on-device settings (RGB sync)
}
```

- **Events** (layer changed, key pressed, battery, lighting changed, link up/down) go out through
  the `EventSink` into the **facts store** (#17) under `device.<id>.*`, plus a typed event stream
  for things that are not facts (key events). Nothing polls a device from the UI thread.
- **One owner per handle.** Each opened device runs as an actor: one thread owns the handle,
  commands come in on a queue, replies on oneshots. This is `digitalis::link::Link`, generalized;
  the ZMK backend *is* `Link` underneath.
- **Errors** are typed: `Unsupported`, `Disconnected`, `Busy` (someone else owns it), `Timeout`,
  `Denied` (permission), `Backend(String)`. The UI can say something useful for each.
- **Risk levels.** `Behaviors` reports a risk per behavior (bootloader, reset, BT clear, soft off
  are `Confirm`). The UI and the Kit API both enforce the confirmation; a Kit can never invoke a
  `Confirm` behavior without the user clicking.

### Lighting, specifically

Lighting is three separate capabilities (decided on #73):

- **RGB out** (`Lighting`, `Caps::LIGHTING`): the device takes colors or effects.
- **Color in** (`ColorSource`, `Caps::COLOR_IN`): colors computed elsewhere (iCUE, LIGHTSYNC, an
  E1.31 sender like OpenRGB) that Foxglove can mirror onto other devices.
- **Vendor registration** (`Exporter`, backend-level): our devices appear inside another
  ecosystem (a SignalRGB device plugin, Razer Connected Devices) so it can drive them.

`Lighting` (RGB out) covers both keyboards and RGB SDKs:

- `layout()`: zones and LEDs with positions (per-key where the device knows them, so an effect
  can paint by key position across devices).
- `set_frame(&[Rgb])` for per-LED streaming, `set_effect(Effect)` for on-device effects
  (ZMK underglow, QMK RGB Matrix modes), `state()`.
- A **lighting arbiter** above all devices decides who draws: layers with priorities
  (user's base effect < profile < app/game Kit < notification flash), each with a TTL. When
  Foxglove has nothing to say it **releases** control, so iCUE/Synapse/G HUB get their devices
  back instead of freezing on our last frame.

### Backends and where they live

| Backend | Capabilities | In-process or Kit | Notes |
|---|---|---|---|
| **ZMK + Digitalis** (Eyelash Corne) | keymap (Studio RPC), layers, behaviors, lighting (0xD7), screens, power, settings | in-process | Wraps today's `ZmkProtocol` + `Link`. HELLO capability bits (#20) map straight to `Caps` |
| **QMK Raw HID** | layers + key events (the KeyPeek 0xC0 subscription) | in-process | Any QMK board with the KeyPeek module. Never sends 0xAA–0xAB or reset/bootloader/factory commands (#74) |
| **VIA** | keymap r/w, layers, RGB out (VIA custom values: RGB Matrix / underglow / backlight channels) | in-process | VIA protocol over Raw HID, USB only; layouts from VIA JSON definitions. Test board: Keychron K5 Pro. Same command blocklist as QMK Raw HID; doesn't run alongside VIA, Keychron Launcher or OpenRGB |
| **Vial** | VIA + embedded layout | in-process | Out of scope for now (the K5 Pro would need a reflash) |
| **OpenRGB** | color in | Kit | First an E1.31 receiver Kit that OpenRGB's E1.31 output drives (#71); an SDK-server Kit or OpenRGB plugin later. The Rust OpenRGB clients are GPL-2.0, so they live only inside a Kit, never in the MIT crates/ |
| **Windows Dynamic Lighting** | lighting for HID LampArray devices | in-process, `cfg(windows)` | WinRT `LampArray` via the `windows` crate. Background control needs the user to pick Foxglove as the ambient controller in Windows settings |
| **Razer Chroma** | vendor registration | Kit | Only if Connected Devices partner access is confirmed (#73) |
| **Logitech LIGHTSYNC** | color in | Kit | Vendor DLL; proprietary terms (#73) |
| **Corsair iCUE** | color in | Kit | iCUE SDK DLL; proprietary terms (#73) |
| **SignalRGB** | vendor registration | Kit | A SignalRGB custom device plugin that lets SignalRGB drive our board; first vendor Kit (#73) |

Vendor SDKs go into Kits for three reasons: their licenses may not mix with GPL-3.0 linking,
their DLLs can crash or hang, and most users have at most one of them. A Kit can load a DLL;
the app never does. Nothing vendor-specific ships by default: the Den's catalog offers each
vendor Kit with its own install button (see Lifecycle).

Firmware follow-up (for 4 · Firmware): if the Corne itself spoke **HID LampArray**, Windows
Dynamic Lighting and every app on top of it could light it with no Foxglove at all. That is
"keyboard first" for RGB and worth an issue.

### Keeping KeyPeek pulls cheap

`app/src/protocols` and `KeyboardProtocol` are upstream KeyPeek files. They stay. A thin
adapter (`KeyPeekKeymap`) presents any `KeyboardProtocol` as Palm's `Keymap + Layers`, so the
VIA/Vial/ZMK keymap code keeps coming from upstream. New capabilities (lighting, behaviors,
screens) are Foxglove code in `crates/palm` and `app/src/foxglove`.

### General keyboards

- **Identification:** VID/PID + usage page, then the backend's own probe (Digitalis HELLO, Vial
  magic, VIA protocol version, ZMK Studio). First matching backend in a fixed order wins; the
  user can override per device.
- **Layouts:** Vial embeds them; VIA uses its JSON definitions (bundled index + "load file");
  ZMK reports physical layouts over Studio. Unknown boards fall back to a grid from rows × cols.
- **Corne-only features** (screens, art) appear only when `Caps::SCREENS` is set, so another
  board sees a clean app, not greyed-out Corne controls.

## Kits: the plugin system

### Decision: out-of-process, JSON-RPC, any language

| Option | Verdict |
|---|---|
| Rust dylibs | Rejected. No stable ABI, no isolation, a panic or bad pointer takes the overlay down |
| WASM components (wasmtime + WIT) | Real sandbox, but can't load vendor DLLs or open HID, and adds ~10 MB. **Later tier** for pure-logic Kits (see phase 6) |
| **Child process, JSON-RPC 2.0 over stdio** | **Chosen.** Any language, crash-isolated, can load SDK DLLs, and it is the same protocol as the local API |

A Kit is a **local API client that Foxglove starts and scopes**. One schema serves the named
pipe (external tools, Stream Deck, Wallpaper Engine #39), Kits over stdio, and the CLI. We design
and version it once.

### Manifest

```toml
# %APPDATA%\Foxglove\kits\com.example.weather\kit.toml
id = "com.example.weather"         # reverse-DNS, unique
name = "Weather"
version = "1.2.0"                  # the Kit's own semver
api = "^1.0"                       # Kit API it was built for
run = ["weather.exe"]              # or ["python", "main.py"]
permissions = [
  "facts.write:weather.*",
  "net",                           # declared only; see Sandboxing
]
provides = ["facts"]               # facts | panel-layout | lighting-effect | action | backend
```

### API surface (v1)

- **Handshake:** `initialize {api: "1.3", kit, permissions_granted}` → the Kit answers with what
  it provides. A Kit asking for an API major the app doesn't speak is refused with a clear error.
- **Facts:** `facts.get`, `facts.set` (only within granted keys), `facts.subscribe`.
- **Devices (through Palm):** `devices.list`, `device.caps`, `lighting.*`, `layers.*`,
  `behaviors.list/invoke`, `screens.panel.override(bitmap, ttl)`. Every call goes through the
  same capability checks and arbiter as the UI.
- **Backends:** a Kit that `provides = ["backend"]` answers `palm.discover` / `palm.open` /
  per-capability calls; the app wraps it as a normal `Backend`. That is how iCUE/Chroma/LIGHTSYNC
  arrive.
- **UI:** a Kit can declare settings (a typed schema the Den tab renders with egui: toggles,
  numbers, text, choices) and tray/actions entries. No arbitrary UI drawing in v1.
- **Never by default:** `key.events` (a per-key stream is a keylogger) and raw HID writes. Both
  are opt-in permissions with an explicit warning, and raw HID stays limited to unassigned
  byte-0 values.

### Versioning

- `foxglove-kit-api` follows semver. **Minor** = additive only (new methods, new optional
  fields; both sides ignore unknown fields). **Major** = breaking; the app speaks N and N-1 for at
  least one release cycle.
- The schema is generated from the Rust types (`schemars`) and checked in. CI fails if the
  schema changes without a version bump, and golden request/response transcripts per method
  keep old Kits working.
- SDKs: `foxglove-kit` (Rust, MIT) first; a tiny Python/TypeScript client later, generated from
  the same schema.

### Sandboxing (honest version)

A child process has the user's rights unless the OS takes them away. So there are two layers:

1. **API boundary (always):** a Kit can only touch Foxglove, the devices and the facts through
   the permissions it was granted at install. Upgrades that add permissions ask again.
2. **OS containment (per platform):**
   - Windows: every Kit runs in a **Job Object** (killed with the app, memory and CPU caps, no
     breakaway). Kits without `net` / file permissions run in an **AppContainer** (no network,
     no user files). Vendor-SDK Kits need normal rights to reach their service, so they are
     marked "trusted" in the Den.
   - Linux: `bubblewrap`/Landlock when available. macOS: later.

The Den shows each Kit's trust level, permissions, status, restarts and log.

### Lifecycle

- **Catalog, à la carte.** The Den lists a curated catalog of optional first-party Kits (the vendor
  lighting Kits, example providers), each with a description, its permissions and a Download /
  Install button. The catalog is a signed index in the repo's releases; a Kit is a zip with
  a checksum. Nothing in the catalog is installed by default. Third-party Kits are a later
  decision.
- Installed Kits live in `%APPDATA%\Foxglove\kits\*\kit.toml`; uninstalling deletes the folder.
- Started by the existing supervisor (`foxglove::services`, backoff 1 s → 60 s). Heartbeat every
  10 s; no answer for 30 s = restart. Five crashes in ten minutes = disabled, with a notice.
- Rate limits per Kit (calls/s, lighting frames/s, panel overrides). A Kit that floods is
  throttled, never the device.
- License: Kits talk to a GPL program over IPC and are separate programs; they don't have to
  be GPL. The SDK is MIT. (A reading, not legal advice.)

## Settings and profiles (sketch, for phase 5)

- Foxglove-owned settings move to a versioned `foxglove.toml` with migrations; KeyPeek's
  `settings.ini` stays for its own settings so upstream pulls stay clean.
- A **profile** bundles: lighting (per device), panel layout, enabled Kits and their settings,
  overlay options. Profiles switch by hand, from the tray, or by rule on `app.foreground` (game
  hints already do this for one fact). Export/import as one file.

## Phases

Each phase leaves the app shippable. Codex writes the code, one worktree per job.

| # | Phase | Delivers | Done when |
|---|---|---|---|
| 1 | **Palm core** | `crates/palm` (types, capability traits, registry, actor helper, mock backend, a contract test suite every backend must pass); facts store (#17) as Palm's event target; ZMK/Digitalis and `KeyPeekKeymap` adapters | Overlay, screens, art, RGB sync run through Palm with **no visible change**; contract tests pass for mock + ZMK |
| 2 | **QMK, VIA, Vial** | Backends + identification + VIA JSON definitions + grid fallback; VIA lighting channels | A QMK/VIA board shows its layers in the overlay and its RGB in the Lighting tab (test board: the user's Keychron K5 Pro, read-only until the user OKs a flash) |
| 3 | **Local API + Den v1** | JSON-RPC schema v1, named pipe, stdio Kits, manifest, permissions, Job Objects, Den tab, Rust SDK, an example Kit | A sample weather Kit puts a fact on the panel; killing it never affects the overlay |
| 4 | **Lighting** | `Lighting` + arbiter; OpenRGB and Dynamic Lighting in-process; Chroma, LIGHTSYNC, iCUE as Kits after a license check; SignalRGB after research | One effect paints the Corne and an OpenRGB device in sync, and gives them back on exit |
| 5 | **Profiles** | `foxglove.toml` + migrations, profiles, foreground rules, export/import | Switching to a game flips lighting, panel and Kits together |
| 6 | **WASM tier** (optional) | wasmtime + WIT for pure-logic Kits (panel layouts, effects, fact transforms) with a real sandbox | Only if third-party Kits show up |

Phase 1 goes before the open UI issues (#13, #59, #60). Phases 1 → 3 are sequential. Phase 2 and the API schema work in phase 3 can run in parallel
once phase 1 lands. Phase 4 needs 1 and 3.

## Routing

- **2 · Planner:** turn the phases into issues and a milestone, set the order against the
  current queue (#13, #17, #46, #59, #60).
- **4 · Firmware:** HELLO capability bits (#20) become `Caps`; HID LampArray idea.
- **6 · Test lab:** a Palm contract-test run on hardware per backend; a QMK board for phase 2.
- **9 · Scout:** vendor SDK license terms (Chroma, LIGHTSYNC, iCUE) and SignalRGB's integration
  options.

## Decisions (user, 2026-10-24)

1. **Names:** Palm, Kits and Den stay for now and are revisited before release (release checklist).
2. **QMK test board:** the user's Keychron K5 Pro (QMK + VIA, Bluetooth). It is their daily
   keyboard: phase 2 starts read-only (layers, keymap read, lighting over VIA). Any flash needs
   the user's OK, and stock firmware stays restorable.
3. **Kits:** optional but offered, à la carte: a curated first-party catalog in the Den with
   download buttons, including the vendor lighting Kits. Nothing vendor-specific ships by
   default. Third-party Kits are a later decision.
4. **Vendor SDKs:** separate optional Kits from the catalog (follows from 3).
5. **Order:** Palm core (phase 1) before #13, #59 and #60.
