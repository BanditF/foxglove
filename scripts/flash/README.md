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
If two bootloaders appear, unplug one. Unknown USB serials are refused. Volume
letters are discovered using Windows PnP/CIM associations, never assumed.
The mapped drive must have `INFO_UF2.TXT`; its Board-ID is logged.

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
isn't built (`cargo build --release -p foxglove-lab`), or the firmware lacks bit 8, it falls
back to double-tap prompts. A settings reset always uses the double tap.
