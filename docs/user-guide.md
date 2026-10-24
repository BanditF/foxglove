# Foxglove user guide

Foxglove is the software for an Eyelash Corne split keyboard with two small nice!view screens.
It has two parts:

- **The firmware**, which runs on the keyboard itself. It types, holds the layers, and draws both
  screens. All of that works without a PC app.
- **The Foxglove app**, a Windows tray app. It shows which layer you're on, draws a clock and
  media panel on the left screen, picks the art on the right screen, sets the lights, and
  runs plugins.

You can quit the app at any time and the keyboard keeps typing.

## 1. Install the app

1. Download `foxglove-x64.msi` from the
   [Releases page](https://github.com/BanditF/foxglove/releases) (the newest `app-v…` release).
2. Run it. **Windows SmartScreen will warn you**: "Windows protected your PC". The installer
   isn't code-signed yet, so Windows doesn't know it. Click **More info**, then **Run anyway**.
3. Windows asks for administrator permission. Foxglove installs for everyone on the PC,
   into `C:\Program Files\Foxglove`.
4. In the setup screen, **Start with Windows** is off by default. Tick it if you want Foxglove
   running whenever you sign in.

Foxglove then appears in the system tray (the fox icon near the clock; it may be hidden under
the **^** arrow). Right-click it for the menu:

| tray item | what it does |
|---|---|
| status line | which keyboard is connected, if any |
| **Settings...** | opens the settings window |
| **Pause left panel** | freezes the PC panel on the left screen |
| **Right art** | picks the right screen's animation |
| **Profile** | switches between your saved setups (see [Profiles](#profiles)) |
| **Quit** | closes Foxglove; the keyboard keeps working |

Updates: Foxglove checks for a new version shortly after it starts, and offers to install it.
**Settings → Updates** has **Check now** and the option to stop checking.

To uninstall, use **Windows Settings → Apps**. Your settings stay in
`%APPDATA%\Foxglove\config` in case you reinstall; delete that folder to remove them too.

## 2. First connection

The **left half** is the one that talks to the PC. The right half talks only to the left.

1. **Plug the left half in with a USB cable** for the first connection. Foxglove reads the
   keymap over USB once and remembers it.
2. Open **Settings...** from the tray. Under **Connection**, pick the keyboard in **Device**
   and click **Connect**.
3. To use Bluetooth too, pair it like any Bluetooth keyboard: **Windows Settings → Bluetooth
   & devices → Add device → Bluetooth → Eyelash Corne**. After that, Foxglove works over USB or
   Bluetooth, and reconnects by itself.

Under **Keyboard** in Settings, the **Connection** line says whether you're on USB or Bluetooth,
which Bluetooth profile is active, and which profile belongs to this PC. If unplugging the cable
would leave the keyboard unable to type (it isn't on this PC's Bluetooth profile), it shows a
warning. Switch profile first ([section 6](#6-it-types-on-the-cable-but-not-on-bluetooth)).

## 3. The layers

The keyboard has seven layers. You reach most of them by holding a thumb key.

| layer | how to get there | what's on it |
|---|---|---|
| **QWERTY** | the base layer | letters |
| **NUMBER** | hold the middle left thumb | numbers |
| **SYMBOL** | hold the middle right thumb | symbols |
| **GAME** | tap Backspace + Esc together (again to leave) | a gaming layout |
| **GAME+** | hold the middle left thumb while in GAME | extra gaming keys |
| **FUNCTION** | hold **both** middle thumbs | F1-F12, mouse, media, Print Screen, Caps Lock (A), Insert (Z) |
| **SYSTEM** | hold both middle thumbs, then also hold **Tab** | Bluetooth, USB/Bluetooth output, reset and bootloader keys |

**SYSTEM** keys (keep both thumbs and Tab held while you tap):

| key | does |
|---|---|
| Q W E R T | Bluetooth profile 1-5 (Q is the first) |
| Y / U | send typing over USB / over Bluetooth |
| A | back to QWERTY, and clears any locked layers |
| N S G F | lock NUMBER / SYMBOL / GAME / FUNCTION on or off |
| Left Ctrl | forget the current Bluetooth profile's pairing |
| Esc | **forget all Bluetooth pairings: avoid this one** |
| M (hold 2 s) | turn both halves off; press their reset buttons to wake them |

The full map, including the reset and bootloader keys, is in [layers.md](layers.md).

## 4. The screens

### Left screen

![The left screen on the base layer](images/left-screen.png) ![The left screen with SYSTEM held](images/left-screen-system.png)

From top to bottom:
- **Battery and connection.** The battery level (with a bolt while charging), and **USB** or
  **BT1-BT5**. A line through the BT number means that profile isn't connected.
- **The layer name**, in the tab.
- **Modifiers.** CTL, SFT, ALT, WIN and CAPS light up while they're held or on.
- **Bluetooth profile boxes, 1-5.** The filled box is the active profile, and an outlined box
  is connected. The word below says what the active one is doing:
  - **USB**: typing goes over the cable.
  - **LINKED**: connected over Bluetooth.
  - **SEARCH**: paired, but the PC isn't in range or is off.
  - **PAIR**: nothing is paired on this profile yet.

  The whole block turns inverted (white) while SYSTEM is held, and for 3 seconds after a
  profile change, so you can see which profile you just picked. This is the quickest way to
  catch a wrong-profile mix-up.
- **The PC panel** at the bottom: the clock, what's playing and the volume. Foxglove draws it.
  If the app closes, the last picture stays.

### Right screen

![Bandit idle, one of the built-in animations](images/right-screen-bandit-idle.gif)

The right screen plays an animation. Several are built into the firmware, and Foxglove can send
others (see **Right screen art** below). The battery and connection strip sits above it.

## 5. Settings tour

Open it from the tray: **Settings...**. From the top:

- **Connection, Overlay Appearance, Theme.** The layer overlay: a picture of the keymap that
  pops up on screen when you change layer. You can pick its monitor, size, colours, which layers
  show it, and how long it stays.
- **Profile.** The setup you're editing (see [Profiles](#profiles) below).
- **Updates.** Version checks.
- **Keyboard.** The connection line, both halves' firmware versions (a warning appears if they
  differ), and:
  - **Lights.** Colour, effect, brightness and speed of the underglow on each half.
    **Follow the Windows accent colour** makes the lights take Windows' accent colour
    whenever you change it, keeping your brightness. These controls need an RGB key in the
    keymap.
  - **Actions.** Things the keyboard's own keys can do, as buttons. Risky ones (resets,
    bootloader) need a second click.
- **Screens.** The left screen's PC panel:
  - **Drive left screen** turns the panel on or off, and **Pause** freezes it.
  - **frames per second** sets how often it redraws.
  - **Show** picks the clock, media and hints, or the current Wallpaper Engine wallpaper's
    animation.
  - **Game hints** show a short note while a given program is in front.
  - The panel status shows the right half's link and battery.
- **Right screen art.** What plays on the right:
  - choose a built-in animation, or push one from a file;
  - **Saved on the keyboard** keeps a pushed animation on the keyboard itself;
  - **Hotlist** cycles through favourites every few minutes, **when the lights wake**, or to
    **match the wallpaper** (with Wallpaper Engine, the keyboard shows the art that goes with
    the wallpaper on screen).
- **Other keyboards (VIA).** Read-only support for VIA/QMK keyboards.
- **Plugins.** Optional add-ons, such as a focus timer or the bridge from OpenRGB lighting.
  Each runs as its own small program with only the permissions you approve. The tab lists
  what's installed (switch it on or off, or remove it) and a catalog. **Install from file…**
  shows exactly what a plugin may do before you agree. Only signed plugins run.

### Profiles

A profile is a saved variation of your settings, for example "Game" with a different screen
rate, lights and art. Changes to screens, lights and art go into the profile you're editing.
Anything a profile doesn't change comes from **Base settings**.

- Switch from the tray (**Profile**) or the **Profile** box at the top of Settings.
- **Switch profile by program** picks a profile automatically when a given program comes to
  the front.
- **Share profiles** saves them to a file, or loads them from one. The files never include this
  PC's keyboard connection.

Don't confuse these with the keyboard's Bluetooth profiles (the 1-5 boxes), which are about
which device the keyboard types to.

## 6. It types on the cable but not on Bluetooth

The keyboard is almost always on the wrong Bluetooth profile, the one paired with another
device. Nothing is broken.

1. Look at the profile boxes on the left screen: the filled box is the active profile.
2. Hold both middle thumbs, then hold **Tab** (SYSTEM). Tap the key for this PC's profile:
   **Q** for profile 1, **W** for 2, **E** for 3, **R** for 4, **T** for 5. The block flashes
   inverted to confirm.
3. If you're not sure which profile is this PC's, plug the cable in. **Settings → Keyboard**
   shows "this PC: profile N", numbered like the boxes.
4. Typing over the cable but want Bluetooth? SYSTEM + **U**. Back to the cable: SYSTEM + **Y**.

If the PC is missing from every profile, pick an empty one (its word says **PAIR**) and pair
again from Windows' Bluetooth settings. **SYSTEM + Left Ctrl** clears just the current profile
so it can pair fresh. Avoid **SYSTEM + Esc**, which clears every pairing.

If the halves stop talking to each other (the right screen freezes, right-hand keys do
nothing), switch both halves off and on again. If that doesn't help, flash both with the same
firmware ([section 7](#7-updating-the-keyboards-firmware)).

## 7. Updating the keyboard's firmware

Firmware updates come as two files, one per half: `eyelash_corne_studio_left.uf2` for the
left and `nice_view_custom-eyelash_corne_right-zmk.uf2` for the right. Putting the right half's
file on the left half (or the other way round) stops that half working until it's flashed
again, so use the flasher, which checks this for you:

```powershell
python scripts/flash/foxglove_flash.py --firmware C:\path\to\firmware --half both
```

It:
- checks both files before writing anything (the right half, the right size);
- puts each half into its bootloader over the cable, so no button presses are needed with
  recent firmware;
- does the left half first and checks that it types, then does the right;
- reads back the version each half is running.

Every write is logged in `%LOCALAPPDATA%\Foxglove\flash-history.log`. `--dry-run` checks the
files without touching the keyboard. Details and options are in
[scripts/flash/README.md](../scripts/flash/README.md). In this project's agent setup, the
**On-call** session runs the flasher for you once you say go.

By hand, the old way: double-tap a half's reset button and a USB drive appears. Copy that
half's `.uf2` onto it, left half first. The bootloader checks nothing, so be sure which half
you're holding.

## Where things are kept

| what | where |
|---|---|
| settings | `%APPDATA%\Foxglove\config\` |
| art library (pushed animations, wallpaper art) | `%APPDATA%\Foxglove\config\art\` |
| updates downloaded | `%LOCALAPPDATA%\Foxglove\updates\` |
| firmware flash history | `%LOCALAPPDATA%\Foxglove\flash-history.log` |
