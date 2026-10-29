"""Batch H of the 0.2 sitting (#586, #527, #70, #571) from the files Wallpaper Engine, Foxglove and the
Bandit wallpapers already write. Read-only: it launches nothing and changes nothing, except
`debug on|off`, which sends one `-control applyProperties` to the running Wallpaper Engine (as
Foxglove's own wallpaper sync does on every layer change) to turn a wallpaper's Diagnostics on or
off; Wallpaper Engine doesn't save it (config.json is not written, checked 10/27).

    python scripts/lab/wallpaper_check.py state
        Wallpaper Engine running, each monitor's wallpaper and its manifest (palette accent,
        supports), the playback settings behind #586 (Stop unloads the wallpaper under a
        full-screen program), the Presets card's expected wallpaper names (#571), and whether
        "Lights follow the wallpaper" is on in the user's store, with the accent it would take.
    python scripts/lab/wallpaper_check.py debug on|off --monitor SAM
        Diagnostics on the monitor whose key contains SAM: the wallpaper then posts a snapshot
        every 2 s (its Foxglove state included) to the devlog listener
        (python -I <bandit-wallpapers>/tools/devlog.py --minutes 30 --idle 15).
    python scripts/lab/wallpaper_check.py game --since 2026-10-29T09:00:00Z
        After the sitting's full-screen game and Win key (#586): Foxglove sent game mode on;
        Wallpaper Engine dropped no -control command ("Windows is reentrant"); DXGI device-lost
        lines (a flicker that isn't Foxglove's); and from the devlog, each reload (a gap in the
        snapshots) and whether the wallpaper still had game mode after it.

Exit 0 PASS, 1 FAIL, 2 BLOCKED (a file or the devlog is missing). Prints one line per check.
"""
import argparse
import json
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from labkit import Checks  # noqa: E402  (prints UTF-8 whatever the console)

STEAM_WE = Path(r'C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine')
APPDATA = Path(os.environ.get('APPDATA', str(Path.home())))
FOXGLOVE_LOG = APPDATA / 'Foxglove' / 'logs' / 'foxglove.log'
STORE = APPDATA / 'Foxglove' / 'foxglove.toml'
DEVLOG = Path(r'C:\Users\fuchs\bandit-wallpapers\logs\devlog.jsonl')
# The wallpaper posts every 2 s while it runs; a longer gap is Wallpaper Engine stopping or
# reloading it (or pausing it: a paused wallpaper posts nothing either).
GAP_S = 6.0


# ----------------------------------------------------------------------------- pure parts

def utc(text):
    """An ISO time (…Z, or Foxglove's "2026-10-29 07:53:54Z") as an aware datetime."""
    t = text.strip().replace(' ', 'T')
    if t.endswith('Z'):
        t = t[:-1] + '+00:00'
    d = datetime.fromisoformat(t)
    return d if d.tzinfo else d.replace(tzinfo=timezone.utc)


def selected(config):
    """Wallpaper Engine's config.json text → [(monitor key, file)], sorted as Foxglove sorts them
    (wallpaper.rs `selected`: the first with a palette gives the accent)."""
    out = []
    for user in json.loads(config.lstrip('\ufeff')).values():
        if not isinstance(user, dict):
            continue
        sel = (((user.get('general') or {}).get('wallpaperconfig') or {}).get('selectedwallpapers') or {})
        for monitor, w in sel.items():
            if isinstance(w, dict) and w.get('file'):
                out.append((monitor, w['file'].replace('\\', '/')))
    return sorted(out)


def playback(config):
    """The playback settings behind #586, from the first user with them."""
    for user in json.loads(config.lstrip('\ufeff')).values():
        g = ((user or {}).get('general') or {}).get('user') if isinstance(user, dict) else None
        if isinstance(g, dict) and 'playbackfullscreen' in g:
            return {k: g.get(k) for k in ('playbackfullscreen', 'playbackmaximized', 'playbackfocus', 'pausevram')}
    return {}


def most_colourful(stops):
    """wallpaper_manifest::most_colourful: the stop with the largest max-min channel spread; on a
    tie the earliest (Rust's max_by_key over the reversed stops keeps the last maximum)."""
    rgb = [hex_rgb(s) for s in stops]
    best = None
    for c in rgb:
        if best is None or max(c) - min(c) > max(best) - min(best):
            best = c
    return best


def hex_rgb(s):
    s = s.lstrip('#')
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))


def hue_sat(r, g, b):
    """accent::hue_sat: (hue 0-359, saturation 0-100) as Foxglove sends it."""
    r, g, b = r / 255, g / 255, b / 255
    mx, mn = max(r, g, b), min(r, g, b)
    d = mx - mn
    if d <= 1e-7:
        return (0, 0)
    if mx == r:
        h = 60 * (((g - b) / d) % 6)
    elif mx == g:
        h = 60 * ((b - r) / d + 2)
    else:
        h = 60 * ((r - g) / d + 4)
    return (round(h) % 360, round(d / mx * 100))


def manifest_accent(m):
    """A manifest's accent as Foxglove reads it: palette.accent, else its most colourful stop."""
    p = m.get('palette') or {}
    if p.get('accent'):
        return hex_rgb(p['accent'])
    return most_colourful(p['stops']) if p.get('stops') else None


def lighting_flag(store_text, key):
    """A `[lighting]` flag of the store (off when missing, as serde's default)."""
    section = re.search(r'^\[lighting\]\s*$(.*?)(?=^\[|\Z)', store_text, re.M | re.S)
    if not section:
        return False
    m = re.search(rf'^\s*{key}\s*=\s*(true|false)', section.group(1), re.M)
    return bool(m and m.group(1) == 'true')


def follow_wallpaper(store_text):
    """The store's `[lighting]` follow_wallpaper ("Lights follow the wallpaper", #70)."""
    return lighting_flag(store_text, 'follow_wallpaper')


def follow_accent(store_text):
    """The store's `[lighting]` follow_accent ("Follow the Windows accent colour", #93)."""
    return lighting_flag(store_text, 'follow_accent')


# The Windows accent, as Foxglove reads it (#70, #93). accent.rs takes UISettings'
# GetColorValue(UIColorType::Accent): the base of the accent palette Windows derives from the
# colour picked in Settings. That is not the picked colour itself (DWM's AccentColor, shown in
# Settings > Personalization > Colors): on this PC the pick is #d39f3f (39/70) and the accent
# Foxglove follows is #9c6f05 (42/97), a darker, more saturated gold.

def palette_accent(palette):
    """The accent from HKCU\\...\\Explorer\\Accent\\AccentPalette (8 RGBA entries, lightest to
    darkest; UISettings' Accent is entry 3, checked against UISettings on 10/29), or None."""
    if not palette or len(palette) < 16:
        return None
    return tuple(palette[12:15])


def dwm_rgb(abgr):
    """DWM's AccentColor (0xAABBGGRR) as (r, g, b): the colour picked in Settings."""
    return (abgr & 0xFF, (abgr >> 8) & 0xFF, (abgr >> 16) & 0xFF)


def windows_accent():
    """(rgb, source) of the Windows accent Foxglove follows: UISettings' Accent (the app's own
    call, through PowerShell's WinRT), else the accent palette's entry 3; (None, why) without
    either. Read only."""
    script = ('[Windows.UI.ViewManagement.UISettings,Windows.UI.ViewManagement,ContentType=WindowsRuntime] | Out-Null; '
              '$c = (New-Object Windows.UI.ViewManagement.UISettings).GetColorValue('
              '[Windows.UI.ViewManagement.UIColorType]::Accent); "$($c.R) $($c.G) $($c.B)"')
    try:
        out = subprocess.run(['powershell', '-NoProfile', '-Command', script], capture_output=True, text=True,
                             timeout=30).stdout.split()
        if len(out) == 3:
            return tuple(int(v) for v in out), 'UISettings Accent (as accent.rs reads it)'
    except (OSError, ValueError, subprocess.TimeoutExpired):
        pass
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r'Software\Microsoft\Windows\CurrentVersion\Explorer\Accent') as k:
            c = palette_accent(winreg.QueryValueEx(k, 'AccentPalette')[0])
        if c:
            return c, 'the accent palette entry 3 (UISettings Accent unreadable)'
    except OSError:
        pass
    return None, 'no Windows accent readable'


def picked_accent():
    """The colour picked in Windows' Settings (DWM AccentColor), for the record only, or None."""
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r'Software\Microsoft\Windows\DWM') as k:
            return dwm_rgb(winreg.QueryValueEx(k, 'AccentColor')[0])
    except OSError:
        return None


def colour(c):
    return '#%02x%02x%02x (hue/sat %d/%d)' % (tuple(c) + hue_sat(*c)) if c else 'none'


def expected(manifest, windows):
    """(hue/sat, source) the lights take for a wallpaper: its palette's accent, else the Windows
    accent (`windows`: (rgb, source) from wallpaper_check.windows_accent)."""
    a = manifest_accent(manifest)
    if a:
        return hue_sat(*a), f"the wallpaper palette {colour(a)}"
    rgb, source = windows
    return (hue_sat(*rgb) if rgb else None), f"the Windows accent {colour(rgb)}, from {source}"


def log_since(lines, since, piece):
    """[(time, line)] of Foxglove log lines containing `piece` at or after `since`."""
    out = []
    for l in lines:
        m = re.match(r'(\d{4}-\d\d-\d\d \d\d:\d\d:\d\dZ) ', l)
        if m and piece in l and utc(m.group(1)) >= since:
            out.append((utc(m.group(1)), l))
    return out


def we_log_since(lines, since, piece):
    """Wallpaper Engine's log.txt lines ("2026-10-28T07:25:20Z, 406408: …") with `piece`."""
    out = []
    for l in lines:
        m = re.match(r'(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ), ', l)
        if m and piece in l and utc(m.group(1)) >= since:
            out.append(l)
    return out


def reloads(snaps, gap_s=GAP_S):
    """The devlog's snapshots (one wallpaper, in time order) → [(gap start, gap end, game before,
    game after)] for each gap longer than `gap_s`; game is snapshot.foxglove.game (None when the
    wallpaper sent no Foxglove block)."""
    out = []
    for a, b in zip(snaps, snaps[1:]):
        ta, tb = utc(a['at']), utc(b['at'])
        if (tb - ta).total_seconds() > gap_s:
            out.append((ta, tb, (a.get('foxglove') or {}).get('game'), (b.get('foxglove') or {}).get('game')))
    return out


# ----------------------------------------------------------------------------- the checks

def we_dir(arg):
    return Path(arg) if arg else STEAM_WE


def we_running():
    r = subprocess.run(['tasklist', '/FO', 'CSV', '/NH'], capture_output=True, text=True, errors='replace')
    names = {line.split('","')[0].strip('"').lower() for line in r.stdout.splitlines() if line}
    return bool(names & {'wallpaper64.exe', 'wallpaper32.exe'})


def manifest_of(file):
    p = Path(file).parent / 'foxglove.json'
    try:
        return json.loads(p.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return None


def state(args, check):
    we = we_dir(args.we)
    cfg = we / 'config.json'
    if not check('Wallpaper Engine config', cfg.is_file(), str(cfg), blocked=not cfg.is_file()):
        return
    text = cfg.read_text(encoding='utf-8', errors='replace')
    check('Wallpaper Engine is running', we_running(), 'wallpaper64.exe / wallpaper32.exe')
    print('  playback (#586):', playback(text), flush=True)
    accent = None
    for monitor, file in selected(text):
        m = manifest_of(file)
        a = manifest_accent(m) if m else None
        accent = accent or a
        print(f'  {monitor}: {file}', flush=True)
        if m:
            print(f"    manifest {m.get('id')} {m.get('title')!r}: supports {m.get('supports')}, accent "
                  f"{'#%02x%02x%02x' % a + ' = hue/sat %d/%d' % hue_sat(*a) if a else 'none (the Windows accent)'}",
                  flush=True)
    hud = any('bandit-hud' in f for _, f in selected(text))
    check('the Bandit HUD is on a monitor (H1 needs no pick)', hud, 'selectedwallpapers')
    names = []
    for d in sorted((we / 'projects' / 'myprojects').glob('*/foxglove.json')):
        try:
            m = json.loads(d.read_text(encoding='utf-8'))
        except ValueError as e:
            print(f'  {d}: broken ({e}); Foxglove skips it', flush=True)
            continue
        names += [x['name'] for x in m.get('motifs', [])]
    print(f'  Presets card, "Wallpapers" (#571, the user reads it in H4): {names}', flush=True)
    win, source = windows_accent()
    print(f'  the Windows accent as Foxglove reads it: {colour(win)}, from {source}; the colour picked in '
          f"Windows' Settings (DWM AccentColor, not what Foxglove follows): {colour(picked_accent())}", flush=True)
    if STORE.is_file():
        store = STORE.read_text(encoding='utf-8', errors='replace')
        on, acc = follow_wallpaper(store), follow_accent(store)
        print(f'  "Lights follow the wallpaper" in the store: {"on" if on else "off"}; with it on the lights take '
              f"{colour(accent) + ' (the wallpaper palette)' if accent else 'the Windows accent'}", flush=True)
        now = (f'{colour(accent)}, the wallpaper palette' if on and accent else
               f'{colour(win)}, the Windows accent' if on or acc else 'nothing (both off)')
        print(f'  the lights follow now: {now}. That is the hue/sat Foxglove asks for (its "accent: applied" '
              'and "re-applied the accent colour" lines); RGB_STATE reads it after the keyboard\'s colour '
              'calibration (the store\'s [calibration], e.g. gamma 2.5 turns 42/97 into about 23/100)', flush=True)
    sync = [l for l in read_lines(FOXGLOVE_LOG) if 'wallpaper sync: ' in l]
    check('Foxglove synced a wallpaper (wallpaper sync line in foxglove.log)', bool(sync), sync[-1:] or str(FOXGLOVE_LOG))


def read_lines(path):
    try:
        return Path(path).read_text(encoding='utf-8', errors='replace').splitlines()
    except OSError:
        return []


def debug(args, check):
    we = we_dir(args.we)
    text = (we / 'config.json').read_text(encoding='utf-8', errors='replace')
    keys = [m for m, _ in selected(text) if args.monitor.lower() in m.lower()]
    if not check(f'one monitor key contains {args.monitor!r}', len(keys) == 1, keys, blocked=len(keys) != 1):
        return
    props = json.dumps({'debug': args.on == 'on'}, separators=(',', ':'))
    cmd = f'"{we / "wallpaper64.exe"}" -control applyProperties -location "{keys[0]}" -properties RAW~({props})~END'
    r = subprocess.run(cmd, capture_output=True, timeout=10, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0))
    check(f'Diagnostics {args.on} sent to {keys[0]}', r.returncode == 0, f'exit {r.returncode}')


def game(args, check):
    since = utc(args.since)
    fox = read_lines(FOXGLOVE_LOG)
    on = log_since(fox, since, 'game mode on')
    check('Foxglove told the wallpaper game mode on (#586 setup)', bool(on), on[-1][1] if on else 'no line')
    we_lines = read_lines(we_dir(args.we) / 'log.txt')
    dropped = we_log_since(we_lines, since, 'Command ignored because Windows is reentrant')
    check('Wallpaper Engine dropped no -control command', not dropped, dropped[-3:] or 'none')
    lost = we_log_since(we_lines, since, 'DXGI device lost')
    print(f'  DXGI device lost since then: {len(lost)} (a GPU reset redraws the wallpaper: a flicker that '
          f"isn't Foxglove's) {lost[-2:]}", flush=True)
    snaps = []
    for l in read_lines(args.devlog):
        try:
            s = json.loads(l)
        except ValueError:
            continue
        if s.get('at') and utc(s['at']) >= since and args.wallpaper.lower() in str(s.get('wallpaper', '')).lower():
            snaps.append(s)
    if not check('devlog snapshots from the HUD since then', len(snaps) >= 2,
                 f'{len(snaps)} in {args.devlog} (Diagnostics on and the listener running?)', blocked=len(snaps) < 2):
        return
    gaps = reloads(snaps)
    for a, b, before, after in gaps:
        print(f'  gap {a:%H:%M:%S}-{b:%H:%M:%S}Z ({(b - a).total_seconds():.0f} s): game {before} -> {after}', flush=True)
    last = utc(snaps[-1]['at'])
    print(f'  last snapshot {last:%H:%M:%S}Z: {snaps[-1].get("foxglove")}', flush=True)
    if not gaps:
        return check('a reload or stop during the game (a gap in the snapshots)', False,
                     'none: Wallpaper Engine kept the wallpaper running (Pause, not Stop?)', blocked=True)
    kept = all(after is True for *_, after in gaps if before is True)
    check('after each reload the wallpaper still has game mode (#586: no re-push needed)', kept,
          'kept' if kept else 'lost: a reloaded wallpaper starts from its defaults (#586 confirmed, needs the re-push event)')


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--we', help="Wallpaper Engine's folder (default: Steam's own library)")
    sub = ap.add_subparsers(dest='what', required=True)
    sub.add_parser('state')
    d = sub.add_parser('debug')
    d.add_argument('on', choices=['on', 'off'])
    d.add_argument('--monitor', required=True, help="a piece of Wallpaper Engine's monitor key, e.g. SAM")
    g = sub.add_parser('game')
    g.add_argument('--since', required=True, help='UTC, e.g. 2026-10-29T09:00:00Z (just before H1)')
    g.add_argument('--devlog', default=str(DEVLOG))
    g.add_argument('--wallpaper', default='Bandit HUD', help="the snapshots' wallpaper title")
    args = ap.parse_args(argv)
    check = Checks()
    {'state': state, 'debug': debug, 'game': game}[args.what](args, check)
    check.finish()


if __name__ == '__main__':
    main()
