"""The sitting's old H1-H3 (#586) without the user: does the Bandit HUD flicker when the Win key leaves
a full-screen game? Now HA5, between HA1 (Diagnostics on, the GAME layer on) and HA2
(wallpaper_check.py game, which reads the devlog's reloads and Wallpaper Engine's log after it).

1. H1, a full-screen game on the HUD's monitor: a lab probe window (desktop.Probe, game=True)
   covers that monitor, borderless and topmost, and takes the foreground with a real click on
   itself. Wallpaper Engine sees a full-screen program (playbackfullscreen "stop" unloads the
   wallpaper under it). It minimizes itself when it loses the focus, as an exclusive full-screen
   game does on the Win key.
2. H2, the Win key: a real Win key tap (SendInput). Then the monitor is captured (GDI, what DWM
   composes, the wallpaper included) about 10 times a second for --watch-s: a frame is a flicker
   when it goes dark (the wallpaper unloaded or reloading) or jumps (a flash), against captures of
   the HUD taken before the game. The frames are kept as PNGs in --out for a later look, and a
   flicker's times go next to HA2's devlog gaps.
3. H3, close the game: Esc closes the Start menu (a real Esc tap), the probe window closes.
4. Foxglove's side (#586's fix): its log has one line per wallpaper push, with why it went. No
   push may land inside Wallpaper Engine's reload, the first RELOAD_QUIET_S after the Win key
   (wallpaper_sync.rs RELOAD_QUIET). With Wallpaper Engine's "fullscreen: Stop" the reloaded
   wallpaper gets the state once after it ("after a full-screen program"), which is printed.

The monitor is the one whose device name contains --monitor (Wallpaper Engine's key piece, SAM).
Only the band above the Start menu is measured (--band, the top 40 %), so the menu itself isn't a
flicker. A capture that sees only black before the game (a protected or exclusive surface) is
BLOCKED, not PASS. The probe is a borderless window: a game in exclusive full-screen (DXGI) may
behave differently, which the report says.

It takes the focus and makes input (the PC unlocked, the user away 2 min: BLOCKED otherwise), so it
runs only with FOXGLOVE_APP_OK=1 (On-call). It launches nothing.

    FOXGLOVE_APP_OK=1 python scripts/lab/game_flicker.py [--monitor SAM] [--out <dir>]   (exit 0 PASS, 1 FAIL, 2 BLOCKED)
"""
import argparse
import re
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from labkit import REPO, Checks, require_app_ok, require_user_idle  # noqa: E402
from wallpaper_check import FOXGLOVE_LOG, read_lines, utc  # noqa: E402

# wallpaper_sync.rs RELOAD_QUIET: Foxglove sends nothing this long after a full-screen program
# leaves the front.
RELOAD_QUIET_S = 2.0
RELOAD_REASON = 'after a full-screen program'


def band(rect, top_share):
    l, t, r, b = rect
    return [l, t, r, t + max(int((b - t) * top_share), 8)]


def pushes(lines, since):
    """[(time, line)] of Foxglove's wallpaper push lines ("wallpaper sync: ... sent to") at or
    after `since`'s second."""
    out = []
    floor = since.replace(microsecond=0)
    for l in lines:
        m = re.match(r'(\d{4}-\d\d-\d\d \d\d:\d\d:\d\dZ) ', l)
        if m and 'wallpaper sync:' in l and ' sent to ' in l and utc(m.group(1)) >= floor:
            out.append((utc(m.group(1)), l))
    return out


def into_reload(found, key_at, quiet_s=RELOAD_QUIET_S):
    """The pushes certainly inside the reload's quiet after the Win key at `key_at`: the log has
    whole seconds, so a line counts when its whole second is after the key and before the end."""
    end = key_at + timedelta(seconds=quiet_s)
    return [(t, l) for t, l in found if t >= key_at and t + timedelta(seconds=1) <= end]


def frames(desktop, rect, seconds, fps, out, tag):
    """[(t since start, luma, change from the previous)] of `rect` for `seconds`; every frame saved."""
    shots, last, start = [], None, time.time()
    n = 0
    while time.time() - start < seconds:
        w, h, buf = desktop.grab(rect)
        lum = desktop.luma(buf)
        shots.append((round(time.time() - start, 2), lum, desktop.change(last, buf) if last else 0.0))
        if out:
            desktop.save_png(out / f'{tag}-{n:03d}.png', w, h, buf)
        last, n = buf, n + 1
        time.sleep(max(0.0, 1 / fps - 0.01))
    return shots


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument('--monitor', default='SAM', help="a piece of the HUD monitor's device name (Wallpaper Engine's key)")
    ap.add_argument('--watch-s', type=float, default=6.0, help='how long to capture after the Win key')
    ap.add_argument('--settle-s', type=float, default=5.0, help='how long the game runs before the Win key')
    ap.add_argument('--band', type=float, default=0.4, help='the top share of the monitor measured')
    ap.add_argument('--fps', type=float, default=10.0)
    ap.add_argument('--out', help='folder for the frames (default lab-reports/game-flicker-<time>)')
    args = ap.parse_args(argv)
    require_app_ok()
    require_user_idle(120)
    import desktop
    check = Checks()
    out = Path(args.out) if args.out else REPO / 'lab-reports' / f'game-flicker-{datetime.now(timezone.utc):%Y%m%dT%H%M%SZ}'
    out.mkdir(parents=True, exist_ok=True)
    print(f'since (for wallpaper_check.py game --since): {datetime.now(timezone.utc):%Y-%m-%dT%H:%M:%SZ}', flush=True)
    found = [(d, r) for d, r in desktop.monitors() if desktop.monitor_matches(d, args.monitor)]
    if not check(f'one monitor named like {args.monitor!r}', len(found) == 1, found, blocked=len(found) != 1):
        return check.finish()
    device, rect = found[0]
    region = band(rect, args.band)
    baseline = [(l, c) for _, l, c in frames(desktop, region, 2.0, args.fps, out, 'before')]
    mean = sum(l for l, _ in baseline) / len(baseline)
    if not check('the capture sees the HUD before the game', mean > 3, f'luma {mean:.1f} on {device} {region}',
                 blocked=mean <= 3):
        return check.finish()

    # H1: the game.
    with desktop.Probe(rect, 'Foxglove lab: full-screen game (#586)', game=True) as game:
        got = game.focus()
        if not check('H1: the full-screen stand-in has the foreground', got, f'{rect}', blocked=not got):
            return check.finish()
        time.sleep(args.settle_s)  # bounded: Wallpaper Engine notices a full-screen program within a second or two
        under = frames(desktop, region, 1.0, args.fps, out, 'game')
        print(f'  while the game runs: luma {sum(l for _, l, _ in under) / len(under):.1f} (the stand-in is black)',
              flush=True)
        # H2: the Win key.
        key_at = datetime.now(timezone.utc)
        sent = desktop.key(desktop.VK_LWIN, 'H2 Win key in the full-screen game')
        check('H2: the Win key went in', sent == 2, f'{sent} events')
        after = frames(desktop, region, args.watch_s, args.fps, out, 'after')
        check('H2: the game minimized on the focus loss, as a full-screen game does', game.iconic(),
              'iconic' if game.iconic() else 'still shown')
        # H3: close the Start menu, then the game.
        desktop.key(desktop.VK_ESCAPE, 'H3 Esc closes the Start menu')
    # The first frames after the key show the game going: from the second the desktop is back.
    shown = [f for f in after if f[0] >= 0.5]
    m, flickers = desktop.flicker(baseline, shown)
    for t, why in flickers:
        print(f'  flicker at +{t:.2f} s: {why}', flush=True)
    check('H2: the HUD did not flicker after the Win key (#586)', not flickers,
          f'{len(flickers)} of {len(shown)} frames against the HUD\'s luma {m:.0f}; frames in {out} '
          '(a borderless stand-in; HA2 reads the devlog\'s reloads and DXGI resets for the cause)')
    end = frames(desktop, region, 1.0, args.fps, None, 'end')
    back = sum(l for _, l, _ in end) / len(end)
    check('H3: the HUD shows again after the game', back > mean * 0.5, f'luma {back:.0f} against {mean:.0f}')
    # Foxglove's side: nothing into the reload; the push after it, if any, is printed.
    found = pushes(read_lines(FOXGLOVE_LOG), key_at)
    for t, l in found:
        print(f'  push {t:%H:%M:%S}Z: {l.split("wallpaper sync: ", 1)[-1]}', flush=True)
    bad = into_reload(found, key_at)
    check(f'H2: Foxglove sent no wallpaper push in the {RELOAD_QUIET_S:.0f} s reload after the Win key (#586)',
          not bad, [l for _, l in bad][-3:] or f'{len(found)} push(es) since the key, none inside')
    after = [l for _, l in found if RELOAD_REASON in l]
    print(f'  reload re-push (Wallpaper Engine on Stop): {after[-1] if after else "none"}', flush=True)
    check.finish()


if __name__ == '__main__':
    main()
