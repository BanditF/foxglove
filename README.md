# Foxglove

Foxglove is ZMK firmware and a Windows app for a keyboard and the lights and screens around it.
The keyboard types on its own; the app adds a layer overlay, draws on the keyboard's screens,
sets its lights and the PC's other lights, and runs plugins. Version 0.2.

- **Firmware:** the keymap and the `nice_view_custom` shield (status screens, the right half's art,
  the PC link), at the repo root where ZMK's build expects them. ZMK v0.3.
- **Foxglove link:** the protocol between the PC and the keyboard over Raw HID. The PC draws the
  left screen (`0xD5`), sends art to the right screen through the left half and the split link
  (`0xD6`), and reads and sets the keyboard's state (`0xD7`). It is defined in
  [`host_link.h`](boards/shields/nice_view_custom/widgets/host_link.h) and
  [`art_relay.h`](boards/shields/nice_view_custom/widgets/art_relay.h); the PC side is
  `crates/foxglove-link`.
- **App:** the Foxglove tray app in `app/`. It grew from [KeyPeek](https://github.com/srwi/keypeek)
  by Stephan Rumswinkel, whose layer overlay and keymap editor it keeps
  ([keeping up with KeyPeek](docs/keypeek-sync.md)).

The keyboard Foxglove is built on is the Eyelash Peripherals Corne (nice!nano v2, two nice!view
screens). It is not foostan's Corne: standard `corne` firmware does not work on it.

## What it works with

Foxglove supports classes of hardware, not single products. Device specifics live in data files
(board profiles, the lighting catalog, VIA definitions), so a new device of a supported class
needs data, not app code ([scope](docs/scope.md)).

| Class | What Foxglove does | Status |
|---|---|---|
| ZMK keyboards with Foxglove's firmware (a board profile plus the shield or the `foxglove_link` shield) | layer overlay, keymap editing over ZMK Studio, screens, art, lights, batteries, firmware updates | Eyelash Corne: built in and tested. Toucan2: profile and CI build. Halcyon Corne e-paper: profile only. Neither has run on hardware |
| Other ZMK keyboards with ZMK Studio and the KeyPeek layer module | layer overlay and keymap editing | as KeyPeek |
| QMK keyboards with VIA | keymap, layers and lights in the overlay, read-only; a per-keyboard pause so VIA or the maker's app can use it | used with a Keychron K5 Pro |
| Windows Dynamic Lighting (LampArray) devices | scenes, a colour per lamp at the lamp's place | while Foxglove's window is in front (Windows' rule without package identity) |
| HID lighting controllers in the lighting catalog (`crates/devices/lighting-catalog.toml`) | colour per zone, off, identify, scenes | HP OMEN 45L case lights. Unknown devices can be added from the app and saved as catalog entries |
| Bluetooth devices and game controllers | battery levels and low-battery notices | from Windows' battery properties |

Optional integrations, each one off unless its program is there: Wallpaper Engine (a wallpaper's
`foxglove.json` gives its scenes, palette and right-screen art,
[format](docs/wallpaper-manifest.md)), OBS, OpenRGB and SignalRGB (over E1.31), and Corsair iCUE.

## Features in 0.2

- **Layer overlay and keymap editor** (KeyPeek's), with save, discard, undo and restore of the
  stock keymap.
- **Screens:** the left screen shows the keyboard's status and a panel the PC draws (clock and
  media, a wallpaper animation, PC load, or notifications you opt into). The right screen plays
  art: one built-in piece, art sent from the PC, and up to 8 pieces kept on the keyboard that play
  without the PC. Dark or light panels for both halves.
- **Art:** one place for every piece (Screens page), rotation through the keyboard's art, art that
  follows the layer, the wallpaper or a scene, and art keys on the keyboard.
- **Lights:** the keyboard's underglow (effect-aware controls, follow the Windows accent or the
  wallpaper), PC lighting devices by zone, colour calibration, and battery dimming done by the
  keyboard itself.
- **Scenes:** one effect across the room's lights (monitors, case, keyboard, mouse) on a map from
  Windows' display layout, with presets, wallpaper looks and triggers (profile, key press, layer
  change).
- **Profiles:** sparse overrides of the settings, switched by hand or by the program in front.
- **Plugins:** sandboxed, signed plugins with the permissions you approve, a local API
  ([docs/local-api.md](docs/local-api.md)), lighting leases, keys F13-F24, per-plugin secrets.
  First-party: Pomodoro, Mic mute, Broadcast Status (OBS), OpenRGB (E1.31), Corsair iCUE mirror.
- **Power:** while plugged in, the keyboard stays lit while the PC is awake and dims when its
  screen goes off; on battery the backlight idles and the halves sleep after 30 minutes.
- **Firmware updates** from the app (both halves, left first, with checks) or the flasher script.
- **Command line:** `foxglove-cli` for status, profiles, art, lights and the lighting sync, over
  the local API.

The full list, with issue numbers, is in the [0.2 release notes](docs/release-notes/0.2.md).

## Install

1. Download the `.msi` from [Releases](https://github.com/BanditF/foxglove/releases) and run it.
   It installs for the current user, without an administrator prompt. The installer isn't signed
   yet, so Windows SmartScreen warns: click **More info**, then **Run anyway**.
2. Plug the keyboard's left half in by USB. Open Foxglove from the tray icon; on the **Keyboard**
   page, under **Keymap source**, pick the keyboard and click **Connect**. For Bluetooth, pair
   "Eyelash Corne" in Windows' Bluetooth settings.
3. To update the keyboard's firmware, use **Update firmware…** on the Keyboard page, or see
   [Building and flashing](#building-and-flashing).

The [user guide](docs/user-guide.md) covers the layers, the screens, every page of the window,
lights and scenes, art, plugins, VIA keyboards, profiles and troubleshooting.
[docs/layers.md](docs/layers.md) has the keymap's layers.

## Repository layout

| Path | What |
|---|---|
| `config/` | the Corne's keymap and `west.yml`: ZMK v0.3.0, the board from Eyelash Peripherals, [zmk-raw-hid](https://github.com/BanditF/zmk-raw-hid) (`nonblocking`), [zmk-keypeek-layer-notifier](https://github.com/srwi/zmk-keypeek-layer-notifier) |
| `config-toucan2/`, `profiles/toucan2/` | the Toucan2 build (CI only, untested) |
| `boards/arm/eyelash_corne/` | board definition (from Eyelash Peripherals) |
| `boards/shields/nice_view_custom/` | the Corne's screens, host link, art relay, lights ([art credits](boards/shields/nice_view_custom/ART_CREDITS.md)) |
| `boards/shields/foxglove_link/` | the host link for boards without the nice!view shield |
| `app/` | the Foxglove app (own Cargo workspace, GPL-3.0); board profiles in `app/resources/boards/` |
| `crates/` | `foxglove-link` (the PC end of the link), `devices` (device layer, PC lighting), `scene`, `panel`, `via`, the plugin host, API, SDK and plugins, `foxglove-cli`, `foxglove-lab` (hardware test runner), `flash`, `foxglove-update`, and `hostpanel` (retired tray; a few CLI commands) |
| `integrations/signalrgb/` | the SignalRGB plugin that sends to Foxglove |
| `scripts/` | `check.ps1` (the gate), `flipper_art.py` (right-screen art), `flash/` (the flasher), `lab/` (hardware checks) |
| `tests/` | `hil/` (foxglove-lab suites), `relay-sim/` (the art relay on a fake split link), `zmk-sim/` (firmware cases in ZMK's native simulator) |
| `keymap-drawer/` | keymap diagram, redrawn by CI |
| `docs/` | user guide, design notes and release notes; [`docs/upstream/`](docs/upstream/README_EN.md) has the vendor's original README |

## Building and flashing

**Firmware.** Every push that touches the firmware runs **Build ZMK firmware** (GitHub Actions;
the matrix is in `build.yaml`). Changes only under `crates/`, `app/`, `docs/`, `keymap-drawer/`
or to Markdown files don't start a build. Flash the artifact with the flasher, which checks each
image's half and puts each half in its bootloader over the cable:

```powershell
python scripts/flash/foxglove_flash.py --firmware <folder> --half both
```

See [scripts/flash/README.md](scripts/flash/README.md). By hand: double-tap one half's reset
button and copy that half's UF2 onto the drive that appears (`eyelash_corne_studio_left` for the
left, `nice_view_custom-eyelash_corne_right` for the right), left half first. Copying the wrong
half's UF2 leaves that half with the other half's firmware. The nice!nano bootloader (0.6.0)
refuses images that end past `0xAD000`.

**App and crates.** `cargo build --release --manifest-path app/Cargo.toml` builds the app;
`cargo build --release` at the root builds the crates. Before pushing desktop changes, run the
gate: `powershell -ExecutionPolicy Bypass -File scripts/check.ps1`.

## Keymap

![Diagram of config/eyelash_corne.keymap](keymap-drawer/eyelash_corne.svg "generated by @caksoylar's Keymap Drawer")

## Names

Foxglove link was called Digitalis until #463. Digitalis ("of the finger", the Latin name
Leonhart Fuchs gave the foxglove in 1542) is kept for a future keyboard.

## License

The code is MIT, with exceptions; the app (`app/`) is GPL-3.0. See [LICENSING.md](LICENSING.md).
