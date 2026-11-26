#!/usr/bin/env python3
"""Source lint (#1526): only the guarded files may change one of the PC's volumes or mutes.

A lab copy of Foxglove once engaged the real master volume. Since then every Core Audio write
goes through a file whose calls ask first whether this run may touch the real audio, and a lab or
test run may not. This check keeps it that way: it fails when any other Rust file names a call
that sets a volume or a mute.

    python scripts/check_audio_writes.py

It reads every `.rs` file under ROOTS, recursively (`target`, `.git` and `node_modules` folders
skipped), test code included, and looks outside comments for:

- `SetMasterVolumeLevelScalar`, `SetMasterVolumeLevel`, `SetMasterVolume`, `SetChannelVolume`,
  `SetChannelVolumeLevel...`, `SetAllVolumes`, `VolumeStepUp`, `VolumeStepDown`,
  `waveOutSetVolume`, `mixerSetControlDetails`: the bare name, however it is reached
  (`v.SetAllVolumes(..)`, `ISimpleAudioVolume::SetMasterVolume(&v, ..)`, a `use`);
- `SetMute` as a call: `x.SetMute(..)` and `IAudioEndpointVolume::SetMute(&e, ..)`. The bare name
  is also a message type and an event (`services::SetMute`, `Event::SetMute { .. }`), which are
  not calls and pass;
- `VK_VOLUME_`: the volume keys, sent as key presses.

A file in ALLOWED passes, with the reason it is safe. An ALLOWED file with no such name fails too,
so the list can't go stale. Exit 0 when clean, 1 with one line per problem. `--self-test` checks
the scanner on made-up text.
"""
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ROOTS = ('app', 'crates', 'examples', 'plugins', 'sdk', 'tools', 'vendor')
SKIP_DIRS = {'target', '.git', 'node_modules'}

# file: why its writes are safe.
ALLOWED = {
    'crates/info-screen/src/audio_reach.rs':
        "Foxglove's five Core Audio writes, each behind guard(): a lab or test run is refused",
    'crates/hideout-services/src/mic_win.rs':
        "the hub's one microphone mute, behind mic_reach::guard(): only the installed hub in the "
        "user's own folder",
}

BARE = ('SetMasterVolumeLevelScalar', 'SetMasterVolumeLevel', 'SetMasterVolume', 'SetChannelVolume',
        'SetAllVolumes', 'VolumeStepUp', 'VolumeStepDown', 'waveOutSetVolume', 'mixerSetControlDetails',
        'VK_VOLUME_')
WRITE = re.compile(r'(?<![A-Za-z0-9_])(?:(' + '|'.join(BARE) + r')|(SetMute)\s*\()')


def code(text):
    """`text` with its comments blanked (line and block, nested), line breaks kept. String
    literals stay: a name in a string is still a name."""
    out, i, n, depth = [], 0, len(text), 0
    while i < n:
        c, two = text[i], text[i:i + 2]
        if depth:
            if two == '/*':
                depth, i = depth + 1, i + 2
            elif two == '*/':
                depth, i = depth - 1, i + 2
            else:
                out.append('\n' if c == '\n' else ' ')
                i += 1
        elif two == '//':
            while i < n and text[i] != '\n':
                i += 1
        elif two == '/*':
            depth, i = 1, i + 2
        elif c == '"':
            # A raw string ends at its quote and hashes; any other at an unescaped quote.
            m = re.search(r'r(#*)$', ''.join(out[-8:]))
            if m:
                end = text.find('"' + m.group(1), i + 1)
                end = n if end < 0 else end + 1 + len(m.group(1))
            else:
                end = i + 1
                while end < n and text[end] != '"':
                    end += 2 if text[end] == '\\' else 1
                end = min(end + 1, n)
            out.append(text[i:end])
            i = end
        elif c == "'" and text[i:i + 3] in ("'\"'", "'/'"):
            out.append(text[i:i + 3])
            i += 3
        else:
            out.append(c)
            i += 1
    return ''.join(out)


def hits(text):
    """(line number, name) for every write named in `text` outside comments."""
    found = []
    for n, line in enumerate(code(text).split('\n'), 1):
        for m in WRITE.finditer(line):
            found.append((n, m.group(1) or m.group(2)))
    return found


def check(files, allowed=None):
    """One line per problem in `files` ({repo-relative path: text})."""
    allowed = ALLOWED if allowed is None else allowed
    problems, used = [], set()
    for rel in sorted(files):
        found = hits(files[rel])
        if rel in allowed:
            if found:
                used.add(rel)
            continue
        for n, name in found:
            problems.append(f'{rel}:{n}: {name}: a volume or mute write outside the guarded files; '
                            f'go through audio_reach (Foxglove) or mic_reach (the hub), which refuse a lab or test run')
    for rel in sorted(set(allowed) - used):
        problems.append(f'{rel}: allowed in scripts/check_audio_writes.py but names no volume or mute write now; '
                        f'take it off the list')
    return problems


def rust_files(repo=REPO):
    """Every `.rs` under ROOTS, as {repo-relative path with forward slashes: text}."""
    files = {}
    for root in ROOTS:
        for folder, dirs, names in os.walk(repo / root):
            dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
            for name in names:
                if name.endswith('.rs'):
                    p = Path(folder) / name
                    files[p.relative_to(repo).as_posix()] = p.read_text(encoding='utf-8', errors='replace')
    return files


def self_test():
    f = 'crates/x/src/deep/y.rs'
    for line in ('v.SetMute(true, &ctx)', 'IAudioEndpointVolume::SetMute(&e, true, null())',
                 'e . SetMute (m, c)', 'v.SetMasterVolumeLevelScalar(0.5, c)', 'v.SetMasterVolumeLevel(-3.0, c)',
                 'ISimpleAudioVolume::SetMasterVolume(&s, 1.0, c)', 'let f = ISimpleAudioVolume::SetMasterVolume;',
                 'v.SetChannelVolumeLevelScalar(0, 1.0, c)', 'c.SetAllVolumes(2, p, c)', 'v.VolumeStepUp(c)',
                 'v.VolumeStepDown(c)', 'waveOutSetVolume(h, 0)', 'mixerSetControlDetails(h, &d, 0)',
                 'key(VK_VOLUME_MUTE)', 'use windows::Win32::UI::Input::KeyboardAndMouse::VK_VOLUME_UP;',
                 'let url = "http://x"; v.SetMute(m, c)', 'let s = "v.SetMute(m, c)";'):
        assert len(check({f: f'fn a() {{\n    {line};\n}}\n'}, {})) == 1, line
    for line in ('// v.SetMute(true, &ctx)', '/// `SetMasterVolume` is never called here.', '/* v.SetMute(m, c) */',
                 '/* a /* v.SetAllVolumes() */ b */', 'Event::SetMute { muted, reply }', 'services::SetMute,',
                 'typed::<crate::services::SetMute>(params)', 'let p: methods::SetMuteParams = params(x)?;',
                 'mic.set_mute(true)', 'const SET_MUTE: &str = "audio.inputs.set_mute";', 'v.GetMute()',
                 'v.GetMasterVolumeLevelScalar()', 'self.DoSetMute(1)'):
        assert check({f: f'fn a() {{\n    {line}\n}}\n'}, {}) == [], line
    assert check({f: 'fn a() {\n    v.SetMute(m, c);\n}\n'}, {f: 'guarded'}) == []
    stale = check({f: 'fn a() {}\n'}, {f: 'guarded'})
    assert len(stale) == 1 and 'take it off the list' in stale[0], stale
    p = check({f: 'fn a() {}\n\n// one\nfn b() {\n    v.SetMute(m, c);\n}\n'}, {})
    assert p[0].startswith(f + ':5: SetMute: '), p
    print('self-test ok')


def main():
    if '--self-test' in sys.argv:
        self_test()
        return 0
    files = rust_files()
    problems = check(files)
    for p in problems:
        print(p)
    if not problems:
        print(f'check_audio_writes: {len(files)} files, every volume and mute write is in a guarded file')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
