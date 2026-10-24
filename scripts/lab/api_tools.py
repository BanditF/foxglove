"""Board check for what an outside tool gets from the local API: the brokered mic (#237/#251) and the
per-plugin secret store (#213/#249).

- mic.* facts are readable (audio.inputs.read): mic.state is muted/unmuted/unavailable with an
  endpoint name, or unknown with mic.error.
- secrets.get / secrets.set are denied to an outside tool (secrets belong to plugins only).
- With --vb-cable only: audio.inputs.set_mute toggles VB-CABLE's mute, mic.state follows, and the
  original mute comes back. Refused unless the default communications mic IS the VB-CABLE
  endpoint, so the user's real microphone is never muted. (Installing VB-CABLE and making it the
  default communications device is a manual step.)

    python scripts/lab/api_tools.py --app <foxglove.exe> [--vb-cable]   (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, time
from labkit import App, Checks, connect, temp_config, wait

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True)
ap.add_argument('--vb-cable', action='store_true', help='also test set_mute (VB-CABLE must be the default communications mic)')
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

    if not args.vb_cable:
        check('set_mute', False, 'needs --vb-cable (VB-CABLE as the default communications mic)', blocked=True)
    elif 'cable' not in str(m.get('mic.endpoint_name', '')).lower():
        check('set_mute', False, f"refused: the default communications mic is {m.get('mic.endpoint_name')!r}, "
              'not VB-CABLE; the real microphone is never muted', blocked=True)
    else:
        original = state == 'muted'
        for want in (not original, original):
            api.call('audio.inputs.set_mute', {'muted': want})
            ok, m = wait(mic, lambda m: m.get('mic.state') == ('muted' if want else 'unmuted'), 5)
            check(f'set_mute {want}: mic.state follows', ok, m)
    api.close()

check.finish()
