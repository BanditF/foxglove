"""restart_parity's pure helpers (device, feed_art_row) and its CASES registry."""

import sys
import unittest
from pathlib import Path

LAB = Path(__file__).resolve().parent
sys.path.insert(0, str(LAB))

import restart_parity  # noqa: E402


class Device(unittest.TestCase):

    def test_device_times_out_returns_none(self):
        """wait_fact returning None means no keyboard; device() returns None."""
        import unittest.mock as m
        with m.patch.object(restart_parity, "wait_fact", return_value=None):
            self.assertIsNone(restart_parity.device(None))

    def test_device_success_with_keyboard_id(self):
        """A connected keyboard's id is returned from devices.list."""
        import unittest.mock as m
        mock_api = m.MagicMock()
        mock_api.call.return_value = {"devices": [{"id": "kb-1234"}]}
        with m.patch.object(restart_parity, "wait_fact", return_value=True):
            self.assertEqual(restart_parity.device(mock_api), "kb-1234")

    def test_device_empty_list_returns_none(self):
        """devices.list returns [] when no keyboard; device() returns None."""
        import unittest.mock as m
        mock_api = m.MagicMock()
        mock_api.call.return_value = {"devices": []}
        with m.patch.object(restart_parity, "wait_fact", return_value=True):
            self.assertIsNone(restart_parity.device(mock_api))


class FeedArtRow(unittest.TestCase):

    def test_matches_subsystem_returns_level(self):
        """The matching subsystem's level is returned."""
        import unittest.mock as m
        mock_api = m.MagicMock()
        mock_api.call.return_value = {
            "rows": [
                {"subsystem": restart_parity.FEED_ART, "level": "ok"},
                {"subsystem": "other", "level": "warn"},
            ]
        }
        self.assertEqual(restart_parity.feed_art_row(mock_api, "dev"), "ok")

    def test_no_match_returns_none(self):
        """No row with FEED_ART subsystem returns None."""
        import unittest.mock as m
        mock_api = m.MagicMock()
        mock_api.call.return_value = {"rows": [{"subsystem": "other", "level": "warn"}]}
        self.assertIsNone(restart_parity.feed_art_row(mock_api, "dev"))


if __name__ == "__main__":
    unittest.main()
