"""Board check for what an outside tool gets from the local API: the brokered mic (#237/#251) and the
per-plugin secret store (#213/#249).

- mic.* facts are readable (audio.inputs.read): mic.state is muted/unmuted/unavailable with an
  endpoint name, or unknown with mic.error.
- secrets.get / secrets.set are denied to an outside tool (secrets belong to plugins only).
- audio.inputs.set_mute on the default communications mic: mic.state follows and Core Audio agrees.
  The mic's mute and volume are recorded first (coreaudio.ps1) and put back exactly in a `finally`,
  so nothing stays muted (the user chose this over installing VB-CABLE).

    python scripts/lab/api_tools.py --app <foxglove.exe>   (exit 0 PASS, 1 FAIL)
"""
import argparse, json, subprocess
from pathlib import Path
from labkit import App, Checks, connect, temp_config, wait

COREAUDIO = Path(__file__).with_name('coreaudio.ps1')


def audio(*a):
    r = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(COREAUDIO), *a],
                       capture_output=True, text=True, check=True)
    return json.loads(r.stdout) if a[0] == 'get' else None

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True)
args = ap.parse_args()
check = Checks()

with App(args.app, temp_config('foxglove-apitools-')):
    api = connect('test-lab-api-tools', ['facts.read:mic.*', 'audio.inputs.read', 'audio.inputs.mute', 'secrets'])
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
    api.close()

check.finish()
