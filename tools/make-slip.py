"""Draw the README picture: docs/assets/slip-dark.svg and slip-light.svg.

A packing slip for a made-up order sits next to the note the server wrote for it. In a 14 s loop a
scan runs down the next order's slip, each field lights as the scan reaches it, the frontmatter lines
that came from that field print, and the folder it was filed in appears above. Then the first order
again. The SVG's base styles are the first frame (the October order, complete), which is what a
viewer that skips the animation shows.

The slips come from tools/samples.json and the notes from docs/samples/, which
tools/make-samples.mjs writes by running the real server, so run that first when either changes.
Text is converted to paths from the page's fonts (docs/fonts), because an SVG shown through <img>
can't load fonts. Run from the repo root:

    python -m pip install -r tools/requirements-art.txt
    python tools/make-slip.py
"""
import json
import os
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FONTS = os.path.join(ROOT, "docs", "fonts")
OUT = os.path.join(ROOT, "docs", "assets")

# RFS Nightshift and Dayshift. The slip is paper in both: light on the dark desk at night.
THEMES = {
    "dark": dict(bg="#0C0D10", line="#262A33", file="#070809", tab="#1A1D24", ink="#E8E6E1", mute="#8A8F99",
                 key="#FFC247", yes="#3DDC97", no="#F06292", ember="#FF6A3D",
                 paper="#E8E6E1", pink="#0C0D10", pmint="#15794D", beam="#FF6A3D", hl="#FFC247"),
    "light": dict(bg="#F6F5F2", line="#D6D3CB", file="#FFFFFF", tab="#ECEAE4", ink="#1A1B1E", mute="#666A73",
                  key="#925F00", yes="#15794D", no="#B0306A", ember="#C03B0A",
                  paper="#FFFFFF", pink="#1A1B1E", pmint="#15794D", beam="#C03B0A", hl="#FFC247"),
}

W, H = 1000, 580
CYCLE = 14.0
SHOW = [0, 1]                 # orders from samples.json: the loop shows these two
SWITCH = [(2.5, 1), (9.0, 0)]  # (time, order index): a new slip arrives and is scanned
SCAN_DELAY, SCAN, LINE_GAP, SETTLE = 0.2, 1.9, 0.065, 0.45

# Which slip field each frontmatter key comes from (as on the page).
FIELD = {"type": "order_id", "order_id": "order_id", "date": "date", "total": "total", "status": "status", "ship_to": "ship_to"}

CODE39 = {"0": "000110100", "1": "100100001", "2": "001100001", "3": "101100000", "4": "000110001",
          "5": "100110000", "6": "001110000", "7": "000100101", "8": "100100100", "9": "001100100",
          "-": "010000101", "*": "010010100"}

_fonts, _glyphs = {}, {}


def font(file, **axes):
    key = (file, tuple(sorted(axes.items())))
    if key not in _fonts:
        _fonts[key] = instantiateVariableFont(TTFont(os.path.join(FONTS, file)), axes)
    return key


ARCHIVO, MARTIAN = "archivo-latin-var.woff2", "martian-mono-latin-var.woff2"
F = dict(
    route=font(ARCHIVO, wdth=62, wght=900),
    big=font(ARCHIVO, wdth=62, wght=800),
    label=font(ARCHIVO, wdth=75, wght=700),
    value=font(ARCHIVO, wdth=87.5, wght=650),
    mono=font(MARTIAN, wdth=75, wght=400),
    monob=font(MARTIAN, wdth=75, wght=600),
)


def advance(s, size, fkey, ls=0.0):
    f = _fonts[fkey]
    cmap, hmtx, upm = f.getBestCmap(), f["hmtx"], f["head"].unitsPerEm
    return sum(hmtx[cmap[ord(c)]][0] for c in s) * size / upm + ls * size * (len(s) - 1)


def text(s, x, y, size, fkey, fill, ls=0.0, anchor="start"):
    """Paths for s with its baseline at y; ls is letter spacing in em."""
    f = _fonts[fkey]
    gs, cmap, upm = f.getGlyphSet(), f.getBestCmap(), f["head"].unitsPerEm
    if anchor != "start":
        x -= advance(s, size, fkey, ls) * (1 if anchor == "end" else 0.5)
    k, pen, uses = size / upm, 0.0, []
    for ch in s:
        if ord(ch) not in cmap:
            sys.exit(f"no glyph for {ch!r}")
        name = cmap[ord(ch)]
        gkey = (fkey, name)
        if gkey not in _glyphs:
            p = SVGPathPen(gs, ntos=lambda v: "%d" % round(v))
            gs[name].draw(p)
            _glyphs[gkey] = (f"g{len(_glyphs)}", p.getCommands())
        gid, d = _glyphs[gkey]
        if d:
            uses.append(f'<use href="#{gid}" x="{round(pen)}"/>')
        pen += gs[name].width + ls * upm
    return f'<g fill="{fill}" transform="translate({x:.1f} {y:.1f}) scale({k:.5f} {-k:.5f})">{"".join(uses)}</g>'


def barcode(s, x, y, w, h, fill):
    rects, pos = [], 0
    for ch in f"*{s}*":
        for i, bit in enumerate(CODE39[ch]):
            wd = 2 if bit == "1" else 1
            if i % 2 == 0:
                rects.append((pos, wd))
            pos += wd
        pos += 1
    total = pos - 1
    d = "".join(f"M{a} 0h{b}v1h-{b}z" for a, b in rects)
    return f'<path fill="{fill}" transform="translate({x} {y}) scale({w / total:.5f} {h})" d="{d}"/>'


def pct(t):
    return f"{t / CYCLE * 100:.3f}%"


def toggle(name, windows):
    """step-end keyframes that show an element only inside the (start, end) windows."""
    shown = lambda t: int(any(a <= t < b for a, b in windows))
    points = sorted({0.0} | {t for w in windows for t in w if 0 < t < CYCLE})
    return f"@keyframes {name}{{" + "".join(f"{pct(t)}{{opacity:{shown(t)}}}" for t in points) + f"100%{{opacity:{shown(0.0)}}}}}"


def flash(name, times, peak=1.0, fade=0.6):
    """Linear keyframes: jump to `peak` at each time, fade out over `fade` seconds."""
    ks = ["0%{opacity:0}"]
    for t in times:
        ks += [f"{pct(t - 0.001)}{{opacity:0}}", f"{pct(t)}{{opacity:{peak}}}", f"{pct(t + fade)}{{opacity:0}}"]
    return f"@keyframes {name}{{" + "".join(ks) + "100%{opacity:0}}"


def note_lines(md):
    """The note's frontmatter and its first heading, as (field, [(token, kind)])."""
    out, fence = [], 0
    for line in md.split("\n"):
        if line == "---":
            fence += 1
            out.append(("order_id" if fence == 1 else "items", [(line, "rule")]))
            if fence == 2:
                continue
        elif fence == 1:
            key, _, val = line.partition(": ")
            toks, i = [(key, "key"), (": ", "ink")], 0
            while i < len(val):
                c = val[i]
                if c == '"':
                    j = i + 1
                    while j < len(val) and val[j] != '"':
                        j += 2 if val[j] == "\\" else 1
                    toks.append((val[i:j + 1], "ink")); i = j + 1
                elif c in "[],":
                    toks.append((c, "mute")); i += 1
                elif val.startswith("true", i):
                    toks.append(("true", "yes")); i += 4
                elif val.startswith("false", i):
                    toks.append(("false", "no")); i += 5
                else:
                    toks.append((c, "ink")); i += 1
            out.append((FIELD.get(key, "items"), toks))
        elif fence >= 2 and line.startswith("# "):
            out.append(("body", [(line, "head")]))
            break
    return out


def build(theme, orders, notes):
    c = THEMES[theme]
    o = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" role="img">']
    css, base = [], []

    sx, sy, sw = 40, 108, 340          # slip
    fx, fy, fw = 404, 108, 556         # file
    fh = H - 36 - fy
    tab = 38

    def field_layout(order):
        rows = [("order_id", 0, 98), ("row", 98, 58), ("status", 156, 58), ("ship_to", 214, 56)]
        items_h = 34 + 66 * len(order["items"])
        rows.append(("items", 270, max(items_h, fh - 270)))
        return rows

    # --- the desk, the file window ---
    o.append(f'<rect x="0.5" y="0.5" width="{W - 1}" height="{H - 1}" rx="18" fill="{c["bg"]}" stroke="{c["line"]}"/>')
    o.append(text("AMAZON-ORDER-SYNC", W - 40, 50, 13, F["label"], c["mute"], ls=0.14, anchor="end"))
    o.append(text("MADE-UP ORDERS, REAL OUTPUT", W - 40, 72, 13, F["label"], c["mute"], ls=0.14, anchor="end"))
    o.append(f'<defs><clipPath id="win"><rect x="{fx}" y="{fy}" width="{fw}" height="{fh}" rx="9"/></clipPath>'
             f'<linearGradient id="fade" x1="0" x2="1"><stop offset="0" stop-color="{c["file"]}" stop-opacity="0"/><stop offset="1" stop-color="{c["file"]}"/></linearGradient>'
             f'<linearGradient id="fadev" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="{c["file"]}" stop-opacity="0"/><stop offset="1" stop-color="{c["file"]}"/></linearGradient></defs>')
    o.append(f'<rect x="{fx + .5}" y="{fy + .5}" width="{fw - 1}" height="{fh - 1}" rx="9" fill="{c["file"]}" stroke="{c["line"]}"/>')
    o.append(f'<g clip-path="url(#win)"><rect x="{fx}" y="{fy}" width="{fw}" height="{tab}" fill="{c["tab"]}"/></g>')
    o.append(f'<rect x="{fx}" y="{fy + tab}" width="{fw}" height="1" fill="{c["line"]}"/>')

    # --- timing ---
    scans = {}       # order index -> list of scan start times
    for t, i in SWITCH:
        scans.setdefault(i, []).append(t + SCAN_DELAY)
    shown_slip = {i: [] for i in SHOW}
    marks = [0.0] + [t for t, _ in SWITCH] + [CYCLE]
    owner = [0] + [i for _, i in SWITCH]
    for k in range(len(owner)):
        shown_slip[owner[k]].append((marks[k], marks[k + 1]))

    for i in SHOW:
        order = orders[i]
        rel = next(k for k in notes if k.endswith(f"/Order-{order['order_id']}.md"))
        folder, fname = rel.rsplit("/", 1)
        lines = note_lines(notes[rel])
        layout = field_layout(order)
        slip_h = fh
        win = shown_slip[i]
        cls = f"o{i}"

        # When does each field get scanned, and each line print?
        field_t = {}
        for name, top, _ in layout:
            for t0 in scans.get(i, []):
                names = ["date", "total"] if name == "row" else [name]
                for nm in names:
                    field_t.setdefault(nm, []).append(t0 + SCAN * (top + 12) / slip_h)
        line_t, count = [], {}
        for f_, _ in lines:
            ts = []
            for n, t0 in enumerate(field_t.get(f_, [])) if f_ != "body" else enumerate(scans.get(i, [])):
                if f_ == "body":
                    ts.append(t0 + SCAN + 0.15)
                else:
                    ts.append(t0 + LINE_GAP * count.get((f_, n), 0))
                    count[(f_, n)] = count.get((f_, n), 0) + 1
            line_t.append(ts)
        done = [t0 + SCAN + SETTLE for t0 in scans.get(i, [])]

        # visibility windows
        def visible_after(starts):
            """Visible from each start until the slip leaves; plus the opening frame if this order owns it."""
            w = []
            for a, b in win:
                if a == 0.0 and i == owner[0]:
                    w.append((a, b) if not any(a < s < b for s in starts) else (a, min(s for s in starts if a < s < b)))
                for s in starts:
                    if a <= s < b:
                        w.append((s, b))
            return w
        css.append(toggle(f"{cls}", win)); base.append((cls, win))
        for n, ts in enumerate(line_t):
            name = f"{cls}l{n}"; w = visible_after(ts)
            css.append(toggle(name, w)); base.append((name, w))
        for name, w in ((f"{cls}r", visible_after(done)), (f"{cls}s", visible_after(done))):
            css.append(toggle(name, w)); base.append((name, w))
        wr = [(a, d) for a in scans.get(i, []) for d in done if d > a and d - a < 3]
        wr = [(a - SCAN_DELAY, d) for a, d in wr]
        css.append(toggle(f"{cls}w", wr)); base.append((f"{cls}w", wr))
        for nm, ts in field_t.items():
            css.append(flash(f"{cls}h{nm}", ts, peak=0.6, fade=0.7))
        css.append("@keyframes " + f"{cls}scan{{0%{{opacity:0;transform:translateY(0)}}" + "".join(
            f"{pct(t0 - 0.001)}{{opacity:0;transform:translateY(0)}}{pct(t0)}{{opacity:1;transform:translateY(0)}}"
            f"{pct(t0 + SCAN)}{{opacity:1;transform:translateY({slip_h - 3}px)}}{pct(t0 + SCAN + 0.001)}{{opacity:0;transform:translateY({slip_h - 3}px)}}"
            for t0 in scans.get(i, [])) + "100%{opacity:0;transform:translateY(0)}}")

        # --- the route: the folder this note was filed in ---
        _, year, month = folder.split("/")
        x = 40
        parts = [("ORDERS", c["mute"]), ("/", c["ember"]), (year, c["mute"]), ("/", c["ember"]), (month.upper(), c["ink"])]
        g = []
        for s, col in parts:
            g.append(text(s, x, 76, 46, F["route"], col))
            x += advance(s, 46, F["route"]) + 7
        o.append(f'<g class="{cls}r">' + "".join(g) + "</g>")

        # --- the slip ---
        g = [f'<g class="{cls}">', f'<rect x="{sx + 1}" y="{sy + 1}" width="{sw - 2}" height="{slip_h - 2}" rx="5" fill="{c["paper"]}" stroke="{c["pink"]}" stroke-width="2"/>']
        for name, top, h in layout:
            for nm in (["date", "total"] if name == "row" else [name]):
                hx, hw = (sx + 2, sw - 4)
                if name == "row":
                    hx, hw = (sx + 2, 186) if nm == "date" else (sx + 190, sw - 192)
                g.append(f'<rect class="{cls}h{nm}" x="{hx}" y="{sy + top + (2 if top else 2)}" width="{hw}" height="{h - 2}" fill="{c["hl"]}" opacity="0"/>')
            if top:
                g.append(f'<rect x="{sx}" y="{sy + top}" width="{sw}" height="2" fill="{c["pink"]}"/>')
        g.append(f'<rect x="{sx + 188}" y="{sy + 98}" width="2" height="58" fill="{c["pink"]}"/>')
        lx = sx + 16
        g.append(text("ORDER #", lx, sy + 24, 11.5, F["label"], c["pink"], ls=0.12))
        g.append(f'<rect x="{sx + sw - 128}" y="{sy + 11}" width="112" height="19" rx="2" fill="none" stroke="{c["pink"]}" stroke-width="1.5"/>')
        g.append(text("MADE-UP ORDER", sx + sw - 72, sy + 25, 10.5, F["label"], c["pink"], ls=0.12, anchor="middle"))
        g.append(text(order["order_id"], lx, sy + 54, 26, F["big"], c["pink"], ls=0.03))
        g.append(barcode(order["order_id"], lx, sy + 64, sw - 32, 24, c["pink"]))
        g.append(text("ORDER PLACED", lx, sy + 98 + 22, 11.5, F["label"], c["pink"], ls=0.12))
        g.append(text(order["date"], lx, sy + 98 + 44, 17, F["value"], c["pink"]))
        g.append(text("TOTAL", sx + 204, sy + 98 + 22, 11.5, F["label"], c["pink"], ls=0.12))
        g.append(text(order["total"], sx + 204, sy + 98 + 44, 17, F["value"], c["pink"]))
        g.append(text("STATUS", lx, sy + 156 + 22, 11.5, F["label"], c["pink"], ls=0.12))
        g.append(text(order["status"].upper(), lx, sy + 156 + 47, 25, F["route"], c["pink"], ls=0.01))
        g.append(text("SHIP TO", lx, sy + 214 + 22, 11.5, F["label"], c["pink"], ls=0.12))
        g.append(text(order["ship_to"], lx, sy + 214 + 44, 17, F["value"], c["pink"]))
        n = len(order["items"])
        g.append(text(f"{n} ITEM" + ("S" if n > 1 else ""), lx, sy + 270 + 22, 11.5, F["label"], c["pink"], ls=0.12))
        iy = sy + 270 + 46
        for k, it in enumerate(order["items"]):
            if k:
                g.append(f'<path d="M{lx} {iy - 15}H{sx + sw - 16}" stroke="{c["pink"]}" stroke-opacity=".55" stroke-dasharray="4 3"/>')
            g.append(text(f"{k + 1:02d}", lx, iy, 12, F["monob"], c["pink"]))
            g.append(text(it["name"], lx + 28, iy, 15, F["value"], c["pink"]))
            g.append(text(f'{it["brand"]} · {it["current_price"]} · {it["asin"]}', lx + 28, iy + 19, 11.5, F["mono"], c["pink"]))
            posted = it.get("is_reviewed") and it.get("review_title")
            g.append(f'<rect x="{lx + 28.75}" y="{iy + 29.75}" width="8.5" height="8.5" rx="1" fill="{c["pmint"] if posted else "none"}" stroke="{c["pmint"] if posted else c["pink"]}" stroke-width="1.5"/>')
            g.append(text(f'REVIEW POSTED, {it["review_rating"]} OF 5' if posted else "NO REVIEW YET", lx + 43, iy + 38, 10.5, F["label"], c["pink"], ls=0.12))
            iy += 66
        g.append(f'<rect class="{cls}scan" x="{sx - 2}" y="{sy}" width="{sw + 4}" height="3" fill="{c["beam"]}" opacity="0"/>')
        g.append("</g>")
        o.append("".join(g))

        # --- the file tab: path, then Writing or Saved ---
        dirpart = folder + "/"
        tx = fx + 14
        o.append(f'<g class="{cls}">' + text(dirpart, tx, fy + 24, 12.5, F["mono"], c["mute"])
                 + text(fname, tx + advance(dirpart, 12.5, F["mono"]), fy + 24, 12.5, F["monob"], c["ink"]) + "</g>")
        for state, col, word in (("s", c["yes"], "SAVED"), ("w", c["ember"], "WRITING")):
            ww = advance(word, 12, F["label"], 0.12)
            o.append(f'<g class="{cls}{state}"><circle cx="{fx + fw - 22 - ww}" cy="{fy + 19.5}" r="4" fill="{col}"/>'
                     + text(word, fx + fw - 14, fy + 24, 12, F["label"], col, ls=0.12, anchor="end") + "</g>")

        # --- the note ---
        g = []
        ly, step, size = fy + tab + 26, 21.4, 14
        for n, (f_, toks) in enumerate(lines):
            x = fx + 16
            parts = []
            for s, kind in toks:
                col = {"key": c["key"], "ink": c["ink"], "mute": c["mute"], "yes": c["yes"], "no": c["no"], "rule": c["mute"], "head": c["ink"]}[kind]
                parts.append(text(s, x, ly + n * step, size, F["monob"] if kind == "head" else F["mono"], col))
                x += advance(s, size, F["mono"])
            g.append(f'<g class="{cls}l{n}">' + "".join(parts) + "</g>")
        o.append(f'<g clip-path="url(#win)">' + "".join(g) + "</g>")

    # fades over the right edge (long url lines) and the bottom (the note goes on)
    o.append(f'<g clip-path="url(#win)"><rect x="{fx + fw - 56}" y="{fy + tab + 1}" width="56" height="{fh - tab - 1}" fill="url(#fade)"/>'
             f'<rect x="{fx}" y="{fy + fh - 46}" width="{fw}" height="46" fill="url(#fadev)"/></g>')
    o.append(text("THE NOTE GOES ON: THE ITEMS, THEIR FEATURES, A REVIEW SECTION EACH", fx + 16, fy + fh - 14, 11, F["label"], c["mute"], ls=0.12))

    rules = "".join(f".{k}{{animation:{k} {CYCLE}s step-end infinite}}" for k, _ in base)
    for i in SHOW:
        rules += f".o{i}scan{{animation:o{i}scan {CYCLE}s linear infinite}}"
        rules += "".join(f".o{i}h{nm}{{animation:o{i}h{nm} {CYCLE}s linear infinite}}" for nm in ("order_id", "date", "total", "status", "ship_to", "items"))
    first = "".join(f".{k}{{opacity:{1 if any(a == 0.0 for a, _ in w) else 0}}}" for k, w in base)
    o.insert(1, f"<style>{first}@media (prefers-reduced-motion:no-preference){{{''.join(css)}{rules}}}</style>")
    defs = "".join(f'<path id="{gid}" d="{d}"/>' for gid, d in _glyphs.values() if d)
    o.insert(2, f"<defs>{defs}</defs>")
    o.append("</svg>")
    return "".join(o)


orders = json.load(open(os.path.join(ROOT, "tools", "samples.json"), encoding="utf-8"))["orders"]
notes = {}
for dirpath, _, files in os.walk(os.path.join(ROOT, "docs", "samples", "Orders")):
    for name in files:
        p = os.path.join(dirpath, name)
        notes[os.path.relpath(p, os.path.join(ROOT, "docs", "samples")).replace(os.sep, "/")] = open(p, encoding="utf-8").read()
for theme in THEMES:
    _glyphs.clear()
    path = os.path.join(OUT, f"slip-{theme}.svg")
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(build(theme, orders, notes) + "\n")
    print(f"wrote {os.path.relpath(path, ROOT)} ({os.path.getsize(path) // 1024} KB)")
