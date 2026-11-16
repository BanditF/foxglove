"""Wallpaper Engine command trace (#1120, H4.6): which processes send WE commands?

Window-free. A fake WE runner records every `wallpaper64.exe -control ...` invocation (it never starts the
real exe) and a fake pipe records the hub's `wallpaper-engine.*` calls; each record names its SENDER (the
process that issued it). The analysis counts distinct senders that reach WE. Done-when for #1120: with
the Foxglove app and Arete both running, exactly ONE process sends WE commands (the hub provider).

Trace file: JSON lines {"t": float, "sender": "foxglove"|"arete"|..., "kind": "exe"|"pipe", "args": [...]}.
"kind":"pipe" records are clients asking the provider and do not count as WE commands; the provider's own
"exe" records do.

    python scripts/lab/we_trace.py analyze <trace.jsonl> [--expect-senders 1]   (exit 0 PASS, 1 FAIL)
    python scripts/lab/we_trace.py scenario [--arete-src C:\\src\\arete\\src] [--via-hub]   (mock-only)

`scenario` drives Arete's real claim()/release() with the fake runner and replays the Foxglove sync worker's
command shape (wallpaper_sync.rs run_cli: `-control applyProperties ...`) as a fake sender. Exit 2 if Arete
cannot be imported. Today's Arete still sends pause/play itself (two senders, reported).

`--via-hub` is the #1120 target, on the wire shape Foxglove serves through the hub (slice C, contract 1.15
section 8.5): `wallpaper-engine.pause {reason}` answers `{lease: "<string id>", paused}`, and
`wallpaper-engine.release {lease}` answers `{played, why?}`; the first lease pauses, the last one plays, and a
caller that disconnects holding a lease gets the same play through the hub's `lease.closed`. FakeProvider below
mirrors app/src/foxglove/wallpaper_leases.rs for that.

On-call, after game over, for the real thing: put a shim named wallpaper64.exe first in the WE path both apps
use (configured exe path), which appends {"t","sender": <parent process name>,"kind":"exe","args": [self, *argv]}
to a JSONL file and exits 0 without starting WE; run Foxglove and Arete's claim/release once each, then
`we_trace.py analyze <file>`. PASS means one sender.
"""
import argparse, json, sys, tempfile, threading, time, types
from pathlib import Path

WE_EXES = ('wallpaper64.exe', 'wallpaper32.exe')


class Trace:
    """Thread-safe recorder; optionally mirrors to a JSONL file."""

    def __init__(self, path=None):
        self.records, self.path, self._lock = [], path, threading.Lock()

    def add(self, sender, kind, args):
        rec = {'t': time.time(), 'sender': sender, 'kind': kind, 'args': list(args)}
        with self._lock:
            self.records.append(rec)
            if self.path:
                with open(self.path, 'a', encoding='utf-8') as f:
                    f.write(json.dumps(rec) + '\n')
        return rec


class FakeRunner:
    """Stands in for running wallpaper64.exe: records, never spawns. `as_sender` binds who is calling."""

    def __init__(self, trace):
        self.trace = trace

    def as_sender(self, sender):
        return lambda exe, *args: self.trace.add(sender, 'exe', [str(exe), *args])


class FakePipe:
    """Stands in for the hub pipe: records calls; the provider (if any) turns them into runner commands."""

    def __init__(self, trace, provider=None):
        self.trace, self.provider = trace, provider

    def call(self, sender, method, params=None):
        self.trace.add(sender, 'pipe', [method, params or {}])
        return self.provider(method, params or {}) if self.provider else None


class FakeProvider:
    """Foxglove's lease table behind the hub, over the fake runner (wallpaper_leases.rs, no user pause)."""

    def __init__(self, send):
        self.send, self.live, self.n = send, {}, 0

    def __call__(self, method, params, caller='?'):
        if method == 'wallpaper-engine.pause':
            if not self.live:
                self.send('pause')
            self.n += 1
            lease = 'we-%x-%016x' % (self.n, 0)
            self.live[lease] = caller
            return {'lease': lease, 'paused': True}
        if method == 'wallpaper-engine.release':
            if self.live.get(params.get('lease')) != caller:
                return {'error': {'code': -32602, 'data': {'id': 'lease.unknown'}}}  # the hub refuses it
            return self._end(params['lease'])
        if method == 'wallpaper-engine.status':
            return {'running': True, 'owner': 'foxglove', 'leases': len(self.live), 'paused_by_lease': bool(self.live)}
        return {'error': {'code': -32601}}

    def _end(self, lease):
        self.live.pop(lease, None)
        if self.live:
            return {'played': False, 'why': 'held'}
        self.send('play')
        return {'played': True}

    def disconnect(self, caller):
        """The hub's lease.closed {reason: "disconnect"} for each lease the caller held."""
        for lease in [k for k, v in self.live.items() if v == caller]:
            self._end(lease)


def load(path):
    out = []
    for n, line in enumerate(Path(path).read_text(encoding='utf-8').splitlines(), 1):
        if line.strip():
            try:
                out.append(json.loads(line))
            except ValueError as e:
                raise SystemExit('%s:%d: bad JSON: %s' % (path, n, e))
    return out


def is_we_command(rec):
    if rec.get('kind') != 'exe' or not rec.get('args'):
        return False
    return Path(str(rec['args'][0]).replace('\\', '/')).name.lower() in WE_EXES and '-control' in rec['args']


def senders(records):
    """{sender: [verbs]} of the processes whose commands reached Wallpaper Engine."""
    out = {}
    for r in records:
        if is_we_command(r):
            a, i = r['args'], r['args'].index('-control')
            out.setdefault(r['sender'], []).append(a[i + 1] if len(a) > i + 1 else '')
    return out


def report(records, expect=1):
    s = senders(records)
    lines = ['WE command senders: %d (expected %d)' % (len(s), expect)]
    for name, verbs in sorted(s.items()):
        lines.append('  %-10s %d command(s): %s' % (name, len(verbs), ', '.join(sorted(set(verbs)))))
    piped = sorted({r['sender'] for r in records if r.get('kind') == 'pipe'})
    if piped:
        lines.append('  pipe clients: ' + ', '.join(piped))
    return len(s) == expect, '\n'.join(lines)


def scenario(arete_src=None, via_hub=False):
    """Both apps 'running': Foxglove applies properties, Arete claims then releases the GPU.
    With via_hub=True Arete's claim goes through the fake pipe to a Foxglove provider (the #1120 target)."""
    trace = Trace()
    runner = FakeRunner(trace)
    fox, arete = runner.as_sender('foxglove'), runner.as_sender('arete')
    exe = 'C:/fake/wallpaper_engine/wallpaper64.exe'
    fox(exe, '-control', 'applyProperties', '-location', 'monitor0', '-properties', 'RAW~({})~END')

    if via_hub:
        provider = FakeProvider(lambda verb: fox(exe, '-control', verb))
        pipe = FakePipe(trace, lambda m, p: provider(m, p, 'arete'))
        got = pipe.call('arete', 'wallpaper-engine.pause', {'reason': 'Arete GPU claim'})
        assert isinstance(got['lease'], str), got
        pipe.call('arete', 'wallpaper-engine.release', {'lease': got['lease']})
        # A claim whose process dies holding the lease: the hub closes it, Foxglove plays.
        pipe.call('arete', 'wallpaper-engine.pause', {'reason': 'Arete GPU claim'})
        provider.disconnect('arete')
        trace.provider = provider
        return trace
    if arete_src:
        sys.path.insert(0, str(arete_src))
    from arete import claim as ac  # read-only import of the real Arete code
    with tempfile.TemporaryDirectory() as tmp:
        exe_file = Path(tmp) / 'wallpaper64.exe'
        exe_file.write_bytes(b'')
        store = {}
        saved = (ac._wallpaper, ac.load_state, ac._save_state, ac.os.name)
        ac._wallpaper = lambda path, action: arete(path, '-control', action)
        ac.load_state = lambda cfg: dict(store)
        ac._save_state = lambda cfg, state: (store.clear(), store.update(state))
        ac.os.name = 'nt'  # claim() is Windows-only; everything it touches is faked here
        cfg = types.SimpleNamespace(gpu_claim=True, claim_restart=(), igpu_apps=(), claim_pause_wallpaper=True)
        try:
            ac.claim(cfg, log=lambda m: None, processes=lambda names: {'wallpaper64': [(1, str(exe_file))]})
            ac.release(cfg, log=lambda m: None)
        finally:
            ac._wallpaper, ac.load_state, ac._save_state, ac.os.name = saved
    return trace


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest='cmd', required=True)
    a = sub.add_parser('analyze')
    a.add_argument('trace')
    a.add_argument('--expect-senders', type=int, default=1)
    s = sub.add_parser('scenario')
    s.add_argument('--arete-src', default=r'C:\src\arete\src')
    s.add_argument('--via-hub', action='store_true')
    s.add_argument('--expect-senders', type=int, default=1)
    s.add_argument('--out')
    args = ap.parse_args(argv)
    if args.cmd == 'analyze':
        records = load(args.trace)
    else:
        try:
            trace = scenario(args.arete_src, args.via_hub)
        except ImportError as e:
            print('cannot import Arete (%s); use analyze on a recorded trace' % e)
            return 2
        records = trace.records
        if args.out:
            Path(args.out).write_text(''.join(json.dumps(r) + '\n' for r in records), encoding='utf-8')
    ok, text = report(records, args.expect_senders)
    print(text)
    print('PASS' if ok else 'FAIL')
    return 0 if ok else 1


if __name__ == '__main__':
    sys.exit(main())
