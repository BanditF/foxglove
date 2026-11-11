"""labkit's restart parity (#1053) and scripts/lab/restart_parity.py, without an app or a keyboard:
the feed's messages folded into art facts, the establish / read / restart / read-again flow, a
restart that swaps the restore guard (one guard per test app), the shared feed client against a
local fake feed, and the case refusing to run without On-call's OK.

    python -m unittest scripts/test_restart_parity.py
"""
import json
import os
import socket
import subprocess
import sys
import threading
import unittest
import unittest.mock
from pathlib import Path

LAB = Path(__file__).resolve().parent / 'lab'
os.environ.setdefault('APPDATA', str(Path.home()))  # labkit reads it at import (CI is Linux)
sys.path.insert(0, str(LAB))
try:
    import labkit
finally:
    sys.path.remove(str(LAB))

PUSHED = {'t': 'art', 'art': {'name': 'ink-fixture', 'from': 'file', 'nva': 'AAAA'}}
UNKNOWN = {'t': 'art', 'art': {'name': 'pushed set', 'from': 'file', 'nva': None, 'missing': 'pushed_unknown'}}
# The feed's messages as a lab client gets them when it joins a restarted app before the pushed set
# is named again (#1075). Exactly what the app sends: the Rust test
# `live_wire::tests::the_restart_fixture_is_the_wire` builds each line with `Message::json`.
RESTART = LAB / 'fixtures' / 'feed-pushed-set-restart.jsonl'


class FeedArtFacts(unittest.TestCase):
    def test_the_hello_then_the_art_make_the_facts(self):
        msgs = [{'t': 'hello', 'state': {'art': 'ink-fixture'}}, {'t': 'keys', 'e': []}, PUSHED]
        f = labkit.feed_art_facts(iter(msgs), lambda f: False)
        self.assertEqual(f, {'state_art': 'ink-fixture',
                             'art': {'from': 'file', 'name': 'ink-fixture', 'frames': True, 'missing': None}})

    def test_it_stops_at_the_first_accepted_facts(self):
        seen = []

        def msgs():
            for m in [{'t': 'hello', 'state': {'art': None}}, UNKNOWN, PUSHED, {'t': 'state', 'art': None}]:
                seen.append(m)
                yield m
        f = labkit.feed_art_facts(msgs(), lambda f: bool(f['art'] and f['art']['frames']))
        self.assertTrue(f['art']['frames'])
        self.assertEqual(len(seen), 3, 'no message read past the accepted one')

    def test_a_state_message_is_flat_on_the_wire(self):
        # #1075: only the hello nests the state; a `state` message carries its fields itself.
        # Reading `m['state']` there made every state after the hello `state_art: None`.
        msgs = [{'t': 'hello', 'state': {'art': None}}, {'t': 'state', 'art': 'ink-fixture', 'layer': None}]
        f = labkit.feed_art_facts(iter(msgs), lambda f: False)
        self.assertEqual(f['state_art'], 'ink-fixture')

    def test_a_restart_before_the_restore_reaches_the_pushed_facts(self):
        # The lab's restart_parity pushed_set case, on the real wire sequence: hello before the
        # restore (art null, pushed_unknown), then the restore's state and art, then a battery state.
        msgs = [json.loads(l) for l in RESTART.read_text(encoding='utf-8').splitlines() if l.strip()]
        want = {'state_art': 'ink-fixture',
                'art': {'from': 'file', 'name': 'ink-fixture', 'frames': True, 'missing': None}}
        self.assertEqual(labkit.feed_art_facts(iter(msgs), lambda f: f == want), want, 'accepted on the way')
        self.assertEqual(labkit.feed_art_facts(iter(msgs), lambda f: False), want, 'still so after a later state')

    def test_art_null_and_no_messages(self):
        f = labkit.feed_art_facts(iter([{'t': 'art', 'art': None}]), lambda f: False)
        self.assertEqual(f, {'state_art': None, 'art': None})
        self.assertIsNone(labkit.feed_art_facts(iter([]), lambda f: True))


class FakeApp:
    def __init__(self):
        self.restarts = 0

    def restart(self):
        self.restarts += 1


class Flow(unittest.TestCase):
    def run_case(self, before, after):
        app, check, calls = FakeApp(), labkit.Checks(), []

        def read(want):
            calls.append(('read', want, app.restarts))
            return before if app.restarts == 0 else after
        with unittest.mock.patch('builtins.print'):
            got = labkit.restart_parity(app, check, 'x', lambda: calls.append(('establish', app.restarts)), read)
        return got, check.results, calls, app

    def test_the_same_facts_after_a_restart_pass(self):
        got, results, calls, app = self.run_case({'a': 1}, {'a': 1})
        self.assertEqual(results, ['PASS', 'PASS'])
        self.assertEqual(got, ({'a': 1}, {'a': 1}))
        # Established and read on the first app; read again for the same facts on the fresh one.
        self.assertEqual(calls, [('establish', 0), ('read', None, 0), ('read', {'a': 1}, 1)])
        self.assertEqual(app.restarts, 1)

    def test_a_fact_lost_on_restart_fails(self):
        _, results, _, _ = self.run_case({'art': 'fury'}, None)
        self.assertEqual(results, ['PASS', 'FAIL'])

    def test_nothing_published_fails_without_a_restart(self):
        _, results, _, app = self.run_case(None, None)
        self.assertEqual(results, ['FAIL'])
        self.assertEqual(app.restarts, 0)


class Restart(unittest.TestCase):
    def test_a_restart_stops_the_old_guard_and_starts_one_for_the_new_app(self):
        app = labkit.App(sys.executable, Path('.'))
        app.previous = [r'C:\Program Files\Foxglove\foxglove.exe']
        procs = []

        def popen(args, **kw):
            p = unittest.mock.MagicMock()
            p.pid = 100 + len(procs)
            p.poll.return_value = None
            p.args = args
            procs.append(p)
            return p
        with unittest.mock.patch.object(subprocess, 'Popen', popen), \
                unittest.mock.patch.object(labkit.time, 'sleep'):
            app._launch()
            first_app, first_guard = procs
            app.restart()
        first_app.terminate.assert_called_once()
        first_guard.terminate.assert_called_once()
        new_app, new_guard = procs[2:]
        self.assertIs(app.proc, new_app)
        self.assertIs(app.guard, new_guard)
        # The new guard watches the new app and still restores the user's app.
        self.assertEqual(new_guard.args[-2:], [str(new_app.pid), app.previous[0]])
        self.assertEqual(app.previous, [r'C:\Program Files\Foxglove\foxglove.exe'], 'not restored in between')


def frame(text):
    data = text.encode()
    n = len(data)
    head = bytes([0x81, n]) if n < 126 else bytes([0x81, 126]) + n.to_bytes(2, 'big')
    return head + data


class FeedClient(unittest.TestCase):
    def test_it_reads_the_feeds_pushed_messages(self):
        server = socket.socket()
        server.bind(('127.0.0.1', 0))
        server.listen(1)
        port = server.getsockname()[1]
        long_art = {'t': 'art', 'art': {'name': 'big', 'from': 'file', 'nva': 'A' * 300}}

        def serve():
            c, _ = server.accept()
            req = b''
            while b'\r\n\r\n' not in req:
                req += c.recv(4096)
            assert b'token=tok' in req
            c.sendall(b'HTTP/1.1 101 Switching Protocols\r\n\r\n' + frame(json.dumps({'t': 'hello', 'state': {}}))
                      + frame(json.dumps(long_art)))
            c.close()
        t = threading.Thread(target=serve, daemon=True)
        t.start()
        got = list(labkit.feed_messages(port, 'tok', 10))
        t.join(5)
        server.close()
        self.assertEqual(got, [{'t': 'hello', 'state': {}}, long_art])

    def test_a_feed_that_never_listens_ends_with_the_app(self):
        s = socket.socket()
        s.bind(('127.0.0.1', 0))
        port = s.getsockname()[1]
        s.close()  # nothing listens there
        self.assertEqual(list(labkit.feed_messages(port, 'tok', 30, alive=lambda: False)), [])


class Gating(unittest.TestCase):
    def test_the_case_refuses_without_the_ok(self):
        env = {k: v for k, v in os.environ.items() if k != 'FOXGLOVE_APP_OK'}
        env.setdefault('USERNAME', 'test')
        r = subprocess.run([sys.executable, str(LAB / 'restart_parity.py'), '--app', sys.executable],
                           capture_output=True, text=True, env=env, cwd=LAB, timeout=60)
        self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
        self.assertIn('FOXGLOVE_APP_OK=1', r.stdout)


if __name__ == '__main__':
    unittest.main()
