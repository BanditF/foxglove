"""Tests for hideout_contained_child provider and consumer helpers via mocked channels.
No actual pipes, network or keyboard involvement."""

import json
import sys
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "sdk" / "python"))

import hideout_contained_child as hc  # noqa: E402


class Provide(unittest.TestCase):

    def setUp(self):
        self.consumer = Mock()
        self.pipe = Mock()
        self.conn = Mock()
        self.conn.__enter__ = Mock(return_value=self.conn)
        self.conn.__exit__ = Mock(return_value=False)
        self.pipe.accept_nonce.return_value = self.conn
        self.patches = [
            patch("hideout_module.channel.Consumer", return_value=self.consumer),
            patch("hideout_module.channel.ChannelPipe.create", return_value=self.pipe),
        ]
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()

    def test_provide_accepts_the_nonce(self):
        """accept_nonce is called with the consumer nonce and 20 s timeout."""
        self.say = self.enterContext(patch.object(hc, "say"))
        hc.provide({"consumer_pid": 1, "nonce": "my-nonce"})
        self.assertEqual(self.pipe.accept_nonce.call_count, 1)
        self.pipe.accept_nonce.assert_called_with("my-nonce", 20)

    def test_provide_writes_5_frames(self):
        """The provider sends bytes [i]*3 for i in range(5)."""
        self.say = self.enterContext(patch.object(hc, "say"))
        hc.provide({"consumer_pid": 1, "nonce": "x"})
        self.assertEqual(self.conn.write_frame.call_count, 5)
        for i in range(5):
            self.conn.write_frame.assert_any_call(bytes([i]) * 3)

    def test_provide_reads_one_end_frame_and_reports_it(self):
        """After writing frames the provider reads one frame back and includes it."""
        self.say = self.enterContext(patch.object(hc, "say"))
        self.conn.read_frame.return_value = b"\xff"
        hc.provide({"consumer_pid": 1, "nonce": "x"})
        last_call = self.say.call_args_list[-1]
        self.assertEqual(last_call[1], {"accepted": True, "end": b"\xff"})


class ProvideError(unittest.TestCase):

    def setUp(self):
        self.consumer = Mock()
        self.pipe = Mock()
        self.conn = Mock()
        self.patches = [
            patch("hideout_module.channel.Consumer", return_value=self.consumer),
            patch("hideout_module.channel.ChannelPipe.create", return_value=self.pipe),
        ]
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()

    def test_provide_reports_why_accept_nonce_failed(self):
        """When accept_nonce raises, the provider answers accepted=False with the error."""
        self.say = self.enterContext(patch.object(hc, "say"))
        self.pipe.accept_nonce.side_effect = OSError("no consumer")
        hc.provide({"consumer_pid": 1, "nonce": "x"})
        last_call = self.say.call_args_list[-1]
        self.assertEqual(last_call[1], {"accepted": False, "error": "no consumer"})

    def test_provide_does_not_write_frames_on_accept_failure(self):
        """No frames are written when accept_nonce fails."""
        self.say = self.enterContext(patch.object(hc, "say"))
        self.pipe.accept_nonce.side_effect = OSError("timeout")
        hc.provide({"consumer_pid": 1, "nonce": "x"})
        self.conn.write_frame.assert_not_called()


class Consume(unittest.TestCase):

    def setUp(self):
        self.conn = Mock()
        self.conn.__enter__ = Mock(return_value=self.conn)
        self.conn.__exit__ = Mock(return_value=False)
        frames = [bytes([i]) * 3 for i in range(5)]
        self.conn.read_frame.side_effect = list(frames) + [None]
        self.patches = [
            patch("hideout_module.channel.connect_grant", return_value=self.conn),
        ]
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()

    def test_consume_reads_5_frames_and_reports_them(self):
        """The consumer collects 5 frames as hex strings."""
        self.say = self.enterContext(patch.object(hc, "say"))
        grant = {"pipe": "\\\\.\\pipe\\hideout-ch-test", "nonce": "x"}
        hc.consume(grant)
        self.assertEqual(len(self.conn.read_frame.call_args_list), 5)
        self.say.assert_called_once_with(
            frames=["000000", "010101", "020202", "030303", "040404"]
        )


class ConsumeError(unittest.TestCase):

    def setUp(self):
        self.patches = [
            patch("hideout_module.channel.connect_grant", side_effect=ValueError("bad pipe")),
        ]
        for p in self.patches:
            p.start()

    def tearDown(self):
        for p in self.patches:
            p.stop()

    def test_consume_reports_connection_error_name_and_message(self):
        """A connection error is answered with its type name and message."""
        self.say = self.enterContext(patch.object(hc, "say"))
        grant = {"pipe": "x", "nonce": "y"}
        hc.consume(grant)
        call_kwargs = self.say.call_args[1] if self.say.called else {}
        self.assertIn("ValueError", call_kwargs["error"])
        self.assertIn("bad pipe", call_kwargs["error"])


if __name__ == "__main__":
    unittest.main()
