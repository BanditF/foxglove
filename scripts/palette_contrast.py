"""Contrast check for the shared palettes (`foxglove-palette` v1, docs/palette.md).

    python scripts/palette_contrast.py [--markdown] [file.json ...]

With no files it checks every palette in app/resources/palettes. It resolves the derived roles
the way the Rust reader does (`palette_file.rs`: `deep` is the accent clamped to 3:1 on
`paper`, `accent_light` to 4.5:1, `accent2_light` / `accent3_light` likewise), then checks
every text pair at WCAG 4.5:1 and every graphics pair at 3:1. Exit 1 when a pair fails.

Bold-tier palettes (`"tier": "bold"`, docs/palette.md "Bold tier") may tint their grounds with
`accent2` (the wash: 12 % on the dark grounds, 8 % on `paper`). Their pairs are checked on the
washed grounds too, and their derived light roles are clamped against the washed `paper`.
"""

from __future__ import annotations

import colorsys
import json
import sys
from pathlib import Path

PALETTES = Path(__file__).resolve().parent.parent / "app" / "resources" / "palettes"
BANDIT_ACCENT = "#f08a3c"
TEXT, GRAPHIC = 4.5, 3.0
DARK = ("base", "app_ground", "wall_ground")
# The light theme's card surface (theme.rs `light().surface`): not a palette role, but where the
# app draws `accent_light` text.
APP_SURFACE = "#ffffff"
# Area budget per tier (share of a still surface's marks: everything that isn't ground or wash),
# for accent2 and accent3. "harmony" is the default when a file has no `tier`.
TIERS = {"harmony": (0.15, 0.03), "bold": (0.35, 0.05)}
# The bold tier's wash: how much accent2 may tint a ground, dark and light.
WASH_DARK, WASH_LIGHT = 0.12, 0.08
# A bold ground is a deep shade: HSL lightness at most this.
BOLD_GROUND_MAX_L = 0.14


def rgb(c: str) -> tuple[int, int, int]:
    c = c.lstrip("#")
    if len(c) != 6:
        raise ValueError(f"{c!r} isn't #rrggbb")
    return tuple(int(c[i : i + 2], 16) for i in (0, 2, 4))  # type: ignore[return-value]


def hexc(c: tuple[int, int, int]) -> str:
    return "#%02x%02x%02x" % c


def luminance(c: str) -> float:
    def ch(v: int) -> float:
        x = v / 255
        return x / 12.92 if x <= 0.04045 else ((x + 0.055) / 1.055) ** 2.4

    r, g, b = rgb(c)
    return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)


def contrast(a: str, b: str) -> float:
    x, y = luminance(a), luminance(b)
    return (max(x, y) + 0.05) / (min(x, y) + 0.05)


def _at(h: float, s: float, l: float) -> str:
    r, g, b = colorsys.hls_to_rgb(h, l, s)
    return hexc(tuple(round(min(max(v, 0.0), 1.0) * 255) for v in (r, g, b)))  # type: ignore[arg-type]


def mix(a: str, b: str, t: float) -> str:
    """`a` with `t` of `b` mixed in, per sRGB channel (how the mocks and wallpapers tint)."""
    x, y = rgb(a), rgb(b)
    return hexc(tuple(round(x[i] + (y[i] - x[i]) * t) for i in range(3)))  # type: ignore[arg-type]


def lightness(c: str) -> float:
    return colorsys.rgb_to_hls(*(v / 255 for v in rgb(c)))[1]


def readable(colour: str, ground: str, minimum: float) -> str:
    """Same hue and saturation (HSL), darker or lighter only as far as `minimum` needs:
    `palette_file::readable` in the app and `FoxgloveLive.readable` in bandit-wallpapers."""
    ok = lambda c: contrast(c, ground) >= minimum  # noqa: E731
    if ok(colour):
        return colour
    h, l, s = colorsys.rgb_to_hls(*(v / 255 for v in rgb(colour)))
    pale = luminance(ground) > 0.18
    near, far = l, (0.0 if pale else 1.0)
    if not ok(_at(h, s, far)):
        return _at(h, s, far)
    for _ in range(24):
        mid = (near + far) / 2
        if ok(_at(h, s, mid)):
            far = mid
        else:
            near = mid
    x, out = far, _at(h, s, far)
    for _ in range(20):
        if ok(out):
            break
        x = max(x - 0.005, 0.0) if pale else min(x + 0.005, 1.0)
        out = _at(h, s, x)
    return out


def resolve(path: Path) -> dict:
    """The palette with its derived roles filled in. Grounds are taken as written (the harmony
    palettes write all of them; Bandit's is the fallback for any that are missing)."""
    data = json.loads(path.read_text(encoding="utf-8"))
    bandit = json.loads((PALETTES / "bandit.json").read_text(encoding="utf-8"))["roles"]
    r = dict(data.get("roles", {}))
    for g in ("base", "ink", "paper", "paper_ink"):
        r.setdefault(g, bandit[g])
    r.setdefault("app_ground", r["base"])
    r.setdefault("wall_ground", bandit["wall_ground"])
    tier = data.get("tier", "harmony")
    if tier not in TIERS:
        raise ValueError(f"{path.name}: tier {tier!r} isn't one of {sorted(TIERS)}")
    if "accent3" in r and "accent2" not in r:
        raise ValueError(f"{path.name}: accent3 needs accent2")
    if tier == "bold":
        if "accent2" not in r:
            raise ValueError(f"{path.name}: the bold tier needs accent2")
        for g in DARK:
            r[f"{g}_wash"] = mix(r[g], r["accent2"], WASH_DARK)
        r["paper_wash"] = mix(r["paper"], r["accent2"], WASH_LIGHT)
    light = r.get("paper_wash", r["paper"])  # the darker of the light grounds
    r.setdefault("deep", readable(r["accent"], light, GRAPHIC))
    r.setdefault("accent_light", readable(r["accent"], r["paper"], TEXT))
    for k in ("accent2", "accent3"):
        if k in r:
            r.setdefault(f"{k}_light", readable(r[k], light, TEXT))
    for k, v in r.items():
        rgb(v)
    r["app_surface"] = APP_SURFACE
    return {"name": data["name"], "harmony": data.get("harmony", ""), "tier": tier,
            "roles": r, "stops": data["stops"]}


def pairs(roles: dict) -> list[tuple[str, str, float]]:
    """(foreground role, ground role, minimum) for every pair the palette promises."""
    out = []
    dark = list(DARK) + [f"{g}_wash" for g in DARK if f"{g}_wash" in roles]
    light = ["paper"] + (["paper_wash"] if "paper_wash" in roles else [])
    for fg in ("ink", "accent", "highlight", "accent2", "accent3"):
        if fg in roles:
            out += [(fg, g, TEXT) for g in dark]
    for fg in ("paper_ink", "accent2_light", "accent3_light"):
        if fg in roles:
            out += [(fg, g, TEXT) for g in light]
    # Bandit's hand-picked accent_light is 4.5:1 on the app's white cards (theme.rs checks it)
    # but only 4.0:1 on paper: graphics and large text there, small text on white.
    out.append(("accent_light", "app_surface", TEXT))
    out += [("accent_light", g, GRAPHIC) for g in light]
    out += [("deep", g, GRAPHIC) for g in light]
    return out


def tier_problems(p: dict) -> list[str]:
    """The bold tier's own promise: its night grounds are deep shades (the duotone ground)."""
    r, bad = p["roles"], []
    if p["tier"] == "bold":
        for g in DARK:
            if lightness(r[g]) > BOLD_GROUND_MAX_L:
                bad.append(f"{g} {r[g]} is lighter than {BOLD_GROUND_MAX_L:.0%} (HSL)")
    return bad


def check(p: dict) -> list[tuple[str, str, float, float, bool]]:
    r = p["roles"]
    return [(f, g, contrast(r[f], r[g]), m, contrast(r[f], r[g]) >= m) for f, g, m in pairs(r)]


def main(argv: list[str]) -> int:
    md = "--markdown" in argv
    files = [Path(a) for a in argv if not a.startswith("--")] or sorted(PALETTES.glob("*.json"))
    bad = 0
    for f in files:
        p = resolve(f)
        rows = check(p)
        tag = f", {p['tier']} tier" if p["tier"] != "harmony" else ""
        print(f"\n## {p['name']} ({f.name}{tag})" if md else f"{p['name']} ({f.name}{tag})")
        if md:
            print("\n| Foreground | Ground | Ratio | Needs | |\n|---|---|---|---|---|")
        for fg, g, ratio, m, ok in rows:
            bad += not ok
            mark = "ok" if ok else "FAIL"
            if md:
                print(f"| `{fg}` {p['roles'][fg]} | `{g}` {p['roles'][g]} | {ratio:.2f} | {m:g} | {mark} |")
            else:
                print(f"  {fg:>14} on {g:<16} {ratio:5.2f} (>= {m:g}) {mark}")
        for problem in tier_problems(p):
            bad += 1
            print(f"  FAIL {problem}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
