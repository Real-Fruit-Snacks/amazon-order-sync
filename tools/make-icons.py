#!/usr/bin/env python3
"""Render the Amazon Order Sync icons in Cyclorama Dawn v2.1.0 (matte #050507 tile,
rising Glacial Steel -> Maritime Beacon Orange cyclorama wash, Big Shoulders Stencil
ExtraBold "AOS" monogram, and Steel -> Beacon horizon bar) into every place that ships one:

    server/icons/icon.svg, icon-192.png, icon-512.png, apple-touch-icon.png, favicon.ico
    browser-extension/icon-16.png, icon-48.png, icon-128.png
    tray/app.ico

Uses server/fonts/big-shoulders-stencil-latin.woff2 directly (zero external repo required).
Requires Python 3.11+, fonttools, brotli, resvg-py and pillow.
"""
import argparse, io, os, sys

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)

BG = "#050507"
TEXT = "#F4F5F8"


def load_display(font_path):
    if not os.path.exists(font_path):
        sys.exit(f"can't find {font_path}")
    return instantiateVariableFont(TTFont(font_path), {"wght": 800})


def measure_word(tt, word, size, ls):
    """Exact glyph ink bounding box (x0, y0, x1, y1) in SVG coordinates with baseline at y=0."""
    gs = tt.getGlyphSet()
    cmap = tt.getBestCmap()
    upm = tt["head"].unitsPerEm
    k = size / upm
    pen_x = 0.0
    x0, y0, x1, y1 = 1e9, 1e9, -1e9, -1e9
    for idx, ch in enumerate(word):
        gname = cmap[ord(ch)]
        bp = BoundsPen(gs)
        gs[gname].draw(bp)
        if bp.bounds:
            gx0, gy0, gx1, gy1 = bp.bounds
            x0 = min(x0, pen_x + gx0 * k)
            x1 = max(x1, pen_x + gx1 * k)
            y0 = min(y0, -gy1 * k)
            y1 = max(y1, -gy0 * k)
        pen_x += gs[gname].width * k + (ls if idx + 1 < len(word) else 0.0)
    return x0, y0, x1, y1


def word_paths(tt, word, x, y, size, ls):
    gs = tt.getGlyphSet()
    cmap = tt.getBestCmap()
    upm = tt["head"].unitsPerEm
    k = size / upm
    pen_x = x
    parts = []
    for idx, ch in enumerate(word):
        gname = cmap[ord(ch)]
        p = SVGPathPen(gs, ntos=lambda v: f"{v:.2f}")
        gs[gname].draw(p)
        d = p.getCommands()
        if d:
            parts.append(
                f'<path d="{d}" transform="translate({pen_x:.2f} {y:.2f}) scale({k:.5f} {-k:.5f})" fill="{TEXT}"/>'
            )
        pen_x += gs[gname].width * k + (ls if idx + 1 < len(word) else 0.0)
    return "".join(parts)


def tile_svg(tt, word="AOS", rx=14, small=False, pad_scale=1.0):
    """64-unit Cyclorama Dawn AOS Monogram Tile matching rfs-brand/tools/make-marks.py."""
    defs = (
        '<defs>'
        '<linearGradient id="cyc-h" x1="0%" y1="0%" x2="100%" y2="0%">'
        '<stop offset="0%" stop-color="#133954"/>'
        '<stop offset="44%" stop-color="#1F547A"/>'
        '<stop offset="76%" stop-color="#CC5016"/>'
        '<stop offset="100%" stop-color="#F06E2B"/>'
        '</linearGradient>'
        '<linearGradient id="cyc-f" x1="0%" y1="0%" x2="0%" y2="100%">'
        '<stop offset="0%" stop-color="#FFFFFF" stop-opacity="0"/>'
        '<stop offset="42%" stop-color="#FFFFFF" stop-opacity="0.08"/>'
        '<stop offset="74%" stop-color="#FFFFFF" stop-opacity="0.58"/>'
        '<stop offset="100%" stop-color="#FFFFFF" stop-opacity="1"/>'
        '</linearGradient>'
        '<mask id="cyc-m" maskContentUnits="objectBoundingBox">'
        '<rect width="1" height="1" fill="url(#cyc-f)"/>'
        '</mask>'
        '<linearGradient id="rule-d" x1="0" y1="0" x2="1" y2="0">'
        '<stop offset="0%" stop-color="#74B8E8"/>'
        '<stop offset="52%" stop-color="#3B85B8"/>'
        '<stop offset="100%" stop-color="#F06E2B"/>'
        '</linearGradient>'
        '</defs>'
    )

    if len(word) == 1:
        draw_sz = 38.0 * pad_scale
        draw_ls = 0.0
    else:
        draw_sz = (33.5 if small else 30.5) * pad_scale
        draw_ls = draw_sz * (0.035 if small else 0.045)

    bx0, by0, bx1, by1 = measure_word(tt, word, draw_sz, draw_ls)
    ink_w = bx1 - bx0
    ink_h = by1 - by0
    gap = (4.2 if small else 5.0) * pad_scale
    bar_h = (4.4 if small else 3.8) * pad_scale
    bar_w = max(ink_w, 34.0 * pad_scale) if len(word) == 1 else ink_w
    group_h = ink_h + gap + bar_h
    group_top = (64.0 - group_h) / 2.0
    cx = 32.0 - (bx0 + bx1) / 2.0
    cy = group_top - by0
    bar_y = group_top + ink_h + gap

    wash_op = "0.86" if small else "0.74"
    frame_stroke = "#3B3E4A" if small else "rgba(255,255,255,0.16)"
    frame = (
        f'<rect x="0.75" y="0.75" width="62.5" height="62.5" rx="{max(0, rx - 0.75):g}" '
        f'fill="none" stroke="{frame_stroke}" stroke-width="1.5"/>'
        if rx > 0 else ""
    )
    bg_el = (
        f'<rect width="64" height="64" rx="{rx}" fill="{BG}"/>'
        f'<rect width="64" height="64" rx="{rx}" fill="url(#cyc-h)" opacity="{wash_op}" mask="url(#cyc-m)"/>'
        f'{frame}'
    )
    glyph = word_paths(tt, word, cx, cy, draw_sz, draw_ls)
    bar = (
        f'<rect x="{32.0 - bar_w / 2.0:.2f}" y="{bar_y:.2f}" width="{bar_w:.2f}" '
        f'height="{bar_h:.2f}" rx="{bar_h / 2.0:.2f}" fill="url(#rule-d)"/>'
    )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="64" height="64" '
        f'role="img" aria-label="Amazon Order Sync">{defs}{bg_el}{glyph}{bar}</svg>\n'
    )


def render_png(svg_str, px):
    import resvg_py
    from PIL import Image
    raw = bytes(resvg_py.svg_to_bytes(svg_string=svg_str, width=px * 4, height=px * 4, skip_system_fonts=True))
    return Image.open(io.BytesIO(raw)).convert("RGBA").resize((px, px), Image.LANCZOS)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument(
        "--font",
        default=os.path.join(ROOT, "server", "fonts", "big-shoulders-stencil-latin.woff2"),
    )
    a = ap.parse_args()
    tt = load_display(a.font)

    svg_large = tile_svg(tt, "AOS", rx=14, small=False)
    svg_small = tile_svg(tt, "AOS", rx=14, small=True)
    svg_tiny = tile_svg(tt, "A", rx=14, small=True)
    svg_apple = tile_svg(tt, "AOS", rx=0, small=False, pad_scale=0.88)

    def icon_for(px):
        if px <= 24:
            return render_png(svg_tiny, px)
        if px <= 48:
            return render_png(svg_small, px)
        return render_png(svg_large, px)

    out = []

    def save_text(text, rel):
        p = os.path.join(ROOT, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        out.append(rel)

    def save_img(im, rel, **kw):
        p = os.path.join(ROOT, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        im.save(p, **kw)
        out.append(rel)

    save_text(svg_small, "server/icons/icon.svg")
    save_img(icon_for(192), "server/icons/icon-192.png", optimize=True)
    save_img(icon_for(512), "server/icons/icon-512.png", optimize=True)
    save_img(render_png(svg_apple, 180), "server/icons/apple-touch-icon.png", optimize=True)
    for px in (16, 48, 128):
        save_img(icon_for(px), f"browser-extension/icon-{px}.png", optimize=True)

    def save_ico(rel, sizes):
        frames = [icon_for(s) for s in sizes]
        p = os.path.join(ROOT, rel)
        frames[-1].save(p, format="ICO", sizes=[(s, s) for s in sizes], append_images=frames[:-1])
        out.append(rel)

    save_ico("server/icons/favicon.ico", [16, 32, 48])
    save_ico("tray/app.ico", [16, 20, 24, 32, 40, 48, 64, 256])
    for r in out:
        print("wrote", r)


if __name__ == "__main__":
    main()
