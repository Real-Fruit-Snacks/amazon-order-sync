// The project page's slips and notes, rendered from tools/samples.json and the notes the real
// server wrote for those orders. Used by tools/make-samples.mjs; not run on its own.
//
// Each order becomes a packing slip (the fields the extension reads from an order card) and the
// note, line by line. Every slip field and note line carries data-f, so the page script can light
// a field and print the lines that came from it.

const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

// Code 39: nine elements per character (bar, space, bar ...), 1 = wide. Wide is 2 narrow units.
const CODE39 = {
    '0': '000110100', '1': '100100001', '2': '001100001', '3': '101100000', '4': '000110001',
    '5': '100110000', '6': '001110000', '7': '000100101', '8': '100100100', '9': '001100100',
    '-': '010000101', '*': '010010100',
};

export function barcode(text) {
    let x = 0, d = '';
    for (const ch of `*${text}*`) {
        const p = CODE39[ch];
        if (!p) throw new Error(`no Code 39 pattern for ${ch}`);
        for (let i = 0; i < 9; i++) {
            const w = p[i] === '1' ? 2 : 1;
            if (i % 2 === 0) d += `M${x} 0h${w}v1h-${w}z`;
            x += w;
        }
        x += 1; // gap between characters
    }
    return { d, width: x - 1 };
}

// Which slip field each frontmatter key comes from.
const FIELD = { type: 'order_id', order_id: 'order_id', date: 'date', total: 'total', status: 'status', ship_to: 'ship_to',
    brand: 'items', price: 'items', asin: 'items', url: 'items', reviewed: 'items', rating: 'items', helpful_votes: 'items', tone: 'items', backfill: 'items' };

// A frontmatter value: quoted strings, true/false, and the [ ] , of an inline array.
function yamlValue(v) {
    let out = '', i = 0;
    while (i < v.length) {
        const c = v[i];
        if (c === '"') {
            let j = i + 1;
            while (j < v.length && v[j] !== '"') j += v[j] === '\\' ? 2 : 1;
            out += `<span class="t-s">${esc(v.slice(i, j + 1))}</span>`;
            i = j + 1;
        } else if ('[],'.includes(c)) { out += `<span class="t-p">${c}</span>`; i++; }
        else {
            const m = v.slice(i).match(/^(true|false)\b/);
            if (m) { out += `<span class="${m[1] === 'true' ? 't-y' : 't-n'}">${m[1]}</span>`; i += m[1].length; }
            else { out += esc(c); i++; }
        }
    }
    return out;
}

function bodyLine(line) {
    if (/^#{1,4} /.test(line)) return `<span class="t-h">${esc(line)}</span>`;
    if (line === '---') return `<span class="t-r">---</span>`;
    if (/^!\[/.test(line) || /^\[/.test(line)) return `<span class="t-l">${esc(line)}</span>`;
    if (/^> /.test(line)) return `<span class="t-q">${esc(line)}</span>`;
    let m = line.match(/^- \[([ x])\] (.*)$/);
    if (m) return `<span class="${m[1] === 'x' ? 't-y' : 't-p'}">- [${m[1]}]</span> ${esc(m[2])}`;
    m = line.match(/^(\*\*[^*]+:\*\*)(.*)$/);
    if (m) return `<span class="t-b">${esc(m[1])}</span>${esc(m[2])}`;
    return esc(line);
}

function note(md) {
    const lines = md.replace(/\n$/, '').split('\n');
    let fence = 0;
    return lines.map((line) => {
        let f = 'body', html;
        if (line === '---' && fence < 2) { fence++; f = fence === 1 ? 'order_id' : 'items'; html = '<span class="t-r">---</span>'; }
        else if (fence === 1) {
            const m = line.match(/^([a-z_]+): (.*)$/);
            f = (m && FIELD[m[1]]) || 'items';
            html = m ? `<span class="t-k">${m[1]}</span>: ${yamlValue(m[2])}` : esc(line);
        } else html = bodyLine(line);
        return `<span class="ln" data-f="${f}">${html}</span>`;
    }).join('\n');
}

// Every file the server wrote, one folder per row, as the "One file per order" listing.
export function tree(files) {
    const rows = new Map();
    for (const f of files) {
        const rel = f.replace(/^Orders\//, '');
        const dir = rel.slice(0, rel.lastIndexOf('/') + 1);
        if (!rows.has(dir)) rows.set(dir, []);
        rows.get(dir).push(rel.slice(dir.length));
    }
    const out = ['<li class="tr-root">~/AmazonOrders/Orders/</li>'];
    for (const [dir, names] of rows) {
        const label = dir.split('/').filter(Boolean).map(esc).join('<i>/</i>') + '<i>/</i>';
        out.push(`<li><span class="tr-d">${label}</span><span class="tr-f">${names.map((n) => `<span>${esc(n)}</span>`).join('')}</span></li>`);
    }
    return `<ul class="tree-list">\n${out.join('\n')}\n</ul>\n`;
}

export function render(orders, notes) {
    return orders.map((o, i) => {
        const rel = Object.keys(notes).find((k) => k.endsWith(`/Order-${o.order_id}.md`));
        if (!rel) throw new Error(`no note for ${o.order_id}`);
        const folder = rel.slice(0, rel.lastIndexOf('/'));
        const [, year, month] = folder.split('/');
        const file = rel.slice(rel.lastIndexOf('/') + 1);
        const bars = barcode(o.order_id);
        const items = o.items.map((it) => {
            const posted = it.is_reviewed && it.review_title;
            const review = posted
                ? `<span class="rv on">Review posted, ${esc(it.review_rating)} of 5</span>`
                : '<span class="rv">No review yet</span>';
            return `<li><span class="nm">${esc(it.name)}</span><span class="meta">${esc(it.brand)} · ${esc(it.current_price)} · ${esc(it.asin)}</span>${review}</li>`;
        }).join('');
        return `<div class="order" data-i="${i}"${i ? ' hidden' : ''}>
<p class="route" data-f="route" aria-label="Filed in ${esc(folder)}"><span>Orders</span><i>/</i><span>${esc(year)}</span><i>/</i><b>${esc(month)}</b></p>
<article class="slip" aria-label="Order ${esc(o.order_id)}, as the extension reads it from your order history">
<i class="scan" aria-hidden="true"></i>
<div class="sf s-id" data-f="order_id"><span class="k">Order #</span><span class="v">${esc(o.order_id)}</span><svg class="bars" viewBox="0 0 ${bars.width} 1" preserveAspectRatio="none" aria-hidden="true"><path d="${bars.d}"/></svg></div>
<div class="s-row"><div class="sf" data-f="date"><span class="k">Order placed</span><span class="v">${esc(o.date)}</span></div><div class="sf" data-f="total"><span class="k">Total</span><span class="v">${esc(o.total)}</span></div></div>
<div class="sf s-status" data-f="status"><span class="k">Status</span><span class="v">${esc(o.status)}</span></div>
<div class="sf" data-f="ship_to"><span class="k">Ship to</span><span class="v">${esc(o.ship_to)}</span></div>
<div class="sf s-items" data-f="items"><span class="k">${o.items.length === 1 ? '1 item' : `${o.items.length} items`}</span><ol>${items}</ol></div>
</article>
<div class="file">
<p class="file-tab"><span class="file-name"><span class="dir">${esc(folder)}/</span>${esc(file)}</span><span class="saved">Saved</span></p>
<pre class="note" tabindex="0" aria-label="${esc(file)}"><code>${note(notes[rel])}</code></pre>
</div>
</div>`;
    }).join('\n') + '\n';
}
