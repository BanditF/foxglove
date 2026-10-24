# OpenRGB Plugin (E1.31)

`crates/plugin-openrgb-e131` (MIT) lets OpenRGB, or any sACN/E1.31 sender, colour your keyboards' lights. It is also the first plugin with the `net` permission (#71).

## Using it with OpenRGB

1. Install the plugin from plugins. It asks for `devices.read`, `lighting.write` and `net`.
2. In OpenRGB, open **Settings → E1.31 Devices**, then **Add**:
   - **IP:** `127.0.0.1` (unicast; the plugin doesn't join multicast)
   - **Universe:** 1, **Start channel:** 1
   - **Number of LEDs:** anything from 1 up; the plugin averages them
   - **Type:** Single, **RGB order:** RGB
3. Restart OpenRGB's devices (or OpenRGB itself). The new device's colour or effect becomes your keyboards' underglow colour.

The plugin doesn't load, link or copy OpenRGB code. E1.31 is a small open protocol, so the plugin parses it itself.

## What it does

- It listens on **127.0.0.1:5568 (UDP) only**. Other machines can't reach it, and it sends nothing out.
- It averages the RGB LEDs it receives into one colour and sets each connected keyboard with lights to that static colour.
- It sends only when the colour changes, at most `--max-hz` times a second (10 by default). Plugins's limit per connection (50 calls a second) is far above that.
- **When the sender stops,** the plugin puts each keyboard's lights back as they were before the first frame (on/off, colour, effect). A sender stops when it ends its stream (the E1.31 terminated bit) or after 2.5 s without data (E1.31's loss timeout).
- It ignores preview packets, other universes, non-level start codes and stale sequence numbers.

## Options

These go in `plugin.toml`'s `run`:

| Option | Default | Meaning |
|---|---|---|
| `--universe N` | 1 | The E1.31 universe to listen to |
| `--start-channel N` | 1 | The first channel of the first LED |
| `--leds N` | every LED sent | How many LEDs to average |
| `--max-hz N` | 10 (1–30) | Most colour changes sent per second |
| `--send-test R G B` | | Send one frame to a running plugin and exit (testing without OpenRGB) |

## Sandbox

`net` plugins run outside the AppContainer but still in their Job Object; see local-api.md, "Plugins". The plugin binds only the loopback address.

## Tests

`cargo test -p plugin-openrgb-e131`:
- **Unit tests:** the packet parser, sequence handling, colour averaging and conversion.
- **End-to-end test:** Plugins's runner starts the plugin as Foxglove does. A frame sent with `--send-test` colours a mock keyboard, a repeat of the same colour sends nothing, and the lights are restored after the loss timeout.
