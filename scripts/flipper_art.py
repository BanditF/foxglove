"""Convert Flipper Zero art into nice!view art for the nice_view_custom shield.

Accepts, in any mix:
  * a Flipper animation folder (meta.txt + frame_N.bm or frame_N.png), as found in the
    official firmware (assets/dolphin/external/*) and community packs
  * a single image file (.png/.gif/.jpg/.bmp)
  * a single compiled Flipper bitmap named like name_128x64.bm

Each input becomes one "art set" on the right-hand screen. Animations keep all their frames
(in meta.txt order) and frame rate; the firmware shows the first frame unless animation is
enabled with CONFIG_NICE_VIEW_CUSTOM_ANIMATE. Frames are stored as RLE-packed XOR deltas and
unpacked into one RAM image buffer on the keyboard.

Usage:
  python scripts/flipper_art.py OUT.c INPUT [INPUT ...] [--preview preview.png]

Requires: pip install pillow heatshrink2
"""

import argparse
import os
import re
import sys

from PIL import Image

ART_W, ART_H = 140, 68  # area left of the status strip on the right-hand nice!view


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


def load_input(path):
    """Returns (name, [PIL images], fps)."""
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
    return name, [Image.open(path)], 0


def fit(img):
    """Fit into ART_W x ART_H (never upscales Flipper-sized art), centre, 1-bit."""
    img = img.convert("L")
    if img.width > ART_W or img.height > ART_H:
        scale = min(ART_W / img.width, ART_H / img.height)
        img = img.resize((max(1, int(img.width * scale)), max(1, int(img.height * scale))))
    canvas = Image.new("L", (ART_W, ART_H), 255)
    canvas.paste(img, ((ART_W - img.width) // 2, (ART_H - img.height) // 2))
    return canvas.point(lambda v: 255 if v >= 128 else 0, "1")


def to_lvgl_bytes(img):
    """LVGL INDEXED_1BIT rows, MSB first. Ink -> index 0 (foreground in the palette below)."""
    stride = (ART_W + 7) // 8
    out = bytearray(stride * ART_H)
    px = img.load()
    for y in range(ART_H):
        for x in range(ART_W):
            if px[x, y]:  # background -> bit set
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
    for si, (name, frames, fps) in enumerate(sets):
        uniq, order = [], []
        for img in frames:
            data = bytes(to_lvgl_bytes(fit(img)))
            if data not in uniq:
                uniq.append(data)
            order.append(uniq.index(data))
        if len(uniq) > 256:
            sys.exit("%s: more than 256 unique frames" % name)
        prev = bytes(len(uniq[0]))
        buf = bytearray(len(prev))
        for fi, data in enumerate(uniq):
            blob = rle(bytes(a ^ b for a, b in zip(data, prev)))
            unrle_xor(blob, buf)
            assert bytes(buf) == data, "RLE round-trip failed for %s frame %d" % (name, fi)
            lines.append("static const uint8_t set_%d_delta_%d[] = {" % (si, fi))
            for i in range(0, len(blob), 16):
                lines.append("    " + ", ".join("0x%02x" % b for b in blob[i : i + 16]) + ",")
            lines.append("};")
            raw_bytes += len(data)
            packed_bytes += len(blob)
            prev = data
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
    lines += ["const struct custom_art_set custom_art_sets[] = {"] + set_lines + ["};"]
    lines.append("const int custom_art_set_count = %d;" % len(sets))
    open(path, "w", newline="\n").write("\n".join(lines) + "\n")
    assert raw_bytes == unique * stride * ART_H
    return unique, raw_bytes, packed_bytes


def write_preview(path, sets):
    s, w, h = 3, 160, 68
    sheet = Image.new("1", (w * s, (h * s + 8) * len(sets)), 1)
    for i, (_, frames, _) in enumerate(sets):
        screen = Image.new("1", (w, h), 1)
        screen.paste(fit(frames[0]), (0, 0))
        sheet.paste(screen.resize((w * s, h * s), Image.NEAREST), (0, i * (h * s + 8)))
    sheet.save(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("out")
    ap.add_argument("inputs", nargs="+")
    ap.add_argument("--preview")
    args = ap.parse_args()

    sets = [load_input(p) for p in args.inputs]
    unique, raw, packed = write_c(args.out, sets)
    total = sum(len(f) for _, f, _ in sets)
    print("%d art sets, %d frames (%d unique)" % (len(sets), total, unique))
    print(
        "flash: %.1f KiB compressed vs %.1f KiB raw (%.0f%%)"
        % (packed / 1024, raw / 1024, 100.0 * packed / raw)
    )
    for name, frames, fps in sets:
        print("  %-32s %3d frame(s)%s" % (name, len(frames), " @ %d fps" % fps if fps else ""))
    if args.preview:
        write_preview(args.preview, sets)


if __name__ == "__main__":
    main()
