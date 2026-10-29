"""Safely flash nice!nano v2 halves (Python 3.12, no dependencies).

python scripts/flash/foxglove_flash.py --firmware DIR --dry-run
python scripts/flash/foxglove_flash.py --left L.uf2 --right R.uf2
Real writes require a terminal and typing FLASH, unless --yes is given (the user has said go:
no prompts; the automatic checks all stay, and a typing check by foxglove-lab replaces TYPES). When the running firmware supports it
(HELLO bit 9, #111) each half is put into its bootloader over the link; otherwise double-tap
reset when asked.
A half left in its bootloader by an earlier run is written first (#654); --resume writes only
such halves and asks none into its bootloader.
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

# This PC's halves and rollback folder unless tools/local.toml or the environment say otherwise
# (scripts/local_config.py; tools/local.example.toml lists the keys).
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import local_config  # noqa: E402

SERIALS = local_config.serials()
LIMIT = 0xAD000
LATE_ENTRY_SECONDS = 15  # after an unanswered hands-free request, before prompting a double tap
RELAY_WAIT_SECONDS = 60  # before the right's D7 08, for the relay to come back
ENTRY_RETRY_SECONDS = 60  # the right's D7 08 is retried this long, as the split re-links (#545)
ENTRY_RETRY_PAUSE = 5  # between the right's D7 08 attempts
SETTLE_SECONDS = 3  # after the bootloader drive appears, before writing
# The drive lookup right after a new disk arrives can be slow (WMI is busy mounting it; #654):
# up to FIND_TRIES tries of at most FIND_TIMEOUT s each, FIND_PAUSE s apart, each one logged.
FIND_TRIES = 3
FIND_TIMEOUT = 30
FIND_PAUSE = 2
START = 0x26000  # All inspected real builds, including settings reset, start here.
ROLLBACK = local_config.get('flash.rollback')
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


class DriveGone(Exception):
    """The bootloader drive went away during the write. Windows caches the writes, so the
    byte count says nothing about what the bootloader received: it may have taken the whole
    image and reset (incident #176: 'failed' runs that had in fact flashed). Only the half's
    build afterwards tells."""
    def __init__(self, written, total, cause):
        super().__init__(f'drive went away after {written}/{total} bytes were queued: {cause}')
        self.written, self.total = written, total


LOCK = Path(os.environ.get('TEMP') or os.environ.get('TMPDIR') or '.') / 'foxglove-flash.lock'


class FlashLock:
    """Only one flasher at a time: two writers on one bootloader drive is how a UF2 lands on the
    wrong half. The lock file holds the owner's PID; a lock whose process is gone is taken over."""
    def __init__(self, path=None, alive=None):
        path = path or os.environ.get('FOXGLOVE_FLASH_LOCK') or LOCK
        self.path, self.alive, self.held = Path(path), alive or pid_alive, False

    def __enter__(self):
        for _ in range(2):
            try:
                fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            except FileExistsError:
                try:
                    owner = int(self.path.read_text().strip() or 0)
                except (OSError, ValueError):
                    owner = 0
                if owner and self.alive(owner):
                    raise Refused(f'another flasher is running (PID {owner}, lock {self.path}); wait for it to finish')
                self.path.unlink(missing_ok=True)  # stale: its process is gone
                continue
            with os.fdopen(fd, 'w') as fh:
                fh.write(str(os.getpid()))
            self.held = True
            return self
        raise Refused(f'could not take the flash lock {self.path}')

    def __exit__(self, *exc):
        if self.held:
            self.path.unlink(missing_ok=True)


def pid_alive(pid):
    if os.name == 'nt':
        out = subprocess.run(['tasklist', '/FI', f'PID eq {pid}', '/NH'], capture_output=True, text=True).stdout
        return str(pid) in out
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def history_path():
    """Where every write is recorded, one line each (FOXGLOVE_FLASH_HISTORY overrides)."""
    override = os.environ.get('FOXGLOVE_FLASH_HISTORY')
    if override:
        return Path(override)
    base = os.environ.get('LOCALAPPDATA') or os.path.expanduser('~')
    return Path(base) / 'Foxglove' / 'flash-history.log'


def record(half, image_sha, outcome, detail=''):
    """Appends 'UTC time, half, image SHA, outcome, detail' to the flash history. Never fails
    the flash: a history that can't be written is reported, not fatal."""
    line = f"{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\t{half}\t{image_sha or '?'}\t{outcome}\t{detail}\n"
    try:
        path = history_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, 'a', encoding='utf-8') as fh:
            fh.write(line)
    except OSError as exc:
        print(f'flash history not written ({exc}): {line.strip()}', file=sys.stderr)


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
    """OS boundary. A CIM query maps serial -> disk -> partition -> logical disk; a Storage-module
    query (volume with INFO_UF2.TXT -> partition -> disk whose path holds the serial) is the
    fallback."""
    def __init__(self, log=print, sleep=time.sleep):
        self.log, self.sleep = log, sleep

    def typing_check(self, half, log):
        """Has foxglove-lab type into its own window through the left (INVOKE &kp for the left;
        key-position presses on the right half, over the split link, for the right). True only
        when the text arrived."""
        exe = lab_binary()
        if exe is None:
            log('foxglove-lab is not built, so typing cannot be checked automatically.')
            return False
        case = 'typing-abc' if half == 'left' else 'typing-both-halves'
        root = Path(__file__).resolve().parents[2]
        out = Path(os.environ.get('TEMP', root)) / f'foxglove-flash-typing-{half}'
        try:
            result = subprocess.run([str(exe), '--allow-app', 'run', '--suite', 'release', '--only', case,
                                     '--non-interactive', '--out', str(out)],
                                    capture_output=True, text=True, timeout=120, cwd=root)
        except subprocess.TimeoutExpired:
            log(f'{half}: typing check timed out.')
            return False
        line = next((l for l in result.stdout.splitlines() if l.endswith(case)), result.stdout.strip())
        log(f'{half}: typing check: {line or result.returncode}')
        if result.returncode == 0 and line.startswith('PASS'):
            return True
        # The harness window couldn't come to the front (PC locked, someone else's window): the
        # keyboard wasn't tested, which is not the same as the keyboard failing.
        try:
            report = json.loads((out / 'report.json').read_text(encoding='utf-8'))
            message = ' '.join(c.get('message', '') for c in report.get('cases', []) if c.get('name') == case)
        except (OSError, ValueError):
            message = ''
        if 'foreground focus' in message or 'lost foreground focus' in message:
            return None
        return False
    def right_link(self):
        """The left's view of the right (HELLO right link: 0 none, 1 split only, 2 relay up), or
        None when it can't be read (no foxglove-lab, left not connected)."""
        exe = lab_binary()
        if exe is None:
            return None
        try:
            out = subprocess.run([str(exe), '--allow-app', 'status'], capture_output=True, text=True,
                                 timeout=20).stdout
        except subprocess.TimeoutExpired:
            return None
        found = re.search(r'"right_link":(\d+)', out)
        return int(found.group(1)) if found else None

    def wait_relay_ready(self, seconds):
        """Waits up to `seconds` for the left's relay-ready event (D6 85 with right link 2,
        HELLO bit 20, #594). True: the relay is up; False: it didn't come up in time; None: this
        can't tell (no relay events in this firmware, no foxglove-lab, left not connected), so
        the caller asks HELLO's right link instead."""
        exe = lab_binary()
        if exe is None or getattr(self, 'no_relay_events', False):
            return None
        seconds = max(0, int(seconds + 0.999))
        try:
            result = subprocess.run([str(exe), '--allow-app', 'relay-wait', '--timeout-s', str(seconds)],
                                    capture_output=True, text=True, timeout=seconds + 30)
        except subprocess.TimeoutExpired:
            return None
        if result.returncode == 4:
            self.no_relay_events = True  # the left, flashed first, stays as it is now
        return {0: True, 3: False}.get(result.returncode)

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
    def _ps(self, script, timeout=20):
        result = subprocess.run(['powershell', '-NoProfile', '-NonInteractive', '-Command',
                                 "$ErrorActionPreference='Stop'; " + script],
                                capture_output=True, text=True, timeout=timeout, check=True)
        return json.loads(result.stdout.strip() or '[]')

    def enumerate_bootloaders(self):
        return self._ps(r"ConvertTo-Json -Compress -InputObject @(Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -like 'USB\VID_239A&PID_00B3\*' } | ForEach-Object { $_.InstanceId.Split('\')[-1] })")

    @staticmethod
    def _cim_script(serial):
        return (r"$disks = @(Get-CimInstance Win32_DiskDrive | Where-Object { $_.PNPDeviceID -match '" + serial
                + r"' }); ConvertTo-Json -Compress -InputObject @($disks | Where-Object { $disks.Count -eq 1 } | ForEach-Object { $disk = $_; Get-CimAssociatedInstance -InputObject $disk -Association Win32_DiskDriveToDiskPartition | ForEach-Object { Get-CimAssociatedInstance -InputObject $_ -Association Win32_LogicalDiskToPartition | ForEach-Object { [pscustomobject]@{ Disk=$disk.DeviceID; Volume=$_.DeviceID } } } })")

    @staticmethod
    def _uf2_volume_script(serial):
        # The fallback (#654): lettered volumes with INFO_UF2.TXT at their root (any UF2
        # bootloader; nice!nano's is labelled NICENANO), each mapped to its disk, kept only when
        # that disk's device path holds this half's USB serial. Exactly one disk may match.
        # SERIALS holds plain letters and digits only (local_config), so it is safe in the script.
        return r'''$rows = @(Get-Volume | Where-Object { $_.DriveLetter -and (Test-Path -LiteralPath "$($_.DriveLetter):\INFO_UF2.TXT") } | ForEach-Object { $v = $_; Get-Partition -DriveLetter $v.DriveLetter | Get-Disk | Where-Object { $_.Path -match '@SERIAL@' } | ForEach-Object { [pscustomobject]@{ Disk=[string]$_.Number; Volume="$($v.DriveLetter):"; Label=$v.FileSystemLabel } } }); ConvertTo-Json -Compress -InputObject @($rows | Where-Object { @($rows.Disk | Sort-Object -Unique).Count -eq 1 })'''.replace('@SERIAL@', serial)

    def _lookup(self, serial, attempt, how, script):
        """One bounded lookup. The rows, or None when it failed (logged)."""
        try:
            rows = self._ps(script, timeout=FIND_TIMEOUT)
        except subprocess.TimeoutExpired:
            self.log(f'{SERIALS[serial]}: drive lookup {attempt}/{FIND_TRIES} ({how}) for {serial} '
                     f'timed out after {FIND_TIMEOUT} s.')
            return None
        except (subprocess.CalledProcessError, ValueError) as exc:
            detail = (getattr(exc, 'stderr', '') or str(exc)).strip().splitlines()
            self.log(f'{SERIALS[serial]}: drive lookup {attempt}/{FIND_TRIES} ({how}) for {serial} failed: '
                     f'{detail[-1] if detail else exc}')
            return None
        if len(rows) > 1:
            # Never guess between drives: this is what keeps a UF2 off the wrong half.
            raise Refused('Cannot map this serial to exactly one disk and volume. Unplug other bootloaders and try again.')
        if not rows:
            self.log(f'{SERIALS[serial]}: drive lookup {attempt}/{FIND_TRIES} ({how}) for {serial}: '
                     'no volume yet.')
        return rows

    def find_volume(self, serial):
        """The bootloader volume of `serial`, by the CIM association, or by the INFO_UF2.TXT
        fallback, retried FIND_TRIES times. Refuses an ambiguous mapping at once."""
        if serial not in SERIALS:
            raise Refused('Unknown bootloader serial')
        timed_out = True
        for attempt in range(1, FIND_TRIES + 1):
            for how, script in (('CIM', self._cim_script(serial)),
                                ('INFO_UF2.TXT', self._uf2_volume_script(serial))):
                rows = self._lookup(serial, attempt, how, script)
                if rows is None:
                    continue
                timed_out = False
                if rows:
                    volume = Path(rows[0]['Volume'] + '\\')
                    if not self.exists(volume / 'INFO_UF2.TXT'):
                        raise Refused('Mapped volume has no INFO_UF2.TXT')
                    if attempt > 1 or how != 'CIM':
                        self.log(f'{SERIALS[serial]}: drive found at {volume} ({how}, try {attempt}/{FIND_TRIES}).')
                    return volume
            if attempt < FIND_TRIES:
                self.sleep(FIND_PAUSE)
        if timed_out:
            raise TimeoutError(f'the drive lookup for {serial} timed out {FIND_TRIES} times')
        raise Refused('Cannot map this serial to exactly one disk and volume. Unplug other bootloaders and try again.')

    def exists(self, path):
        # Once the bootloader reboots, Windows answers a stat on its drive with WinError 433 or
        # 55 instead of "not found", and Python 3.12 raises. That means the drive is gone (#176).
        try:
            return Path(path).exists()
        except OSError:
            return False

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
                if not self.exists(Path(path).parent):
                    raise DriveGone(written, len(data), exc) from exc
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
    same = set()
    running = {half: backend.build_info(half) for half in ('left', 'right')}
    for half in ('left', 'right'):
        now = running[half]
        log(f"{half}: running {describe(now) if now else 'unknown (the firmware does not say)'}"
            + (f"; image {describe({'version': marks[half].version, 'profile': marks[half].profile, 'sha': marks[half].sha})}"
               if half in marks else '; not flashing'))
        if now and half in marks and marks[half].sha and now.get('sha') == marks[half].sha:
            same.add(half)
    after = {half: (marks[half].profile if half in marks and marks[half].profile else
                    (running[half] or {}).get('profile')) for half in ('left', 'right')}
    if all(after.values()) and after['left'] != after['right'] and not allow_profile_mix and not settings_reset:
        raise Refused(f"this would leave the left {after['left']} and the right {after['right']}; flash both halves "
                      'with the same profile, or pass --allow-profile-mix')
    return {half: m.sha for half, m in marks.items() if m.sha}, same


def verify_build(half, expected_sha, backend, log, clock=time.monotonic, sleep=time.sleep, strict=False):
    """After a flash: the half must report the image's SHA (when both sides know it). `strict`:
    not being able to read it back is a failure too (used when the write's outcome is unknown).
    Returns True only when the image's SHA was read back."""
    if not expected_sha:
        return False
    # The right answers over the relay, which comes back about 40 s after the right reboots (#122).
    deadline = clock() + (20 if half == 'left' else 60)
    while True:
        now = backend.build_info(half)
        if now:
            if now.get('sha') != expected_sha:
                raise FlashFailed(f"{half} runs {describe(now)}, expected SHA {expected_sha}")
            log(f'{half}: verified, running {describe(now)}.')
            return True
        if clock() >= deadline:
            if strict:
                raise FlashFailed(f"{half}: couldn't read its build back, so whether it was flashed is unknown.")
            log(f"{half}: couldn't read its build back to verify (firmware can't tell, or the right isn't back yet).")
            return False
        sleep(2)


def wait_relay(backend, seconds, clock=time.monotonic, sleep=time.sleep, or_bootloader=False):
    """Waits up to `seconds` for the left's relay to the right (right link 2). With relay events
    (HELLO bit 20) it waits on the relay-ready event (#594); older firmware is asked for HELLO's
    right link every second, as before (and, with `or_bootloader`, a bootloader drive ends that
    wait too). True when the relay is up (or a drive showed up while polling)."""
    waited = backend.wait_relay_ready(seconds)
    if waited is not None:
        return waited
    if or_bootloader:
        up = lambda: backend.right_link() == 2 or bool(backend.enumerate_bootloaders())
    else:
        up = lambda: backend.right_link() == 2
    try:
        wait_for(up, seconds, 'relay to the right not up', clock, sleep)
    except TimeoutError:
        return False
    return True


def check_right_typing(backend, log, clock=time.monotonic, sleep=time.sleep):
    """After the right is flashed: its typing check goes through the left's relay, so wait for
    the relay first (the relay-ready event with HELLO bit 20, #594; else HELLO right link 2; up
    to RELAY_WAIT_SECONDS) instead of a fixed sleep (#600). A check that gets no reply is tried
    once more after the next relay-ready wait before the #122 hint. Never fatal. Returns the last
    typing_check answer (True, False, or None when the PC had no window focus)."""
    for attempt in (1, 2):
        if not wait_relay(backend, RELAY_WAIT_SECONDS, clock, sleep):
            log(f'right: the relay to the right half is not up after {RELAY_WAIT_SECONDS} s; '
                'checking typing anyway.')
        typed = backend.typing_check('right', log)
        if typed is None:
            log('right: typing not checked (no window focus: is the PC locked?).')
            return None
        if typed:
            return True
        if attempt == 1:
            log('right: no reply to the typing check; trying once more once the relay is ready.')
    log('right: typing over the split link not confirmed yet; check it by hand '
        '(one tap on the LEFT reset is the known fix, #122).')
    return False


def enter_with_retries(half, backend, log, clock=time.monotonic, sleep=time.sleep):
    """Asks `half` to enter its bootloader (D7 08). The left is asked once. The right is asked
    again every few seconds for up to ENTRY_RETRY_SECONDS, waiting for the relay to the right
    (right link 2; with relay events, the relay-ready event, #594) before each retry (#545).
    Returns (entered, attempted)."""
    deadline = clock() + (ENTRY_RETRY_SECONDS if half == 'right' else 0)
    retrying = False
    while True:
        if backend.enter_bootloader(half, log):
            return True, True
        if clock() >= deadline:
            return False, True
        if not retrying:
            retrying = True
            log(f'{half}: retrying the bootloader request for up to {ENTRY_RETRY_SECONDS} s while the split re-links.')
        sleep(ENTRY_RETRY_PAUSE)
        # An earlier request may land late (#111; or #585's unconfirmed send over ZMK's split):
        # a bootloader showing up ends the retries, and the late-entry watch takes it from there.
        if backend.enumerate_bootloaders():
            return False, True
        if not wait_relay(backend, max(0, deadline - clock()), clock, sleep, or_bootloader=True):
            return False, True
        if backend.enumerate_bootloaders():
            return False, True


def flash(images, backend, seconds, stdin=sys.stdin, ask=input, log=print,
          settings_reset=False, allow_unknown_half=False, clock=time.monotonic, sleep=time.sleep,
          allow_profile_mix=False, yes=False, force=False, resume=False):
    """Flashes the halves in `images`, left first. Resume (#654): before any half is asked into
    its bootloader, the bootloaders already connected are listed. A half already in its
    bootloader (a run that stopped after the entry, or a double tap) is written first, scoped to
    its serial, and no half is asked in while another half's bootloader is unresolved. Two known
    bootloaders are written one at a time, each scoped to its serial; an unknown serial refuses.
    `resume`: only the halves already in their bootloaders are written; none is asked in."""
    log('Flash left first, check that it types, then flash right. Double-tap reset only on the requested half.')
    log('Rollback: known-good firmware is in ' + ROLLBACK)
    expected, same = check_builds(images, backend, log, allow_profile_mix, settings_reset)
    if not force and not settings_reset:
        for half in sorted(same):
            log(f'{half}: already running this exact build; skipping it (--force flashes it anyway).')
        images = {half: data for half, data in images.items() if half not in same}
        if not images:
            log('Nothing to flash: both selected halves already run these builds.')
            return
    if yes:
        log('--yes: the user has said go; no prompts. Every automatic check still applies.')
    else:
        confirm('Type FLASH to continue: ', 'FLASH', stdin, ask)
        if settings_reset:
            confirm('This erases saved settings. Type ERASE SETTINGS to confirm: ', 'ERASE SETTINGS', stdin, ask)
    def one(half, resumed):
        entered = attempted = False
        if not settings_reset and not resumed:
            # The right goes through the left's relay, which comes back about 20-40 s after the
            # left reboots (#122, #184): wait for it (the relay-ready event with HELLO bit 20,
            # #594; else HELLO right link 2; up to 60 s) before asking. A request the left can't
            # relay yet is retried for up to a minute, each time once the relay is up again
            # (#545), before falling back to a double tap.
            if half == 'right' and not wait_relay(backend, RELAY_WAIT_SECONDS, clock, sleep):
                log(f'right: the relay to the right half is not up after {RELAY_WAIT_SECONDS} s; trying anyway.')
            entered, attempted = enter_with_retries(half, backend, log, clock, sleep)
        asked = entered or (attempted and not settings_reset)
        if entered:
            log(f'Asked the {half} half to enter its bootloader. Waiting up to {seconds:g} seconds for its drive.')
        chosen = []
        said = set()
        def note(message):  # once per wait, not once a second
            if message not in said:
                said.add(message)
                log(message)
        def ready():
            serials = backend.enumerate_bootloaders()
            mine = known_bootloaders(serials).get(half, [])
            if len(mine) > 1:
                note(f'More than one {half} bootloader is connected ({", ".join(mine)}). Unplug one; '
                     'no image will be written yet.')
                return False
            if not mine:
                return False
            if len(serials) > 1:
                # The serial map says which drive is which half (#654): write this half's only.
                note(f'{len(serials)} bootloaders are connected; writing the {half} one only ({mine[0]}), '
                     'scoped to its serial.')
            chosen[:] = mine
            return True
        if resumed:
            pass
        elif not entered:
            if asked:
                # A hands-free request that got no reply may still land (#111: the right entered
                # its bootloader after the double-tap prompt). Never ask the user while it might.
                log(f'{half}: no answer to the bootloader request yet; watching {LATE_ENTRY_SECONDS} s '
                    'for a late entry before asking for a double tap.')
                try:
                    wait_for(ready, LATE_ENTRY_SECONDS, 'no late entry', clock, sleep)
                except TimeoutError:
                    log(f'Double-tap reset on the {half} half only. Waiting up to {seconds:g} seconds.')
            else:
                log(f'Double-tap reset on the {half} half only. Waiting up to {seconds:g} seconds.')
        if not chosen:
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
        unconfirmed = False
        for attempt in (1, 2):
            # Recheck identity immediately before touching the drive.
            if (known_bootloaders(backend.enumerate_bootloaders()).get(half, []) != chosen
                    or backend.find_volume(chosen[0]) != volume):
                raise Refused('Bootloader identity or volume changed before writing')
            try:
                backend.write_file(volume / 'flash.uf2', images[half])
            except DriveGone as gone:
                # Don't retry and don't call it a failure yet: the build read-back decides.
                log(f'{half}: {gone}; the bootloader may have taken the whole image, checking the build.')
                record(half, expected.get(half), 'written-drive-gone', str(gone))
                unconfirmed = True
                break
            try:
                wait_for(lambda: not backend.exists(info), 10 if attempt == 1 else 15,
                         f'{half} drive did not disappear after writing', clock, sleep)
                break
            except TimeoutError:
                if attempt == 2:
                    raise
                log(f'{half}: the bootloader did not take the image; writing it once more.')
        if unconfirmed:
            if not expected.get(half):
                raise FlashFailed(f"{half}: the drive went away mid-write and this image has no SHA to check "
                                  "against; can't tell whether it was flashed. Re-run to be sure.")
        log(f'{half} bootloader drive disappeared.')
        if half == 'left' and not settings_reset:
            wait_for(lambda: backend.running_keyboard(chosen[0]), seconds, 'Left keyboard did not re-enumerate (1D50:615E)', clock, sleep)
        if not unconfirmed:
            record(half, expected.get(half), 'written')
        if not settings_reset:
            try:
                verified = verify_build(half, expected.get(half), backend, log, clock, sleep, strict=unconfirmed)
            except FlashFailed as exc:
                record(half, expected.get(half), 'verify-failed', str(exc))
                raise
            # The history says exactly what the console said.
            record(half, expected.get(half), 'verified' if verified else 'unverified')
        if settings_reset:
            return
        if yes:
            # The right is flashed only after the left is proven to type. The right's own check
            # needs its relay up again, so it waits for that (#600) and reports rather than fails.
            if half == 'left':
                typed = backend.typing_check('left', log)
                if typed is None:
                    log('left: typing not checked (no window focus: is the PC locked?). The build read-back '
                        'passed; carrying on.')
                elif not typed:
                    raise FlashFailed('the left half did not type after flashing; not touching the right. '
                                      'Rollback: ' + ROLLBACK)
            if half == 'right':
                check_right_typing(backend, log, clock, sleep)
        elif half == 'left':
            confirm('Test the left keyboard in a text editor. If it types, enter TYPES here: ', 'TYPES', stdin, ask)

    done = set()
    while True:
        # Listed again before each half: never ask a half into its bootloader while another
        # one's bootloader is unresolved (#654: that stranded both halves).
        present = {h: s for h, s in known_bootloaders(backend.enumerate_bootloaders()).items() if h not in done}
        pending = [h for h in ('left', 'right') if h in images and h not in done]
        stranded = [h for h in pending if h in present]
        if stranded:
            half = stranded[0]
            log(f'{half}: already in its bootloader ({", ".join(present[half])}); writing it first, '
                'without asking any half into its bootloader.')
        elif resume or not pending or present:
            break
        else:
            half = pending[0]
        one(half, resumed=bool(stranded))
        done.add(half)
    if present:
        # A bootloader this run has no image for: writing nothing to it, and asking no other half in.
        for h, s in sorted(present.items()):
            log(f'{h}: in its bootloader ({", ".join(s)}), and this run has no {h} image.')
        which = next(iter(present)) if len(present) == 1 else 'both'
        raise Refused(f'a half is still in its bootloader; no other half is asked in. Run again with '
                      f'--half {which} (or --resume) to write it.')
    if resume and not done:
        log('Nothing to resume: no selected half is in its bootloader.')


def known_bootloaders(serials):
    """{half: [serials]} for the connected bootloaders. Refuses any serial this PC doesn't know:
    a UF2 is never written to a drive the serial map can't place."""
    if any(s not in SERIALS for s in serials):
        raise Refused('Unknown bootloader serial; unplug it before proceeding.')
    found = {}
    for serial in serials:
        found.setdefault(SERIALS[serial], []).append(serial)
    return found


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
    parser.add_argument('--force', action='store_true',
                        help='flash a half even when it already runs the same build')
    parser.add_argument('--yes', action='store_true',
                        help='no prompts: the normal path once the user has said go for this flash '
                             '(typed FLASH/TYPES waived by the user). Every automatic check still runs: '
                             'USB serial identity, UF2 preflight and half marker, dev/GA mix, the '
                             'single-flasher lock, settle and retry, D7 09 build read-back, and a '
                             'typing check in place of TYPES')
    parser.add_argument('--allow-profile-mix', action='store_true',
                        help='allow leaving one half dev and the other GA')
    parser.add_argument('--resume', action='store_true',
                        help='doctor mode (#654): write only the halves already in their bootloaders '
                             '(stranded by an earlier run, or double-tapped), each scoped to its USB '
                             'serial, and never ask a half into its bootloader. Normal runs resume too: '
                             'a half already in its bootloader is written first')
    args = parser.parse_args(argv)
    if not math.isfinite(args.wait_seconds) or args.wait_seconds <= 0:
        parser.error('--wait-seconds must be positive and finite')
    report = {'images': {}, 'messages': []}
    def log(message):
        report['messages'].append(message)
        if not args.json:
            print(message, flush=True)
    code = 0
    touched = False  # the run reached the devices
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
            with FlashLock():
                touched = True
                flash(images, WindowsOS(log=log), args.wait_seconds, log=log, ask=ask_terminal,
                      settings_reset=args.settings_reset, allow_unknown_half=args.allow_unknown_half,
                      allow_profile_mix=args.allow_profile_mix, yes=args.yes, force=args.force,
                      resume=args.resume)
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
        if touched:
            log('If a half is still in its bootloader, run the same command again (or add --resume): '
                'a half already in its bootloader is written first, and no other half is asked in.')
    if args.json:
        report['exit_code'] = code
        print(json.dumps(report))
    return code


if __name__ == '__main__':
    sys.exit(main())

