# Lab cases: VIA/Vial writes on a real board (#723)

VIA/Vial writes are off by default (#581). They stay off, and no setting or page turns them on,
until these cases pass on a real board. This page is the test that gates turning them on. The
window-free tests cover the same paths against fakes (`cargo test -p foxglove-via`, and
`app/src/protocols/qmk_common.rs`).

**Status: not run yet.** Queued for On-call, in a batched sitting. It writes to a real keyboard,
so it needs the user's go, relayed by the Hub, before any `--apply` row. The K5 Pro is the user's
daily driver: every row below is reversible, and the keycode row uses a key on the top layer
that holds nothing (`KC_TRNS`).

## How writes are turned on for the test (dev only)

Both are needed. Either one alone leaves writes off.

1. **A `via-writes` build.** Release builds don't have it.
2. **`FOXGLOVE_VIA_WRITES=1`** in the environment of that process. Only the exact value `1`
   counts. Nothing in the UI or in `foxglove.toml` shows or sets it; a `[via] writes = true` key
   is ignored and dropped on the next save.

When writes are on:
- stderr says `VIA/Vial WRITES ARE ON` once, at start;
- every write report is logged before it goes out (`Foxglove: VIA WRITE [..]`);
- in the app, Health shows `VIA/Vial writes: ON: dev test switch FOXGLOVE_VIA_WRITES=1 (#723),
  not for daily use` as a warning.

Whatever the switch says, these never go out: resets (0x06, 0x0A), the bootloader (0x0B),
Keychron's 0xAA-0xAB, macro and encoder writes, the keymap buffer write (0x13), any Vial command
other than the three definition reads (so never unlock 0xFE 0x06), and a bootloader, reboot or
EEPROM-clear keycode on a key (0x7C00, 0x7C01, 0x7C03, legacy 0x5C00).

## The tool

`crates/via/examples/via_write_test.rs`. It's window-free, and it doesn't use the app or its
config. It runs one step at a time. Each step reads before, writes, reads after, puts the old
value back and reads again. It prints `READ-BEFORE`, `WRITE`, `READ-AFTER`, `RESTORE` and
`READ-RESTORED` lines, then `RESULT PASS` or `RESULT FAIL`. Exit codes: 0 pass, 1 fail, 2
refused. Without `--apply`, it only reads and prints what it would write (`WOULD-WRITE`).
`--leave` skips the put-back for the replug check and prints the exact command that puts it back.

Build it once in a worktree on the PR's commit, with your own pool target dir:

```powershell
$env:CARGO_TARGET_DIR = 'C:\Users\fuchs\splitkeeb\worktrees\_targets\pool-N'
cargo build -p foxglove-via --features via-writes --example via_write_test
$t = "$env:CARGO_TARGET_DIR\debug\examples\via_write_test.exe"
```

## Setup

- **One configuration client at a time.** Quit VIA (browser tab), Keychron Launcher and OpenRGB.
  Pause the board in Foxglove (Settings › the board › Pause), or stop Foxglove's VIA scanning.
  The tool and the app must never have the board open together.
- **The board on its USB cable.** The K5 Pro's Raw HID (UP:FF60 U:0061) is USB only; over
  Bluetooth Classic it has none.
- **K5 Pro (VIA): 3434:0253, VIA protocol 11.** It needs a definition for the keycode row: the
  Keychron K5 Pro VIA JSON, or the QMK `keyboard.json` for the matching variant, passed with
  `--definition`. Without one, the keycode and layout rows can't run (the `info` row says so).
  The catalog has no VIA lighting for this model, so the lighting rows are N/A on it. Record that,
  don't force it.
- **Vial board (if one becomes available; not the daily driver).** Its definition comes from the
  board (`--generic`, no file). Prefer one with RGB for the lighting rows.
- Write down the `info` output first. It's the ground truth every `READ-RESTORED` line is
  compared with.

## Cases (K5 Pro, VIA)

`$t` is the tool, and `$k5` is `3434:0253 --definition <path to the K5 Pro JSON>`.

| # | Who | Does | Pass |
|---|---|---|---|
| 1 | agent | `$t $k5 info` (no `--apply`) | `BOARD … VIA protocol 11`; `KEYMAP n keycodes, checksum C0`; no `VIA WRITE` line; exit 0 |
| 2 | agent | `$t $k5 keycode 3 0 0 0x0068` (dry run) | `READ-BEFORE key 3,0,0 = 0x0001` (KC_TRNS; if not, pick a top-layer key that is and use it below); `WOULD-WRITE`; no `VIA WRITE` line |
| 3 | agent | without `FOXGLOVE_VIA_WRITES`: `$t $k5 keycode 3 0 0 0x0068 --apply` | `refused: --apply needs a via-writes build and FOXGLOVE_VIA_WRITES=1`; exit 2; checksum unchanged on a new `info` |
| 4 | agent | `$env:FOXGLOVE_VIA_WRITES='1'; $t $k5 keycode 3 0 0 0x0068 --apply` | stderr banner `VIA/Vial WRITES ARE ON`; `READ-AFTER key = 0x0068`; `READ-AFTER keymap: 1 keycode(s) changed`; `READ-RESTORED key = 0x0001, keymap checksum C0`; `RESULT PASS` |
| 5 | agent | `$t $k5 keycode 3 0 0 0x0068 --apply --leave` | `READ-AFTER key = 0x0068`; `LEFT … put it back with: keycode 3 0 0 0x0001 --apply --leave` |
| 6 | HUMAN | unplug the K5 Pro's USB cable, wait 5 s, plug it back in (one action) | the board types normally on layer 0 |
| 7 | agent | `$t $k5 info` | the checksum differs from C0 (the write survived the replug, so it's in EEPROM) |
| 8 | agent | the put-back command from row 5 | `READ-AFTER key = 0x0001`; a new `info` shows checksum C0 |
| 9 | agent | `$t $k5 keycode 3 0 0 0x7C00 --apply` (QK_BOOT) | `WRITE-ERROR denied: keycode 0x7c00 … is never written`, `NOTHING-WRITTEN`, exit 1 (here the refusal is the pass); no `VIA WRITE` line; checksum C0 |
| 10 | agent | `$t $k5 keycode 9 0 0 0x0068 --apply` (layer past the last) | `WRITE-ERROR denied: … outside this keyboard's … layers`, `NOTHING-WRITTEN`, exit 1; no `VIA WRITE` line; checksum C0 |
| 11 | agent | `$t $k5 layout <a valid other value> --apply`, only if `info` shows layout options and the definition has groups | `READ-AFTER` = the value; `READ-RESTORED` = the `info` value; `RESULT PASS`. Otherwise N/A (record which) |
| 12 | agent | lighting rows 13-15 | N/A on the K5 Pro white (catalog `lighting = "none"`). If a K5 Pro RGB is used instead, run them |
| 16 | agent | the tool's stderr from rows 4-11 | every `VIA WRITE` line is 0x05, or 0x03 for row 11; no 0x06, 0x0A, 0x0B, 0xAA, 0xAB, 0x13 or 0xFE line |
| 17 | agent | `Remove-Item Env:FOXGLOVE_VIA_WRITES`; final `$t $k5 info` | checksum C0, layout options as at row 1; unpause the board in Foxglove |

## Cases (Vial board, if available)

`$v` is `<vvvv:pppp> --generic`. Run rows 1-10 as above with `$v` (the keycode on the top layer).
Then add these rows:

| # | Who | Does | Pass |
|---|---|---|---|
| 13 | agent | `$t $v lighting 120 100 50 <an effect the board has> 128 --apply` | `READ-AFTER` effect = asked, brightness within 1 of 50; `READ-RESTORED raw` = `READ-BEFORE raw`; `RESULT PASS`; no 0x09 (save) line |
| 14 | agent | the same with `--leave` | `LEFT lighting in RAM` |
| 15 | HUMAN | unplug and replug the board (one action) | then the agent runs `$t $v info`: `LIGHTING` equals row 14's `READ-BEFORE raw` (RAM writes don't persist) |
| 18 | agent | `$t $v save-unchanged --apply` | `READ-AFTER raw` = `READ-BEFORE raw`; `RESULT PASS`; one `VIA WRITE [09, …]` line. Saves the lighting as it already is, so nothing changes |
| 19 | agent | the tool's stderr from every Vial row | no `0xFE 0x06/0x07/0x08` (unlock start/poll/lock) line; the only Vial lines are 0xFE 0x00/0x01/0x02 reads |
| 20 | HUMAN | optional, on a spare board only: hold the top-layer key and press the written key during row 5's state | a key tester sees F13 |

## Put back (always, also after an abort)

- Run any `LEFT … put it back with:` command that hasn't run yet.
- `info` must show the row 1 checksum and layout options. If it doesn't, compare the keymaps key
  by key with `keycode L R C <row 1 value>` for each difference, and write them back.
- Remove `FOXGLOVE_VIA_WRITES` from the environment, then unpause the board in Foxglove (and
  restart the user's Foxglove if the lab stopped it).

## What the report must show before writes go on by default

Turning writes on by default (or as a per-keyboard opt-in, #581 option B) waits for one report on
#723 that shows all of the following:

1. **K5 Pro (VIA): rows 1-10, 16 and 17 pass**, with the logs attached. That covers one keycode
   write read back exactly, only that key changed, put back to the starting checksum, survived a
   replug, refusals for QK_BOOT and out-of-range positions, and nothing outside the write list on
   the wire.
2. **Layout options (row 11): pass on a board that has them**, or N/A with the reason.
3. **Lighting (rows 13-15, 18): pass on one board with VIA lighting.** That covers a RAM-only
   write read back, an exact raw put-back, gone after a replug, and a save of the unchanged state
   that changes nothing. If no board here has VIA lighting, lighting writes stay off and only
   keymap writes go on.
4. **Vial (rows 1-10, 19): pass on one Vial board that isn't the daily driver**, with no unlock
   command in the log. If no Vial board is available, writes go on for VIA boards only, and Vial
   stays read-only.
5. **Both write paths named:** the device backend (`crates/via`, these rows) and the KeyPeek
   editor's adapters (`app/src/protocols/{via,vial}.rs`, through `qmk_via_api`). The editor's
   path needs one more HUMAN row: one key in the editor, then undo. Its log shows one 0x05 write
   and the put-back. Until it passes, the editor stays read-only even if the backend goes on.
6. **No regressions while off:** a release build, with no `via-writes` and no switch, shows
   `VIA/Vial writes: off (0.2)`, and its log has no `VIA WRITE` line during a full sitting.

Only keycodes, checksums, lighting values and log lines go into the report. Never record what was
typed.
