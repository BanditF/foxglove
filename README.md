# hostpanel

Drives the Eyelash Corne's nice!view screens from the PC over Raw HID (usage page 0xFF60), so new
screen content is a PC-side change instead of a reflash. Needs the `host-panel` firmware
(BanditF/zmk-new_corne); the protocol is in `boards/shields/nice_view_custom/widgets/host_link.h`.

- **Left screen:** a 64x52 panel in place of the WPM chart showing the clock, volume, the media
  title (scrolling) and a per-game hint. The PC draws every pixel, so changing the layout
  (`src/render.rs`) never needs a reflash. If the app stops, the WPM chart comes back after 65 s.
- **Right screen:** push an art set without flashing. It goes PC -> left -> BLE split link ->
  right and is kept in RAM, so a power cycle brings back the built-in art.

```
hostpanel                      tray icon: Pause panel / Open config / Quit
hostpanel run --console        same, logging to the console
hostpanel status               panel size, right-half link + battery, art relay state
hostpanel push-art set.bin     art blob from scripts/flipper_art.py OUT.bin INPUT
hostpanel select -1            back to a built-in set (-1 = random, n = set n)
hostpanel preview out.pbm      draw the panel once, no keyboard needed
```

Config: `%APPDATA%\hostpanel\config.toml` (written on first run): product filter, frame rate and
`[games]` (exe name -> hint text). The right half's battery is logged to
`%APPDATA%\hostpanel\right-battery.csv` whenever it changes.

Runs alongside KeyPeek: both open the same interface and ignore each other's reports.

Build: `cargo build --release` (Rust, MSVC).
