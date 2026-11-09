"""SLOT_READ on hardware (#911; the readback is #878/#908): a piece kept on the keyboard that this PC's
art library has no file for is read back from the keyboard, its frames reach a live-feed client (what
the paired wallpaper is), and art/kept-<crc>.bin lands in the config folder with a matching CRC.

The keyboard has no empty flash place, so this borrows a KEPT one and gives it back, never losing it:
  1. picks the place (--slot N, default the first kept one);
  2. reads its exact bytes (SLOT_READ), writes them to a backup file on disk FIRST, records the CRC;
  3. runs the test in that place: writes tests/art/ink-fixture.bin, checks the live feed sends frames
     and art/kept-<crc>.bin lands with a matching CRC;
  4. writes the original bytes back, reads them back, compares CRC and every byte;
  5. on ANY failure or exception it still restores (try/finally); if the restore cannot be verified
     it prints the backup file path and exact manual restore instructions, and exits 1.
--dry-run does only steps 1, 2 and a verify of the place against the backup: nothing is written to
the keyboard, no app is started.

Window-free. The test app runs on a temp FOXGLOVE_CONFIG_DIR with an EMPTY art library (so the
piece is "removed from the PC library" from the start) and the live feed on at a known token and port.
The lab cases are tests/hil/slot-read.toml (the `art_slot` step).

    FOXGLOVE_APP_OK=1 python scripts/lab/slot_read.py --app <foxglove.exe> [--lab <foxglove-lab.exe>] [--slot N] [--dry-run]
Exit 0 PASS, 1 FAIL, 2 BLOCKED (no app OK, no kept place, the right not connected).
Backups stay in <temp>/foxglove-slot-backups (this script never deletes them).
On-call runs it; it launches Foxglove and (unless --dry-run) rewrites one flash place and restores it.
"""
import argparse, base64, binascii, json, os, socket, struct, subprocess, sys, tempfile, time
from pathlib import Path

from labkit import App, Checks, REPO, TARGET, temp_config, tool_arg, log_lines

BLOB = REPO / 'tests' / 'art' / 'ink-fixture.bin'
SUITE = REPO / 'tests' / 'hil' / 'slot-read.toml'
PORT = 8768
TOKEN = 'labslotread0000000000000'  # 24 characters, the feed's own length


def ws_read(port, token, seconds):
    """Yields the feed's JSON text messages for `seconds` (a minimal client: handshake, then frames)."""
    s = socket.create_connection(('127.0.0.1', port), timeout=5)
    key = base64.b64encode(os.urandom(16)).decode()
    s.sendall((f'GET /live?token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n'
               f'Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
    buf = b''
    while b'\r\n\r\n' not in buf:
        chunk = s.recv(4096)
        if not chunk:
            return
        buf += chunk
    buf = buf.split(b'\r\n\r\n', 1)[1]
    end = time.time() + seconds
    s.settimeout(1)
    while time.time() < end:
        try:
            while len(buf) < 2:
                buf += s.recv(65536)
            n = buf[1] & 0x7f
            at = 2
            if n == 126:
                while len(buf) < 4:
                    buf += s.recv(65536)
                n, at = struct.unpack('>H', buf[2:4])[0], 4
            elif n == 127:
                while len(buf) < 10:
                    buf += s.recv(65536)
                n, at = struct.unpack('>Q', buf[2:10])[0], 10
            while len(buf) < at + n:
                buf += s.recv(65536)
        except socket.timeout:
            continue
        except OSError:
            return
        payload, buf = buf[at:at + n], buf[at + n:]
        if not payload:
            continue
        try:
            yield json.loads(payload)
        except ValueError:
            continue
    s.close()


def lab_step(args, case, env):
    """Runs one tests/hil/slot-read.toml case; (exit code, last output)."""
    try:
        r = subprocess.run([str(args.lab), '--allow-app', 'run', '--suite', str(SUITE), '--only', case, '--non-interactive',
                            '--keep-lights'], capture_output=True, text=True, encoding='utf-8', errors='replace',
                           cwd=REPO, timeout=300, env={**os.environ, **env})
    except subprocess.TimeoutExpired:
        return 1, f'{case}: no end within 300 s'
    return r.returncode, (r.stdout + r.stderr).strip()[-600:]


def manual_restore(slot, base):
    return (f'MANUAL RESTORE NEEDED for flash place {slot}. The original bytes are in {base}.bin '
            f'(name, CRC and length in {base}.json). To retry: set FOXGLOVE_SLOT_READ_SLOT={slot} and '
            f'FOXGLOVE_SLOT_READ_BACKUP={base}, then run foxglove-lab --allow-app run --suite '
            f'tests/hil/slot-read.toml --only slot-read-restore --non-interactive. If that fails, store '
            f'{base}.bin into place {slot} under the name in the json (the Foxglove art page, or art::store '
            f'in foxglove-link), and check the place\'s CRC equals the json\'s crc.')


def pick_slot(step, base, want):
    """The place to borrow: `want`, else the first kept place (the lab's `list` case writes the kept
    places to <base>). Returns (slot, None) or (None, why)."""
    if want is not None:
        return want, None
    code, out = step('slot-read-list', {'FOXGLOVE_SLOT_READ_BACKUP': f'{base}.list.json'})
    if code != 0:
        return None, out
    try:
        kept = json.loads(Path(f'{base}.list.json').read_text(encoding='utf-8'))
    except (OSError, ValueError) as e:
        return None, f'the kept-places list is unreadable: {e}'
    if not kept:
        return None, 'no kept flash place to borrow'
    return kept[0]['slot'], None


def run_with_backup(step, check, slot, base, dry_run, body, retries=3, pause=3.0):
    """The data-safe flow. `step(case, env)` -> (code, out) runs a lab case; `body()` is the test
    inside the borrowed place. The restore runs in a finally whenever the fixture write was even tried.
    Returns True when everything passed."""
    env = {'FOXGLOVE_SLOT_READ_SLOT': str(slot), 'FOXGLOVE_SLOT_READ_BACKUP': base}
    code, out = step('slot-read-backup', env)
    if code != 0:
        check(f'place {slot} is backed up to disk', False, out, blocked=code == 2)
        return False
    try:
        meta = json.loads(Path(f'{base}.json').read_text(encoding='utf-8'))
        detail = f"crc {meta['crc']:08x}, {meta['len']} bytes, {base}.bin"
    except (OSError, ValueError, KeyError):
        detail = out
    check(f'place {slot} is backed up to disk', True, detail)
    if dry_run:
        code, out = step('slot-read-verify', env)
        check(f'place {slot} reads back identical to its backup (dry run, nothing written)', code == 0, out)
        return code == 0
    written = False
    ok = True
    try:
        written = True  # from here the place may have changed, even if the write step then fails
        code, out = step('slot-read-write', env)
        if code != 0:
            check('the fixture is written in the place', False, out, blocked=code == 2)
            ok = False
        else:
            check('the fixture is written in the place', True, out)
            body()
    except BaseException as e:  # noqa: BLE001 - the restore below runs for anything
        check('the test ran to the end', False, f'{type(e).__name__}: {e}')
        ok = False
        if isinstance(e, (KeyboardInterrupt, SystemExit)):
            raise
    finally:
        if written:
            restored = False
            out = ''
            for attempt in range(retries):
                try:
                    code, out = step('slot-read-restore', env)
                except BaseException as e:  # noqa: BLE001
                    code, out = 1, f'{type(e).__name__}: {e}'
                if code == 0:
                    restored = True
                    break
                if attempt + 1 < retries:
                    time.sleep(pause)
            check(f'place {slot} is restored and verified byte-identical', restored, out)
            if not restored:
                print(manual_restore(slot, base), flush=True)
                ok = False
    return ok


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'))
    ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
    ap.add_argument('--within', type=float, default=60.0, help='seconds for the frames to arrive')
    ap.add_argument('--slot', type=int, default=None, help='the kept flash place to borrow (default: the first kept one)')
    ap.add_argument('--dry-run', action='store_true', help='only back up the place and verify it; nothing is written')
    args = ap.parse_args()
    check = Checks()
    crc = binascii.crc32(BLOB.read_bytes()) & 0xffffffff

    def step(case, env):
        return lab_step(args, case, env)

    folder = Path(tempfile.gettempdir()) / 'foxglove-slot-backups' / time.strftime('%Y%m%d-%H%M%S')
    folder.mkdir(parents=True, exist_ok=True)
    slot, why = pick_slot(step, str(folder / 'pick'), args.slot)
    if slot is None:
        check('a kept flash place to borrow', False, why, blocked=True)
        check.finish()
    base = str(folder / f'slot{slot}')
    if args.dry_run:
        run_with_backup(step, check, slot, base, True, None)
        check.finish()
    cfg = temp_config('foxglove-slotread-', f'[live_wallpaper]\nenabled = true\nport = {PORT}\ntoken = "{TOKEN}"\n')
    kept = cfg / 'art' / f'kept-{crc:08x}.bin'

    def body():
        with App(args.app, cfg):
            time.sleep(8)  # the feed listens once the app is up
            art_dir = cfg / 'art'
            library = [p.name for p in art_dir.glob('*.bin') if not p.name.startswith('kept-')] if art_dir.exists() else []
            check('the PC library has no file for it', not library, library)
            got = None
            for msg in ws_read(PORT, TOKEN, args.within):
                art = msg.get('art') if msg.get('t') == 'art' else None
                if art and art.get('from') == 'kept' and art.get('nva'):
                    got = art
                    break
            check('the feed sends the kept piece with frames', got is not None,
                  f"name {got.get('name')}" if got else 'no art message with nva within the time')
            data = kept.read_bytes() if kept.exists() else None
            check(f'art/kept-{crc:08x}.bin is written', data is not None, str(kept))
            check('its CRC matches the piece', data is not None and binascii.crc32(data) & 0xffffffff == crc,
                  f'{binascii.crc32(data) & 0xffffffff:08x}' if data else 'no file')
            if got and data:
                check("the frames are the piece's bytes", base64.b64decode(got['nva']) == data, f'{len(data)} bytes')
            for line in log_lines(cfg, 'read back') + log_lines(cfg, 'not read back'):
                print('app log:', line, flush=True)

    run_with_backup(step, check, slot, base, False, body)
    check.finish()


if __name__ == '__main__':
    main()
