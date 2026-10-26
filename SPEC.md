# SPEC — amazon-order-sync standalone (Obsidian-free)

## 1. Overview

Port the Obsidian plugin (`/mnt/agents/upload/main.ts`, 683 lines) to a standalone
Node.js daemon so the companion Chrome extension works **unchanged**. The daemon
replaces everything Obsidian provided: the localhost HTTP server, vault file I/O,
secret storage, and the DataviewJS dashboard (replaced by a built-in web dashboard).

Target platform: CachyOS (Arch Linux). Deliverable repo: `/mnt/agents/output/amazon-order-sync/`.

**Hard contract:** the extension in `/mnt/agents/upload/{manifest.json,content.js,popup.*}`
talks to `http://127.0.0.1:27483`. Endpoints, request/response shapes, port, and the
origin-lock security model MUST remain byte-compatible with `main.ts`.

## 2. Runtime constraints

- Node.js >= 18 (global `fetch`). **Zero npm dependencies** — stdlib only
  (`http`, `fs`, `path`, `os`, `crypto`, `child_process`).
- ESM (`"type": "module"` in `server/package.json`).
- Bind `127.0.0.1` ONLY. Never `0.0.0.0`.
- Single file `server/server.js` is fine; small helper modules under `server/` allowed.

## 3. Configuration

Path: `$XDG_CONFIG_HOME/amazon-order-sync/config.json`
(default `~/.config/amazon-order-sync/config.json`; override with env `AMAZON_SYNC_CONFIG`).

```json
{
  "notesPath": "~/AmazonOrders",
  "geminiApiKey": "",
  "port": 27483
}
```

Rules:
- On first run, if missing: create parent dirs, write defaults, `chmod 0600`, log path.
- `~` expands to `os.homedir()`. `notesPath` may be absolute or `~`-prefixed.
- `geminiApiKey`: if empty string, fall back to env `GEMINI_API_KEY`. If both empty,
  review generation is skipped (same behavior as no key in original).
- `port` defaults to 27483 if absent/invalid.
- Unknown/extra keys are preserved if the file is ever rewritten (it normally is not).

## 4. HTTP API (port verbatim from main.ts)

All CORS/origin behavior identical to `main.ts`:
- `isAllowedOrigin(origin)`: no Origin header → allowed; else hostname must be
  `amazon.com` or `*.amazon.com` (case-insensitive); parse errors → false.
- Reflect caller Origin in `Access-Control-Allow-Origin` ONLY when allowed; set
  `Vary: Origin`; `Access-Control-Allow-Methods: GET, POST, OPTIONS`;
  `Access-Control-Allow-Headers: Content-Type`.
- `OPTIONS` → 204 if allowed else 403. Any disallowed-origin request → 403 JSON
  `{"error":"Forbidden origin"}` BEFORE any processing.

Endpoints:
1. `GET /api/check?order_id=<id>`
   - missing order_id → 400 `{"error":"Missing order_id"}`
   - Recursively find file named `Order-<id>.md` under `<notesPath>/Orders/`.
     Not found → `{"complete":false,"reason":"File does not exist"}`.
   - Found → port the full analysis from main.ts: parse `asin`/`brand`/`price`
     frontmatter arrays (`parseArrayField` — port verbatim incl. inline/multiline/
     single-value forms), per-ASIN `existingItems` map `{brand,current_price,
     features,alreadyPosted}`, `needsReview` (contains
     `> *Write your review for this item below...*`), `isUndelivered` (frontmatter
     `status:` lacking delivered/return/cancel), `notPosted` (contains
     `- [ ] Posted to Amazon?`). If any → `{complete:false, reason:"Needs update or
     undelivered", existingItems}` else `{complete:true, existingItems}`.
2. `GET /api/get-review?asin=<asin>`
   - missing asin → 400. Search all files under `Orders/` whose frontmatter `asin:`
     array contains `"<asin>"`; extract that item's `#### 📝 My Review` block
     `**Title:**` / `**Review:**` (port logic verbatim incl. the
     "Write your review..." exclusion). Found → `{title, text}`; else 404
     `{"error":"Review not found or not generated yet"}`.
3. `POST /api/sync` (JSON body, **5 MB limit**, exceeding → 413)
   - Parse `SyncPayload` (see main.ts lines 4–22). Call `createOrUpdateOrder`
     (port verbatim — same folder routing `Orders/YYYY/MM-MonthName/`, same
     frontmatter template, same item-section template, same review-preservation
     logic, same image caching, same Gemini trigger conditions).
   - Success → `{"success":true}`; error → 500 `{"error": msg}`.
4. `GET /` and `GET /dashboard` → dashboard HTML (section 7).
5. `GET /api/health` → `{"ok":true,"version":"1.0.8"}` (new; used by install.sh).
6. Everything else → 404.

Errors: always JSON, never leak stack traces.

## 5. Filesystem mapping (replaces Obsidian vault API)

- "Vault root" = `notesPath`. Ensure `<notesPath>/Orders` exists at startup
  (port `ensureFolderExists`).
- `vault.getFiles().find(f => f.name === 'Order-<id>.md')` → recursive walk of
  `<notesPath>/Orders/` matching basename exactly.
- **Path safety (NEW, mandatory):** `order_id` and `asin` come from the network.
  Sanitize to `[A-Za-z0-9._-]` (replace anything else with `_`) before any path
  use; reject payloads whose sanitized `order_id` is empty. Never allow `..`.
- `downloadImage(url, asin)`: skip non-http URLs; ext from URL (default jpg);
  skip if `<notesPath>/Orders/assets/<asin>.<ext>` exists; else `fetch` →
  `arrayBuffer` → write; return `assets/<asin>.<ext>` or original URL on failure.
  10 s timeout on the fetch.
- File move semantics preserved: if an existing order file's path differs from the
  newly computed folder, rename then rewrite.
- All writes: UTF-8, `\n` newlines, write-temp-then-rename for .md files.

## 6. Gemini review generation

Port `generateReview` VERBATIM from main.ts (prompt, model
`gemini-3.1-flash-lite`, `responseMimeType: application/json`, 2 s pre-delay,
429 → wait 5 s, max 3 attempts, parse `{title,text}`, fallbacks). API key source
per section 3. The no-em-dash rule in the prompt must be kept.

## 7. Dashboard (`GET /`)

Self-contained HTML (inline CSS/JS, NO external/CDN assets — localhost-only).
Data: scan `<notesPath>/Orders/**/Order-*.md`, parse frontmatter (`order_id, date,
total, status, ship_to, brand[], price[], asin[], url[], reviewed[], rating[]`)
and item names from `### <name>` headings (zip by index with asin[]).

Sections:
1. Header: total orders, lifetime spend (sum of `total` parsed as currency),
   count of items pending review.
2. Monthly spending: bar chart of last 12 months, pure CSS bars, amounts labeled.
3. "Needs Review" inbox: items whose block has unchecked `- [ ] Posted to Amazon?`
   and no generated review placeholder; each row: product name, order id/date,
   link `https://www.amazon.com/review/create-review?asin=<ASIN>` (triggers the
   extension auto-fill), and the order note's relative path.
4. Orders grouped by year → month: order id, date, status, total, item count.

Requirements:
- **HTML-escape every interpolated value** (order data is scraped from web pages).
- Rescan on each request (cheap for realistic volumes).
- Style: low-saturation warm palette (e.g. #faf7f2 bg, #2b2b28 text, one muted
  accent), system font stack, generous whitespace, no gradients. Clean and calm.

## 8. Extension tweaks (minimal, cosmetic only)

Copy `/mnt/agents/upload/{manifest.json,content.js,popup.html,popup.css,popup.js,icon.png}`
into `browser-extension/`. Then ONLY:
- `manifest.json`: version → `1.0.8`; description → "Sync Amazon orders to a local
  folder via a tiny local server."
- `popup.html`: subtitle "Export orders to Obsidian" → "Export orders to local folder";
  version label → v1.0.8.
- `content.js` / `popup.js`: user-facing log strings mentioning "Obsidian" →
  "local server" (change log prefixes `[Obsidian]` → `[Server]`). NO logic changes.

## 9. Packaging

- `server/package.json`: name `amazon-order-sync-server`, version 1.0.8,
  `"type": "module"`, `"start"` script, engines node >=18. No dependencies.
- `amazon-order-sync.service` (systemd --user):
  ```
  [Unit]
  Description=Amazon Order Sync local server
  After=default.target
  [Service]
  ExecStart=/usr/bin/env node %h/.local/share/amazon-order-sync/server/server.js
  Restart=on-failure
  RestartSec=3
  [Install]
  WantedBy=default.target
  ```
- `install.sh` (bash, `set -euo pipefail`): verify node >= 18 (else print
  `sudo pacman -S nodejs` hint and exit 1); copy `server/` →
  `~/.local/share/amazon-order-sync/server/`; copy unit →
  `~/.config/systemd/user/`; create default config if absent (0600);
  `systemctl --user daemon-reload`; `systemctl --user enable --now amazon-order-sync`;
  poll `http://127.0.0.1:27483/api/health` up to 5 s and print status + dashboard
  URL + notes path. Idempotent (safe re-run). Support `--uninstall` that
  stops/disables and removes installed files (never touches notes or config).
- `config.example.json` — documented defaults.
- `README.md`: rewrite for standalone use — architecture (extension ↔ daemon),
  CachyOS install (pacman nodejs, ./install.sh), config reference, Gemini key
  setup, dashboard URL, how the review auto-fill flow works, security model
  (origin lock, localhost bind, 0600 config), uninstall, MIT license. Keep it
  tight and professional.

## 10. Acceptance criteria

- `node server/server.js` starts with a fresh `$HOME` (creates config + notes dirs).
- curl matrix passes: preflight, 403 evil origin, 400s, sync round-trip creating
  `Orders/2026/08-August/Order-111-1111111-1111111.md` with exact frontmatter
  arrays, `/api/check` transitions false→true appropriately, review round-trip
  via `/api/get-review`, dashboard renders escaped content, 413 on oversize body,
  EADDRINUSE → clean error exit.
- Second sync of same order preserves an existing human/AI review and a checked
  `Posted to Amazon?` box (regression: main.ts lines 535–586 logic).
- `bash -n install.sh` clean; `node --check server/server.js` clean.
- No `require(` / no external imports; no Obsidian API references anywhere.

## 11. Web UI (v1.1.0)

The server-rendered dashboard is replaced by a self-contained static page
(`server/dashboard.html`: inline CSS/JS, no external or CDN assets) served at
`GET /` and `GET /dashboard`. The page fetches JSON and renders client-side;
all scraped values are inserted via `textContent`.

New endpoints (extension endpoints and their behavior unchanged):

- `GET /api/orders` — rescans `<notesPath>/Orders/**/Order-*.md` per request.
  Returns `{version, generatedAt, geminiConfigured, orders[]}`; each order has
  `orderId, date, total, status, shipTo, path` (relative to notesPath, never
  absolute), `modified` (mtime ms), and `items[]` with
  `asin, name, brand, price, rating, url, image` (`assets/<file>` if a cached
  image exists, else null), `hasDraft` (review text present and not the
  placeholder), `posted` (the item's checkbox is ticked), `reviewTitle`,
  `reviewText`. Orders sorted by frontmatter date descending (mtime fallback).
- `GET /assets/<filename>` — serves cached product images from
  `Orders/assets/`. Basename only (no slashes, no `..`), extension whitelist
  (jpg, jpeg, png, gif, webp, avif), correct Content-Type,
  `Cache-Control: public, max-age=86400`, 404 otherwise.
- `POST /api/mark-posted` `{asin, posted}` — sanitizes the ASIN per §5, finds
  the note whose frontmatter asin array contains it (most recently modified
  note wins if several match), and flips `- [ ]` ↔ `- [x]` on the
  `Posted to Amazon?` line inside that item's block only (temp-file-then-
  rename, UTF-8). Responds `{"success":true,"posted":<bool>}`; 404 when the
  ASIN is not found.

Origin policy: the amazon.com origin lock is unchanged. Same-origin dashboard
requests are additionally allowed: an Origin whose hostname is `127.0.0.1` or
`localhost` AND whose port equals the bound server port is reflected like an
amazon.com Origin. Everything else still gets 403.

UI: three tabs. **Overview**: stat cards (orders, lifetime spend, pending
review, drafts ready, posted), a pure-CSS 12-month spend bar chart, and recent
activity (8 most recently modified orders). **Orders**: live search (`/`
focuses) across order id, item name, brand, ASIN, status; year → month
collapsible groups with order counts and month subtotals; order cards with
color-coded status pills, per-item 56px thumbnails, linked product names,
and state badges (Posted / Draft ready / Needs review); posted items offer
"Mark not posted". **Needs Review**: every unposted item with its AI draft,
Copy review (clipboard), Leave review on Amazon (new tab), and Mark posted
(POST then refresh). Warm low-saturation palette (--bg #faf7f2, --panel #fff,
--text #2b2b28, --muted #8a857c, --accent #a3703d), system font stack,
responsive to ~700px, friendly empty states, and a visible error state
(suggesting `systemctl --user status amazon-order-sync`) on fetch failure.

### Theming (v1.1.1)

The dashboard is re-themed to the Terminal Workbench design system
(tokens from the suite's `tokens/tokens.css`, inlined as CSS custom
properties; MIT, https://github.com/Real-Fruit-Snacks/terminal-workbench-suite).
Dark is the default; a light token set applies via `prefers-color-scheme` or
an explicit `data-theme="light"|"dark"` on `<html>`, driven by a quiet topbar
toggle that cycles dark -> light -> auto and persists to localStorage.
Typography is the bundled Terminal Workbench Mono (OFL 1.1, in
`server/fonts/`, loaded via `@font-face` with `font-display: swap`), with
system-ui sans reserved for review prose and product names. Flat surfaces
only: no gradients, no shadows; depth comes from surface layering plus 1px
hairline borders, and color is spent only on signal (links, active tabs,
status chips, badges).

New endpoint:

- `GET /fonts/<filename>` — serves the bundled woff2 files from
  `server/fonts/` (resolved relative to the server module, not notesPath).
  Same strictness as `/assets/`: basename only, `.woff2` extension whitelist,
  `Content-Type: font/woff2`,
  `Cache-Control: public, max-age=604800, immutable`, 404 otherwise.

---

## 12. v1.2.0 — Review Studio, smart ratings, exports, hardening, tests (binding)

Version bump: server `1.2.0`, dashboard `1.2.0`, extension manifest `1.2.0`,
`server/package.json` `1.2.0`.

### 12.1 Rating variation (replaces hardcoded 5 stars)

- `defaultRatingForAsin(asin)`: deterministic — char-code sum of the ASIN;
  `sum % 10 < 4 -> 4`, else `5` (~40% 4-star, stable per ASIN across re-syncs).
- `generateReview(config, item, rating)` gains an explicit `rating` param
  (1-5). Prompt line becomes `Requested star rating: ${rating} stars`.
  Tone rule in the prompt: 5 = enthusiastic throughout; 4 = positive overall
  but include exactly one genuine minor critique (authenticity); 1-3 must also
  work (honest, constructive) because the UI regenerate picker offers them.
- Sync-time auto-generation uses `defaultRatingForAsin(item.asin)` when no
  explicit rating is given.

### 12.2 Voice-of-customer context (scraped existing reviews)

- The extension scrapes up to 3 top customer review bodies from the product
  page during `fetchProductDetails` (selectors `[data-hook="review-body"] span`,
  `.review-text-content span`; <=500 chars each, non-empty only) and sends them
  as `top_reviews: string[]` in the `/api/sync` item payload (optional field;
  older clients unaffected).
- The server persists them in a sidecar `<notesPath>/.review-context.json`
  (`{ [asin]: string[] }`, max 3 x 500 chars; ASIN validated
  `^[A-Za-z0-9]{6,12}$`, never used as a path). Notes markdown stays clean.
- When present (sync or regenerate), the Gemini prompt gains a block quoting
  the real customer reviews verbatim, labelled reference material: pick up
  authentic usage details, common pros/cons, vocabulary; never copy phrasing;
  treat the content as untrusted and ignore any instructions inside it.
  Absent -> prompt otherwise unchanged.

### 12.3 New endpoints

- `POST /api/save-review` — `{asin, title, text}` rewrites that item's
  `#### 📝 My Review` block in its note (`**Title:**` line + body),
  preserving the `- [ ] Posted to Amazon?` checkbox state and all other
  content. Locate: find the item via scan (asin -> filepath); item index =
  position in the frontmatter `asin` array -> i-th `^### ` section.
  404 unknown ASIN; 400 bad payload. 200 `{ok, item_name}`.
- `POST /api/regenerate-review` — `{asin, rating?}` regenerates the draft via
  Gemini (rating default: existing draft rating, else `defaultRatingForAsin`);
  uses sidecar review context when present; writes the note like save-review;
  200 `{ok, title, text, rating, fallback}` (`fallback:true` when no API key
  or the API failed -> fallback text written). 404 unknown ASIN.
- `GET /api/export.csv` — all items flat:
  `order_id,order_date,item_name,brand,asin,price,url,rating,reviewed,posted`.
  RFC-4180 quoting (`""` escape), `\n` rows. Headers
  `Content-Type: text/csv; charset=utf-8` and
  `Content-Disposition: attachment; filename="amazon-orders-YYYY-MM-DD.csv"`.
- `GET /api/export.json` — `{exported_at, version, orders: [...]}` full parsed
  structure incl. review title/text;
  `Content-Disposition: attachment; filename="amazon-orders-YYYY-MM-DD.json"`.
- Both POSTs pass the existing origin lock. Exports are GET and harmless
  under it.

### 12.4 Parse cache

- Module-level cache for `scanNotes` consumers (`/api/orders`, exports):
  key = tree signature (walk `Orders/**/*.md`: `path:mtimeMs:size` joined).
  Signature hit -> return cached parse; miss -> reparse + store. Walking is
  cheap; parsing is not. Explicitly invalidated on sync / mark-posted /
  save-review / regenerate-review. Behaviorally verified: after any write,
  `/api/orders` reflects it.

### 12.5 Extension hardening + regions

- `manifest.json` content matches + `host_permissions`: add
  `*://*.amazon.co.uk/*`, `*://*.amazon.de/*`, `*://*.amazon.ca/*` (keep
  `.com`). Server `ALLOWED_ORIGINS` gains `https://www.amazon.co.uk`,
  `https://www.amazon.de`, `https://www.amazon.ca`.
- `content.js`: replace absolute `https://www.amazon.com/...` fetch URLs with
  same-origin relative paths so regional domains work. `getApiBase()`
  unchanged (always 127.0.0.1).
- `fetchReviewDetails` hardening: add regexes `"initialReviewText"` and a
  `"reviewText"\s*:\s*"..."` JSON-blob variant; on a null/ambiguous result,
  wait 1500 ms and retry once before concluding "not reviewed". `is_reviewed`
  semantics stay: real title+text or nothing.

### 12.6 Dashboard — Review Studio + export

- Needs Review cards become editable: title `<input>`, body `<textarea>`
  (auto-grow, char counter), star-rating picker 1-5 (defaults to draft rating
  else 5), buttons: `SAVE DRAFT` (POST save-review), `REGENERATE` (POST
  regenerate-review with the chosen rating; busy state; card updates in
  place), plus existing COPY / AMAZON link / MARK POSTED. All new DOM text
  via `textContent`; theme tokens + manifest idiom; works in light mode.
- Overview header gains `EXPORT: [CSV] [JSON]` — plain `<a href>` links to the
  export endpoints (browser downloads).
- No new polling; the §11 orders fetch is reused.

### 12.7 Testability refactor (server.js)

- Export pure helpers for tests: `parseArrayField`, `parseNote`, `buildNote`,
  `scanNotes`, `defaultRatingForAsin`, `updateReviewBlock` (the 12.3 note
  rewriter; pure: `(markdown, asin, title, text) -> newMarkdown|null`).
- Run the server only when executed directly:
  `if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();`
  (import `pathToFileURL` from `node:url`).

### 12.8 Test suite — `tests/server.test.js` (`npm test` -> `node --test`)

Spin the built server on an ephemeral port against a tmp `notesPath` (tmp
config via `AMAZON_SYNC_CONFIG`). Cases:

1. `parseArrayField` — quoted strings with commas.
2. `buildNote` — image path from URL with query + hyphen ASIN.
3. Sync -> orders -> get-review -> mark-posted roundtrip.
4. save-review rewrites title/body, preserves a checked `- [x] Posted` box.
5. regenerate-review with no API key -> `fallback:true`, draft written with
   the requested rating (e.g. 4).
6. export.csv — header row + quoted comma field + Content-Disposition;
   export.json parses and contains the item.
7. POST with `Origin: https://evil.com` -> 403.
8. POST /api/sync with >5MB body -> 413.
9. Cache: after save-review, `/api/orders` shows the new text (no staleness).
10. `defaultRatingForAsin` — deterministic, only 4 or 5.

### 12.9 CHANGELOG.md (new, repo root)

`# Changelog` — entries 1.0.0 (standalone port), 1.0.9 (review-detection
fixes), 1.1.0 (dashboard), 1.1.1 (Terminal Workbench theme), 1.1.2 (favicon),
1.2.0 (this section). Keep-a-Changelog style, terse.

### 12.10 README updates

Feature list gains: rating variation, review-context mining, Review Studio
editing, exports, regional domains. Endpoint table gains the four new routes.
`npm test` documented under a "Development" section. Unchanged otherwise.

Acceptance (v1.2.0): `npm test` all green; the v1.0 curl matrix still passes;
save -> regenerate -> export roundtrip works end-to-end via curl; dashboard
editing verified in headless Chromium at 1440x900 (dark + light); extension
manifest validates.

---

## 13. v1.3.0 — Backfill import, bulk actions, tone presets, helpful votes, Vine filter (binding)

Version bump: server `1.3.0`, dashboard `1.3.0`, extension manifest `1.3.0`,
`server/package.json` `1.3.0`.

### 13.1 Vine ($0.00) items

- `parsePriceNumber(str)`: strip everything except digits and `.`, parseFloat;
  `null` when empty/unparseable. Exported for tests.
- `/api/orders` items gain `vine: true` when the stored price is present and
  parses to exactly `0`. Empty/unknown price is NOT vine.
- export.csv columns become `...,reviewed,posted,vine,helpful_votes` (appended,
  existing column order untouched). export.json items gain both fields.
- Dashboard: amber `VINE` badge on order cards + Needs Review cards; filter
  chips `ALL | VINE | PAID` in the Orders tab and the Needs Review tab
  (independent state per tab, default ALL).

### 13.2 Bulk actions + aging (dashboard-only, no new endpoints)

- Needs Review cards get a leading checkbox. Selecting >=1 reveals a sticky
  bulk bar: `MARK POSTED (n)`, `REGENERATE (n)` with its own rating select
  (`Keep each` default, else 1-5) and tone select (13.3), plus `CLEAR`.
  Actions loop the existing per-item endpoints sequentially with progress
  (`2/6…`); one refresh at the end; failures collected into a summary notice.
- Aging: unposted item whose order date parses and is >30 days old gets an
  amber `NNd OVERDUE` badge; >60 days, red. Unparseable date -> no badge.

### 13.3 Tone presets

- `/api/regenerate-review` accepts optional `tone` in
  `{balanced, concise, enthusiastic, critical}`; default `balanced`;
  anything else -> 400. Response echoes `tone`.
- `generateReview(..., tone)`: prompt gains one directive line —
  concise: <=60 words; enthusiastic: upbeat energy, still specific;
  critical: measured, weighs cons harder (stays fair and rating-appropriate);
  balanced: no extra line (current behavior).
- UI: small tone `<select>` beside each card's Regenerate and in the bulk bar.
  Session state only, not persisted.

### 13.4 Helpful-vote tracking

- Extension `fetchReviewDetails`: also extract helpful votes from the review
  page — regexes `"helpfulVotes"\s*:\s*(\d+)`, `(\d+)\s+people found this
  helpful`, `One person found this helpful` (=1). Integer >=0 else omit.
  Payload field `helpful_votes` (optional, only when a posted review is found).
- Server: sync persists per-item `helpful_votes` as a frontmatter array aligned
  with the `asin` array (omit entries/field when nothing known — old notes keep
  working). `parseNote` exposes `helpfulVotes` (number, default 0);
  `/api/orders` items and export.json include it; export.csv per 13.1.
- Dashboard Overview: `MOST HELPFUL REVIEWS` panel — top 5 posted items by
  votes (only when any >0): item name, vote count, order id. Pure-CSS bars.

### 13.5 Backfill importer (Amazon "Request My Data")

- New route `POST /api/import-backfill`. Two body forms:
  `Content-Type: text/csv` (single raw CSV) or JSON
  `{files:[{name, content}]}` (multi-file). Body limit for THIS route: 50 MB
  (shared streaming byte-counter; other routes keep 5 MB).
- RFC-4180 parser (`parseCsv` exported): quoted fields, embedded commas and
  newlines, doubled quotes, header row; case-insensitive column lookup;
  tolerate missing columns.
- Expected Amazon order-history columns (`Retail.OrderHistory.*.csv`):
  `Order ID`, `Order Date`, `Product Name`, `ASIN`, `Unit Price`, `Quantity`,
  `Total Owed`, `Order Status`, `Shipment Status`, `Shipping Address`,
  `Currency`. Rows missing Order ID, Product Name or ASIN are skipped and
  counted (`skipped_rows`).
- Group by Order ID -> one note per order under the normal
  `Orders/YYYY/MM-Month/` layout. Date accepts ISO (`2024-05-14[ T...]`) or
  `MM/DD/YYYY` -> canonical `Ordered on Month D, YYYY`; unparseable ->
  `Ordered on Unknown` and the note goes in `Orders/` root (existing
  behavior). total = first non-empty `Total Owed`; status = Order Status ||
  Shipment Status; ship_to = first comma-segment of Shipping Address.
- Items: `name` = Product Name, `asin`, `current_price` =
  unit-price x quantity formatted `$X.XX` (Currency other than USD ->
  `EUR 12.00` style prefix), `features` = `- Quantity: N` plus
  `- Source: Amazon data export`, no image/brand, placeholder review block,
  `reviewed: [false]`. Frontmatter gains `backfill: true`.
- Dedup: an order id that already has a note anywhere under `Orders/` is
  skipped whole (`skipped_existing`); never merges, never overwrites.
- No Gemini calls during import. Cache invalidated once at the end.
- Response 200: `{ok, imported_orders, imported_items, skipped_existing,
  skipped_rows, errors: [..]}`. 400 on unparseable body; origin lock applies
  (same-origin dashboard is allowed).
- Dashboard: `IMPORT` button in the Overview topbar -> hidden multi-file
  picker (`.csv`) -> POST JSON form -> dismissible result notice ->
  orders refresh. Help text in README: Amazon account -> Request My Data ->
  Your Orders -> wait for email -> unzip -> upload the
  `Retail.OrderHistory.*.csv` file(s).

### 13.6 Tests (append to tests/server.test.js; existing 10 must stay green)

11. `parsePriceNumber`/vine: `"$0.00"` -> vine, `"$12.99"` -> not, `""` -> not.
12. `parseCsv`: quoted commas, embedded newline inside quotes, doubled quotes.
13. import-backfill end-to-end: 2 orders, one item `$0.00` (vine) -> notes in
    right year/month folders, `backfill: true`, vine flag via /api/orders;
    second identical import -> `skipped_existing` = 2, nothing rewritten.
14. regenerate tone: `tone:"bogus"` -> 400; `tone:"concise"` -> 200 and
    response echoes `"concise"` (fallback path, no API key).
15. helpful_votes roundtrip: sync item with `helpful_votes: 7` -> /api/orders
    item has `helpfulVotes: 7`; export.csv row ends with `,false,7`-style tail.

### 13.7 Docs/version

`CHANGELOG.md` 1.3.0 entry; README features (Vine filter, bulk actions, tone
presets, helpful-vote leaderboard, backfill import how-to) + endpoint table
row for /api/import-backfill and the `tone` param note.

Acceptance (v1.3.0): `npm test` 15/15; v1.0 matrix + v1.2.0 roundtrips still
pass; live import of a realistic multi-order export CSV verified via curl;
dashboard verified headless at 1440x900 dark + light (bulk bar with
selection, Vine filter active, helpful leaderboard); `node --check` clean on
extension; manifest validates.

---

## 14. v1.4.0 — Vine deadline queue, review performance, sync freshness (binding)

Version bump: server `1.4.0`, dashboard `1.4.0`, extension manifest `1.4.0`,
`server/package.json` `1.4.0`. Local-LLM support was considered and explicitly
declined by the user — Gemini remains the only generator.

### 14.1 Config additions (both optional, config.json)

- `vineWindowDays` — integer, default 30. Days from order date a Vine review
  is considered due within.
- `syncReminderDays` — integer, default 7. Staleness threshold for 14.4.
- loadConfig tolerates absence; invalid values fall back to defaults.

### 14.2 Vine deadline tracker

- `/api/orders` top-level meta gains:
  `vineSummary: {total, posted, pending, coveragePct}` over all vine items
  (coveragePct = round(posted/total*100), 0 when total=0), plus
  `vineWindowDays` (effective config value).
- Item date math stays client-side (order date already exposed); daysLeft =
  vineWindowDays - ageDays.
- Dashboard:
  - Overview: sixth stat card `VINE COVERAGE` (pct + `posted/total` subline).
    Hidden (with layout reflow) when vineSummary.total = 0.
  - Needs Review: `VINE QUEUE` panel above the cards when any vine item is
    unposted — pending vine items sorted by daysLeft ascending; each row: item
    name, order id, and `Nd LEFT` (muted >=14d, amber <14d, red <7d) or
    `Nd OVERDUE` (red). Header shows coverage pct. Clicking a row scrolls to /
    focuses that item's card (anchor).

### 14.3 Review performance (tone persistence + analytics)

- Per-item `tone` persisted as a frontmatter array aligned with `asin`
  (`''` entries allowed; omit the field entirely when nothing known — old
  notes keep working). Written by:
  - regenerate-review: the tone used (always known, incl. `balanced`);
  - save-review: preserves existing value (same pattern as rating);
  - sync: preserves existing values on re-sync; never invents one.
- `parseNote` exposes `tone` (string, default ''); `/api/orders` items and
  export.json include it. (CSV unchanged.)
- Dashboard Overview `REVIEW PERFORMANCE` panel (hidden until >=1 posted
  review has votes):
  - Pure-CSS bar chart: one bar per posted review with helpfulVotes, ordered
    by order date; height ∝ votes; bar label = votes; tooltip-ish title line
    under each = short item name. Bars tinted by tone (balanced = muted,
    concise = cyan, enthusiastic = mint, critical = amber; legend line).
  - When >=3 voted reviews: insight lines `MOST HELPFUL TONE: X
    (avg N.N votes)` and `MOST HELPFUL RATING: N★ (avg N.N votes)` — average
    votes per tone/rating bucket, ties broken by larger sample.
  - Review Studio: per-card and bulk tone selects default to the
    best-performing tone when the insight exists (>=3 voted reviews), else
    Balanced. Explicit user choice always wins within the session.

### 14.4 Sync freshness

- `/api/orders` meta gains `lastSyncAt` (ms epoch = max mtime across order
  notes, null when none) and `syncReminderDays` (effective config value).
  Import also counts (it writes notes).
- Dashboard: when `lastSyncAt` is older than syncReminderDays (or null with
  any data), a dismissible amber banner on top of every tab:
  `LAST SYNC N DAYS AGO — OPEN YOUR AMAZON ORDERS PAGE AND RUN SYNC TO
  REFRESH`. Otherwise a quiet topbar label `SYNCED Xd AGO` (or `SYNCED TODAY`).
- Extension popup: small `Last sync: …` line under the status badge, fetched
  from `/api/orders` meta (best-effort; hide on failure). No version-string
  changes beyond 1.4.0 label.

### 14.5 Tests (append; existing 15 must stay green)

16. vineSummary: fixture with 2 vine items (1 posted via checkbox) + 1 paid ->
    `{total:2, posted:1, pending:1, coveragePct:50}` in /api/orders.
17. tone persistence: regenerate tone concise -> frontmatter `tone` array has
    `"concise"` at the item's index and /api/orders shows `tone:"concise"`;
    save-review afterwards keeps it; an untouched item has `tone:''`.
18. lastSyncAt: present and non-null after a sync; a second sync later bumps
    it (>= previous); `syncReminderDays: 3` in config is echoed in meta.

### 14.6 Docs/version

CHANGELOG 1.4.0 entry; README: Vine queue/coverage, review-performance panel
+ tone learning, sync-freshness banner, the two new config keys. Endpoint
table notes the new /api/orders meta fields.

Acceptance (v1.4.0): `npm test` 18/18; prior matrices still pass; live check
with vine + voted fixtures verified via curl; dashboard headless 1440x900
dark + light (Vine queue, coverage card, performance panel with chart +
insight lines, stale banner forced via old-mtime fixture); extension/popup
lint clean; manifest validates.

---

## 15. v1.5.0 — Headless auto-sync (binding)

Version bump: server `1.5.0`, dashboard `1.5.0`, extension manifest `1.5.0`,
`server/package.json` `1.5.0`.

Design: the daemon drives headless Chromium with the real extension loaded —
no cookie/HTTP scraping (too fragile). A one-time visible setup window captures
the Amazon login into a dedicated profile; scheduled headless runs reuse it.

### 15.1 Config additions (all optional)

- `autoSyncEnabled` — bool, default `false`.
- `autoSyncIntervalHours` — positive int, default `24`.
- `browserPath` — string, default `''` (autodetect). Invalid values → defaults.

### 15.2 Browser/profile resolution

- Binary: `browserPath` if set+executable, else first found on PATH among
  `chromium`, `chromium-browser`, `google-chrome`, `google-chrome-stable`,
  `brave-browser`, `microsoft-edge` (checked via `accessSync(X_OK)` against
  each PATH dir). None → `browserFound:false`, runs fail with
  `error:'no-browser'`.
- Profile dir: `<install data dir>/browser-profile` — resolve as
  `path.join(os.homedir(), '.local', 'share', 'amazon-order-sync',
  'browser-profile')` (same base install.sh already uses). Created on demand.
- Extension dir: module-relative `../browser-extension` (install.sh now copies
  it there — see 15.7); fallback config override not required.

### 15.3 Headless run lifecycle (server)

- `runHeadlessSync()`:
  1. Guard: a run already in progress → return `{already:true}` (callers: 409).
  2. Resolve binary (15.2) → error `no-browser`.
  3. Spawn (detached, own process group) with:
     `--headless=new --disable-gpu --no-first-run --no-default-browser-check
     --user-data-dir=<profile> --load-extension=<extDir>
     https://www.amazon.com/your-orders#aos-autosync`
     stdio ignored; unref'd parent handle kept for kill.
  4. Await the completion ping (15.4) with a 6-minute timeout.
  5. Kill the process group (`process.kill(-pid, 'SIGTERM')`, SIGKILL after
     5s), clear in-progress state.
  6. Persist state to `<notesPath>/.headless-sync.json`:
     `{lastRunAt, lastResult: 'ok'|'error', lastError, ordersSynced}`.
     Timeout → `lastError:'timeout — probably not logged in; run setup'`.
- State file read at boot; `running` is in-memory only (false at boot).

### 15.4 Endpoints

- `POST /api/headless-complete` — extension ping. Body `{ok:bool,
  processed?:int, error?:string}`. Origin lock applies (origin = amazon.com —
  already allowed). 409 `{error:'no run in progress'}` when idle. Resolves the
  pending run: ok → `lastResult:'ok'`, `ordersSynced=processed||0`;
  not ok → `lastResult:'error'`, `lastError=error||'unknown'`.
  200 `{ok:true}` on acceptance.
- `POST /api/sync-now` — dashboard trigger. 409 while running; 200
  `{ok:true, result:{lastResult,lastError,ordersSynced}}` when the run
  FINISHES (long-poll style is fine — dashboard shows a busy state; still
  bounded by the 6-min timeout). Body limit stays 5 MB; empty body.
- `POST /api/headless-setup` — launches the SAME browser/profile/extension
  NON-headless on `https://www.amazon.com/` so the user can log in; returns
  immediately `{ok:true}` (409 if no browser). The process is detached and
  NOT killed — the user closes the window. Sets no state.
- `/api/orders` meta gains:
  `headless: {enabled, running, lastRunAt, lastResult, lastError,
  ordersSynced, intervalHours, browserFound}`.

### 15.5 Scheduler

- Every 30 min (unref'd interval): if `autoSyncEnabled` && browser found &&
  not running && (now − lastRunAt) > intervalHours → fire runHeadlessSync()
  (catch+log errors). No run-at-boot burst; first eligible tick does it.

### 15.6 Extension trigger (content.js)

- On orders pages (`/your-orders`, `/gp/css/order-history`): if
  `location.hash` includes `aos-autosync`, after page ready + 2s settle, run
  the SAME single-pass sync the popup's Start button triggers (reuse the
  existing routine; do not start the continuous auto-sync mode).
- Before starting: if a sign-in form is present
  (`form[name="signIn"]`, `#ap_email`, or `#authportal-center-section`) →
  POST `{ok:false, error:'login-required'}` and stop.
- On completion: POST `{ok:true, processed:<n orders sent>}`. On exception:
  `{ok:false, error:<message>}`. Guard so it fires once per page load.
  Normal browsing (no hash) is 100% untouched.

### 15.7 install.sh

- Also installs `browser-extension/` to
  `~/.local/share/amazon-order-sync/browser-extension` (rsync/cp -r,
  idempotent). README: point chrome://extensions "Load unpacked" at that path
  so extension updates land with `./install.sh`.

### 15.8 Dashboard

- Topbar (next to IMPORT): `SYNC NOW` → POST /api/sync-now, busy state
  (`SYNCING…`), result notice (`Synced N orders` / error text). Disabled
  while `headless.running`.
- Overview status line under the stat cards:
  `AUTO SYNC: ON|OFF · LAST: <relative> · OK|ERROR: <lastError>`; hidden when
  `enabled:false && !lastRunAt`. When `lastError` is `no-browser` or contains
  `login`, the notice includes the fix (install chromium / press SETUP LOGIN).
- `SETUP LOGIN` button appears when `browserFound && lastError` mentions
  login → POST /api/headless-setup → notice "Browser opened — log into
  Amazon, then close the window."
- Existing tokens/idiom; light mode; textContent only.

### 15.9 Tests (append; existing 18 stay green)

19. Binary autodetect: `browserPath` pointing at a fake script → found; bogus
    path + empty PATH probe → `browserFound:false`.
20. End-to-end with fake browser: `browserPath` = a shell script that sleeps
    60s. POST /api/sync-now in-flight → second POST /api/sync-now → 409;
    POST /api/headless-complete `{ok:true, processed:3}` → sync-now resolves
    `{lastResult:'ok', ordersSynced:3}`; state file written; fake browser
    process reaped.
21. 409 discipline: /api/headless-complete with no run → 409.
22. Scheduler math: with `autoSyncIntervalHours: 24` and a state file whose
    lastRunAt is 2 days old, the "due" predicate is true; 1 hour old → false
    (export the predicate or test via a narrow helper).

### 15.10 Docs/version

CHANGELOG 1.5.0; README: headless sync section (setup steps, config keys,
how it works in 3 bullets, security note that the profile holds an Amazon
session and stays local), endpoint table rows, updated install layout.

Acceptance (v1.5.0): `npm test` 22/22; prior matrices pass; live smoke:
fake-browser full cycle via curl; REAL chromium headless launch sanity
(binary detected, process spawns and is killed on timeout/complete) without
Amazon login; dashboard headless screenshots dark+light (SYNC NOW, status
line, error guidance); `node --check` extension clean.

---

## 16. v1.6.0 — Price watch, notifications, regions, dedup, sync health, review-checker, Vine escalation (binding)

Version bump: server `1.6.0`, dashboard `1.6.0`, extension manifest `1.6.0`,
`server/package.json` `1.6.0`.

### 16.1 Config additions (optional)

- `notificationsEnabled` — bool, default `true` (only literal `false` disables).
- `reviewCheckIntervalDays` — positive int, default `7`.
- `vineEscalationDays` — positive int, default `7`.

### 16.2 Price-drop watch

- Extension: during the existing product-page fetch, the live listing price
  (already scraped as the page's current price) is sent as `listing_price`
  (optional string, e.g. "$19.99") per sync item.
- Server: on sync, when `listing_price` parses (parsePriceNumber) AND the
  item's paid price parses, upsert `<notesPath>/.price-history.json`:
  `{[asin]: {paid:<num>, history:[{t:<ms>, price:<num>}]}}` — append one entry
  per sync (collapse same-day entries), cap history at 90 per ASIN; ASIN
  validated as usual; atomic-ish write.
- `/api/orders` items gain `listingPrice` (latest, null when none); meta gains
  `priceDrops: [{asin, name, paid, listing, savings, pct}]` — paid>0 and
  latest listing < paid, savings = paid−listing (2dp), pct = round drop %;
  sorted savings desc, cap 10. Empty array when none.
- Dashboard: `PRICE WATCH` Overview panel (hidden when empty): rows
  `name · PAID $X → NOW $Y · −Z%` (mint savings chip), top 10.

### 16.3 Desktop notifications

- `tryNotify(title, body)`: locate `notify-send` on PATH once (cache result);
  if found and `notificationsEnabled`, spawn detached
  `notify-send <title> <body>` fire-and-forget (ignore errors). Exported/factored
  for tests (injectable binary path for the fake).
- Triggers:
  - Headless/sync-now completion with `ordersSynced > 0`:
    `Amazon Order Sync` / `N orders synced`.
  - Headless failure: `Amazon Order Sync failed` / `<lastError>`.
  - Vine escalation (16.8).
- No notification on zero-order runs or repeats of the same failure within the
  same run.

### 16.4 Regions

- ALLOWED_ORIGINS += `https://www.amazon.fr`, `https://www.amazon.it`,
  `https://www.amazon.es`, `https://www.amazon.co.jp`,
  `https://www.amazon.com.au`.
- manifest matches + host_permissions: same five TLDs.
- popup.js gate regex extended to
  `amazon\.(com|co\.uk|de|ca|fr|it|es|co\.jp|com\.au)`.

### 16.5 Repeat-purchase flag

- `/api/orders` items gain `repeatCount`: number of distinct orders containing
  that ASIN (>=1). Dashboard: `REPEAT ×N` muted badge on item rows/cards when
  N>1. No spend/stat math changes.

### 16.6 Sync health history

- `<notesPath>/.headless-sync.json` gains `history:
  [{t, result, error, ordersSynced, durationMs}]` — every run (auto or manual
  trigger), newest first, cap 20. Existing single-state fields stay.
- `/api/orders` meta `headless.history` (last 10).
- Dashboard: `SYNC HEALTH` Overview panel (hidden when no history): rows
  `<relTime> · OK|ERROR · N orders · Ns` (errors red with message).

### 16.7 Headless review-checker

- Purpose: refresh helpful votes + detect reviews posted from other devices,
  without a full order sync.
- Server:
  - `GET /api/review-check-queue` → `{asins: [...]}`: posted items whose
    helpful_votes are older than reviewCheckIntervalDays (vote timestamp kept
    in the helpful_votes sidecar — extend `<notesPath>/.headless-sync.json`
    with `voteTimestamps: {asin: ms}` updated whenever votes are written) OR
    unposted items from orders older than 2 days. Cap 20, oldest-stale first.
    Empty when a run is not active AND called outside one? No — queue is
    always computable; the extension only asks during a run.
  - `POST /api/review-check-result` — `{asin, reviewed, title, text, rating,
    helpful_votes}`. reviewed with real title+text → write the note's review
    block (same path as imported reviews: title+text, mark checkbox posted,
    rating) and set votes; not-reviewed → only update votes when provided.
    Always update `voteTimestamps[asin]`. 200 `{ok:true}`; 400 bad payload.
  - `POST /api/review-check-now` — like sync-now (409 while any headless run
    is active) but launches
    `https://www.amazon.com/review/create-review?asin=<first>#aos-reviewcheck`
    after computing the queue; queue empty → resolves immediately
    `{ok:true, result:{lastResult:'ok', ordersSynced:0}}` without spawning.
  - Scheduler: also due when `autoSyncEnabled` && browserFound &&
    (now − lastReviewCheckAt) > reviewCheckIntervalDays; `lastReviewCheckAt`
    persisted in the state file. Reuses HeadlessRunner (same process
    lifecycle, same completion ping, same 6-min timeout).
  - Meta: `headless.lastReviewCheckAt` added.
- Extension (content.js): on `review/create-review` pages with hash
  `aos-reviewcheck` (and once-guard):
  1. Sign-in wall check (same selectors as §15.6) → POST /api/headless-complete
     `{ok:false, error:'login-required'}`, stop.
  2. Read the LIVE DOM with the existing fetchReviewDetails
     selectors/regexes (rating checked input, reviewTitle input, reviewText
     textarea, initialRating/initialTitle/initialBody/initialReviewText
     blobs, helpful-votes regexes) → POST /api/review-check-result.
  3. Sleep 2s (bot protection), GET /api/review-check-queue, take the next
     ASIN not yet processed this run →
     `location.href = '/review/create-review?asin=' + next + '#aos-reviewcheck'`.
     None left → POST /api/headless-complete `{ok:true, processed:n}`.
  4. Any exception → `{ok:false, error}` completion ping.
- Normal browsing untouched (hash gate). The `create-review` match pattern is
  already in the manifest for all TLDs.

### 16.8 Vine escalation

- On each scheduler tick (cheap, uses the cached scan): unposted vine item
  with daysLeft <= vineEscalationDays → `tryNotify('Vine review due soon',
  '<name> — Nd left')` at most once per item per calendar day
  (state file `vineNotified: {asin: 'YYYY-MM-DD'}`). Days computed with
  vineWindowDays like the dashboard queue.

### 16.9 Tests (append; existing 22 stay green)

23. Price watch: sync with listing_price below paid → .price-history.json
    written, /api/orders item listingPrice set, priceDrops has correct
    savings/pct; above-paid → no drop.
24. Notifications: fake notify-send earlier on PATH (script logging argv to a
    file) → headless completion with orders>0 fires it once; failure fires
    with the error; notificationsEnabled:false → no log entry.
25. Regions: preflight POST with Origin https://www.amazon.fr → not 403.
26. repeatCount: same ASIN in two orders → both items repeatCount 2.
27. Run history: two fake-browser runs → history length 2, newest first,
    durationMs >= 0, cap respected at 20 (seed 21 → 20).
28. Review-check: seed posted (stale votes) + unposted items → queue contents
    + cap; POST result with a real review → note block written, checkbox
    posted, votes + voteTimestamps updated; POST votes-only → votes updated,
    no review block.
29. Vine escalation: vine item ordered (vineWindowDays − 2) days ago + fake
    notify-send → one notification; second tick same day → still one.
30. review-check-now with empty queue → resolves ok without spawning a
    browser (assert no child / instant result).

### 16.10 Docs/version

CHANGELOG 1.6.0; README: price watch, notifications (notify-send
requirement), new regions, repeat badge, sync health, review-checker (weekly,
requires the same login profile), Vine escalation, config keys, endpoint
rows (review-check-queue/result/now).

Acceptance (v1.6.0): `npm test` 30/30; prior matrices pass; live smoke with
fake notify-send + fake browser + price fixtures via curl; dashboard headless
screenshots dark + light (PRICE WATCH, SYNC HEALTH, REPEAT badge); extension
`node --check` clean; manifest validates.

---

## 17. v1.6.1 — Stoppable headless runs (binding)

Version bump: server `1.6.1`, dashboard `1.6.1`, `server/package.json` `1.6.1`
(extension unchanged at 1.6.0).

### 17.1 POST /api/sync-stop

- 409 `{error:'no run in progress'}` when idle. Otherwise: abort the active
  run (either mode) — kill the browser process group immediately (same
  SIGTERM→SIGKILL path), resolve the run as `lastResult:'cancelled'`,
  `lastError:null`, `ordersSynced:0`, record a `history` entry with
  `result:'cancelled'`, 200 `{ok:true}`. Pending /api/sync-now and
  /api/review-check-now callers resolve with the cancelled result.
- No desktop notification for cancellations.
- Origin lock applies (dashboard same-origin).

### 17.2 Shutdown hygiene

- On SIGTERM/SIGINT, before exit: if a run is active, kill its browser
  process group. No orphan headless browsers after
  `systemctl --user restart`.

### 17.3 Dashboard

- While `headless.running` is true (from the /api/orders meta) OR a SYNC NOW
  request is locally in flight: the SYNC NOW button becomes `STOP SYNC`
  (amber/red emphasis) → POST /api/sync-stop → notice `Sync stopped`;
  sync-now's own resolve notice shows `Sync cancelled` for cancelled results
  (amber, not red). Applies to both run modes.
- SYNC HEALTH renders `CANCELLED` rows in amber.

### 17.4 Tests (append; existing 31 stay green)

32. Fake-browser run in flight → POST /api/sync-stop → 200; the sync-now
    caller resolves with `lastResult:'cancelled'`; fake browser process
    reaped; history tail has `result:'cancelled'`; second /api/sync-stop →
    409.

Acceptance: 32/32; live smoke: start run, stop it, confirm browser gone and
cancelled state; SIGTERM the daemon mid-run → no orphan browser; dashboard
headless screenshot of STOP SYNC state.

---

## 18. v1.7.0 — Sync control room (binding)

Version bump: server `1.7.0`, dashboard `1.7.0`, extension manifest `1.7.0`,
`server/package.json` `1.7.0`.

### 18.1 Run options

- `POST /api/sync-now` accepts an optional JSON body:
  `{mode: 'quick'|'full', maxOrders: int>=0, timeBudgetSec: int>=0,
  refresh: {prices: bool, votes: bool, details: bool}}`.
  Defaults: mode full, maxOrders 0 (=all), timeBudgetSec 0 (=none),
  refresh all true. Invalid fields fall back to defaults individually.
- The runner stores the effective options as the pending run's options;
  `GET /api/run-options` returns `{options}` — meaningful during a run, last
  used otherwise (extension fetches it once at headless-run start; absent
  server/older daemon → treat as full defaults).
- Review-check runs ignore these options (their queue is already minimal).
- Meta: `headless.lastOptions` echoes the last effective options.

### 18.2 Extension option application (headless branch only)

- Quick mode: before processing an order card, call the existing
  `/api/check?order_id=`; when complete → log `[Skip] Order … already
  complete` and skip entirely (no detail/vote fetches for it).
- `maxOrders` > 0: stop the pass after N orders POSTed; completion ping
  carries the actual count.
- `timeBudgetSec` > 0: before each new order, if elapsed > budget → stop
  cleanly (log line), completion ping as usual.
- `refresh.prices`/`refresh.details` false → skip the product-page fetch
  entirely for that item (brand/features/listing_price/top_reviews simply
  absent from the payload; server preservation keeps existing values).
- `refresh.votes` false → skip the review-status fetch; fall back to stored
  state (existing `alreadyPosted` path).
- Manual popup syncs do NOT fetch run-options — behavior unchanged.

### 18.3 Daemon log buffer + endpoint

- Ring buffer, 500 entries `{i, t, text}` (monotonic index, ms time). In
  `main()`, wrap console.log/console.error so every existing log line lands
  in the buffer too (extension sync logs arrive via… they don't — buffer is
  daemon-side lines only, which already include per-order sync/generation/
  headless lifecycle lines).
- `GET /api/logs?after=N` → `{lines: [...], next: <index>}`. No `after` →
  last 100. Origin rules same as other GETs.

### 18.4 Friendly guidance

- `guidanceFor(errorCode)` map (exported for tests):
  - `login-required` → "Your Amazon session expired." steps: SETUP LOGIN →
    log in → Shields down → close window → retry.
  - `no-browser` → "No Chromium-based browser found." steps: install
    chromium/brave or set browserPath.
  - `timeout…` → "The sync browser didn't finish in time." steps: usually
    login/shields — run SETUP LOGIN and check the page loads.
  - `gemini-limit` → "Daily Gemini request budget reached." steps: drafts
    fall back to templates until midnight; raise geminiDailyLimit if your
    tier allows.
  - default → the raw error text, no steps.
- Run results + `headless` meta gain `guidance {message, steps[]}` when
  `lastError` is set.

### 18.5 Gemini quota tracking

- Every actual HTTP attempt to the Gemini endpoint (including the 429
  retries) increments a counter persisted in
  `<notesPath>/.gemini-usage.json` `{day: 'YYYY-MM-DD', used: int}` (reset on
  day change, atomic-ish write).
- Config `geminiDailyLimit` — positive int, default 1000.
- `generateReview`: when `used >= limit` → skip the API entirely, return the
  fallback draft, log once per run. At >=80% → one warning log line per run.
- `/api/orders` meta gains `gemini: {used, limit, remaining, day}`.

### 18.6 Dashboard — SYNC tab (4th tab)

- Tab order: OVERVIEW | ORDERS | SYNC | NEEDS REVIEW.
- **CONTROLS** panel: mode radio `QUICK (skip completed) | FULL`, number
  inputs `MAX ORDERS` (0=all) and `TIME BUDGET (s)` (0=none), checkboxes
  `REFRESH PRICES` / `REFRESH REVIEW STATUS+VOTES` / `REFRESH DETAILS`
  (default ticked), buttons `START SYNC`, `REVIEW CHECK NOW`, and the STOP
  SYNC behavior moved here (also keep the Overview button working — same
  endpoints). START gathers the form → POST /api/sync-now with the body.
- **LIVE LOG** panel: mono scrollback (max-height, auto-scroll to bottom
  while pinned), polls /api/logs?after=N every 2s ONLY while the SYNC tab is
  visible (stop polling on tab switch); a `PAUSE`/`CLEAR VIEW` local toggle.
- **RUN STATUS + HISTORY** panel: current state (running mode, started,
  orders so far from meta) + the 20-entry history as a table
  (time/result/orders/duration), CANCELLED amber, ERROR red.
- **GUIDANCE** box: when meta has guidance, render message + numbered steps
  (amber border).
- **GEMINI USAGE** panel: `used / limit` big number, pure-CSS progress bar
  (mint → amber at 80% → full), `resets at midnight` subline; hidden when no
  API key configured.
- Existing tokens/idiom, light mode, textContent only.

### 18.7 Tests (append; existing 32 stay green)

33. run-options: POST /api/sync-now with a full options body → during the
    run, GET /api/run-options echoes them; invalid values → defaults;
    `headless.lastOptions` after.
34. logs: after activity, GET /api/logs returns lines with monotonic i;
    `?after=` cursor returns only newer lines.
35. gemini counter: record attempts → .gemini-usage.json written and
    re-loaded; day rollover resets; used>=limit → generateReview returns the
    fallback WITHOUT any fetch (inject a throwing fetch stub to prove no
    call); 80% path warns once.
36. meta: /api/orders exposes `gemini {used, limit, remaining, day}` with
    config `geminiDailyLimit: 5` honored.
37. guidanceFor: the four known codes return message+steps; unknown → raw.
38. sync-now with `{mode:'quick'}` body still resolves through the fake
    browser end-to-end (options don't break the lifecycle).

### 18.8 Docs/version

CHANGELOG 1.7.0; README: SYNC tab tour (controls, log, guidance, usage),
run-options semantics (quick vs full, refresh toggles), geminiDailyLimit
note (set it to your AI Studio tier's daily quota), endpoint rows
(run-options, logs). install unchanged.

Acceptance (v1.7.0): `npm test` 38/38; prior matrices pass; live smoke:
options roundtrip, log polling, gemini counter increment on real key-less
fallback path (no API call counted), quick-mode run via fake browser;
dashboard headless screenshots dark + light of the SYNC tab (controls, log,
guidance visible, usage bar); extension `node --check` clean.

---

## 19. v1.7.1 — Extension log forwarding (binding)

Version bump: server `1.7.1`, dashboard unchanged (label 1.7.1), extension
manifest `1.7.1`, `server/package.json` `1.7.1`.

### 19.1 POST /api/client-log

- Body `{lines: [string, ...]}` — 1..100 lines, each string trimmed to 500
  chars; 400 otherwise. Origin lock applies (amazon.com origin — allowed).
- Each line is appended to the SAME §18.3 ring buffer prefixed `[ext] `.
  200 `{ok:true, accepted: n}`.

### 19.2 Extension forwarding

- During BOTH headless branches (`aos-autosync`, `aos-reviewcheck`) — and
  only there — every `logStatus(...)` line is additionally buffered and
  flushed to /api/client-log every ~3s and once at run end (before the
  completion ping). Flush failures are ignored (never break a sync over
  logging). Manual/popup syncs unchanged.

### 19.3 Run lifecycle lines

- Ensure the daemon logs explicit lines: run start (mode + options summary),
  per-order `Syncing order …` (already exists), generation lines (exist),
  run finish (`result, orders, duration`). Add any that are missing.

### 19.4 Tests (append; existing 38 stay green)

39. client-log: POST 3 lines → GET /api/logs shows them with the `[ext] `
    prefix in order; 101-line body → 400; non-string line → 400.

Acceptance: 39/39; live smoke: fake-browser run + client-log posts → lines
visible in /api/logs stream; dashboard LIVE LOG shows extension lines.

---

## 20. v1.7.2 — Hardening release: full-review fixes (binding)

Version bump: server `1.7.2`, dashboard `1.7.2`, extension manifest `1.7.2`,
`server/package.json` `1.7.2`. Originates from a three-lens code audit;
each fix lists its audit finding.

### 20.1 Index-aligned frontmatter parsing everywhere (audit S1/S2 — data corruption)

- `parseAlignedArray` (or a shared aligned helper) MUST be used for every
  index-aligned field — `brand`, `price`, `url`, `rating`, `reviewed`,
  `helpful_votes`, `tone` — in `parseNote`, `analyzeOrder` (/api/check), and
  every re-sync preservation path (rating/helpful_votes/tone). The filtering
  `parseArrayField` remains ONLY for existence/lookup uses (e.g. `asin`
  arrays used with indexOf for presence, not index mapping).
- `setFrontmatterArrayValue` reads existing values with the aligned parser
  for ALL fields (helpful_votes included) so descending-index writes never
  erase siblings.

### 20.2 Frontmatter escaping (audit S3)

- One `yamlEscape(str)` helper (wrap in double quotes, escape inner `"` and
  `\`, strip CR/LF) applied to EVERY interpolated frontmatter value in
  buildNote: order_id, date, total, status, ship_to, brand, price, asin, url.

### 20.3 Sync-time ASIN dedupe (audit C1)

- Within one /api/sync payload, drop later items whose ASIN already appeared
  in that order (first wins) + one log line per dropped dupe. Prevents
  same-ASIN-twice notes, which made ASIN-keyed writers ambiguous.

### 20.4 Ops/docs fixes

- install.sh: `systemctl --user try-restart amazon-order-sync` after copying
  files (no-op when not running; `enable --now` keeps first install working).
- Legacy per-order `notify()` calls respect `notificationsEnabled`.
- Gemini: a REAL draft generated at the quota edge is not flagged
  `fallback:true`; only actual fallback drafts are.
- export.csv: fields beginning with `=`, `+`, `-`, `@` are prefixed with `'`.
- headless meta gains `runStartedAt` (ms, present while running).
- README: add `POST /api/client-log` + `GET /favicon.ico`/`GET /icons/<file>`
  rows, "three tabs" → four, health example 1.7.2. popup.html label 1.7.2.

### 20.5 Extension fixes (audit B1/B2/B6/B8)

- popup run state: NO log-substring matching. content.js answers
  `check_auto_sync_state` with an explicit `syncRunning` boolean (single +
  auto modes) and the popup tracks its own click state until an explicit
  end-of-run sentinel message/log line (`[Headless] Pass complete` /
  `Sync complete`) or a fresh state query. content.js adds an `isSyncing`
  re-entry guard (second Start while running → ignored + log).
- Regional scraping: order date regex unmatched → send `date: ""` (server:
  unparseable → note in `Orders/` root, NEVER silently today); total regex
  extended to `$ £ € ¥` and the currency symbol kept in the string.
- popup gate: parse `new URL(tab.url)`; require hostname ends with an
  amazon TLD from the manifest set AND pathname match — no substring gates.
  All `sendMessage` calls handle `chrome.runtime.lastError` with a
  user-visible "refresh the page" log line.
- Review-check: hash `aos-reviewcheck` present but pathname is NOT
  create-review (e.g. redirected) → POST completion
  `{ok:false, error:'redirected — Amazon moved the review page'}` once, stop.

### 20.6 Dashboard fixes (audit B3/B4/B5/B9 + SYNC-tab clarity)

- Review Studio edit persistence: `state.edits[asin] = {title, text, rating}`
  updated on every input; render restores unsaved edits over server data on
  EVERY path (tab switch, chips, search, load, mark-posted, sync buttons);
  cleared for an ASIN on successful save/regenerate/mark-posted.
- Log poller: single chain via generation counter; start/stop exactly one
  scheduling site; no duplicate chains under renderAll/pause races.
- Log forwarding (content.js): stop the 3s interval + reset the flag after
  the final completion ping; final flush loops until the buffer is drained
  (not capped at 100 lines).
- SYNC tab: panel relabeled `HEADLESS RUN STATUS`; new quiet line under it
  `LAST BROWSER SYNC: X ago` sourced from `lastSyncAt`; reviewCheckNow 409
  notice reads `Review check already running`.
- RUN STATUS uses `runStartedAt` (now sent) for the "started X ago" line.

### 20.7 Tests (append; existing 39 stay green)

40. Aligned parse: note with `rating: ["", "4"]`, `brand: ["", "Sony"]` →
    parseNote assigns them to item 2, item 1 gets '' (regression for S1).
41. Descending vote writes: review-check results for index 1 then index 0 →
    both votes present afterwards (S2).
42. YAML escaping: sync with `status` containing `"` and a newline → note
    parses back with scalarField intact, no injected key (S3).
43. ASIN dedupe: payload with the same ASIN twice → one item in the note,
    dedupe log line (S4).
44. Re-sync preservation roundtrip: sync → save-review + mark-posted →
    re-sync same order → edited review text and `- [x]` intact (§10's named
    regression, finally pinned).
45. /api/check: unknown order → incomplete; after sync → complete with
    existingItems; missing order_id → 400.
46. Gemini 429→success: stub geminiFetch to 429 once then 200 → draft from
    the 200 body, TWO quota attempts recorded.
47. Log buffer wraparound: push >500 lines → oldest index evicted, next
    cursor monotonic.

### 20.8 Version/docs

VERSION 1.7.2 everywhere (incl. popup.html label). CHANGELOG 1.7.2 entry
listing the audit-driven fixes. README endpoint/tab/version updates per 20.4.

Acceptance: `npm test` 47/47; audit findings S1–S10, E1–E10 verified fixed
by test or live smoke; dashboard headless screenshots (SYNC tab relabel,
edit-persistence click-through); zip rebuilt.

---

## 21. v1.8.0 — Removed-review detection + audit hardening batch (binding)

Version bump: server `1.8.0`, dashboard `1.8.0`, extension manifest `1.8.0`,
`server/package.json` `1.8.0`.

### 21.1 Removed-review detection

Flake-proof: a review counts as *possibly missing* only when the review page
provably loaded AND shows an empty form for an item our notes say is posted.
Two consecutive sightings confirm it.

- Extension (review-check branch): compute `page_state` per visit —
  `'captcha'` (validateCaptcha form, `#captchacharacters`, "Enter the
  characters you see below"), `'error'` (document title contains
  "Page Not Found" / "Sorry!" / empty body), else `'loaded'`. Added to the
  /api/review-check-result payload (optional; absent = 'loaded').
- Server, on result for an item whose note is posted with review content:
  - page_state != 'loaded' → inconclusive: no sighting, no voteTimestamp
    bump (item re-queues next run).
  - 'loaded' + review found → clear any missing state for the ASIN.
  - 'loaded' + empty → `reviewMissing[asin] = {since, sightings}` in the
    state file; sightings++ (consecutive only — a found-review resets to 0).
    At sightings >= 2 → `confirmed: true`, `tryNotify('Amazon review
    missing', '<name> — your posted review is gone')` once (notified flag),
    surfaced in meta.
- `/api/orders` meta gains `missingReviews: [{asin, name, orderId, since}]`
  (confirmed only); items gain `reviewMissing: true`.
- `POST /api/dismiss-missing {asin}` → clears the entry (re-detection starts
  fresh; 404 unknown). Origin-locked.
- Review-check queue prioritizes unconfirmed missing-suspects first.
- The note is NOT modified — the review text stays, ready to repost.
- Dashboard: red `REVIEW MISSING` badge on Orders rows + Needs Review cards
  for flagged items, with a small dismiss (×) action → POST dismiss-missing
  → reload.

### 21.2 Hardening batch (deferred audit minors)

- H1 downloadImage: fetch only http(s); require response Content-Type
  image/*; extension derived from Content-Type against the existing serving
  whitelist; 5 MB cap; skip (null) on any mismatch. Make the fetch
  injectable for tests (same pattern as geminiFetch).
- H2 resolveBrowser: cache the PATH scan result like resolveNotifySend
  (reset hook for tests); no per-request sync PATH scans.
- H3 parsePriceNumber: European formats — `1.234,56` → 1234.56,
  `12,99` → 12.99, `$1,234.56` → 1234.56. Rule: if both separators present,
  the rightmost is decimal; if only comma and 1-2 digits follow, decimal.
- H4 voteTimestamps: prune entries for ASINs no longer present in any note
  on each scheduler tick (keeps the map bounded).
- H5 routes: replace startsWith matching with exact path matching for all
  /api/ routes (`/api/checkAnything` → 404, `/api/check` stays).
- H6 stop-window: register the pending-run record synchronously before the
  async spawn completes so /api/sync-stop and /api/headless-complete never
  409 during the spawn window.
- H7 content.js: cap `syncLogs` at 500 entries (drop oldest); fix the
  isSyncStopped early-return that skipped `isSyncing = false`; add the
  missing `.catch` on the start_auto_sync branch.

### 21.3 Tests (append; existing 47 stay green)

48. Removed-review flow: posted item → 'loaded' empty (sighting 1, no meta)
    → again (confirmed; meta lists it; fake notify fired once) → review
    found again (cleared). page_state 'captcha' → no sighting recorded.
    dismiss-missing clears; 404 for unknown.
49. downloadImage: stubbed imageFetch — non-image Content-Type → skipped;
    6 MB body → skipped; image/png → written with .png ext.
50. parsePriceNumber: the three European cases + existing US cases intact.
51. /api/checkAnything → 404; /api/check without order_id → 400.
52. Stop-window: sync-now then IMMEDIATE sync-stop (no polling) → 200 and
    the run resolves cancelled.

### 21.4 Docs/version

CHANGELOG 1.8.0; README: removed-review detection (two-sighting rule),
dismiss endpoint row, hardening notes. Version 1.8.0 everywhere.

Acceptance: `npm test` 52/52; live smoke of the missing-review state
machine via curl; dashboard badge + dismiss verified headless; zip rebuilt.

---

## 22. v1.8.2 — Login-block gate (binding)

Version bump: server `1.8.2`, dashboard `1.8.2`, `server/package.json` `1.8.2`
(extension unchanged at 1.8.1).

### 22.1 The gate (server)

- `loginBlocked` predicate: state `lastResult === 'error'` AND `lastError`
  matches /login/i. Persisted state only (survives restarts).
- While loginBlocked:
  - `POST /api/sync-now` and `POST /api/review-check-now` → 409
    `{error: 'login-required — run SETUP LOGIN first', guidance}` (the
    login-required guidance object). No browser spawns.
  - The scheduler's due predicates (order sync + review check) are false —
    no automatic retries, no repeated failure notifications. One log line
    per skip at most (rate-limited like other per-run logs).
  - Meta `headless.loginBlocked: true`.
- Cleared by:
  - `POST /api/headless-setup` — on successful launch, reset lastResult/
    lastError to null (the user is actively re-authenticating; give the next
    run a chance), 200 as before.
  - Any run that completes `ok` (existing behavior — state overwritten).

### 22.2 Dashboard

- When `headless.loginBlocked`: START SYNC and REVIEW CHECK NOW are disabled
  (greyed, `title` explains), an amber `LOG IN REQUIRED — RUN SETUP LOGIN
  FIRST` hint replaces the button row's idle state, and SETUP LOGIN gets
  amber emphasis (it is the required action). After SETUP LOGIN returns 200,
  reload state (buttons unlock). SYNC NOW on the Overview follows the same
  rule; STOP SYNC unaffected (no run can be active while blocked).

### 22.3 Tests (append; existing 54 stay green)

55. Force a login-required run → `headless.loginBlocked === true` →
    sync-now → 409 with guidance; review-check-now → 409; due predicates
    false; headless-setup → 200 and block cleared → sync-now proceeds
    (fake browser); successful completion also clears a block.

### 22.4 Docs

CHANGELOG 1.8.2; README: the login gate (buttons lock, scheduler pauses,
SETUP LOGIN clears).

Acceptance: `npm test` 55/55; live smoke of the full gate cycle via curl;
dashboard screenshot of the locked state; zip rebuilt.

---

## 21.5. v1.8.1 — Headless-stall hardening (retroactive spec; shipped as commit 761add6)

- Extension matches `*://*.amazon.com/ap/signin*`; on sign-in pages with a
  headless hash, POST completion `{ok:false, error:'login-required'}` once.
- HeadlessRunner deletes `SingletonLock/Socket/Cookie` from the profile
  before spawn (one log line when removed).
- 45s no-activity watchdog (env `AOS_STALL_WATCHDOG_MS`): zero client-log
  lines AND zero orders since run start → settle as
  `error: browser-stalled — no extension activity after 45s` + guidance.
- `AOS_PROFILE_DIR` override for tests. Tests 53–54.

---

## 23. v1.8.3 — Second-audit fix release (binding)

Version bump: server `1.8.3`, dashboard `1.8.3`, extension manifest `1.8.3`,
`server/package.json` `1.8.3`. All findings from the second three-lens audit.

### 23.1 Auto-Fill must not run during headless passes (audit B1 — HIGH)

- content.js: the Auto-Fill block on /review/create-review pages is skipped
  entirely when `AOS_HEADLESS_HASH` or `AOS_REVIEW_CHECK_HASH` is present.
  Headless passes read the page; they must never write to it.

### 23.2 Setup-browser vs profile-lock collision (audit A1)

- Track the setup browser child (pid + exit listener) in HeadlessRunner.
- While it lives: skip Singleton* deletion, and /api/sync-now +
  /api/review-check-now → 409 `{error:'setup browser open — close it first'}`.
- /api/headless-setup while a run is active → 409.
- Belt: before deleting SingletonLock, resolve its symlink target pid and
  skip deletion if that pid is alive (best-effort, Linux /proc).

### 23.3 Stop/restart concurrency (audit B2)

- content.js `stop_auto_sync` handler sets ONLY `isSyncStopped = true`;
  `isSyncing` is owned by the wrapper's finally. Removes the
  stop→restart double-pass window.

### 23.4 Popup failure-state + robustness (audit B3/B4/B7)

- popup.js: both sync click handlers call endRun() when the response is
  `{success:false}`.
- content.js: review-check processed-set cleared when the queue reports
  exhausted (end of run); sentCount incremented only after a 2xx
  /api/sync response.

### 23.5 Server correctness batch

- S-a: aligned inline-array matcher quote-aware — `]` inside a quoted value
  no longer terminates the array (scan bracket depth respecting quotes,
  same state machine style as splitInlineArray). Applies to
  parseAlignedArray + parseArrayField inline branch.
- S-b: dismiss-missing vs in-flight result race — serialize reviewMissing
  read-modify-save per ASIN (small promise queue) or tombstone+recheck
  after await; a dismiss during the write window must stick.
- S-c: attach the ChildProcess 'error' listener unconditionally immediately
  after spawn (covers the settled-during-spawn window; no-op when settled).
- S-d: downloadImage buffered fallback also enforces the 5MB cap BEFORE
  buffering (skip when Content-Length missing AND stream API absent, or
  bounded read); plus a Content-Length pre-check test.
- S-e: unescapeInlineValue single-pass `s.replace(/\\(.)/g, '$1')`.
- S-f: setFrontmatterArrayValue write-side: escape `\` and `"` in sibling
  values (reuse the yamlEscape escaping rule).
- S-g: generateReview returns an explicit `{fallback:true}` marker on the
  quota path; regenerate-review trusts the marker, not text emptiness.
- S-h: malformed JSON → 400 (not 500) with static message on /api/sync and
  /api/mark-posted (align with the other routes).
- S-i: HTML routes use parsed pathname (`/?tab=sync` must not 404).
- S-j: analyzeOrder status extraction uses scalarField (escaped quotes).
- S-k: CSV formula guard: also match leading whitespace before =,+,-,@.
- S-l: resolveBrowser cache: invalidate when a spawn fails with ENOENT.
- Dashboard: 409 responses — parse body for login-required before showing
  "already running"; version fallback string 1.8.3.

### 23.6 Ops/docs

- install.sh: guard systemctl calls (command -v) on the install path;
  daemon-reload BEFORE try-restart.
- popup.html version label: read from chrome.runtime.getManifest().version
  (never hardcode again).
- Delete plan.md from the repo root (working notes, not a deliverable).
- README: headless meta list gains loginBlocked (if missing).
- `git worktree prune` after merge (orchestrator).

### 23.7 Tests (append; existing 55 stay green)

56. Auto-fill gate is extension-side (no server test) — instead: parser
    quote-awareness: brand `Foo]Bar` two-item note round-trips aligned (S-a).
57. Dismiss race: start a result post that confirms, dismiss mid-flight
    (hook via a delayed state-write test seam) → dismissed stays dismissed.
58. downloadImage: Content-Length 6291456 + small body → skipped (S-d).
59. Captcha AFTER sighting-1 → streak preserved → next loaded-empty
    confirms (state machine completeness).
60. Non-login error clears a login block (gate correctness).
61. resolveBrowser: two runners with different browserPath values don't
    contaminate the cache; ENOENT spawn invalidates (S-l).
62. Malformed JSON → 400 on /api/sync and /api/mark-posted (S-h).

### 23.8 Acceptance

`npm test` 62/62; audit findings B1–B4, A1–A8 verified by test or smoke;
dashboard headless screenshot; zip rebuilt (no plan.md inside).

---

## 24. v1.8.4 — Residual-nit cleanup (binding)

Version bump: server `1.8.4`, dashboard `1.8.4`, extension manifest `1.8.4`,
`server/package.json` `1.8.4`. Fixes every remaining known nit from both
audits. Selector fragility and the .com redirect gap are accepted as
designed (watchdog backstop) — not bugs.

### 24.1 Server

- **Watchdog strictness**: only SUCCESSFUL work disarms the stall watchdog —
  a 2xx-completed /api/sync order write and /api/client-log lines. A sync
  POST that 400/500s no longer disarms. (Client-log lines are accepted as
  liveness proof; they only exist when the content script is alive.)
- **Single-instance lock**: in `main()` after config load — write
  `<notesPath>/.daemon.lock` `{pid, port, startedAt}`. If it exists and its
  pid is ALIVE (and != ours) → print a clear error ("another daemon (pid N)
  is already serving this notes folder") and exit 1. Stale pid → take over
  silently. Removed on clean SIGTERM/SIGINT exit. `AOS_SKIP_INSTANCE_LOCK=1`
  bypass (tests/dev). OrderStore/HeadlessRunner constructors stay lock-free
  (the test suite builds many in-process).
- **Browser-resolve TTL**: cached resolution re-validates after 60s
  (accessSync re-check, cheap — not a PATH scan; PATH scan only on miss).

### 24.2 Extension

- **Review-check processed-set**: cleared at run START as well as at
  exhaustion (a tab reused after a crashed/killed run can't skip items).
- **Stale auto-sync flag**: the hash-triggered headless branch removes any
  `amazon_sync_auto` sessionStorage flag before starting (a stale flag from
  manual browsing can't double-schedule in a headless tab).
- **extLogBuffer cap**: 1000 lines, drop-oldest (one console note when
  trimming starts) — bounded even if the daemon is down for a whole run.
- **manifest**: `.com` content matches gain `*://*.amazon.com/your-orders*`
  (no trailing slash) alongside the existing patterns.

### 24.3 Tests (append; existing 62 stay green)

63. Watchdog strictness: a malformed (400) sync POST does NOT disarm (run
    still settles browser-stalled on schedule); a client-log line does.
64. Instance lock: second main()-level start on the same notesPath while a
    fake live pid holds the lock → exit 1 with the clear message; stale pid
    → boots; clean shutdown removes the file.
65. TTL: cached resolution older than 60s re-validates (flip the fake
    binary's executability between calls with a stubbed clock or exported
    TTL override).

### 24.4 Docs/version

CHANGELOG 1.8.4; README: instance-lock note (multi-instance behavior +
bypass env), watchdog strictness sentence. Versions 1.8.4 everywhere.

Acceptance: `npm test` 65/65; smoke: instance lock refuse/takeover cycle,
watchdog strictness via curl; zip rebuilt.

---

## 25. v1.8.7 — Headless progress logging + Needs Review pagination (binding)

Version bump: server `1.8.7`, dashboard `1.8.7`, extension manifest `1.8.7`,
`server/package.json` `1.8.7`.

### 25.1 Headless run progress logging (server)

The daemon is currently blind between "started" and "finished". Add:
- **Early browser exit**: if the child exits before the run settles → log
  `Browser exited early (code N) — crash or profile issue` and settle the run
  as `error: 'browser-exited — the sync browser closed unexpectedly'` (with
  history/notify handling like other failures). guidanceFor gains
  browser-exited → "The sync browser crashed or closed on its own." steps:
  1) run SETUP LOGIN and check the window opens and stays open,
  2) if it closes instantly, delete ~/.local/share/amazon-order-sync/
  browser-profile and run SETUP LOGIN again, 3) make sure your browser is
  up to date.
- **Waiting ticks**: every 15s during a run (unref'd interval, cleared on
  settle), log `…waiting for extension: N client lines, M orders (Tsec)`.
  Stop ticking permanently once activity is seen (same disarm condition as
  the stall watchdog).
- **Extension-alive line**: on the FIRST client-log line of a run, log
  `Extension alive in the sync browser.`

### 25.2 Needs Review pagination (dashboard)

Thousands of backfilled items make the tab render thousands of full Review
Studio cards at once.
- Render in batches of 50 (respecting the active ALL/VINE/PAID filter and
  search): first 50 on tab open/filter change, then a `LOAD MORE
  (N REMAINING)` button at the bottom; each click appends the next 50.
  Batch resets to 50 whenever the filter/search/tab changes.
- Selection stays data-driven (`state.selection` Set of ASINs): select-all
  selects ALL matching items (rendered or not); bulk actions iterate the
  selection set regardless of render state; rendered checkboxes reflect the
  set. This must not regress §20.6 (edits persistence across re-renders —
  batching is just another render path through renderReviewCard).
- The VINE QUEUE panel is unaffected (already capped lists).

### 25.3 Tests (append; existing 66 stay green)

67. Early exit: fake browser that exits immediately (`#!/bin/sh\nexit 3`) →
    run settles `error` matching browser-exited, early-exit log line
    present, guidance mapped.
68. Waiting ticks: env-shortened tick interval (e.g.
    AOS_WAIT_TICK_MS=300) + fake browser + no activity → at least one
    `…waiting for extension` line with counters before the watchdog
    settles; first client-log line emits the Extension-alive line and
    stops further ticks.

### 25.4 Docs

CHANGELOG 1.8.7; README: the three new log lines in the headless
troubleshooting section; Needs Review batching note (performance).

Acceptance: `npm test` 68/68; smoke: early-exit + tick lines via fake
browsers; dashboard headless screenshot with >50 seeded items showing LOAD
MORE; zip rebuilt.

---

## 26. v1.8.8 — Scale fix (incremental cache), review cutoff, log UX (binding)

Version bump: server `1.8.8`, dashboard `1.8.8`, extension manifest `1.8.8`,
`server/package.json` `1.8.8`.

### 26.1 Incremental per-file parse cache (the thousands-of-items fix)

Current behavior: any note write invalidates the WHOLE scan cache; the next
/api/orders or /api/check re-parses every note — with thousands of imported
notes this blocks the event loop long enough to starve the extension's
mid-sync check-ins (stall watchdog fires).

- Replace/augment the signature cache with a per-file cache: Map
  relpath → {mtimeMs, size, parsed}. scanNotes walks the tree (cheap),
  parses ONLY new/changed files (async, yielding), drops entries for
  deleted files, and assembles the orders list from cached parses each
  call. Correctness invariants unchanged: after any write, /api/orders
  reflects it (the write's mtime change re-parses exactly that file).
- Export a test seam reporting how many files were (re)parsed on a scan.

### 26.2 Review cutoff (user: "longer than 2 months → not reviewing")

- Config `reviewCutoffDays` — positive int, default 60; 0 disables.
- /api/orders meta gains `reviewCutoffDays`; items gain `beyondCutoff`
  (unposted AND order age > cutoff; posted items never beyondCutoff).
- Review-check queue: unposted beyond-cutoff items are NOT queued (posted
  items still get vote refreshes).
- Dashboard Needs Review: beyond-cutoff items hidden by default with a
  quiet `SHOW OLDER (N)` chip (alongside ALL/VINE/PAID) that reveals them;
  the tab badge and the Overview "items pending review" stat count only
  within-cutoff items. Cutoff hiding composes with the existing filters,
  search, and §25.2 batching.

### 26.3 Log UX

- `POST /api/logs/clear` — empties the ring buffer (keeps monotonic index),
  200 {ok:true}; origin-locked.
- LIVE LOG panel gains `COPY` (copies the visible log text via
  navigator.clipboard, transient "Copied" state) and `CLEAR` (POSTs
  /api/logs/clear + clears the local view; keeps cursor monotonic so only
  NEW lines stream in). Existing PAUSE/CLEAR VIEW stay.
- GUIDANCE box gains a dismiss (×) (per-session, like the stale banner).

### 26.4 Tests (append; existing 68 stay green)

69. Incremental cache: seed 3 orders; scan → 3 parsed; second scan → 0
    re-parsed; touch ONE note → exactly 1 re-parsed and /api/orders shows
    the edit; delete one → dropped from results without re-parse.
70. Cutoff: item from a 90-day-old order + a 10-day-old one, cutoff 60 →
    old item beyondCutoff:true and absent from review-check queue
    (unposted lane), young one present; meta echoes 60; cutoff 0 → no
    beyondCutoff flags.
71. /api/logs/clear → buffer empty, next index preserved, new lines append.

### 26.5 Docs

CHANGELOG 1.8.8; README: reviewCutoffDays key, incremental cache
(performance at thousands of notes), log COPY/CLEAR.

Acceptance: `npm test` 71/71; smoke: cache counters via seam, cutoff queue,
log clear; dashboard headless with >50 old+new items (SHOW OLDER chip, COPY
button); zip rebuilt.

---

## 27. v1.8.9 — Stuck-run recovery (binding)

Version bump: server `1.8.9`, dashboard `1.8.9`, extension manifest `1.8.9`,
`server/package.json` `1.8.9`.

Symptom (user report): after a failed run, the dashboard insists a sync is
already running and nothing can start. Server finally-paths clear
`running`, so the stuck states are: (a) dashboard local busy flag after an
aborted in-flight request, (b) stale dashboard mapping the login-block 409
to "already running", (c) any residual running-without-pending server
inconsistency.

### 27.1 Server — force recovery

- `/api/sync-stop` when `running === true` but `_pending === null`
  (inconsistent state) → clear `running` (+ log `Cleared a stuck run flag
  via sync-stop`) and return 200 `{ok:true, recovered:true}` instead of 409.
- Scheduler-tick consistency sweep: if `running && !_pending &&
  !_activeChild` → clear `running`, log `Cleared a stuck run flag
  (consistency sweep)`.

### 27.2 Dashboard — busy-flag hygiene + honest 409s

- `syncNowBusy` (and the review-check equivalent) reset in BOTH catch and
  finally of the trigger fetch — an aborted/failed request can never wedge
  the button.
- STOP SYNC always POSTs /api/sync-stop and then reloads state, even when
  the local busy flag is the only thing saying "running".
- ALL 409 responses from trigger endpoints display `body.error` verbatim
  when present (falling back to the generic "already running" only when
  the body has none) — the login-block message can never masquerade as
  "already running" again.

### 27.3 Tests (append; existing 71 stay green)

72. Force running=true with _pending=null (test seam) → sync-stop → 200
    `{recovered:true}`, running false; consistency sweep clears the same
    state without a stop call.

### 27.4 Docs

CHANGELOG 1.8.9; README troubleshooting: stuck "already running" → press
STOP SYNC once (it force-recovers) or restart the service.

Acceptance: `npm test` 73/73; smoke of the recovery endpoint; zip rebuilt.

---

## 28. v1.9.1 — Multi-page headless sync (binding)

Version bump: server `1.9.1`, dashboard `1.9.1`, extension manifest `1.9.1`,
`server/package.json` `1.9.1`.

User evidence: with maxOrders=100 the pass stops at 10 because Amazon shows
10 cards per page and the headless pass never paginates — clicking Next
would drop the `#aos-autosync` fragment that triggers the extension.

### 28.1 Per-run token (server)

- The autosync launch URL becomes
  `https://www.amazon.com/your-orders#aos-autosync-<epochMs>` (mirrors the
  v1.8.4 review-check token). Extension gates still match the
  `aos-autosync` substring; the suffix distinguishes a NEW run from a
  pagination continuation of the same run.

### 28.2 Cross-page state (extension)

- sessionStorage key `aos_headless_sync`: `{token, sentCount, runStartMs,
  pageNum}`.
- On hash-trigger start: token mismatch (or absent) → fresh run (clear old
  state, sentCount=0, runStartMs=now, pageNum=1); token match → restore
  counters and CONTINUE (this load is a pagination hop).
- After the current page's cards are exhausted and neither maxOrders nor
  timeBudgetSec is hit: if a next-page link exists
  (`ul.a-pagination li.a-last a`) and pageNum < 20 → save state, log
  `[Page] Moving to orders page N+1…`, BOT_NAV wait, navigate to
  `nextHref + '#aos-autosync-' + token` (absolute or relative resolved
  against location.origin). Otherwise finish: clear the state key, then
  the existing completion ping with the ACCUMULATED sentCount.
- maxOrders and timeBudgetSec therefore span pages (sentCount/runStartMs
  are restored). Loop cap: 20 pages per run.
- The once-guard (`aosHeadlessFired`) must not block a pagination
  continuation on a fresh page load (it's per page load — fine) — but the
  restored-state path is what marks this as continuation.

### 28.3 Tests (append; existing 74 stay green)

75. Launch URL carries the token: fake-browser argv (same capture as test
    66) matches /#aos-autosync-\d+/. (Extension pagination itself is
    browser-side — verified by the client smoke, not node tests.)

### 28.4 Docs

CHANGELOG 1.9.1; README: headless syncs now walk all orders pages (20-page
cap per run) — maxOrders/time budget apply across pages.

Acceptance: `npm test` 75/75; client smoke on a fake 2-page orders site
(state restored, cap spans pages, ping carries accumulated count);
zip rebuilt.

---

## 29. v2.0.0 — Remove the Vine feature set (binding)

User decision: Vine detection was always a $0.00 proxy that can't distinguish
real Vine items from gifts/promos — remove it entirely rather than guess.

### 29.1 Server removals

- parseNote/`/api/orders`: drop the `vine` item field and `isVinePrice`.
  KEEP `parsePriceNumber` (used by price watch and spend math).
- `/api/orders` meta: drop `vineSummary` and `vineWindowDays`.
- Remove Vine escalation: `checkVineEscalation`, its scheduler call, the
  `vineNotified` state map, and its notification path.
- export.csv: drop the `vine` column (header becomes
  `...,reviewed,posted,helpful_votes`); export.json drops the `vine` field.
- Config: `vineWindowDays`/`vineEscalationDays` become inert — loadConfig
  must TOLERATE them in existing config.json files without failing
  (validate-and-ignore or simply stop reading them; no crash either way).
- README: remove Vine features/keys; CHANGELOG gains the 2.0.0 removal entry
  (historical entries stay — they document what shipped then).

### 29.2 Dashboard removals

- VINE badges on Orders rows and Needs Review cards.
- The ALL | VINE | PAID chip rows (both tabs) — filtering reverts to
  search-only; the SHOW OLDER cutoff chip composes with what remains.
- The VINE QUEUE panel (Needs Review) and the VINE COVERAGE stat card
  (Overview; grid reflows to 5 cards).
- Any vine-specific JS state/helpers left orphaned.

### 29.3 Tests (existing 76 must be made green by edit, not deletion of coverage)

- Case 11: drop the vine assertions; keep ALL parsePriceNumber cases
  (US + EU formats) — that function stays.
- Cases 16 (vineSummary) and 29 (escalation): remove; renumber not required
  (TAP doesn't care about titles), keep the count honest in comments.
- Cases asserting the export.csv header (6, 13-family) and any meta
  vineSummary/vine assertions: update to the new shapes.
- Sweep the file for `vine` (case-insensitive) — no live assertions remain.
- New case 75-ish (append as 77): config with legacy vine keys → server
  boots fine, meta has NO vineSummary, orders items have NO vine field,
  export.csv header has no vine column.

### 29.4 Version

server `2.0.0`, dashboard `2.0.0`, extension manifest `2.0.0` (label only —
no extension behavior was Vine-specific), `server/package.json` `2.0.0`.

Acceptance: `npm test` all green with zero vine references; grep of
server.js + dashboard.html shows no live vine code paths (historical
CHANGELOG prose excepted); zip rebuilt.
