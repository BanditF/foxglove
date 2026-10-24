"""Safely flash nice!nano v2 halves (Python 3.12, no dependencies).

python scripts/flash/foxglove_flash.py --firmware DIR --dry-run
python scripts/flash/foxglove_flash.py --left L.uf2 --right R.uf2
Real writes require a terminal and typing FLASH. When the running firmware supports it
(HELLO bit 9, #111) each half is put into its bootloader over the link; otherwise double-tap
reset when asked.
Settings reset: supply its path with --left/--right, select --half, and add
--settings-reset; this erases settings and requires a separate confirmation.
"""
import argparse
import re
from dataclasses import asdict, dataclass
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
import time

SERIALS = {'1E89EF085AA8A635': 'left', '35B4541F78DE9CF8': 'right'}
LIMIT = 0xAD000
SETTLE_SECONDS = 3  # after the bootloader drive appears, before writing
START = 0x26000  # All inspected real builds, including settings reset, start here.
ROLLBACK = r'C:\Users\fuchs\splitkeeb\firmware-358c3d0\firmware'
MARKER = re.compile(rb'FOXGLOVE_HALF=(?P<half>left|right)'
                    rb'(?:;VER=(?P<ver>[0-9A-Za-z.+-]{1,32}))?'
                    rb'(?:;PROFILE=(?P<profile>dev|ga))?'
                    rb'(?:;SHA=(?P<sha>[0-9a-f]{7,40}))?')
FILENAMES = {'left': 'eyelash_corne_studio_left.uf2',
             'right': 'nice_view_custom-eyelash_corne_right-zmk.uf2'}
# Decoded address-ordered payloads of firmware-358c3d0 and other local builds:
# require both stable feature names, never compiler paths, hashes, or absence alone.
# Reset also contains raw HID events, so detect SETTINGS RESET before this table.
HALF_MARKERS = {'left': (b'studio_rpc_thread', b'raw_hid_received_event'),
                'right': (b'Split Peripheral Notification Queue', b'ble_peripheral\x00')}


@dataclass
class Preflight:
    ok: bool
    reasons: list[str]
    end_address: int = 0
    headroom_kib: float = 0
    detected_half: str = 'unknown'
    # From the firmware's marker (#79/#133): FOXGLOVE_HALF=left;VER=0.1.0;PROFILE=dev;SHA=abc1234
    version: str = ''
    profile: str = ''
    sha: str = ''


def firmware_file(folder, half, profile):
    """The UF2 for `half` in a firmware folder: CI's foxglove-fw-<ver>-<sha>-<half>-<profile>.uf2
    (#133) when present, else the older fixed names."""
    named = sorted(Path(folder).glob(f'foxglove-fw-*-{half}-{profile}.uf2'))
    if len(named) > 1:
        raise Refused(f'more than one {half} {profile} UF2 in {folder}; pass --{half} explicitly')
    return named[0] if named else Path(folder) / FILENAMES[half]


def preflight(data, half, allow_unknown_half=False, settings_reset=False):
    """Validate all UF2 blocks, then inspect a bounded flat flash image."""
    reasons = []
    end = 0
    detected = 'unknown'
    if not data or len(data) % 512:
        return Preflight(False, ['UF2 must contain whole 512-byte blocks'])
    count = len(data) // 512
    numbers = []
    image = bytearray(b'\xff' * (LIMIT - START))
    ranges = []
    for offset in range(0, len(data), 512):
        block = data[offset:offset + 512]
        m0, m1, flags, addr, size, number, total, family = struct.unpack('<8I', block[:32])
        label = f'block {offset // 512}'
        if (m0, m1, struct.unpack('<I', block[508:])[0]) != (0x0A324655, 0x9E5D5157, 0x0AB16F30):
            reasons.append(f'{label}: bad magic')
        if not flags & 0x2000 or family != 0xADA52840:
            reasons.append(f'{label}: missing/wrong nRF52840 family')
        if flags & ~0x2000:
            reasons.append(f'{label}: unsupported UF2 flags')
        if total != count:
            reasons.append(f'{label}: inconsistent numBlocks')
        numbers.append(number)
        end = max(end, addr + size)
        if size > 476 or size == 0:
            reasons.append(f'{label}: invalid payload size {size}')
        if addr < START or addr + size > LIMIT:
            reasons.append(f'{label}: address outside 0x{START:X}..0x{LIMIT:X}')
        elif 0 < size <= 476:
            ranges.append((addr, addr + size))
            image[addr - START:addr - START + size] = block[32:32 + size]
    if sorted(numbers) != list(range(count)):
        reasons.append('block numbers must be exactly 0..numBlocks-1')
    ordered = sorted(ranges)
    if any(a[1] > b[0] for a, b in zip(ordered, ordered[1:])):
        reasons.append('overlapping flash payloads')
    # Searched in the decoded flat image, so a marker split across UF2 blocks is still found.
    markers = [m.groupdict() for m in MARKER.finditer(bytes(image))]
    explicit = {m['half'].decode() for m in markers}
    reset = b'SETTINGS RESET' in image
    if reset:
        detected = 'settings_reset'
        if not settings_reset:
            reasons.append('settings reset requires --settings-reset (erases saved settings)')
        if explicit:
            reasons.append('conflicting settings-reset and half markers')
    elif settings_reset:
        reasons.append('--settings-reset requires an identified SETTINGS RESET image')
    elif explicit:
        detected = next(iter(explicit)) if len(explicit) == 1 else 'conflicting'
        if explicit != {half}:
            reasons.append(f'explicit half marker does not match {half}')
    else:
        matches = [h for h, markers in HALF_MARKERS.items() if all(m in image for m in markers)]
        if len(matches) == 1:
            detected = matches[0]
            if detected != half:
                reasons.append(f'image is for {detected}, not {half}')
        elif not allow_unknown_half:
            reasons.append('half heuristic inconclusive; use --allow-unknown-half only after checking the build')
    info = markers[0] if len(markers) == 1 else {}
    text = lambda key: (info.get(key) or b'').decode()
    return Preflight(not reasons, reasons, end, (LIMIT - end) / 1024, detected,
                     text('ver'), text('profile'), text('sha'))


class Refused(Exception):
    pass


class FlashFailed(Exception):
    pass


def lab_binary():
    """foxglove-lab, which sends the bootloader command; None when it isn't built."""
    root = Path(__file__).resolve().parents[2]
    target = Path(os.environ.get('CARGO_TARGET_DIR') or root / 'target')
    for build in ('release', 'debug'):
        exe = target / build / ('foxglove-lab.exe' if os.name == 'nt' else 'foxglove-lab')
        if exe.exists():
            return exe
    return None


class WindowsOS:
    """OS boundary. One CIM query maps serial -> disk -> partition -> logical disk."""
    def build_info(self, half):
        """The running build of `half` from the keyboard (D7 09) as a dict with version, profile
        and sha, or None when it can't tell (old firmware, right not reachable, no foxglove-lab)."""
        exe = lab_binary()
        if exe is None:
            return None
        try:
            result = subprocess.run([str(exe), '--allow-app', 'build', '--half', half],
                                    capture_output=True, text=True, timeout=20)
        except subprocess.TimeoutExpired:
            return None
        if result.returncode != 0:
            return None
        try:
            return json.loads(result.stdout.strip().splitlines()[-1])
        except (ValueError, IndexError):
            return None
    def enter_bootloader(self, half, log):
        """Asks the running firmware to reboot `half` into its bootloader (D7 08, the right
        through the left's relay). True when asked; False means double-tap instead."""
        exe = lab_binary()
        if exe is None:
            log('foxglove-lab is not built (cargo build --release -p foxglove-lab), so no hands-free entry.')
            return False
        result = subprocess.run([str(exe), '--allow-app', 'bootloader', '--half', half, '--confirmed', 'FLASH'],
                                capture_output=True, text=True, timeout=30)
        if result.returncode == 0:
            return True
        reason = (result.stderr or result.stdout).strip().splitlines()
        log(f'{half}: hands-free entry unavailable ({reason[-1] if reason else result.returncode}).')
        return False
    def _ps(self, script):
        result = subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-Command',
                                 "$ErrorActionPreference='Stop'; " + script],
                                capture_output=True, text=True, timeout=20, check=True)
        return json.loads(result.stdout.strip() or '[]')

    def enumerate_bootloaders(self):
        return self._ps(r"ConvertTo-Json -Compress -InputObject @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\VID_239A&PID_00B3\*' } | ForEach-Object { $_.InstanceId.Split('\')[-1] })")

    def find_volume(self, serial):
        if serial not in SERIALS:
            raise Refused('Unknown bootloader serial')
        rows = self._ps(r"$disks = @(Get-CimInstance Win32_DiskDrive | Where-Object { $_.PNPDeviceID -match '" + serial + r"' }); ConvertTo-Json -Compress -InputObject @($disks | Where-Object { $disks.Count -eq 1 } | ForEach-Object { $disk = $_; Get-CimAssociatedInstance -InputObject $disk -Association Win32_DiskDriveToDiskPartition | ForEach-Object { Get-CimAssociatedInstance -InputObject $_ -Association Win32_LogicalDiskToPartition | ForEach-Object { [pscustomobject]@{ Disk=$disk.DeviceID; Volume=$_.DeviceID } } } })")
        if len(rows) != 1:
            raise Refused('Cannot map this serial to exactly one disk and volume. Unplug other bootloaders and try again.')
        volume = Path(rows[0]['Volume'] + '\\')
        if not self.exists(volume / 'INFO_UF2.TXT'):
            raise Refused('Mapped volume has no INFO_UF2.TXT')
        return volume

    def exists(self, path):
        return Path(path).exists()

    def read_text(self, path):
        return Path(path).read_text(errors='replace')

    def running_keyboard(self, serial):
        # Match the serial: the right half on USB power also shows up as 1D50:615E (with no
        # interfaces and Windows problem 28), so any 1D50:615E is not proof that the left is back.
        instance = 'USB\\VID_1D50&PID_615E\\' + serial
        return bool(self._ps(r"ConvertTo-Json -Compress -InputObject @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -eq '" + instance + r"' -and $_.Status -eq 'OK' } | ForEach-Object { $_.InstanceId })"))

    def write_file(self, path, data):
        # An error is expected only AFTER the complete last block was accepted.
        written = 0
        try:
            with open(path, 'wb', buffering=0) as stream:
                while written < len(data):
                    n = stream.write(data[written:written + 512])
                    if not n:
                        raise OSError('short write')
                    written += n
                stream.flush()
                os.fsync(stream.fileno())
        except OSError as exc:
            if written < len(data):
                raise FlashFailed(f'Write stopped after {written}/{len(data)} bytes: {exc}') from exc


def wait_for(predicate, seconds, message, clock=time.monotonic, sleep=time.sleep):
    deadline = clock() + seconds
    while True:
        if predicate():
            return
        remaining = deadline - clock()
        if remaining <= 0:
            raise TimeoutError(message)
        sleep(min(1, remaining))


def confirm(prompt, word, stdin, ask):
    if not stdin.isatty():
        raise Refused('Real flashing requires an interactive terminal. Agents may use --dry-run.')
    if ask(prompt).strip() != word:
        raise Refused('Confirmation did not match; nothing further will be written.')


def describe(build):
    return f"{build.get('version') or '?'} {build.get('profile') or '?'} {build.get('sha') or '?'}"


def check_builds(images, backend, log, allow_profile_mix=False, settings_reset=False):
    """Compares each image's marker with what the halves run now (D7 09). Refuses an update that
    would leave one half dev and the other GA, unless allowed. Returns the expected SHA per half."""
    marks = {half: preflight(data, half, settings_reset=settings_reset) for half, data in images.items()}
    running = {half: backend.build_info(half) for half in ('left', 'right')}
    for half in ('left', 'right'):
        now = running[half]
        log(f"{half}: running {describe(now) if now else 'unknown (the firmware does not say)'}"
            + (f"; image {describe({'version': marks[half].version, 'profile': marks[half].profile, 'sha': marks[half].sha})}"
               if half in marks else '; not flashing'))
        if now and half in marks and marks[half].sha and now.get('sha') == marks[half].sha:
            log(f'{half}: already running this exact build.')
    after = {half: (marks[half].profile if half in marks and marks[half].profile else
                    (running[half] or {}).get('profile')) for half in ('left', 'right')}
    if all(after.values()) and after['left'] != after['right'] and not allow_profile_mix and not settings_reset:
        raise Refused(f"this would leave the left {after['left']} and the right {after['right']}; flash both halves "
                      'with the same profile, or pass --allow-profile-mix')
    return {half: m.sha for half, m in marks.items() if m.sha}


def verify_build(half, expected_sha, backend, log, clock=time.monotonic, sleep=time.sleep):
    """After a flash: the half must report the image's SHA (when both sides know it)."""
    if not expected_sha:
        return
    deadline = clock() + 20  # the right answers over the relay once it has reconnected
    while True:
        now = backend.build_info(half)
        if now:
            if now.get('sha') != expected_sha:
                raise FlashFailed(f"{half} runs {describe(now)}, expected SHA {expected_sha}")
            log(f'{half}: verified, running {describe(now)}.')
            return
        if clock() >= deadline:
            log(f"{half}: couldn't read its build back to verify (firmware can't tell, or the right isn't back yet).")
            return
        sleep(2)


def flash(images, backend, seconds, stdin=sys.stdin, ask=input, log=print,
          settings_reset=False, allow_unknown_half=False, clock=time.monotonic, sleep=time.sleep,
          allow_profile_mix=False):
    log('Flash left first, check that it types, then flash right. Double-tap reset only on the requested half.')
    log('Rollback: known-good firmware is in ' + ROLLBACK)
    expected = check_builds(images, backend, log, allow_profile_mix, settings_reset)
    confirm('Type FLASH to continue: ', 'FLASH', stdin, ask)
    if settings_reset:
        confirm('This erases saved settings. Type ERASE SETTINGS to confirm: ', 'ERASE SETTINGS', stdin, ask)
    for half in ('left', 'right'):
        if half not in images:
            continue
        entered = False
        if not settings_reset:
            # The right goes through the left's relay, which needs a few seconds after the left
            # reboots into new firmware: retry before falling back to a double tap.
            for attempt in range(3 if half == 'right' else 1):
                if attempt:
                    sleep(5)
                entered = backend.enter_bootloader(half, log)
                if entered:
                    break
        if entered:
            log(f'Asked the {half} half to enter its bootloader. Waiting up to {seconds:g} seconds for its drive.')
        else:
            log(f'Double-tap reset on the {half} half only. Waiting up to {seconds:g} seconds.')
        chosen = []
        def ready():
            serials = backend.enumerate_bootloaders()
            if any(s not in SERIALS for s in serials):
                raise Refused('Unknown bootloader serial; unplug it before proceeding.')
            if len(serials) > 1:
                log('Two bootloaders are connected. Unplug one; no image will be written yet.')
                return False
            if not serials or SERIALS[serials[0]] != half:
                return False
            chosen[:] = serials
            return True
        wait_for(ready, seconds, f'Timed out waiting for {half} bootloader', clock, sleep)
        volume = backend.find_volume(chosen[0])
        info = volume / 'INFO_UF2.TXT'
        if not backend.exists(info):
            raise Refused('INFO_UF2.TXT disappeared before writing')
        board = [line for line in backend.read_text(info).splitlines() if line.startswith('Board-ID:')]
        if len(board) != 1:
            raise Refused('INFO_UF2.TXT has no unambiguous Board-ID line')
        log(f'{half}: {board[0]}')
        result = preflight(images[half], half, allow_unknown_half, settings_reset)
        if not result.ok:
            raise Refused('; '.join(result.reasons))
        # A UF2 written about 1 s after the drive appears is sometimes ignored by the bootloader
        # (left half, 2026-10-24): let it settle first, and write once more if the drive is
        # still there 10 s later.
        sleep(SETTLE_SECONDS)
        for attempt in (1, 2):
            # Recheck identity immediately before touching the drive.
            if backend.enumerate_bootloaders() != chosen or backend.find_volume(chosen[0]) != volume:
                raise Refused('Bootloader identity or volume changed before writing')
            backend.write_file(volume / 'flash.uf2', images[half])
            try:
                wait_for(lambda: not backend.exists(info), 10 if attempt == 1 else 15,
                         f'{half} drive did not disappear after writing', clock, sleep)
                break
            except TimeoutError:
                if attempt == 2:
                    raise
                log(f'{half}: the bootloader did not take the image; writing it once more.')
        log(f'{half} bootloader drive disappeared.')
        if half == 'left' and not settings_reset:
            wait_for(lambda: backend.running_keyboard(chosen[0]), seconds, 'Left keyboard did not re-enumerate (1D50:615E)', clock, sleep)
        if not settings_reset:
            verify_build(half, expected.get(half), backend, log, clock, sleep)
        if half == 'left' and not settings_reset:
            confirm('Test the left keyboard in a text editor. If it types, enter TYPES here: ', 'TYPES', stdin, ask)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--firmware', type=Path)
    parser.add_argument('--profile', choices=('dev', 'ga'), default='dev',
                        help='which build in a --firmware folder with foxglove-fw-* names (default dev)')
    parser.add_argument('--left', type=Path)
    parser.add_argument('--right', type=Path)
    parser.add_argument('--half', choices=('left', 'right', 'both'), default='both')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--wait-seconds', type=float, default=600)
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--allow-unknown-half', action='store_true')
    parser.add_argument('--settings-reset', action='store_true')
    parser.add_argument('--allow-profile-mix', action='store_true',
                        help='allow leaving one half dev and the other GA')
    args = parser.parse_args(argv)
    if not math.isfinite(args.wait_seconds) or args.wait_seconds <= 0:
        parser.error('--wait-seconds must be positive and finite')
    report = {'images': {}, 'messages': []}
    def log(message):
        report['messages'].append(message)
        if not args.json:
            print(message, flush=True)
    code = 0
    try:
        images = {}
        for half in ('left', 'right'):
            if args.half not in ('both', half):
                continue
            path = getattr(args, half) or (firmware_file(args.firmware, half, args.profile) if args.firmware else None)
            if path is None:
                raise Refused(f'Provide --{half} or --firmware DIR')
            images[half] = path.read_bytes()
            result = preflight(images[half], half, args.allow_unknown_half, args.settings_reset)
            report['images'][half] = asdict(result)
            build = ' '.join(f'{k}={v}' for k, v in (('version', result.version), ('profile', result.profile),
                                                       ('sha', result.sha)) if v)
            log(f'{half}: {path.name}: end 0x{result.end_address:X}, headroom {result.headroom_kib:.1f} KiB; '
                f'half={result.detected_half}' + (f'; {build}' if build else ''))
            for reason in result.reasons:
                log(f'{half}: REFUSED: {reason}')
        if not all(r['ok'] for r in report['images'].values()):
            raise Refused('Preflight failed. No devices were touched.')
        if not args.dry_run:
            def ask_terminal(prompt):
                print(prompt, end='', file=sys.stderr, flush=True)
                return input()
            flash(images, WindowsOS(), args.wait_seconds, log=log, ask=ask_terminal,
                  settings_reset=args.settings_reset, allow_unknown_half=args.allow_unknown_half,
                  allow_profile_mix=args.allow_profile_mix)
    except (TimeoutError, subprocess.TimeoutExpired) as exc:
        code = 2
        log('TIMEOUT: ' + str(exc))
    except (Refused, OSError, ValueError) as exc:
        code = 1
        log('REFUSED: ' + str(exc))
    except (FlashFailed, subprocess.SubprocessError) as exc:
        code = 3
        log('FLASH FAILED: ' + str(exc))
    except (EOFError, KeyboardInterrupt):
        code = 1
        log('Stopped by user.')
    if code:
        log('Rollback: known-good firmware is in ' + ROLLBACK)
    if args.json:
        report['exit_code'] = code
        print(json.dumps(report))
    return code


if __name__ == '__main__':
    sys.exit(main())

