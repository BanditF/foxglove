# Keyboard self-report (D7 13 HEALTH)

Each half of the keyboard keeps small counters and states per subsystem in RAM, and the PC reads
them as one versioned block per half (#705). The point: after a batch flash, one read says what
works and what doesn't, so a change doesn't need a flash of its own to be checked.

- Firmware: `boards/shields/nice_view_custom/widgets/health.h` and `health.c`; the command is in
  `host_link.h` (D7 13), the right's copy travels over the art relay (`art_relay.h`,
  `ART_RELAY_HEALTH_AT`). Kconfig `FOXGLOVE_HEALTH` (on by default, dev and GA).
- PC: `crates/foxglove-link/src/health.rs` reads and parses it and turns it into rows (OK, warning,
  failed, with a reason in plain words); the thresholds are data
  (`crates/foxglove-link/data/health.toml`).
- Announced by HELLO capability bit 28 (`HOST_LINK_CAP_HEALTH`).
- Notifications (#814, `app/src/foxglove/health_notice.rs`): a WARN or FAIL row (the keyboard's or
  a feature's) that wasn't there at the previous read is a notification ("<item>: warning" or
  "<item> failed", the reason, "See Keyboard health on the App page."), through `notice::show`, so
  a live wallpaper may show it. At most once a day per subsystem and item; the last day is kept in
  the store's `[health_notices]` section. Raw HID refusals and "not tested" rows stay on the card.

Nothing runs on its own: counters move when their subsystem's event happens, and the boot
self-test and the live values are worked out when the block is read. No timer, no boot work.
Counters count since that half's boot and saturate (u16 at 65535, u32 at 2^31 - 1).

## The command

Request: `D7 13 half chunk` (half 0 left, 1 right; chunk 0-4). A report cut short reads half 0,
chunk 0.

Reply `D7 93`:

| Byte | Field |
|---|---|
| 2 | status: 0 ok, 1 busy (a read of the right runs; its reply comes), 2 no relay link, 3 the right didn't answer, 4 not built in, or a right without the block, 5 bad half or chunk, 6 no copy yet (ask chunk 0 first) |
| 3 | half |
| 4 | copy number (one up with each chunk 0) |
| 5 | chunk |
| 6 | chunks (5; 4 before version 2) |
| 7 | block length (116; 112 in version 3, 104 in version 2, 96 before) |
| 8-31 | the block's bytes `chunk * 24` to `chunk * 24 + 23` |

Chunk 0 takes a new copy of that half's block; chunks 1-4 come from that copy, so the five
replies are one moment (check that their copy numbers match). For the right, chunk 0 makes the
left read the right's block over the art relay, one response's worth per request (six at MTU
23), each spaced `REQ_GAP_MS` after the right's last answer as every relay request is (#662);
under the link's peripheral latency that is about a second. Its reply comes when the read is
done (or failed: 2, 3 or 4). Wait up to 8 s for it.

## The block (version 4, 116 bytes, little endian)

Version 1 was bytes 0-95; version 2 (#719) appends the knob, bytes 96-103; version 3 appends the
Raw HID counts per transport, bytes 104-111; version 4 each transport's last refusal's error
code, bytes 112-115.

Fields a half doesn't have are 0, or 0xff where 0 means something; byte 3's flags say which parts
the half has built in.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | version (4; 3 before the last error codes, 2 before the per-transport counts, 1 before #719) |
| 1 | 1 | half (0 left, 1 right) |
| 2 | 1 | length (116; 112 in version 3, 104 in version 2, 96 in version 1) |
| 3 | 1 | flags: bit 0 relay (the left's side), 1 Raw HID counters (the module's `raw_hid/stats.h`), 2 host link, 3 relay (the right's side), 4 lighting, 5 PC state, 6 polarity, 7 display |
| 4 | 4 | firmware major, minor, patch, profile (0 dev, 1 GA) |
| 8 | 8 | short git SHA, ASCII, zero-padded |
| 16 | 4 | uptime, seconds |
| 20 | 4 | reset reason: Zephyr's `RESET_*` bits from hwinfo (0 unknown) |
| 24 | 4 | boot self-test, one byte per check: display, settings, relay, LEDs (below) |
| 28 | 10 | the left: HELLO's relay debug bytes for this link (chrcs, ends, last ret, ATT error, timeouts, answered, in flight, security, MTU u16), folded in here rather than kept twice |
| 38 | 2 | relay requests let go without an answer, since boot |
| 40 | 2 | split links dropped to end a stuck request (the unstick) or a stuck relay discovery (#873) |
| 42 | 2 | the left: times the relay was found; the right: times the left subscribed |
| 44 | 1 | relay requests in flight now (the left) |
| 45 | 1 | relay discovery result (int8: 0 idle, 1 running, 2 no service, 3 no characteristics, 4 found; negative a `bt_gatt_discover` error) |
| 46 | 2 | the left: relay requests that failed; the right: ops it refused |
| 48 | 4 | Raw HID reports sent (the module's count; a report on USB and BLE counts twice; USB's plus Bluetooth's) |
| 52 | 2 | Raw HID reports that waited for a USB transfer in flight |
| 54 | 2 | Raw HID reports that waited and were dropped (ring full, or a USB reset) |
| 56 | 2 | Raw HID reports a transport refused (USB's plus Bluetooth's, 104 and 108). Since the module's `raw_hid/stats.h` version 2 a Bluetooth report nobody listens for isn't counted here but at 110 |
| 58 | 2 | host link replies dropped: its reply queue (4) was full |
| 60 | 4 | screen redraws: the left's host panel; the right's whole-screen rewrites (#618) |
| 64 | 2 | art jobs done: the left's pushes, SELECTs and slot ops; blobs the right took |
| 66 | 2 | art jobs failed: the left's; blobs the right refused |
| 68 | 1 | the left: `enum art_relay_error` of the last failure; the right: its state then (3 bad CRC, 4 bad blob, 5 too big) |
| 69 | 1 | art set shown (int8; -1 a pushed set; -128 not known yet) |
| 70 | 2 | screen snapshots that failed (a capture, or the right's transfer timing out) |
| 72 | 2 | the PC's colour, frames or effect applied on this half (on the left also a HID LampArray frame, #88; its own counters are D7 90's bytes 12-23) |
| 74 | 2 | the left: underglow ops the right acknowledged; the right: ops it took |
| 76 | 2 | the left: underglow ops to the right that failed (each try) |
| 78 | 2 | the left: status reads that showed the right with another look than it was sent (#624) |
| 80 | 1 | the PC effect kind running (0 none, 1 scanline, 2 sweep, 3 ripple, 4 breathe) |
| 81 | 1 | ZMK's underglow effect (0 solid, 1 breathe, 2 spectrum, 3 swirl; 0xff unknown) |
| 82 | 2 | split links that went down |
| 84 | 4 | uptime of the last split link-up, seconds (0 never) |
| 88 | 1 | split link up now (0/1) |
| 89 | 1 | links the #122 guard dropped (the left) |
| 90 | 1 | PC state (`pc_state.h`: 0 none, 1 awake, 2 away; 0xff not built) |
| 91 | 1 | why the hold is on or off (D7 91 byte 8's reason; 0xff not built) |
| 92 | 1 | panel polarity (0 dark, 1 light; 0xff not built) |
| 93 | 1 | the left: the polarity the right acknowledged on this link (0xff none) |
| 94 | 1 | this half's LEDs (its chain) |
| 95 | 1 | the left: the LEDs it frames for the right (`LAMP_PARTS_RIGHT`); compare with the right's byte 94 |
| 96 | 1 | the knob (the left, #719; `knob.h`): bit 0 the keymap has `&knob`, 1 `&knob_press`, 2 the PC holds it (D7 14), 3 its target is an app (0: master), 4 the PC holds master volume (D7 16, #855: master's turns go to the PC) |
| 97 | 1 | the last knob event sent to the PC (0 none, 1 turn, 2 click, 3 double click, 4 hold, 5 handed back) |
| 98 | 2 | knob events sent to the PC (D7 94 01) |
| 100 | 2 | fallback keys the knob tapped (a detent, a press, a double click's mute on master) |
| 102 | 2 | holds the board dropped (a USB disconnect, an endpoint change or a lapsed hold lease, #847) |
| 104 | 2 | Raw HID reports the USB driver refused (version 3, the left; 0 with the module's `raw_hid/stats.h` before version 2) |
| 106 | 2 | Raw HID Bluetooth notifications queued |
| 108 | 2 | Raw HID Bluetooth notifies refused on a link where the PC listens |
| 110 | 2 | Raw HID Bluetooth reports skipped: nobody listens (no link on the active profile, the PC hasn't turned the report's notifications on, or the link isn't encrypted yet). Normal while the PC uses USB: the health row says it as information (OK), never a warning; byte 56's refusals stay a warning (`hid_failed` in `data/health.toml`) |
| 112 | 2 | int16: the error code of the last report the USB driver refused (version 4; the module's `raw_hid/stats.h` version 3, else 0): a negative errno, 0 while it refused none. When it's -11 (EAGAIN: the PC hadn't configured the keyboard, as during a PC restart, sleep or replug) the USB refusals are said as information (OK), not counted towards `hid_failed`'s warning (#741) |
| 114 | 2 | int16: the same for Bluetooth: the last notify refused on a link where the PC listens (-12 ENOMEM: no buffer free) |

### Boot self-test (bytes 24-27)

Each byte is a state in bits 0-1 (0 pending, 1 pass, 2 fail, 3 not applicable) and a code in bits
2-7. Worked out when read, from what happened since boot:

| Check | Pass | Fail codes |
|---|---|---|
| display | the display device is ready and ZMK's display is up | 1 device not ready, 2 ZMK's display not up 10 s after boot |
| settings | the settings were loaded (their commit ran) | 1 not loaded 30 s after boot |
| relay | the relay was found within `FOXGLOVE_HEALTH_RELAY_S` (60) of boot; on the right, the left subscribed | 1 found, but later, 2 no split link by then, 3 a link, but no relay |
| LEDs | the underglow strip is ready and has LEDs | 1 strip not ready, 2 no LEDs in the chain |

## Compatibility

A newer field goes after the last one with a new version (version 2 added bytes 96-103, version 3
bytes 104-111, version 4 bytes 112-115); a reader takes the fields it knows and the length the block says. A left reads an
older right's 96-, 104- or 112-byte block to its own length and leaves the rest 0, so its D7 93
says 116 bytes while the block's byte 2 says 96, 104 or 112: read byte 2. The block stays within
five chunks (120 bytes) up to version 4. Firmware without bit 28 answers D7 13 with nothing (no reply): ask only
with the bit. A right before #705 ends its relay status value at `ART_RELAY_HEALTH_AT`, so a left
with the block answers 4 for it.

## Tests

- zmk-sim `health` (`tests/zmk-sim/health`): HELLO bit 28, a block read in five chunks (version 4,
  with the Raw HID counts per transport and last error codes from the module's faked `raw_hid/stats.h`), a burst
  that drops two replies and the next block counting them, the settings check, and every error
  status the left gives without a relay.
- relay-sim `health` (`tests/relay-sim`): the boot self-test's relay check pending, failed with no
  link, then failed late; the right's block read under ZMK's latency and the right's
  response-buffer rule with nothing dropped; a forced relay timeout; a push the right refuses and
  one it takes, on both halves' counters; a right without the block (status 4, nothing counted as
  a refusal); a re-link. `stall` checks the unstick, drop and re-found counts.
