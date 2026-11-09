"""The pure half of the hideout idle-footprint case (#935): sets file, sample summary, baseline
compare. Stdlib only, no Windows calls and no process launches, so scripts/test_hideout_footprint.py
runs it window-free anywhere.

Metrics (all about the hub process, hideout.exe, while it idles):
  private_mb, working_set_mb  memory (MB)
  cpu_pct                     CPU time over the window / wall time, % of one core
  wakeups_per_s               context switches per second
  gdi_handles, user_handles   GetGuiResources counts
  vram_mb                     dedicated GPU memory, None where Windows has no counter for it
"""
import json

METRICS = ('private_mb', 'working_set_mb', 'cpu_pct', 'wakeups_per_s', 'gdi_handles', 'user_handles', 'vram_mb')


def load_sets(text):
    """{"sets": {"none": [], "default": [<module folder>, ...]}} -> {name: [folders]}. The "none"
    set is always there and always empty; a set that isn't a list of strings is an error."""
    data = json.loads(text)
    sets = data.get('sets', {})
    if not isinstance(sets, dict):
        raise ValueError('"sets" must be an object of name -> list of module folders')
    out = {'none': []}
    for name, folders in sets.items():
        if name == 'none':
            if folders:
                raise ValueError('the "none" set is the empty set')
            continue
        if not isinstance(folders, list) or not all(isinstance(f, str) and f for f in folders):
            raise ValueError(f'set "{name}": a list of module folders is needed')
        out[name] = list(folders)
    return out


def summarize(first, last, wall_s):
    """Two samples of the same process, `wall_s` seconds apart, -> the metrics. A sample is a dict:
    private_b, working_set_b, cpu_100ns, ctx_switches, gdi, user, vram_b (None when unavailable).
    The memory and handle numbers are those at the end of the window."""
    if wall_s <= 0:
        raise ValueError('the window must be longer than zero')
    mb = 1024 * 1024
    vram = last.get('vram_b')
    return {
        'private_mb': round(last['private_b'] / mb, 2),
        'working_set_mb': round(last['working_set_b'] / mb, 2),
        'cpu_pct': round((last['cpu_100ns'] - first['cpu_100ns']) / 1e7 / wall_s * 100, 3),
        'wakeups_per_s': round((last['ctx_switches'] - first['ctx_switches']) / wall_s, 2),
        'gdi_handles': last['gdi'],
        'user_handles': last['user'],
        'vram_mb': None if vram is None else round(vram / mb, 2),
    }


def limit(entry):
    """The most a metric may read: baseline * (1 + pct/100) + slack. A baseline entry is
    {"value": v, "pct": p, "slack": s}; slack (an absolute allowance in the metric's unit) keeps
    a tiny baseline from failing on noise."""
    return entry['value'] * (1 + entry.get('pct', 0) / 100) + entry.get('slack', 0)


def compare(measured, baseline):
    """Per metric: (name, measured, limit, status). Status is PASS, FAIL (past the limit), SKIP
    (no baseline for it, or no reading such as VRAM without a counter) or MISSING (a baselined
    metric that should have a reading but has none, except vram_mb which may be absent)."""
    rows = []
    for name in METRICS:
        got, entry = measured.get(name), baseline.get(name)
        if entry is None:
            rows.append((name, got, None, 'SKIP'))
        elif got is None:
            rows.append((name, None, limit(entry), 'SKIP' if name == 'vram_mb' else 'MISSING'))
        else:
            cap = limit(entry)
            rows.append((name, got, cap, 'FAIL' if got > cap else 'PASS'))
    return rows


def verdict(rows):
    """FAIL when any metric is past its limit, BLOCKED when a reading is missing, else PASS."""
    states = {r[3] for r in rows}
    return 'FAIL' if 'FAIL' in states else 'BLOCKED' if 'MISSING' in states else 'PASS'


def record(batch, results):
    """The per-batch results file's text: {batch, sets: {name: {metrics, rows, verdict}}}."""
    return json.dumps({'batch': batch, 'sets': results}, indent=2, sort_keys=True) + '\n'
