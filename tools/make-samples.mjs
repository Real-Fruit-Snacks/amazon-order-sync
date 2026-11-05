#!/usr/bin/env node
// Run the real server on made-up orders and keep exactly what it writes.
//
// tools/samples.json holds three orders shaped like the extension's POST /api/sync body. This
// starts server/server.js against a temporary notes folder, with no Gemini key and no network
// (product images come from a throwaway image server on 127.0.0.1), posts the orders, and copies
// the notes it wrote to docs/samples/. Then it writes the project page's slips and notes between
// the <!-- samples:begin --> and <!-- samples:end --> markers in docs/index.html, so the page
// shows the same text. tools/make-slip.py draws the README picture from the same files.
//
//   node tools/make-samples.mjs
//
// CI runs it and fails if docs/samples/ or docs/index.html changed.
import { spawn } from 'node:child_process';
import fs from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SERVER = path.join(ROOT, 'server', 'server.js');
const OUT = path.join(ROOT, 'docs', 'samples');
const PAGE = path.join(ROOT, 'docs', 'index.html');

// A valid 1x1 JPEG: the server only checks the Content-Type and the size.
const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');

const README = path.join(ROOT, 'README.md');

// Replace what sits between <!-- name:begin --> and <!-- name:end --> in a file.
async function fill(file, parts) {
    const old = await fs.readFile(file, 'utf8');
    let text = old;
    for (const [name, body] of parts) {
        const begin = `<!-- ${name}:begin -->`, end = `<!-- ${name}:end -->`;
        const a = text.indexOf(begin), b = text.indexOf(end);
        if (a === -1 || b < a) throw new Error(`${path.relative(ROOT, file)} has no ${begin} ... ${end}`);
        text = text.slice(0, a + begin.length) + '\n' + body + text.slice(b);
    }
    if (text !== old) await fs.writeFile(file, text);
}

const freePort = () => new Promise((ok, fail) => {
    const s = net.createServer().once('error', fail);
    s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => ok(port)); });
});

async function walk(dir) {
    const out = [];
    for (const e of await fs.readdir(dir, { withFileTypes: true })) {
        const p = path.join(dir, e.name);
        if (e.isDirectory()) out.push(...await walk(p)); else out.push(p);
    }
    return out.sort();
}

const { orders } = JSON.parse(await fs.readFile(path.join(ROOT, 'tools', 'samples.json'), 'utf8'));
const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'aos-samples-'));
const notes = path.join(tmp, 'AmazonOrders');
const images = http.createServer((req, res) => {
    res.writeHead(200, { 'Content-Type': 'image/jpeg', 'Content-Length': JPEG.length });
    res.end(JPEG);
});
await new Promise((ok) => images.listen(0, '127.0.0.1', ok));
const imageBase = `http://127.0.0.1:${images.address().port}/images/I`;

const port = await freePort();
const config = path.join(tmp, 'config.json');
await fs.writeFile(config, JSON.stringify({ notesPath: notes, geminiApiKey: '', port, notificationsEnabled: false }));
const env = { ...process.env, AMAZON_SYNC_CONFIG: config, GEMINI_API_KEY: '' };
const child = spawn(process.execPath, [SERVER], { env, stdio: ['ignore', 'pipe', 'pipe'] });
let log = '';
child.stdout.on('data', (d) => { log += d; });
child.stderr.on('data', (d) => { log += d; });
const exited = new Promise((ok) => child.once('exit', ok));
const base = `http://127.0.0.1:${port}`;

try {
    for (let i = 0; ; i++) {
        try { if ((await fetch(`${base}/api/health`)).ok) break; } catch { /* not up yet */ }
        if (i > 100) throw new Error(`the server did not start:\n${log}`);
        await new Promise((ok) => setTimeout(ok, 100));
    }
    for (const order of orders) {
        const body = JSON.stringify(order).replaceAll('{images}', imageBase);
        const res = await fetch(`${base}/api/sync`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body });
        if (!res.ok) throw new Error(`POST /api/sync ${order.order_id}: ${res.status} ${await res.text()}`);
    }
    await fetch(`${base}/api/shutdown`, { method: 'POST' }).catch(() => {});
    await Promise.race([exited, new Promise((ok) => setTimeout(ok, 5000))]);

    // Keep the notes; list every file the server wrote (the 1x1 images stay behind).
    await fs.rm(OUT, { recursive: true, force: true });
    const written = (await walk(notes)).map((f) => path.relative(notes, f).split(path.sep).join('/'));
    for (const rel of written.filter((f) => f.endsWith('.md'))) {
        await fs.mkdir(path.dirname(path.join(OUT, rel)), { recursive: true });
        await fs.copyFile(path.join(notes, rel), path.join(OUT, rel));
    }
    await fs.writeFile(path.join(OUT, 'files.txt'), written.join('\n') + '\n');
    console.log(`wrote docs/samples/ (${written.length} files written by the server)`);

    // The page's slips, notes and file list, and the README's file list and item, between markers.
    const md = {};
    for (const rel of written.filter((f) => f.endsWith('.md'))) md[rel] = await fs.readFile(path.join(notes, rel), 'utf8');
    const { render, tree } = await import('./page-samples.mjs');
    const first = orders[0];
    const note0 = md[Object.keys(md).find((k) => k.endsWith(`/Order-${first.order_id}.md`))];
    const posted = first.items.find((it) => it.is_reviewed && it.review_title);
    const from = note0.indexOf(`### ${posted.name}`), to = note0.indexOf('\n---\n', from) + 5;
    await fill(PAGE, [['samples', render(orders, md)], ['files', tree(written)]]);
    await fill(README, [
        ['sample-files', '```text\n' + written.map((f) => `~/AmazonOrders/${f}`).join('\n') + '\n```\n'],
        ['sample-item', '```markdown\n' + note0.slice(from, to) + '```\n'],
    ]);
    console.log('wrote the samples into docs/index.html and README.md');
} finally {
    if (child.exitCode === null) child.kill();
    images.close();
    await fs.rm(tmp, { recursive: true, force: true }).catch(() => {});
}
