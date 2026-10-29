"""Synthetic UF2 and fake USB/volume tests: never touch a keyboard."""
import io
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from flash import foxglove_flash as f

# Tests never touch the real flash history.
_HISTORY = tempfile.TemporaryDirectory()
os.environ['FOXGLOVE_FLASH_HISTORY'] = str(Path(_HISTORY.name) / 'flash-history.log')
# ...nor the real flasher lock: a test must never block (or be blocked by) a real flash.
os.environ['FOXGLOVE_FLASH_LOCK'] = str(Path(_HISTORY.name) / 'flash.lock')


def uf2(payload=b'FOXGLOVE_HALF=left', addr=f.START, family=0xADA52840,
        numbers=(0,), flags=0x2000):
    return b''.join(struct.pack('<8I', 0x0A324655, 0x9E5D5157, flags, addr + i*256,
                                256, number, len(numbers), family)
                    + payload.ljust(476, b'\0') + struct.pack('<I', 0x0AB16F30)
                    for i, number in enumerate(numbers))


class FakeOS:
    """`serials` is what the bootloader enumeration shows once the half is asked in (or
    double-tapped). Before that, the flasher's first look (#654: bootloaders already connected)
    finds none, unless `stranded`."""
    def __init__(self, serials=None, stranded=False):
        self.serials = serials or ['1E89EF085AA8A635']
        self.looked = stranded
        self.present = True
        self.writes = []
        self.stays = False
        self.ignore_writes = 0  # writes the bootloader ignores before taking one
        self.info = 'Board-ID: nRF52840-nice!nano-v2\n'
        self.keyboard = True
        self.keyboard_serials = []
        self.auto = False
        self.builds = {}
        self.typing = []
        self.links = []
        self.link_polls = 0
        self.gone_mid_write = False
        self.types = {}
        self.build_calls = []
        self.entered = []
        # Relay-ready event waits (#594): each answer is True (event came), False (timed out)
        # or None (no relay events: poll HELLO). Empty: None, the older firmware's path.
        self.relay = []
        self.relay_waits = []
        self.clock = None
        # Snapshot at the first image write, so entry tests don't count the post-flash typing
        # check's relay wait (#600).
        self.at_write = None
        # Each half's running-firmware USB device (#654): its status, or None when it isn't on
        # USB. Both present by default; the right on USB power shows problem 28 ('Error').
        self.usb = {'left': 'OK', 'right': 'Error'}
        self.usb_checks = []

    def usb_device(self, serial):
        self.usb_checks.append(serial)
        return self.usb.get(f.SERIALS[serial])

    def wait_relay_ready(self, seconds):
        self.relay_waits.append(seconds)
        answer = self.relay.pop(0) if self.relay else None
        if answer is False and self.clock:
            self.clock.now += seconds  # waited out the whole timeout
        return answer

    def enumerate_bootloaders(self):
        if not self.looked:
            self.looked = True
            return []
        return self.serials

    def find_volume(self, serial):
        return Path('fake-volume')

    def exists(self, path):
        return self.present

    def read_text(self, path):
        return self.info

    def write_file(self, path, data):
        self.writes.append((path, data))
        if self.at_write is None:
            self.at_write = {'link_polls': self.link_polls, 'relay_waits': list(self.relay_waits),
                             'now': self.clock.now if self.clock else None}
        if self.gone_mid_write:
            self.present = False
            raise f.DriveGone(512, len(data), OSError('WinError 433'))
        if self.ignore_writes:
            self.ignore_writes -= 1
            self.present = True
        else:
            self.present = self.stays

    def right_link(self):
        self.link_polls += 1
        return self.links.pop(0) if self.links else 2

    def build_info(self, half):
        self.build_calls.append(half)
        return self.builds.get(half)

    def typing_check(self, half, log):
        self.typing.append(half)
        return self.types.get(half, True)

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

    def test_unknown_serial_and_wrong_half_never_write(self):
        for serials, error in [(['unknown'], f.Refused), (['1E89EF085AA8A635', 'unknown'], f.Refused),
                               (['35B4541F78DE9CF8'], TimeoutError)]:
            backend = FakeOS(serials)
            with self.assertRaises(error):
                self.run_flash(backend)
            self.assertEqual(backend.writes, [])

    def test_two_known_bootloaders_write_only_this_half(self):
        # #654: the serial map places each drive, so a second known bootloader no longer blocks.
        backend = FakeOS(list(f.SERIALS))
        volumes = []
        backend.find_volume = lambda serial: (volumes.append(serial), Path('fake-volume'))[1]
        # The right's bootloader stays and this run has no right image: said, not ignored.
        with self.assertRaisesRegex(f.Refused, '--half right'):
            self.run_flash(backend)
        self.assertEqual(len(backend.writes), 1)
        self.assertEqual(set(volumes), {'1E89EF085AA8A635'})

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
        clock = Clock()
        real = backend.enumerate_bootloaders
        # The user double-taps after the retries and the late-entry watch.
        after = f.ENTRY_RETRY_SECONDS + f.LATE_ENTRY_SECONDS + 1
        backend.enumerate_bootloaders = lambda: real() if clock.now > after else []
        answers = iter(('FLASH',))
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 60,
                stdin=type('TTY', (), {'isatty': lambda self: True})(),
                ask=lambda prompt: next(answers), log=lambda message: None,
                clock=clock.clock, sleep=clock.sleep)
        # Every ENTRY_RETRY_PAUSE s for ENTRY_RETRY_SECONDS, plus the first ask (#545).
        tries = f.ENTRY_RETRY_SECONDS // f.ENTRY_RETRY_PAUSE + 1
        self.assertEqual(backend.entered, ['right'] * tries)
        self.assertEqual(len(backend.writes), 1)

    def test_left_entry_is_asked_once(self):
        backend = FakeOS()
        self.run_flash(backend)
        self.assertEqual(backend.entered, ['left'])

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
        replies = iter([[], backend.serials, ['35B4541F78DE9CF8']])  # first look: none yet
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


def marked(half, profile='dev', sha='abc1234'):
    return uf2(f'FOXGLOVE_HALF={half};VER=0.1.0;PROFILE={profile};SHA={sha}'.encode())


class BuildInfoTests(unittest.TestCase):
    def run_flash(self, backend, answers=('FLASH', 'TYPES'), images=None, **kwargs):
        clock = Clock()
        answers = iter(answers)
        f.flash(images or {'left': marked('left')}, backend, 2,
                stdin=type('TTY', (), {'isatty': lambda self: True})(),
                ask=lambda prompt: next(answers), log=lambda message: None,
                clock=clock.clock, sleep=clock.sleep, **kwargs)

    def test_refuses_a_dev_ga_mix_before_asking_anything(self):
        backend = FakeOS()
        backend.builds = {'right': {'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}}
        with self.assertRaises(f.Refused):
            self.run_flash(backend, ('FLASH', 'TYPES'), {'left': marked('left', 'ga')})
        self.assertEqual(backend.writes, [])
        self.assertEqual(backend.entered, [])
        self.run_flash(FakeOS(), ('FLASH', 'TYPES'), {'left': marked('left', 'ga')}, allow_profile_mix=True)

    def test_verifies_the_new_build_after_flashing(self):
        backend = FakeOS()
        backend.builds = {'left': {'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}}
        self.run_flash(backend)
        self.assertIn('left', backend.build_calls)
        backend = FakeOS()
        backend.builds = {'left': {'version': '0.0.9', 'profile': 'dev', 'sha': '0000000'}}
        with self.assertRaises(f.FlashFailed):
            self.run_flash(backend)

    def test_unknown_running_build_is_not_an_error(self):
        backend = FakeOS()
        self.run_flash(backend)
        self.assertEqual(len(backend.writes), 1)


class YesTests(unittest.TestCase):
    def run_yes(self, backend, images, **kwargs):
        clock = Clock()
        f.flash(images, backend, 2, stdin=io.StringIO(''), ask=lambda prompt: self.fail(f'asked: {prompt}'),
                log=lambda message: None, clock=clock.clock, sleep=clock.sleep, yes=True, **kwargs)

    def test_yes_never_prompts_and_checks_typing(self):
        backend = FakeOS()
        self.run_yes(backend, {'left': uf2()})
        self.assertEqual(len(backend.writes), 1)
        self.assertEqual(backend.typing, ['left'])

    def test_yes_keeps_every_automatic_check(self):
        backend = FakeOS()
        with self.assertRaises(f.Refused):
            self.run_yes(backend, {'left': uf2(b'FOXGLOVE_HALF=right')})
        backend = FakeOS(['35B4541F78DE9CF8'])
        with self.assertRaises(TimeoutError):  # wrong half in the bootloader: never written
            self.run_yes(backend, {'left': uf2()})
        self.assertEqual(backend.writes, [])

    def test_yes_stops_before_the_right_if_the_left_does_not_type(self):
        backend = FakeOS()
        backend.types = {'left': False}
        with self.assertRaises(f.FlashFailed):
            self.run_yes(backend, {'left': uf2(), 'right': uf2(b'FOXGLOVE_HALF=right')})
        self.assertEqual(len(backend.writes), 1)

    def test_yes_right_typing_failure_is_reported_not_fatal(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.types = {'right': False}
        self.run_yes(backend, {'right': uf2(b'FOXGLOVE_HALF=right')})
        self.assertEqual(backend.typing, ['right', 'right'], 'one retry after the relay wait (#600)')


class SkipSameBuildTests(unittest.TestCase):
    def run_yes(self, backend, images, **kwargs):
        clock = Clock()
        f.flash(images, backend, 2, stdin=io.StringIO(''), ask=lambda prompt: self.fail(prompt),
                log=lambda message: None, clock=clock.clock, sleep=clock.sleep, yes=True, **kwargs)

    def test_half_already_on_the_build_is_skipped_unless_forced(self):
        backend = FakeOS()
        backend.builds = {'left': {'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}}
        self.run_yes(backend, {'left': marked('left')})
        self.assertEqual(backend.writes, [])
        self.assertEqual(backend.entered, [])
        self.run_yes(backend, {'left': marked('left')}, force=True)
        self.assertEqual(len(backend.writes), 1)


class LockTests(unittest.TestCase):
    def test_second_flasher_is_refused_and_stale_locks_are_taken_over(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'flash.lock'
            with f.FlashLock(path, alive=lambda pid: True):
                with self.assertRaises(f.Refused):
                    with f.FlashLock(path, alive=lambda pid: True):
                        pass
            self.assertFalse(path.exists())
            path.write_text('999999')
            with f.FlashLock(path, alive=lambda pid: False):
                self.assertEqual(path.read_text(), str(os.getpid()))
            self.assertFalse(path.exists())


class DriveGoneTests(unittest.TestCase):
    """#176: a drive that vanishes mid-write may have taken the whole image."""
    def run_yes(self, backend, images):
        clock = Clock()
        f.flash(images, backend, 2, stdin=io.StringIO(''), ask=lambda p: self.fail(p),
                log=lambda m: None, clock=clock.clock, sleep=clock.sleep, yes=True)

    def test_build_read_back_decides(self):
        backend = FakeOS()
        backend.gone_mid_write = True
        backend.builds = {'left': {'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}}
        # Running build == image SHA: skipped as same build, so force a different running SHA first.
        calls = iter([{'version': '0.0.9', 'profile': 'dev', 'sha': '0000000'}] * 2
                     + [{'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}] * 10)
        backend.build_info = lambda half: next(calls)
        self.run_yes(backend, {'left': marked('left')})
        self.assertEqual(len(backend.writes), 1, 'no blind retry after a vanished drive')

    def test_wrong_or_unknown_build_after_vanished_drive_fails(self):
        backend = FakeOS()
        backend.gone_mid_write = True
        backend.builds = {'left': {'version': '0.0.9', 'profile': 'dev', 'sha': '0000000'}}
        with self.assertRaises(f.FlashFailed):
            self.run_yes(backend, {'left': marked('left')})
        backend = FakeOS()
        backend.gone_mid_write = True
        with self.assertRaises(f.FlashFailed):
            self.run_yes(backend, {'left': marked('left')})
        backend = FakeOS()
        backend.gone_mid_write = True
        with self.assertRaises(f.FlashFailed):  # no SHA in the image: can't tell
            self.run_yes(backend, {'left': uf2()})


class PostWriteDriveVanishTests(unittest.TestCase):
    """#176: after a complete write, the drive vanishing (stat raising WinError 433) is success."""
    def test_exists_treats_oserror_as_gone(self):
        with patch.object(f.Path, 'exists', side_effect=OSError(433, 'device not connected')):
            self.assertFalse(f.WindowsOS().exists('X:/INFO_UF2.TXT'))

    def test_vanish_during_the_post_write_wait_counts_as_success_and_is_recorded(self):
        with tempfile.TemporaryDirectory() as d:
            history = Path(d) / 'history.log'
            with patch.dict(os.environ, {'FOXGLOVE_FLASH_HISTORY': str(history)}):
                backend = FakeOS()
                backend.stays = True  # the fake drive would stay; the real stat raises instead
                written = []
                fake_write = backend.write_file
                def write(path, data):
                    fake_write(path, data)
                    written.append(path)
                backend.write_file = write
                fake_exists = backend.exists
                def exists(path):
                    if not written:
                        return fake_exists(path)
                    # After the write: what WindowsOS.exists returns when the stat raises 433.
                    with patch.object(f.Path, 'exists', side_effect=OSError(433, 'device not connected')):
                        return f.WindowsOS().exists(path)
                backend.exists = exists
                calls = iter([{'version': '0.0.9', 'profile': 'dev', 'sha': '0000000'}] * 2
                             + [{'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}] * 10)
                backend.build_info = lambda half: next(calls)
                clock = Clock()
                f.flash({'left': marked('left')}, backend, 2, stdin=io.StringIO(''), ask=lambda p: self.fail(p),
                        log=lambda m: None, clock=clock.clock, sleep=clock.sleep, yes=True)
                self.assertEqual(len(written), 1, 'one write, no retry')
                lines = history.read_text().splitlines()
                self.assertEqual([l.split('	')[3] for l in lines], ['written', 'verified'])
                self.assertEqual(lines[0].split('	')[1:3], ['left', 'abc1234'])


class YesKeepsChecksInMain(unittest.TestCase):
    def test_yes_still_preflights_and_takes_the_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            good = Path(directory) / 'left.uf2'
            good.write_bytes(uf2())
            bad = Path(directory) / 'right-image-as-left.uf2'
            bad.write_bytes(uf2(b'FOXGLOVE_HALF=right'))
            entered = []
            class Lock:
                def __enter__(self):
                    entered.append(True)
                    return self
                def __exit__(self, *exc):
                    return False
            with patch.object(f, 'FlashLock', Lock), patch.object(f, 'flash') as flash,                     patch.object(f, 'WindowsOS'), patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(f.main(['--left', str(good), '--half', 'left', '--yes']), 0)
                self.assertEqual(entered, [True])
                self.assertTrue(flash.call_args.kwargs['yes'])
                # Preflight still refuses a wrong-half image before any device access or lock.
                self.assertEqual(f.main(['--left', str(bad), '--half', 'left', '--yes']), 1)
                self.assertEqual(entered, [True])


class HistoryMatchesConsoleTests(unittest.TestCase):
    def test_unreadable_build_is_recorded_unverified(self):
        with tempfile.TemporaryDirectory() as d:
            history = Path(d) / 'h.log'
            with patch.dict(os.environ, {'FOXGLOVE_FLASH_HISTORY': str(history)}):
                backend = FakeOS()  # build_info returns None: the firmware can't tell
                clock = Clock()
                logs = []
                f.flash({'left': marked('left')}, backend, 2, stdin=io.StringIO(''), ask=lambda p: self.fail(p),
                        log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
                outcomes = [l.split('	')[3] for l in history.read_text().splitlines()]
                self.assertEqual(outcomes, ['written', 'unverified'])
                self.assertTrue(any("couldn't read its build back" in m for m in logs))

    def test_right_gets_a_minute_to_answer(self):
        backend = FakeOS()
        answers = iter([None] * 25 + [{'version': '0.1.0', 'profile': 'dev', 'sha': 'abc1234'}])
        backend.build_info = lambda half: next(answers)
        clock = Clock()
        self.assertTrue(f.verify_build('right', 'abc1234', backend, lambda m: None, clock.clock, clock.sleep))


class NoFocusTests(unittest.TestCase):
    def test_typing_not_checked_is_not_a_failure(self):
        backend = FakeOS()
        backend.types = {'left': None}
        clock = Clock()
        logs = []
        # It carries on to the right (whose bootloader never appears in this fake: timeout there).
        with self.assertRaises(TimeoutError) as ctx:
            f.flash({'left': uf2(), 'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 2, stdin=io.StringIO(''),
                    ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
        self.assertIn('right bootloader', str(ctx.exception))
        self.assertTrue(any('typing not checked' in m for m in logs))
        self.assertEqual(len(backend.writes), 1)


class RelayWaitTests(unittest.TestCase):
    def test_right_entry_waits_for_the_relay(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.auto = True
        backend.links = [0, 1, 1, 1, 2]  # relay comes back on the fifth poll
        clock = Clock()
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 2, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=lambda m: None, clock=clock.clock, sleep=clock.sleep, yes=True)
        self.assertEqual(backend.at_write['link_polls'], 5)
        self.assertEqual(backend.entered, ['right'], 'asked once, after the relay was up')


class RelayEventTests(unittest.TestCase):
    """Firmware with relay events (HELLO bit 20): the flasher waits on the relay-ready event
    (D6 85) instead of asking HELLO every second (#594)."""

    def run_right(self, backend, logs):
        clock = Clock()
        backend.clock = clock
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 2, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
        return clock

    def test_first_request_waits_on_the_event_not_the_poll(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.auto = True
        backend.relay = [True, True]
        self.run_right(backend, [])
        # Before the D7 08, and again before the typing check (#600).
        self.assertEqual(backend.relay_waits, [f.RELAY_WAIT_SECONDS] * 2)
        self.assertEqual(backend.link_polls, 0, 'no HELLO polling')
        self.assertEqual(backend.entered, ['right'])
        self.assertEqual(len(backend.writes), 1)

    def test_no_event_in_time_still_asks_once(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.auto = True
        backend.relay = [False, True]
        logs = []
        self.run_right(backend, logs)
        self.assertTrue(any('trying anyway' in m for m in logs), logs)
        self.assertEqual(backend.link_polls, 0)
        self.assertEqual(backend.entered, ['right'])

    def test_each_retry_waits_for_the_next_relay_ready_event(self):
        # #545: the first request can't be relayed; the relay comes back, the second lands.
        backend = FakeOS(['35B4541F78DE9CF8'])
        answers = [False, True]
        backend.enter_bootloader = lambda half, log: (backend.entered.append(half), answers.pop(0))[1]
        real = backend.enumerate_bootloaders
        backend.enumerate_bootloaders = lambda: real() if not answers else []
        backend.relay = [True, True, True]
        logs = []
        self.run_right(backend, logs)
        self.assertEqual(backend.entered, ['right'] * 2)
        self.assertEqual(len(backend.at_write['relay_waits']), 2)
        self.assertEqual(len(backend.relay_waits), 3, 'and one before the typing check (#600)')
        self.assertLessEqual(backend.relay_waits[1], f.ENTRY_RETRY_SECONDS - f.ENTRY_RETRY_PAUSE)
        self.assertEqual(backend.link_polls, 0)
        self.assertFalse(any(m.startswith('Double-tap reset on') or 'late entry' in m for m in logs), logs)
        self.assertEqual(len(backend.writes), 1)

    def test_retries_end_when_the_event_never_comes(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.relay = [False]
        clock = Clock()
        backend.clock = clock
        backend.enumerate_bootloaders = lambda: []
        entered, attempted = f.enter_with_retries('right', backend, lambda m: None, clock.clock, clock.sleep)
        self.assertEqual((entered, attempted), (False, True))
        self.assertEqual(backend.entered, ['right'], 'one ask, then one bounded wait')
        self.assertEqual(backend.relay_waits, [f.ENTRY_RETRY_SECONDS - f.ENTRY_RETRY_PAUSE])
        self.assertLessEqual(clock.now, f.ENTRY_RETRY_SECONDS + 1)

    def test_a_drive_after_the_pause_skips_the_wait(self):
        # #585: an unconfirmed send over ZMK's split did reboot the right.
        backend = FakeOS(['35B4541F78DE9CF8'], stranded=True)  # the lambda below sets the timing
        clock = Clock()
        backend.clock = clock
        real = backend.enumerate_bootloaders
        backend.enumerate_bootloaders = lambda: real() if clock.now >= f.ENTRY_RETRY_PAUSE else []
        entered, attempted = f.enter_with_retries('right', backend, lambda m: None, clock.clock, clock.sleep)
        self.assertEqual((entered, attempted), (False, True))
        self.assertEqual(backend.relay_waits, [])

    def test_windows_backend_maps_relay_wait_exit_codes(self):
        backend = f.WindowsOS()
        done = lambda code: unittest.mock.Mock(returncode=code, stdout='', stderr='')
        with patch.object(f, 'lab_binary', return_value=Path('foxglove-lab.exe')):
            with patch.object(f.subprocess, 'run', return_value=done(0)) as run:
                self.assertIs(backend.wait_relay_ready(12.2), True)
                self.assertEqual(run.call_args.args[0][-3:], ['relay-wait', '--timeout-s', '13'])
            with patch.object(f.subprocess, 'run', return_value=done(3)):
                self.assertIs(backend.wait_relay_ready(5), False)
            with patch.object(f.subprocess, 'run', return_value=done(1)):
                self.assertIsNone(backend.wait_relay_ready(5))
            with patch.object(f.subprocess, 'run', return_value=done(4)):
                self.assertIsNone(backend.wait_relay_ready(5))
            with patch.object(f.subprocess, 'run', side_effect=AssertionError('asked again')):
                self.assertIsNone(backend.wait_relay_ready(5), 'no relay events: not asked again')
        with patch.object(f, 'lab_binary', return_value=None):
            self.assertIsNone(f.WindowsOS().wait_relay_ready(5))


class RightTypingRelayTests(unittest.TestCase):
    """#600: the right's typing check waits for the relay (the relay-ready event, #594) instead of
    a fixed 20 s sleep, and a check with no reply is tried once more after the next relay-ready
    wait before the #122 hint. A fake relay comes up RELAY_DELAY s after the first wait starts;
    the fake keyboard only answers once it is up. No hardware."""
    RELAY_DELAY = 35

    def setUp(self):
        self.clock = Clock()
        self.events = []
        self.relay_up_at = None
        self.backend = FakeOS(['35B4541F78DE9CF8'])
        self.backend.clock = self.clock
        self.backend.wait_relay_ready = self.fake_relay_wait
        self.backend.typing_check = self.fake_typing
        self.replies = None  # None: answer when the relay is up; else a list of answers
        self.logs = []

    def fake_relay_wait(self, seconds):
        self.events.append(('relay-wait', self.clock.now))
        if self.relay_up_at is None:
            self.relay_up_at = self.clock.now + self.RELAY_DELAY
        if self.relay_up_at - self.clock.now > seconds:
            self.clock.now += seconds
            return False
        self.clock.now = max(self.clock.now, self.relay_up_at)  # blocks until the event
        return True

    def fake_typing(self, half, log):
        self.events.append(('typing', self.clock.now))
        if self.replies is not None:
            return self.replies.pop(0)
        return self.relay_up_at is not None and self.clock.now >= self.relay_up_at

    def check(self):
        return f.check_right_typing(self.backend, self.logs.append, self.clock.clock, self.clock.sleep)

    def hinted(self):
        return any('#122' in m for m in self.logs)

    def test_waits_for_the_relay_ready_event_not_a_fixed_sleep(self):
        self.assertIs(self.check(), True)
        self.assertEqual(self.events, [('relay-wait', 0), ('typing', self.RELAY_DELAY)])
        self.assertFalse(self.hinted(), self.logs)

    def test_relay_already_up_checks_at_once(self):
        self.relay_up_at = 0
        self.assertIs(self.check(), True)
        self.assertEqual(self.events, [('relay-wait', 0), ('typing', 0)], 'no 20 s sleep')

    def test_no_reply_is_retried_once_after_the_next_relay_ready(self):
        self.replies = [False, True]
        self.assertIs(self.check(), True)
        self.assertEqual([e for e, _ in self.events], ['relay-wait', 'typing', 'relay-wait', 'typing'])
        self.assertFalse(self.hinted(), self.logs)

    def test_two_no_replies_give_the_122_hint_not_a_failure(self):
        self.replies = [False, False]
        self.assertIs(self.check(), False)
        self.assertEqual([e for e, _ in self.events], ['relay-wait', 'typing', 'relay-wait', 'typing'])
        self.assertTrue(self.hinted(), self.logs)

    def test_no_focus_is_not_retried(self):
        self.replies = [None]
        self.assertIsNone(self.check())
        self.assertEqual([e for e, _ in self.events], ['relay-wait', 'typing'])
        self.assertFalse(self.hinted())

    def test_relay_wait_is_bounded(self):
        self.RELAY_DELAY = 10 * f.RELAY_WAIT_SECONDS  # never in time
        self.assertIs(self.check(), False)
        self.assertEqual(self.clock.now, 2 * f.RELAY_WAIT_SECONDS, 'two bounded waits, then the hint')
        self.assertTrue(any('not up after' in m for m in self.logs))
        self.assertTrue(self.hinted())

    def test_older_firmware_falls_back_to_the_hello_wait(self):
        backend = FakeOS()  # wait_relay_ready answers None: no relay events
        backend.links = [0, 1, 2]
        typed = f.check_right_typing(backend, self.logs.append, self.clock.clock, self.clock.sleep)
        self.assertIs(typed, True)
        self.assertEqual(backend.link_polls, 3)
        self.assertEqual(backend.typing, ['right'])

    def test_flash_runs_it_after_the_right_is_written(self):
        self.backend.auto = True
        self.relay_up_at = 0  # up for the D7 08
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, self.backend, 2, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=self.logs.append, clock=self.clock.clock,
                sleep=self.clock.sleep, yes=True)
        self.assertEqual([e for e, _ in self.events], ['relay-wait', 'relay-wait', 'typing'])
        self.assertEqual(len(self.backend.writes), 1)
        self.assertFalse(self.hinted(), self.logs)


class EntryRetryTests(unittest.TestCase):
    def test_right_request_lands_once_the_split_relinks(self):
        # #545: the left can't relay D7 08 at first; the split drops and comes back, then it can.
        backend = FakeOS(['35B4541F78DE9CF8'])
        answers = [False, False, True]
        backend.enter_bootloader = lambda half, log: (backend.entered.append(half), answers.pop(0))[1]
        real = backend.enumerate_bootloaders
        backend.enumerate_bootloaders = lambda: real() if not answers else []  # once it's asked
        backend.links = [2, 0, 1, 2, 2]  # up before the first ask, then drops and re-links
        logs = []
        clock = Clock()
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 2, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
        self.assertEqual(backend.entered, ['right'] * 3)
        self.assertEqual(backend.at_write['link_polls'], 5, 'each retry waits for the link')
        self.assertFalse(any(m.startswith('Double-tap reset on') or 'late entry' in m for m in logs), logs)
        self.assertEqual(len(backend.writes), 1)

    def test_a_late_bootloader_ends_the_retries_without_a_prompt(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.links = [2] + [0] * 1000  # up for the first ask, then down for good
        real = backend.enumerate_bootloaders
        clock = Clock()
        backend.clock = clock
        backend.enumerate_bootloaders = lambda: real() if clock.now >= 10 else []  # lands ~10 s in
        logs = []
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 2, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
        self.assertEqual(backend.entered, ['right'], 'no retry once the drive is there')
        self.assertLess(backend.at_write['now'], 40)
        self.assertFalse(any(m.startswith('Double-tap reset on') for m in logs), logs)
        self.assertEqual(len(backend.writes), 1)

    def test_retries_stop_when_the_split_stays_down(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.links = [0] * 1000  # the split never re-links after the first ask
        clock = Clock()
        entered, attempted = f.enter_with_retries('right', backend, lambda m: None, clock.clock, clock.sleep)
        self.assertEqual((entered, attempted), (False, True))
        self.assertEqual(backend.entered, ['right'])
        self.assertLessEqual(clock.now, f.ENTRY_RETRY_SECONDS + 1)


class LateEntryTests(unittest.TestCase):
    def test_late_bootloader_after_unanswered_request_never_prompts(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        backend.auto = False  # D7 08 got no reply...
        real = backend.enumerate_bootloaders
        polls = {'n': 0}
        def late():
            polls['n'] += 1
            return real() if polls['n'] > 5 else []  # ...but the bootloader shows up ~5 s later
        backend.enumerate_bootloaders = late
        logs = []
        clock = Clock()
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 2, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
        self.assertEqual(len(backend.writes), 1)
        self.assertFalse(any(m.startswith('Double-tap reset on the right') for m in logs), logs)
        self.assertTrue(any('late entry' in m for m in logs))

    def test_prompts_only_after_the_watch_window(self):
        backend = FakeOS(['35B4541F78DE9CF8'])
        real = backend.enumerate_bootloaders
        clock = Clock()
        # Beyond the retries and the 15 s watch: the user double-taps.
        after = f.ENTRY_RETRY_SECONDS + f.LATE_ENTRY_SECONDS + 5
        backend.enumerate_bootloaders = lambda: real() if clock.now > after else []
        logs = []
        f.flash({'right': uf2(b'FOXGLOVE_HALF=right')}, backend, 60, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep, yes=True)
        watch = next(i for i, m in enumerate(logs) if 'late entry' in m)
        prompt = next(i for i, m in enumerate(logs) if m.startswith('Double-tap reset on the right'))
        self.assertLess(watch, prompt)
        self.assertEqual(len(backend.writes), 1)


LEFT, RIGHT = '1E89EF085AA8A635', '35B4541F78DE9CF8'


class FindVolumeTests(unittest.TestCase):
    """#654: one slow CIM query right after the drive arrived ended a run with the right half
    left in its bootloader. The lookup is retried, each try bounded and logged, with an
    INFO_UF2.TXT fallback scoped by the serial's disk."""
    def lookup(self, replies, exists=True):
        logs, sleeps, calls = [], [], []
        backend = f.WindowsOS(log=logs.append, sleep=sleeps.append)
        replies = list(replies)
        def ps(script, timeout=20):
            calls.append((script, timeout))
            reply = replies.pop(0)
            if isinstance(reply, BaseException):
                raise reply
            return reply
        with patch.object(backend, '_ps', side_effect=ps), patch.object(backend, 'exists', return_value=exists):
            try:
                return backend.find_volume(RIGHT), logs, sleeps, calls
            except Exception as exc:
                exc.logs, exc.sleeps, exc.calls = logs, sleeps, calls
                raise

    def timeout(self):
        return f.subprocess.TimeoutExpired(['powershell'], f.FIND_TIMEOUT)

    def test_a_lookup_timeout_then_success(self):
        volume, logs, sleeps, calls = self.lookup([self.timeout(), [], [{'Disk': 'd', 'Volume': 'X:'}]])
        self.assertEqual(volume, Path('X:\\'))
        self.assertTrue(any('drive lookup 1/3 (CIM)' in m and 'timed out' in m for m in logs), logs)
        self.assertTrue(any('drive lookup 1/3 (INFO_UF2.TXT)' in m and 'no volume yet' in m for m in logs), logs)
        self.assertTrue(any('drive found at' in m and 'try 2/3' in m for m in logs), logs)
        self.assertEqual(sleeps, [f.FIND_PAUSE])
        self.assertTrue(all(t == f.FIND_TIMEOUT for _, t in calls), 'every try is bounded')

    def test_fallback_finds_the_uf2_volume_on_the_serials_disk(self):
        volume, logs, _, calls = self.lookup([self.timeout(), [{'Disk': '3', 'Volume': 'E:', 'Label': 'NICENANO'}]])
        self.assertEqual(volume, Path('E:\\'))
        script = calls[1][0]
        self.assertIn('INFO_UF2.TXT', script)
        self.assertIn(RIGHT, script, "scoped to this half's serial")
        self.assertNotIn(LEFT, script)
        self.assertTrue(any('(INFO_UF2.TXT, try 1/3)' in m for m in logs), logs)

    def test_every_try_times_out_bounded(self):
        with self.assertRaises(TimeoutError) as ctx:
            self.lookup([self.timeout()] * (2 * f.FIND_TRIES))
        self.assertEqual(len(ctx.exception.calls), 2 * f.FIND_TRIES)
        self.assertEqual(sum('timed out' in m for m in ctx.exception.logs), 2 * f.FIND_TRIES)
        self.assertEqual(ctx.exception.sleeps, [f.FIND_PAUSE] * (f.FIND_TRIES - 1))

    def test_ambiguous_mapping_refuses_at_once(self):
        with self.assertRaises(f.Refused) as ctx:
            self.lookup([self.timeout(), [{'Volume': 'X:'}, {'Volume': 'Y:'}]])
        self.assertEqual(len(ctx.exception.calls), 2, 'never retried into a guess')

    def test_no_volume_after_every_try_refuses(self):
        with self.assertRaises(f.Refused):
            self.lookup([[]] * (2 * f.FIND_TRIES))

    def test_volume_without_info_uf2_refuses(self):
        with self.assertRaises(f.Refused):
            self.lookup([[{'Volume': 'X:'}]], exists=False)


class BootloaderOS(FakeOS):
    """Bootloaders that come and go: asking a half in adds its serial; a write the bootloader
    takes removes it. Each half's drive is its own fake volume. `fail_lookup`: serials whose
    drive lookup times out once (#654)."""
    def __init__(self, present=()):
        super().__init__(stranded=True)
        self.present_serials = list(present)
        self.serial_of = {half: serial for serial, half in f.SERIALS.items()}
        self.auto = True
        self.at_enter = []
        self.lookups = []
        self.fail_lookup = set()

    def enumerate_bootloaders(self):
        return list(self.present_serials)

    def enter_bootloader(self, half, log):
        self.entered.append(half)
        self.at_enter.append(list(self.present_serials))
        self.present_serials.append(self.serial_of[half])
        return True

    def find_volume(self, serial):
        self.lookups.append(serial)
        if serial in self.fail_lookup:
            self.fail_lookup.discard(serial)
            raise TimeoutError(f'the drive lookup for {serial} timed out {f.FIND_TRIES} times')
        return Path('fake-' + serial)

    def exists(self, path):
        return Path(path).parts[0][len('fake-'):] in self.present_serials

    def write_file(self, path, data):
        serial = Path(path).parts[0][len('fake-'):]
        self.writes.append((serial, data))
        self.present_serials.remove(serial)


class ResumeTests(unittest.TestCase):
    """#654: a half already in its bootloader is written first, scoped to its serial, and no
    half is asked into its bootloader while another one's is unresolved."""
    IMAGES = {'left': uf2(b'FOXGLOVE_HALF=left'), 'right': uf2(b'FOXGLOVE_HALF=right')}

    def run_yes(self, backend, images=None, **kwargs):
        clock = Clock()
        logs = []
        f.flash(images or dict(self.IMAGES), backend, 120, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep,
                yes=True, **kwargs)
        return logs

    def assertNeverTwoOpen(self, backend):
        self.assertTrue(all(not open_ for open_ in backend.at_enter),
                        f'a half was asked in while a bootloader was open: {backend.at_enter}')

    def written(self, backend):
        return [(serial, f.SERIALS[serial], data) for serial, data in backend.writes]

    def test_stranded_right_then_half_both(self):
        # The incident: run 1 entered the right, then its drive lookup timed out.
        backend = BootloaderOS()
        backend.fail_lookup = {RIGHT}
        with self.assertRaises(TimeoutError):
            self.run_yes(backend)
        self.assertEqual(backend.present_serials, [RIGHT], 'the right is stranded in its bootloader')
        self.assertEqual([s for s, _ in backend.writes], [LEFT])
        # Run 2, --half both: the stranded right is written first; only then is the left asked in.
        backend.writes, backend.entered, backend.at_enter = [], [], []
        logs = self.run_yes(backend)
        self.assertEqual([(s, h) for s, h, _ in self.written(backend)], [(RIGHT, 'right'), (LEFT, 'left')])
        self.assertEqual([d for _, _, d in self.written(backend)], [self.IMAGES['right'], self.IMAGES['left']])
        self.assertEqual(backend.entered, ['left'], 'the right was not asked again')
        self.assertNeverTwoOpen(backend)
        self.assertEqual(backend.present_serials, [])
        self.assertTrue(any(m.startswith('right: already in its bootloader') for m in logs), logs)
        self.assertFalse(any('Unplug one' in m for m in logs), logs)

    def test_two_known_bootloaders_are_written_one_at_a_time(self):
        backend = BootloaderOS([RIGHT, LEFT])
        logs = self.run_yes(backend)
        self.assertEqual([(s, h) for s, h, _ in self.written(backend)], [(LEFT, 'left'), (RIGHT, 'right')])
        self.assertEqual([d for _, _, d in self.written(backend)], [self.IMAGES['left'], self.IMAGES['right']])
        self.assertEqual(backend.entered, [], 'no half asked in')
        self.assertEqual(set(backend.lookups[:2]), {LEFT}, 'the left drive found by its own serial')
        self.assertTrue(any('writing the left one only' in m for m in logs), logs)
        self.assertEqual(backend.present_serials, [])

    def test_unknown_serial_refuses_before_anything(self):
        for present in (['DEADBEEF00000000'], [RIGHT, 'DEADBEEF00000000']):
            with self.subTest(present=present):
                backend = BootloaderOS(present)
                with self.assertRaisesRegex(f.Refused, 'Unknown bootloader serial'):
                    self.run_yes(backend)
                self.assertEqual(backend.writes, [])
                self.assertEqual(backend.entered, [])
                self.assertEqual(backend.lookups, [])

    def test_stranded_half_without_its_image_asks_no_other_half_in(self):
        backend = BootloaderOS([RIGHT])
        with self.assertRaisesRegex(f.Refused, '--half right'):
            self.run_yes(backend, {'left': self.IMAGES['left']})
        self.assertEqual(backend.entered, [])
        self.assertEqual(backend.writes, [])

    def test_resume_writes_only_the_stranded_half(self):
        backend = BootloaderOS([RIGHT])
        self.run_yes(backend, resume=True)
        self.assertEqual([h for _, h, _ in self.written(backend)], ['right'])
        self.assertEqual(backend.entered, [])

    def test_resume_with_nothing_stranded_touches_nothing(self):
        backend = BootloaderOS()
        logs = self.run_yes(backend, resume=True)
        self.assertEqual((backend.writes, backend.entered), ([], []))
        self.assertTrue(any('Nothing to resume' in m for m in logs), logs)

    def test_resume_keeps_the_image_half_check(self):
        backend = BootloaderOS([RIGHT])
        with self.assertRaises(f.Refused):
            self.run_yes(backend, {'right': uf2(b'FOXGLOVE_HALF=left')}, resume=True)
        self.assertEqual(backend.writes, [])

    def test_resume_cli_dry_run_touches_no_device(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = {}
            for half in ('left', 'right'):
                paths[half] = Path(directory) / f'{half}.uf2'
                paths[half].write_bytes(self.IMAGES[half])
            with patch.object(f, 'WindowsOS', side_effect=AssertionError('device access')), \
                    patch('sys.stdout', new_callable=io.StringIO):
                self.assertEqual(f.main(['--left', str(paths['left']), '--right', str(paths['right']),
                                         '--resume', '--dry-run']), 0)
            with patch.object(f, 'FlashLock'), patch.object(f, 'flash') as flash, patch.object(f, 'WindowsOS'), \
                    patch('sys.stdout', new_callable=io.StringIO):
                f.main(['--left', str(paths['left']), '--half', 'left', '--resume', '--yes'])
                self.assertTrue(flash.call_args.kwargs['resume'])


class UsbPresenceTests(unittest.TestCase):
    """#654, second incident: a half is asked into its bootloader only while its running
    firmware's USB device (1D50:615E with its serial, any status) is present, because the
    bootloader can only be reached over USB. Otherwise the run refuses and changes nothing."""
    IMAGES = ResumeTests.IMAGES

    def run_yes(self, backend, images=None, **kwargs):
        clock = Clock()
        logs = []
        f.flash(images or dict(self.IMAGES), backend, 120, stdin=io.StringIO(''),
                ask=lambda p: self.fail(p), log=logs.append, clock=clock.clock, sleep=clock.sleep,
                yes=True, **kwargs)
        return logs

    def refused(self, backend, images=None):
        logs = []
        with self.assertRaisesRegex(f.Refused, r"isn't on USB; plug its cable in, then run again") as caught:
            logs = self.run_yes(backend, images)
        return str(caught.exception), logs

    def test_right_not_on_usb_refuses_before_any_entry(self):
        for images in (dict(self.IMAGES), {'right': self.IMAGES['right']}):
            with self.subTest(halves=sorted(images)):
                backend = BootloaderOS()
                backend.usb['right'] = None
                message, _ = self.refused(backend, images)
                self.assertIn('the right half', message)
                self.assertEqual(backend.entered, [], 'no half asked in, not even the left')
                self.assertEqual(backend.writes, [])

    def test_right_present_with_problem_28_proceeds(self):
        backend = BootloaderOS()
        logs = self.run_yes(backend)
        self.assertEqual(backend.entered, ['left', 'right'])
        self.assertEqual([h for _, h, _ in ResumeTests.written(self, backend)], ['left', 'right'])
        self.assertIn(f'right: on USB ({f.APP_DEVICE}{RIGHT}, status Error)', logs)
        self.assertIn(f'left: on USB ({f.APP_DEVICE}{LEFT}, status OK)', logs)

    def test_left_not_on_usb_refuses_before_entry(self):
        for images in (dict(self.IMAGES), {'left': self.IMAGES['left']}):
            with self.subTest(halves=sorted(images)):
                backend = BootloaderOS()
                backend.usb['left'] = None
                message, _ = self.refused(backend, images)
                self.assertIn('the left half', message)
                self.assertEqual((backend.entered, backend.writes), ([], []))

    def test_left_present_proceeds(self):
        backend = BootloaderOS()
        self.run_yes(backend, {'left': self.IMAGES['left']})
        self.assertEqual(backend.entered, ['left'])
        self.assertEqual(len(backend.writes), 1)

    def test_summary_logs_a_missing_half_even_when_not_flashing_it(self):
        backend = BootloaderOS()
        backend.usb['right'] = None
        logs = self.run_yes(backend, {'left': self.IMAGES['left']})
        self.assertIn(f'right: not on USB (no {f.APP_DEVICE}{RIGHT} present)', logs)
        self.assertEqual(backend.entered, ['left'])

    def test_right_dropping_off_usb_after_the_left_is_checked_again(self):
        backend = BootloaderOS()
        write = backend.write_file
        def write_then_drop(path, data):
            write(path, data)
            backend.usb['right'] = None  # the right's port went away while the left was flashed
        backend.write_file = write_then_drop
        message, _ = self.refused(backend)
        self.assertIn('the right half', message)
        self.assertEqual(backend.entered, ['left'], 'the right was not asked in')
        self.assertEqual([s for s, _ in backend.writes], [LEFT])

    def test_a_stranded_half_needs_no_app_device(self):
        # In its bootloader the half has no 1D50:615E device; it's written, not asked in.
        backend = BootloaderOS([RIGHT])
        backend.usb['right'] = None
        self.run_yes(backend, {'right': self.IMAGES['right']})
        self.assertEqual(backend.entered, [])
        self.assertEqual([s for s, _ in backend.writes], [RIGHT])

    def test_settings_reset_double_tap_is_not_gated(self):
        backend = FakeOS()
        backend.usb['left'] = None
        FlashTests.run_flash(self, backend, ('FLASH', 'ERASE SETTINGS'), {'left': uf2(b'SETTINGS RESET')},
                             settings_reset=True)
        self.assertEqual(len(backend.writes), 1)

    def test_windows_backend_reads_any_status_present_only(self):
        backend = f.WindowsOS(log=lambda m: None)
        scripts = []
        with patch.object(backend, '_ps', side_effect=lambda s, **k: (scripts.append(s), ['Error'])[1]):
            self.assertEqual(backend.usb_device(RIGHT), 'Error')
        self.assertIn('-PresentOnly', scripts[0])
        self.assertIn(f"-eq '{f.APP_DEVICE}{RIGHT}'", scripts[0])
        self.assertEqual(f.APP_DEVICE, 'USB' + chr(92) + 'VID_1D50&PID_615E' + chr(92))
        self.assertNotIn("Status -eq 'OK'", scripts[0])
        with patch.object(backend, '_ps', return_value=[]):
            self.assertIsNone(backend.usb_device(RIGHT))

    def test_a_failed_presence_query_refuses_untouched(self):
        backend = BootloaderOS()
        def broken(serial):
            raise f.subprocess.CalledProcessError(1, 'powershell')
        backend.usb_device = broken
        with self.assertRaisesRegex(f.Refused, "couldn't check whether the left half is on USB"):
            self.run_yes(backend)
        self.assertEqual((backend.entered, backend.writes), ([], []))
