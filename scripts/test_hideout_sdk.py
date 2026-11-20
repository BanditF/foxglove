"""The Python Hideout SDK (sdk/python/hideout_module, #921): the generated models are current, the
sample module replays the module side of the shared transcripts, and the sample itself works."""

import importlib.util
import io
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk" / "python"))
sys.path.insert(0, str(ROOT / "examples" / "hideout-module-python"))

import replay  # noqa: E402
from sample_module import Counter  # noqa: E402
from hideout_module.session import INVALID_PARAMS, Session, router_pipe_name  # noqa: E402


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


if __name__ == "__main__":
    unittest.main()
