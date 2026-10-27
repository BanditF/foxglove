# Live wallpaper feed (`foxglove-live` v1)

Foxglove's side of the live keyboard wallpaper (#498, #499). Off by default: Settings → Live
wallpaper. This page is also the public spec: any wallpaper, or another host (#501), can be
a client. A Wallpaper Engine wallpaper opts in to the port push with its `foxglove.json`
(docs/wallpaper-manifest.md, `supports: ["foxglove_live"]`). The code is `app/src/foxglove/live_feed.rs` (runtime) and `live_wire.rs` (format,
checks, batching).

## Can a Wallpaper Engine wallpaper open `ws://127.0.0.1`? (#320)

Yes.
- **Where wallpapers load from:** Wallpaper Engine 2.x renders web wallpapers in CEF 146
  (`bin\libcef.dll`, Chromium 146.0.7680), from `file:///…/index.html`. The wallpaper
  renderer's own history (`ui\wpcache\monitor0\…\History`) lists only `file:///` URLs.
- **The test:** Chromium 156 (headless Edge) on this PC loaded a `file://` page that opened
  `ws://127.0.0.1:<port>`. The page sent a message after `open` both times, there was no
  prompt, and the listener saw `Origin: null`. The token worked as `?token=` and as a second
  sub-protocol.
- **Local Network Access:** Chrome 147 gated WebSockets from public pages to loopback, and
  Chromium 156 still let a `file://` page through. Wallpaper Engine's CEF 146 is older and
  laxer.
- **Still to see:** one run in Wallpaper Engine itself, at a live sitting.

So the feed is a WebSocket on `127.0.0.1` only. Origin `null`, `file://`, or none (a local
program) is admitted; any web origin is refused. On top of that, the wallpaper must present the
install's pairing token. A sandboxed iframe on any website also sends `null`, so the token is
the real check.

## Pairing and discovery

- **The token:** Foxglove makes a 24-character token from the OS's random source when the feed
  is first turned on. The user pastes it into the wallpaper's "Foxglove pairing token"
  property. It never goes on a command line.
- **The port:** the feed listens on `8768` by default (`[live_wallpaper] port`). Foxglove tells
  each wallpaper whose manifest supports `foxglove_live` the port as the `foxglove_live`
  property through Wallpaper Engine's command line (`wallpaper_sync`), and sends `0` when it
  stops. The wallpaper connects when that property changes, so nothing retries on a timer. A
  client outside Wallpaper Engine uses the configured port.

## Messages

JSON text frames on `ws://127.0.0.1:<port>/live?token=<token>`. The wallpaper can instead
offer the sub-protocols `foxglove-live.v1, <token>`.

| `t` | Direction | Body |
|---|---|---|
| `hello` | → wallpaper | `v`, `topics`, `board` (`name`, `keys`: `[x, y, w, h, r]` in key units, rotation about the key's centre), `state`, `quiet` |
| `state` | → wallpaper | `layer` (`index`, `name`), `art`, `battery` (`left`, `right`), `link` (`connected`, `output`, `profile`) |
| `keys` | → wallpaper | `e`: `[index, 1 down / 0 up, ms since the batch's first]` |
| `board` | → wallpaper | the geometry changed |
| `quiet` | → wallpaper | `why`: `paused`, `away` or `fullscreen`; nothing follows until `live` |
| `live` | → wallpaper | sending again (a `state` follows) |
| `hi` | → Foxglove | `wallpaper`: its name |
| `paused` | → Foxglove | `paused`: Wallpaper Engine paused the wallpaper (`setPaused`) |

A client ignores any `t` it doesn't know. The reserved topics `stats` (PC stats) and `notes`
(notifications) come later without a new version.

## Privacy and gates

- **What a key event is:** its index into `board.keys` (the overlay's layout order, ZMK's key
  position), up or down, and a few milliseconds of timing. Never a keycode, binding, label or
  character (`live_wire`'s privacy test).
- **What an index can still reveal:** someone who knows the keymap can tell keys apart from
  their positions, and the setting's text says so.
- **When nothing is sent:**
  - Foxglove is paused;
  - the session is locked or the display is off;
  - `SHQueryUserNotificationState` reports a full-screen program (busy, D3D full screen,
    presentation mode);
  - the wallpaper itself is paused.

  The full-screen check runs when the program in front changes and at the start of a burst
  of typing. Key events are subscribed only while an unpaused wallpaper is connected.
- **Limits:** a burst is held at most 16 ms (at most 60 frames a second, 64 events a frame).
  Four wallpapers at once, eight handshakes in flight.
