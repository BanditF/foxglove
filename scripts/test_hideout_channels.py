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


SID = "S-1-15-2-2542057647-2638163769-1067566796-1147736380-124582988-875803485-2694164844"
NS = f"Sessions\\1\\AppContainerNamedObjects\\{SID}"


class ContainedRules(unittest.TestCase):
    """#1092, any platform: the ACL, the names in an AppContainer and whom a pipe admits."""

    def test_app_container_sid(self):
        self.assertTrue(channel.is_app_container_sid(SID))
        for bad in ("S-1-5-21-1-2-3-1001", "S-1-15-2-", "S-1-15-2-1)(A;;GA;;;WD", None, "s-1-15-2-1"):
            self.assertFalse(channel.is_app_container_sid(bad), bad)

    def test_pipe_sddl(self):
        me = "S-1-5-21-1-2-3-1001"
        self.assertEqual(channel.pipe_sddl(me), f"D:P(A;;GA;;;{me})", "the user alone, no label")
        self.assertEqual(channel.pipe_sddl(me, SID),
                         f"D:P(A;;GA;;;{me})(A;;0x12019b;;;{SID})S:(ML;;NW;;;LW)",
                         "client rights for the consumer's SID only, and a Low label")
        for bad in ("S-1-15-2-1)(A;;GA;;;WD)", "WD", me):
            with self.assertRaises(ValueError):
                channel.pipe_sddl(me, bad)

    def test_own_namespace_names(self):
        plain = channel.PIPE_PREFIX + "4242-0-example.room-map-map"
        self.assertEqual(channel.own_namespace_names(plain, None), (plain, plain))
        make, full = channel.own_namespace_names(plain, NS)
        self.assertEqual(make, "\\\\.\\pipe\\LOCAL\\hideout-ch-4242-0-example.room-map-map")
        self.assertEqual(channel.parse_channel_pipe(full), (1, SID))
        # The vectors' contained row is exactly what a contained provider answers.
        row = next(r for r in _load("channels/channel-wire.json")["pipes"] if SID in r["pipe"] and r["result"] == "valid")
        self.assertEqual(full, row["pipe"])
        # A long name is cut to 256 and stays a valid name in its session.
        long_ = channel.unique_pipe_name("m" * 200, "c" * 100)
        make, full = channel.own_namespace_names(long_, NS)
        self.assertEqual(len(full), channel.MAX_PIPE_NAME)
        self.assertTrue(channel.is_channel_pipe_in(full, 1), full)
        self.assertTrue(full.endswith(make[len("\\\\.\\pipe\\LOCAL\\"):]))

    def test_admits_client(self):
        def boom(pid):
            raise PermissionError("no token")

        C = channel.Consumer
        self.assertIsNone(channel.admits_client(C(), 7, boom), "an older hub: the nonce decides")
        self.assertIsNone(channel.admits_client(C(pid=7), 7, boom))
        self.assertIn("not the consumer's 7", channel.admits_client(C(pid=7, sid=SID), 8, lambda p: SID))
        self.assertIsNone(channel.admits_client(C(sid=SID), 8, lambda p: SID))
        self.assertIn("isn't the consumer", channel.admits_client(C(sid=SID), 8, lambda p: None))
        self.assertIn("no token", channel.admits_client(C(sid=SID), 8, boom), "unreadable is a refusal")

    def test_consumer_of(self):
        from hideout_module import models

        create = models.ChannelCreate.from_json(
            {"channel": "map", "consumer": "x", "nonce": "n", "consumer_pid": 5, "consumer_sid": SID})
        self.assertEqual(channel.consumer_of(create), channel.Consumer(5, SID))
        self.assertEqual(channel.consumer_of({"channel": "map", "consumer": "x", "nonce": "n"}), channel.Consumer())


def _security(handle):
    """A kernel object's DACL ACEs and label ACEs, each (type, mask, SID string)."""
    import ctypes
    from ctypes import wintypes

    adv = ctypes.WinDLL("advapi32", use_last_error=True)
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    adv.GetSecurityInfo.argtypes = [ctypes.c_void_p, ctypes.c_int, wintypes.DWORD, ctypes.c_void_p,
                                    ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
                                    ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(ctypes.c_void_p)]
    adv.GetAce.argtypes = [ctypes.c_void_p, wintypes.DWORD, ctypes.POINTER(ctypes.c_void_p)]
    adv.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
    k32.LocalFree.argtypes = [ctypes.c_void_p]
    dacl, sacl, sd = ctypes.c_void_p(), ctypes.c_void_p(), ctypes.c_void_p()
    # SE_KERNEL_OBJECT; DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION
    rc = adv.GetSecurityInfo(handle, 6, 0x4 | 0x10, None, None, ctypes.byref(dacl), ctypes.byref(sacl),
                             ctypes.byref(sd))
    assert rc == 0, rc

    def aces(acl):
        out = []
        for i in range(ctypes.c_uint16.from_address(acl.value + 4).value if acl.value else 0):
            ace = ctypes.c_void_p()
            assert adv.GetAce(acl, i, ctypes.byref(ace))
            text = ctypes.c_void_p()
            assert adv.ConvertSidToStringSidW(ace.value + 8, ctypes.byref(text))
            out.append((ctypes.c_uint8.from_address(ace.value).value,
                        ctypes.c_uint32.from_address(ace.value + 4).value, ctypes.wstring_at(text.value)))
            k32.LocalFree(text)
        return out

    try:
        assert dacl.value, "a null DACL would grant everyone"
        return aces(dacl), aces(sacl)
    finally:
        k32.LocalFree(sd)


@unittest.skipUnless(os.name == "nt", "named pipes are Windows")
class PipeIdentity(unittest.TestCase):
    """#1092 in this process: the guest ACE and label, the client pid before a byte is read,
    the server pid before a byte is sent (channels and the router pipe)."""

    def test_guest_ace_and_low_label_only_with_a_guest(self):
        me = channel.current_user_sid()
        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "guest"), channel.Consumer(sid=SID))
        try:
            dacl, label = _security(p.handle)
            self.assertEqual(dacl, [(0, 0x1F01FF, me), (0, channel.PIPE_CLIENT_ACCESS, SID)])
            self.assertEqual(label, [(0x11, 0x1, "S-1-16-4096")], "Low, no write up")
        finally:
            p.close()
        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "noguest"))
        try:
            dacl, label = _security(p.handle)
            self.assertEqual(dacl, [(0, 0x1F01FF, me)])
            self.assertEqual(label, [], "no label set")
        finally:
            p.close()

    def _refused(self, consumer):
        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "who"), consumer)
        result = {}

        def provider():
            try:
                result["conn"] = p.accept_nonce("n-who", 10)
            except channel.ChannelError as e:
                result["err"] = e

        t = threading.Thread(target=provider)
        t.start()
        try:
            with channel.connect(p.name, "n-who", server_pid=os.getpid()) as conn:
                t.join(10)
                self.assertIsNone(conn.read_frame(), "dropped")
        except BrokenPipeError:
            pass  # dropped before the nonce was even written: the same refusal
        t.join(10)
        return result.get("err")

    def test_only_the_named_client_pid(self):
        err = self._refused(channel.Consumer(pid=4))
        self.assertIn(f"pid {os.getpid()}, not the consumer's 4", str(err))

    def test_sid_only_refuses_a_client_outside_that_container(self):
        err = self._refused(channel.Consumer(sid=SID))
        self.assertIn("isn't the consumer", str(err))

    def test_consumer_sends_nothing_to_another_server(self):
        p = channel.ChannelPipe.create(channel.unique_pipe_name("test.module", "server"))
        result = {}

        def provider():
            try:
                p.accept_nonce("n-s", 10)
            except channel.ChannelError as e:
                result["err"] = e

        t = threading.Thread(target=provider)
        t.start()
        with self.assertRaises(PermissionError):
            channel.connect_grant({"pipe": p.name, "nonce": "n-s", "pid": 4})
        t.join(10)
        self.assertIn("not the nonce", str(result["err"]), "the provider read no nonce")

    def test_serve_create_for_the_named_consumer(self):
        create = {"channel": "c", "consumer": "x", "nonce": "n-sc", "consumer_pid": os.getpid()}
        pipe = channel.serve_create("test.module", create, lambda conn: conn.write_frame(b"hi"))
        with channel.connect_grant({"pipe": pipe, "nonce": "n-sc", "pid": os.getpid()}) as conn:
            self.assertEqual(conn.read_frame(), b"hi")

    def test_consumer_refuses_names_windows_would_rewrite(self):
        for bad in ("\\\\.\\pipe\\hideout-ch-a\\..\\..\\..\\UNC\\h\\s\\hideout-ch-x", "\\\\.\\pipe\\hideout-ch-x.",
                    "\\\\.\\pipe\\LOCAL\\hideout-ch-x"):
            with self.assertRaises(ValueError):
                channel.connect(bad, "n")
        self.assertTrue(channel.stays_a_local_pipe("\\\\.\\pipe\\hideout-ch-x"))
        self.assertFalse(channel.stays_a_local_pipe("\\\\.\\pipe\\hideout-ch-a/../x"))

    def test_router_pipe_checks_the_hub_pid(self):
        from hideout_module import session

        self.assertEqual(session.hub_pid_from_env({"HIDEOUT_HUB_PID": " 42 "}), 42)
        self.assertIsNone(session.hub_pid_from_env({}), "an older hub: no check")
        self.assertIsNone(session.hub_pid_from_env({"HIDEOUT_HUB_PID": "x"}))
        for hub, ok in ((4, False), (os.getpid(), True)):
            with self.subTest(hub=hub):
                import _winapi

                p = channel.ChannelPipe.create(channel.unique_pipe_name("test.hub", "router"))
                h, p.handle = p.handle, None
                ov = _winapi.ConnectNamedPipe(h, overlapped=True)
                if ok:
                    f = session.open_router_pipe(p.name, hub)
                    f.write(b"line\n")
                    f.close()
                else:
                    with self.assertRaises(PermissionError):
                        session.open_router_pipe(p.name, hub)
                channel._wait(ov, time.monotonic() + 10)
                with channel.ChannelConn(h) as conn:
                    self.assertEqual(conn._read_exact(5), b"line\n" if ok else b"", "nothing before the check")


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
