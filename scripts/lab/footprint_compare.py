"""The pure half of the hideout idle-footprint case (#935): sets file, sample summary, baseline
compare. Stdlib only, no Windows calls and no process launches, so scripts/test_hideout_footprint.py
runs it window-free anywhere.

Metrics (all about the hub process, hideout.exe, while it idles):
  private_mb, working_set_mb  memory (MB)
  cpu_pct                     CPU time over the window / wall time, % of one core
  loop_wakes_per_s            the hub's own event-loop wakes per second, from its log lines
                              (`event loop: N wakes in S s (repaint a, timer b, ...)`, #1000); the
                              limit is per window state: `loop_wakes_per_s` hidden (at most 1/s),
                              `loop_wakes_shown_per_s` shown
  wakeups_per_s               context switches per second summed over all the process's threads
                              (the Thread counter class: the Process class reads 0 for the hub),
                              None where no counter exists. INFO only, never a failure (#1000):
                              most are OS and driver work on the hub's threads (the GL driver,
                              TSF/IME, COM) that never return from the loop's wait
  gdi_handles, user_handles   GetGuiResources counts
  vram_mb                     dedicated GPU memory, None where Windows has no counter for it
"""
import json
import re

# Metrics that may legitimately have no reading on a Windows build: recorded as unavailable, never a failure.
OPTIONAL = ('wakeups_per_s', 'vram_mb')
# Recorded and shown, never compared: status INFO whatever the reading.
INFO = ('wakeups_per_s',)
METRICS = ('private_mb', 'working_set_mb', 'cpu_pct', 'loop_wakes_per_s', 'wakeups_per_s', 'gdi_handles',
           'user_handles', 'vram_mb')
STATES = ('hidden', 'shown')
# Baseline entries: the metrics, plus the shown window's own loop-wake limit.
BASELINE_KEYS = METRICS + ('loop_wakes_shown_per_s',)

WAKE_LINE = re.compile(r'event loop: (\d+) wakes in (\d+) s(?: \(([^)]*)\))?')


def baseline_key(metric, state='hidden'):
    """The baseline entry a metric is compared with in `state`: the loop-wake limit is per state."""
    if metric == 'loop_wakes_per_s' and state == 'shown':
        return 'loop_wakes_shown_per_s'
    return metric


def loop_wakes(text):
    """The hub's wake reports in `text` (log lines written during the window, in order) ->
    (wakes per second or None, {cause: count}). The first report is dropped: its period began
    before the window opened. None when no whole period is left."""
    found = []
    for m in WAKE_LINE.finditer(text):
        causes = {}
        for part in (m.group(3) or '').split(','):
            bits = part.split()
            if len(bits) == 2 and bits[1].isdigit():
                causes[bits[0]] = int(bits[1])
        found.append((int(m.group(1)), int(m.group(2)), causes))
    found = found[1:]
    wakes, secs = sum(f[0] for f in found), sum(f[1] for f in found)
    if not found or secs <= 0:
        return None, {}
    total = {}
    for _, _, causes in found:
        for k, v in causes.items():
            total[k] = total.get(k, 0) + v
    return round(wakes / secs, 3), total


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


def _switches(first, last):
    """{tid: switches in the window} from two {tid: cumulative switches} maps. A thread born in the
    window counts from zero, and so does a reused id whose count went down."""
    out = {}
    for tid, cs in last.items():
        before = first.get(tid, 0)
        out[tid] = cs if cs < before else cs - before
    return out


def summarize(first, last, wall_s):
    """Two samples of the same process, `wall_s` seconds apart, -> the metrics. A sample is a dict:
    private_b, working_set_b, cpu_100ns, threads ({thread id: cumulative context switches}), gdi,
    user, vram_b (threads and vram_b are None when Windows has no counter for them).
    Wakeups are the switches of all the process's threads over the window, per second.
    The memory and handle numbers are those at the end of the window."""
    if wall_s <= 0:
        raise ValueError('the window must be longer than zero')
    mb = 1024 * 1024
    vram = last.get('vram_b')
    t0, t1 = first.get('threads'), last.get('threads')
    wakeups = None if t0 is None or t1 is None else round(sum(_switches(t0, t1).values()) / wall_s, 2)
    return {
        'private_mb': round(last['private_b'] / mb, 2),
        'working_set_mb': round(last['working_set_b'] / mb, 2),
        'cpu_pct': round((last['cpu_100ns'] - first['cpu_100ns']) / 1e7 / wall_s * 100, 3),
        'wakeups_per_s': wakeups,
        'gdi_handles': last['gdi'],
        'user_handles': last['user'],
        'vram_mb': None if vram is None else round(vram / mb, 2),
    }


def top_threads(first, last, wall_s, names=None, n=3):
    """The `n` threads with the most context switches per second over the window, highest first:
    [{tid, name, switches_per_s}] (name from `names`, {tid: description}, '' if unnamed). Empty
    without per-thread counts. Kept out of the metrics: it explains a wakeups reading."""
    if first is None or last is None or wall_s <= 0:
        return []
    names = names or {}
    rows = sorted(_switches(first, last).items(), key=lambda r: (-r[1], r[0]))[:n]
    return [{'tid': tid, 'name': names.get(tid, ''), 'switches_per_s': round(cs / wall_s, 2)}
            for tid, cs in rows]


def limit(entry):
    """The most a metric may read: baseline * (1 + pct/100) + slack. A baseline entry is
    {"value": v, "pct": p, "slack": s}; slack (an absolute allowance in the metric's unit) keeps
    a tiny baseline from failing on noise. {"max": m} is a fixed limit instead (the hidden hub's
    1 loop wake/s): the value is then only the expected reading."""
    if 'max' in entry:
        return entry['max']
    return entry['value'] * (1 + entry.get('pct', 0) / 100) + entry.get('slack', 0)


def compare(measured, baseline, state='hidden'):
    """Per metric: (name, measured, limit, status). Status is PASS, FAIL (past the limit), SKIP
    (no baseline for it, or no reading such as VRAM without a counter), MISSING (a baselined
    metric that should have a reading but has none, except the OPTIONAL ones) or INFO (recorded
    only: context switches). `state` picks the window state's loop-wake limit."""
    rows = []
    for name in METRICS:
        got, entry = measured.get(name), baseline.get(baseline_key(name, state))
        if name in INFO:
            rows.append((name, got, None, 'INFO'))
        elif entry is None:
            rows.append((name, got, None, 'SKIP'))
        elif got is None:
            rows.append((name, None, limit(entry), 'SKIP' if name in OPTIONAL else 'MISSING'))
        else:
            cap = limit(entry)
            rows.append((name, got, cap, 'FAIL' if got > cap else 'PASS'))
    return rows


def verdict(rows):
    """FAIL when any metric is past its limit, BLOCKED when a reading is missing, else PASS
    (INFO rows never count)."""
    states = {r[3] for r in rows}
    return 'FAIL' if 'FAIL' in states else 'BLOCKED' if 'MISSING' in states else 'PASS'


def record(batch, results):
    """The per-batch results file's text: {batch, sets: {name: {metrics, rows, verdict}}}."""
    return json.dumps({'batch': batch, 'sets': results}, indent=2, sort_keys=True) + '\n'
