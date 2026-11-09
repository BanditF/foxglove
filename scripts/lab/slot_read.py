"""SLOT_READ on hardware (#911; the readback is #878/#908): a piece kept on the keyboard that this PC's
art library has no file for is read back from the keyboard, its frames reach a live-feed client (what
the paired wallpaper is), and art/kept-<crc>.bin lands in the config folder with a matching CRC.

Window-free. Starts the test app on a temp FOXGLOVE_CONFIG_DIR with an EMPTY art library (so the
piece is "removed from the PC library" from the start) and the live feed on at a known token and port.
Then tests/hil/slot-read.toml's keep case stores tests/art/ink-fixture.bin in the first empty flash
place and shows it, and this script, as the wallpaper, reads the feed on ws://127.0.0.1:<port> until
the `art` message for that piece has frames (`nva` not null), and checks the file and its CRC.
The erase case puts the keyboard back (it empties only the place that keeps this blob).

    FOXGLOVE_APP_OK=1 python scripts/lab/slot_read.py --app <foxglove.exe> [--lab <foxglove-lab.exe>]
Exit 0 PASS, 1 FAIL, 2 BLOCKED (no app OK, no free flash place, the right not connected).
On-call runs it; it launches Foxglove and writes one flash place.
"""
import argparse, base64, binascii, json, os, socket, struct, subprocess, sys, time
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


def lab(args, only):
    r = subprocess.run([str(args.lab), '--allow-app', 'run', '--suite', str(SUITE), '--only', only, '--non-interactive',
                        '--keep-lights'], capture_output=True, text=True, encoding='utf-8', errors='replace',
                       cwd=REPO, timeout=300)
    return r.returncode, (r.stdout + r.stderr).strip()[-600:]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'))
    ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
    ap.add_argument('--within', type=float, default=60.0, help='seconds for the frames to arrive')
    args = ap.parse_args()
    check = Checks()
    crc = binascii.crc32(BLOB.read_bytes()) & 0xffffffff
    cfg = temp_config('foxglove-slotread-', f'[live_wallpaper]\nenabled = true\nport = {PORT}\ntoken = "{TOKEN}"\n')
    kept = cfg / 'art' / f'kept-{crc:08x}.bin'
    with App(args.app, cfg) as app:
        time.sleep(8)  # the feed listens once the app is up
        code, out = lab(args, 'slot-read-keep')
        if code == 2:
            check('the piece is kept on the keyboard', False, out, blocked=True)
            check.finish()
        check('the piece is kept on the keyboard', code == 0, out)
        try:
            library = [p.name for p in (cfg / 'art').glob('*.bin') if not p.name.startswith('kept-')] if (cfg / 'art').exists() else []
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
                check('the frames are the piece\'s bytes', base64.b64decode(got['nva']) == data, f'{len(data)} bytes')
            for line in log_lines(cfg, 'read back') + log_lines(cfg, 'not read back'):
                print('app log:', line, flush=True)
        finally:
            code, out = lab(args, 'slot-read-erase')
            check('the keyboard is put back (place erased)', code == 0, out)
    check.finish()


if __name__ == '__main__':
    main()
