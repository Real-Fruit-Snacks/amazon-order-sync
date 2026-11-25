// Test suite for the amazon-order-sync server.
//
// Runs the built program (amazon-order-sync serve) as a child process on an
// ephemeral port against a tmp notesPath (tmp config via AMAZON_SYNC_CONFIG,
// no Gemini key) and exercises the HTTP API end to end. The internals (note
// parsing, the store, Gemini, the headless runner) are tested in Rust:
// src/unit_tests.rs, run by cargo test. No network access is required: the
// Gemini path is never configured, item image URLs are empty (so the image
// download short-circuits), and the regenerate test uses the no-API-key
// fallback path.
//
// Top-level tests run sequentially and share the order synced in before().

import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import net from 'node:net';
import http from 'node:http';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';


// The headless-run tests drive a fake browser written as a shell script and
// use ps, sleep and notify-send stubs; on Windows they are skipped (CI runs
// them on Ubuntu). Everything else runs on both.
const POSIX_ONLY = process.platform === 'win32'
    ? 'needs POSIX test fixtures (shell-script fake browser, ps, sleep, notify-send)'
    : false;

// The server under test: the built program, the newer of target/release and target/debug
// (cargo build --release, or cargo build). AOS_SERVER_BIN names another one.
const EXE = process.platform === 'win32' ? '.exe' : '';
const SERVER_BIN = process.env.AOS_SERVER_BIN || ['release', 'debug']
    .map(p => fileURLToPath(new URL(`../target/${p}/amazon-order-sync${EXE}`, import.meta.url)))
    .filter(p => fs.existsSync(p))
    .sort((a, b) => fs.statSync(b).mtimeMs - fs.statSync(a).mtimeMs)[0];
if (!SERVER_BIN) throw new Error('Build the server first (cargo build --release), or set AOS_SERVER_BIN.');
const serverCommand = () => [SERVER_BIN, ['serve']];

// The extension's fixed ID (the "key" in browser-extension/manifest.json).
const EXTENSION_ID = 'cikcnmangaegpkglmcfgbfbmknjmdifh';

const ASIN_A = 'B0DEMOAAA1'; // synced with an imported posted review
const ASIN_B = 'B0DEMOBBB2'; // synced unreviewed, with top_reviews context
const ORDER_ID = '111-1111111-1111111';

let tmpDir;
let notesPath;
let noteFile;
let baseUrl;
let child;
let fakeBrowser;

function getFreePort() {
    return new Promise((resolve, reject) => {
        const srv = net.createServer();
        srv.once('error', reject);
        srv.listen(0, '127.0.0.1', () => {
            const { port } = srv.address();
            srv.close(() => resolve(port));
        });
    });
}

async function waitForServer(url, timeoutMs = 5000) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        try {
            const res = await fetch(`${url}/api/health`);
            if (res.ok) return;
        } catch { /* not up yet */ }
        if (Date.now() > deadline) throw new Error('server did not start in time');
        await new Promise(resolve => setTimeout(resolve, 100));
    }
}

async function api(pathname, options = {}) {
    const res = await fetch(`${baseUrl}${pathname}`, options);
    const text = await res.text();
    let body = null;
    try { body = JSON.parse(text); } catch { /* CSV etc. */ }
    return { status: res.status, headers: res.headers, body, text };
}

function postJson(payload, headers = {}) {
    return {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', ...headers },
        body: JSON.stringify(payload)
    };
}

before(async () => {
    tmpDir = await fsp.mkdtemp(path.join(os.tmpdir(), 'aos-test-'));
    notesPath = path.join(tmpDir, 'notes');
    noteFile = path.join(notesPath, 'Orders', '2026', '08-August', `Order-${ORDER_ID}.md`);

    // v1.5.0 (§15.9): fake "browser" used as browserPath by cases 19/20.
    fakeBrowser = path.join(tmpDir, 'fake-browser.sh');
    await fsp.writeFile(fakeBrowser, '#!/bin/sh\nsleep 60\n');
    await fsp.chmod(fakeBrowser, 0o755);

    const port = await getFreePort();
    baseUrl = `http://127.0.0.1:${port}`;
    const configPath = path.join(tmpDir, 'config.json');
    // v1.8.8 (§26.2): reviewCutoffDays 0 — the shared server's old seeded
    // orders (e.g. case 28's Jan-2020 unposted item) must stay queueable;
    // the cutoff itself is covered by case 70 with explicit configs.
    await fsp.writeFile(configPath, JSON.stringify({ notesPath, geminiApiKey: '', port, syncReminderDays: 3, reviewCutoffDays: 0 }, null, 2) + '\n');

    child = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: configPath,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    child.stderr.on('data', d => process.stderr.write(`[server] ${d}`));
    await waitForServer(baseUrl);

    // One order, two items: A with an imported posted review, B unreviewed
    // and carrying scraped top_reviews context.
    const res = await api('/api/sync', postJson({
        order_id: ORDER_ID,
        date: 'August 1, 2026',
        total: '$42.50',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [
            {
                asin: ASIN_A,
                name: 'USB-C Cable 2m',
                brand: 'Acme',
                current_price: '$12.50',
                link: 'https://www.amazon.com/dp/' + ASIN_A,
                features: '- Braided nylon\n- 100W',
                image: '',
                is_reviewed: true,
                review_title: 'Imported title',
                review_text: 'Imported body, with comma',
                review_rating: '5'
            },
            {
                asin: ASIN_B,
                name: 'Wireless Mouse, Ergonomic',
                brand: 'Acme',
                current_price: '$30.00',
                link: 'https://www.amazon.com/dp/' + ASIN_B,
                features: '- Silent clicks\n- 2.4 GHz',
                image: '',
                top_reviews: [
                    'First customer review body',
                    'Second customer review body',
                    '',
                    'Third customer review body'
                ]
            }
        ]
    }));
    assert.equal(res.status, 200, `sync failed: ${res.text}`);
    assert.deepEqual(res.body, { success: true });
});

after(async () => {
    if (child && !child.killed) {
        child.kill('SIGTERM');
        await Promise.race([
            new Promise(resolve => child.once('exit', resolve)),
            new Promise(resolve => setTimeout(resolve, 3000))
        ]);
        if (child.exitCode === null) child.kill('SIGKILL');
    }
    if (tmpDir) await fsp.rm(tmpDir, { recursive: true, force: true });
});



// Case 3: Sync -> orders -> get-review -> mark-posted roundtrip.
test('3. sync -> orders -> get-review -> mark-posted roundtrip', async () => {
    const health = await api('/api/health');
    assert.equal(health.body.ok, true);
    assert.equal(health.body.version, '3.0.3');

    const ordersRes = await api('/api/orders');
    assert.equal(ordersRes.status, 200);
    assert.equal(ordersRes.body.version, '3.0.3');
    assert.equal(ordersRes.body.orders.length, 1);
    const order = ordersRes.body.orders[0];
    assert.equal(order.orderId, ORDER_ID);
    assert.equal(order.items.length, 2);

    const itemA = order.items.find(i => i.asin === ASIN_A);
    const itemB = order.items.find(i => i.asin === ASIN_B);
    assert.equal(itemA.posted, true);
    assert.equal(itemA.reviewed, true);
    assert.equal(itemA.reviewText, 'Imported body, with comma');
    assert.equal(itemB.posted, false);
    assert.equal(itemB.hasDraft, false);

    // get-review finds the imported review for A, 404s for the placeholder B.
    const reviewA = await api(`/api/get-review?asin=${ASIN_A}`);
    assert.equal(reviewA.status, 200);
    assert.deepEqual(reviewA.body, { title: 'Imported title', text: 'Imported body, with comma', rating: '' });
    const reviewB = await api(`/api/get-review?asin=${ASIN_B}`);
    assert.equal(reviewB.status, 404);

    // mark-posted flips the checkbox both ways.
    const unmark = await api('/api/mark-posted', postJson({ asin: ASIN_A, posted: false }));
    assert.deepEqual(unmark.body, { success: true, posted: false });
    let orders = (await api('/api/orders')).body.orders;
    assert.equal(orders[0].items.find(i => i.asin === ASIN_A).posted, false);

    const remark = await api('/api/mark-posted', postJson({ asin: ASIN_A, posted: true }));
    assert.deepEqual(remark.body, { success: true, posted: true });
    orders = (await api('/api/orders')).body.orders;
    assert.equal(orders[0].items.find(i => i.asin === ASIN_A).posted, true);

    const missing = await api('/api/mark-posted', postJson({ asin: 'B0UNKNOWN0', posted: true }));
    assert.equal(missing.status, 404);

    // Sidecar review context persisted at sync time: max 3 non-empty entries.
    const sidecar = JSON.parse(await fsp.readFile(path.join(notesPath, '.review-context.json'), 'utf8'));
    assert.deepEqual(sidecar, {
        [ASIN_B]: [
            'First customer review body',
            'Second customer review body',
            'Third customer review body'
        ]
    });
});

// Case 4: save-review rewrites title/body, preserves a checked Posted box.
test('4. save-review rewrites the draft and preserves the checked box', async () => {
    const res = await api('/api/save-review', postJson({
        asin: ASIN_A,
        title: 'Edited title',
        text: 'Edited body text'
    }));
    assert.equal(res.status, 200);
    assert.deepEqual(res.body, { ok: true, item_name: 'USB-C Cable 2m' });

    const note = await fsp.readFile(noteFile, 'utf8');
    assert.ok(note.includes('**Title:** Edited title'));
    assert.ok(note.includes('**Review:** Edited body text'));
    assert.ok(!note.includes('Imported body, with comma'), 'old body should be replaced');

    // The checked checkbox of item A survives inside its own block.
    const blockA = note.split(/(?=^### )/m).find(b => b.includes(ASIN_A) && b.includes('#### 📝 My Review'));
    assert.ok(blockA.includes('- [x] Posted to Amazon?'), blockA);

    // Bad payloads and unknown ASINs.
    assert.equal((await api('/api/save-review', postJson({ asin: ASIN_A, title: 't' }))).status, 400);
    assert.equal((await api('/api/save-review', postJson({ asin: 'B0UNKNOWN0', title: 't', text: 'x' }))).status, 404);
});

// Case 5: regenerate-review with no API key -> fallback, requested rating written.
test('5. regenerate-review without an API key writes the fallback draft at the requested rating', async () => {
    const res = await api('/api/regenerate-review', postJson({ asin: ASIN_B, rating: 4 }));
    assert.equal(res.status, 200);
    assert.equal(res.body.ok, true);
    assert.equal(res.body.fallback, true);
    assert.equal(res.body.rating, 4);
    assert.ok(res.body.title.length > 0);
    assert.ok(res.body.text.length > 0);

    const note = await fsp.readFile(noteFile, 'utf8');
    const blockB = note.split(/(?=^### )/m).find(b => b.includes(ASIN_B) && b.includes('#### 📝 My Review'));
    assert.ok(blockB.includes('**Rating:** 4'), blockB);
    assert.ok(blockB.includes(`**Title:** ${res.body.title}`));
    assert.ok(!blockB.includes('Write your review for this item below'), 'placeholder should be gone');
    assert.ok(blockB.includes('- [ ] Posted to Amazon?'), 'unchecked box preserved');

    // Rating validation.
    assert.equal((await api('/api/regenerate-review', postJson({ asin: ASIN_B, rating: 9 }))).status, 400);
    assert.equal((await api('/api/regenerate-review', postJson({ asin: 'B0UNKNOWN0' }))).status, 404);

    // The new draft shows up in /api/orders with its rating.
    const orders = (await api('/api/orders')).body.orders;
    const itemB = orders[0].items.find(i => i.asin === ASIN_B);
    assert.equal(itemB.hasDraft, true);
    assert.equal(itemB.draftRating, '4');
    assert.equal(itemB.reviewText, res.body.text);
});

// Case 6: export.csv header + quoting + Content-Disposition; export.json parses.
test('6. export.csv and export.json', async () => {
    const csv = await api('/api/export.csv');
    assert.equal(csv.status, 200);
    assert.match(csv.headers.get('content-type'), /^text\/csv; charset=utf-8/);
    assert.match(
        csv.headers.get('content-disposition'),
        /^attachment; filename="amazon-orders-\d{4}-\d{2}-\d{2}\.csv"$/
    );
    const lines = csv.text.trimEnd().split('\n');
    assert.equal(
        lines[0],
        'order_id,order_date,item_name,brand,asin,price,url,rating,reviewed,posted,helpful_votes'
    );
    assert.equal(lines.length, 3); // header + 2 items
    assert.ok(csv.text.includes('"Wireless Mouse, Ergonomic"'), 'comma field must be quoted');
    assert.ok(csv.text.includes(`${ASIN_A}`));
    assert.ok(csv.text.includes('true')); // reviewed/posted booleans for item A

    const json = await api('/api/export.json');
    assert.equal(json.status, 200);
    assert.match(
        json.headers.get('content-disposition'),
        /^attachment; filename="amazon-orders-\d{4}-\d{2}-\d{2}\.json"$/
    );
    assert.equal(json.body.version, '3.0.3');
    assert.ok(json.body.exported_at);
    const exported = json.body.orders[0].items.find(i => i.asin === ASIN_A);
    assert.equal(exported.name, 'USB-C Cable 2m');
    assert.equal(exported.reviewTitle, 'Edited title');
    assert.equal(exported.reviewText, 'Edited body text');
});

// Case 7: POST with a disallowed Origin -> 403.
test('7. disallowed origin gets 403 before any processing', async () => {
    const res = await api('/api/save-review', postJson(
        { asin: ASIN_A, title: 'x', text: 'y' },
        { Origin: 'https://evil.com' }
    ));
    assert.equal(res.status, 403);
    assert.deepEqual(res.body, { error: 'Forbidden origin' });

    const syncRes = await api('/api/sync', postJson({ order_id: 'x' }, { Origin: 'https://evil.com' }));
    assert.equal(syncRes.status, 403);

    // v2.2.0: Amazon page origins are refused (any script on any Amazon
    // subdomain could otherwise drive the API); only the extension's own
    // origin, the dashboard and origin-less local tools get in.
    for (const origin of ['https://www.amazon.com', 'https://www.amazon.co.uk', 'https://sellercentral.amazon.com', 'chrome-extension://aaaabbbbccccddddeeeeffffgggghhhh']) {
        const no = await api('/api/health', { headers: { Origin: origin } });
        assert.equal(no.status, 403, origin);
    }
    const ext = `chrome-extension://${EXTENSION_ID}`;
    const ok = await api('/api/health', { headers: { Origin: ext } });
    assert.equal(ok.status, 200);
    assert.equal(ok.headers.get('access-control-allow-origin'), ext);
});

// Case 8: POST /api/sync with a >5MB body -> 413.
test('8. oversize sync body gets 413', async () => {
    const payload = JSON.stringify({
        order_id: '222-2222222-2222222',
        items: [],
        padding: 'x'.repeat(5 * 1024 * 1024 + 1024)
    });
    const res = await fetch(`${baseUrl}/api/sync`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: payload
    });
    assert.equal(res.status, 413);
    assert.deepEqual(await res.json(), { error: 'Payload too large' });
});

// Case 9: cache — after save-review, /api/orders shows the new text.
test('9. scan cache never serves stale data after a write', async () => {
    // Populate the cache, then write, then read again.
    await api('/api/orders');
    await api('/api/orders');

    const saved = await api('/api/save-review', postJson({
        asin: ASIN_A,
        title: 'Cache bust title',
        text: 'Cache bust body'
    }));
    assert.equal(saved.status, 200);

    const orders = (await api('/api/orders')).body.orders;
    const itemA = orders[0].items.find(i => i.asin === ASIN_A);
    assert.equal(itemA.reviewText, 'Cache bust body');
    assert.equal(itemA.reviewTitle, 'Cache bust title');
});


// ---------------------------------------------------------------------------
// v1.3.0 (spec §13.6)
// ---------------------------------------------------------------------------

// Realistic Amazon "Request My Data" order-history export fixture: a quoted
// product name with a comma, a $0.00 unit price, a quantity-2 EUR row,
// ISO and slash dates, and a junk row missing its ASIN.
const BACKFILL_CSV = [
    'Order ID,Order Date,Product Name,ASIN,Unit Price,Quantity,Total Owed,Order Status,Shipment Status,Shipping Address,Currency',
    '111-3333333-1111111,2024-05-14,"Echo Dot, 5th Gen",B0FREE001X,$0.00,1,$0.00,Delivered,Delivered,"123 Main St, Springfield",USD',
    '111-3333333-2222222,03/09/2024,Stainless Water Bottle,B0PAID002X,12.00,2,24.00,Shipped,,"456 Oak Ave, Berlin",EUR',
    '111-3333333-3333333,2024-06-01,Junk Row Missing Asin,,5.00,1,5.00,Delivered,Delivered,"789 Pine Rd, Rome",USD',
    ''
].join('\n');



// Case 13: import-backfill end-to-end, then dedup on re-import.
test('13. import-backfill creates notes and skips existing orders on re-import', async () => {
    // First import: raw text/csv single-file form.
    const first = await fetch(`${baseUrl}/api/import-backfill`, {
        method: 'POST',
        headers: { 'Content-Type': 'text/csv' },
        body: BACKFILL_CSV
    });
    assert.equal(first.status, 200);
    const firstBody = await first.json();
    assert.equal(firstBody.ok, true);
    assert.equal(firstBody.imported_orders, 2);
    assert.equal(firstBody.imported_items, 2);
    assert.equal(firstBody.skipped_existing, 0);
    assert.equal(firstBody.skipped_rows, 1); // junk row missing ASIN
    assert.deepEqual(firstBody.errors, []);

    // Notes land in the right year/month folders with backfill frontmatter.
    const freeNote = await fsp.readFile(
        path.join(notesPath, 'Orders', '2024', '05-May', 'Order-111-3333333-1111111.md'), 'utf8');
    assert.ok(freeNote.includes('backfill: true'));
    assert.ok(freeNote.includes('date: "Ordered on May 14, 2024"'));
    assert.ok(freeNote.includes('price: ["$0.00"]'));
    assert.ok(freeNote.includes('### Echo Dot, 5th Gen'));
    assert.ok(freeNote.includes('- Quantity: 1'));
    assert.ok(freeNote.includes('- Source: Amazon data export'));
    assert.ok(freeNote.includes('reviewed: [false]'));
    assert.ok(freeNote.includes('Write your review for this item below'));

    const eurNote = await fsp.readFile(
        path.join(notesPath, 'Orders', '2024', '03-March', 'Order-111-3333333-2222222.md'), 'utf8');
    assert.ok(eurNote.includes('date: "Ordered on March 9, 2024"'));
    assert.ok(eurNote.includes('price: ["EUR 24.00"]')); // unit 12.00 x qty 2
    assert.ok(eurNote.includes('- Quantity: 2'));
    assert.ok(eurNote.includes('status: "Shipped"'));
    assert.ok(eurNote.includes('ship_to: "456 Oak Ave"'));

    // Imported items surface via /api/orders with their stored prices
    // (v2.0.0: a $0.00 price is just a price — no vine flag anywhere).
    const orders = (await api('/api/orders')).body.orders;
    const freeItem = orders.flatMap(o => o.items).find(i => i.asin === 'B0FREE001X');
    const paidItem = orders.flatMap(o => o.items).find(i => i.asin === 'B0PAID002X');
    assert.equal(freeItem.price, '$0.00');
    assert.equal(paidItem.price, 'EUR 24.00');

    // Second identical import (JSON multi-file form): both orders skipped,
    // nothing rewritten.
    const second = await api('/api/import-backfill', postJson({
        files: [{ name: 'Retail.OrderHistory.1.csv', content: BACKFILL_CSV }]
    }));
    assert.equal(second.status, 200);
    assert.equal(second.body.imported_orders, 0);
    assert.equal(second.body.skipped_existing, 2);
    const freeNoteAfter = await fsp.readFile(
        path.join(notesPath, 'Orders', '2024', '05-May', 'Order-111-3333333-1111111.md'), 'utf8');
    assert.equal(freeNoteAfter, freeNote, 'existing note must never be overwritten');

    // Unparseable body -> 400.
    const bad = await api('/api/import-backfill', postJson({ nope: true }));
    assert.equal(bad.status, 400);
});

// Case 14: regenerate-review tone validation and echo.
test('14. regenerate-review validates tone and echoes it back', async () => {
    const bogus = await api('/api/regenerate-review', postJson({ asin: ASIN_B, tone: 'bogus' }));
    assert.equal(bogus.status, 400);

    const concise = await api('/api/regenerate-review', postJson({ asin: ASIN_B, tone: 'concise' }));
    assert.equal(concise.status, 200);
    assert.equal(concise.body.ok, true);
    assert.equal(concise.body.tone, 'concise');
    assert.equal(concise.body.fallback, true); // no API key in tests

    // Default is balanced.
    const def = await api('/api/regenerate-review', postJson({ asin: ASIN_B }));
    assert.equal(def.status, 200);
    assert.equal(def.body.tone, 'balanced');
});

// Case 15: helpful_votes roundtrip through sync, /api/orders, and export.csv.
test('15. helpful_votes persists, survives re-sync, and exports', async () => {
    const orderId = '111-4444444-1111111';
    const asin = 'B0VOTES001X';
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 2, 2026',
        total: '$9.99',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin,
            name: 'Voted Widget',
            brand: 'Acme',
            current_price: '$9.99',
            link: 'https://www.amazon.com/dp/' + asin,
            features: '- feature',
            image: '',
            helpful_votes: 7
        }]
    }));
    assert.equal(syncRes.status, 200);

    const note = await fsp.readFile(
        path.join(notesPath, 'Orders', '2026', '08-August', `Order-${orderId}.md`), 'utf8');
    assert.ok(note.includes('helpful_votes: [7]'), note);

    let item = (await api('/api/orders')).body.orders
        .flatMap(o => o.items).find(i => i.asin === asin);
    assert.equal(item.helpfulVotes, 7);

    // Re-sync omitting helpful_votes preserves the stored value (rating pattern).
    const resync = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 2, 2026',
        total: '$9.99',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin,
            name: 'Voted Widget',
            brand: 'Acme',
            current_price: '$9.99',
            link: 'https://www.amazon.com/dp/' + asin,
            features: '- feature',
            image: ''
        }]
    }));
    assert.equal(resync.status, 200);
    item = (await api('/api/orders')).body.orders
        .flatMap(o => o.items).find(i => i.asin === asin);
    assert.equal(item.helpfulVotes, 7, 're-sync without the field preserves it');

    // export.csv row ends with the ...,reviewed,posted,helpful_votes tail.
    const csv = await api('/api/export.csv');
    const row = csv.text.trimEnd().split('\n').find(l => l.includes(asin));
    assert.ok(row, 'csv row for the voted item');
    assert.ok(row.endsWith(',false,false,7'), row);
});

// ---------------------------------------------------------------------------
// v1.4.0 (spec §14.5)
// ---------------------------------------------------------------------------

// (Case 16 — vineSummary — removed in v2.0.0 with the Vine feature set; the
// case number is retired, not reused.)

// Case 17: tone persistence — regenerate writes it, save-review preserves it,
// untouched items default to ''.
test('17. tone persists through regenerate and save-review', async () => {
    const orderId = '111-7777777-1111111';
    const asin1 = 'B0TONE201X';
    const asin2 = 'B0TONE202X';
    const mkItem = (asin, name) => ({
        asin,
        name,
        brand: 'Acme',
        current_price: '$9.99',
        link: 'https://www.amazon.com/dp/' + asin,
        features: '- feature',
        image: ''
    });
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 4, 2026',
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [mkItem(asin1, 'Tone Widget One'), mkItem(asin2, 'Tone Widget Two')]
    }));
    assert.equal(syncRes.status, 200);

    const notePath = path.join(notesPath, 'Orders', '2026', '08-August', `Order-${orderId}.md`);
    let note = await fsp.readFile(notePath, 'utf8');
    assert.ok(!note.includes('tone:'), 'fresh sync must not invent a tone line');

    const regen = await api('/api/regenerate-review', postJson({ asin: asin1, tone: 'concise' }));
    assert.equal(regen.status, 200);
    assert.equal(regen.body.tone, 'concise');

    note = await fsp.readFile(notePath, 'utf8');
    assert.ok(note.includes('tone: ["concise", ""]'), note);

    let items = (await api('/api/orders')).body.orders
        .flatMap(o => o.items);
    assert.equal(items.find(i => i.asin === asin1).tone, 'concise');
    assert.equal(items.find(i => i.asin === asin2).tone, '', 'untouched item has no tone');

    // save-review rewrites the draft but preserves the tone frontmatter.
    const saved = await api('/api/save-review', postJson({
        asin: asin1,
        title: 'Kept tone title',
        text: 'Kept tone body'
    }));
    assert.equal(saved.status, 200);
    note = await fsp.readFile(notePath, 'utf8');
    assert.ok(note.includes('tone: ["concise", ""]'), note);
    items = (await api('/api/orders')).body.orders.flatMap(o => o.items);
    assert.equal(items.find(i => i.asin === asin1).tone, 'concise', 'save-review preserves tone');

    // Re-sync of the same order preserves the tone (aligned-index pattern).
    const resync = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 4, 2026',
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [mkItem(asin1, 'Tone Widget One'), mkItem(asin2, 'Tone Widget Two')]
    }));
    assert.equal(resync.status, 200);
    note = await fsp.readFile(notePath, 'utf8');
    assert.ok(note.includes('tone: ["concise", ""]'), note);

    // export.json carries tone too (CSV is unchanged by spec).
    const exported = (await api('/api/export.json')).body.orders
        .flatMap(o => o.items);
    assert.equal(exported.find(i => i.asin === asin1).tone, 'concise');
    assert.equal(exported.find(i => i.asin === asin2).tone, '');
});

// Case 18: lastSyncAt + syncReminderDays meta.
test('18. lastSyncAt is present, bumps on re-sync, syncReminderDays echoed', async () => {
    const first = (await api('/api/orders')).body;
    assert.equal(typeof first.lastSyncAt, 'number');
    assert.ok(first.lastSyncAt > 0, 'non-null after syncs');
    assert.equal(first.syncReminderDays, 3, 'config value echoed');

    // A second sync later bumps (or at least never lowers) lastSyncAt. Bump
    // mtimes explicitly so a same-millisecond write can't flake the check.
    await new Promise(resolve => setTimeout(resolve, 25));
    const resync = await api('/api/sync', postJson({
        order_id: ORDER_ID,
        date: 'August 1, 2026',
        total: '$42.50',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin: ASIN_A,
            name: 'USB-C Cable 2m',
            brand: 'Acme',
            current_price: '$12.50',
            link: 'https://www.amazon.com/dp/' + ASIN_A,
            features: '- Braided nylon\n- 100W',
            image: ''
        }]
    }));
    assert.equal(resync.status, 200);

    const second = (await api('/api/orders')).body;
    assert.equal(typeof second.lastSyncAt, 'number');
    assert.ok(second.lastSyncAt >= first.lastSyncAt,
        `${second.lastSyncAt} >= ${first.lastSyncAt}`);
    assert.equal(second.syncReminderDays, 3);
});

// ---------------------------------------------------------------------------
// v1.5.0 (spec §15.9)
// ---------------------------------------------------------------------------


// Case 20: end-to-end headless run with the fake browser against a dedicated
// second server instance (its own port + notesPath + config).
test('20. headless run: sync-now -> 409 while running -> complete ping resolves', { skip: POSIX_ONLY }, async (t) => {
    const notes2 = path.join(tmpDir, 'notes-headless');
    const port2 = await getFreePort();
    const base2 = `http://127.0.0.1:${port2}`;
    const config2 = path.join(tmpDir, 'config-headless.json');
    await fsp.writeFile(config2, JSON.stringify({
        notesPath: notes2,
        geminiApiKey: '',
        port: port2,
        autoSyncEnabled: true,
        autoSyncIntervalHours: 24,
        browserPath: fakeBrowser
    }, null, 2) + '\n');

    const child2 = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: config2,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg2')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    child2.stderr.on('data', d => process.stderr.write(`[server2] ${d}`));
    t.after(async () => {
        if (!child2.killed) {
            child2.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => child2.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (child2.exitCode === null) child2.kill('SIGKILL');
        }
    });

    const api2 = async (pathname, options = {}) => {
        const res = await fetch(`${base2}${pathname}`, options);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch { /* non-JSON */ }
        return { status: res.status, body, text };
    };
    const headlessMeta = async () => (await api2('/api/orders')).body.headless;
    const fakeBrowserPids = async () => {
        const { execFile: execFileCb } = await import('node:child_process');
        const stdout = await new Promise((resolve, reject) => {
            execFileCb('ps', ['-eo', 'pid=,args='], (err, out) => err ? reject(err) : resolve(out));
        });
        return stdout.split('\n')
            .filter(line => line.includes(fakeBrowser))
            .map(line => parseInt(line.trim(), 10));
    };

    await waitForServer(base2);

    // Meta baseline: enabled, browser found, never run.
    const before_ = await headlessMeta();
    assert.equal(before_.enabled, true);
    assert.equal(before_.browserFound, true);
    assert.equal(before_.running, false);
    assert.equal(before_.intervalHours, 24);
    assert.equal(before_.lastRunAt, null);

    // Fire a run (resolves only when the run finishes — do not await yet).
    const syncNowPromise = api2('/api/sync-now', { method: 'POST' });

    // Wait until the run is in-flight, then a second sync-now must 409.
    let meta = null;
    for (let i = 0; i < 50; i++) {
        meta = await headlessMeta();
        if (meta.running) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.equal(meta.running, true, 'run should be in progress');
    const second = await api2('/api/sync-now', { method: 'POST' });
    assert.equal(second.status, 409);

    // The fake browser process exists while the run is in progress.
    const pidsDuring = await fakeBrowserPids();
    assert.ok(pidsDuring.length > 0, 'fake browser should be running');

    // The extension's completion ping resolves the pending run.
    const ping = await api2('/api/headless-complete', postJson({ ok: true, processed: 3 }));
    assert.equal(ping.status, 200);
    assert.deepEqual(ping.body, { ok: true });

    const syncNow = await syncNowPromise;
    assert.equal(syncNow.status, 200);
    assert.deepEqual(syncNow.body, {
        ok: true,
        result: { lastResult: 'ok', lastError: null, ordersSynced: 3 }
    });

    // State file persisted with the result.
    const state = JSON.parse(await fsp.readFile(path.join(notes2, '.headless-sync.json'), 'utf8'));
    assert.equal(state.lastResult, 'ok');
    assert.equal(state.ordersSynced, 3);
    assert.equal(state.lastError, null);
    assert.equal(typeof state.lastRunAt, 'number');

    // Meta reflects the finished run.
    meta = await headlessMeta();
    assert.equal(meta.running, false);
    assert.equal(meta.lastResult, 'ok');
    assert.equal(meta.ordersSynced, 3);
    assert.equal(typeof meta.lastRunAt, 'number');

    // Process hygiene: the fake browser (and its sleep child) is reaped
    // (SIGTERM immediately, SIGKILL backstop after 5 s).
    let pidsAfter = await fakeBrowserPids();
    for (let i = 0; i < 80 && pidsAfter.length > 0; i++) {
        await new Promise(resolve => setTimeout(resolve, 100));
        pidsAfter = await fakeBrowserPids();
    }
    assert.deepEqual(pidsAfter, [], 'fake browser process must be reaped');
});

// Case 21: 409 discipline — headless-complete with no run in progress.
test('21. headless-complete with no run in progress returns 409', async () => {
    const res = await api('/api/headless-complete', postJson({ ok: true, processed: 1 }));
    assert.equal(res.status, 409);
    assert.deepEqual(res.body, { error: 'no run in progress' });
});


// ---------------------------------------------------------------------------
// v1.6.0 (spec §16.9)
// ---------------------------------------------------------------------------


// Case 23: price watch — listing_price below paid records history and
// produces a priceDrops row; above paid removes the drop.
test('23. price watch: listing_price upserts history and drives priceDrops', async () => {
    const orderId = '111-8888888-1111111';
    const asin = 'B0PRICE01X';
    const mkSync = (listing) => api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 5, 2026',
        total: '$24.99',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin,
            name: 'Price Watch Widget',
            brand: 'Acme',
            current_price: '$24.99',
            listing_price: listing,
            link: 'https://www.amazon.com/dp/' + asin,
            features: '- feature',
            image: ''
        }]
    }));

    const first = await mkSync('$19.99');
    assert.equal(first.status, 200, first.text);

    const hist = JSON.parse(await fsp.readFile(path.join(notesPath, '.price-history.json'), 'utf8'));
    assert.equal(hist[asin].paid, 24.99);
    assert.equal(hist[asin].history.length, 1);
    assert.equal(hist[asin].history[0].price, 19.99);
    assert.equal(typeof hist[asin].history[0].t, 'number');

    let body = (await api('/api/orders')).body;
    let item = body.orders.flatMap(o => o.items).find(i => i.asin === asin);
    assert.equal(item.listingPrice, 19.99);
    const drop = body.priceDrops.find(d => d.asin === asin);
    assert.ok(drop, 'priceDrops row expected');
    assert.equal(drop.name, 'Price Watch Widget');
    assert.equal(drop.paid, 24.99);
    assert.equal(drop.listing, 19.99);
    assert.equal(drop.savings, 5);
    assert.equal(drop.pct, 20);

    // Re-sync above paid: same-day collapse keeps one entry, drop is gone.
    const second = await mkSync('$29.99');
    assert.equal(second.status, 200, second.text);
    const hist2 = JSON.parse(await fsp.readFile(path.join(notesPath, '.price-history.json'), 'utf8'));
    assert.equal(hist2[asin].history.length, 1, 'same-day entries collapse');
    assert.equal(hist2[asin].history[0].price, 29.99);
    body = (await api('/api/orders')).body;
    item = body.orders.flatMap(o => o.items).find(i => i.asin === asin);
    assert.equal(item.listingPrice, 29.99);
    assert.ok(!body.priceDrops.some(d => d.asin === asin), 'no drop when listing > paid');
});


// Case 25: the extension origin passes preflight and requests; the
// manifest's key really produces the ID the server trusts.
test('25. the extension origin is allowed, and matches manifest.json', async () => {
    const ext = `chrome-extension://${EXTENSION_ID}`;
    const preflight = await fetch(`${baseUrl}/api/sync`, {
        method: 'OPTIONS',
        headers: { Origin: ext, 'Access-Control-Request-Method': 'POST' }
    });
    assert.equal(preflight.status, 204);
    const manifest = JSON.parse(await fsp.readFile(new URL('../browser-extension/manifest.json', import.meta.url), 'utf8'));
    const digest = crypto.createHash('sha256').update(Buffer.from(manifest.key, 'base64')).digest('hex').slice(0, 32);
    const id = [...digest].map(c => String.fromCharCode(97 + parseInt(c, 16))).join('');
    assert.equal(id, EXTENSION_ID);
});

// Case 25b: DNS-rebinding guard — a Host other than 127.0.0.1/localhost on
// the bound port is refused even without an Origin header.
test('25b. requests naming another Host are refused', async () => {
    const port = Number(new URL(baseUrl).port);
    const get = host => new Promise((resolve, reject) => {
        const req = http.request({ host: '127.0.0.1', port, path: '/api/health', headers: { Host: host } }, res => {
            res.resume(); res.on('end', () => resolve(res.statusCode));
        });
        req.on('error', reject); req.end();
    });
    assert.equal(await get(`rebind.example:${port}`), 403);
    assert.equal(await get(`localhost:${port + 1}`), 403);
    assert.equal(await get(`localhost:${port}`), 200);
    assert.equal(await get(`127.0.0.1:${port}`), 200);
});

// Case 26: repeatCount — the same ASIN in two orders flags both items.
test('26. repeatCount counts distinct orders per ASIN', async () => {
    const asin = 'B0REPEAT1X';
    const mkItem = () => ({
        asin,
        name: 'Repeat Widget',
        brand: 'Acme',
        current_price: '$9.99',
        link: 'https://www.amazon.com/dp/' + asin,
        features: '- feature',
        image: ''
    });
    for (const orderId of ['111-9999999-1111111', '111-9999999-2222222']) {
        const res = await api('/api/sync', postJson({
            order_id: orderId,
            date: 'August 6, 2026',
            total: '$9.99',
            status: 'Delivered',
            ship_to: 'Tester',
            items: [mkItem()]
        }));
        assert.equal(res.status, 200, res.text);
    }

    const orders = (await api('/api/orders')).body.orders;
    const repeats = orders.flatMap(o => o.items).filter(i => i.asin === asin);
    assert.equal(repeats.length, 2);
    for (const item of repeats) assert.equal(item.repeatCount, 2);
    // An ASIN living in a single order gets repeatCount 1.
    const single = orders.flatMap(o => o.items).find(i => i.asin === 'B0VOTES001X');
    assert.equal(single.repeatCount, 1);
});


// Case 28: review-check — queue contents, real-review result post, votes-only
// result post, vote timestamps, and 400 discipline.
test('28. review-check queue + result posts update notes and vote timestamps', async () => {
    const orderId = '111-5555555-1111111';
    const postedAsin = 'B0RCPOST1X';
    const unpostedAsin = 'B0RCUNP01X';
    // An old order (>2 days): one posted item with never-refreshed votes
    // (stale) and one unposted item.
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'January 5, 2020',
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [
            {
                asin: postedAsin,
                name: 'Review Check Posted Widget',
                brand: 'Acme',
                current_price: '$9.99',
                link: 'https://www.amazon.com/dp/' + postedAsin,
                features: '- feature',
                image: '',
                is_reviewed: true,
                review_title: 'Old posted title',
                review_text: 'Old posted body',
                review_rating: '5'
            },
            {
                asin: unpostedAsin,
                name: 'Review Check Unposted Widget',
                brand: 'Acme',
                current_price: '$9.99',
                link: 'https://www.amazon.com/dp/' + unpostedAsin,
                features: '- feature',
                image: ''
            }
        ]
    }));
    assert.equal(syncRes.status, 200, syncRes.text);
    const notePath = path.join(notesPath, 'Orders', '2020', '01-January', `Order-${orderId}.md`);

    // Queue: both seeds present, capped at 20.
    const queue = await api('/api/review-check-queue');
    assert.equal(queue.status, 200);
    assert.ok(Array.isArray(queue.body.asins));
    assert.ok(queue.body.asins.length <= 20);
    assert.ok(queue.body.asins.includes(postedAsin), 'posted item with stale votes queued');
    assert.ok(queue.body.asins.includes(unpostedAsin), 'unposted item from an old order queued');

    // Real review -> note block written, checkbox posted, votes set.
    const reviewed = await api('/api/review-check-result', postJson({
        asin: postedAsin,
        reviewed: true,
        title: 'Live title from review-check',
        text: 'Live body from review-check',
        rating: 4,
        helpful_votes: 3
    }));
    assert.equal(reviewed.status, 200, reviewed.text);
    assert.deepEqual(reviewed.body, { ok: true });
    let note = await fsp.readFile(notePath, 'utf8');
    const postedBlock = note.split(/(?=^### )/m).find(b => b.includes(postedAsin) && b.includes('#### 📝 My Review'));
    assert.ok(postedBlock.includes('**Title:** Live title from review-check'), postedBlock);
    assert.ok(postedBlock.includes('**Review:** Live body from review-check'), postedBlock);
    assert.ok(postedBlock.includes('**Rating:** 4'), postedBlock);
    assert.ok(postedBlock.includes('- [x] Posted to Amazon?'), postedBlock);
    assert.ok(note.includes('helpful_votes: ["3", ""'), note);

    // Vote timestamps bumped and persisted.
    const state = JSON.parse(await fsp.readFile(path.join(notesPath, '.headless-sync.json'), 'utf8'));
    assert.equal(typeof state.voteTimestamps[postedAsin], 'number');

    // The freshly checked posted item drops out of the queue.
    const queue2 = await api('/api/review-check-queue');
    assert.ok(!queue2.body.asins.includes(postedAsin), 'fresh votes are not stale');

    // Votes-only post -> votes updated, no review block written.
    const votesOnly = await api('/api/review-check-result', postJson({
        asin: unpostedAsin,
        reviewed: false,
        helpful_votes: 5
    }));
    assert.equal(votesOnly.status, 200, votesOnly.text);
    note = await fsp.readFile(notePath, 'utf8');
    const unpostedBlock = note.split(/(?=^### )/m).find(b => b.includes(unpostedAsin) && b.includes('#### 📝 My Review'));
    assert.ok(unpostedBlock.includes('Write your review for this item below'), 'no review block written');
    assert.ok(unpostedBlock.includes('- [ ] Posted to Amazon?'), 'checkbox untouched');
    assert.ok(note.includes('helpful_votes: ["3", "5"]'), note);
    const state2 = JSON.parse(await fsp.readFile(path.join(notesPath, '.headless-sync.json'), 'utf8'));
    assert.equal(typeof state2.voteTimestamps[unpostedAsin], 'number');

    // 400 discipline.
    assert.equal((await api('/api/review-check-result', postJson({
        asin: postedAsin, reviewed: true, title: 'x' // missing text
    }))).status, 400);
    assert.equal((await api('/api/review-check-result', postJson({
        asin: postedAsin, reviewed: false, helpful_votes: -1
    }))).status, 400);
    assert.equal((await api('/api/review-check-result', postJson({
        asin: '../evil', reviewed: false
    }))).status, 400);
    assert.equal((await api('/api/review-check-result', postJson({
        asin: 'B0UNKNOWN0', reviewed: false
    }))).status, 404);
});

// (Case 29 — Vine escalation — removed in v2.0.0 with the Vine feature set;
// the case number is retired, not reused.)

// Case 30: review-check-now with an empty queue resolves immediately without
// spawning a browser; 409 while any headless run is active.
test('30. review-check-now: empty queue is instant, 409 while running', { skip: POSIX_ONLY }, async (t) => {
    const notes3 = path.join(tmpDir, 'notes-reviewcheck');
    const port3 = await getFreePort();
    const base3 = `http://127.0.0.1:${port3}`;
    const config3 = path.join(tmpDir, 'config-reviewcheck.json');
    await fsp.writeFile(config3, JSON.stringify({
        notesPath: notes3,
        geminiApiKey: '',
        port: port3,
        autoSyncEnabled: true,
        browserPath: fakeBrowser
    }, null, 2) + '\n');

    const child3 = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: config3,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg3')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    child3.stderr.on('data', d => process.stderr.write(`[server3] ${d}`));
    t.after(async () => {
        if (!child3.killed) {
            child3.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => child3.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (child3.exitCode === null) child3.kill('SIGKILL');
        }
    });

    const api3 = async (pathname, options = {}) => {
        const res = await fetch(`${base3}${pathname}`, options);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch { /* non-JSON */ }
        return { status: res.status, body, text };
    };
    const fakeBrowserPids = async () => {
        const { execFile: execFileCb } = await import('node:child_process');
        const stdout = await new Promise((resolve, reject) => {
            execFileCb('ps', ['-eo', 'pid=,args='], (err, out) => err ? reject(err) : resolve(out));
        });
        return stdout.split('\n')
            .filter(line => line.includes(fakeBrowser))
            .map(line => parseInt(line.trim(), 10));
    };

    await waitForServer(base3);

    // Empty notes -> empty queue -> immediate ok, no browser spawned.
    const started = Date.now();
    const empty = await api3('/api/review-check-now', { method: 'POST' });
    const elapsed = Date.now() - started;
    assert.equal(empty.status, 200, empty.text);
    assert.deepEqual(empty.body, { ok: true, result: { lastResult: 'ok', ordersSynced: 0 } });
    assert.ok(elapsed < 5000, `empty queue must not spawn a browser (took ${elapsed}ms)`);
    assert.deepEqual(await fakeBrowserPids(), [], 'no browser spawned for an empty queue');

    // 409 discipline: while a sync run is active, review-check-now conflicts.
    const syncNowPromise = api3('/api/sync-now', { method: 'POST' });
    for (let i = 0; i < 50; i++) {
        const meta = (await api3('/api/orders')).body.headless;
        if (meta.running) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    const conflict = await api3('/api/review-check-now', { method: 'POST' });
    assert.equal(conflict.status, 409);
    const ping = await api3('/api/headless-complete', postJson({ ok: true, processed: 1 }));
    assert.equal(ping.status, 200);
    const syncNow = await syncNowPromise;
    assert.equal(syncNow.status, 200);

    // lastReviewCheckAt surfaces in meta; due-predicate math sanity.
    const meta = (await api3('/api/orders')).body.headless;
    assert.ok('lastReviewCheckAt' in meta);
});


// ---------------------------------------------------------------------------
// v1.6.1 (spec §17.4)
// ---------------------------------------------------------------------------

// Case 32: a fake-browser run in flight is aborted by POST /api/sync-stop;
// the sync-now caller resolves cancelled, the browser is reaped, history
// records the cancellation, and a second stop is 409.
test('32. sync-stop aborts a running headless sync as cancelled', { skip: POSIX_ONLY }, async (t) => {
    const notes4 = path.join(tmpDir, 'notes-stop');
    const port4 = await getFreePort();
    const base4 = `http://127.0.0.1:${port4}`;
    const config4 = path.join(tmpDir, 'config-stop.json');
    await fsp.writeFile(config4, JSON.stringify({
        notesPath: notes4,
        geminiApiKey: '',
        port: port4,
        autoSyncEnabled: true,
        autoSyncIntervalHours: 24,
        browserPath: fakeBrowser
    }, null, 2) + '\n');

    const child4 = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: config4,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg4')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    child4.stderr.on('data', d => process.stderr.write(`[server4] ${d}`));
    t.after(async () => {
        if (!child4.killed) {
            child4.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => child4.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (child4.exitCode === null) child4.kill('SIGKILL');
        }
    });

    const api4 = async (pathname, options = {}) => {
        const res = await fetch(`${base4}${pathname}`, options);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch { /* non-JSON */ }
        return { status: res.status, body, text };
    };
    const headlessMeta = async () => (await api4('/api/orders')).body.headless;
    const fakeBrowserPids = async () => {
        const { execFile: execFileCb } = await import('node:child_process');
        const stdout = await new Promise((resolve, reject) => {
            execFileCb('ps', ['-eo', 'pid=,args='], (err, out) => err ? reject(err) : resolve(out));
        });
        return stdout.split('\n')
            .filter(line => line.includes(fakeBrowser))
            .map(line => parseInt(line.trim(), 10));
    };

    await waitForServer(base4);

    // Fire a run (resolves only when the run finishes — do not await yet).
    const syncNowPromise = api4('/api/sync-now', { method: 'POST' });

    // Wait until the run is in flight.
    let meta = null;
    for (let i = 0; i < 50; i++) {
        meta = await headlessMeta();
        if (meta.running) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.equal(meta.running, true, 'run should be in progress');
    assert.ok((await fakeBrowserPids()).length > 0, 'fake browser should be running');

    // Abort it: 200 {ok:true}.
    const stop = await api4('/api/sync-stop', { method: 'POST' });
    assert.equal(stop.status, 200);
    assert.deepEqual(stop.body, { ok: true });

    // The pending sync-now caller resolves with the cancelled result.
    const syncNow = await syncNowPromise;
    assert.equal(syncNow.status, 200);
    assert.deepEqual(syncNow.body, {
        ok: true,
        result: { lastResult: 'cancelled', lastError: null, ordersSynced: 0 }
    });

    // Fake browser reaped (SIGTERM immediately, SIGKILL backstop after 5 s).
    let pidsAfter = await fakeBrowserPids();
    for (let i = 0; i < 80 && pidsAfter.length > 0; i++) {
        await new Promise(resolve => setTimeout(resolve, 100));
        pidsAfter = await fakeBrowserPids();
    }
    assert.deepEqual(pidsAfter, [], 'fake browser process must be reaped');

    // Persisted state + history tail record the cancellation.
    const state = JSON.parse(await fsp.readFile(path.join(notes4, '.headless-sync.json'), 'utf8'));
    assert.equal(state.lastResult, 'cancelled');
    assert.equal(state.lastError, null);
    assert.equal(state.ordersSynced, 0);
    assert.equal(state.history[0].result, 'cancelled');
    meta = await headlessMeta();
    assert.equal(meta.running, false);
    assert.equal(meta.lastResult, 'cancelled');
    assert.equal(meta.history[0].result, 'cancelled');

    // Second stop while idle: 409.
    const second = await api4('/api/sync-stop', { method: 'POST' });
    assert.equal(second.status, 409);
    assert.deepEqual(second.body, { error: 'no run in progress' });
});

// ---------------------------------------------------------------------------
// v1.7.0 (spec §18.7)
// ---------------------------------------------------------------------------

// Shared helper: dedicated server with the fake browser as browserPath.
async function startFakeBrowserServer(t, name, configExtra = {}, envExtra = {}) {
    const notes = path.join(tmpDir, `notes-${name}`);
    const port = await getFreePort();
    const base = `http://127.0.0.1:${port}`;
    const configPath = path.join(tmpDir, `config-${name}.json`);
    await fsp.writeFile(configPath, JSON.stringify({
        notesPath: notes,
        geminiApiKey: '',
        port,
        autoSyncEnabled: true,
        autoSyncIntervalHours: 24,
        browserPath: fakeBrowser,
        ...configExtra
    }, null, 2) + '\n');

    const proc = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: configPath,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, `xdg-${name}`),
            ...envExtra
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    proc.stderr.on('data', d => process.stderr.write(`[${name}] ${d}`));
    t.after(async () => {
        if (!proc.killed) {
            proc.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => proc.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (proc.exitCode === null) proc.kill('SIGKILL');
        }
    });
    await waitForServer(base);

    const apiN = async (pathname, options = {}) => {
        const res = await fetch(`${base}${pathname}`, options);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch { /* non-JSON */ }
        return { status: res.status, body, text };
    };
    const headlessMeta = async () => (await apiN('/api/orders')).body.headless;
    const waitRunning = async () => {
        for (let i = 0; i < 50; i++) {
            const meta = await headlessMeta();
            if (meta.running) return meta;
            await new Promise(resolve => setTimeout(resolve, 100));
        }
        throw new Error('run did not start in time');
    };
    return { notes, api: apiN, headlessMeta, waitRunning, proc };
}

// Local-time YYYY-MM-DD, mirroring the server's quota day key.
function todayKey(ms = Date.now()) {
    const d = new Date(ms);
    const p = n => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

const FULL_DEFAULT_OPTIONS = { mode: 'full', maxOrders: 0, timeBudgetSec: 0, refresh: { prices: true, votes: true, details: true } };

// Case 33 (§18.7): run-options roundtrip — echo during the run, invalid
// values fall back per-field, headless.lastOptions after.
test('33. run-options: sync-now body echoes during run, invalid fields fall back, lastOptions persists', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'runopts');

    // Before any run: full defaults.
    const initial = await srv.api('/api/run-options');
    assert.equal(initial.status, 200);
    assert.deepEqual(initial.body.options, FULL_DEFAULT_OPTIONS);

    // A full options body is echoed verbatim while the run is in flight.
    const wanted = { mode: 'quick', maxOrders: 3, timeBudgetSec: 120, refresh: { prices: false, votes: true, details: false } };
    const run1Promise = srv.api('/api/sync-now', postJson(wanted));
    await srv.waitRunning();
    const during = await srv.api('/api/run-options');
    assert.deepEqual(during.body.options, wanted);
    const ping1 = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 2 }));
    assert.equal(ping1.status, 200);
    const run1 = await run1Promise;
    assert.equal(run1.body.result.lastResult, 'ok');

    // headless.lastOptions echoes the last effective options.
    const meta1 = await srv.headlessMeta();
    assert.deepEqual(meta1.lastOptions, wanted);

    // Invalid fields fall back to defaults individually (votes:false is
    // valid and survives; the rest are junk).
    const run2Promise = srv.api('/api/sync-now', postJson({
        mode: 'turbo', maxOrders: -1, timeBudgetSec: '90', refresh: { prices: 'no', votes: false }
    }));
    await srv.waitRunning();
    const during2 = await srv.api('/api/run-options');
    assert.deepEqual(during2.body.options, {
        mode: 'full', maxOrders: 0, timeBudgetSec: 0, refresh: { prices: true, votes: false, details: true }
    });

    // Finish with an error to also exercise the guidance wiring (§18.4).
    const ping2 = await srv.api('/api/headless-complete', postJson({ ok: false, error: 'login-required' }));
    assert.equal(ping2.status, 200);
    const run2 = await run2Promise;
    assert.equal(run2.body.result.lastResult, 'error');
    assert.equal(run2.body.result.lastError, 'login-required');
    assert.equal(run2.body.result.guidance.message, 'Your Amazon session expired.');
    assert.ok(run2.body.result.guidance.steps.length > 0);

    const meta2 = await srv.headlessMeta();
    assert.equal(meta2.guidance.message, 'Your Amazon session expired.');
    assert.deepEqual(meta2.lastOptions, {
        mode: 'full', maxOrders: 0, timeBudgetSec: 0, refresh: { prices: true, votes: false, details: true }
    });
});

// Case 34 (§18.7): log buffer — monotonic indices, cursor returns only newer.
test('34. logs: ring buffer with monotonic i and after-cursor', async () => {
    const first = await api('/api/logs');
    assert.equal(first.status, 200);
    assert.ok(Array.isArray(first.body.lines));
    assert.ok(first.body.lines.length > 0, 'startup lines should be buffered');
    assert.ok(first.body.lines.length <= 100, 'no cursor returns at most the last 100');
    for (const line of first.body.lines) {
        assert.equal(typeof line.i, 'number');
        assert.equal(typeof line.t, 'number');
        assert.equal(typeof line.text, 'string');
    }
    for (let k = 1; k < first.body.lines.length; k++) {
        assert.ok(first.body.lines[k].i > first.body.lines[k - 1].i, 'indices must be monotonic');
    }
    const next = first.body.next;
    assert.equal(next, first.body.lines[first.body.lines.length - 1].i);

    // Cursor at the latest index: nothing newer (yet).
    const caughtUp = await api(`/api/logs?after=${next}`);
    assert.equal(caughtUp.status, 200);
    assert.equal(caughtUp.body.next, next);
    assert.ok(caughtUp.body.lines.every(l => l.i > next));

    // Force a daemon log line and confirm the cursor surfaces exactly the
    // new activity. (v1.8.3 §23.5 S-h: malformed /api/sync JSON is now a
    // 400 WITHOUT an error log, so a /api/client-log line is the source.)
    await api('/api/client-log', postJson({ lines: ['cursor-test activity'] }));
    const newer = await api(`/api/logs?after=${next}`);
    assert.ok(newer.body.lines.length > 0, 'new activity must appear after the cursor');
    assert.ok(newer.body.lines.every(l => l.i > next));
    assert.ok(newer.body.next > next);
    assert.ok(newer.body.lines.some(l => l.text.includes('[ext] cursor-test activity')));
});


// Case 36 (§18.7): /api/orders meta exposes gemini {used, limit, remaining,
// day}; config geminiDailyLimit is honored.
test('36. meta: gemini usage block honors geminiDailyLimit', async (t) => {
    // The shared server has no geminiDailyLimit configured: default 1000.
    const mainOrders = await api('/api/orders');
    assert.deepEqual(mainOrders.body.gemini, { used: 0, limit: 1000, remaining: 1000, day: todayKey() });

    const srv = await startFakeBrowserServer(t, 'geminimeta', { geminiDailyLimit: 5 });
    await fsp.writeFile(path.join(srv.notes, '.gemini-usage.json'), JSON.stringify({ day: todayKey(), used: 3 }) + '\n');
    const res = await srv.api('/api/orders');
    assert.equal(res.status, 200);
    assert.deepEqual(res.body.gemini, { used: 3, limit: 5, remaining: 2, day: todayKey() });
});


// Case 38 (§18.7): sync-now with a quick-mode body still resolves through
// the fake browser end to end — options don't break the run lifecycle.
test('38. sync-now with {mode:quick} resolves end-to-end via the fake browser', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'quick');

    const runPromise = srv.api('/api/sync-now', postJson({ mode: 'quick' }));
    const running = await srv.waitRunning();
    assert.equal(running.running, true);

    // Options normalize into the full shape during the run.
    const during = await srv.api('/api/run-options');
    assert.deepEqual(during.body.options, { ...FULL_DEFAULT_OPTIONS, mode: 'quick' });

    const ping = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 1 }));
    assert.equal(ping.status, 200);
    const run = await runPromise;
    assert.equal(run.status, 200);
    assert.deepEqual(run.body, {
        ok: true,
        result: { lastResult: 'ok', lastError: null, ordersSynced: 1 }
    });

    const meta = await srv.headlessMeta();
    assert.equal(meta.running, false);
    assert.equal(meta.lastResult, 'ok');
    assert.deepEqual(meta.lastOptions, { ...FULL_DEFAULT_OPTIONS, mode: 'quick' });
});

// Case 39 (§19.4): client-log forwarding endpoint — accepted lines land in
// the SAME §18.3 ring buffer with the `[ext] ` prefix, in order; oversized
// and non-string payloads are 400.
test('39. client-log: [ext]-prefixed lines in /api/logs, 400 on bad payloads', async () => {
    const before = await api('/api/logs');
    const cursor = before.body.next;

    const res = await api('/api/client-log', postJson({
        lines: ['[10:00:01] --- Headless auto-sync pass started ---', '[10:00:02] Syncing order 111-1...', '[10:00:03] Pass complete; 3 order(s) sent.']
    }));
    assert.equal(res.status, 200);
    assert.deepEqual(res.body, { ok: true, accepted: 3 });

    const logs = await api(`/api/logs?after=${cursor}`);
    const texts = logs.body.lines.map(l => l.text);
    const idx1 = texts.indexOf('[ext] [10:00:01] --- Headless auto-sync pass started ---');
    const idx2 = texts.indexOf('[ext] [10:00:02] Syncing order 111-1...');
    const idx3 = texts.indexOf('[ext] [10:00:03] Pass complete; 3 order(s) sent.');
    assert.ok(idx1 !== -1 && idx2 !== -1 && idx3 !== -1, 'all three [ext] lines present');
    assert.ok(idx1 < idx2 && idx2 < idx3, 'lines keep their posted order');

    // Lines are trimmed to 500 chars (prefix added on top of the trim).
    const longLine = 'x'.repeat(600);
    const trim = await api('/api/client-log', postJson({ lines: [longLine] }));
    assert.equal(trim.status, 200);
    const trimLogs = await api('/api/logs');
    const extLines = trimLogs.body.lines.filter(l => l.text.startsWith('[ext] x'));
    assert.equal(extLines[extLines.length - 1].text, '[ext] ' + 'x'.repeat(500));

    // 101 lines → 400; a non-string line → 400; empty array → 400.
    const tooMany = await api('/api/client-log', postJson({ lines: Array.from({ length: 101 }, (_, i) => `line ${i}`) }));
    assert.equal(tooMany.status, 400);
    const nonString = await api('/api/client-log', postJson({ lines: ['ok', 42] }));
    assert.equal(nonString.status, 400);
    const empty = await api('/api/client-log', postJson({ lines: [] }));
    assert.equal(empty.status, 400);
});

// ---------------------------------------------------------------------------
// v1.7.2 (spec §20.7) — hardening-release regression tests
// ---------------------------------------------------------------------------


// Case 41 (§20.7, audit S2): descending-index writes through
// setFrontmatterArrayValue must not erase siblings — votes for index 1 then
// index 0 leave BOTH entries in the note.
test('41. descending vote writes: index 1 then index 0 keeps both votes (S2)', async () => {
    const orderId = '111-8888888-1111111';
    const asin1 = 'B0VSEQ001X';
    const asin2 = 'B0VSEQ002X';
    const mkItem = (asin, name) => ({
        asin,
        name,
        brand: 'Acme',
        current_price: '$9.99',
        link: 'https://www.amazon.com/dp/' + asin,
        features: '- feature',
        image: ''
    });
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 4, 2026',
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [mkItem(asin1, 'Vote Seq One'), mkItem(asin2, 'Vote Seq Two')]
    }));
    assert.equal(syncRes.status, 200);

    // Review-check results arrive for index 1 first, then index 0.
    const r1 = await api('/api/review-check-result', postJson({ asin: asin2, helpful_votes: 9 }));
    assert.equal(r1.status, 200);
    const r0 = await api('/api/review-check-result', postJson({ asin: asin1, helpful_votes: 3 }));
    assert.equal(r0.status, 200);

    const note = await fsp.readFile(
        path.join(notesPath, 'Orders', '2026', '08-August', `Order-${orderId}.md`), 'utf8');
    assert.ok(note.includes('helpful_votes: ["3", "9"]'), `both votes present and aligned:\n${note}`);

    const items = (await api('/api/orders')).body.orders
        .flatMap(o => o.items).filter(i => [asin1, asin2].includes(i.asin));
    assert.equal(items.find(i => i.asin === asin1).helpfulVotes, 3);
    assert.equal(items.find(i => i.asin === asin2).helpfulVotes, 9);
});

// Case 42 (§20.7, audit S3): a status containing a double quote and a newline
// is escaped by yamlEscape — the note's frontmatter gains no injected key and
// scalarField reads the value back intact.
test('42. YAML escaping: quotes and a newline in status cannot inject frontmatter (S3)', async () => {
    const orderId = '111-9999999-1111111';
    const asin = 'B0ESCAPE01X';
    const status = 'Delivered "late"\nevicted: yes';
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 5, 2026',
        total: '$9.99',
        status,
        ship_to: 'Tester',
        items: [{
            asin,
            name: 'Escape Widget',
            brand: 'Acme',
            current_price: '$9.99',
            link: 'https://www.amazon.com/dp/' + asin,
            features: '- feature',
            image: ''
        }]
    }));
    assert.equal(syncRes.status, 200);

    const note = await fsp.readFile(
        path.join(notesPath, 'Orders', '2026', '08-August', `Order-${orderId}.md`), 'utf8');
    const fmMatch = note.match(/^---\r?\n([\s\S]*?)\r?\n---/);
    assert.ok(fmMatch, 'frontmatter intact');
    assert.ok(!/^evicted\s*:/m.test(fmMatch[1]), 'no injected frontmatter key (the word survives only inside the escaped status scalar)');
    assert.ok(fmMatch[1].includes('status: "Delivered \\"late\\" evicted: yes"'), fmMatch[1]);

    const parsed = (await api('/api/orders')).body.orders.find(o => o.orderId === orderId);
    assert.equal(parsed.status, 'Delivered "late" evicted: yes', 'scalarField round-trips the escaped value');
});

// Case 43 (§20.7, audit C1/S4): the same ASIN twice in one sync payload is
// deduped (first occurrence wins) with one log line per dropped dupe.
test('43. ASIN dedupe: duplicate ASIN in one order collapses to one item + log line (S4)', async () => {
    const orderId = '111-2222222-1111111';
    const asin = 'B0DUPE001X';
    const cursor = (await api('/api/logs')).body.next;

    const mkItem = (name) => ({
        asin,
        name,
        brand: 'Acme',
        current_price: '$9.99',
        link: 'https://www.amazon.com/dp/' + asin,
        features: '- feature',
        image: ''
    });
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 6, 2026',
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [mkItem('Dupe Widget'), mkItem('Dupe Widget (second row)')]
    }));
    assert.equal(syncRes.status, 200);

    const note = await fsp.readFile(
        path.join(notesPath, 'Orders', '2026', '08-August', `Order-${orderId}.md`), 'utf8');
    assert.equal((note.match(/^### /gm) || []).length, 1, 'one item block in the note');
    assert.ok(note.includes(`asin: ["${asin}"]`), note);
    assert.ok(note.includes('### Dupe Widget\n'), 'first occurrence wins');

    const logs = await api(`/api/logs?after=${cursor}`);
    const dupeLines = logs.body.lines.filter(l => l.text.includes('[Dedupe]'));
    assert.equal(dupeLines.length, 1, 'exactly one dedupe log line');
    assert.ok(dupeLines[0].text.includes(`[Dedupe] Dropped duplicate ASIN ${asin} in order ${orderId}`), dupeLines[0].text);
});

// Case 44 (§20.7; §10's named regression, finally pinned): sync -> edit the
// draft + mark posted -> re-sync the same order — the edited review text and
// the ticked checkbox survive.
test('44. re-sync preservation roundtrip: edited review and - [x] survive a re-sync', async () => {
    const orderId = '111-3333333-1111111';
    const asin = 'B0ROUND001X';
    const mkPayload = () => postJson({
        order_id: orderId,
        date: 'August 7, 2026',
        total: '$9.99',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin,
            name: 'Roundtrip Widget',
            brand: 'Acme',
            current_price: '$9.99',
            link: 'https://www.amazon.com/dp/' + asin,
            features: '- feature',
            image: ''
        }]
    });

    const syncRes = await api('/api/sync', mkPayload());
    assert.equal(syncRes.status, 200);

    const save = await api('/api/save-review', postJson({
        asin,
        title: 'My edited title',
        text: 'My edited body, kept across syncs.'
    }));
    assert.equal(save.status, 200);
    const mark = await api('/api/mark-posted', postJson({ asin, posted: true }));
    assert.equal(mark.status, 200);

    // Re-sync the same order with no review fields in the payload.
    const resync = await api('/api/sync', mkPayload());
    assert.equal(resync.status, 200);

    const note = await fsp.readFile(
        path.join(notesPath, 'Orders', '2026', '08-August', `Order-${orderId}.md`), 'utf8');
    assert.ok(note.includes('**Title:** My edited title'), note);
    assert.ok(note.includes('**Review:** My edited body, kept across syncs.'), note);
    assert.ok(/- \[x\] Posted to Amazon\?/.test(note), note);

    const item = (await api('/api/orders')).body.orders
        .flatMap(o => o.items).find(i => i.asin === asin);
    assert.equal(item.posted, true);
    assert.equal(item.reviewTitle, 'My edited title');
    assert.equal(item.reviewText, 'My edited body, kept across syncs.');
});

// Case 45 (§20.7): /api/check — unknown order is incomplete, a synced and
// fully-posted order is complete with existingItems, missing order_id is 400.
test('45. /api/check: unknown incomplete, synced complete with existingItems, missing id 400', async () => {
    const unknown = await api('/api/check?order_id=111-0000000-0000000');
    assert.equal(unknown.status, 200);
    assert.equal(unknown.body.complete, false);

    const missing = await api('/api/check');
    assert.equal(missing.status, 400);

    const orderId = '111-5555555-1111111';
    const asin = 'B0CHECK001X';
    const syncRes = await api('/api/sync', postJson({
        order_id: orderId,
        date: 'August 8, 2026',
        total: '$5.00',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin,
            name: 'Check Widget',
            brand: 'Acme',
            current_price: '$5.00',
            link: 'https://www.amazon.com/dp/' + asin,
            features: '- feature',
            image: '',
            is_reviewed: true,
            review_title: 'Posted title',
            review_text: 'Posted body',
            review_rating: '5'
        }]
    }));
    assert.equal(syncRes.status, 200);

    const res = await api(`/api/check?order_id=${orderId}`);
    assert.equal(res.status, 200);
    assert.equal(res.body.complete, true, JSON.stringify(res.body));
    assert.ok(res.body.existingItems && res.body.existingItems[asin], 'existingItems keyed by ASIN');
    assert.equal(res.body.existingItems[asin].brand, 'Acme');
    assert.equal(res.body.existingItems[asin].current_price, '$5.00');
    assert.equal(res.body.existingItems[asin].alreadyPosted, true);
});


// Case 47 (§20.7): log ring-buffer wraparound — pushing more than the 500
// cap evicts the oldest lines and the cursor stays monotonic.
test('47. log buffer wraparound: >500 lines evict the oldest, next cursor monotonic', async () => {
    for (let batch = 0; batch < 6; batch++) {
        const res = await api('/api/client-log', postJson({
            lines: Array.from({ length: 100 }, (_, i) => `wrap-${batch}-${i}`)
        }));
        assert.equal(res.status, 200);
    }

    const logs = await api('/api/logs?after=0');
    assert.equal(logs.status, 200);
    assert.equal(logs.body.lines.length, 500, 'buffer holds exactly the cap');
    const next = logs.body.next;
    assert.equal(logs.body.lines[0].i, next - 499, 'oldest indices evicted');
    assert.equal(logs.body.lines[logs.body.lines.length - 1].i, next);
    for (let k = 1; k < logs.body.lines.length; k++) {
        assert.ok(logs.body.lines[k].i > logs.body.lines[k - 1].i, 'indices monotonic');
    }
    assert.ok(!logs.body.lines.some(l => l.text.includes('wrap-0-0')), 'the first pushed line is gone');
    assert.ok(logs.body.lines.some(l => l.text.includes('wrap-5-99')), 'the newest line is present');
});

// ---------------------------------------------------------------------------
// v1.8.0 (spec §21.3)
// ---------------------------------------------------------------------------

// Case 48 (§21.1): removed-review state machine end to end — an inconclusive
// (captcha) visit records nothing, two CONSECUTIVE loaded-empty sightings
// confirm (meta + one desktop notification), a found review clears, a
// hand-ticked checkbox without review text never flags, dismiss-missing
// clears, unknown dismiss 404s. Runs against a dedicated server whose
// AOS_NOTIFY_SEND is a fake logging script.
test('48. removed-review detection: two-sighting confirm, captcha inconclusive, found clears, dismiss', { skip: POSIX_ONLY }, async (t) => {
    const notes = path.join(tmpDir, 'notes-missing');
    const port = await getFreePort();
    const base = `http://127.0.0.1:${port}`;
    const missingNotifyLog = path.join(tmpDir, 'notify-missing.log');
    const fakeNotifyBin = path.join(tmpDir, 'fake-notify-send-missing.sh');
    await fsp.writeFile(fakeNotifyBin, `#!/bin/sh\necho "$@" >> "${missingNotifyLog}"\n`);
    await fsp.chmod(fakeNotifyBin, 0o755);
    const configPath = path.join(tmpDir, 'config-missing.json');
    await fsp.writeFile(configPath, JSON.stringify({
        notesPath: notes,
        geminiApiKey: '',
        port,
        autoSyncEnabled: true,
        browserPath: fakeBrowser
    }, null, 2) + '\n');

    const proc = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: configPath,
            GEMINI_API_KEY: '',
            AOS_NOTIFY_SEND: fakeNotifyBin,
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg-missing')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    proc.stderr.on('data', d => process.stderr.write(`[missing] ${d}`));
    t.after(async () => {
        if (!proc.killed) {
            proc.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => proc.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (proc.exitCode === null) proc.kill('SIGKILL');
        }
    });
    await waitForServer(base);

    const apiM = async (pathname, options = {}) => {
        const res = await fetch(`${base}${pathname}`, options);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch { /* non-JSON */ }
        return { status: res.status, body, text };
    };
    const stateFile = path.join(notes, '.headless-sync.json');
    const readState = async () => {
        try { return JSON.parse(await fsp.readFile(stateFile, 'utf8')); }
        catch { return {}; }
    };
    const missingNotifyLines = async () => {
        try { return (await fsp.readFile(missingNotifyLog, 'utf8')).split('\n').filter(Boolean); }
        catch { return []; }
    };
    const waitForMissingNotify = async (count, timeoutMs = 5000) => {
        const deadline = Date.now() + timeoutMs;
        for (;;) {
            const lines = await missingNotifyLines();
            if (lines.length >= count) return lines;
            if (Date.now() > deadline) throw new Error(`expected ${count} missing-review notification(s); got: ${lines.join(' | ')}`);
            await new Promise(resolve => setTimeout(resolve, 50));
        }
    };

    const orderId = '111-7777777-5555555';
    const asin = 'B0MISSING1X';   // posted WITH review content
    const handAsin = 'B0MISSHAND1'; // will be hand-ticked WITHOUT content
    const syncRes = await apiM('/api/sync', postJson({
        order_id: orderId,
        date: 'January 5, 2020',
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [
            {
                asin,
                name: 'Missing Review Widget',
                brand: 'Acme',
                current_price: '$9.99',
                link: 'https://www.amazon.com/dp/' + asin,
                features: '- feature',
                image: '',
                is_reviewed: true,
                review_title: 'Was posted',
                review_text: 'Was posted body',
                review_rating: '5'
            },
            {
                asin: handAsin,
                name: 'Hand Ticked Widget',
                brand: 'Acme',
                current_price: '$9.99',
                link: 'https://www.amazon.com/dp/' + handAsin,
                features: '- feature',
                image: ''
            }
        ]
    }));
    assert.equal(syncRes.status, 200, syncRes.text);

    // Hand-tick the second item's checkbox (posted by hand, no review text).
    const mark = await apiM('/api/mark-posted', postJson({ asin: handAsin, posted: true }));
    assert.equal(mark.status, 200);

    // Invalid page_state -> 400.
    assert.equal((await apiM('/api/review-check-result', postJson({
        asin, reviewed: false, page_state: 'bogus'
    }))).status, 400);

    // Captcha visit: inconclusive — NOTHING is recorded (no sighting, no
    // vote-timestamp bump), so the item re-queues next run.
    const captcha = await apiM('/api/review-check-result', postJson({ asin, reviewed: false, page_state: 'captcha' }));
    assert.equal(captcha.status, 200);
    let state = await readState();
    assert.ok(!state.reviewMissing || !state.reviewMissing[asin], 'captcha records no sighting');
    assert.ok(!state.voteTimestamps || !state.voteTimestamps[asin], 'captcha does not bump the vote timestamp');

    // Sighting 1 (absent page_state = 'loaded'): recorded, not confirmed,
    // invisible in meta — but the suspect jumps the review-check queue.
    const s1 = await apiM('/api/review-check-result', postJson({ asin, reviewed: false }));
    assert.equal(s1.status, 200);
    state = await readState();
    assert.equal(state.reviewMissing[asin].sightings, 1);
    assert.equal(state.reviewMissing[asin].confirmed, false);
    assert.equal(typeof state.reviewMissing[asin].since, 'number');
    let body = (await apiM('/api/orders')).body;
    assert.ok(Array.isArray(body.missingReviews), 'meta carries missingReviews');
    assert.ok(!body.missingReviews.some(m => m.asin === asin), 'one sighting is not meta-visible');
    assert.equal(body.orders.flatMap(o => o.items).find(i => i.asin === asin).reviewMissing, undefined);
    const queue = await apiM('/api/review-check-queue');
    assert.equal(queue.body.asins[0], asin, 'unconfirmed suspect is queued first');

    // Sighting 2 (explicit 'loaded'): confirmed, meta-visible, one notify.
    const s2 = await apiM('/api/review-check-result', postJson({ asin, reviewed: false, page_state: 'loaded' }));
    assert.equal(s2.status, 200);
    state = await readState();
    assert.equal(state.reviewMissing[asin].sightings, 2);
    assert.equal(state.reviewMissing[asin].confirmed, true);
    assert.equal(state.reviewMissing[asin].notified, true);
    const notifyLines = await waitForMissingNotify(1);
    assert.ok(notifyLines[0].includes('Amazon review missing'), notifyLines[0]);
    assert.ok(notifyLines[0].includes('Missing Review Widget — your posted review is gone'), notifyLines[0]);
    body = (await apiM('/api/orders')).body;
    const missing = body.missingReviews.find(m => m.asin === asin);
    assert.ok(missing, 'confirmed missing review is meta-visible');
    assert.equal(missing.name, 'Missing Review Widget');
    assert.equal(missing.orderId, orderId);
    assert.equal(typeof missing.since, 'number');
    assert.equal(body.orders.flatMap(o => o.items).find(i => i.asin === asin).reviewMissing, true);

    // A further empty sighting does NOT re-notify (notified flag).
    const s3 = await apiM('/api/review-check-result', postJson({ asin, reviewed: false }));
    assert.equal(s3.status, 200);
    await new Promise(resolve => setTimeout(resolve, 300));
    assert.equal((await missingNotifyLines()).length, 1, 'notification fires exactly once');

    // The hand-ticked item (posted, but NO review text) never becomes a
    // suspect, even with consecutive empty sightings.
    await apiM('/api/review-check-result', postJson({ asin: handAsin, reviewed: false }));
    await apiM('/api/review-check-result', postJson({ asin: handAsin, reviewed: false }));
    state = await readState();
    assert.ok(!state.reviewMissing[handAsin], 'hand-ticked without content never flags');

    // Found again: the entry clears (streak reset), meta drops the flag.
    const foundAgain = await apiM('/api/review-check-result', postJson({
        asin, reviewed: true, title: 'Back again', text: 'Republished body'
    }));
    assert.equal(foundAgain.status, 200);
    state = await readState();
    assert.ok(!state.reviewMissing[asin], 'a found review clears the entry');
    body = (await apiM('/api/orders')).body;
    assert.ok(!body.missingReviews.some(m => m.asin === asin));
    assert.equal(body.orders.flatMap(o => o.items).find(i => i.asin === asin).reviewMissing, undefined);

    // Fresh re-detection (the note still says posted with content), then
    // dismiss-missing clears it; a second dismiss and unknown ASINs 404.
    await apiM('/api/review-check-result', postJson({ asin, reviewed: false }));
    await apiM('/api/review-check-result', postJson({ asin, reviewed: false }));
    state = await readState();
    assert.equal(state.reviewMissing[asin].confirmed, true, 're-detection starts fresh after a clear');
    const dismiss = await apiM('/api/dismiss-missing', postJson({ asin }));
    assert.equal(dismiss.status, 200);
    assert.deepEqual(dismiss.body, { ok: true });
    state = await readState();
    assert.ok(!state.reviewMissing[asin], 'dismiss clears the entry');
    body = (await apiM('/api/orders')).body;
    assert.ok(!body.missingReviews.some(m => m.asin === asin));
    assert.equal((await apiM('/api/dismiss-missing', postJson({ asin }))).status, 404);
    assert.equal((await apiM('/api/dismiss-missing', postJson({ asin: 'B0UNKNOWN0' }))).status, 404);
});



// Case 51 (§21.2 H5): exact /api/ route matching — /api/checkAnything is a
// 404 (was an alias of /api/check under startsWith), /api/check without
// order_id stays 400, the real routes keep working.
test('51. exact route matching: /api/checkAnything 404, /api/check without order_id 400', async () => {
    assert.equal((await api('/api/checkAnything')).status, 404);
    assert.equal((await api('/api/checkAnything?order_id=x')).status, 404);
    assert.equal((await api('/api/check')).status, 400);
    assert.deepEqual((await api('/api/check')).body, { error: 'Missing order_id' });
    // The real route still works, query string included.
    const ok = await api(`/api/check?order_id=${ORDER_ID}`);
    assert.equal(ok.status, 200);
    // Other previously prefix-matched /api/ routes are exact too.
    assert.equal((await api('/api/ordersXYZ')).status, 404);
    assert.equal((await api('/api/logs2')).status, 404);
    assert.equal((await api('/api/logs')).status, 200);
});

// Case 52 (§21.2 H6): stop-window — sync-now then an IMMEDIATE sync-stop (no
// polling for the run to arm) must not 409 in the spawn window; the run
// resolves cancelled.
test('52. stop-window: immediate sync-stop after sync-now resolves cancelled', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'stopwindow');

    const runPromise = srv.api('/api/sync-now', { method: 'POST' });
    const stop = await srv.api('/api/sync-stop', { method: 'POST' });
    assert.equal(stop.status, 200, `immediate stop must not 409 in the spawn window: ${stop.text}`);
    assert.deepEqual(stop.body, { ok: true });

    const run = await runPromise;
    assert.equal(run.status, 200);
    assert.deepEqual(run.body, {
        ok: true,
        result: { lastResult: 'cancelled', lastError: null, ordersSynced: 0 }
    });

    const meta = await srv.headlessMeta();
    assert.equal(meta.running, false);
    assert.equal(meta.lastResult, 'cancelled');
});

// ---------------------------------------------------------------------------
// v1.8.1: headless-stall hardening
// ---------------------------------------------------------------------------


// Case 54: no-activity watchdog — a headless run with ZERO client-log lines
// and ZERO order POSTs aborts as browser-stalled shortly after the
// (injected) watchdog delay; any client-log line disarms it for that run.
test('54. stall watchdog: silent run aborts browser-stalled; client-log disarms', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'stallwd', {}, { AOS_STALL_WATCHDOG_MS: '1500' });

    // Run 1: the fake browser produces NO extension activity -> watchdog fires.
    const run1Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const run1 = await run1Promise; // resolves when the watchdog aborts the run
    assert.equal(run1.status, 200);
    assert.equal(run1.body.result.lastResult, 'error');
    assert.ok(run1.body.result.lastError.startsWith('browser-stalled'),
        `expected browser-stalled error, got: ${run1.body.result.lastError}`);
    assert.ok(run1.body.result.guidance, 'stalled run carries guidance');
    assert.equal(run1.body.result.guidance.message, 'The sync browser started but the extension never responded.');

    // Persisted state/meta reflect the aborted run (history included).
    const meta1 = await srv.headlessMeta();
    assert.equal(meta1.running, false);
    assert.equal(meta1.lastResult, 'error');
    assert.ok(meta1.lastError.startsWith('browser-stalled'), meta1.lastError);
    assert.equal(meta1.guidance.message, 'The sync browser started but the extension never responded.');
    assert.ok(meta1.history.length >= 1, 'history recorded the stalled run');
    assert.equal(meta1.history[0].result, 'error');
    assert.ok(meta1.history[0].error.startsWith('browser-stalled'), meta1.history[0].error);

    // Run 2: a client-log line disarms the watchdog for the whole run.
    const run2Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const clog = await srv.api('/api/client-log', postJson({ lines: ['extension alive'] }));
    assert.equal(clog.status, 200, clog.text);
    await new Promise(resolve => setTimeout(resolve, 3500)); // > 2× the 1500ms watchdog
    const meta2 = await srv.headlessMeta();
    assert.equal(meta2.running, true, 'run must still be alive after 2× the watchdog delay');
    const stop = await srv.api('/api/sync-stop', { method: 'POST' });
    assert.equal(stop.status, 200, stop.text);
    const run2 = await run2Promise;
    assert.equal(run2.body.result.lastResult, 'cancelled');
});

// ---------------------------------------------------------------------------
// v1.8.2 (spec §22.3)
// ---------------------------------------------------------------------------

// Case 55: the login-block gate — a login-required run result blocks manual
// AND scheduled sync until SETUP LOGIN clears it; a successful completion
// keeps the gate clear.
test('55. login-block gate: 409s with guidance, due predicates false, setup clears', { skip: POSIX_ONLY }, async (t) => {
    // v1.8.3 (§23.2): while the setup browser lives, sync-now 409s — so
    // this test's fake browser exits immediately on NON-headless (setup)
    // invocations, modelling the user closing the login window; headless
    // run invocations still linger.
    const setupAwareBrowser = path.join(tmpDir, 'fake-browser-setup-aware.sh');
    await fsp.writeFile(setupAwareBrowser,
        '#!/bin/sh\ncase " $* " in\n  *--headless=new*) exec sleep 60 ;;\n  *) exit 0 ;;\nesac\n');
    await fsp.chmod(setupAwareBrowser, 0o755);
    const srv = await startFakeBrowserServer(t, 'logingate', { browserPath: setupAwareBrowser },
        { AOS_PROFILE_DIR: path.join(tmpDir, 'profile-logingate') });

    // Force a login-required run: sync-now in flight, then the extension's
    // sign-in completion ping resolves it as a login error.
    const run1Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const ping = await srv.api('/api/headless-complete', postJson({ ok: false, error: 'login-required' }));
    assert.equal(ping.status, 200, ping.text);
    const run1 = await run1Promise;
    assert.equal(run1.status, 200, run1.text);
    assert.equal(run1.body.result.lastResult, 'error');
    assert.equal(run1.body.result.lastError, 'login-required');

    // Meta reports the gate; the persisted state file carries it (survives
    // restarts — the runner reads lastResult/lastError back at boot).
    const meta1 = await srv.headlessMeta();
    assert.equal(meta1.loginBlocked, true);
    assert.equal(meta1.lastResult, 'error');
    assert.equal(meta1.lastError, 'login-required');
    const stateOnDisk = JSON.parse(await fsp.readFile(path.join(srv.notes, '.headless-sync.json'), 'utf8'));
    assert.equal(stateOnDisk.lastResult, 'error');
    assert.equal(stateOnDisk.lastError, 'login-required');
    // (A restart boots already blocked, and the predicate itself, are tested
    // in Rust: t55_login_block_survives_a_restart.)

    // Both manual triggers 409 with the login-required guidance; no spawn.
    const expectedGate = {
        error: 'login-required — run SETUP LOGIN first',
        guidance: {
            message: 'Your Amazon session expired.',
            steps: [
                'Run SETUP LOGIN to open the setup browser window.',
                'Log in to your Amazon account in that window.',
                'If you use Brave, turn Shields down for amazon.com.',
                'Close the window, then retry the sync.'
            ]
        }
    };
    const sync409 = await srv.api('/api/sync-now', { method: 'POST' });
    assert.equal(sync409.status, 409, sync409.text);
    assert.deepEqual(sync409.body, expectedGate);
    const rc409 = await srv.api('/api/review-check-now', { method: 'POST' });
    assert.equal(rc409.status, 409, rc409.text);
    assert.deepEqual(rc409.body, expectedGate);
    const metaIdle = await srv.headlessMeta();
    assert.equal(metaIdle.running, false, 'no run was spawned while gated');

    // (The scheduler's due predicates under the block: Rust, t22_due_predicate.)

    // SETUP LOGIN: 200 and the block is cleared (lastResult/lastError nulled
    // BEFORE the response — the meta right after shows the gate open).
    const setup = await srv.api('/api/headless-setup', { method: 'POST' });
    assert.equal(setup.status, 200, setup.text);
    assert.deepEqual(setup.body, { ok: true });
    const meta2 = await srv.headlessMeta();
    assert.equal(meta2.loginBlocked, false);
    assert.equal(meta2.lastResult, null);
    assert.equal(meta2.lastError, null);

    // v1.8.3 (§23.2): the quick-exit setup browser must be fully gone
    // (exit listener ran) before sync-now — while it lives, sync-now
    // 409s 'setup browser open — close it first'.
    for (let i = 0; i < 50; i++) {
        const m = await srv.headlessMeta();
        if (m.setupOpen === false) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }

    // sync-now proceeds again (fake browser) and a successful completion
    // keeps the block clear.
    const run2Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const ping2 = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 4 }));
    assert.equal(ping2.status, 200, ping2.text);
    const run2 = await run2Promise;
    assert.equal(run2.status, 200, run2.text);
    assert.equal(run2.body.result.lastResult, 'ok');
    assert.equal(run2.body.result.ordersSynced, 4);
    const meta3 = await srv.headlessMeta();
    assert.equal(meta3.loginBlocked, false);
    assert.equal(meta3.lastResult, 'ok');
    assert.equal(meta3.lastError, null);
});

// ---------------------------------------------------------------------------
// v1.8.3 (spec §23.7)
// ---------------------------------------------------------------------------




// Case 59 (state machine completeness): a captcha visit AFTER sighting 1
// preserves the consecutive-sighting streak (records nothing), and the next
// loaded-empty sighting confirms.
test('59. captcha after sighting 1 preserves the streak; next loaded-empty confirms', async () => {
    const stateFile = path.join(notesPath, '.headless-sync.json');
    const readMissing = async () => {
        const s = JSON.parse(await fsp.readFile(stateFile, 'utf8'));
        return (s.reviewMissing || {})[ASIN_A] || null;
    };
    // Clean slate for ASIN_A (posted WITH review content since before()).
    await api('/api/dismiss-missing', postJson({ asin: ASIN_A })); // 200 or 404

    // Sighting 1: recorded, unconfirmed.
    const s1 = await api('/api/review-check-result', postJson({ asin: ASIN_A, reviewed: false }));
    assert.equal(s1.status, 200, s1.text);
    let entry = await readMissing();
    assert.equal(entry.sightings, 1);
    assert.equal(entry.confirmed, false);

    // Captcha: inconclusive — the streak is PRESERVED (not cleared, not
    // incremented).
    const cap = await api('/api/review-check-result', postJson({ asin: ASIN_A, reviewed: false, page_state: 'captcha' }));
    assert.equal(cap.status, 200, cap.text);
    entry = await readMissing();
    assert.equal(entry.sightings, 1, 'captcha neither clears nor increments the streak');
    assert.equal(entry.confirmed, false);

    // Next loaded-empty sighting: confirms (the captcha did not break the
    // consecutive streak).
    const s2 = await api('/api/review-check-result', postJson({ asin: ASIN_A, reviewed: false, page_state: 'loaded' }));
    assert.equal(s2.status, 200, s2.text);
    entry = await readMissing();
    assert.equal(entry.sightings, 2);
    assert.equal(entry.confirmed, true);
    const orders = await api('/api/orders');
    assert.ok(orders.body.missingReviews.some(m => m.asin === ASIN_A), 'confirmed missing review is meta-visible');

    // Leave a clean slate.
    assert.equal((await api('/api/dismiss-missing', postJson({ asin: ASIN_A }))).status, 200);
});

// Case 60 (gate correctness): a fresh run that fails with a NON-login error
// replaces the persisted login failure — the login-block gate clears.
test('60. non-login error result clears the login-block gate', { skip: POSIX_ONLY }, async (t) => {
    // Fake browser that exits immediately on NON-headless (setup)
    // invocations — the user "closes the login window" right away (§23.2).
    const setupAwareBrowser2 = path.join(tmpDir, 'fake-browser-setup-aware2.sh');
    await fsp.writeFile(setupAwareBrowser2,
        '#!/bin/sh\ncase " $* " in\n  *--headless=new*) exec sleep 60 ;;\n  *) exit 0 ;;\nesac\n');
    await fsp.chmod(setupAwareBrowser2, 0o755);
    const srv = await startFakeBrowserServer(t, 'gateclear', { browserPath: setupAwareBrowser2 });

    // Force a login-required result -> gate on.
    const run1Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    await srv.api('/api/headless-complete', postJson({ ok: false, error: 'login-required' }));
    const run1 = await run1Promise;
    assert.equal(run1.body.result.lastError, 'login-required');
    assert.equal((await srv.headlessMeta()).loginBlocked, true);

    // SETUP LOGIN re-opens the gate.
    const setup = await srv.api('/api/headless-setup', { method: 'POST' });
    assert.equal(setup.status, 200, setup.text);
    // v1.8.3 (§23.2): wait for the setup browser window to close.
    for (let i = 0; i < 50; i++) {
        const m = await srv.headlessMeta();
        if (m.setupOpen === false) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }

    // The next run fails with a NON-login error -> gate stays clear.
    const run2Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    await srv.api('/api/headless-complete', postJson({ ok: false, error: 'browser-stalled — no extension activity after 45s' }));
    const run2 = await run2Promise;
    assert.equal(run2.body.result.lastResult, 'error');
    const meta = await srv.headlessMeta();
    assert.equal(meta.lastError, 'browser-stalled — no extension activity after 45s');
    assert.equal(meta.loginBlocked, false, 'a non-login error replaced the login failure — gate clear');

    // sync-now starts again (not login-gated) and completes ok.
    const run3Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    await srv.api('/api/headless-complete', postJson({ ok: true, processed: 1 }));
    const run3 = await run3Promise;
    assert.equal(run3.body.result.lastResult, 'ok');
    assert.equal((await srv.headlessMeta()).loginBlocked, false);
});


// Case 62 (§23.5 S-h): malformed JSON -> 400 with a static message on
// /api/sync and /api/mark-posted (was a 500 leaking the parser error).
test('62. malformed JSON -> 400 on /api/sync and /api/mark-posted (S-h)', async () => {
    const bad = {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{not json'
    };
    const sync = await api('/api/sync', bad);
    assert.equal(sync.status, 400, sync.text);
    assert.deepEqual(sync.body, { error: 'Invalid JSON body' });
    const mark = await api('/api/mark-posted', bad);
    assert.equal(mark.status, 400, mark.text);
    assert.deepEqual(mark.body, { error: 'Invalid JSON body' });
});

// ---------------------------------------------------------------------------
// v1.8.4 (spec §24.3)
// ---------------------------------------------------------------------------

// Case 63 (§24.1): watchdog strictness — only SUCCESSFUL work disarms the
// stall watchdog. A malformed (400) sync POST no longer counts as activity
// (the run still settles browser-stalled on schedule); a client-log line
// still disarms (liveness proof); a 2xx order write still disarms.
test('63. watchdog strictness: 400 sync POST does not disarm; client-log and 2xx write do', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'stallstrict', {}, { AOS_STALL_WATCHDOG_MS: '1500' });

    // Run 1: a malformed sync POST (400) must NOT disarm — the run still
    // aborts browser-stalled right on the watchdog schedule.
    const run1Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const bad = await srv.api('/api/sync', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{not json'
    });
    assert.equal(bad.status, 400, bad.text);
    const run1 = await run1Promise; // resolves when the watchdog aborts the run
    assert.equal(run1.status, 200, run1.text);
    assert.equal(run1.body.result.lastResult, 'error');
    assert.ok(run1.body.result.lastError.startsWith('browser-stalled'),
        `malformed sync POST must not disarm the watchdog; got: ${run1.body.result.lastError}`);

    // Run 2: a client-log line disarms the watchdog for the whole run.
    const run2Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const clog = await srv.api('/api/client-log', postJson({ lines: ['extension alive'] }));
    assert.equal(clog.status, 200, clog.text);
    await new Promise(resolve => setTimeout(resolve, 3500)); // > 2× the 1500ms watchdog
    assert.equal((await srv.headlessMeta()).running, true, 'a client-log line must disarm the watchdog');
    const stop2 = await srv.api('/api/sync-stop', { method: 'POST' });
    assert.equal(stop2.status, 200, stop2.text);
    const run2 = await run2Promise;
    assert.equal(run2.body.result.lastResult, 'cancelled');

    // Run 3: a successful (2xx) order write still disarms.
    const run3Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const good = await srv.api('/api/sync', postJson({
        order_id: '111-2222222-3333333',
        date: 'August 2, 2026',
        total: '$1.00',
        status: 'Delivered',
        ship_to: 'Tester',
        items: []
    }));
    assert.equal(good.status, 200, good.text);
    await new Promise(resolve => setTimeout(resolve, 3500)); // > 2× the watchdog
    assert.equal((await srv.headlessMeta()).running, true, 'a 2xx order write must disarm the watchdog');
    const stop3 = await srv.api('/api/sync-stop', { method: 'POST' });
    assert.equal(stop3.status, 200, stop3.text);
    const run3 = await run3Promise;
    assert.equal(run3.body.result.lastResult, 'cancelled');
});

// Case 64 (§24.1): single-instance lock — <notesPath>/.daemon.lock. A LIVE
// foreign pid makes a second daemon exit 1 with the clear message; a stale
// pid is taken over silently; a clean SIGTERM removes the file;
// AOS_SKIP_INSTANCE_LOCK=1 bypasses without touching the lock.
test('64. instance lock: live-pid refuse, stale takeover, clean exit removes, env bypass', { skip: POSIX_ONLY }, async (t) => {
    const notes = path.join(tmpDir, 'notes-instlock');
    await fsp.mkdir(notes, { recursive: true });
    const lockPath = path.join(notes, '.daemon.lock');

    // (acquire/release in-process: Rust, t64_instance_lock_acquire_release.)

    // A fake LIVE pid (sleep) holds the lock.
    const sleeper = spawn('sleep', ['120']);
    t.after(() => { try { sleeper.kill('SIGKILL'); } catch { /* already gone */ } });
    await fsp.writeFile(lockPath, JSON.stringify({ pid: sleeper.pid, port: 1, startedAt: new Date().toISOString() }) + '\n');

    const port = await getFreePort();
    const base = `http://127.0.0.1:${port}`;
    const configPath = path.join(tmpDir, 'config-instlock.json');
    await fsp.writeFile(configPath, JSON.stringify({ notesPath: notes, geminiApiKey: '', port }, null, 2) + '\n');

    const procs = [];
    t.after(async () => {
        for (const proc of procs) {
            if (proc.exitCode === null && !proc.killed) {
                proc.kill('SIGKILL');
                await Promise.race([
                    new Promise(resolve => proc.once('exit', resolve)),
                    new Promise(resolve => setTimeout(resolve, 2000))
                ]);
            }
        }
    });
    const spawnServer = (envExtra = {}) => {
        const proc = spawn(...serverCommand(), {
            env: {
                ...process.env,
                AMAZON_SYNC_CONFIG: configPath,
                GEMINI_API_KEY: '',
                XDG_CONFIG_HOME: path.join(tmpDir, 'xdg-instlock'),
                ...envExtra
            },
            stdio: ['ignore', 'pipe', 'pipe']
        });
        procs.push(proc);
        let err = '';
        proc.stderr.on('data', d => { err += d; });
        return { proc, stderr: () => err };
    };
    const waitExit = (proc, ms = 5000) => Promise.race([
        new Promise(resolve => proc.once('exit', code => resolve(code))),
        new Promise(resolve => setTimeout(() => resolve('timeout'), ms))
    ]);

    // 1. Live foreign pid -> refuse: exit 1 with the clear message.
    const refused = spawnServer();
    const code1 = await waitExit(refused.proc);
    assert.equal(code1, 1, `live-pid lock must exit 1; stderr: ${refused.stderr()}`);
    assert.ok(
        refused.stderr().includes(`another daemon (pid ${sleeper.pid}) is already serving this notes folder`),
        `clear refuse message expected; got: ${refused.stderr()}`);

    // 2. AOS_SKIP_INSTANCE_LOCK=1 bypasses entirely — the server boots and
    // the foreign lock is left untouched (no write, no removal on exit).
    const bypass = spawnServer({ AOS_SKIP_INSTANCE_LOCK: '1' });
    await waitForServer(base);
    bypass.proc.kill('SIGTERM');
    await waitExit(bypass.proc);
    const foreignAfterBypass = JSON.parse(await fsp.readFile(lockPath, 'utf8'));
    assert.equal(foreignAfterBypass.pid, sleeper.pid, 'bypass mode never touches the lock');

    // 3. The holder dies -> the lock is stale -> the next daemon takes over
    // silently and rewrites it with its own pid.
    sleeper.kill('SIGKILL');
    await waitExit(sleeper);
    const second = spawnServer();
    await waitForServer(base);
    const taken = JSON.parse(await fsp.readFile(lockPath, 'utf8'));
    assert.equal(taken.pid, second.proc.pid, 'stale lock is taken over by the new daemon');
    assert.equal(taken.port, port);

    // 4. Clean SIGTERM exit removes the lockfile.
    second.proc.kill('SIGTERM');
    const code2 = await waitExit(second.proc);
    assert.equal(code2, 0, 'clean shutdown exit code');
    await assert.rejects(fsp.lstat(lockPath), /ENOENT/, 'clean shutdown removes the lockfile');
});


// Case 66 (v1.8.5): both spawn paths pass --disable-extensions-except so a
// duplicate extension copy persisted in the profile can't double-scrape.
test('66. headless spawn passes --disable-extensions-except alongside --load-extension', { skip: POSIX_ONLY }, async (t) => {
    const argvBrowser = path.join(tmpDir, 'argv-browser.sh');
    await fsp.writeFile(argvBrowser, '#!/bin/sh\nprintf "%s\\n" "$@" > "$0.argv"\nsleep 60\n');
    await fsp.chmod(argvBrowser, 0o755);

    const srv = await startFakeBrowserServer(t, 'spawnargs', { browserPath: argvBrowser });
    const syncNowPromise = srv.api('/api/sync-now', { method: 'POST' });

    // Wait for the fake browser to record its argv, then complete the run.
    let argvRaw = '';
    for (let i = 0; i < 50; i++) {
        argvRaw = await fsp.readFile(`${argvBrowser}.argv`, 'utf8').catch(() => '');
        if (argvRaw) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(argvRaw, 'fake browser should have recorded its argv');
    const complete = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 0 }));
    assert.equal(complete.status, 200);
    const syncNow = await syncNowPromise;
    assert.equal(syncNow.status, 200);

    const loadExt = argvRaw.split('\n').find(a => a.startsWith('--load-extension='));
    const except = argvRaw.split('\n').find(a => a.startsWith('--disable-extensions-except='));
    assert.ok(loadExt, 'must pass --load-extension');
    assert.ok(except, 'must pass --disable-extensions-except (v1.8.5 duplicate-extension guard)');
    assert.equal(except, '--disable-extensions-except=' + loadExt.slice('--load-extension='.length),
        'both flags must pin the same extension directory');
    assert.ok(loadExt.endsWith('browser-extension'));
});

// ---------------------------------------------------------------------------
// v1.8.7 (spec §25.3): headless run progress logging
// ---------------------------------------------------------------------------

// Case 67 (§25.1): early browser exit — a fake browser that exits
// immediately (code 3) settles the run as browser-exited within seconds
// (not on the 45s watchdog or the 6-minute timeout), with the early-exit
// log line and the mapped guidance.
test('67. early exit: exit-3 fake browser settles browser-exited with log line + guidance', { skip: POSIX_ONLY }, async (t) => {
    const exitBrowser = path.join(tmpDir, 'fake-browser-exit3.sh');
    await fsp.writeFile(exitBrowser, '#!/bin/sh\nexit 3\n');
    await fsp.chmod(exitBrowser, 0o755);
    const srv = await startFakeBrowserServer(t, 'earlyexit', { browserPath: exitBrowser });

    const startedAt = Date.now();
    const run = await srv.api('/api/sync-now', { method: 'POST' });
    assert.equal(run.status, 200, run.text);
    assert.ok(Date.now() - startedAt < 20000, 'early exit settles in seconds, not on the watchdog');
    assert.equal(run.body.result.lastResult, 'error');
    assert.ok(run.body.result.lastError.startsWith('browser-exited'),
        `expected browser-exited error, got: ${run.body.result.lastError}`);
    assert.equal(run.body.result.lastError, 'browser-exited — the sync browser closed unexpectedly');
    assert.ok(run.body.result.guidance, 'early-exit run carries guidance');
    assert.equal(run.body.result.guidance.message, 'The sync browser crashed or closed on its own.');

    // The early-exit log line landed in the daemon log ring buffer.
    const logs = await srv.api('/api/logs');
    const earlyLines = logs.body.lines.filter(l => l.text.includes('Browser exited early'));
    assert.equal(earlyLines.length, 1,
        `exactly one early-exit line; got: ${logs.body.lines.map(l => l.text).join(' | ')}`);
    assert.ok(earlyLines[0].text.includes('(code 3) — crash or profile issue'), earlyLines[0].text);

    // Persisted state/meta reflect the crashed run (history included).
    const meta = await srv.headlessMeta();
    assert.equal(meta.running, false);
    assert.equal(meta.lastResult, 'error');
    assert.ok(meta.lastError.startsWith('browser-exited'), meta.lastError);
    assert.equal(meta.guidance.message, 'The sync browser crashed or closed on its own.');
    assert.ok(meta.history.length >= 1, 'history recorded the crashed run');
    assert.equal(meta.history[0].result, 'error');
    assert.ok(meta.history[0].error.startsWith('browser-exited'), meta.history[0].error);
});

// Case 68 (§25.1): waiting ticks — with an env-shortened tick interval and
// no extension activity, at least one `…waiting for extension` line with
// the counters appears before the watchdog settles; the first client-log
// line of a run emits the Extension-alive line and stops further ticks.
test('68. waiting ticks: counters logged while silent; alive line stops ticks', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'waittick', {},
        { AOS_WAIT_TICK_MS: '300', AOS_STALL_WATCHDOG_MS: '5000' });

    // Run 1: no activity -> tick lines with counters, then the stall
    // watchdog settles the run.
    const run1Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    let tickLines = [];
    for (let i = 0; i < 50 && tickLines.length === 0; i++) {
        const logs = await srv.api('/api/logs');
        tickLines = logs.body.lines.filter(l => l.text.includes('…waiting for extension'));
        if (tickLines.length === 0) await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(tickLines.length >= 1, 'at least one waiting-tick line before the watchdog settles');
    assert.ok(/…waiting for extension: 0 client lines, 0 orders \(\d+s\)/.test(tickLines[0].text),
        tickLines[0].text);
    const run1 = await run1Promise;
    assert.ok(run1.body.result.lastError.startsWith('browser-stalled'),
        `silent run still settles browser-stalled, got: ${run1.body.result.lastError}`);

    // Run 2: the first client-log line emits Extension-alive and the ticks
    // stop permanently for the run.
    const run2Promise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const clog = await srv.api('/api/client-log', postJson({ lines: ['hello from extension'] }));
    assert.equal(clog.status, 200, clog.text);
    await new Promise(resolve => setTimeout(resolve, 1500)); // > 4× the 300ms tick interval
    const logs2 = await srv.api('/api/logs');
    const aliveLines = logs2.body.lines.filter(l => l.text.includes('Extension alive in the sync browser.'));
    assert.equal(aliveLines.length, 1,
        `exactly one Extension-alive line; got: ${aliveLines.map(l => l.text).join(' | ')}`);
    const aliveIndex = aliveLines[0].i;
    const ticksAfterAlive = logs2.body.lines.filter(l =>
        l.i > aliveIndex && l.text.includes('…waiting for extension'));
    assert.equal(ticksAfterAlive.length, 0,
        `no ticks after the alive line; got: ${ticksAfterAlive.map(l => l.text).join(' | ')}`);
    const meta2 = await srv.headlessMeta();
    assert.equal(meta2.running, true, 'run must still be alive (activity disarmed the watchdog too)');
    const stop = await srv.api('/api/sync-stop', { method: 'POST' });
    assert.equal(stop.status, 200, stop.text);
    const run2 = await run2Promise;
    assert.equal(run2.body.result.lastResult, 'cancelled');
});


// Case 70 (§26.2): review cutoff — unposted items from orders older than
// reviewCutoffDays gain beyondCutoff:true and are skipped by the
// review-check queue's unposted lane; the posted lane is unchanged; meta
// echoes the cutoff; 0 disables (no flags at all).
test('70. review cutoff: beyondCutoff flags, queue skip, meta echo, 0 disables', async (t) => {
    // (Validation of the setting itself: Rust, t70_config_validation.)

    // A dedicated server with reviewCutoffDays: 60.
    const notes70 = path.join(tmpDir, 'notes-cutoff');
    const port70 = await getFreePort();
    const base70 = `http://127.0.0.1:${port70}`;
    const config70 = path.join(tmpDir, 'config-cutoff.json');
    await fsp.writeFile(config70, JSON.stringify({
        notesPath: notes70,
        geminiApiKey: '',
        port: port70,
        reviewCutoffDays: 60
    }, null, 2) + '\n');
    const child70 = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: config70,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg70')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    child70.stderr.on('data', d => process.stderr.write(`[server70] ${d}`));
    t.after(async () => {
        if (!child70.killed) {
            child70.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => child70.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (child70.exitCode === null) child70.kill('SIGKILL');
        }
    });
    const api70 = async (pathname, options = {}) => {
        const res = await fetch(`${base70}${pathname}`, options);
        const text = await res.text();
        let body = null;
        try { body = JSON.parse(text); } catch { /* non-JSON */ }
        return { status: res.status, body, text };
    };
    await waitForServer(base70);

    const DAY = 24 * 60 * 60 * 1000;
    const fmtDate = ms => new Date(ms).toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' });
    const item = (asin, reviewed) => ({
        asin,
        name: `Cutoff Widget ${asin}`,
        brand: 'Acme',
        current_price: '$9.99',
        link: 'https://www.amazon.com/dp/' + asin,
        features: '- feature',
        image: '',
        ...(reviewed ? {
            is_reviewed: true,
            review_title: 'Posted title',
            review_text: 'Posted body',
            review_rating: '5'
        } : {})
    });
    // 90-day-old order: one unposted + one posted item.
    const oldSync = await api70('/api/sync', postJson({
        order_id: '111-9090909-0909090',
        date: fmtDate(Date.now() - 90 * DAY),
        total: '$19.98',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [item('B0CUTOLDU1', false), item('B0CUTOLDP1', true)]
    }));
    assert.equal(oldSync.status, 200, oldSync.text);
    // 10-day-old order: one unposted item.
    const youngSync = await api70('/api/sync', postJson({
        order_id: '111-1010101-0101010',
        date: fmtDate(Date.now() - 10 * DAY),
        total: '$9.99',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [item('B0CUTYOUNG1', false)]
    }));
    assert.equal(youngSync.status, 200, youngSync.text);

    const ordersRes = await api70('/api/orders');
    assert.equal(ordersRes.status, 200);
    assert.equal(ordersRes.body.reviewCutoffDays, 60, 'meta echoes the cutoff');
    const allItems = ordersRes.body.orders.flatMap(o => o.items);
    const byAsin = new Map(allItems.map(i => [i.asin, i]));
    assert.equal(byAsin.get('B0CUTOLDU1').beyondCutoff, true, 'old unposted is beyond the cutoff');
    assert.equal(byAsin.get('B0CUTOLDP1').beyondCutoff, false, 'posted items are never beyondCutoff');
    assert.equal(byAsin.get('B0CUTYOUNG1').beyondCutoff, false, 'young unposted is within the cutoff');

    const queue = await api70('/api/review-check-queue');
    assert.equal(queue.status, 200);
    assert.ok(!queue.body.asins.includes('B0CUTOLDU1'), 'beyond-cutoff unposted item is not queued');
    assert.ok(queue.body.asins.includes('B0CUTYOUNG1'), 'within-cutoff unposted item is queued');
    assert.ok(queue.body.asins.includes('B0CUTOLDP1'), 'posted lane unchanged: stale votes still queued');

    // Cutoff 0 (the shared test server) disables the flags entirely.
    const shared = await api('/api/orders');
    assert.equal(shared.body.reviewCutoffDays, 0);
    for (const order of shared.body.orders) {
        for (const it of order.items) {
            assert.ok(!('beyondCutoff' in it), `no beyondCutoff flag when disabled (${it.asin})`);
        }
    }
});

// Case 71 (§26.3): POST /api/logs/clear empties the ring buffer, keeps the
// monotonic index running (old cursors stay valid), new lines still stream.
test('71. /api/logs/clear: buffer empties, index monotonic, new lines stream', async () => {
    const before = await api('/api/logs');
    assert.equal(before.status, 200);
    const next0 = before.body.next;

    // Origin-locked like every other /api route.
    const forbidden = await api('/api/logs/clear', {
        method: 'POST',
        headers: { Origin: 'https://evil.example.com' }
    });
    assert.equal(forbidden.status, 403);

    const cleared = await api('/api/logs/clear', { method: 'POST' });
    assert.equal(cleared.status, 200);
    assert.deepEqual(cleared.body, { ok: true });

    const emptied = await api('/api/logs');
    assert.equal(emptied.body.lines.length, 0, 'buffer is empty after clear');
    assert.ok(emptied.body.next >= next0, 'monotonic index survives the clear');

    // New lines still append, with indices beyond the pre-clear cursor.
    await api('/api/client-log', postJson({ lines: ['after-clear activity'] }));
    const tailed = await api(`/api/logs?after=${emptied.body.next}`);
    assert.ok(tailed.body.lines.length > 0, 'new lines stream after a clear');
    assert.ok(tailed.body.lines.every(l => l.i > emptied.body.next));
    assert.ok(tailed.body.lines.some(l => l.text.includes('[ext] after-clear activity')));
    assert.ok(tailed.body.next > emptied.body.next);

    // And an old cursor (from before the clear) only sees the NEW lines.
    const fromOldCursor = await api(`/api/logs?after=${next0}`);
    assert.ok(fromOldCursor.body.lines.every(l => l.i > next0));
    assert.ok(!fromOldCursor.body.lines.some(l => l.text.includes('cursor-test activity')),
        'pre-clear lines are gone for old cursors too');
});

// ---------------------------------------------------------------------------
// v1.8.9 (spec §27.3)
// ---------------------------------------------------------------------------


// Case 73 (v1.8.10): /api/sync-history/clear wipes run history but keeps
// lastRunAt and never clears an active login block.
test('73. sync-history clear: history emptied, lastRunAt kept, login block preserved', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'histclear');
    const statePath = path.join(tmpDir, 'notes-histclear', '.headless-sync.json');

    // Two runs: one ok, one login-required error (creates history + a block).
    let p = srv.api('/api/sync-now', { method: 'POST' });
    for (let i = 0; i < 50; i++) {
        if ((await srv.api('/api/orders')).body.headless.running) break;
        await new Promise(r => setTimeout(r, 100));
    }
    await srv.api('/api/headless-complete', postJson({ ok: true, processed: 2 }));
    await p;
    p = srv.api('/api/sync-now', { method: 'POST' });
    for (let i = 0; i < 50; i++) {
        if ((await srv.api('/api/orders')).body.headless.running) break;
        await new Promise(r => setTimeout(r, 100));
    }
    await srv.api('/api/headless-complete', postJson({ ok: false, error: 'login-required' }));
    await p;

    let meta = (await srv.api('/api/orders')).body.headless;
    assert.equal(meta.history.length, 2);
    assert.equal(meta.loginBlocked, true);
    const keptLastRunAt = meta.lastRunAt;
    assert.ok(keptLastRunAt > 0);

    const cleared = await srv.api('/api/sync-history/clear', { method: 'POST' });
    assert.equal(cleared.status, 200);
    assert.deepEqual(cleared.body, { ok: true });

    meta = (await srv.api('/api/orders')).body.headless;
    assert.deepEqual(meta.history, [], 'history must be emptied');
    assert.equal(meta.lastRunAt, keptLastRunAt, 'lastRunAt preserved (freshness + scheduler)');
    assert.equal(meta.loginBlocked, true, 'an active login block must survive history cleanup');
    assert.equal(meta.lastError, 'login-required', 'block-causing error kept');
    const onDisk = JSON.parse(await fsp.readFile(statePath, 'utf8'));
    assert.deepEqual(onDisk.history, []);

    // Clear the block via setup, then clear again — non-blocked clear also
    // resets lastResult/lastError so stale guidance disappears.
    await srv.api('/api/headless-setup', { method: 'POST' });
    for (let i = 0; i < 50; i++) {
        if (!(await srv.api('/api/orders')).body.headless.setupOpen) break;
        await new Promise(r => setTimeout(r, 100));
    }
    await srv.api('/api/sync-history/clear', { method: 'POST' });
    meta = (await srv.api('/api/orders')).body.headless;
    assert.equal(meta.lastResult, null);
    assert.equal(meta.lastError, null);
    assert.equal(meta.loginBlocked, false);
});

// Case 74 (v1.9.0): /api/check exposes `exists` for catch-up mode, and
// sync-now accepts mode:'catchup' (round-trips through run-options).
test('74. catch-up mode: check exists flag + catchup run option', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'catchup');

    // Unknown order → exists:false; after a sync → exists:true (complete or not).
    let chk = await srv.api('/api/check?order_id=111-CATCHUP-01');
    assert.equal(chk.status, 200);
    assert.equal(chk.body.exists, false);
    const sync = await srv.api('/api/sync', postJson({
        order_id: '111-CATCHUP-01', date: 'Ordered on July 1, 2026', total: '$9.99',
        status: 'Delivered', ship_to: 'Alex Doe',
        items: [{ asin: 'B0CATCHUP001', name: 'Catch Widget', link: '', image: '',
                  current_price: '$9.99', is_reviewed: false, features: '- x' }]
    }));
    assert.equal(sync.status, 200);
    chk = await srv.api('/api/check?order_id=111-CATCHUP-01');
    assert.equal(chk.body.exists, true);

    // mode:'catchup' is accepted and echoed as the in-flight run's options.
    const syncNowPromise = srv.api('/api/sync-now', postJson({
        mode: 'catchup', maxOrders: 0, timeBudgetSec: 300,
        refresh: { prices: true, votes: true, details: true }
    }));
    let opts = null;
    for (let i = 0; i < 50; i++) {
        const r = await srv.api('/api/run-options');
        if (r.body.options && r.body.options.mode === 'catchup') { opts = r.body.options; break; }
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(opts, 'run-options should echo catchup mode during the run');
    assert.equal(opts.timeBudgetSec, 300);
    const ping = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 3 }));
    assert.equal(ping.status, 200);
    const done = await syncNowPromise;
    assert.equal(done.status, 200);
    // …and persists as lastOptions so the dashboard prefill keeps the mode.
    const meta = (await srv.api('/api/orders')).body.headless;
    assert.equal(meta.lastOptions.mode, 'catchup');
});

// Case 75 (§28.1, v1.9.1): the autosync launch URL carries a fresh per-run
// token (`#aos-autosync-<epochMs>`, mirroring the v1.8.4 review-check token)
// so the extension can tell a NEW run apart from a pagination continuation.
// Same fake-browser argv capture as test 66. (The extension's cross-page
// pagination itself is browser-side — verified by the client smoke.)
test('75. headless launch URL carries the per-run token (#aos-autosync-<epochMs>)', { skip: POSIX_ONLY }, async (t) => {
    const argvBrowser = path.join(tmpDir, 'argv-browser75.sh');
    await fsp.writeFile(argvBrowser, '#!/bin/sh\nprintf "%s\\n" "$@" > "$0.argv"\nsleep 60\n');
    await fsp.chmod(argvBrowser, 0o755);

    const srv = await startFakeBrowserServer(t, 'synctoken', { browserPath: argvBrowser });
    const syncNowPromise = srv.api('/api/sync-now', { method: 'POST' });

    // Wait for the fake browser to record its argv, then complete the run.
    let argvRaw = '';
    for (let i = 0; i < 50; i++) {
        argvRaw = await fsp.readFile(`${argvBrowser}.argv`, 'utf8').catch(() => '');
        if (argvRaw) break;
        await new Promise(resolve => setTimeout(resolve, 100));
    }
    assert.ok(argvRaw, 'fake browser should have recorded its argv');
    const complete = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 0 }));
    assert.equal(complete.status, 200);
    const syncNow = await syncNowPromise;
    assert.equal(syncNow.status, 200);

    const launchUrl = argvRaw.split('\n').find(a => a.includes('aos-autosync'));
    assert.ok(launchUrl, 'argv must contain the autosync launch URL');
    assert.match(launchUrl, /#aos-autosync-\d+/,
        `launch URL must carry the per-run token (v1.9.1); got: ${launchUrl}`);
});

// Case 76 (v1.9.2): a manual stop mid-run reports the orders that actually
// landed, not a hardcoded 0.
test('76. cancelled run reports observed order count', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'cancelcount');
    const syncNowPromise = srv.api('/api/sync-now', { method: 'POST' });
    for (let i = 0; i < 50; i++) {
        if ((await srv.api('/api/orders')).body.headless.running) break;
        await new Promise(r => setTimeout(r, 100));
    }
    // Two successful order writes land mid-run (2xx = counts, per v1.8.4).
    for (let n = 1; n <= 2; n++) {
        const r = await srv.api('/api/sync', postJson({
            order_id: `111-CANCEL-0${n}`, date: 'Ordered on July 2, 2026', total: '$5.00',
            status: 'Delivered', ship_to: 'Alex Doe',
            items: [{ asin: `B0CANCEL00${n}`, name: `Widget ${n}`, link: '', image: '',
                      current_price: '$5.00', is_reviewed: false, features: '- x' }]
        }));
        assert.equal(r.status, 200);
    }
    const stop = await srv.api('/api/sync-stop', { method: 'POST' });
    assert.equal(stop.status, 200);
    const done = await syncNowPromise;
    assert.equal(done.status, 200);
    assert.equal(done.body.result.lastResult, 'cancelled');
    assert.equal(done.body.result.ordersSynced, 2, 'cancelled run must report orders that landed before the stop');
    const meta = (await srv.api('/api/orders')).body.headless;
    assert.equal(meta.history[0].result, 'cancelled');
    assert.equal(meta.history[0].ordersSynced, 2);
});

// Case 77 (v2.0.0, §29.1/§29.3): legacy vine keys in config.json are inert —
// the server boots clean, /api/orders meta has NO vineSummary/vineWindowDays,
// items have NO vine field (even at $0.00), and export.csv has no vine column.
test('77. legacy vine config keys boot clean; no vine shape anywhere', async (t) => {
    const notes77 = path.join(tmpDir, 'notes-legacycfg');
    const port77 = await getFreePort();
    const base77 = `http://127.0.0.1:${port77}`;
    const config77 = path.join(tmpDir, 'config-legacycfg.json');
    await fsp.writeFile(config77, JSON.stringify({
        notesPath: notes77,
        geminiApiKey: '',
        port: port77,
        vineWindowDays: 30,     // pre-2.0.0 keys — must be tolerated, not read
        vineEscalationDays: 7
    }, null, 2) + '\n');

    const child77 = spawn(...serverCommand(), {
        env: {
            ...process.env,
            AMAZON_SYNC_CONFIG: config77,
            GEMINI_API_KEY: '',
            XDG_CONFIG_HOME: path.join(tmpDir, 'xdg77')
        },
        stdio: ['ignore', 'pipe', 'pipe']
    });
    child77.stderr.on('data', d => process.stderr.write(`[server77] ${d}`));
    t.after(async () => {
        if (!child77.killed) {
            child77.kill('SIGTERM');
            await Promise.race([
                new Promise(resolve => child77.once('exit', resolve)),
                new Promise(resolve => setTimeout(resolve, 3000))
            ]);
            if (child77.exitCode === null) child77.kill('SIGKILL');
        }
    });
    await waitForServer(base77); // boots clean with the legacy keys present
    const health = await fetch(`${base77}/api/health`).then(r => r.json());
    assert.equal(health.ok, true);
    assert.equal(health.version, '3.0.3');

    // Seed one order with a $0.00 item — just a price now, never a flag.
    const sync = await fetch(`${base77}/api/sync`, postJson({
        order_id: '111-7777777-0000000',
        date: 'August 4, 2026',
        total: '$0.00',
        status: 'Delivered',
        ship_to: 'Tester',
        items: [{
            asin: 'B0LEGACY01X',
            name: 'Zero Price Widget',
            brand: 'Acme',
            current_price: '$0.00',
            link: 'https://www.amazon.com/dp/B0LEGACY01X',
            features: '- feature',
            image: ''
        }]
    }));
    assert.equal(sync.status, 200, await sync.text());

    const ordersRes = await fetch(`${base77}/api/orders`);
    assert.equal(ordersRes.status, 200);
    const body = await ordersRes.json();
    assert.ok(!('vineSummary' in body), 'meta must not carry vineSummary');
    assert.ok(!('vineWindowDays' in body), 'meta must not carry vineWindowDays');
    const items = body.orders.flatMap(o => o.items);
    assert.ok(items.length > 0);
    for (const item of items) {
        assert.ok(!('vine' in item), `item ${item.asin} must not carry a vine field`);
    }

    const csv = await fetch(`${base77}/api/export.csv`).then(r => r.text());
    const header = csv.trimEnd().split('\n')[0];
    assert.equal(
        header,
        'order_id,order_date,item_name,brand,asin,price,url,rating,reviewed,posted,helpful_votes'
    );
    assert.ok(!header.includes('vine'), 'csv header must not carry a vine column');
});

// Case 77→78 numbering note: case 77 was the v2.0.0 legacy-config test.
// Case 78 (v2.0.1): /api/gemini-usage lightweight snapshot.
test('78. gemini-usage endpoint returns the live quota snapshot', async (t) => {
    const srv = await startFakeBrowserServer(t, 'gemusage', { geminiDailyLimit: 250 });
    const notesPath = path.join(tmpDir, 'notes-gemusage');
    // Two attempts already counted today.
    await fsp.writeFile(path.join(notesPath, '.gemini-usage.json'), JSON.stringify({ day: todayKey(), used: 2 }) + '\n');
    const res = await srv.api('/api/gemini-usage');
    assert.equal(res.status, 200);
    assert.deepEqual(res.body, { used: 2, limit: 250, remaining: 248, day: res.body.day, configured: false });
});


// ---------------------------------------------------------------------------
// v2.1.0: Windows port
// ---------------------------------------------------------------------------




// Case 82: end-to-end — /api/health carries shell-facing fields, a headless
// run's completion toast is readable via /api/notifications, and
// POST /api/shutdown exits the daemon cleanly (lock released).
test('82. health details, notifications endpoint and clean /api/shutdown', { skip: POSIX_ONLY }, async (t) => {
    const srv = await startFakeBrowserServer(t, 'winshell');
    const health = await srv.api('/api/health');
    assert.equal(health.status, 200);
    assert.equal(health.body.ok, true);
    assert.equal(health.body.platform, process.platform);
    assert.equal(health.body.notesPath, path.join(tmpDir, 'notes-winshell'));
    assert.equal(health.body.browserFound, true);
    assert.equal(health.body.headlessRunning, false);
    assert.ok(Number.isInteger(health.body.pid));

    const first = await srv.api('/api/notifications');
    assert.equal(first.status, 200);
    assert.deepEqual(first.body.items, []);
    const cursor = first.body.next;

    // Drive a headless run to completion so the "N orders synced" toast fires.
    const startPromise = srv.api('/api/sync-now', { method: 'POST' });
    await srv.waitRunning();
    const done = await srv.api('/api/headless-complete', postJson({ ok: true, processed: 3 }));
    assert.equal(done.status, 200, done.text);
    const start = await startPromise;
    assert.equal(start.status, 200, start.text);
    let items = [];
    for (let i = 0; i < 50 && items.length === 0; i++) {
        await new Promise(r => setTimeout(r, 100));
        items = (await srv.api(`/api/notifications?after=${cursor}`)).body.items;
    }
    assert.ok(items.some(n => n.title === 'Amazon Order Sync' && n.body === '3 orders synced'), JSON.stringify(items));

    const lockPath = path.join(tmpDir, 'notes-winshell', '.daemon.lock');
    await fsp.access(lockPath);
    const shut = await srv.api('/api/shutdown', { method: 'POST' });
    assert.equal(shut.status, 200);
    assert.deepEqual(shut.body, { ok: true });
    const exitCode = await Promise.race([
        new Promise(resolve => srv.proc.once('exit', resolve)),
        new Promise(resolve => setTimeout(() => resolve('timeout'), 5000))
    ]);
    assert.equal(exitCode, 0, 'daemon must exit 0 after /api/shutdown');
    await assert.rejects(fsp.access(lockPath), 'instance lock must be released on shutdown');
});










test('v2.2.0: save-review keeps the chosen rating and get-review returns it', async () => {
    const synced = await api('/api/sync', postJson({
        order_id: '111-5550000-0000001', date: 'Ordered on May 1, 2026', status: 'Delivered',
        items: [{ asin: 'B0RATE0001', name: 'Lamp', brand: 'Acme', current_price: '$5', link: '', features: '- f', image: '' }]
    }));
    assert.equal(synced.status, 200);
    const call = async (p, o) => { const r = await api(p, o); return { status: r.status, body: r.body }; };
    const saved = await call('/api/save-review', postJson({ asin: 'B0RATE0001', title: 'Mine', text: 'My own text.', rating: 3 }));
    assert.equal(saved.status, 200);
    const got = await call('/api/get-review?asin=B0RATE0001');
    assert.deepEqual(got.body, { title: 'Mine', text: 'My own text.', rating: 3 });
    const bad = await call('/api/save-review', postJson({ asin: 'B0RATE0001', title: 'x', text: 'y', rating: 6 }));
    assert.equal(bad.status, 400);
});
