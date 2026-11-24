// Parity test for the Rust port: the same requests go to the last Node.js server (server/ as
// released in v2.3.0, read from git) and to the Rust program, each with its own notes folder,
// and every response, every file each one writes and every prompt each one sends to Gemini
// must match once the parts that differ by nature (ports, paths, pids, timestamps, durations)
// are masked.
//
//   cargo build && cargo build --examples
//   AOS_SERVER_BIN=target/debug/amazon-order-sync[.exe] \
//   AOS_FAKE_BROWSER_BIN=target/debug/examples/fake_browser[.exe] \
//   node --test tests/parity.test.js
//
// Needs Node.js 18+ and git with the v2.3.0 tag (AOS_PARITY_REF names another). Skipped
// without AOS_SERVER_BIN. No network: images and Gemini are served by local mocks.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const BIN = process.env.AOS_SERVER_BIN ? path.resolve(process.env.AOS_SERVER_BIN) : '';
const REF = process.env.AOS_PARITY_REF || 'v2.3.0';
// The two sides report their own version numbers; those are masked, everything else must match.
const VERSIONS = [REF.replace(/^v/, ''), /^version = "([^"]+)"/m.exec(fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8'))[1]];
const unversion = (s) => VERSIONS.reduce((out, v) => out.split(v).join('<VERSION>'), s);
const FAKE = process.env.AOS_FAKE_BROWSER_BIN ? path.resolve(process.env.AOS_FAKE_BROWSER_BIN) : '';
const SKIP = BIN ? false : 'set AOS_SERVER_BIN to the built Rust binary';
const SKIP_HEADLESS = SKIP || (FAKE ? false : 'set AOS_FAKE_BROWSER_BIN to target/debug/examples/fake_browser');

// The reference: server/ and browser-extension/ at REF, with the test hooks the Rust server
// reads from the environment added (the Gemini endpoint, timeout and waits, the browser-cache
// TTL). Both servers load the extension from the same copy, so their browser command lines match.
const HOOKS = [
    [
        "let GEMINI_REQUEST_TIMEOUT_MS = GEMINI_REQUEST_TIMEOUT_DEFAULT_MS;\nexport function setGeminiTimeoutForTests(ms) {\n    GEMINI_REQUEST_TIMEOUT_MS = Number.isInteger(ms) && ms > 0 ? ms : GEMINI_REQUEST_TIMEOUT_DEFAULT_MS;",
        "// Test seams shared with the Rust server: AOS_GEMINI_BASE_URL points the Gemini calls at a\n// mock, AOS_GEMINI_TIMEOUT_MS shortens the request timeout, AOS_GEMINI_WAIT_SCALE scales the\n// 2 s pacing wait and the retry back-off.\nconst GEMINI_BASE_URL = process.env.AOS_GEMINI_BASE_URL || 'https://generativelanguage.googleapis.com';\nconst GEMINI_WAIT_SCALE = Number(process.env.AOS_GEMINI_WAIT_SCALE) >= 0 && process.env.AOS_GEMINI_WAIT_SCALE ? Number(process.env.AOS_GEMINI_WAIT_SCALE) : 1;\nconst GEMINI_TIMEOUT_ENV_MS = Number.isInteger(Number(process.env.AOS_GEMINI_TIMEOUT_MS)) && Number(process.env.AOS_GEMINI_TIMEOUT_MS) > 0 ? Number(process.env.AOS_GEMINI_TIMEOUT_MS) : GEMINI_REQUEST_TIMEOUT_DEFAULT_MS;\nlet GEMINI_REQUEST_TIMEOUT_MS = GEMINI_TIMEOUT_ENV_MS;\nexport function setGeminiTimeoutForTests(ms) {\n    GEMINI_REQUEST_TIMEOUT_MS = Number.isInteger(ms) && ms > 0 ? ms : GEMINI_TIMEOUT_ENV_MS;"
    ],
    [
        "        await new Promise(resolve => setTimeout(resolve, 2000));",
        "        await new Promise(resolve => setTimeout(resolve, 2000 * GEMINI_WAIT_SCALE));"
    ],
    [
        "                response = await geminiFetch(`https://generativelanguage.googleapis.com/v1beta/models/${model}:generateContent?key=${apiKey.trim()}`, {",
        "                response = await geminiFetch(`${GEMINI_BASE_URL}/v1beta/models/${model}:generateContent?key=${apiKey.trim()}`, {"
    ],
    [
        "                await new Promise(resolve => setTimeout(resolve, waitMs));",
        "                await new Promise(resolve => setTimeout(resolve, waitMs * GEMINI_WAIT_SCALE));"
    ],
    [
        "let browserCacheTtlMs = BROWSER_CACHE_TTL_DEFAULT_MS;",
        "let browserCacheTtlMs = Number.isInteger(Number(process.env.AOS_BROWSER_CACHE_TTL_MS)) && process.env.AOS_BROWSER_CACHE_TTL_MS ? Number(process.env.AOS_BROWSER_CACHE_TTL_MS) : BROWSER_CACHE_TTL_DEFAULT_MS;"
    ],
    [
        "                const r = await fetch(`https://generativelanguage.googleapis.com/v1beta/models?pageSize=100&key=${encodeURIComponent(store.geminiApiKey.trim())}`);",
        "                const r = await fetch(`${GEMINI_BASE_URL}/v1beta/models?pageSize=100&key=${encodeURIComponent(store.geminiApiKey.trim())}`);"
    ]
];

async function prepareReference() {
    const dir = await fsp.mkdtemp(path.join(os.tmpdir(), 'aos-parity-ref-'));
    const files = execFileSync('git', ['ls-tree', '-r', '--name-only', REF, 'server', 'browser-extension'], { cwd: ROOT })
        .toString('utf8').split('\n').filter(Boolean);
    for (const f of files) {
        const out = path.join(dir, ...f.split('/'));
        await fsp.mkdir(path.dirname(out), { recursive: true });
        await fsp.writeFile(out, execFileSync('git', ['show', `${REF}:${f}`], { cwd: ROOT, maxBuffer: 1 << 28 }));
    }
    const js = path.join(dir, 'server', 'server.js');
    let src = (await fsp.readFile(js, 'utf8')).replace(/\r\n/g, '\n');
    for (const [a, b] of HOOKS) {
        if (!src.includes(a)) throw new Error(`${REF}:server/server.js no longer has: ${a.slice(0, 80)}`);
        src = src.replace(a, () => b);
    }
    await fsp.writeFile(js, src);
    return { dir, serverJs: js, extension: path.join(dir, 'browser-extension') };
}

const REFERENCE = SKIP ? null : await prepareReference();
process.on('exit', () => { if (REFERENCE) fs.rmSync(REFERENCE.dir, { recursive: true, force: true }); });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const freePort = () => new Promise((ok, fail) => {
    const s = net.createServer().once('error', fail);
    s.listen(0, '127.0.0.1', () => { const { port } = s.address(); s.close(() => ok(port)); });
});

// ---------------------------------------------------------------------------
// Mocks: product images and the Gemini API. One instance per server, so
// request counts line up.
// ---------------------------------------------------------------------------

const JPEG = Buffer.from('/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=', 'base64');
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');

async function startMock(handler) {
    const srv = http.createServer(handler);
    await new Promise((ok) => srv.listen(0, '127.0.0.1', ok));
    return { url: `http://127.0.0.1:${srv.address().port}`, close: () => new Promise((ok) => { srv.closeAllConnections?.(); srv.close(ok); }) };
}

function imageMock() {
    return startMock((req, res) => {
        const u = req.url;
        if (u.startsWith('/img.jpg')) { res.writeHead(200, { 'Content-Type': 'image/jpeg', 'Content-Length': JPEG.length }); res.end(JPEG); }
        else if (u.startsWith('/img.png')) { res.writeHead(200, { 'Content-Type': 'image/png; charset=binary' }); res.end(PNG); }
        else if (u.startsWith('/pic.gif')) { res.writeHead(200, { 'Content-Type': 'image/webp' }); res.end(PNG); }
        else if (u.startsWith('/page')) { res.writeHead(200, { 'Content-Type': 'text/html' }); res.end('<html></html>'); }
        else if (u.startsWith('/tiff')) { res.writeHead(200, { 'Content-Type': 'image/tiff' }); res.end(PNG); }
        else if (u.startsWith('/big-declared')) { res.writeHead(200, { 'Content-Type': 'image/jpeg', 'Content-Length': 6 * 1024 * 1024 }); res.end(JPEG); }
        else if (u.startsWith('/big-stream')) {
            res.writeHead(200, { 'Content-Type': 'image/jpeg' });
            const chunk = Buffer.alloc(1024 * 1024, 1);
            for (let i = 0; i < 6; i++) res.write(chunk);
            res.end();
        }
        else if (u.startsWith('/redirect')) { res.writeHead(302, { Location: '/img.jpg' }); res.end(); }
        else { res.writeHead(404); res.end('nope'); }
    });
}

// A scripted Gemini: `script` is a list of responses, one per generateContent call (the last
// one repeats). Each request body is recorded.
async function geminiMock(script) {
    const calls = [];
    const mock = await startMock((req, res) => {
        let body = '';
        req.on('data', (d) => { body += d; });
        req.on('end', async () => {
            if (req.url.startsWith('/v1beta/models?')) {
                res.writeHead(200, { 'Content-Type': 'application/json' });
                res.end(JSON.stringify({ models: [
                    { name: 'models/gemini-a', displayName: 'A', description: 'first', supportedGenerationMethods: ['generateContent'] },
                    { name: 'models/embed', displayName: 'E', supportedGenerationMethods: ['embedContent'] },
                    { name: 'models/gemini-b', supportedGenerationMethods: ['countTokens', 'generateContent'] }
                ] }));
                return;
            }
            const i = calls.length;
            let prompt = null;
            try { prompt = JSON.parse(body); } catch { /* keep null */ }
            // A regenerate (sent here with the gemini-other model) rotates its shape and voice
            // samples on a random seed: mask those two sections; sync-time prompts stay exact.
            if (req.url.includes('gemini-other') && prompt?.contents?.[0]?.parts?.[0]) {
                prompt.contents[0].parts[0].text = prompt.contents[0].parts[0].text
                    .replace(/(# Shape for THIS review[^\n]*\n)[^\n]*\n/, '$1<SHAPE>\n')
                    .replace(/\n# My past reviews[\s\S]*?---END MY PAST REVIEWS---\n/, '<SAMPLES>');
            }
            calls.push({ url: req.url.replace(/key=[^&]*/, 'key=<KEY>'), body: prompt });
            const step = script[Math.min(i, script.length - 1)];
            if (step.hang) { await sleep(step.hang); }
            if (res.destroyed) return;
            res.writeHead(step.status ?? 200, { 'Content-Type': 'application/json' });
            res.end(typeof step.raw === 'string' ? step.raw : JSON.stringify(step.json ?? {
                candidates: [{ content: { parts: [{ text: JSON.stringify(step.draft ?? { title: 'Mock title', text: 'Mock body' }) }] } }]
            }));
        });
    });
    return { ...mock, calls };
}

// ---------------------------------------------------------------------------
// Servers
// ---------------------------------------------------------------------------

async function startServer(kind, { config = {}, env = {} } = {}) {
    const dir = await fsp.mkdtemp(path.join(os.tmpdir(), `aos-parity-${kind}-`));
    const notes = path.join(dir, 'notes');
    const port = await freePort();
    const configPath = path.join(dir, 'config.json');
    await fsp.writeFile(configPath, JSON.stringify({ notesPath: notes, geminiApiKey: '', port, reviewCutoffDays: 0, ...config }, null, 2) + '\n');
    const cmd = kind === 'rust' ? [BIN, ['serve']] : [process.execPath, [REFERENCE.serverJs]];
    const child = spawn(...cmd, {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: configPath,
            GEMINI_API_KEY: '',
            AOS_PROFILE_DIR: path.join(dir, 'profile'),
            AOS_EXTENSION_DIR: REFERENCE.extension,
            XDG_CONFIG_HOME: path.join(dir, 'xdg'),
            ...env
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    let output = '';
    child.stdout.on('data', (d) => { output += d; });
    child.stderr.on('data', (d) => { output += d; });
    const base = `http://127.0.0.1:${port}`;
    for (let i = 0; ; i++) {
        try { if ((await fetch(`${base}/api/health`)).ok) break; } catch { /* not up */ }
        if (i > 100 || child.exitCode !== null) throw new Error(`${kind} server did not start:\n${output}`);
        await sleep(100);
    }
    const transcript = [];
    const ctx = {
        kind, dir, notes, port, base, child, transcript,
        output: () => output,
        // Notes without a date sort by mtime: give each note the same mtime on both sides.
        async pinMtimes() {
            const walk = async (d) => {
                for (const e of await fsp.readdir(d, { withFileTypes: true }).catch(() => [])) {
                    const p = path.join(d, e.name);
                    if (e.isDirectory()) await walk(p);
                    else if (e.name.endsWith('.md')) {
                        const rel = path.relative(notes, p).split(path.sep).join('/');
                        const h = parseInt(crypto.createHash('sha256').update(rel).digest('hex').slice(0, 8), 16);
                        const t = new Date(1_700_000_000_000 + (h % 1_000_000) * 1000);
                        await fsp.utimes(p, t, t);
                    }
                }
            };
            await walk(path.join(notes, 'Orders'));
        },
        // One request; the masked response lands in the transcript.
        async call(method, pathname, body, headers = {}, { record = true, label } = {}) {
            const init = { method, headers: { ...headers } };
            if (body !== undefined) {
                if (typeof body === 'string' || Buffer.isBuffer(body)) init.body = body;
                else { init.body = JSON.stringify(body); init.headers['Content-Type'] ??= 'application/json'; }
            }
            let res;
            try { res = await fetch(`${base}${pathname}`, init); }
            catch (e) { if (record) transcript.push({ step: label ?? `${method} ${pathname}`, error: 'network' }); return { status: 0 }; }
            const text = await res.text();
            let json;
            try { json = JSON.parse(text); } catch { json = undefined; }
            const keep = ['content-type', 'content-disposition', 'cache-control', 'access-control-allow-origin',
                'access-control-allow-methods', 'access-control-allow-headers', 'vary'];
            const hdrs = {};
            for (const k of keep) { const v = res.headers.get(k); if (v !== null) hdrs[k] = v; }
            const entry = { step: label ?? `${method} ${pathname}`, status: res.status, headers: hdrs };
            if (json !== undefined) entry.json = json;
            else entry.text = text.length > 4000 ? `<${text.length} bytes sha256 ${crypto.createHash('sha256').update(unversion(text)).digest('hex').slice(0, 16)}>` : text;
            if (record) transcript.push(mask(entry, ctx));
            return { status: res.status, json, text, headers: res.headers };
        },
        async stop() {
            if (child.exitCode === null) {
                child.kill();
                await Promise.race([new Promise((ok) => child.once('exit', ok)), sleep(3000)]);
            }
            await fsp.rm(dir, { recursive: true, force: true }).catch(() => {});
        }
    };
    return ctx;
}

// ---------------------------------------------------------------------------
// Masking and snapshots
// ---------------------------------------------------------------------------

function escapeRe(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); }

function maskString(s, ctx) {
    let out = unversion(s);
    for (const m of Object.values(ctx.mocks ?? {})) if (m?.url) out = out.split(m.url).join('<MOCK>');
    // Node prints an error's stack; the class name stays, the frames go.
    out = out.replace(/\n    at [^\n]*/g, '');
    for (const p of [ctx.dir, ctx.dir.replace(/\\/g, '/'), ctx.dir.replace(/\\/g, '\\\\')]) {
        out = out.split(p).join('<DIR>');
    }
    out = out.replace(new RegExp(`(127\\.0\\.0\\.1|localhost):${ctx.port}`, 'g'), '$1:<PORT>');
    out = out.replace(new RegExp(`\\b${ctx.port}\\b`, 'g'), '<PORT>');
    out = out.replace(/\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z/g, '<ISO>');
    out = out.replace(/amazon-orders-\d{4}-\d\d-\d\d/g, 'amazon-orders-<DATE>');
    out = out.replace(/#aos-(autosync|reviewcheck)-\d+/g, '#aos-$1-<TS>');
    out = out.replace(/\(\d+s\)$/, '(<N>s)').replace(/, \d+s$/, ', <N>s');
    out = out.replace(/\b1\d{12}\b/g, '<TS>');
    return out;
}

const TIMED_KEYS = new Set(['t', 'since', 'lastRunAt', 'lastReviewCheckAt', 'generatedAt', 'lastSyncAt', 'modified',
    'runStartedAt', 'durationMs', 'pid', 'startedAt', 'exported_at']);

function mask(v, ctx, key) {
    if (typeof v === 'string') return TIMED_KEYS.has(key) ? '<T>' : maskString(v, ctx);
    if (typeof v === 'number') return TIMED_KEYS.has(key) || (v > 1e12 && v < 1e14) ? '<T>' : (v === ctx.port ? '<PORT>' : v);
    if (Array.isArray(v)) return v.map((x) => mask(x, ctx));
    if (v && typeof v === 'object') {
        const o = {};
        for (const [k, x] of Object.entries(v)) o[k] = (x === null ? null : mask(x, ctx, k));
        return o;
    }
    return v;
}

async function snapshot(ctx) {
    const out = {};
    const walk = async (dir) => {
        let entries;
        try { entries = await fsp.readdir(dir, { withFileTypes: true }); } catch { return; }
        for (const e of entries) {
            const p = path.join(dir, e.name);
            if (e.isDirectory()) { await walk(p); continue; }
            const rel = path.relative(ctx.notes, p).split(path.sep).join('/').replace(/\.tmp-\d+-[0-9a-f]+$/, '.tmp-<X>');
            const buf = await fsp.readFile(p);
            if (rel.endsWith('.json') || rel === '.daemon.lock') {
                let j;
                try { j = JSON.parse(buf.toString('utf8')); } catch { j = undefined; }
                // Key order and layout matter too: compare the masked text, then the masked value.
                out[rel] = j === undefined ? maskString(buf.toString('utf8'), ctx)
                    : { text: maskString(buf.toString('utf8'), ctx).replace(/: \d+(\.\d+)?/g, ': <N>'), value: mask(j, ctx) };
            } else if (rel.endsWith('.md')) {
                out[rel] = maskString(buf.toString('utf8'), ctx);
            } else {
                out[rel] = `<${buf.length} bytes sha256 ${crypto.createHash('sha256').update(buf).digest('hex').slice(0, 16)}>`;
            }
        }
    };
    await walk(ctx.notes);
    return Object.fromEntries(Object.entries(out).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
}

// Run `script` against both servers (in parallel) and compare everything.
async function both(opts, script, { compareLogs = false } = {}) {
    const run = async (kind) => {
        const mocks = opts.mocks ? await opts.mocks() : {};
        const env = typeof opts.env === 'function' ? opts.env(mocks) : (opts.env ?? {});
        const config = typeof opts.config === 'function' ? opts.config(mocks) : (opts.config ?? {});
        const ctx = await startServer(kind, { config, env });
        ctx.mocks = mocks;
        try {
            const extra = await script(ctx);
            const result = { transcript: ctx.transcript, files: await snapshot(ctx) };
            if (extra !== undefined) result.extra = mask(extra, ctx);
            if (mocks.gemini) result.gemini = mocks.gemini.calls.map((c) => mask(c, ctx));
            if (compareLogs) {
                const logs = (await ctx.call('GET', '/api/logs', undefined, {}, { record: false })).json;
                // Repeats of one line (the waiting ticks) collapse: how many land
                // before the watchdog depends on timer jitter, not on the server.
                result.logs = logs.lines.map((l) => maskString(l.text, ctx)).filter((l, i, a) => i === 0 || l !== a[i - 1]);
            }
            return result;
        } finally {
            await ctx.stop();
            for (const m of Object.values(mocks)) await m.close?.();
        }
    };
    const [node, rust] = await Promise.all([run('node'), run('rust')]);
    // PARITY_DUMP=<dir> keeps both sides' results for a closer look.
    if (process.env.PARITY_DUMP) {
        await fsp.mkdir(process.env.PARITY_DUMP, { recursive: true });
        const name = (script.name || 'scenario') + '-' + Date.now();
        await fsp.writeFile(path.join(process.env.PARITY_DUMP, `${name}.node.json`), JSON.stringify(node, null, 2));
        await fsp.writeFile(path.join(process.env.PARITY_DUMP, `${name}.rust.json`), JSON.stringify(rust, null, 2));
    }
    const sections = Object.keys(node);
    for (const s of sections) {
        if (s === 'transcript') {
            for (let i = 0; i < Math.max(node.transcript.length, rust.transcript.length); i++) {
                assert.deepEqual(rust.transcript[i], node.transcript[i], `transcript step ${i}: ${node.transcript[i]?.step}`);
            }
        } else if (s === 'files') {
            assert.deepEqual(Object.keys(rust.files), Object.keys(node.files), 'the same files are written');
            for (const f of Object.keys(node.files)) assert.deepEqual(rust.files[f], node.files[f], `file ${f}`);
        } else {
            assert.deepEqual(rust[s], node[s], s);
        }
    }
    return { node, rust };
}

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

const ORDER_A = {
    order_id: '111-1111111-1111111',
    date: 'August 1, 2026',
    total: '$42.50',
    status: 'Delivered',
    ship_to: 'Tester',
    items: [
        { asin: 'B0DEMOAAA1', name: 'USB-C Cable 2m', brand: 'Acme', current_price: '$12.50', link: 'https://www.amazon.com/dp/B0DEMOAAA1',
            features: '- Braided nylon\n- 100W', image: '', is_reviewed: true, review_title: 'Imported title',
            review_text: 'Imported body, with comma', review_rating: '5', helpful_votes: 3 },
        { asin: 'B0DEMOBBB2', name: 'Wireless Mouse, Ergonomic', brand: 'Acme', current_price: '$30.00', link: 'https://www.amazon.com/dp/B0DEMOBBB2',
            features: '- Silent clicks\n- 2.4 GHz', image: '', top_reviews: ['First customer review body', 'Second', '', 'Third'],
            listing_price: '$25.00' }
    ]
};

const ORDER_TRICKY = {
    order_id: '222-2222222-2222222',
    date: 'Ordered on 3 March 2025',
    total: 42.5,
    status: 'Shipped "soon"\nreturn: [x]',
    ship_to: 'O\'Brien, \\ Esq.',
    items: [
        { asin: 'B0TRICK001', name: 'Brand ] test 📦 中文', brand: 'Foo]Bar', current_price: '1.234,56 €', link: '', features: 'line one\n---\nline two', image: null, helpful_votes: '7', review_rating: 0, listing_price: '1.000,00 €' },
        { asin: 'B0TRICK002', name: 'Acme, Inc. thing', brand: 'Acme, Inc.', current_price: '12,99', features: '', helpful_votes: null, review_rating: '4' },
        { asin: 'B0TRICK001', name: 'duplicate of first', brand: 'dup' },
        { asin: 'B0TRICK003', brand: '', current_price: '', helpful_votes: -1, features: 5, image: 7 },
        { asin: 'B0TRICK004', name: 'votes', helpful_votes: 1.5, current_price: '$0.00', listing_price: '$1.00' },
        { asin: 1234567, name: 'numeric asin' },
        { name: 'no asin at all' }
    ]
};

const DATES = ['August 1, 2026', '1 August 2026', '2026-08-01', '2026-08-01T10:00:00', 'Ordered on Unknown', '', 'garbage', 'Feb 30, 2026',
    '12/25/2025', 'Dec 5 99', 'Sat Aug 01 2026 12:00:00 GMT+0200', '2026-13-01', 'July 4, 1776', 'Sept. 9, 2024'];

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

test('parity: sync, read back, edit, export', { skip: SKIP }, async () => {
    await both({}, async (s) => {
        await s.call('POST', '/api/sync', ORDER_A);
        await s.call('POST', '/api/sync', ORDER_TRICKY);
        for (const [i, date] of DATES.entries()) {
            await s.call('POST', '/api/sync', { order_id: `333-0000000-00000${String(i).padStart(2, '0')}`, date, total: '$1', status: 'Delivered', ship_to: 'X',
                items: [{ asin: `B0DATE${String(i).padStart(4, '0')}`, name: `Date ${i}`, current_price: '$1.00' }] }, {}, { label: `date ${date}` });
        }
        // Odd payloads.
        for (const [label, body] of [['null', 'null'], ['number', '5'], ['string', '"x"'], ['array', '[]'], ['empty', '{}'],
            ['dotdot', JSON.stringify({ order_id: '../x', items: [] })], ['slash', JSON.stringify({ order_id: 'a/b', items: [] })],
            ['items not array', JSON.stringify({ order_id: '444-1', items: 'no' })],
            ['null item', JSON.stringify({ order_id: '444-2', items: [{ asin: 'B0NULLITM1', name: 'ok' }, null] })],
            ['number item', JSON.stringify({ order_id: '444-3', items: [5, { asin: 'B0NUMITEM1' }] })],
            ['bad json', '{nope'], ['empty body', '']]) {
            await s.call('POST', '/api/sync', body, { 'Content-Type': 'application/json' }, { label: `odd sync ${label}` });
        }
        await s.pinMtimes();
        await s.call('GET', '/api/orders');
        for (const id of ['111-1111111-1111111', '222-2222222-2222222', '333-0000000-0000004', 'missing', '..', '', 'a b']) {
            await s.call('GET', `/api/check?order_id=${encodeURIComponent(id)}`);
        }
        await s.call('GET', '/api/check');
        for (const asin of ['B0DEMOAAA1', 'B0DEMOBBB2', 'B0TRICK001', 'nope', '']) await s.call('GET', `/api/get-review?asin=${asin}`);
        // Edits.
        await s.call('POST', '/api/mark-posted', { asin: 'B0DEMOBBB2', posted: true });
        await s.call('POST', '/api/mark-posted', { asin: 'B0DEMOBBB2', posted: true });
        await s.call('POST', '/api/mark-posted', { asin: 'B0DEMOBBB2', posted: false });
        await s.call('POST', '/api/mark-posted', { asin: 'NOPE000001', posted: true });
        await s.call('POST', '/api/mark-posted', 'null', { 'Content-Type': 'application/json' }, { label: 'mark-posted null' });
        await s.call('POST', '/api/save-review', { asin: 'B0DEMOBBB2', title: 'My title\nwith newline', text: 'Body line\n---\nafter rule', rating: 4 });
        await s.call('POST', '/api/save-review', { asin: 'B0DEMOBBB2', title: 'Again', text: 'No rating this time' });
        await s.call('POST', '/api/save-review', { asin: 'B0DEMOBBB2', title: 'x', text: '   ' });
        await s.call('POST', '/api/save-review', { asin: 'B0DEMOBBB2', title: 'x', text: 'y', rating: 6 });
        await s.call('POST', '/api/save-review', { asin: 'B0DEMOBBB2', title: 'x', text: 'y', rating: '3' });
        await s.call('POST', '/api/save-review', { asin: 'B0TRICK003', title: 'T3', text: 'review for item three' });
        await s.call('GET', '/api/get-review?asin=B0DEMOBBB2');
        for (const [rating, tone] of [[undefined, undefined], [2, 'critical'], [5, 'concise'], [3, 'bogus'], [0, 'balanced'], [null, null]]) {
            await s.call('POST', '/api/regenerate-review', { asin: 'B0TRICK002', rating, tone }, {}, { label: `regenerate ${rating} ${tone}` });
        }
        await s.call('POST', '/api/regenerate-review', { asin: 'B0TRICK001', model: 'x y' });
        // Re-sync keeps edits and the checkbox; a new date moves the note.
        await s.call('POST', '/api/mark-posted', { asin: 'B0TRICK002', posted: true });
        await s.call('POST', '/api/sync', ORDER_TRICKY, {}, { label: 're-sync tricky' });
        await s.call('POST', '/api/sync', { ...ORDER_A, date: 'September 2, 2026', status: 'Returned' }, {}, { label: 're-sync moved' });
        // Review-check results.
        for (const body of [
            { asin: 'B0DEMOAAA1', reviewed: true, title: 'Live title', text: 'Live text', helpful_votes: 9, rating: 4 },
            { asin: 'B0DEMOBBB2', reviewed: false, helpful_votes: 2 },
            { asin: 'B0TRICK002', reviewed: false, page_state: 'captcha' },
            { asin: 'B0TRICK002', reviewed: false },
            { asin: 'B0TRICK002', reviewed: false },
            { asin: 'B0TRICK002', reviewed: 'yes' },
            { asin: 'B0TRICK002', page_state: 'weird' },
            { asin: 'B0TRICK002', reviewed: true, title: '', text: 'x' },
            { asin: 'B0TRICK002', helpful_votes: -3 },
            { asin: 'B0TRICK002', rating: 9 },
            { asin: 'NOPE000001' }
        ]) await s.call('POST', '/api/review-check-result', body, {}, { label: `review-check-result ${JSON.stringify(body)}` });
        await s.call('POST', '/api/dismiss-missing', { asin: 'B0TRICK002' });
        await s.call('POST', '/api/dismiss-missing', { asin: 'B0TRICK002' });
        await s.call('GET', '/api/review-check-queue');
        await s.pinMtimes();
        await s.call('GET', '/api/orders');
        await s.pinMtimes();
        await s.call('GET', '/api/export.csv');
        await s.call('GET', '/api/export.json');
        await s.call('GET', '/api/run-options');
        await s.call('GET', '/api/gemini-usage');
        await s.call('GET', '/api/gemini-models');
        await s.call('GET', '/api/notifications');
        await s.call('GET', '/api/notifications?after=0');
        await s.call('POST', '/api/sync-history/clear');
        await s.call('POST', '/api/headless-complete', { ok: true });
        await s.call('POST', '/api/sync-stop');
        await s.call('POST', '/api/client-log', { lines: ['one', 'two'] });
        await s.call('POST', '/api/client-log', { lines: [] });
        await s.call('POST', '/api/client-log', { lines: [1] });
    });
});

test('parity: backfill import', { skip: SKIP }, async () => {
    const csv = [
        '\ufeffOrder ID,Order Date,Product Name,ASIN,Quantity,Unit Price,Currency,Total Owed,Order Status,Shipping Address',
        '555-0000000-0000001,2024-05-14T10:00:00Z,"Thing, with comma",B0BACK0001,2,12.5,USD,25.00,Closed,"Jane Doe, 1 Road, Town"',
        '555-0000000-0000001,2024-05-14,Second thing,B0BACK0002,x,"1,234.56",EUR,25.00,,',
        '555-0000000-0000001,2024-05-14,Dup asin,B0BACK0001,1,3,USD,,,',
        '555-0000000-0000002,05/31/2023,"Multi\nline ""quoted""",B0BACK0003,,,,9.99,,Somewhere',
        '555-0000000-0000003,2023-02-30,Bad date,B0BACK0004,1,free,usd,1,Shipped,',
        ',2023-01-01,No id,B0BACK0005,1,1,USD,1,,',
        '../evil,2023-01-01,Evil,B0BACK0006,1,1,USD,1,,',
        ''
    ].join('\r\n');
    await both({}, async (s) => {
        await s.call('POST', '/api/import-backfill', csv, { 'Content-Type': 'text/csv' });
        await s.call('POST', '/api/import-backfill', csv, { 'Content-Type': 'text/csv; charset=utf-8' }, { label: 're-import' });
        await s.call('POST', '/api/import-backfill', { files: [{ name: 'a.csv', content: 'order id,product name,asin,order date\n666-1,One,B0BACK0101,2022-01-02\n' }, { content: 'Order ID,Product Name,ASIN\n666-2,Two,B0BACK0102' }] });
        await s.call('POST', '/api/import-backfill', { files: [{ name: 'bad' }] });
        await s.call('POST', '/api/import-backfill', { nope: 1 });
        await s.pinMtimes();
        await s.call('GET', '/api/orders');
        await s.call('GET', '/api/review-check-queue');
    });
});

test('parity: product images', { skip: SKIP }, async () => {
    await both({ mocks: async () => ({ images: await imageMock() }) }, async (s) => {
        const u = s.mocks.images.url;
        const items = [
            ['B0IMAGE001', `${u}/img.jpg?x=1`], ['B0IMAGE002', `${u}/img.png`], ['B0IMAGE003', `${u}/pic.gif`],
            ['B0IMAGE004', `${u}/page.html`], ['B0IMAGE005', `${u}/tiff`], ['B0IMAGE006', `${u}/big-declared.jpg`],
            ['B0IMAGE007', `${u}/big-stream.jpg`], ['B0IMAGE008', `${u}/missing.jpg`], ['B0IMAGE009', `${u}/redirect`],
            ['B0IMAGE010', 'ftp://example.invalid/x.jpg'], ['B0IMAGE011', 'HTTP://127.0.0.1:1/refused.jpg']
        ].map(([asin, image]) => ({ asin, name: asin, image, current_price: '$1' }));
        await s.call('POST', '/api/sync', { order_id: '777-1', date: 'May 5, 2026', total: '$1', status: 'Delivered', ship_to: 'X', items });
        await s.call('POST', '/api/sync', { order_id: '777-2', date: 'May 6, 2026', total: '$1', status: 'Delivered', ship_to: 'X', items: [{ asin: 'B0IMAGE001', name: 'again', image: `${u}/img.png` }] });
        await s.pinMtimes();
        await s.call('GET', '/api/orders');
        for (const p of ['/assets/B0IMAGE001.jpg', '/assets/B0IMAGE002.png', '/assets/b0image001.JPG', '/assets/..%2Fx.jpg', '/assets/%2e%2e%5cx.png',
            '/assets/x.txt', '/assets/', '/assets/%E0%A4%A.jpg', '/assets/B0IMAGE001.jpg?v=1', '/assets/../api/orders',
            '/fonts/OFL.txt', '/fonts/nope.woff2', '/fonts/..%2Fserver.js',
            '/icons/x.svg', '/nope']) {
            await s.call('GET', p);
        }
        // Embedded UI assets (dashboard HTML, Cyclorama Dawn fonts & icons) updated past v2.3.0:
        // verify status + Content-Type parity without freezing visual bytes at v2.3.0.
        for (const p of ['/icons/icon-192.png', '/icons/favicon.ico', '/favicon.ico', '/', '/dashboard', '/dashboard/']) {
            const r = await s.call('GET', p, undefined, {}, { record: false });
            s.transcript.push({ step: `ui ${p}`, status: r.status, contentType: r.headers.get('content-type') });
        }
        if (s.kind === 'rust') {
            for (const f of ['big-shoulders-stencil-latin.woff2', 'albert-sans-latin.woff2', 'azeret-mono-latin.woff2']) {
                const r = await s.call('GET', `/fonts/${f}`, undefined, {}, { record: false });
                assert.equal(r.status, 200, `/fonts/${f} status`);
                assert.equal(r.headers.get('content-type'), 'font/woff2');
            }
        }
    });
});

test('parity: HTTP locks and limits', { skip: SKIP }, async () => {
    await both({}, async (s) => {
        const port = s.port;
        for (const host of [`127.0.0.1:${port}`, `localhost:${port}`, `LOCALHOST:${port}`, `127.0.0.1:${port + 1}`, `evil.example:${port}`, `127.0.0.1`]) {
            // fetch() won't override Host; use a raw request.
            const status = await new Promise((ok) => {
                const r = http.request({ host: '127.0.0.1', port, path: '/api/health', headers: { Host: host } }, (res) => { res.resume(); ok(res.statusCode); });
                r.on('error', () => ok(0));
                r.end();
            });
            s.transcript.push({ step: `host ${host.replace(String(port), '<PORT>').replace(String(port + 1), '<PORT+1>')}`, status });
        }
        const origins = [`chrome-extension://cikcnmangaegpkglmcfgbfbmknjmdifh`, 'chrome-extension://otherextensionid', `http://127.0.0.1:${port}`,
            `http://localhost:${port}`, `http://127.0.0.1:${port + 1}`, 'https://www.amazon.com', 'https://evil.example', 'null', `https://127.0.0.1:${port}`,
            'http://127.0.0.1', 'not a url'];
        for (const origin of origins) {
            const label = origin.replace(String(port + 1), '<PORT+1>').replace(String(port), '<PORT>');
            await s.call('GET', '/api/run-options', undefined, { Origin: origin }, { label: `origin ${label}` });
            await s.call('OPTIONS', '/api/sync', undefined, { Origin: origin }, { label: `preflight ${label}` });
        }
        // Node answers 413 and drops the socket, so fetch may see either.
        for (const [p, body] of [['/api/sync', 'x'.repeat(6 * 1024 * 1024)], ['/api/save-review', JSON.stringify({ asin: 'B0X', title: 'x', text: 'y'.repeat(6 * 1024 * 1024) })]]) {
            const r = await s.call('POST', p, body, { 'Content-Type': 'application/json' }, { record: false });
            s.transcript.push({ step: `oversize ${p}`, refused: r.status === 413 || r.status === 0 });
        }
        for (const p of ['/api/checkAnything', '/api/check/', '/API/CHECK?order_id=1', '/api/health?x=1', '/api/logs/clear', '//api/health', '/api/./health', '/api/x/../health']) {
            await s.call('GET', p);
        }
        for (const p of ['/api/save-review', '/api/regenerate-review', '/api/review-check-result', '/api/dismiss-missing', '/api/headless-complete', '/api/client-log']) {
            await s.call('POST', p, '{broken', { 'Content-Type': 'application/json' }, { label: `malformed ${p}` });
            await s.call('POST', p, 'null', { 'Content-Type': 'application/json' }, { label: `null ${p}` });
        }
        await s.call('GET', '/api/sync');
        await s.call('POST', '/api/orders');
        await s.call('PUT', '/api/sync', '{}');
        await s.call('DELETE', '/api/orders');
        await s.call('GET', '/api/logs?after=abc');
        await s.call('GET', '/api/notifications?after=-1');
    });
});

test('parity: Gemini drafts, retries, budget and prompts', { skip: SKIP }, async () => {
    const script = [
        { draft: { title: 'First draft', text: 'Draft body one.' } },
        { status: 429 }, { draft: { title: 'After 429', text: 'Body after 429.' } },
        { status: 503 }, { draft: { title: 'After 503', text: 'Body after 503.' } },
        { status: 400, raw: '{"error":{"message":"bad   request\n  here"}}' },
        { raw: 'not json' },
        { json: { candidates: [] } },
        { json: { candidates: [{ content: { parts: [{ text: 'not json either' }] } }] } },
        { draft: { text: 'No title given.' } },
        { draft: { title: 'Empty text', text: '' } },
        { hang: 1500 }, { draft: { title: 'After hang', text: 'Body after hang.' } },
        { draft: { title: 'Regenerated', text: 'Regenerated body.' } }
    ];
    const posted = (i) => ({ asin: `B0POSTED0${i}`, name: `Posted ${i}`, brand: 'B', current_price: '$5', features: 'f',
        is_reviewed: true, review_title: `Posted title ${i}`, review_text: `${'A long posted review body. '.repeat(6)}#${i}`, helpful_votes: i });
    const result = await both({
        mocks: async () => ({ gemini: await geminiMock(script) }),
        config: { geminiApiKey: ' test-key ', geminiDailyLimit: 16, geminiModel: 'gemini-test', geminiTemperature: 0.4, notificationsEnabled: true },
        env: (m) => ({ AOS_GEMINI_BASE_URL: m.gemini.url, AOS_GEMINI_WAIT_SCALE: '0.002', AOS_GEMINI_TIMEOUT_MS: '500' })
    }, async (s) => {
        await s.call('POST', '/api/sync', { order_id: '888-0', date: 'January 3, 2026', total: '$5', status: 'Delivered', ship_to: 'X', items: [posted(1), posted(2), posted(3), posted(4)] });
        const items = Array.from({ length: 11 }, (_, i) => ({ asin: `B0GEMINI${String(i).padStart(2, '0')}`, name: `Gadget ${i}`, brand: 'Acme',
            features: `- feature ${i}`, current_price: '$9', top_reviews: i === 0 ? ['Loud but good', 'Works'] : undefined }));
        await s.call('POST', '/api/sync', { order_id: '888-1', date: 'January 5, 2026', total: '$9', status: 'Delivered', ship_to: 'X', items });
        await s.call('POST', '/api/regenerate-review', { asin: 'B0GEMINI01', rating: 3, tone: 'enthusiastic', model: 'gemini-other' });
        await s.call('GET', '/api/gemini-usage');
        // The budget runs out: drafts fall back to the templates.
        await s.call('POST', '/api/sync', { order_id: '888-2', date: 'January 6, 2026', total: '$9', status: 'Delivered', ship_to: 'X',
            items: [{ asin: 'B0BUDGET01', name: 'Budget 1' }, { asin: 'B0BUDGET02', name: 'Budget 2' }, { asin: 'B0BUDGET03', name: 'Budget 3' }] });
        await s.call('POST', '/api/regenerate-review', { asin: 'B0BUDGET01', rating: 1 });
        await s.call('POST', '/api/sync', { order_id: '888-2', date: 'January 6, 2026', total: '$9', status: 'Delivered', ship_to: 'X',
            items: [{ asin: 'B0BUDGET01', name: 'Budget 1' }] }, {}, { label: 're-sync over template' });
        await s.call('GET', '/api/gemini-models');
        await s.call('GET', '/api/gemini-usage');
        await s.pinMtimes();
        await s.call('GET', '/api/orders');
        await s.call('GET', '/api/notifications?after=0');
    }, { compareLogs: false });
    // The regenerate call's shape and voice samples rotate on a random seed: compare it
    // without those two sections, and every sync-time prompt exactly (above).
    assert.ok(result.node.gemini.length > 10);
});

test('parity: headless runs with a fake browser', { skip: SKIP_HEADLESS }, async () => {
    const opts = {
        config: { autoSyncEnabled: true, autoSyncIntervalHours: 24, browserPath: FAKE },
        env: { AOS_FAKE_BROWSER: 'headless-only', AOS_STALL_WATCHDOG_MS: '1500', AOS_WAIT_TICK_MS: '600' }
    };
    await both(opts, async (s) => {
        const meta = async (label) => {
            const r = await s.call('GET', '/api/orders', undefined, {}, { record: false });
            const h = { ...r.json.headless };
            s.transcript.push(mask({ step: `meta ${label}`, headless: { ...h, browserBinary: h.browserBinary ? path.basename(h.browserBinary) : null } }, s));
            return h;
        };
        await meta('before');
        await s.call('POST', '/api/sync', ORDER_A);
        // A run the extension completes.
        let run = s.call('POST', '/api/sync-now', { mode: 'quick', maxOrders: 5, refresh: { prices: false, votes: 'no' } }, {}, { label: 'sync-now complete' });
        await waitFor(s, (h) => h.running);
        await s.call('POST', '/api/sync-now', undefined, {}, { label: 'second sync-now' });
        await s.call('POST', '/api/review-check-now', undefined, {}, { label: 'review-check-now while running' });
        await s.call('POST', '/api/headless-setup', undefined, {}, { label: 'setup while running' });
        await s.call('GET', '/api/run-options', undefined, {}, { label: 'run-options during run' });
        await s.call('POST', '/api/client-log', { lines: ['hello from the extension'] });
        await s.call('POST', '/api/headless-complete', { ok: true, processed: 3 });
        await run;
        await meta('after complete');
        await s.call('GET', '/api/run-options', undefined, {}, { label: 'run-options after' });
        // Stopped by the user.
        run = s.call('POST', '/api/sync-now', '', {}, { label: 'sync-now stopped' });
        await waitFor(s, (h) => h.running);
        await s.call('POST', '/api/sync', { ...ORDER_A, order_id: '111-1111111-2222222' }, {}, { label: 'order during run' });
        await s.call('POST', '/api/sync-stop');
        await run;
        // Nothing from the extension: the watchdog aborts it.
        await s.call('POST', '/api/sync-now', 'not json', {}, { label: 'sync-now stalled' });
        await meta('after stall');
        // A failed run with a login error blocks syncing until setup.
        run = s.call('POST', '/api/sync-now', undefined, {}, { label: 'sync-now login fail' });
        await waitFor(s, (h) => h.running);
        await s.call('POST', '/api/headless-complete', { ok: false, error: 'login-required: session expired' });
        await run;
        await s.call('POST', '/api/sync-now', undefined, {}, { label: 'sync-now blocked' });
        await s.call('POST', '/api/review-check-now', undefined, {}, { label: 'review-check-now blocked' });
        await s.call('POST', '/api/sync-history/clear', undefined, {}, { label: 'history clear keeps the block' });
        await meta('blocked');
        await s.call('POST', '/api/headless-setup');
        for (let i = 0; i < 50 && (await meta2(s)).setupOpen; i++) await sleep(100);
        await meta('after setup');
        // A review check over the queue.
        await s.call('GET', '/api/review-check-queue');
        run = s.call('POST', '/api/review-check-now', undefined, {}, { label: 'review-check-now' });
        await waitFor(s, (h) => h.running);
        await s.call('POST', '/api/review-check-result', { asin: 'B0DEMOAAA1', reviewed: false, helpful_votes: 4 });
        await s.call('POST', '/api/headless-complete', { ok: false });
        await run;
        await meta('end');
        await s.call('GET', '/api/notifications?after=0');
        await s.call('POST', '/api/sync-history/clear');
        await meta('cleared');
    }, { compareLogs: true });
});

test('parity: a browser that exits early, and its arguments', { skip: SKIP_HEADLESS }, async () => {
    const opts = {
        config: { autoSyncEnabled: false, browserPath: FAKE, notificationsEnabled: false },
        env: { AOS_FAKE_BROWSER: 'exit:3' }
    };
    await both(opts, async (s) => {
        await s.call('POST', '/api/sync-now', '{"mode":"catchup"}');
        const r = await s.call('GET', '/api/orders', undefined, {}, { record: false });
        s.transcript.push(mask({ step: 'meta', headless: { ...r.json.headless, browserBinary: path.basename(r.json.headless.browserBinary) } }, s));
        await s.call('GET', '/api/notifications?after=0');
    }, { compareLogs: true });

    // The command line each server gives the browser.
    const argv = {};
    for (const kind of ['node', 'rust']) {
        const dir = await fsp.mkdtemp(path.join(os.tmpdir(), `aos-argv-${kind}-`));
        const file = path.join(dir, 'argv.txt');
        const s = await startServer(kind, { config: { browserPath: FAKE }, env: { AOS_FAKE_BROWSER: 'sleep', AOS_FAKE_BROWSER_ARGV: file } });
        try {
            const run = s.call('POST', '/api/sync-now');
            for (let i = 0; i < 50; i++) { try { await fsp.access(file); break; } catch { await sleep(100); } }
            await sleep(200);
            await s.call('POST', '/api/sync-stop');
            await run;
            argv[kind] = (await fsp.readFile(file, 'utf8')).split('\n').map((l) => maskString(l, s).replace(/aos-autosync-\d+/, 'aos-autosync-<TS>'));
        } finally {
            await s.stop();
            await fsp.rm(dir, { recursive: true, force: true });
        }
    }
    assert.deepEqual(argv.rust, argv.node);
});

async function meta2(s) {
    return (await s.call('GET', '/api/orders', undefined, {}, { record: false })).json.headless;
}

async function waitFor(s, pred) {
    for (let i = 0; i < 100; i++) {
        if (pred(await meta2(s))) return;
        await sleep(50);
    }
    throw new Error(`${s.kind}: condition not reached`);
}
