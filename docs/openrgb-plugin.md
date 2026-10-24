# OpenRGB Plugin (E1.31)

`crates/plugin-openrgb-e131` (MIT) lets OpenRGB, or any sACN/E1.31 sender, colour your keyboards' lights. It is also the first plugin that gets network data through Foxglove instead of a socket of its own (#71, #214).

## Using it with OpenRGB

1. Install the plugin from plugins. It asks for `devices.read`, `lighting.write` and `net.listen:udp:127.0.0.1:5568`.
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
- **While a stream runs,** the plugin holds each keyboard's lights with a **low-priority lease** (#73), whose colour changes in place (`lighting.update`). A notification, such as mic muted or a focus timer, outranks it.
- **When the sender stops,** the lease ends, and Foxglove puts each keyboard's lights back as they were before the first frame (on/off, colour, brightness, effect, speed, confirmed by readback, #293). A sender stops when it ends its stream (the E1.31 terminated bit) or after 2.5 s without data (E1.31's loss timeout). The plugin stopping or crashing ends the lease too.
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

The plugin runs in the AppContainer, with no network of its own. Foxglove listens on 127.0.0.1:5568 for it and passes the datagrams on (see local-api.md, "Network, scoped"). The end-to-end test runs it sandboxed.

## Tests

`cargo test -p plugin-openrgb-e131`:
- **Unit tests:** the packet parser, sequence handling, colour averaging and conversion.
- **End-to-end test:** Plugins's runner starts the plugin as Foxglove does. A frame sent with `--send-test` takes one low-priority lease on a mock keyboard. A new colour updates it in place, and a repeat of the same colour sends nothing. The stream ending, or going silent past the loss timeout, releases the lease.
