"""Tests for restore_guard's process-wait and cleanup logic (no GUI, no keyboard).

- foxglove_running(): returns True only when Get-Process reports a non-zero count.
- main(lab_pid, app_pid, paths): waits for the lab's handle, then cleans up the test app, and starts Foxglove paths only when none is running (mocked kernel32).
"""
import subprocess
import sys
import unittest
import unittest.mock
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import restore_guard as rg  # noqa: E402


class MockKernel:
    """Minimal ctypes.windll.kernel32 mock for main's Windows-API calls.

    restore_guard opens lab first (returns truthy), then app (also returns truthy).
    WaitForSingleObject is called on lab first, then optionally on app. We use a counter
    to distinguish the two calls since both handles are truthy.
    """
    def __init__(self, lab_done=True, app_done=False):
        self.lab_done = lab_done
        self.app_done = app_done
        self._wait_count = 0
        # MagicMock accepts arbitrary attribute assignment (e.g. .restype/.argtypes) and is callable.
        self.OpenProcess = unittest.mock.MagicMock(return_value=True)
        self.TerminateProcess = unittest.mock.MagicMock()
        self.CloseHandle = unittest.mock.MagicMock()

        def wait_single(handle, timeout):
            """Return 0 for lab (first call), then based on app_done for subsequent calls."""
            self._wait_count += 1
            if self._wait_count == 1:
                return 0 if self.lab_done else rg.WAIT_TIMEOUT
            return 0 if self.app_done else rg.WAIT_TIMEOUT

        self.WaitForSingleObject = unittest.mock.MagicMock(side_effect=wait_single)


class FoxgloveRunning(unittest.TestCase):
    def test_zero_processes_returns_false(self):
        with unittest.mock.patch.object(subprocess, 'run', return_value=unittest.mock.Mock(
                stdout='0\n')):
            self.assertFalse(rg.foxglove_running())

    def test_empty_output_returns_false(self):
        with unittest.mock.patch.object(subprocess, 'run', return_value=unittest.mock.Mock(
                stdout='\n')):
            self.assertFalse(rg.foxglove_running())

    def test_nonzero_count_returns_true(self):
        with unittest.mock.patch.object(subprocess, 'run', return_value=unittest.mock.Mock(
                stdout=' 3\n')):
            self.assertTrue(rg.foxglove_running())


class MainCleanup(unittest.TestCase):
    """Branches of main() when lab finishes normally (wait == 0)."""

    def test_foxglove_is_started_when_paths_and_not_running(self):
        """After cleaning up, Foxglove starts for each path when none is running."""
        kernel = MockKernel(lab_done=True, app_done=True)

        paths = ['C:/user/foxglove1.exe', 'C:/user/foxglove2.exe']
        with unittest.mock.patch('ctypes.windll.kernel32', kernel), \
                unittest.mock.patch.object(rg, 'foxglove_running', return_value=False), \
                unittest.mock.patch('subprocess.Popen') as popen_mock, \
                unittest.mock.patch('time.sleep'):
            rg.main(12345, 67890, paths)

        self.assertEqual(popen_mock.call_count, 2)
        args_list = [call[0][0] for call in popen_mock.call_args_list]
        self.assertIn(['C:/user/foxglove1.exe'], args_list)
        self.assertIn(['C:/user/foxglove2.exe'], args_list)

    def test_foxglove_not_started_when_already_running(self):
        """If Foxglove is already running, paths are ignored even when provided."""
        kernel = MockKernel(lab_done=True, app_done=True)

        with unittest.mock.patch('ctypes.windll.kernel32', kernel), \
                unittest.mock.patch.object(rg, 'foxglove_running', return_value=True), \
                unittest.mock.patch('subprocess.Popen') as popen_mock:
            rg.main(12345, 67890, ['C:/user/foxglove.exe'])

        popen_mock.assert_not_called()

    def test_timeout_aborts_without_restart(self):
        """When lab times out, both handles are closed, the app is not terminated, nothing starts."""
        kernel = MockKernel(lab_done=False, app_done=True)

        with unittest.mock.patch('ctypes.windll.kernel32', kernel), \
                unittest.mock.patch.object(rg, 'foxglove_running', return_value=False), \
                unittest.mock.patch('subprocess.Popen') as popen_mock:
            rg.main(12345, 67890, ['C:/user/foxglove.exe'])

        popen_mock.assert_not_called()
        kernel.TerminateProcess.assert_not_called()
        self.assertEqual(kernel.CloseHandle.call_count, 2)


if __name__ == '__main__':
    unittest.main()
