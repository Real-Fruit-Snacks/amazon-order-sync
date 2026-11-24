#!/usr/bin/env python3
"""Render the Amazon Order Sync icons in Cyclorama Dawn v2.1.0 (matte dark tile,
"aos" in Azeret Mono 600, Glacial Steel -> Maritime Beacon Orange horizon rule)
into every place that ships one:

    server/icons/icon-192.png, icon-512.png, apple-touch-icon.png, favicon.ico
    browser-extension/icon-16.png, icon-48.png, icon-128.png
    tray/app.ico

Uses docs/fonts/azeret-mono-latin.woff2 directly (zero external repo required).
Requires Python 3.11+, fonttools, brotli, resvg-py and pillow.
"""
import argparse, io, os, sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

BG = "#0D0E12"
LINE = "#242834"
TEXT = "#F2F4F8"
STEEL = "#1F547A"
BEACON = "#F06E2B"


def load_mono(font_path):
    if not os.path.exists(font_path):
        sys.exit(f"can't find {font_path}")
    return instantiateVariableFont(TTFont(font_path), {"wght": 600})


def text_path(tt, word, x, y, size, ls):
    gs = tt.getGlyphSet()
    cmap = tt.getBestCmap()
    upm = tt["head"].unitsPerEm
    k = size / upm
    pen_x = x
    parts = []
    min_x, max_x = 1e9, -1e9
    for idx, ch in enumerate(word):
        gname = cmap[ord(ch)]
        p = SVGPathPen(gs, ntos=lambda v: f"{v:.1f}")
        gs[gname].draw(p)
        d = p.getCommands()
        if d:
            parts.append(
                f'<path d="{d}" transform="translate({pen_x:.2f} {y:.2f}) scale({k:.5f} {-k:.5f})" fill="{TEXT}"/>'
            )
            min_x = min(min_x, pen_x)
            max_x = max(max_x, pen_x + gs[gname].width * k)
        pen_x += gs[gname].width * k + (ls if idx + 1 < len(word) else 0.0)
    return "".join(parts), pen_x - x, (min_x - x if min_x < 1e8 else 0.0), (max_x - x if max_x > -1e8 else pen_x - x)


def svg(tt, word, n=512):
    size = n * (0.34 if len(word) > 1 else 0.58)
    ls = -0.04 * size
    _, adv, il, ir = text_path(tt, word, 0.0, 0.0, size, ls)
    xh = 0.53 * size
    rule_h, gap = n * 0.06, n * 0.115
    top = (n - (xh + gap + rule_h)) / 2
    base = top + xh
    x = n / 2 - adv / 2
    paths, _, _, _ = text_path(tt, word, x, base, size, ls)
    rx = n * 14 / 64
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}">'
        f'<defs><linearGradient id="hz" x1="0" y1="0" x2="1" y2="0">'
        f'<stop offset="0%" stop-color="{STEEL}"/><stop offset="100%" stop-color="{BEACON}"/>'
        f'</linearGradient></defs>'
        f'<rect x="4" y="4" width="{n - 8}" height="{n - 8}" rx="{rx:.1f}" fill="{BG}" stroke="{LINE}" stroke-width="8"/>'
        f'{paths}'
        f'<rect x="{x + il:.1f}" y="{base + gap:.1f}" width="{max(ir - il, adv):.1f}" height="{rule_h:.1f}" '
        f'rx="{rule_h / 2:.1f}" fill="url(#hz)"/>'
        f'</svg>'
    )


def png(tt, word, px):
    import resvg_py
    from PIL import Image
    s = svg(tt, word).replace('width="512" height="512"', f'width="{px * 4}" height="{px * 4}"', 1)
    im = Image.open(io.BytesIO(bytes(resvg_py.svg_to_bytes(svg_string=s, skip_system_fonts=True))))
    return im.convert("RGBA").resize((px, px), Image.LANCZOS)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--font", default=os.path.join(ROOT, "docs", "fonts", "azeret-mono-latin.woff2"))
    a = ap.parse_args()
    tt = load_mono(a.font)

    def icon(px):
        return png(tt, "aos" if px >= 48 else "a", px)

    out = []

    def save(im, rel, **kw):
        p = os.path.join(ROOT, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        im.save(p, **kw)
        out.append(rel)

    save(icon(192), "server/icons/icon-192.png", optimize=True)
    save(icon(512), "server/icons/icon-512.png", optimize=True)
    save(icon(180), "server/icons/apple-touch-icon.png", optimize=True)
    for px in (16, 48, 128):
        save(icon(px), f"browser-extension/icon-{px}.png", optimize=True)

    def ico(rel, sizes):
        frames = [icon(s) for s in sizes]
        p = os.path.join(ROOT, rel)
        frames[-1].save(p, format="ICO", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
        out.append(rel)

    ico("server/icons/favicon.ico", [16, 32, 48])
    ico("tray/app.ico", [16, 20, 24, 32, 40, 48, 64, 256])
    for r in out:
        print("wrote", r)


if __name__ == "__main__":
    main()
