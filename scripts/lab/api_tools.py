"""Board check for what an outside tool gets from the local API: the brokered mic (#237/#251) and the
per-plugin secret store (#213/#249).

- mic.* facts are readable (audio.inputs.read): mic.state is muted/unmuted/unavailable with an
  endpoint name, or unknown with mic.error.
- secrets.get / secrets.set are denied to an outside tool (secrets belong to plugins only).
- audio.inputs.set_mute on the default communications mic: mic.state follows and Core Audio agrees.
  The mic's mute and volume are recorded first (coreaudio.ps1) and put back exactly in a `finally`,
  so nothing stays muted (the user chose this over installing VB-CABLE).
- pc_lighting.set_zone on the first writable PC lighting device (#617): zone 1 to a test colour,
  read back with pc_lighting.list, one "zone colours of" line in the log; the user's saved colour
  (copied from the real foxglove.toml's [pc_lighting]) goes back in a `finally`.
- lighting.set_sync on the keyboard (#617, #606): flipped, read back, one "lighting sync" log
  line; put back in a `finally`.

    python scripts/lab/api_tools.py --app <foxglove.exe>   (exit 0 PASS, 1 FAIL)
"""
import argparse, json, os, subprocess, tomllib
from pathlib import Path
from labkit import App, tool_arg, Checks, connect, temp_config, wait

COREAUDIO = Path(__file__).with_name('coreaudio.ps1')
REAL_STORE = Path(os.environ['APPDATA']) / 'Foxglove' / 'foxglove.toml'


def user_pc_lighting():
    """The user's [pc_lighting] (read only), as TOML for the temp store: the test app puts the same
    colours back on arrival, and the checks know what to restore."""
    try:
        devices = tomllib.loads(REAL_STORE.read_text(encoding='utf-8')).get('pc_lighting', {}).get('devices', {})
    except (OSError, tomllib.TOMLDecodeError):
        devices = {}
    out = ''
    for key, d in devices.items():
        out += f'\n[pc_lighting.devices."{key}"]\nmatch_keyboard = {str(bool(d.get("match_keyboard"))).lower()}\n'
        out += f'[pc_lighting.devices."{key}".colors]\n'
        out += ''.join(f'"{z}" = "{c}"\n' for z, c in d.get('colors', {}).items())
    return devices, out


def log_lines(cfg, piece):
    log = cfg / 'logs' / 'foxglove.log'
    return [l for l in log.read_text(encoding='utf-8', errors='replace').splitlines() if piece in l] if log.exists() else []


def audio(*a):
    r = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(COREAUDIO), *a],
                       capture_output=True, text=True, check=True)
    return json.loads(r.stdout) if a[0] == 'get' else None

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'))
args = ap.parse_args()
check = Checks()

saved, store = user_pc_lighting()
cfg = temp_config('foxglove-apitools-', store)
with App(args.app, cfg):
    api = connect('test-lab-api-tools', ['facts.read:mic.*', 'audio.inputs.read', 'audio.inputs.mute', 'secrets',
                                         'devices.read', 'lighting.write'])
    print('granted:', api.granted, flush=True)

    def mic():
        return {f['key']: f['value'] for f in api.call('facts.list', {'pattern': 'mic.*'})['facts']}

    ok, m = wait(mic, lambda m: m.get('mic.state') not in (None, 'unknown'), 10)
    state = m.get('mic.state')
    check('mic.* facts readable', ok and (state == 'unavailable' or m.get('mic.endpoint_name')), m)
    for method, params in (('secrets.get', {'name': 'token'}), ('secrets.set', {'name': 'token', 'value': 'x'})):
        try:
            api.call(method, params)
            check(f'{method} denied to an outside tool', False, 'it answered')
        except RuntimeError as e:
            check(f'{method} denied to an outside tool', 'denied' in str(e).lower() or "'code': 5" in str(e), e)

    original = audio('get')
    comms = lambda: next(e for e in audio('get') if e['role'] == 'communications')
    try:
        if not comms().get('id'):
            check('set_mute', False, 'no default capture device on this PC: needs a human', blocked=True)
        else:
            was = comms()['muted']
            for want in (not was, was):
                api.call('audio.inputs.set_mute', {'muted': want})
                ok, m = wait(mic, lambda m: m.get('mic.state') == ('muted' if want else 'unmuted'), 5)
                check(f'set_mute {want}: mic.state follows', ok, m)
                ok, e = wait(comms, lambda e: e['muted'] == want, 5)
                check(f'set_mute {want}: Core Audio agrees', ok, e)
    finally:
        for e in original:
            if e.get('id'):
                audio('set', e['role'], '1' if e['muted'] else '0', str(e['volume']))
        now = audio('get')
        check('microphone left exactly as it was', [(e.get('muted'), e.get('volume')) for e in now]
              == [(e.get('muted'), e.get('volume')) for e in original], now)

    # Case zones (#617): set, read back, log line, the user's colour back.
    pcs = lambda: api.call('pc_lighting.list', {})
    ok, listed = wait(pcs, lambda l: any(d['writable'] and d['zones'] for d in l['devices']), 10)
    dev = next((d for d in listed['devices'] if d['writable'] and d['zones']), None)
    if not dev:
        check('pc_lighting.set_zone', False, 'no writable PC lighting device on this PC', blocked=True)
    else:
        zone = dev['zones'][0]
        was = saved.get(dev['device'], {}).get('colors', {}).get(zone['id']) or zone['color']
        test = '#00ff00' if (was or '').lower() != '#00ff00' else '#0000ff'
        before = len(log_lines(cfg, 'zone colours of'))
        try:
            got = api.call('pc_lighting.set_zone', {'device': dev['device'], 'zone': zone['id'], 'color': test})
            shown = next(z['color'] for d in got['devices'] if d['device'] == dev['device'] for z in d['zones'] if z['id'] == zone['id'])
            check('set_zone: the answer reads the colour back', shown == test, got.get('status'))
            again = next(z['color'] for d in pcs()['devices'] if d['device'] == dev['device'] for z in d['zones'] if z['id'] == zone['id'])
            check('pc_lighting.list reads it back', again == test, again)
            ok, lines = wait(lambda: log_lines(cfg, 'zone colours of'), lambda l: len(l) == before + 1, 5)
            check('one "zone colours of" log line, old -> new', ok and test in lines[-1], lines[-1:] or 'no log file')
            try:
                api.call('pc_lighting.set_zone', {'device': dev['device'], 'zone': zone['id'], 'color': 'red'})
                check('set_zone refuses a bad colour', False, 'it answered')
            except RuntimeError as e:
                check('set_zone refuses a bad colour', "'code': -32602" in str(e), e)
        finally:
            if was:
                api.call('pc_lighting.set_zone', {'device': dev['device'], 'zone': zone['id'], 'color': was})
            now = next(z['color'] for d in pcs()['devices'] if d['device'] == dev['device'] for z in d['zones'] if z['id'] == zone['id'])
            check('case zone left as it was', not was or now == was, f'{now} (was {was})', blocked=not was)

    # Lighting sync (#617, #606): flip, read back, log line, put back.
    board = next((d for d in api.call('devices.list', {})['devices'] if 'settings' in d['caps'] and d['connected']), None)
    if not board:
        check('lighting.set_sync', False, 'no keyboard with Foxglove\'s host link connected', blocked=True)
    else:
        was = api.call('lighting.sync', {'device': board['id']})['on']
        before = len(log_lines(cfg, 'lighting sync (right follows left)'))
        try:
            got = api.call('lighting.set_sync', {'device': board['id'], 'on': not was})
            check('set_sync: read back flipped', got['on'] == (not was), got)
            check('lighting.sync agrees', api.call('lighting.sync', {'device': board['id']})['on'] == (not was), '')
            ok, lines = wait(lambda: log_lines(cfg, 'lighting sync (right follows left)'), lambda l: len(l) == before + 1, 5)
            check('one "lighting sync" log line, old -> new', ok, lines[-1:] or 'no log file')
        finally:
            back = api.call('lighting.set_sync', {'device': board['id'], 'on': was})
            check('lighting sync left as it was', back['on'] == was, back)
    api.close()

check.finish()
