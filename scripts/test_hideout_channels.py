"""The Python SDK's direct channels (sdk/python/hideout_module/channel.py, #977): the shared
channel and panel-frame vectors (the Rust contract crate passes the same files), and on Windows
real one-time pipes: the nonce, one consumer only, the accept timeout and the current-user ACL.
Window-free; no hub, no router."""

import json
import os
import sys
import threading
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from hideout_module import channel  # noqa: E402

VECTORS = ROOT / "tests" / "hideout-vectors"


def _load(rel):
    return json.loads((VECTORS / rel).read_text(encoding="utf-8"))


class WireVectors(unittest.TestCase):
    def test_index_lists_the_channel_vectors(self):
        index = _load("index.json")["vectors"]
        self.assertEqual(index["channels/channel-wire.json"]["result"], "table")

    def test_streams(self):
        doc = _load("channels/channel-wire.json")
        self.assertGreaterEqual(len(doc["streams"]), 10)
        for row in doc["streams"]:
            with self.subTest(row["name"]):
                frames, end = channel.read_stream(bytes.fromhex(row["hex"]), doc["nonce"])
                got = {"frames_hex": [f.hex() for f in frames], "end": end}
                self.assertEqual(got, row["expect"])

    def test_streams_reencode(self):
        """Encoding the frames a stream carried gives the stream's bytes back (both SDKs frame
        alike)."""
        doc = _load("channels/channel-wire.json")
        for row in doc["streams"]:
            if row["expect"]["end"] != "closed":
                continue
            with self.subTest(row["name"]):
                data = channel.encode_frame(doc["nonce"].encode()) + b"".join(
                    channel.encode_frame(bytes.fromhex(f)) for f in row["expect"]["frames_hex"])
                self.assertEqual(data.hex(), row["hex"])

    def test_pipes(self):
        doc = _load("channels/channel-wire.json")
        for row in doc["pipes"]:
            with self.subTest(row["pipe"]):
                got = channel.is_channel_pipe_in(row["pipe"], doc["session"])
                self.assertEqual(got, row["result"] == "valid")

    def test_panel_frames(self):
        for row in _load("frames/panel-frames.json")["frames"]:
            with self.subTest(row["name"]):
                exp = row["expect"]
                try:
                    f = channel.decode_panel_wire(bytes.fromhex(row["wire_hex"]))
                except channel.Dropped:
                    self.assertFalse(exp["accept"])
                    continue
                if isinstance(f, channel.Image):
                    got = {"accept": True, "kind": "image", "width": f.width, "height": f.height,
                           "seq": f.seq, "pixels_rgba": f.pixels}
                elif isinstance(f, channel.Canvas):
                    got = {"accept": True, "kind": "canvas", "count": len(f.colours), "seq": f.seq,
                           "colours_rgba_by_shape_order": f.colours}
                else:
                    got = {"accept": True, "kind": "text", "seq": f.seq, "append": f.append}
                self.assertEqual({k: got.get(k) for k in exp if k != "reason"},
                                 {k: v for k, v in exp.items() if k != "reason"})
                self.assertEqual(channel.encode_panel(f).hex(), row["wire_hex"], "re-encodes byte for byte")


@unittest.skipUnless(os.name == "nt", "named pipes are Windows")
class Pipes(unittest.TestCase):
    def test_stream_one_consumer_only(self):
        sent = threading.Event()

        def stream(conn):
            for seq in range(200):
                conn.write(channel.encode_panel(channel.Canvas(seq, [[seq % 256, 1, 2, 255]] * 16)))
            sent.set()

        pipe = channel.serve("foxglove.example-art", "previews", "n-py", stream)
        self.assertTrue(channel.is_channel_pipe(pipe), pipe)
        with channel.connect(pipe, "n-py") as conn:
            with self.assertRaises(OSError):
                channel.connect(pipe, "n-py")  # the one instance is taken
            seqs = []
            while (p := conn.read_frame()) is not None:
                seqs.append(channel.decode_panel_payload(p).seq)
        self.assertEqual(seqs, list(range(200)))
        self.assertTrue(sent.wait(10))
        with self.assertRaises(OSError):
            channel.connect(pipe, "n-py")  # spent

    def test_a_pending_read_ends_cleanly_when_the_provider_closes(self):
        # #1006: the consumer waits inside a read while the provider is still busy; the
        # provider's close ends the stream (None), never a BrokenPipeError.
        got = threading.Event()

        def stream(conn):
            conn.write_frame(b"last")
            got.wait(10)
            time.sleep(0.2)  # the consumer's next ReadFile is pending by now; then the close

        pipe = channel.serve("foxglove.example-art", "previews", "n-late", stream)
        with channel.connect(pipe, "n-late") as conn:
            self.assertEqual(conn.read_frame(), b"last")
            got.set()
            self.assertIsNone(conn.read_frame())

    def test_wrong_nonce_spends_the_channel(self):
        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "nonce"))
        result = {}

        def provider():
            try:
                p.accept_nonce("good", 10)
            except channel.ChannelError as e:
                result["err"] = e

        t = threading.Thread(target=provider)
        t.start()
        conn = channel.connect(p.name, "bad")
        t.join(10)
        self.assertFalse(result["err"].timed_out)
        self.assertIsNone(conn.read_frame(), "the provider hung up")
        conn.close()

    def test_late_consumer_times_out(self):
        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "late"))
        name = p.name
        with self.assertRaises(channel.ChannelError) as e:
            p.accept_nonce("n", 0.05)
        self.assertTrue(e.exception.timed_out)
        with self.assertRaises(OSError):
            channel.connect(name, "n")

    def test_consumer_refuses_other_pipes(self):
        for bad in ("\\\\host\\pipe\\hideout-ch-x", "\\\\.\\pipe\\hideout-someone"):
            with self.assertRaises(ValueError):
                channel.connect(bad, "n")

    def test_current_user_only_acl(self):
        import ctypes
        from ctypes import wintypes

        adv = ctypes.WinDLL("advapi32", use_last_error=True)
        k32 = ctypes.WinDLL("kernel32", use_last_error=True)
        adv.GetSecurityInfo.argtypes = [ctypes.c_void_p, ctypes.c_int, wintypes.DWORD, ctypes.c_void_p,
                                        ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p,
                                        ctypes.POINTER(ctypes.c_void_p)]
        adv.GetAce.argtypes = [ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p)]
        adv.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
        k32.LocalFree.argtypes = [ctypes.c_void_p]

        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "acl"))
        try:
            dacl, sd = ctypes.c_void_p(), ctypes.c_void_p()
            # SE_KERNEL_OBJECT, DACL_SECURITY_INFORMATION
            rc = adv.GetSecurityInfo(p.handle, 6, 4, None, None, ctypes.byref(dacl), None, ctypes.byref(sd))
            self.assertEqual(rc, 0)
            self.assertTrue(dacl.value, "a null DACL would grant everyone")
            ace_count = ctypes.c_uint16.from_address(dacl.value + 4).value
            self.assertEqual(ace_count, 1, "one ACE only")
            ace = ctypes.c_void_p()
            self.assertTrue(adv.GetAce(dacl, 0, ctypes.byref(ace)))
            self.assertEqual(ctypes.c_uint8.from_address(ace.value).value, 0, "an allow ACE")
            text = ctypes.c_void_p()
            self.assertTrue(adv.ConvertSidToStringSidW(ace.value + 8, ctypes.byref(text)))
            sid = ctypes.wstring_at(text.value)
            k32.LocalFree(text)
            k32.LocalFree(sd)
            self.assertEqual(sid, channel.current_user_sid())
        finally:
            p.close()


if __name__ == "__main__":
    unittest.main()
