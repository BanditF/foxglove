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
- **A wrong or old token:** a browser can't see the HTTP status of a refused upgrade (its
  `onclose` only says 1006), so for a local client (Origin `null`, `file://` or none) with a
  wrong or missing token the upgrade completes, Foxglove sends `{"t":"refused","why":"token"}`
  and closes with code `4001` (reason `token`). That is all the client learns: no `hello`,
  nothing about the right token. The wallpaper should then stop its fast retry and ask for the
  new token (the Bandit wallpapers retry at most once a minute, or at once when the token
  property changes). A web origin still gets a plain HTTP 403 and no WebSocket.
- **In the log and health view:** the first refusal is logged in full (`live feed refused a
  connection: wrong or missing token`), then at most one line every 10 minutes, written with
  the next refusal: `live feed refused N connections: wrong token` (N since the last line).
  While they keep coming, Keyboard health shows a WARN, "Live wallpaper pairing: a wallpaper is
  using an old pairing token". It clears when a wallpaper pairs, or 10 minutes after the last
  refusal. Nothing runs on a timer for either.

## Messages

JSON text frames on `ws://127.0.0.1:<port>/live?token=<token>`. The wallpaper can instead
offer the sub-protocols `foxglove-live.v1, <token>`.

| `t` | Direction | Body |
|---|---|---|
| `hello` | → wallpaper | `v`, `topics`, `board` (`name`, `keys`: `[x, y, w, h, r]` in key units, rotation about the key's centre), `state`, `quiet` |
| `state` | → wallpaper | `layer` (`index`, `name`), `art`, `battery` (`left`, `right`), `link` (`connected`, `output`, `profile`, `profile_connected`), `lights` (below), `keyboard` (`name`, `source`, `connected`; below) |
| `art` | → wallpaper | `art`: `name` (as `state.art`), `nva` (the piece's frames, below, or `null`); or `art: null` |
| `panel` | → wallpaper | `panel`: `w`, `h`, `bits` (the left screen's PC panel, below), or `null` |
| `keys` | → wallpaper | `e`: `[index, 1 down / 0 up, ms since the batch's first]` |
| `board` | → wallpaper | the geometry changed |
| `quiet` | → wallpaper | `why`: `paused`, `away` or `fullscreen`; nothing follows until `live` |
| `live` | → wallpaper | sending again (a `state` follows) |
| `notify` | → wallpaper | `id`, `title`, `body`, `level` (`info` or `warn`): one of Foxglove's notifications; only to a client whose `hi` had `caps: ["notify"]` |
| `refused` | → wallpaper | `why`: `token`; the only message to a client with a wrong or missing token, then a close `4001` |
| `hi` | → Foxglove | `wallpaper`: its name; `caps`: what it can do beyond the feed (`notify`) |
| `paused` | → Foxglove | `paused`: Wallpaper Engine paused the wallpaper (`setPaused`) |
| `notified` | → Foxglove | `id`: it drew that notification |

`lights` (#40) is the keyboard's lights as it reports them, left out until it has: `on`,
`color` (`#rrggbb`: the chosen colour at full brightness, so a dim keyboard still gives a
usable accent), `brightness` (0-100), `effect` (its name, such as `Solid` or `Breathe`) and
`rainbow` (true for an effect that makes its own colours, such as Spectrum, so `color` isn't
what the keyboard shows). A wallpaper can take `color` as its accent while `on` and not
`rainbow`, so the wallpaper matches the keyboard. It comes from the `keyboard.lighting.*`
facts (docs/local-api.md) and changes only when the keyboard reports a change.

`keyboard` (#801) names the keyboard the feed is about: `name` (as Foxglove shows it, such as
`Eyelash Corne`), `source` (`host_link`, `via` or `lamp_array`) and `connected`. It is the
keyboard picked at the start of Foxglove's status strip, or the one picked in Settings → Live
wallpaper → Keyboard, which keeps the wallpaper on that keyboard whatever the strip shows.
It is left out before Foxglove found any keyboard. For a `host_link` keyboard (Foxglove's own
firmware) everything above and below follows it: the layer, battery, link, lights, art, the
board and the keys. For any other keyboard Foxglove can't read those, so `state` carries only
`keyboard` and `link.connected`, `board` is the keyboard's own shape (from its layout, or a
guess from its key count), and no `keys` are sent. A change of keyboard sends a new `board` and
`state`. The keyboard's name is all that identifies it: never a serial number or a device path.

`art` (code: `live_art.rs`) carries the right screen's piece, so a wallpaper can draw the literal art:
- **Format:** `nva` is an NVA1 blob in base64, re-encoded **upright** (as the art is drawn, 68x140 on
  the Corne), with a set bit for ink whatever the source's polarity. The header is `NVA1`, width,
  height, frames a second, flags (1 = ink bits), unique frame count (u16 LE), play-order length (u16
  LE), the play order, then a u16 LE offset per unique frame. Each unique frame is an XOR delta on
  the one before (the first on blank), packed as byte RLE: a control byte `op << 6 | (n - 1)`, where
  op 0 is `n` literal bytes, op 1 is `n` zero bytes, op 2 is `n` 0xff bytes and op 3 is the next
  byte `n` times. Rows are MSB first, `ceil(width / 8)` bytes a row. The play order runs on a loop
  at the given rate, as the keyboard plays it.
- **When:** after the `hello` (on joining, and on coming back from a pause), and whenever the
  piece changes. Never per frame, and not with each `state`. A change while the feed is quiet
  goes out after `live`.
- **Size:** at most 32 KiB of blob (twice the right half's 16 KiB art slot), so at most about
  43 KiB of text. A piece past that goes with `nva: null`. The art library's Bandit_radar (12 frames) is 2.9 KiB.
- **What has frames:** a library file the right half plays, a built-in (from the firmware's own
  `custom_art.c`, when its manifest matches the keyboard), and a kept place when the library has
  a file of the same name and size. Otherwise `nva` is `null`, and the wallpaper draws a stand-in.
- **The left screen:** it shows the firmware's own status panel, not art. A wallpaper draws it
  from `state` (layer, `battery.left`, `link`) and `panel` below.

`link.profile_connected` says whether the active Bluetooth profile's host is connected (the left
screen's LINKED or SEARCH, and its BT label struck through when not); it is left out until the
keyboard said. `link.connected` is Foxglove's own link to the keyboard.

`panel` is the part of the left screen Foxglove draws (`screens.rs`; on the Corne 64x52 at x 2,
y 106 of the 68x160 screen): the clock, volume and media, as Foxglove last sent it, rows MSB
first in whole bytes, a set bit for ink, base64. `null` while the firmware draws its own (its WPM
chart: Foxglove paused, away, the panel off or the keyboard gone). It goes out after the hello,
when it changes, at most one frame every 250 ms (an animated panel: the latest frame at the end
of each window), and at most 1024 bytes of bits. It shows what the keyboard shows, so a
notification Foxglove puts on the left screen is in it too; like everything else here it reaches
only a wallpaper with the pairing token.

A client ignores any `t` it doesn't know, and any field it doesn't know. The reserved topic `stats` (PC stats) comes later
without a new version.

## Notifications

Foxglove's own notifications (a low battery on a half or on another device, a firmware update done or failed, a new health warning or failure; `app/src/foxglove/notice.rs`)
go to the wallpaper first when Settings → Live wallpaper → "Show notifications on the wallpaper
when it's on screen" is on (the default) and at least one connected, unpaused client said
`caps: ["notify"]` in its `hi`. Any wallpaper or program may declare it.

- The client draws `{"t":"notify","id":7,"title":"…","body":"…","level":"info"}` and answers
  `{"t":"notified","id":7}` once it's on screen. It may queue several and show them one by one,
  but it acknowledges each when drawn, not when it was queued.
- **No silent loss:** without a `notified` within 2 seconds Foxglove shows the Windows
  notification instead. While the feed is quiet (Foxglove paused, the PC locked or the display
  off, a full-screen program), or with no capable client, the setting off or the feed off, the
  Windows notification shows at once. Windows' own focus or game mode may then hold it back,
  as for any app.
- The left screen's notifications (other apps' Windows notifications on the nice!view) are
  apart and unchanged.

## Privacy and gates

- **What a key event is:** its index into `board.keys` (the overlay's layout order, ZMK's key
  position), up or down, and a few milliseconds of timing. Never a keycode, binding, label or
  character (`live_wire`'s privacy test).
- **What an index can still reveal:** someone who knows the keymap can tell keys apart from
  their positions, and the setting's text says so.
- **Notification text** goes only to a token-paired client that declared `caps: ["notify"]`;
  the others never see it. Its keys are in `live_wire`'s privacy allowlist (`id`, `title`,
  `body`, `level`).
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
