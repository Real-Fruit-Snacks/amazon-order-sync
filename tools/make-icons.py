#!/usr/bin/env python3
"""Render the Amazon Order Sync icons in the RFS style (dark tile, "aos" in
Plex Mono 600, Ember rule), into every place that ships one:

    server/icons/icon-192.png, icon-512.png, apple-touch-icon.png, favicon.ico
    browser-extension/icon-16.png, icon-48.png, icon-128.png
    tray/app.ico

Uses the shared text outlining and colours from Real-Fruit-Snacks/rfs-brand, so
clone it next to this repo first:

    python3 tools/make-icons.py [--brand ../rfs-brand]

Requires Python 3.11+, fonttools, resvg-py and pillow.
"""
import argparse, importlib.util, io, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def load_brand(path):
    p = os.path.join(path, "tools", "make-cover.py")
    if not os.path.exists(p):
        sys.exit(f"can't find {p}: clone Real-Fruit-Snacks/rfs-brand next to this repo or pass --brand")
    spec = importlib.util.spec_from_file_location("make_cover", p)
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    return m


def svg(mc, word, n=512):
    """Square tile, rounded 14/64 like the Caps Tile; the word and its rule
    are centred as one block."""
    c = mc.THEMES["dark"]
    size = n * (0.36 if len(word) > 1 else 0.62)
    ls = -0.05 * size
    il, ir = mc.ink_extent(word, size, ls)
    xh = 0.516 * size                      # Plex Mono x-height
    rule_h, gap = n * 0.055, n * 0.11
    top = (n - (xh + gap + rule_h)) / 2
    base = top + xh
    x = n / 2 - (il + ir) / 2
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" width="{n}" height="{n}">'
            f'<rect width="{n}" height="{n}" rx="{n * 14 / 64:.1f}" fill="{c["raised"]}"/>'
            f'<path d="{mc.text_path("mono-semibold", word, x, base, size, ls)}" fill="{c["text"]}"/>'
            f'<rect x="{x + il:.1f}" y="{base + gap:.1f}" width="{ir - il:.1f}" height="{rule_h:.1f}" '
            f'rx="{rule_h / 2:.1f}" fill="{c["ember"]}"/></svg>')


def png(mc, word, px):
    import resvg_py
    from PIL import Image
    s = svg(mc, word).replace('width="512" height="512"', f'width="{px * 4}" height="{px * 4}"', 1)
    im = Image.open(io.BytesIO(bytes(resvg_py.svg_to_bytes(svg_string=s, skip_system_fonts=True))))
    return im.convert("RGBA").resize((px, px), Image.LANCZOS)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--brand", default=os.path.join(ROOT, "..", "rfs-brand"))
    a = ap.parse_args()
    mc = load_brand(a.brand)
    # "aos" reads from 48px up; below that the tile carries just the "a".
    def icon(px):
        return png(mc, "aos" if px >= 48 else "a", px)
    out = []
    def save(im, rel, **kw):
        p = os.path.join(ROOT, rel); os.makedirs(os.path.dirname(p), exist_ok=True)
        im.save(p, **kw); out.append(rel)
    save(icon(192), "server/icons/icon-192.png", optimize=True)
    save(icon(512), "server/icons/icon-512.png", optimize=True)
    save(icon(180), "server/icons/apple-touch-icon.png", optimize=True)
    for px in (16, 48, 128):
        save(icon(px), f"browser-extension/icon-{px}.png", optimize=True)
    # .ico files: Pillow writes every listed size from the largest frame, so
    # build each size separately and pass them as appended frames.
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
