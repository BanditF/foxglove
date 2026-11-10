"""Restart parity on hardware (#1053): a fact Foxglove publishes must come back the same after the
app restarts while the keyboard keeps going. Each case establishes its state through the app's
real interfaces, reads the published facts, has labkit.restart_parity stop the test app and start
a fresh one on the same temp config folder, and asserts the same facts.

Cases (--case, default all):
  pushed_set  push tests/art/ink-fixture.bin (art.select file); the live feed's art (from, name,
              frames, missing) and the `feed_art` health row (#1053, not Warn) are the same after a
              restart (#1044: NO FRAMES). The right is put back on the built-in it showed at the end.

To add a case for a feature that publishes state: write `establish()` (an API call or UI flow,
never a store write by hand) and `read(want)` (event-driven: the feed's pushed messages, an API
answer; bounded), add it to CASES; docs/agents.md "Restart parity" has the rule.

Window-free (no window is looked at or clicked); it launches Foxglove twice on a temp
FOXGLOVE_CONFIG_DIR with the live feed on at a known port and token. On-call runs it:

    FOXGLOVE_APP_OK=1 python scripts/lab/restart_parity.py --app <foxglove.exe> [--case pushed_set]
Exit 0 PASS, 1 FAIL, 2 BLOCKED (no app OK, no keyboard).
"""
import argparse, sys

from labkit import App, Checks, REPO, connect, feed_art_facts, feed_messages, log_lines, restart_parity, temp_config, \
    tool_arg, users_keyboard, wait_fact

FIXTURE = REPO / 'tests' / 'art' / 'ink-fixture.bin'
PORT = 8769
TOKEN = 'labrestartparity00000000'  # 24 characters, the feed's own length
FEED_ART = 'live_feed.feed_art'


def device(api, seconds=30):
    """The keyboard's id once the app has it (the `keyboard.connected` event, bounded), else None."""
    if wait_fact('restart-parity-wait', 'keyboard.connected', lambda v: v is True, seconds) is None:
        return None
    d = api.call('devices.list', {}).get('devices', [])
    return d[0]['id'] if d else None


def feed_art_row(api, dev):
    """The `feed_art` health row's level (health.read), or None when there is no row."""
    rows = api.call('health.read', {'device': dev}).get('rows', [])
    return next((r.get('level') for r in rows if r.get('subsystem') == FEED_ART), None)


def pushed_set(app, check, within):
    """#1044 / #1053: a pushed set's feed art and health row across a restart."""
    api = connect('restart-parity', ['devices.read', 'art.write'])
    dev = device(api)
    if dev is None:
        check('a keyboard', False, 'none found', blocked=True)
        return
    start = api.call('art.status', {'device': dev})
    print('art.status before:', start, flush=True)

    def establish():
        print('push:', api.call('art.select', {'device': dev, 'target': {'file': str(FIXTURE)}}), flush=True)

    def read(want):
        alive = lambda: app.exited() is None
        if want is None:
            until = lambda f: bool(f and f.get('art') and f['art'].get('from') == 'file' and f['art'].get('frames'))
        else:
            until = lambda f: f == want
        facts = feed_art_facts(feed_messages(PORT, TOKEN, within, alive), until)
        return facts if facts and facts.get('art') else None

    try:
        restart_parity(app, check, 'pushed set: the feed art', establish, read)
        fresh = connect('restart-parity', ['devices.read', 'art.write'])
        dev = device(fresh) or dev
        level = feed_art_row(fresh, dev)
        check('pushed set: the feed_art health row after the restart', level == 'ok', f'level {level}')
        for line in log_lines(app.cfg, 'live feed: no art') + log_lines(app.cfg, 'pushed set'):
            print('app log:', line, flush=True)
        api = fresh
    finally:
        if start and start.get('built_in'):
            print('back to', start['built_in'],
                  api.call('art.select', {'device': dev, 'target': {'built_in': start['built_in']}}), flush=True)


CASES = {'pushed_set': pushed_set}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'))
    ap.add_argument('--case', choices=sorted(CASES), action='append', help='a case to run (default: all)')
    ap.add_argument('--within', type=float, default=60.0, help='seconds for the feed to publish its facts')
    args = ap.parse_args()
    check = Checks()
    for name in args.case or sorted(CASES):
        cfg = temp_config(f'foxglove-restart-{name}-', users_keyboard() +
                          f'\n[live_wallpaper]\nenabled = true\nport = {PORT}\ntoken = "{TOKEN}"\n')
        with App(args.app, cfg) as app:
            CASES[name](app, check, args.within)
    check.finish()


if __name__ == '__main__':
    sys.exit(main())
