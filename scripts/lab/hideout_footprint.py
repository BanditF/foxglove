"""Hideout hub idle footprint (#935, H2.10): hideout.exe idles on a temp HIDEOUT_CONFIG_DIR, once
with no modules and once per named set in tests/hil/hideout-footprint-sets.json (today only
"default", empty), and the case records its

  private and working-set RAM, CPU % (of one core), wakeups (context switches per second),
  GDI and USER handles, and dedicated VRAM where Windows has a counter for it. Wakeups try the process counter, then
  the per-thread "Context Switches/sec" summed for the process; with neither they are recorded as
  unavailable (null) and never compared or failed.

After --settle-s (30) the hub is sampled, left alone for --window-s (60), and sampled again; CPU and
wakeups are the difference over the window, the rest are read at its end. The numbers go to
lab-reports/hideout-footprint/<batch>.json (batch = --batch, default a UTC timestamp) and are
compared with tests/hil/hideout-footprint-baseline.json: a metric past value * (1 + pct/100) + slack
fails the case. --update-baseline rewrites the baseline's values from this run (the pct and slack
stay) for review in a PR. The compare is scripts/lab/footprint_compare.py, tested window-free.

Bounded: every wait is one sleep; a run is about (settle + window + 5) s per set. Only the hub it
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
    # Cumulative context switches (raw perf data, so two samples make a rate) and the process's
    # dedicated GPU memory; the GPU class is absent on a machine without WDDM counters.
    out = ps(
        f'$c = (Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter "IDProcess={pid}" '
        f'-ErrorAction SilentlyContinue | Select-Object -First 1).ContextSwitchesPersec; '
        f'if ($null -eq $c) {{ $t = Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "IDProcess={pid}" '
        f'-ErrorAction SilentlyContinue | Measure-Object -Property ContextSwitchesPersec -Sum; '
        f'if ($t.Count) {{ $c = $t.Sum }} }}; '
        f'$v = Get-CimInstance Win32_PerfRawData_GPUPerformanceCounters_GPUProcessMemory -ErrorAction SilentlyContinue | '
        f'Where-Object {{ $_.Name -like "pid_{pid}_*" }} | Measure-Object -Property DedicatedUsage -Sum; '
        f'@{{ctx = $c; vram = $(if ($v.Count) {{ $v.Sum }} else {{ $null }})}} | ConvertTo-Json -Compress')
    try:
        extra = json.loads(out)
    except json.JSONDecodeError:
        extra = {}
    if not isinstance(extra, dict):
        extra = {}
    ctx = extra.get('ctx')  # None: no process or thread counter here; wakeups are then unavailable
    return {'private_b': mem.PrivateUsage, 'working_set_b': mem.WorkingSetSize,
            'cpu_100ns': ft(t[2]) + ft(t[3]), 'ctx_switches': None if ctx is None else int(ctx),
            'gdi': gdi, 'user': user, 'vram_b': extra.get('vram')}


def measure(exe, folders, settle_s, window_s):
    """Runs the hub on a temp HIDEOUT_CONFIG_DIR with `folders` installed as modules; returns
    (metrics, config folder). Raises RuntimeError if the hub is gone before the end of the window."""
    cfg = Path(tempfile.mkdtemp(prefix='hideout-footprint-')) / 'config'
    cfg.mkdir()
    for f in folders:
        src = Path(f) if Path(f).is_absolute() else REPO / f
        shutil.copytree(src, cfg / 'modules' / src.name)
    proc = subprocess.Popen([str(exe)], env={**os.environ, 'HIDEOUT_CONFIG_DIR': str(cfg)})
    try:
        time.sleep(settle_s)  # one bounded wait: first-run work finishes, nothing is polled
        first = sample(proc.pid)
        t0 = time.monotonic()
        time.sleep(window_s)
        last = sample(proc.pid)
        if proc.poll() is not None:
            raise RuntimeError(f'the hub exited ({proc.returncode}) during the window')
        return fc.summarize(first, last, time.monotonic() - t0), cfg
    finally:
        proc.terminate()  # by its own handle, never by name
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()


def update_baseline(baseline, results):
    """The baseline with each measured value in place (pct and slack kept); unmeasured metrics stay."""
    for name, res in results.items():
        entries = baseline.setdefault('sets', {}).setdefault(name, {})
        for metric, got in res['metrics'].items():
            if got is not None:
                entries.setdefault(metric, {'pct': 20, 'slack': 0})['value'] = got
    return baseline


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--app', required=True, type=tool_arg('--app'), help='the hideout.exe under test')
    ap.add_argument('--batch', default=time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()),
                    help='the batch id the results are stored under')
    ap.add_argument('--settle-s', type=float, default=30)
    ap.add_argument('--window-s', type=float, default=60)
    ap.add_argument('--update-baseline', action='store_true', help="write this run's values into the baseline file")
    args = ap.parse_args()
    require_app_ok()
    sets = fc.load_sets(SETS.read_text(encoding='utf-8'))
    baseline = json.loads(BASELINE.read_text(encoding='utf-8'))
    check, results = Checks(), {}
    for name, folders in sets.items():
        try:
            metrics, cfg = measure(args.app, folders, args.settle_s, args.window_s)
        except (RuntimeError, OSError) as e:
            check(f'{name}: measured', False, e, blocked=True)
            continue
        rows = fc.compare(metrics, baseline.get('sets', {}).get(name, {}))
        results[name] = {'metrics': metrics, 'verdict': fc.verdict(rows), 'modules': folders,
                         'rows': [{'metric': m, 'measured': g, 'limit': c, 'status': s} for m, g, c, s in rows]}
        for m, g, c, s in rows:
            check(f'{name}: {m}', s in ('PASS', 'SKIP'), f'{g} (limit {c})', blocked=s == 'MISSING')
    out = REPO / 'lab-reports' / 'hideout-footprint'
    out.mkdir(parents=True, exist_ok=True)
    (out / f'{args.batch}.json').write_text(fc.record(args.batch, results), encoding='utf-8')
    print(f'results: {out / (args.batch + ".json")}', flush=True)
    if args.update_baseline and results:
        BASELINE.write_text(json.dumps(update_baseline(baseline, results), indent=2) + '\n', encoding='utf-8')
        print(f'baseline rewritten: {BASELINE}', flush=True)
    check.finish()


if __name__ == '__main__':
    main()
