"""Convert Flipper Zero art into nice!view art for the nice_view_custom shield.

Accepts, in any mix:
  * a Flipper animation folder (meta.txt + frame_N.bm or frame_N.png), as found in the
    official firmware (assets/dolphin/external/*) and community packs
  * a single image file (.png/.gif/.jpg/.bmp)
  * a single compiled Flipper bitmap named like name_128x64.bm
  * an LVGL C file of 1-bit images (LV_IMG_CF_INDEXED_1BIT / LV_COLOR_FORMAT_I1), as used by
    nice!view art modules; these are already stored sideways, so they come out exactly as they
    looked on their original nice!view. All images in the file become one animation.

Each input becomes one "art set" on the right-hand screen. The nice!view is mounted portrait, so
art is fitted upright into a 68x140 area (below the status strip) and stored rotated 90 degrees
clockwise. Options go after the input, separated by colons:
  :landscape  store it as-is in the 140x68 buffer, which reads sideways on the keyboard
  :stills     make every frame its own still art set instead of one animation
  :fps=N      override the frame rate (default: meta.txt, or 2 for LVGL C files)
  :name=X     override the set name (default: the file or folder name)
  :label=Text override the display label (default: set name with underscores replaced by spaces)
  :labels=A|B|C  with :stills, label each frame in order; missing labels use the default
  :polarity=P which way round the source's ink is (#666): auto (the default), as-is or invert
  Labels may contain spaces but no colons.

Animations keep all their frames
(in meta.txt order) and frame rate; the firmware shows the first frame unless animation is
enabled with CONFIG_NICE_VIEW_CUSTOM_ANIMATE. Frames are stored as RLE-packed XOR deltas and
unpacked into one RAM image buffer on the keyboard.

Usage:
  python scripts/flipper_art.py OUT.c INPUT[:OPTION...] [INPUT[:OPTION...] ...] [--preview preview.png]
  python scripts/flipper_art.py OUT.bin INPUT[:OPTION...] [--preview preview.png]
  python scripts/flipper_art.py OUT.bin INPUT[:OPTION...] --size WxH[:ROT] [--variant SET]
  Quote inputs with spaces or pipes, e.g. "INPUT:stills:labels=First frame|Second frame".

OUT.c also writes OUT.json with built-in set names, labels, frame counts, and frame rates.
Use :label=Text for one set or :labels=A|B|C with :stills for individual labels.

An OUT.bin is one art set as a blob to push from the PC without flashing (hostpanel push-art
OUT.bin); the format is in boards/shields/nice_view_custom/widgets/art_relay.h.

Bits: 1 = ink (#610), the same as the left's PC info screen. Ink shows in the panel polarity's
foreground (#666: dark or light, set from Foxglove; light on dark by default). Blobs carry
ART_RELAY_FLAG_INK in header byte 7; blobs from before #610 (flags 0, background bits set) still
play correctly on the keyboard, which inverts them.

Polarity (#666): every source is normalised so that its drawing is the ink and its background the
field, whichever way round it was drawn. :polarity=auto (the default) finds the background: the
majority of the edge pixels of every frame of the set, at 50%, after fitting; a tie goes to the
majority of all their pixels. A light background means the dark pixels are the ink (as-is); a
dark background means the light ones are, so the source is inverted. as-is takes the dark pixels
as ink whatever the background, invert the light ones. The whole set goes one way, so an
animation never flips mid-way. app/src/foxglove/art_encode.rs does the same (parity tests with
tests/art/ink-fixture and tests/art/dark-fixture).

Other art screens (#835): --size WxH[:ROT] makes a set for a screen that isn't the Corne's. W x H
is the art upright, as drawn; ROT is how many quarter turns clockwise it is stored turned (0 by
default), so the blob is W x H for ROT 0 or 2 and H x W for 1 or 3, at most 255 each way. The fit
is the app's (art_encode.rs): art too big is scaled down evenly until it fits, smaller art keeps
its size, either way it is centred on the field; never stretched or scaled up. It scales with the
app's filter (bilinear), so the app and the script make the same blob (parity test with
tests/art/ink-fixture-144x112.bin); without --size the Corne's output is as it always was
(Pillow's bicubic). --size 68x140:1 is the Corne's own format: the same bytes as no --size for
art that needs no scaling. With --size, OUT is a .bin (no :landscape, which ROT replaces).

--variant SET writes the fitted frames as the set's folder for that screen, SET/<W>x<H>/
(meta.txt + frame_N.png, upright, ink black on a white field), which Foxglove uses in place of
fitting the set's own frames on a board with that art screen (#831). It needs --size and one set.

The preview shows the screen as you see it on the keyboard: portrait, status strip on top, in the
default dark theme (light ink on a dark field). With --size it shows each set's first frame upright.

Requires: pip install pillow heatshrink2
"""

import argparse
import json
import os
import re
import struct
import sys
from collections import namedtuple

from PIL import Image

FLAG_INK = 0x01  # blob header byte 7 (art_relay.h ART_RELAY_FLAG_INK): set bits are ink
ART_W, ART_H = 140, 68  # stored art buffer, left of the status strip on the right-hand nice!view
SCREEN_W, STRIP_W = 160, 20  # stored screen; the status strip is the top 20px when held upright

# An art screen (art_encode.rs ArtTarget): the stored buffer's size, and how many quarter turns
# clockwise the upright art is turned to it.
Target = namedtuple("Target", "width height rotation")
FIRMWARE = Target(ART_W, ART_H, 1)  # the Corne's: mounted portrait, stored turned
LANDSCAPE = Target(ART_W, ART_H, 0)  # :landscape, stored as drawn


def target_of(landscape):
    """A set's target: a Target from --size, else the Corne's, as drawn for :landscape."""
    if isinstance(landscape, Target):
        return landscape
    return LANDSCAPE if landscape else FIRMWARE


def upright(target):
    """The art's size upright, as drawn."""
    if target.rotation % 2:
        return target.height, target.width
    return target.width, target.height


def parse_size(text):
    """--size WxH[:ROT]: upright W x H, stored ROT quarter turns clockwise."""
    m = re.fullmatch(r"\s*(\d+)\s*[xX]\s*(\d+)\s*(?::\s*(\d+)\s*)?", text)
    if not m:
        raise argparse.ArgumentTypeError("%r: give the upright size as WxH or WxH:ROT" % text)
    w, h, rot = int(m.group(1)), int(m.group(2)), int(m.group(3) or 0)
    if rot > 3:
        raise argparse.ArgumentTypeError("%r: ROT is 0 to 3 quarter turns" % text)
    if not (1 <= w <= 255 and 1 <= h <= 255):
        raise argparse.ArgumentTypeError("%r: a blob holds 1 to 255 pixels each way" % text)
    return Target(h, w, rot) if rot % 2 else Target(w, h, rot)


def decode_bm(data, w, h):
    """Flipper .bm: [0x00] + raw, or [0x01, 0x00, len_lo, len_hi] + heatshrink(8, 4)."""
    if data[0] == 0x01:
        import heatshrink2

        data = heatshrink2.decompress(data[4:], window_sz2=8, lookahead_sz2=4)
    elif data[0] == 0x00:
        data = data[1:]
    else:
        raise ValueError("unknown .bm header byte %#x" % data[0])
    stride = (w + 7) // 8
    img = Image.new("1", (w, h), 1)
    px = img.load()
    for y in range(h):
        for x in range(w):
            byte = data[y * stride + x // 8]
            if byte >> (x % 8) & 1:  # XBM order: LSB is the leftmost pixel; 1 = ink
                px[x, y] = 0
    return img


def parse_meta(path):
    meta = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        if ":" in line:
            k, v = line.split(":", 1)
            meta[k.strip()] = v.strip()
    return meta


def load_frame(folder, index, w, h):
    for ext in (".png", ".bm"):
        p = os.path.join(folder, "frame_%d%s" % (index, ext))
        if os.path.exists(p):
            if ext == ".png":
                return Image.open(p)
            return decode_bm(open(p, "rb").read(), w, h)
    raise FileNotFoundError("frame_%d not found in %s" % (index, folder))


def decode_lvgl_c(path):
    """Upright images from an LVGL C file of 1-bit indexed images (palette + MSB-first rows)."""
    src = open(path, encoding="utf-8", errors="replace").read()
    maps = {}
    for m in re.finditer(r"uint8_t\s+(\w+)\[\]\s*=\s*\{(.*?)\};", src, re.S):
        body = m.group(2)
        # Use the non-inverted palette: the #else branch of CONFIG_NICE_VIEW_WIDGET_INVERTED.
        body = re.sub(r"#if[^\n]*\n(.*?)#else[^\n]*\n(.*?)#endif", r"\2", body, flags=re.S)
        body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
        maps[m.group(1)] = bytes(int(x, 16) for x in re.findall(r"0x([0-9a-fA-F]{1,2})", body))
    images = []
    for m in re.finditer(r"lv_img_dsc_t\s+\w+\s*=\s*\{(.*?)\};", src, re.S):
        body = m.group(1)
        if not re.search(r"INDEXED_1BIT|FORMAT_I1\b", body):
            continue
        w = int(re.search(r"\.header\.w\s*=\s*(\d+)", body).group(1))
        h = int(re.search(r"\.header\.h\s*=\s*(\d+)", body).group(1))
        data = maps[re.search(r"\.data\s*=\s*\(?[^\w]*(\w+)", body).group(1)]
        ink = 1 if data[0:3] == b"\xff\xff\xff" else 0  # index whose colour is black
        stride = (w + 7) // 8
        img = Image.new("1", (w, h), 1)
        px = img.load()
        for y in range(h):
            for x in range(w):
                if (data[8 + y * stride + x // 8] >> (7 - x % 8) & 1) == ink:
                    px[x, y] = 0
        images.append(img.rotate(90, expand=True))  # stored sideways -> upright
    if not images:
        sys.exit("%s: no 1-bit LVGL images found" % path)
    return images


def load_input(spec, size=None):
    """Returns [(name, [PIL images], fps, landscape, label, invert)]: one set, or one per still;
    invert says the set is drawn light on dark (its polarity, #666). With a --size Target, that
    Target stands in for landscape."""
    path, opts = spec, {}
    while True:
        m = re.search(
            r":(landscape|stills|fps=\d+|name=\w+|polarity=[\w-]+|labels?=[^:]+)$", path, re.I
        )
        if not m:
            break
        k, _, v = m.group(1).partition("=")
        opts[k.lower()] = v
        path = path[: m.start()]
    name, frames, fps = load_frames(path)
    name = opts.get("name", name)
    if "fps" in opts:
        fps = int(opts["fps"])
    landscape = "landscape" in opts
    if size is not None:
        if landscape:
            sys.exit("%s: --size's ROT says how the art is stored; drop :landscape" % spec)
        landscape = size
    polarity = opts.get("polarity", "auto").lower()
    if polarity not in POLARITIES:
        sys.exit("%s: polarity is one of %s" % (spec, ", ".join(POLARITIES)))
    if "stills" in opts:
        labels = opts["labels"].split("|") if "labels" in opts else []
        sets = []
        for i, f in enumerate(frames):
            set_name = "%s_%d" % (name, i + 1)
            label = labels[i] if i < len(labels) else set_name.replace("_", " ")
            if "labels" not in opts:
                label = opts.get("label", label)
            invert = should_invert([f], landscape, polarity)
            sets.append((set_name, [f], 0, landscape, label, invert))
        return sets
    label = opts.get("label", name.replace("_", " "))
    invert = should_invert(frames, landscape, polarity)
    return [(name, frames, fps if len(frames) > 1 else 0, landscape, label, invert)]


def load_frames(path):
    name = re.sub(r"[^A-Za-z0-9]+", "_", os.path.basename(os.path.normpath(path))).strip("_")
    if os.path.isdir(path):
        meta = parse_meta(os.path.join(path, "meta.txt"))
        w, h = int(meta["Width"]), int(meta["Height"])
        order = [int(i) for i in meta.get("Frames order", "0").split()] or [0]
        cache = {}
        frames = []
        for i in order:
            if i not in cache:
                cache[i] = load_frame(path, i, w, h)
            frames.append(cache[i])
        return name, frames, int(meta.get("Frame rate", "2") or 2)
    if path.lower().endswith(".bm"):
        m = re.search(r"(\d+)x(\d+)", os.path.basename(path))
        if not m:
            sys.exit("%s: name a single .bm like name_128x64.bm so its size is known" % path)
        w, h = int(m.group(1)), int(m.group(2))
        return name, [decode_bm(open(path, "rb").read(), w, h)], 0
    if path.lower().endswith(".c"):
        name = re.sub(r"_c$", "", name)
        return name, decode_lvgl_c(path), 2
    return name, [Image.open(path)], 0


POLARITIES = ("auto", "as-is", "invert")


def resample_of(landscape):
    """The filter a set is scaled down with: a --size set uses bilinear, which is what the app's
    fit uses (art_encode.rs, image's Triangle), so both make the same blob; the Corne's built-in
    format keeps Pillow's default (bicubic), so its output stays as it was."""
    return Image.BILINEAR if isinstance(landscape, Target) else None


def fitted(img, w, h, resample=None):
    """The source in grey, scaled down to fit w x h (never upscaled), before any padding."""
    img = img.convert("L")
    if img.width > w or img.height > h:
        scale = min(w / img.width, h / img.height)
        size = (max(1, int(img.width * scale)), max(1, int(img.height * scale)))
        img = img.resize(size) if resample is None else img.resize(size, resample)
    return img


def dark_counts(img):
    """(dark edge pixels, edge pixels, dark pixels, pixels) of a fitted source, at 50%."""
    px = img.load()
    w, h = img.size
    edge_dark = edge = dark = 0
    for y in range(h):
        for x in range(w):
            d = px[x, y] < 128
            dark += d
            if x == 0 or y == 0 or x == w - 1 or y == h - 1:
                edge += 1
                edge_dark += d
    return edge_dark, edge, dark, w * h


def should_invert(frames, landscape, polarity="auto"):
    """Whether a set's sources are drawn light on dark, so their light pixels are the ink (#666).
    auto: the edge pixels of every frame decide (a dark majority means a dark background); a tie
    goes to all their pixels. as-is never inverts, invert always does."""
    if polarity == "as-is":
        return False
    if polarity == "invert":
        return True
    w, h = upright(target_of(landscape))
    edge_dark = edge = dark = total = 0
    for f in frames:
        a, b, c, d = dark_counts(fitted(f, w, h, resample_of(landscape)))
        edge_dark, edge, dark, total = edge_dark + a, edge + b, dark + c, total + d
    if edge_dark * 2 != edge:
        return edge_dark * 2 > edge
    return dark * 2 > total


def fit(img, w, h, invert=False, resample=None):
    """Fit into w x h (never upscales), centre, 1-bit; a light-on-dark source is inverted first,
    so the padding stays the field (#666)."""
    img = fitted(img, w, h, resample)
    if invert:
        img = img.point(lambda v: 255 - v)
    canvas = Image.new("L", (w, h), 255)
    canvas.paste(img, ((w - img.width) // 2, (h - img.height) // 2))
    return canvas.point(lambda v: 255 if v >= 128 else 0, "1")


def to_stored(img, landscape, invert=False):
    """The 1-bit image to store (ART_W x ART_H, or a --size Target's). The upright view of the
    Corne's stored buffer is the buffer rotated 90 degrees counter-clockwise, so portrait art is
    rotated clockwise here; a Target turns it its rotation's quarter turns clockwise."""
    target = target_of(landscape)
    return fit(img, *upright(target), invert, resample_of(landscape)).rotate(-90 * (target.rotation % 4), expand=True)


def to_lvgl_bytes(img):
    """LVGL INDEXED_1BIT rows, MSB first. Ink (a black source pixel) -> a set bit, index 1, which
    the firmware draws in LVGL_FOREGROUND (peripheral_status.c). Padding bits stay clear."""
    width, height = img.size
    stride = (width + 7) // 8
    out = bytearray(stride * height)
    px = img.load()
    for y in range(height):
        for x in range(width):
            if not px[x, y]:  # ink -> bit set
                out[y * stride + x // 8] |= 0x80 >> (x % 8)
    return out


def rle(data):
    """Byte RLE. Control byte: top two bits pick the op, low six bits hold n - 1 (n = 1..64).
    00: n literal bytes follow; 01: n x 0x00; 10: n x 0xff; 11: n x the next byte.
    The firmware XORs the output into its frame buffer (art_apply in peripheral_status.c)."""
    out = bytearray()
    lit = bytearray()

    def flush():
        for i in range(0, len(lit), 64):
            chunk = lit[i : i + 64]
            out.append(len(chunk) - 1)
            out.extend(chunk)
        lit.clear()

    i = 0
    while i < len(data):
        j = i
        while j < len(data) and data[j] == data[i] and j - i < 64:
            j += 1
        n = j - i
        if n >= 3 or (n == 2 and data[i] in (0x00, 0xFF)):
            flush()
            if data[i] == 0x00:
                out.append(0x40 | (n - 1))
            elif data[i] == 0xFF:
                out.append(0x80 | (n - 1))
            else:
                out += bytes([0xC0 | (n - 1), data[i]])
            i = j
        else:
            lit.append(data[i])
            i += 1
    flush()
    return bytes(out)


def unrle_xor(blob, buf):
    """Python twin of the firmware decoder, used to check every frame round-trips."""
    i = pos = 0
    while pos < len(buf):
        op, n = blob[i] >> 6, (blob[i] & 0x3F) + 1
        i += 1
        for k in range(n):
            if op == 0:
                v = blob[i + k]
            elif op == 1:
                v = 0
            elif op == 2:
                v = 0xFF
            else:
                v = blob[i]
            buf[pos + k] ^= v
        i += n if op == 0 else (1 if op == 3 else 0)
        pos += n


def encode_set(name, frames, landscape, invert=False):
    """Returns (order, deltas): the play order as indices into the unique frames, and each
    unique frame as an RLE-packed XOR delta against the previous one (checked to round-trip)."""
    uniq, order = [], []
    for img in frames:
        data = bytes(to_lvgl_bytes(to_stored(img, landscape, invert)))
        if data not in uniq:
            uniq.append(data)
        order.append(uniq.index(data))
    if len(uniq) > 256:
        sys.exit("%s: more than 256 unique frames" % name)
    prev = bytes(len(uniq[0]))
    buf = bytearray(len(prev))
    deltas = []
    for fi, data in enumerate(uniq):
        blob = rle(bytes(a ^ b for a, b in zip(data, prev)))
        unrle_xor(blob, buf)
        assert bytes(buf) == data, "RLE round-trip failed for %s frame %d" % (name, fi)
        deltas.append(blob)
        prev = data
    return order, deltas


def write_blob(path, sets):
    """One set as a pushable blob: header, order, delta offsets, deltas (see art_relay.h)."""
    if len(sets) != 1:
        sys.exit("a .bin holds one art set; got %d (drop :stills or pass one input)" % len(sets))
    name, frames, fps, landscape, label, invert = sets[0]
    target = target_of(landscape)
    order, deltas = encode_set(name, frames, landscape, invert)
    head = b"NVA1" + struct.pack(
        "<BBBBHH", target.width, target.height, fps, FLAG_INK, len(deltas), len(order)
    )
    at = len(head) + len(order) + 2 * len(deltas)
    offsets = []
    for d in deltas:
        offsets.append(at)
        at += len(d)
    if at > 0xFFFF:
        sys.exit("%s: %d bytes is more than a blob can hold" % (name, at))
    blob = head + bytes(order) + struct.pack("<%dH" % len(offsets), *offsets) + b"".join(deltas)
    open(path, "wb").write(blob)
    raw = len(deltas) * ((target.width + 7) // 8) * target.height
    return len(deltas), raw, len(blob)


def write_variant(set_dir, sets):
    """One set's frames fitted for its --size Target, as the set's folder for that screen (#831):
    set_dir/<w>x<h>/ (upright size) with meta.txt and frame_N.png, upright, ink black on a white
    field, so the app encodes them as they are. Returns the folder."""
    if len(sets) != 1:
        sys.exit("--variant writes one art set; got %d (drop :stills or pass one input)" % len(sets))
    name, frames, fps, landscape, label, invert = sets[0]
    if not isinstance(landscape, Target):
        sys.exit("--variant needs --size")
    w, h = upright(landscape)
    folder = os.path.join(set_dir, "%dx%d" % (w, h))
    os.makedirs(folder, exist_ok=True)
    uniq, order = [], []
    for img in frames:
        data = fit(img, w, h, invert, Image.BILINEAR).tobytes()
        if data not in uniq:
            uniq.append(data)
        order.append(uniq.index(data))
    for old in os.listdir(folder):
        if re.fullmatch(r"frame_\d+\.(png|bm)", old):
            os.remove(os.path.join(folder, old))
    for i, data in enumerate(uniq):
        Image.frombytes("1", (w, h), data).convert("L").save(os.path.join(folder, "frame_%d.png" % i))
    with open(os.path.join(folder, "meta.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("Filetype: Flipper Animation\nVersion: 1\n\n")
        f.write("Width: %d\nHeight: %d\n" % (w, h))
        f.write("Passive frames: %d\nActive frames: 0\n" % len(order))
        f.write("Frames order: %s\n" % " ".join(map(str, order)))
        f.write("Active cycles: 0\nFrame rate: %d\n" % fps)
    return folder


def write_c(path, sets):
    """Each set stores its unique frames as XOR deltas against the previous unique frame (the
    first against an empty buffer), RLE-packed, plus the play order as indices into them.
    Applying delta k toggles the buffer between unique frames k - 1 and k, so the firmware can
    step forwards or backwards through a set with one small static buffer."""
    lines = [
        "/* Generated by scripts/flipper_art.py - do not edit. */",
        "",
        "#include <stdint.h>",
        '#include "custom_art.h"',
        "",
    ]
    stride = (ART_W + 7) // 8
    raw_bytes = packed_bytes = unique = 0
    set_lines = []
    manifest_sets = []
    for si, (name, frames, fps, landscape, label, invert) in enumerate(sets):
        order, deltas = encode_set(name, frames, landscape, invert)
        uniq = deltas
        for fi, blob in enumerate(deltas):
            lines.append("static const uint8_t set_%d_delta_%d[] = {" % (si, fi))
            for i in range(0, len(blob), 16):
                lines.append("    " + ", ".join("0x%02x" % b for b in blob[i : i + 16]) + ",")
            lines.append("};")
            raw_bytes += stride * ART_H
            packed_bytes += len(blob)
        unique += len(uniq)
        lines.append(
            "static const uint8_t *const set_%d_deltas[] = {%s};"
            % (si, ", ".join("set_%d_delta_%d" % (si, i) for i in range(len(uniq))))
        )
        lines.append(
            "static const uint8_t set_%d_order[] = {%s};" % (si, ", ".join(map(str, order)))
        )
        lines.append("")
        set_lines.append(
            '    {.name = "%s", .deltas = set_%d_deltas, .order = set_%d_order, '
            ".frame_count = %d, .fps = %d},"
            % (name, si, si, len(order), fps)
        )
        # pointer table + order table
        packed_bytes += 4 * len(uniq) + len(order)
        manifest_sets.append(
            {"index": si, "name": name, "label": label, "frames": len(order), "fps": fps}
        )
    lines += ["const struct custom_art_set custom_art_sets[] = {"] + set_lines + ["};"]
    lines.append("const int custom_art_set_count = %d;" % len(sets))
    open(path, "w", newline="\n").write("\n".join(lines) + "\n")
    manifest = {
        "format": "foxglove-builtin-art",
        "version": 1,
        "set_count": len(sets),
        "sets": manifest_sets,
    }
    with open(os.path.splitext(path)[0] + ".json", "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    assert raw_bytes == unique * stride * ART_H
    return unique, raw_bytes, packed_bytes


def write_preview(path, sets, scale=3, per_row=6):
    """First frame of each set as seen on the keyboard: portrait, status strip on top, in the
    dark theme (ink light on a dark field). Sets made with --size show their first frame
    upright, without the Corne's status strip."""
    if any(isinstance(s[3], Target) for s in sets):
        return write_sized_preview(path, sets, scale, per_row)
    w, h, gap = ART_H, SCREEN_W, 8
    rows = (len(sets) + per_row - 1) // per_row
    cols = min(len(sets), per_row)
    sheet = Image.new("L", ((w * scale + gap) * cols, (h * scale + gap) * rows), 160)
    for i, (_, frames, _, landscape, _, invert) in enumerate(sets):
        screen = Image.new("1", (SCREEN_W, ART_H), 1)
        screen.paste(to_stored(frames[0], landscape, invert), (0, 0))
        for x in range(ART_W, SCREEN_W):  # stand-in for the battery/connection strip
            for y in range(ART_H):
                if (x + y) % 4 == 0:
                    screen.putpixel((x, y), 0)
        upright = screen.rotate(90, expand=True).resize((w * scale, h * scale), Image.NEAREST)
        upright = upright.convert("L").point(lambda v: 255 - v)  # dark theme: ink lit
        sheet.paste(upright.convert("L"), ((i % per_row) * (w * scale + gap), (i // per_row) * (h * scale + gap)))
    sheet.save(path)


def write_sized_preview(path, sets, scale, per_row, gap=8):
    """First frame of each --size set, upright, in the dark theme."""
    w, h = upright(target_of(sets[0][3]))
    rows = (len(sets) + per_row - 1) // per_row
    cols = min(len(sets), per_row)
    sheet = Image.new("L", ((w * scale + gap) * cols, (h * scale + gap) * rows), 160)
    for i, (_, frames, _, _, _, invert) in enumerate(sets):
        up = fit(frames[0], w, h, invert, Image.BILINEAR).resize((w * scale, h * scale), Image.NEAREST)
        up = up.convert("L").point(lambda v: 255 - v)  # dark theme: ink lit
        sheet.paste(up, ((i % per_row) * (w * scale + gap), (i // per_row) * (h * scale + gap)))
    sheet.save(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("out")
    ap.add_argument("inputs", nargs="+")
    ap.add_argument("--preview")
    ap.add_argument("--size", type=parse_size, help="upright WxH[:ROT] of another art screen")
    ap.add_argument("--variant", metavar="SET", help="with --size: write SET/<W>x<H>/")
    args = ap.parse_args()

    is_bin = args.out.lower().endswith(".bin")
    if args.size is not None and not is_bin:
        ap.error("--size makes a blob: OUT is a .bin (the firmware's built-ins stay 140x68)")
    if args.variant and args.size is None:
        ap.error("--variant needs --size")

    sets = [s for p in args.inputs for s in load_input(p, args.size)]
    write = write_blob if is_bin else write_c
    unique, raw, packed = write(args.out, sets)
    if args.variant:
        print("variant folder: %s" % write_variant(args.variant, sets))
    total = sum(len(s[1]) for s in sets)
    print("%d art sets, %d frames (%d unique)" % (len(sets), total, unique))
    print(
        "flash: %.1f KiB compressed vs %.1f KiB raw (%.0f%%)"
        % (packed / 1024, raw / 1024, 100.0 * packed / raw)
    )
    for name, frames, fps, landscape, label, invert in sets:
        print(
            "  %-32s %3d frame(s)%s%s%s"
            % (
                name,
                len(frames),
                " @ %d fps" % fps if fps else "",
                " (%dx%d)" % upright(landscape) if isinstance(landscape, Target)
                else " (landscape)" if landscape else "",
                " (inverted: drawn light on dark)" if invert else "",
            )
        )
    if args.preview:
        write_preview(args.preview, sets)


if __name__ == "__main__":
    main()
