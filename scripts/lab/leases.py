"""Board check for lighting leases (#236/#250): who shows, giving back, and ending with the connection.

Two outside-tool connections on the local API hold leases on the keyboard's underglow:
A low green, then B normal blue (shows), then A high red (shows); A releases red → B's blue;
B's connection closes → A's green; A's closes → the lights as they were before the first lease.
Both halves are read back with RGB_STATE; the right half takes seconds to follow (split relay).

    python scripts/lab/leases.py --app <foxglove.exe>      (exit 0 PASS, 1 FAIL)
"""
import argparse, sys, time
from labkit import App, Checks, TARGET, connect, hue_is, rgb, same, temp_config, wait

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True)
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'))
args = ap.parse_args()

cfg = temp_config('foxglove-leases-')
check = Checks()
RIGHT_LAG = 10  # seconds; the right half follows over the split link (median 1.7 s, max 5.9 s seen)

with App(args.app, cfg) as app:
    read = lambda: rgb(args.lab, app.env)
    perms = ['devices.read', 'lighting.write']
    a = connect('test-lab-leases-a', perms)
    b = connect('test-lab-leases-b', perms)
    device = next(d['id'] for d in a.call('devices.list', {})['devices'] if 'lighting' in d['caps'])
    before = read()
    print('before:', before, flush=True)
    lease = lambda api, hue, priority: api.call('lighting.lease', {
        'device': device, 'color': {'hue': hue, 'saturation': 100, 'brightness': 100}, 'priority': priority})['lease']

    lease(a, 120, 'low')
    check('A low green shows', *wait(read, hue_is(120), RIGHT_LAG))
    lease(b, 240, 'normal')
    check('B normal blue shows over A low', *wait(read, hue_is(240), RIGHT_LAG))
    red = lease(a, 0, 'high')
    check('A high red shows over B normal', *wait(read, hue_is(0), RIGHT_LAG))
    lease(b, 60, 'low')
    ok, s = wait(read, hue_is(60), 4)
    check('B low yellow does not take over from red', not ok and hue_is(0)(s), s)
    a.call('lighting.release', {'lease': red})
    check('A releases red: B normal blue is back', *wait(read, hue_is(240), RIGHT_LAG))
    b.close()
    check("B's connection closes: A low green is back", *wait(read, hue_is(120), RIGHT_LAG))
    a.close()
    ok, s = wait(read, lambda s: same(s, before), RIGHT_LAG)
    check('last lease ends: the lights are as before', ok, s)
    time.sleep(5)
    s = read()
    check('...and still as before 5 s later', same(s, before), s)

check.finish()
