"""Writes the curated underglow presets (app/resources/lighting-presets.json) and their preview
chips (app/resources/lighting/<id>.svg). Spec: docs/lighting-presets.md.

  python -I scripts/lighting_presets.py                 write both
  python -I scripts/lighting_presets.py --sheet x.png   also a review sheet (needs resvg-py)

Each preset matches a Bandit wallpaper family. Hue and saturation are tuned for the WS2812
underglow, where low saturation washes out to white, so they are stronger than the wallpapers'
screen colours; "swatch" is the screen colour the chip shows.
"""

import argparse
import colorsys
import io
import json
import os
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EFFECTS = ["solid", "breathe", "spectrum", "swirl"]  # index = the keyboard's effect number

PRESETS = [
    {"id": "bandit", "name": "Bandit", "description": "The Bandit orange of the fox and the HUD.",
     "color": {"hue": 22, "saturation": 92, "brightness": 60}, "effect": "solid", "swatch": "#f08a3c",
     "wallpapers": ["hud", "bandit-radar", "bandit-skyline"], "art": {"builtin": "Bandit_Emblem"}},
    {"id": "fury", "name": "Fury", "description": "Linocut red, breathing slowly like ink on a press.",
     "color": {"hue": 2, "saturation": 95, "brightness": 60}, "effect": "breathe", "swatch": "#c8342b",
     "wallpapers": ["fury-print", "fury-chase", "fury-pattern", "fury-forest"], "art": {"file": "fury.bin", "from_wallpaper": "fury-print", "fallback": "Bandit_Emblem"}},
    {"id": "constellations", "name": "Constellations", "description": "Night-sky indigo, breathing like starlight.",
     "color": {"hue": 232, "saturation": 85, "brightness": 50}, "effect": "breathe", "swatch": "#3f4fb0",
     "wallpapers": ["fury-stars"], "art": {"file": "constellations.bin", "from_wallpaper": "fury-stars", "fallback": "Bandit_Emblem"}},
    {"id": "hearth", "name": "Hearth", "description": "Candlelight amber, flickering softly.",
     "color": {"hue": 30, "saturation": 88, "brightness": 45}, "effect": "breathe", "swatch": "#e2a048",
     "wallpapers": ["hearth-cauldron", "hearth-wheel"], "art": {"file": "hearth.bin", "from_wallpaper": "hearth-cauldron", "fallback": "Bandit_Emblem"}},
    {"id": "moon-garden", "name": "Moon Garden", "description": "Night-garden violet, steady.",
     "color": {"hue": 282, "saturation": 70, "brightness": 45}, "effect": "solid", "swatch": "#b98ad0",
     "wallpapers": ["hearth-moongarden"], "art": {"file": "moon-garden.bin", "from_wallpaper": "hearth-moongarden", "fallback": "Bandit_Emblem"}},
    {"id": "courier", "name": "Courier", "description": "Dossier paper: a quiet warm white.",
     "color": {"hue": 35, "saturation": 25, "brightness": 35}, "effect": "solid", "swatch": "#e6e1d6",
     "wallpapers": ["courier"], "art": {"builtin": "Bandit_Emblem"}},
    {"id": "signal", "name": "Signal", "description": "Every colour, swirling: for parties and demos.",
     "color": {"hue": 22, "saturation": 100, "brightness": 50}, "effect": "swirl", "swatch": "#f08a3c",
     "wallpapers": [], "art": {"builtin": "Bandit_Emblem"}},
]


def hsv_hex(h, s, v=1.0):
    r, g, b = colorsys.hsv_to_rgb(h / 360, s, v)
    return f"#{round(r * 255):02x}{round(g * 255):02x}{round(b * 255):02x}"


def chip(p):
    """64x24 rounded chip: solid fill; breathe fades to dark at the ends; swirl is a rainbow."""
    c, e = p["swatch"], p["effect"]
    if e == "swirl":
        stops = "".join(f'<stop offset="{i / 6:g}" stop-color="{hsv_hex(i * 60, 0.9)}"/>' for i in range(7))
        fill = "url(#g)"
        defs = f'<linearGradient id="g" x1="0" x2="1">{stops}</linearGradient>'
    elif e == "breathe":
        fill = "url(#g)"
        defs = (f'<linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="{c}" stop-opacity="0.25"/>'
                f'<stop offset="0.5" stop-color="{c}"/><stop offset="1" stop-color="{c}" stop-opacity="0.25"/>'
                '</linearGradient>')
    else:
        fill, defs = c, ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="64" height="24" viewBox="0 0 64 24">\n'
            f'<defs>{defs}</defs>\n'
            '<rect x="0.5" y="0.5" width="63" height="23" rx="6" fill="#16171b" stroke="#2c2e35"/>\n'
            f'<rect x="3" y="3" width="58" height="18" rx="4" fill="{fill}"/>\n'
            '</svg>\n')


ART_LIMIT = 12 * 1024
WALLPAPERS = os.environ.get("BANDIT_WALLPAPERS", r"C:\Users\fuchs\bandit-wallpapers")
BANDIT_GIFS = os.environ.get("BANDIT_GIFS", r"C:\Users\fuchs\splitkeeb\art-src\bandit\gifs")


def copy_art(res):
    """Copies each preset's right-screen set from the wallpaper repo's keyboard build when it's
    there (the committed copies stay otherwise), plus a preview GIF for the docs."""
    import shutil

    (Path(res) / "lighting" / "art").mkdir(parents=True, exist_ok=True)
    gifs = ROOT / "docs" / "lighting-art"
    gifs.mkdir(parents=True, exist_ok=True)
    for p in PRESETS:
        art = p["art"]
        if "file" in art:
            src = Path(WALLPAPERS) / "dist" / "keyboard" / art["from_wallpaper"]
            dst = Path(res) / "lighting" / "art" / art["file"]
            if (src / "right.bin").exists():
                shutil.copy2(src / "right.bin", dst)
                shutil.copy2(src / "preview_right.gif", gifs / (p["id"] + ".gif"))
            blob = open(dst, "rb").read()
            assert blob[:4] == b"NVA1" and len(blob) <= ART_LIMIT, (p["id"], len(blob))
        else:
            gif = Path(BANDIT_GIFS) / ("Bandit_" + art["builtin"][len("Bandit_"):].lower() + ".gif")
            if gif.exists():
                shutil.copy2(gif, gifs / (p["id"] + ".gif"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sheet")
    a = ap.parse_args()
    for p in PRESETS:
        assert p["effect"] in EFFECTS, p["id"]
    data = {"format": "foxglove-lighting-presets", "version": 1, "effects": EFFECTS, "presets": PRESETS}
    res = ROOT / "app" / "resources"
    with open(res / "lighting-presets.json", "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    (res / "lighting").mkdir(parents=True, exist_ok=True)
    for p in PRESETS:
        with open(res / "lighting" / (p["id"] + ".svg"), "w", encoding="utf-8", newline="\n") as f:
            f.write(chip(p))
    copy_art(res)
    print("wrote", len(PRESETS), "presets")
    if a.sheet:
        import resvg_py
        from PIL import Image, ImageDraw

        im = Image.new("RGB", (760, 46 * len(PRESETS) + 10), "#1b1b1f")
        d = ImageDraw.Draw(im)
        for i, p in enumerate(PRESETS):
            y = 8 + i * 46
            png = bytes(resvg_py.svg_to_bytes(svg_string=chip(p), width=96))
            im.paste(Image.open(io.BytesIO(png)).convert("RGB"), (8, y))
            # the LED colour itself, as hue/saturation at full value
            led = hsv_hex(p["color"]["hue"], p["color"]["saturation"] / 100)
            d.rectangle([114, y + 4, 138, y + 28], fill=led)
            d.text((150, y + 2), f"{p['name']}  ({p['effect']})", fill="#ece8e1")
            d.text((150, y + 18), f"{p['description']}   LED h{p['color']['hue']} s{p['color']['saturation']}"
                   f" b{p['color']['brightness']}   {', '.join(p['wallpapers']) or '-'}",
                   fill="#9a968e")
        im.save(a.sheet)
        print("sheet", a.sheet)


if __name__ == "__main__":
    main()
