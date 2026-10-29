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

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EFFECTS = ["solid", "breathe", "spectrum", "swirl"]  # index = the keyboard's effect number

PRESETS = [
    {"id": "bandit", "name": "Bandit", "description": "The Bandit orange of the fox and the HUD.",
     "color": {"hue": 22, "saturation": 92, "brightness": 60}, "effect": "solid", "swatch": "#e5732b",
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
     "color": {"hue": 22, "saturation": 100, "brightness": 50}, "effect": "swirl", "swatch": "#e5732b",
     "wallpapers": [], "art": {"builtin": "Bandit_Emblem"}},
]


def hsv_hex(h, s, v=1.0):
    r, g, b = colorsys.hsv_to_rgb(h / 360, s, v)
    return "#%02x%02x%02x" % (round(r * 255), round(g * 255), round(b * 255))


def chip(p):
    """64x24 rounded chip: solid fill; breathe fades to dark at the ends; swirl is a rainbow."""
    c, e = p["swatch"], p["effect"]
    if e == "swirl":
        stops = "".join('<stop offset="%g" stop-color="%s"/>' % (i / 6, hsv_hex(i * 60, 0.9)) for i in range(7))
        fill = "url(#g)"
        defs = '<linearGradient id="g" x1="0" x2="1">%s</linearGradient>' % stops
    elif e == "breathe":
        fill = "url(#g)"
        defs = ('<linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="%s" stop-opacity="0.25"/>'
                '<stop offset="0.5" stop-color="%s"/><stop offset="1" stop-color="%s" stop-opacity="0.25"/>'
                '</linearGradient>' % (c, c, c))
    else:
        fill, defs = c, ""
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="64" height="24" viewBox="0 0 64 24">\n'
            '<defs>%s</defs>\n'
            '<rect x="0.5" y="0.5" width="63" height="23" rx="6" fill="#16171b" stroke="#2c2e35"/>\n'
            '<rect x="3" y="3" width="58" height="18" rx="4" fill="%s"/>\n'
            '</svg>\n' % (defs, fill))


ART_LIMIT = 12 * 1024
WALLPAPERS = os.environ.get("BANDIT_WALLPAPERS", r"C:\Users\fuchs\bandit-wallpapers")
BANDIT_GIFS = os.environ.get("BANDIT_GIFS", r"C:\Users\fuchs\splitkeeb\art-src\bandit\gifs")


def copy_art(res):
    """Copies each preset's right-screen set from the wallpaper repo's keyboard build when it's
    there (the committed copies stay otherwise), plus a preview GIF for the docs."""
    import shutil

    os.makedirs(os.path.join(res, "lighting", "art"), exist_ok=True)
    gifs = os.path.join(ROOT, "docs", "lighting-art")
    os.makedirs(gifs, exist_ok=True)
    for p in PRESETS:
        art = p["art"]
        if "file" in art:
            src = os.path.join(WALLPAPERS, "dist", "keyboard", art["from_wallpaper"])
            dst = os.path.join(res, "lighting", "art", art["file"])
            if os.path.exists(os.path.join(src, "right.bin")):
                shutil.copy2(os.path.join(src, "right.bin"), dst)
                shutil.copy2(os.path.join(src, "preview_right.gif"), os.path.join(gifs, p["id"] + ".gif"))
            blob = open(dst, "rb").read()
            assert blob[:4] == b"NVA1" and len(blob) <= ART_LIMIT, (p["id"], len(blob))
        else:
            gif = os.path.join(BANDIT_GIFS, "Bandit_" + art["builtin"][len("Bandit_"):].lower() + ".gif")
            if os.path.exists(gif):
                shutil.copy2(gif, os.path.join(gifs, p["id"] + ".gif"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sheet")
    a = ap.parse_args()
    for p in PRESETS:
        assert p["effect"] in EFFECTS, p["id"]
    data = {"format": "foxglove-lighting-presets", "version": 1, "effects": EFFECTS, "presets": PRESETS}
    res = os.path.join(ROOT, "app", "resources")
    with open(os.path.join(res, "lighting-presets.json"), "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, indent=2)
        f.write("\n")
    os.makedirs(os.path.join(res, "lighting"), exist_ok=True)
    for p in PRESETS:
        with open(os.path.join(res, "lighting", p["id"] + ".svg"), "w", encoding="utf-8", newline="\n") as f:
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
            d.text((150, y + 2), "%s  (%s)" % (p["name"], p["effect"]), fill="#ece8e1")
            d.text((150, y + 18), "%s   LED h%d s%d b%d   %s" % (p["description"], p["color"]["hue"],
                   p["color"]["saturation"], p["color"]["brightness"], ", ".join(p["wallpapers"]) or "-"),
                   fill="#9a968e")
        im.save(a.sheet)
        print("sheet", a.sheet)


if __name__ == "__main__":
    main()
