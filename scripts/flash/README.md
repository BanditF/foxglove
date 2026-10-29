# Flash a Corne safely

Python 3.12, standard library only. From the repository root:

```powershell
python scripts/flash/foxglove_flash.py --firmware C:\Users\fuchs\splitkeeb\firmware-358c3d0\firmware --dry-run
python scripts/flash/foxglove_flash.py --firmware C:\path\to\firmware
```

Dry-run only reads images; it never queries USB or writes a device. For other
names use `--left L.uf2 --right R.uf2`. `--half left` or `--half right` selects
one half. `--wait-seconds 600` sets the finite device wait; `--json` emits a
structured report with reasons, end addresses, and headroom in KiB.

For a real flash, use an interactive terminal and type `FLASH`. Double-tap reset
on **left only** when prompted. Wait for its keyboard to reappear, test typing in
a text editor, then enter `TYPES` in the terminal. Only then reset the right.
Unknown USB serials are refused. Two bootloaders with known serials are written one
at a time, each half's drive found by its own serial (#654). Volume letters are
discovered using Windows PnP/CIM associations, never assumed: up to 3 tries of
30 s each, each logged, with a fallback that takes the volume with `INFO_UF2.TXT`
on the disk whose device path has that half's serial. More than one match refuses.
The mapped drive must have `INFO_UF2.TXT`; its Board-ID is logged.

**A half stuck in its bootloader (#654).** Before asking any half into its
bootloader, the flasher lists the bootloaders already connected. A half already
in its bootloader (an earlier run stopped after the entry, or a double tap) is
written first, scoped to its serial, and no other half is asked in while it is
unresolved. So after a failed run, run the same command again. `--resume` is the
doctor mode: it writes only the halves already in their bootloaders and asks none
in (`Nothing to resume` when there are none). A bootloader whose half has no image
in this run is reported and the run stops without asking another half in.

Preflight checks every UF2 block's magic, family, flags, payload length, block
numbers, and flash range `0x26000..0xAD000`. All 96 inspected local UF2s start at
`0x26000`. Payloads are decoded into an address-ordered flat image for markers.
Explicit `FOXGLOVE_HALF=left/right` markers take priority. Until firmware gains
those, left requires `studio_rpc_thread` and `raw_hid_received_event`; right
requires `Split Peripheral Notification Queue` and the null-terminated
`ble_peripheral` string. These stable feature names distinguish all 32 inspected
left/right build pairs. Unknown heuristics fail closed; `--allow-unknown-half`
allows only inconclusive identification, never a known wrong half or bad UF2.

Settings-reset images are identified separately by `SETTINGS RESET`. Example:

```powershell
python scripts/flash/foxglove_flash.py --left C:\path\settings_reset-nice_nano_v2-zmk.uf2 --half left --settings-reset
```

Repeat for right if needed. This erases saved settings, requires both `FLASH`
and a separate `ERASE SETTINGS` confirmation, and does not expect a typing
keyboard from the reset firmware. `--settings-reset` refuses ordinary firmware.

The checks prevent past wrong-half flashes and images extending into the
bootloader. The archived `firmware-screens` right image exceeds `0xAD000` and
is correctly refused. Partial writes fail; only errors after every byte was
accepted may be tolerated, and the drive must disappear within 15 seconds.
Exit codes: 0 success, 1 refusal/preflight failure, 2 timeout, 3 flash failure.
Hardware enumeration, actual reset/write behavior, and typing require manual
validation; unit tests use fake devices and synthetic UF2s.

Rollback: use known-good firmware from
`C:\Users\fuchs\splitkeeb\firmware-358c3d0\firmware\`, left first, test typing,
then right. Do not retry with the oversized image.

Run offline checks with:

```powershell
python -m unittest discover -s scripts -p "test_*.py"
python scripts/keymap/keymap_lint.py config/eyelash_corne.keymap
```

The keymap check finds Q/S/Z at positions 1/15/29 and forbids their vendor
soft-off combo. Soft-off is ADMIN + M (position 37), configured for a 2000 ms
hold. ADMIN is conditional on NUMBER+SYMBOL or GAME+ + SYMBOL held together.
The replay covers both thumb press orders and both release orders, normal and
GAME, including releasing both keys. This is a small layer model, not a timing
simulation of the complete ZMK engine.

## Hands-free entry (#111)

When the running firmware advertises HELLO capability bit 9 (test-hooks builds), you don't
need to double-tap:
1. After you type `FLASH`, the flasher asks the left half to reboot into its bootloader. It runs
   `foxglove-lab bootloader`, which sends D7 08: ARM, then TRIGGER with the returned nonce.
2. It flashes the left, waits for the left's own serial to come back, and asks you to confirm
   it types (`TYPES`).
3. It asks the right half to enter its bootloader through the left's relay, retrying for about
   10 s while the relay reconnects, then flashes it.

Serial identity, the UF2 preflight and the typed confirmations are unchanged. If foxglove-lab
isn't built (`cargo build --release -p foxglove-lab`), or the firmware lacks bit 9, it falls
back to double-tap prompts. A settings reset always uses the double tap.

## No prompts: `--yes`

`--yes` is the normal path. The user waived the typed confirmations on 2026-10-24, so On-call
uses it once the user has said go in chat for that flash. It skips the typed
`FLASH`/`ERASE SETTINGS`/`TYPES` prompts and the terminal requirement:

```powershell
python scripts/flash/foxglove_flash.py --firmware C:\path	oirmware --half both --yes
```

Every automatic check still runs:
- serial identity and the UF2 preflight;
- the dev/GA mix refusal;
- the settle wait and one retry;
- reading back the running build.

Instead of `TYPES`, foxglove-lab types into its own window. For the left half that's INVOKE
`&kp`; for the right half it's key-position presses over the split link. The flasher never
touches the right half if the left doesn't type afterwards. The right-half typing check waits
for the relay to the right first (the relay-ready event, up to a minute, #600), and a check
with no reply is tried once more after the next relay-ready wait. If that fails too it is
reported, with the known fix (one tap on the left reset), rather than treated as fatal.

## One flasher at a time

A real flash holds `%TEMP%\foxglove-flash.lock`, which contains its PID. A second flasher
refuses to start while that process is alive, because two writers on one bootloader drive is how
a UF2 lands on the wrong half. A lock left by a process that's gone is taken over. `--dry-run`
takes no lock. Older scripts such as `flash_both.py` don't know about the lock, so don't run
them alongside this one.

## After the write (#176)

When the bootloader has the whole image it reboots, and its drive goes away. Windows then
answers a stat on that drive with WinError 433 or 55, which Python raises. The flasher treats
that as "drive gone" in every post-write step, and then reads the half's build back over D7 09:
the image's SHA running means the flash worked. The 3-second settle before writing stays, since
writes about 1 s after the drive appears were sometimes ignored.

Every write is recorded in `%LOCALAPPDATA%\Foxglove\flash-history.log` (override with
`FOXGLOVE_FLASH_HISTORY`), one tab-separated line each: UTC time, half, image SHA, outcome
(`written`, `written-drive-gone`, `verified`, `unverified`, `verify-failed`), and detail.
