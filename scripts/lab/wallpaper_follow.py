"""Lights follow the wallpaper (#70), the sitting's old H7-H8, without the user: the build under test
runs on a temp FOXGLOVE_CONFIG_DIR with "Lights follow the wallpaper" on, and its wallpaper test
hook (FOXGLOVE_WALLPAPER_OVERRIDE, wallpaper.rs: a watched file naming the wallpaper on screen)
switches between installed Bandit wallpapers as Wallpaper Engine's config would. The manifests are
the real ones in Wallpaper Engine's projects; nothing is sent to Wallpaper Engine (the hook has no
monitors to sync). For each switch the underglow (RGB_STATE, both halves) must take the wallpaper's
accent (palette.accent, else its most colourful stop) and the log must say "Foxglove accent:
applied h/s"; a wallpaper without a palette gives the Windows accent. The lights are put back after.

The case only follows with its "Match keyboard" on (off in the user's store), so it isn't checked
here; whether the colours look like the wallpaper is the sitting's one eyes step (H5).

    FOXGLOVE_APP_OK=1 python scripts/lab/wallpaper_follow.py --app <foxglove.exe>   (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse, json, os, re, tempfile, time
from pathlib import Path
from labkit import App, Checks, TARGET, log_lines, require_app_ok, restore_lights, rgb, temp_config, tool_arg, wait
from wallpaper_check import STEAM_WE, hue_sat, manifest_accent

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument('--app', required=True, type=tool_arg('--app'))
ap.add_argument('--lab', default=str(TARGET / 'release' / 'foxglove-lab.exe'), type=tool_arg('--lab'))
ap.add_argument('--we', default=str(STEAM_WE), help="Wallpaper Engine's folder (its projects hold the manifests)")
# Two with different accents, then one without a palette (the Windows accent).
ap.add_argument('--walls', nargs=3, default=['fury-pattern', 'hearth-cauldron', 'fury-forest'],
                metavar=('PALETTE_A', 'PALETTE_B', 'NO_PALETTE'))
args = ap.parse_args()
require_app_ok()
check = Checks()
LEFT_S, RIGHT_S = 3, 10  # the left within ~1 s plus the log's 1 s grain; the right over the relay


def manifests(we):
    out = {}
    for f in (Path(we) / 'projects' / 'myprojects').glob('*/foxglove.json'):
        try:
            m = json.loads(f.read_text(encoding='utf-8'))
            out[m['id']] = m
        except (ValueError, KeyError):
            pass
    return out


def windows_accent():
    """The Windows accent as (hue, sat): DWM's AccentColor (0xAABBGGRR)."""
    import winreg
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r'Software\Microsoft\Windows\DWM') as k:
        v = winreg.QueryValueEx(k, 'AccentColor')[0]
    return hue_sat(v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF)


def near(a, b, tol=3):
    return abs((a[0] - b[0] + 180) % 360 - 180) <= tol and abs(a[1] - b[1]) <= tol


found = manifests(args.we)
a_id, b_id, none_id = args.walls
missing = [w for w in args.walls if w not in found]
if missing:
    check('the wallpapers are installed', False, f'{missing} not in {args.we}', blocked=True)
    check.finish()
want = {w: (hue_sat(*manifest_accent(found[w])) if manifest_accent(found[w]) else windows_accent()) for w in args.walls}
if manifest_accent(found[none_id]) is not None or want[a_id] == want[b_id] or want[b_id] == want[none_id]:
    check('the three wallpapers give three different colours', False, want, blocked=True)
    check.finish()
print('expected hue/sat:', want, flush=True)

over = Path(tempfile.mkdtemp(prefix='foxglove-wallpaper-hook-')) / 'wallpaper.txt'
over.write_text(a_id, encoding='utf-8')
cfg = temp_config('foxglove-wallpaper-follow-', '[lighting]\nfollow_wallpaper = true\n')
original = rgb(args.lab, dict(os.environ))
with App(args.app, cfg, env={'FOXGLOVE_WALLPAPER_OVERRIDE': str(over)}) as app:
    read = lambda: rgb(args.lab, app.env)
    hs = lambda h: (lambda s: len(s) == 2 and all(near((x['hue'], x['sat']), h) for x in s))
    left = lambda h: (lambda s: len(s) >= 1 and near((s[0]['hue'], s[0]['sat']), h))
    ok, s = wait(read, hs(want[a_id]), 40)  # the start: the keymap is learned first (#633)
    check(f'{a_id} on screen at start: both halves take its accent {want[a_id]}', ok, s)
    for w in (b_id, none_id):
        before = len(log_lines(cfg, 'Foxglove accent: applied'))
        t0 = time.time()
        over.write_text(w, encoding='utf-8')
        ok, s = wait(read, left(want[w]), LEFT_S, poll=0.25)
        check(f'switch to {w}: the left takes {want[w]} within {LEFT_S} s', ok, f'{time.time() - t0:.1f} s: {s}')
        ok, s = wait(read, hs(want[w]), RIGHT_S)
        check(f'switch to {w}: the right follows within {RIGHT_S} s', ok, s)
        lines = log_lines(cfg, 'Foxglove accent: applied')[before:]
        said = [(int(m[1]), int(m[2])) for m in (re.search(r'applied (\d+)/(\d+)', l) for l in lines) if m]
        check(f'switch to {w}: an "accent: applied" line with it', any(near(x, want[w]) for x in said), lines[-2:])
restore_lights(args.app, args.lab, original)
check.finish()
