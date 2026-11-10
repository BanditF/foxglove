"""Hideout hub idle footprint (#935, H2.10): hideout.exe idles on a temp HIDEOUT_CONFIG_DIR, once
with no modules and once per named set in tests/hil/hideout-footprint-sets.json (today only
"default", empty), and the case records its

  private and working-set RAM, CPU % (of one core), event-loop wakes per second (the hub's own),
  GDI and USER handles, and dedicated VRAM where Windows has a counter for it.

Loop wakes (#1000) are the gate's wake limit: at most 1/s with the window hidden to the tray and
no flyout (--state hidden, the default); --state shown keeps the window up and has its own limit.
They come from the hub's own counter, its `event loop: N wakes in S s (repaint a, timer b, ...)`
log line: the case starts the hub with HIDEOUT_WAKE_REPORT_S=15 so it reports every 15 s instead
of every 10 min, and reads the lines written during the window from logs/hideout.log under its
HIDEOUT_CONFIG_DIR (the first one is dropped: its period began before the window). Why the log:
it needs no admin, no request channel into the hub and no wake beyond the report itself (one per
15 s, 0.07/s, inside the reading), and On-call can read the same line on any hub. The causes
(repaint, timer, window, accesskit, device, poll, other) go to the result as loop_wake_causes.

Context switches (wakeups_per_s: the Thread-class "Context Switches/sec" summed over all the
process's threads) and top_threads (the three threads that switch most: id, description,
switches/s) are recorded as INFO only, never compared or failed: most are OS and driver work on
the hub's threads (the GL driver, TSF/IME, COM) that never return from the loop's wait.

Hidden: after --settle-s (30) the case posts WM_CLOSE to the hub's own visible windows (found by
its process id), which closes the main window to the tray as its X does, and waits 5 s. Then the
hub is sampled, left alone for --window-s (60), and sampled again; CPU and loop wakes are over the
window, the rest are read at its end. The numbers go to
lab-reports/hideout-footprint/<batch>.json (batch = --batch, default a UTC timestamp) and are
compared with tests/hil/hideout-footprint-baseline.json: a metric past value * (1 + pct/100) + slack
fails the case. --update-baseline rewrites the baseline's values from this run (the pct and slack
stay) for review in a PR. The compare is scripts/lab/footprint_compare.py, tested window-free.

Bounded: every wait is one sleep; a run is about (settle + window + 10) s per set. Only the hub it
started is stopped, by its own process handle; Foxglove and the user's Hideout are not touched
(HIDEOUT_CONFIG_DIR gives the test hub its own single-instance names). It launches hideout.exe, so
it runs only with FOXGLOVE_APP_OK=1 (On-call), never on the user's desktop otherwise.

    FOXGLOVE_APP_OK=1 python scripts/lab/hideout_footprint.py --app <hideout.exe>   (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, ctypes, json, os, shutil, subprocess, sys, tempfile, time
from ctypes import wintypes
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import footprint_compare as fc
from labkit import tool_arg, Checks, REPO, require_app_ok, ps

SETS = REPO / 'tests' / 'hil' / 'hideout-footprint-sets.json'
BASELINE = REPO / 'tests' / 'hil' / 'hideout-footprint-baseline.json'
REPORT_S = 15  # HIDEOUT_WAKE_REPORT_S for the hub under test
HIDE_SETTLE_S = 5


class MemCounters(ctypes.Structure):  # PROCESS_MEMORY_COUNTERS_EX
    _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD),
                ('PeakWorkingSetSize', ctypes.c_size_t), ('WorkingSetSize', ctypes.c_size_t),
                ('QuotaPeakPagedPoolUsage', ctypes.c_size_t), ('QuotaPagedPoolUsage', ctypes.c_size_t),
                ('QuotaPeakNonPagedPoolUsage', ctypes.c_size_t), ('QuotaNonPagedPoolUsage', ctypes.c_size_t),
                ('PagefileUsage', ctypes.c_size_t), ('PeakPagefileUsage', ctypes.c_size_t),
                ('PrivateUsage', ctypes.c_size_t)]


def sample(pid):
    """One reading of process `pid`: memory, CPU time, GUI handles through Win32; context switches
    and VRAM through the performance counters (PowerShell CIM; VRAM None without the GPU counter)."""
    k, u = ctypes.windll.kernel32, ctypes.windll.user32
    k.OpenProcess.restype = wintypes.HANDLE
    h = k.OpenProcess(0x1000 | 0x0400, False, pid)  # QUERY_LIMITED_INFORMATION | QUERY_INFORMATION
    if not h:
        raise RuntimeError(f'cannot open process {pid}')
    try:
        mem = MemCounters()
        mem.cb = ctypes.sizeof(mem)
        if not ctypes.windll.psapi.GetProcessMemoryInfo(h, ctypes.byref(mem), mem.cb):
            raise RuntimeError('GetProcessMemoryInfo failed')
        t = [wintypes.FILETIME() for _ in range(4)]
        if not k.GetProcessTimes(h, *[ctypes.byref(x) for x in t]):
            raise RuntimeError('GetProcessTimes failed')
        ft = lambda f: (f.dwHighDateTime << 32) | f.dwLowDateTime
        gdi, user = u.GetGuiResources(h, 0), u.GetGuiResources(h, 1)
    finally:
        k.CloseHandle(h)
    # Cumulative context switches per thread (raw perf data, so two samples make a rate) and the
    # process's dedicated GPU memory; the GPU class is absent on a machine without WDDM counters.
    out = ps(
        f'$t = @(Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "IDProcess={pid}" '
        f'-ErrorAction SilentlyContinue | ForEach-Object {{ @{{tid = [int]$_.IDThread; cs = [int64]$_.ContextSwitchesPersec}} }}); '
        f'$v = Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction SilentlyContinue | '
        f'Where-Object {{ $_.Name -like "pid_{pid}_*" }} | Measure-Object -Property DedicatedUsage -Sum; '
        f'@{{threads = $t; vram = $(if ($v.Count) {{ $v.Sum }} else {{ $null }})}} | ConvertTo-Json -Compress -Depth 3')
    try:
        extra = json.loads(out)
    except json.JSONDecodeError:
        extra = {}
    if not isinstance(extra, dict):
        extra = {}
    rows = extra.get('threads') or []
    if isinstance(rows, dict):  # one thread: ConvertTo-Json drops the array
        rows = [rows]
    # None: no thread counters here; wakeups are then unavailable.
    threads = {int(r['tid']): int(r['cs']) for r in rows if isinstance(r, dict) and 'tid' in r} or None
    return {'private_b': mem.PrivateUsage, 'working_set_b': mem.WorkingSetSize,
            'cpu_100ns': ft(t[2]) + ft(t[3]), 'threads': threads,
            'gdi': gdi, 'user': user, 'vram_b': extra.get('vram')}


def thread_names(tids):
    """{tid: description} from GetThreadDescription; '' for unnamed or gone threads."""
    k = ctypes.windll.kernel32
    k.OpenThread.restype = wintypes.HANDLE
    k.GetThreadDescription.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_void_p)]
    k.LocalFree.argtypes = [ctypes.c_void_p]
    names = {}
    for tid in tids:
        name = ''
        h = k.OpenThread(0x0800, False, tid)  # THREAD_QUERY_LIMITED_INFORMATION
        if h:
            p = ctypes.c_void_p()
            if k.GetThreadDescription(h, ctypes.byref(p)) >= 0 and p.value:
                name = ctypes.wstring_at(p.value)
                k.LocalFree(p)
            k.CloseHandle(h)
        names[tid] = name
    return names


def own_windows(pid):
    """The visible top-level windows of process `pid` (one EnumWindows pass)."""
    u = ctypes.windll.user32
    found = []
    proc_t = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

    def each(hwnd, _):
        owner = wintypes.DWORD()
        u.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and u.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    u.EnumWindows(proc_t(each), 0)
    return found


def hide_to_tray(pid):
    """Closes the hub's main window to the tray as its X does (WM_CLOSE: the hub cancels the close
    and hides). Raises RuntimeError when it has no window to close or one is still up after."""
    u = ctypes.windll.user32
    wins = own_windows(pid)
    if not wins:
        raise RuntimeError('the hub has no visible window to close to the tray')
    for h in wins:
        u.PostMessageW(h, 0x0010, 0, 0)  # WM_CLOSE
    time.sleep(HIDE_SETTLE_S)  # one bounded wait: the close and its last frame
    if any(u.IsWindowVisible(h) for h in wins):
        raise RuntimeError('the hub window is still shown after WM_CLOSE')


def log_size(path):
    try:
        return path.stat().st_size
    except OSError:
        return 0


def log_since(path, offset):
    try:
        with open(path, 'rb') as f:
            f.seek(offset)
            return f.read().decode('utf-8', 'replace')
    except OSError:
        return ''


def measure(exe, folders, settle_s, window_s, state='hidden'):
    """Runs the hub on a temp HIDEOUT_CONFIG_DIR with `folders` installed as modules, hidden to the
    tray or shown (`state`); returns (metrics, top threads, loop-wake causes, config folder).
    Raises RuntimeError if the hub is gone before the end of the window or won't hide."""
    cfg = Path(tempfile.mkdtemp(prefix='hideout-footprint-')) / 'config'
    cfg.mkdir()
    for f in folders:
        src = Path(f) if Path(f).is_absolute() else REPO / f
        shutil.copytree(src, cfg / 'modules' / src.name)
    log = cfg / 'logs' / 'hideout.log'
    env = {**os.environ, 'HIDEOUT_CONFIG_DIR': str(cfg), 'HIDEOUT_WAKE_REPORT_S': str(REPORT_S)}
    proc = subprocess.Popen([str(exe)], env=env)
    try:
        time.sleep(settle_s)  # one bounded wait: first-run work finishes, nothing is polled
        if state == 'hidden':
            hide_to_tray(proc.pid)
        offset = log_size(log)
        first = sample(proc.pid)
        t0 = time.monotonic()
        time.sleep(window_s)
        last = sample(proc.pid)
        if proc.poll() is not None:
            raise RuntimeError(f'the hub exited ({proc.returncode}) during the window')
        wall = time.monotonic() - t0
        top = fc.top_threads(first['threads'], last['threads'], wall, thread_names(last['threads'] or {}))
        metrics = fc.summarize(first, last, wall)
        metrics['loop_wakes_per_s'], causes = fc.loop_wakes(log_since(log, offset))
        return metrics, top, causes, cfg
    finally:
        proc.terminate()  # by its own handle, never by name
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()


def update_baseline(baseline, results, state='hidden'):
    """The baseline with each measured value in place (pct, slack and max kept; the loop-wake
    reading goes to `state`'s entry); unmeasured and INFO metrics stay."""
    for name, res in results.items():
        entries = baseline.setdefault('sets', {}).setdefault(name, {})
        for metric, got in res['metrics'].items():
            if got is not None and metric not in fc.INFO:
                key = fc.baseline_key(metric, state)
                entries.setdefault(key, {'pct': 20, 'slack': 0})['value'] = got
    return baseline


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the hideout.exe under test')
    ap.add_argument('--batch', default=time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()),
                    help='the batch id the results are stored under')
    ap.add_argument('--settle-s', type=float, default=30)
    ap.add_argument('--window-s', type=float, default=60)
    ap.add_argument('--state', choices=fc.STATES, default='hidden',
                    help='hidden: the window closed to the tray, no flyout (the 1 wake/s gate); shown: left up')
    ap.add_argument('--update-baseline', action='store_true', help="write this run's values into the baseline file")
    args = ap.parse_args()
    require_app_ok()
    sets = fc.load_sets(SETS.read_text(encoding='utf-8'))
    baseline = json.loads(BASELINE.read_text(encoding='utf-8'))
    check, results = Checks(), {}
    for name, folders in sets.items():
        try:
            metrics, top, causes, cfg = measure(args.app, folders, args.settle_s, args.window_s, args.state)
        except (RuntimeError, OSError) as e:
            check(f'{name}: measured', False, e, blocked=True)
            continue
        rows = fc.compare(metrics, baseline.get('sets', {}).get(name, {}), args.state)
        results[name] = {'metrics': metrics, 'verdict': fc.verdict(rows), 'modules': folders,
                         'state': args.state, 'top_threads': top, 'loop_wake_causes': causes,
                         'rows': [{'metric': m, 'measured': g, 'limit': c, 'status': s} for m, g, c, s in rows]}
        for m, g, c, s in rows:
            detail = f'{g} (info only)' if s == 'INFO' else f'{g} (limit {c})'
            check(f'{name} ({args.state}): {m}', s in ('PASS', 'SKIP', 'INFO'), detail, blocked=s == 'MISSING')
    out = REPO / 'lab-reports' / 'hideout-footprint'
    out.mkdir(parents=True, exist_ok=True)
    (out / f'{args.batch}.json').write_text(fc.record(args.batch, results), encoding='utf-8')
    print(f'results: {out / (args.batch + ".json")}', flush=True)
    if args.update_baseline and results:
        BASELINE.write_text(json.dumps(update_baseline(baseline, results, args.state), indent=2) + '\n', encoding='utf-8')
        print(f'baseline rewritten: {BASELINE}', flush=True)
    check.finish()


if __name__ == '__main__':
    main()
