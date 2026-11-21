"""The Python Hideout SDK (sdk/python/hideout_module, #921): the generated models are current, the
sample module replays the module side of the shared transcripts, and the sample itself works."""

import importlib.util
import io
import json
import sys
import threading
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk" / "python"))
sys.path.insert(0, str(ROOT / "examples" / "hideout-module-python"))

import replay  # noqa: E402
from sample_module import Counter  # noqa: E402
from hideout_module.session import INVALID_PARAMS, Handler, Session, SessionClosed, router_pipe_name  # noqa: E402


def _generator():
    spec = importlib.util.spec_from_file_location("gen_hideout_models", ROOT / "tools" / "gen_hideout_models.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Models(unittest.TestCase):
    def test_models_are_current(self):
        gen = _generator()
        checked_in = gen.OUT.read_text(encoding="utf-8").replace("\r\n", "\n")
        self.assertTrue(
            checked_in == gen.render(),
            "sdk/python/hideout_module/models.py is out of date: run python tools/gen_hideout_models.py",
        )


class Transcripts(unittest.TestCase):
    def test_module_side_of_every_transcript(self):
        replayed, failures = replay.replay_all()
        self.assertEqual(failures, [])
        self.assertGreaterEqual(replayed, 25)


def _call(method, params, id_=1):
    return {"jsonrpc": "2.0", "id": id_, "method": method, "params": params}


class SampleModule(unittest.TestCase):
    def test_counter(self):
        out = io.BytesIO()
        s, c = Session(io.BytesIO(), out), Counter()
        s.handle(_call("ui.event", {"panel": "counter", "node": "add", "kind": "click"}), c)
        self.assertEqual(c.count, 1)
        self.assertIsNotNone(c.patch)
        c.counting_down = True
        s.handle(_call("module.drain", {"reason": "update"}), c)
        s.handle(_call("state.export", {}), c)
        s.handle(_call("module.drain", {"reason": "sideways"}), c)
        replies = [json.loads(line) for line in out.getvalue().splitlines()]
        self.assertEqual(replies[1]["result"], {"state": "busy", "reason": "a count-down"})
        self.assertEqual(replies[3]["error"]["code"], INVALID_PARAMS)
        nxt = Counter()
        s.handle(_call("state.import", replies[2]["result"]), nxt)
        self.assertEqual(nxt.count, 1)
        tree = nxt.panels("inst-1").to_json()["panels"][0]["tree"]
        self.assertEqual(tree["children"][0]["props"]["text"], "1")

    def test_denied_subscribe_is_sent_again_on_module_grants(self):
        out = io.BytesIO()
        s, c = Session(io.BytesIO(), out), Counter()
        denied = {"code": 5, "message": "denied"}
        first = s.subscribe("keyboard.game_mode")
        s.subscribe("keyboard.game_mode")
        other = s.subscribe("gpu.*")
        s.handle({"jsonrpc": "2.0", "id": first, "error": denied}, c)
        s.handle({"jsonrpc": "2.0", "id": first + 1, "error": denied}, c)
        # Only a denial is kept; another error is the module's own to handle.
        s.handle({"jsonrpc": "2.0", "id": other, "error": {"code": 4, "message": "timeout"}}, c)
        self.assertEqual(s.denied_subscriptions(), [{"pattern": "keyboard.game_mode"}])
        sent = len(out.getvalue().splitlines())
        s.handle({"jsonrpc": "2.0", "method": "module.grants", "params": {"added": ["x"]}}, c)
        lines = [json.loads(line) for line in out.getvalue().splitlines()][sent:]
        self.assertEqual([(m["method"], m["params"]) for m in lines],
                         [("facts.subscribe", {"pattern": "keyboard.game_mode"})])
        self.assertEqual(s.denied_subscriptions(), [])
        # Allowed now: nothing is kept, and the next module.grants sends nothing.
        s.handle({"jsonrpc": "2.0", "id": lines[0]["id"], "result": {"subscription": 1}}, c)
        sent = len(out.getvalue().splitlines())
        s.handle({"jsonrpc": "2.0", "method": "module.grants", "params": {"added": ["y"]}}, c)
        self.assertEqual(len(out.getvalue().splitlines()), sent)

    def test_subscription_the_hub_ended_waits_for_the_next_allow(self):
        out = io.BytesIO()
        s, c = Session(io.BytesIO(), out), Counter()
        kept = s.subscribe("keyboard.game_mode")
        gone = s.subscribe("gpu.*")
        s.handle({"jsonrpc": "2.0", "id": kept, "result": {"subscription": 3}}, c)
        s.handle({"jsonrpc": "2.0", "id": gone, "result": {"subscription": 4}}, c)
        # The module ended this one itself: it never comes back.
        s.request("facts.unsubscribe", {"subscription": 4})
        sent = len(out.getvalue().splitlines())
        # A turn-off: nothing is sent, the ended subscription waits.
        s.handle({"jsonrpc": "2.0", "method": "module.grants",
                  "params": {"added": [], "ended": [3, 4, 9]}}, c)
        self.assertEqual(len(out.getvalue().splitlines()), sent)
        self.assertEqual(s.denied_subscriptions(), [{"pattern": "keyboard.game_mode"}])
        # The next Allow: asked for again.
        s.handle({"jsonrpc": "2.0", "method": "module.grants", "params": {"added": ["x"]}}, c)
        lines = [json.loads(line) for line in out.getvalue().splitlines()][sent:]
        self.assertEqual([(m["method"], m["params"]) for m in lines],
                         [("facts.subscribe", {"pattern": "keyboard.game_mode"})])

    def test_denied_subscribes_kept_are_capped(self):
        s, c = Session(io.BytesIO(), io.BytesIO()), Counter()
        for n in range(70):
            id_ = s.subscribe("a.%d" % n)
            s.handle({"jsonrpc": "2.0", "id": id_, "error": {"code": 5, "message": "denied"}}, c)
        kept = s.denied_subscriptions()
        self.assertEqual(len(kept), 64)
        self.assertEqual(kept[0], {"pattern": "a.6"})
        self.assertEqual(kept[-1], {"pattern": "a.69"})
        self.assertTrue(s.forget_denied("a.69"))
        self.assertFalse(s.forget_denied("a.69"))
        self.assertEqual(s.denied_subscriptions()[-1], {"pattern": "a.68"})

    def test_call_handles_hub_calls_meanwhile(self):
        hub = b'{"jsonrpc":"2.0","id":7,"method":"module.drain","params":{"reason":"reload"}}\n' \
              b'{"jsonrpc":"2.0","id":1,"result":{"rejected":[]}}\n'
        out = io.BytesIO()
        s = Session(io.BytesIO(hub), out)
        self.assertEqual(s.call("module.register", Counter.registration(), Counter()), {"rejected": []})
        self.assertEqual(json.loads(out.getvalue().splitlines()[1])["result"], {"state": "done"})
        self.assertEqual(s.pending, {})

    def test_pipe_name(self):
        self.assertEqual(router_pipe_name("Ana \u00d6-x_1"), r"\\.\pipe\hideout-Ana-x_1")


@unittest.skipUnless(sys.platform == "win32", "named pipes are Windows only")
class RouterPipe(unittest.TestCase):
    """A read blocked in one thread must not delay a write from another (as Rust SDK #1338): a
    synchronous pipe handle serialises I/O per file object, which stalled `facts.set` behind the
    reader and got the module dropped. Real named pipe, no windows."""

    def test_blocked_read_does_not_block_write(self):
        import _winapi
        import os
        import threading

        from hideout_module.session import open_router_pipe

        name = rf"\\.\pipe\hideout-test-{os.getpid()}-{id(self)}"
        server = _winapi.CreateNamedPipe(name, 0x3, 0, 1, 65536, 65536, 0, 0)  # duplex, byte, wait
        self.addCleanup(_winapi.CloseHandle, server)
        pipe = open_router_pipe(name)
        s = Session(pipe, pipe)
        self.addCleanup(pipe.close)

        got = []
        reader = threading.Thread(target=lambda: got.append(s.read()), daemon=True)
        reader.start()  # blocks in ReadFile: the hub has sent nothing yet
        reader.join(0.3)
        self.assertTrue(reader.is_alive())

        wrote = threading.Thread(target=lambda: s.notify("module.ready", {}), daemon=True)
        wrote.start()
        wrote.join(5)
        self.assertFalse(wrote.is_alive(), "a write stalled behind a blocked read")

        data = b""
        while not data.endswith(b"\n"):
            data += _winapi.ReadFile(server, 4096)[0]
        self.assertEqual(json.loads(data)["method"], "module.ready")

        _winapi.WriteFile(server, b'{"jsonrpc":"2.0","method":"x","params":{}}\n')
        reader.join(5)
        self.assertEqual(got, [{"jsonrpc": "2.0", "method": "x", "params": {}}])


class _Wire:
    """An in-memory transport: the module's writes are parsed and queued for the fake hub; the hub
    feeds lines back through `send`; `readline` blocks until one is there or `close`."""

    def __init__(self):
        self.cv = threading.Condition()
        self.inbox, self.sent, self.closed = [], [], False
        self._partial = b""

    def write(self, data):
        with self.cv:
            self._partial += data
            while b"\n" in self._partial:
                line, self._partial = self._partial.split(b"\n", 1)
                self.sent.append(json.loads(line))
            self.cv.notify_all()

    def send(self, msg):
        with self.cv:
            self.inbox.append(json.dumps(msg).encode() + b"\n")
            self.cv.notify_all()

    def close(self):
        with self.cv:
            self.closed = True
            self.cv.notify_all()

    def readline(self, limit=-1):
        with self.cv:
            while not self.inbox:
                if self.closed:
                    return b""
                self.cv.wait()
            return self.inbox.pop(0)


class Threads(unittest.TestCase):
    def test_many_threads_get_their_own_reply_out_of_order(self):
        wire = _Wire()
        s = Session(wire, wire)
        n = 40
        results, errors = {}, []

        def worker(k):
            try:
                results[k] = s.call("hub.notify", {"k": k}, Handler())
            except BaseException as e:  # noqa: BLE001
                errors.append(e)

        threads = [threading.Thread(target=worker, args=(k,), daemon=True) for k in range(n)]
        for t in threads:
            t.start()
        # The fake hub: wait for every request, then answer in reverse order of arrival.
        with wire.cv:
            self.assertTrue(wire.cv.wait_for(lambda: len(wire.sent) == n, 10))
            reqs = list(wire.sent)
        ids = [r["id"] for r in reqs]
        self.assertEqual(len(set(ids)), n, "ids must be unique")
        for r in reversed(reqs):
            wire.send({"jsonrpc": "2.0", "id": r["id"], "result": {"echo": r["params"]["k"]}})
        for t in threads:
            t.join(10)
            self.assertFalse(t.is_alive(), "a caller never got its reply (deadlock)")
        self.assertEqual(errors, [])
        self.assertEqual(results, {k: {"echo": k} for k in range(n)})
        self.assertEqual(s.pending, {})

    def test_request_from_threads_allocates_distinct_ids(self):
        wire = _Wire()
        s = Session(wire, wire)
        ids, lock = [], threading.Lock()

        def worker():
            for _ in range(50):
                i = s.request("facts.set", {})
                with lock:
                    ids.append(i)

        threads = [threading.Thread(target=worker, daemon=True) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(10)
        self.assertEqual(sorted(ids), list(range(1, 401)))
        self.assertEqual(len(s.pending), 400)
        self.assertEqual(len(wire.sent), 400)

    def test_close_wakes_every_waiter(self):
        wire = _Wire()
        s = Session(wire, wire)
        outcomes = []

        def worker():
            try:
                s.call("hub.notify", {}, Handler())
            except SessionClosed:
                outcomes.append("closed")

        threads = [threading.Thread(target=worker, daemon=True) for _ in range(5)]
        for t in threads:
            t.start()
        with wire.cv:
            self.assertTrue(wire.cv.wait_for(lambda: len(wire.sent) == 5, 10))
        wire.close()
        for t in threads:
            t.join(10)
        self.assertEqual(outcomes, ["closed"] * 5)


if __name__ == "__main__":
    unittest.main()
