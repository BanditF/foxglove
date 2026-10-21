# Keyboard first: test pass

The rule (tracker #23, #24, #25): if the PC app is gone, crashed or unplugged, the keyboard keeps
typing and both screens keep doing what they were doing. The left keeps its last panel frame, and
the right keeps its art.

Fill in the **Result** column on hardware. Type throughout every case (a few words per step is
enough), and watch both screens.

## What the firmware does now

| Part | Behaviour |
|---|---|
| Left panel (`host_link.c`) | Takes the WPM area at the first PRESENT and keeps the last presented frame until RELEASE or a reboot. `CONFIG_NICE_VIEW_HOST_PANEL_TIMEOUT_S` (default 0 = never) brings the WPM chart back after N s of silence. The back buffer is never shown: ROWS/FILL only write it, PRESENT copies it to the front. |
| Left relay (`art_relay_central.c`) | A receive from the PC with no BEGIN or DATA for 10 s is dropped: state goes back to IDLE with error `ART_ERR_TIMEOUT` (8). A new BEGIN replaces a stuck receive at any time. Pushes to the right hold their own connection reference, so a split link that drops mid-push fails the push (`ART_ERR_BLE`) instead of touching a freed connection. Every request has a 3 s timeout. |
| Right relay (`art_relay_peripheral.c`) | Receives into a spare RAM slot while the current set keeps playing. The screen switches only after the blob passes CRC and header checks. A failed, partial or never-committed push leaves the screen as it was. |

Before this change, the right froze its animation at BEGIN while a pushed set was playing and
stayed frozen if the push never committed or failed its CRC; the left stayed RECEIVING forever;
the left panel went back to WPM 65 s after the last report.

Pushed art and the panel live in RAM only: after a power cycle the right shows a built-in set and
the left shows the WPM chart. That is expected, not a failure.

## Before you start

1. Flash only when you know which half is in the bootloader. The drive name is the same for both,
   so check which half you double-tapped. Left = `eyelash_corne_studio_left`, right =
   `nice_view_custom-eyelash_corne_right`.
2. Pair and type on both halves before you start the app.
3. Have the app's status visible: the HELLO reply (left panel, right link, battery, discovery)
   and the art STATUS reply (left state/error, right state/set). In the reports below, left
   states are 0 idle, 1 receiving, 2 pushing, 3 done, 4 failed; right states are 0 built-in,
   1 receiving, 2 playing pushed, 3 bad CRC, 4 bad blob, 5 too big.

## Matrix

| # | Case | How | Expect | Result |
|---|---|---|---|---|
| 1 | App never started | Power-cycle both halves, don't start the app, type for a minute | Types normally on both halves. Left shows the WPM chart, right a built-in set (animated while typing) | |
| 2a | App killed mid-frame | App running with the panel. Kill it from Task Manager (End task) | Left keeps the last frame (its clock freezes) for at least 5 minutes, and doesn't go back to WPM. Typing OK | |
| 2b | App killed mid-push, PC side | Start a push of a large set (close to 16 KiB, so the USB part takes a few seconds), kill the app while it's sending | Right keeps its current set and keeps animating. After about 10 s an art STATUS shows left state 0, error 8 (timeout). A new push afterwards works without a reboot | |
| 2c | App killed mid-push, BLE side | Kill the app right after COMMIT, while the left pushes to the right | The left finishes the push on its own (it already has the whole blob), so the right switches to the new set. Typing OK | |
| 2d | Stuck receive, then a new push | Send BEGIN and a few DATA, then stop (debug tool or kill the app), and within 10 s start a full push | The new BEGIN replaces the stuck receive. The push succeeds and the right shows the new set | |
| 3 | USB unplugged and replugged | App running. Unplug the left's USB (it's on BLE to the PC now), wait 30 s, plug it back | Left keeps its last frame and the right its art. Typing over BLE OK. The app reconnects by itself after the replug and the panel updates again | |
| 4 | PC sleep / resume | App running. Sleep the PC, wait 1 minute, wake it | Same as 3. After resume the app reconnects and the panel updates again | |
| 5a | Split link drops during a relay: right powered off | Start a push, switch the right half off mid-relay, then back on | The push fails cleanly: left state 4, error 6 (BLE) or 1 (no link). The left keeps typing. The right boots to a built-in set (RAM only, expected) and types once reconnected. A push after reconnect (discovery takes ~15 s after security) works | |
| 5b | Split link drops during a relay: out of range | Start a push, then carry the right half out of range or put it in a metal tin until the link drops, then bring it back | Push fails as in 5a. The right keeps the set it was showing (it never switches to a half-received one). The right types after reconnecting | |
| 5c | Bad blob | Push a blob with a wrong CRC, or a valid CRC but a broken header (e.g. wrong width byte) | Left reports error 5 (bad CRC, caught on the left) or 7 (right rejected it, right state 3/4). The right keeps its current set and keeps animating | |
| 6 | Flood | Run the flood script below for 60 s while typing quickly on both halves, including held keys and layer switches | No dropped, doubled or stuck keys. The panel updates. After stopping, the left keeps the last frame | |
| 7 | RELEASE still works | Send PANEL RELEASE (or quit the app cleanly, if it sends one) | The WPM chart comes back right away | |

Typing check for every case: type a short sentence on each half, hold a key for 2 s (one
character + repeat, no stuck key after release), switch a layer.

## Flood script

Python with `hidapi` (`pip install hidapi`). It finds the Raw HID interface by usage page and
writes panel reports as fast as it can. Stop the app first so it doesn't fight the script.

```python
import hid, time

dev = next(d for d in hid.enumerate() if d["usage_page"] == 0xFF60 and d["usage"] == 0x61)
h = hid.device()
h.open_path(dev["path"])

def send(*data):
    h.write(bytes([0, *data]).ljust(33, b"\0"))  # report ID 0 + one 32-byte report

end = time.time() + 60
n = 0
while time.time() < end:
    for y in range(0, 52, 3):
        rows = min(3, 52 - y)
        send(0xD5, 0x03, y, rows, *([0xAA if (y + n) % 2 else 0x55] * 8 * rows))  # ROWS
        n += 1
    send(0xD5, 0x04)  # PRESENT
    n += 1
print(n, "reports")
```

The leading 0 is the report ID that hidapi expects; Windows wants the full report length. Note the report count; on a stalled USB
pipe `write` blocks and the count stays low, which is fine as long as typing is unaffected.

## Firmware for this pass

PR: see the PR description for the CI run, the UF2 artifacts and their end addresses (the right
half must stay under 0xAD000).
