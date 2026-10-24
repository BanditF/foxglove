"""Synthetic UF2 and fake USB/volume tests: never touch a keyboard."""
import io
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from flash import foxglove_flash as f


def uf2(payload=b'FOXGLOVE_HALF=left', addr=f.START, family=0xADA52840,
        numbers=(0,), flags=0x2000):
    return b''.join(struct.pack('<8I', 0x0A324655, 0x9E5D5157, flags, addr + i*256,
                                256, number, len(numbers), family)
                    + payload.ljust(476, b'\0') + struct.pack('<I', 0x0AB16F30)
                    for i, number in enumerate(numbers))


class FakeOS:
    def __init__(self, serials=None):
        self.serials = serials or ['1E89EF085AA8A635']
        self.present = True
        self.writes = []
        self.stays = False
        self.ignore_writes = 0  # writes the bootloader ignores before taking one
        self.info = 'Board-ID: nRF52840-nice!nano-v2\n'
        self.keyboard = True
        self.keyboard_serials = []
        self.auto = False
        self.entered = []

    def enumerate_bootloaders(self):
        return self.serials

    def find_volume(self, serial):
        return Path('fake-volume')

    def exists(self, path):
        return self.present

    def read_text(self, path):
        return self.info

    def write_file(self, path, data):
        self.writes.append((path, data))
        if self.ignore_writes:
            self.ignore_writes -= 1
            self.present = True
        else:
            self.present = self.stays

    def enter_bootloader(self, half, log):
        self.entered.append(half)
        return self.auto

    def running_keyboard(self, serial):
        self.keyboard_serials.append(serial)
        return self.keyboard


class Clock:
    def __init__(self):
        self.now = 0

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class PreflightTests(unittest.TestCase):
    def test_good_and_headroom(self):
        r = f.preflight(uf2(), 'left')
        self.assertTrue(r.ok, r.reasons)
        self.assertEqual(r.end_address, f.START + 256)
        self.assertEqual(r.headroom_kib, (f.LIMIT-f.START-256)/1024)

    def test_invalid_blocks(self):
        good = uf2()
        cases = [b'', good[:-1], b'BAD!' + good[4:], good[:-4] + b'BAD!',
                 uf2(family=0), uf2(flags=0), uf2(numbers=(0, 2)),
                 uf2(addr=f.LIMIT), uf2(addr=f.START-256)]
        for data in cases:
            with self.subTest(data=data[:32]):
                self.assertFalse(f.preflight(data, 'left').ok)
        for offset, value in ((16, 477), (24, 2)):
            data = bytearray(good)
            struct.pack_into('<I', data, offset, value)
            self.assertFalse(f.preflight(data, 'left').ok)

    def test_marked_halves_cannot_be_overridden(self):
        self.assertTrue(f.preflight(uf2(b'FOXGLOVE_HALF=right'), 'right').ok)
        self.assertFalse(f.preflight(uf2(), 'right', True).ok)
        self.assertFalse(f.preflight(uf2(b'FOXGLOVE_HALF=left FOXGLOVE_HALF=right'), 'left', True).ok)

    def test_flat_payload_handles_split_marker_and_reordered_blocks(self):
        data = uf2(b'x'*250 + b'FOXGLO', numbers=(0, 1))
        data = bytearray(data)
        data[544:544+12] = b'VE_HALF=left'
        # First block ends FOXGLO, second starts VE_HALF=left.
        data = bytes(data[512:] + data[:512])
        self.assertTrue(f.preflight(data, 'left').ok)

    def test_heuristics_and_unknown(self):
        for half, markers in f.HALF_MARKERS.items():
            data = uf2(b'\0'.join(markers))
            self.assertTrue(f.preflight(data, half).ok)
            self.assertFalse(f.preflight(data, 'right' if half == 'left' else 'left').ok)
        self.assertFalse(f.preflight(uf2(b'unknown'), 'left').ok)
        self.assertTrue(f.preflight(uf2(b'unknown'), 'left', True).ok)

    def test_settings_reset(self):
        data = uf2(b'SETTINGS RESET raw_hid_received_event')
        self.assertFalse(f.preflight(data, 'left', True).ok)
        self.assertTrue(f.preflight(data, 'left', settings_reset=True).ok)
        self.assertFalse(f.preflight(uf2(), 'left', settings_reset=True).ok)


class FlashTests(unittest.TestCase):
    def run_flash(self, backend, answers=('FLASH', 'TYPES'), images=None, **kwargs):
        clock = Clock()
        answers = iter(answers)
        f.flash(images or {'left': uf2()}, backend, 2,
                stdin=type('TTY', (), {'isatty': lambda self: True})(),
                ask=lambda prompt: next(answers), log=lambda message: None,
                clock=clock.clock, sleep=clock.sleep, **kwargs)

    def test_unknown_serial_and_two_bootloaders_never_write(self):
        for serials, error in [(['unknown'], f.Refused), (list(f.SERIALS), TimeoutError),
                               (['35B4541F78DE9CF8'], TimeoutError)]:
            backend = FakeOS(serials)
            with self.assertRaises(error):
                self.run_flash(backend)
            self.assertEqual(backend.writes, [])

    def test_no_piped_confirmation(self):
        backend = FakeOS()
        with self.assertRaises(f.Refused):
            f.flash({'left': uf2()}, backend, 1, stdin=io.StringIO('FLASH\n'), log=lambda x: None)
        self.assertEqual(backend.writes, [])

    def test_success_and_reboot_timeout(self):
        backend = FakeOS()
        self.run_flash(backend)
        self.assertEqual(len(backend.writes), 1)
        backend = FakeOS()
        backend.stays = True
        with self.assertRaises(TimeoutError):
            self.run_flash(backend)
        backend = FakeOS()
        backend.keyboard = False
        with self.assertRaises(TimeoutError):
            self.run_flash(backend)

    def test_hands_free_entry_only_after_flash_confirmation(self):
        backend = FakeOS()
        backend.auto = True
        with self.assertRaises(f.Refused):
            self.run_flash(backend, ('NO',))
        self.assertEqual(backend.entered, [])
        self.run_flash(backend)
        self.assertEqual(backend.entered, ['left'])
        self.assertEqual(len(backend.writes), 1)

    def test_right_entry_retries_then_falls_back_to_double_tap(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        self.run_flash(backend, ('FLASH',), {'right': uf2(b'FOXGLOVE_HALF=right')})
        self.assertEqual(backend.entered, ['right'] * 3)
        self.assertEqual(len(backend.writes), 1)

    def test_settings_reset_never_uses_hands_free_entry(self):
        backend = FakeOS()
        backend.auto = True
        self.run_flash(backend, ('FLASH', 'ERASE SETTINGS'), {'left': uf2(b'SETTINGS RESET')},
                       settings_reset=True)
        self.assertEqual(backend.entered, [])

    def test_ignored_write_is_retried_once_after_settling(self):
        backend = FakeOS()
        backend.ignore_writes = 1
        self.run_flash(backend)
        self.assertEqual(len(backend.writes), 2)
        backend = FakeOS()
        backend.stays = True
        with self.assertRaises(TimeoutError):
            self.run_flash(backend)
        self.assertEqual(len(backend.writes), 2, 'never more than one retry')

    def test_left_reenumeration_checks_the_left_serial(self):
        # The right half on USB power also enumerates as 1D50:615E; only the left's serial counts.
        backend = FakeOS()
        self.run_flash(backend)
        self.assertEqual(set(backend.keyboard_serials), {'1E89EF085AA8A635'})

    def test_left_typing_gate_precedes_right(self):
        backend = FakeOS()
        with self.assertRaises(f.Refused):
            self.run_flash(backend, ('FLASH', 'NO'), {'left': uf2(), 'right': uf2(b'FOXGLOVE_HALF=right')})
        self.assertEqual(len(backend.writes), 1)

    def test_ambiguous_missing_volume_and_board_id(self):
        for mode in ('ambiguous', 'missing', 'board'):
            backend = FakeOS()
            if mode == 'ambiguous':
                backend.find_volume = lambda serial: (_ for _ in ()).throw(f.Refused('ambiguous'))
            elif mode == 'missing':
                backend.present = False
            else:
                backend.info = 'No board ID'
            with self.assertRaises(f.Refused):
                self.run_flash(backend)
            self.assertEqual(backend.writes, [])

    def test_identity_rechecked(self):
        backend = FakeOS()
        replies = iter([backend.serials, ['35B4541F78DE9CF8']])
        backend.enumerate_bootloaders = lambda: next(replies)
        with self.assertRaises(f.Refused):
            self.run_flash(backend)
        self.assertEqual(backend.writes, [])

    def test_settings_confirmation_separate(self):
        backend = FakeOS()
        with self.assertRaises(f.Refused):
            self.run_flash(backend, ('FLASH', 'NO'), {'left': uf2(b'SETTINGS RESET')}, settings_reset=True)
        self.assertFalse(backend.writes)

    def test_windows_mapping_refuses_ambiguous(self):
        backend = f.WindowsOS()
        with patch.object(backend, '_ps', return_value=[{'Volume': 'X:'}, {'Volume': 'Y:'}]):
            with self.assertRaises(f.Refused):
                backend.find_volume(next(iter(f.SERIALS)))

    def test_write_requires_all_bytes_before_ignoring_oserror(self):
        stream = unittest.mock.MagicMock()
        stream.__enter__.return_value = stream
        stream.write.return_value = 512
        with patch('builtins.open', return_value=stream), patch.object(f.os, 'fsync', side_effect=OSError('reset')):
            f.WindowsOS().write_file('unused', uf2())
            stream.flush.assert_called_once()
        stream.write.side_effect = OSError('early reset')
        with patch('builtins.open', return_value=stream):
            with self.assertRaises(f.FlashFailed):
                f.WindowsOS().write_file('unused', uf2())

    def test_cli_exit_codes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'left.uf2'
            path.write_bytes(uf2())
            for error, expected in ((f.Refused('no'), 1), (TimeoutError('wait'), 2),
                                    (f.FlashFailed('write'), 3)):
                with self.subTest(expected=expected), patch.object(f, 'flash', side_effect=error), patch('sys.stdout', new_callable=io.StringIO):
                    self.assertEqual(f.main(['--left', str(path), '--half', 'left']), expected)

    def test_dry_run_never_constructs_backend_and_json(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'left.uf2'
            path.write_bytes(uf2())
            with patch.object(f, 'WindowsOS', side_effect=AssertionError('device access')), patch('sys.stdout', new_callable=io.StringIO) as out:
                self.assertEqual(f.main(['--left', str(path), '--half', 'left', '--dry-run', '--json']), 0)
                self.assertEqual(f.json.loads(out.getvalue())['exit_code'], 0)


class OptionalRealImages(unittest.TestCase):
    @unittest.skipUnless(Path(f.ROLLBACK).is_dir(), 'local known-good UF2 archive not available')
    def test_known_good_archive(self):
        for half, filename in f.FILENAMES.items():
            result = f.preflight((Path(f.ROLLBACK) / filename).read_bytes(), half)
            self.assertTrue(result.ok, result.reasons)
        result = f.preflight((Path(f.ROLLBACK) / 'settings_reset-nice_nano_v2-zmk.uf2').read_bytes(),
                             'left', settings_reset=True)
        self.assertTrue(result.ok, result.reasons)


if __name__ == '__main__':
    unittest.main()


class MarkerTests(unittest.TestCase):
    def test_full_marker_across_blocks_reports_build(self):
        marker = b'FOXGLOVE_HALF=right;VER=0.1.0;PROFILE=ga;SHA=abc1234'
        # Two blocks, the marker split between them.
        first, second = marker[:20], marker[20:]
        blocks = []
        for i, part in enumerate((first.rjust(256, b'\0'), second.ljust(256, b'\0'))):
            blocks.append(struct.pack('<8I', 0x0A324655, 0x9E5D5157, 0x2000, f.START + i * 256,
                                      256, i, 2, 0xADA52840) + part.ljust(476, b'\0')
                          + struct.pack('<I', 0x0AB16F30))
        result = f.preflight(b''.join(blocks), 'right')
        self.assertTrue(result.ok, result.reasons)
        self.assertEqual((result.detected_half, result.version, result.profile, result.sha),
                         ('right', '0.1.0', 'ga', 'abc1234'))
        self.assertFalse(f.preflight(b''.join(blocks), 'left').ok)

    def test_old_short_marker_still_works(self):
        result = f.preflight(uf2(b'FOXGLOVE_HALF=left'), 'left')
        self.assertTrue(result.ok)
        self.assertEqual(result.version, '')

    def test_firmware_folder_picks_new_names_by_profile(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            for name in ('foxglove-fw-v0.1.0-abc1234-left-dev.uf2', 'foxglove-fw-v0.1.0-abc1234-left-ga.uf2'):
                (d / name).write_bytes(b'')
            self.assertEqual(f.firmware_file(d, 'left', 'ga').name, 'foxglove-fw-v0.1.0-abc1234-left-ga.uf2')
            self.assertEqual(f.firmware_file(d, 'right', 'dev').name, f.FILENAMES['right'])
            (d / 'foxglove-fw-v0.1.1-def5678-left-dev.uf2').write_bytes(b'')
            with self.assertRaises(f.Refused):
                f.firmware_file(d, 'left', 'dev')
