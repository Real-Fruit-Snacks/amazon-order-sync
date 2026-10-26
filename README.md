# Amazon Order Sync (standalone)

Sync your Amazon order history to a local folder of Markdown notes using a tiny
zero-dependency Node.js server and a companion Chrome extension. Designed for
heavy Amazon shoppers: it extracts your orders,
caches product images locally, generates AI-powered review drafts with Gemini,
and serves a built-in spending dashboard, all on localhost with no cloud
service in between.

This is the standalone port of the original Obsidian plugin. Nothing except
Node.js 18+ and Chrome is required.

**Features**

- One Markdown note per order, with frontmatter arrays and per-item review blocks
- Local caching of product images
- AI review drafts via Gemini, with deterministic per-ASIN star-rating
  variation and rating-aware prompts (1-5 stars)
- Review-context mining: the extension scrapes top customer reviews during
  sync and the server feeds them to Gemini as untrusted reference material
- Review Studio in the dashboard: edit draft title/body, pick a star rating
  and tone preset (balanced/concise/enthusiastic/critical), save or regenerate
  drafts in place, plus bulk mark-posted/regenerate with aging badges
- Helpful-vote tracking with a "most helpful reviews" leaderboard
- Backfill import of your full Amazon order history ("Request My Data" CSV
  exports), deduplicated against existing notes
- CSV/JSON exports of your entire order and review history
- Works on amazon.com, amazon.co.uk, amazon.de, amazon.ca, amazon.fr,
  amazon.it, amazon.es, amazon.co.jp, and amazon.com.au
- Built-in spending dashboard, localhost-only, zero npm dependencies
- Headless auto-sync: the daemon drives headless Chromium with the real
  extension on a schedule — no browser window needed after a one-time login

## Architecture

1. **Local server** (`server/server.js`): a single-file Node.js daemon listening
   on `http://127.0.0.1:27483`. It receives order data from the extension,
   writes one Markdown note per order under `~/AmazonOrders/Orders/YYYY/MM-Month/`,
   downloads product images into `Orders/assets/`, optionally drafts reviews
   with the Gemini API, and serves the dashboard at `http://127.0.0.1:27483/`.
2. **Chrome extension** (`browser-extension/`): runs only on Amazon order and
   review pages. It scrapes your order history, posts it to the local server,
   and auto-fills Amazon review forms from your saved review drafts.

## Install (Windows)

See [README-WINDOWS.md](README-WINDOWS.md): run `install.ps1`, then load
`browser-extension/` in Brave. A native tray app (`AmazonOrderSync.exe`)
replaces the systemd service and desktop notifications.

## Install (CachyOS / Arch)

```bash
sudo pacman -S nodejs   # Node.js >= 18, the only dependency
./install.sh
```

The installer copies the server to `~/.local/share/amazon-order-sync/server/`
and the companion extension to
`~/.local/share/amazon-order-sync/browser-extension/`, installs and starts a
systemd **user** service (`amazon-order-sync.service`), creates a default
config if missing, and verifies the server is healthy. It is idempotent, so
re-running it upgrades in place.

Then load the extension:

1. Open `chrome://extensions` in Chrome.
2. Enable **Developer mode**.
3. Click **Load unpacked** and select
   `~/.local/share/amazon-order-sync/browser-extension/` (loading from the
   installed copy means extension updates land with every `./install.sh`).

## Configuration

Config file: `~/.config/amazon-order-sync/config.json`
(override with the `AMAZON_SYNC_CONFIG` environment variable). Created on first
run with mode `0600`.

```json
{
  "notesPath": "~/AmazonOrders",
  "geminiApiKey": "",
  "port": 27483,
  "syncReminderDays": 7,
  "autoSyncEnabled": false,
  "autoSyncIntervalHours": 24,
  "browserPath": "",
  "notificationsEnabled": true,
  "reviewCheckIntervalDays": 7,
  "reviewCutoffDays": 60,
  "geminiDailyLimit": 1000
}
```

| Key                     | Meaning                                                                 |
| ----------------------- | ----------------------------------------------------------------------- |
| `notesPath`             | Folder for your notes. Absolute or `~`-prefixed.                         |
| `geminiApiKey`          | Gemini API key for AI review drafts. Empty falls back to `GEMINI_API_KEY`. |
| `port`                  | Listen port. Defaults to `27483`; the extension expects this port.       |
| `syncReminderDays`      | Optional. Days without a sync before the dashboard reminds you. Default `7`. |
| `autoSyncEnabled`       | Optional. Enable scheduled headless auto-sync. Default `false`.          |
| `autoSyncIntervalHours` | Optional. Positive int, hours between headless syncs. Default `24`.      |
| `browserPath`           | Optional. Explicit browser binary; empty = autodetect on `PATH`.         |
| `notificationsEnabled`  | Optional. Desktop notifications via `notify-send`. Default `true`.      |
| `reviewCheckIntervalDays` | Optional. Days between helpful-vote/review checks. Default `7`.        |
| `reviewCutoffDays`        | Optional. Unposted items from orders older than this are flagged `beyondCutoff`, hidden in Needs Review behind a **SHOW OLDER** chip, and skipped by the review-check queue (posted items are unaffected). Default `60`; `0` disables. |
| `geminiDailyLimit`      | Optional. Positive int, daily Gemini request budget. Default `1000` — set it to your AI Studio tier's daily quota. |
| `geminiModel`           | Optional (v2.1.0). Gemini model id for drafts. Default `gemini-3.1-flash-lite`; try `gemini-3.1-flash` for more specific drafts at higher cost per request. |
| `geminiTemperature`     | Optional (v2.1.0). 0–2, default `0.9`. Higher = more varied phrasing. |
| `styleExemplars`        | Optional (v2.1.0). Default `true`: your 3 most-helpful posted reviews are fed to Gemini as voice samples so drafts sound like you. `false` disables. |

### Single-instance lock (v1.8.4)

One daemon per notes folder. At startup the server writes
`<notesPath>/.daemon.lock` (`{pid, port, startedAt}`); if a lock exists and
its pid is still alive, the new daemon refuses to start
(`another daemon (pid N) is already serving this notes folder`, exit 1)
instead of racing the first one's writes. A lock left behind by a crashed or
killed daemon is stale (dead pid) and is taken over silently; a clean
SIGTERM/SIGINT exit removes the file. Different notes folders (different
configs) can still run side by side. Set `AOS_SKIP_INSTANCE_LOCK=1` to
bypass the lock entirely (dev/tests).

### Performance at scale (v1.8.8)

The server keeps an incremental per-file parse cache for your notes: each
scan walks the note tree (cheap) and re-parses only notes that are new,
changed (mtime/size), or deleted — a single write no longer re-parses every
note. With thousands of backfilled orders, `/api/orders` and the headless
review-checker stay fast and the event loop stays responsive mid-sync.
Correctness is unchanged: after any write, the next read reflects it.

### Gemini API key

Get a key from [Google AI Studio](https://aistudio.google.com/apikey) and either
put it in `geminiApiKey` in the config or export `GEMINI_API_KEY` for the
service. Without a key, everything still works; AI review generation is simply
skipped.

The daemon counts every actual Gemini API attempt (rate-limit retries
included) in `<notesPath>/.gemini-usage.json`, resetting at midnight. When the
count reaches `geminiDailyLimit` (default `1000` — set it to your AI Studio
tier's daily quota) review drafts fall back to templates until the day rolls
over; a warning is logged once per run from 80% usage. The dashboard's SYNC
tab shows live usage.

## Usage

1. Open your Amazon **order history** page.
2. Click the extension icon and press **Sync Current Page**, or **Auto-Sync All
   Pages** to walk every page automatically.
3. Orders appear as Markdown notes under `~/AmazonOrders/Orders/`, one per
   order, with frontmatter arrays (brand, price, asin, url, reviewed, rating)
   and a per-item review section.
4. Open the dashboard at <http://127.0.0.1:27483/>.

### Dashboard

The dashboard is a self-contained page that reads `GET /api/orders` (a JSON
export of every order note, rescanned on each request) and renders four tabs:

- **Overview**: total orders, lifetime spend, pending/ready/posted review
  counts, a 12-month spending chart, recent activity, and **EXPORT** links
  (CSV/JSON) that download your full order history. Once posted reviews have
  helpful votes, a **REVIEW PERFORMANCE** panel charts votes per review tinted
  by the generation tone and names your most helpful tone and rating — the
  Review Studio then defaults its tone picker to your best-performing tone
  (explicit choices always win).
- **Sync freshness**: when your notes are older than `syncReminderDays`, an
  amber banner on every tab reminds you to re-run a sync; otherwise a quiet
  "synced Xd ago" label sits in the topbar.
- **Orders**: every order grouped by year and month with live search (press
  `/` to focus), status pills, per-item thumbnails, and state badges. Posted
  items can be flipped back with **Mark not posted**.
- **Needs Review** (Review Studio): every unposted item with its AI draft in
  an editable form — title field, auto-growing body, and a 1-5 star picker.
  **SAVE DRAFT** writes your edits into the note, **REGENERATE** produces a
  fresh AI draft at the chosen rating, plus **Copy review**, **Leave review
  on Amazon**, and **Mark posted**. For large backfilled inboxes the tab
  renders in batches of 50 (v1.8.7, respecting the filter/search) with a
  **LOAD MORE** button — select-all and bulk actions still cover every
  matching item, rendered or not.

### Backfill import (full order history)

To import your complete Amazon order history (not just what the extension can
scrape): go to your Amazon account, choose **Request My Data** → **Your
Orders**, wait for the confirmation email, download and unzip the archive,
then upload the `Retail.OrderHistory.*.csv` file(s) via the **IMPORT** button
in the dashboard's Overview topbar (or `POST /api/import-backfill` directly).
Each order becomes a normal note marked `backfill: true`; orders that already
have a note are skipped, never overwritten.

### Headless auto-sync

The daemon can sync your orders by itself, on a schedule, by driving headless
Chromium with the real extension loaded — no cookie or HTTP scraping, and no
browser window after a one-time login.

How it works:

- A one-time **visible** setup window captures your Amazon login into a
  dedicated profile at `~/.local/share/amazon-order-sync/browser-profile/`.
- Every 30 minutes the server checks whether a sync is due
  (`autoSyncIntervalHours` since the last run) and, if so, launches headless
  Chromium with that profile and the extension; the extension syncs one pass
  of your orders page and pings the server when done (6-minute timeout).
- Results (`lastRunAt`, `lastResult`, `lastError`, `ordersSynced`) persist in
  `<notesPath>/.headless-sync.json` and show up in the dashboard.
- Since v1.9.1 a headless run walks **all orders pages**, not just the first
  (capped at 20 pages per run): the extension carries a per-run token in the
  page hash and keeps cross-page counters in sessionStorage, so `maxOrders`
  and the time budget apply across pages and the completion ping reports the
  accumulated total.

Setup:

1. Install a browser (`sudo pacman -S chromium`, or set `browserPath`).
2. Run `node ~/.local/share/amazon-order-sync/server/server.js --setup-headless`
   (or press **SETUP LOGIN** in the dashboard when offered) and log into
   Amazon in the window that opens, then close it.
3. Set `"autoSyncEnabled": true` in the config and restart the service
   (`systemctl --user restart amazon-order-sync`). Trigger a manual run any
   time with the dashboard's **SYNC NOW** button or `POST /api/sync-now`.
   During a run the button becomes **STOP SYNC**, aborting the run via
   `POST /api/sync-stop`.

**Security note:** the dedicated browser profile holds a logged-in Amazon
session. It never leaves your machine — it lives only under
`~/.local/share/amazon-order-sync/browser-profile/` and is used solely by the
local headless runs. Uninstalling preserves it (delete it manually if you want
the session gone).

#### Login-block gate (v1.8.2)

When a headless run reports `login-required` (your Amazon session expired),
sync locks down until you log in again:

- **Buttons lock.** START SYNC, REVIEW CHECK NOW, and the Overview SYNC NOW
  are disabled with an explaining tooltip; an amber `LOG IN REQUIRED — RUN
  SETUP LOGIN FIRST` hint shows on the SYNC tab and SETUP LOGIN is
  highlighted as the required action.
- **The scheduler pauses.** No automatic retries (and no repeated failure
  notifications) while the session is known-expired — the skip is logged at
  most once per day.
- **SETUP LOGIN clears it.** Pressing **SETUP LOGIN** (200) resets the gate
  immediately and unlocks the buttons; the next run then gets a chance, and
  any successful run keeps the gate clear. The API equivalents return
  `409 {error:'login-required — run SETUP LOGIN first', guidance}` while
  blocked.

#### Troubleshooting headless stalls (v1.8.1)

A stalled headless run used to be silent: zero extension log lines, zero
orders, ending only when you pressed **STOP SYNC** or the 6-minute timeout
fired. Three defenses now catch each root cause:

- **Expired Amazon session → `login-required`.** Amazon redirects the run to
  `/ap/signin`; the extension now matches that page, recognizes the run's
  `#aos-autosync` / `#aos-reviewcheck` fragment (fragments survive the
  redirect), and reports `login-required` within seconds. Fix: run
  **SETUP LOGIN** and log in again.
- **Stale browser profile locks.** A SIGTERM/SIGKILLed headless browser can
  leave `SingletonLock`/`SingletonSocket`/`SingletonCookie` behind, and the
  next launch stalls waiting on the dead owner's lock. The server deletes
  those artifacts before every spawn (logged when it does) — no action
  needed.
- **Any other silent stall → `browser-stalled`.** If 45s pass with zero
  extension log lines AND zero orders synced, the run aborts with
  `browser-stalled — no extension activity after 45s`. Since v1.8.4 only
  *successful* work disarms this watchdog — a completed (2xx) order write or
  extension log lines; a sync POST that fails (400/500) no longer counts as
  activity. Fix, in order:
  1. run **SETUP LOGIN** and confirm the orders page loads while logged in,
  2. if it persists, delete `~/.local/share/amazon-order-sync/browser-profile`
     and run **SETUP LOGIN** again,
  3. make sure Brave Shields are down for amazon.com in the setup window.

#### Headless run progress logging (v1.8.7)

A headless run is no longer blind between "started" and "finished" — three
new log lines (daemon stdout and the dashboard LIVE LOG) show what it is
doing:

- `…waiting for extension: N client lines, M orders (Ts)` — every 15s while
  the run waits for the extension, with the live activity counters and
  elapsed seconds. The ticks stop permanently once the extension shows any
  activity (the same condition that disarms the stall watchdog).
- `Extension alive in the sync browser.` — logged once per run, on the
  first log line forwarded by the extension: proof the content script is
  up.
- `Browser exited early (code N) — crash or profile issue` — the sync
  browser process quit before the run finished. The run settles within
  seconds as `browser-exited — the sync browser closed unexpectedly`
  (instead of waiting out the watchdog) with guidance: run **SETUP LOGIN**
  and check the window opens and stays open; if it closes instantly,
  delete `~/.local/share/amazon-order-sync/browser-profile` and run
  **SETUP LOGIN** again; make sure your browser is up to date.

#### Stuck "already running" (v1.8.9)

If the dashboard insists a sync is already running and nothing can start
(a wedged busy flag or a residual server inconsistency after a failed
run), recovery is one click: **press STOP SYNC once.** It always POSTs
`/api/sync-stop` — even when only the local button state says "running" —
and the server force-clears a stuck run flag, answering
`200 {ok:true, recovered:true}` (logged as
`Cleared a stuck run flag via sync-stop`) instead of a 409 that never
clears. A scheduler-tick consistency sweep also clears the same state
automatically (`Cleared a stuck run flag (consistency sweep)`), and all
409 responses now show the server's real error text (a login block can no
longer masquerade as "already running"). If all else fails, restart the
service (`systemctl --user restart amazon-order-sync`).

### Price watch, sync health, and repeat purchases

- **Price watch**: when the extension includes the live listing price on a
  sync, the server records it in `<notesPath>/.price-history.json` (one entry
  per day per ASIN, 90-entry cap). The dashboard's **PRICE WATCH** panel
  shows items currently listed below what you paid (`PAID $X → NOW $Y · −Z%`).
- **Sync health**: every headless run (scheduled or manual) is recorded in
  the state file's `history` (newest first, cap 20); the dashboard's
  **SYNC HEALTH** panel shows the last 10 with duration and errors.
- **Repeat purchases**: items ordered in more than one order get a muted
  `REPEAT ×N` badge (`repeatCount` in `/api/orders`).

### Desktop notifications

With `notificationsEnabled` (default `true`) the server fires
`notify-send` (must be installed and on `PATH` — e.g. `libnotify`) for:

- headless syncs that finished with orders synced (`N orders synced`),
- headless run failures (with the error).

### SYNC tab (sync control room)

The dashboard's **SYNC** tab (OVERVIEW | ORDERS | SYNC | NEEDS REVIEW) is the
control room for headless runs:

- **CONTROLS** — pick the mode and limits, then **START SYNC** (the same
  button row also has **REVIEW CHECK NOW** and, during a run, **STOP SYNC**):
  - `QUICK (skip completed)` skips orders whose note is already complete —
    no detail/vote fetches for them; `FULL` reprocesses everything.
  - `MAX ORDERS` caps how many orders one pass posts (`0` = all).
  - `TIME BUDGET (s)` stops the pass cleanly after that many seconds
    (`0` = no budget).
  - `REFRESH PRICES` / `REFRESH REVIEW STATUS+VOTES` / `REFRESH DETAILS`
    (all on by default): untick to skip those fetches; the server keeps the
    previously stored values.
  The options are posted as the `/api/sync-now` body and echoed back by
  `GET /api/run-options` (the extension reads them once at headless-run
  start; review-check runs ignore them). The last used options persist in
  the state file's `lastOptions`.
- **LIVE LOG** — the daemon's own log lines, polled from
  `GET /api/logs?after=N` every 2 s while the tab is visible, with local
  PAUSE / CLEAR VIEW toggles plus **COPY** (copies the visible log text)
  and **CLEAR** (v1.8.8: `POST /api/logs/clear` empties the daemon's ring
  buffer; only new lines stream afterwards). During headless runs the
  extension forwards its scrape progress too, shown inline with an `[ext]`
  prefix.
- **RUN STATUS + HISTORY** — current run state plus the 20-entry persisted
  run history (CANCELLED amber, ERROR red).
- **GUIDANCE** — when a run fails with a known error (expired login, no
  browser, timeout, Gemini budget), the plain-language fix steps from the
  `headless.guidance` meta.
- **GEMINI USAGE** — `used / limit` for the day with a progress bar;
  hidden when no API key is configured.

### Review-checker

A lightweight headless pass that refreshes helpful-vote counts and detects
reviews you posted from other devices — without a full order sync. It uses
the same login profile and the same HeadlessRunner lifecycle as auto-sync,
so setup is identical (install a browser, log in once, `autoSyncEnabled`).
Every `reviewCheckIntervalDays` (default 7) the server queues posted items
whose votes are stale plus unposted items from orders older than 2 days
(cap 20) and walks their Amazon review pages headlessly. Unposted items
beyond `reviewCutoffDays` (default 60, `0` disables) are skipped — posted
items still get their vote refreshes. Trigger a check manually with
`POST /api/review-check-now`.

**Removed-review detection (v1.8.0).** Each visited review page also reports
a `page_state`. When a page that provably LOADED shows an empty review form
for an item your notes say is posted (with review content), the server
records a sighting; a captcha wall or error page is inconclusive and records
nothing. **Two consecutive empty sightings** confirm the removal: you get
one desktop notification, the dashboard shows a red `REVIEW MISSING` badge
(Orders rows and Needs Review cards), and `/api/orders` meta lists the item
under `missingReviews`. A found review clears the flag, as does the badge's
dismiss (×) action (`POST /api/dismiss-missing`). Your note is never
modified — the review text stays, ready to repost.

### API endpoints

| Route                        | Purpose                                                          |
| ---------------------------- | ---------------------------------------------------------------- |
| `GET /api/check?order_id=`   | Whether an order note exists and is complete (extension).        |
| `GET /api/get-review?asin=`  | Fetch one item's saved review draft (extension auto-fill).       |
| `POST /api/sync`             | Create/update an order note (5 MB limit).                        |
| `GET /api/orders`            | Parsed JSON of every order note (dashboard). Meta includes `lastSyncAt`, `syncReminderDays`, `priceDrops`, `missingReviews` (confirmed removed reviews: `asin`, `name`, `orderId`, `since`), `gemini` (`used`, `limit`, `remaining`, `day`), `reviewCutoffDays`, and `headless` (`enabled`, `running`, `loginBlocked`, `runStartedAt`, `lastRunAt`, `lastResult`, `lastError`, `ordersSynced`, `intervalHours`, `browserFound`, `history`, `lastReviewCheckAt`, `lastOptions`, `guidance`); items include `helpfulVotes`, `tone`, `listingPrice`, `repeatCount`, `reviewMissing`, and `beyondCutoff`. |
| `POST /api/mark-posted`      | Tick/untick an item's `Posted to Amazon?` checkbox.              |
| `POST /api/save-review`      | Rewrite one item's draft title/body, checkbox preserved.         |
| `POST /api/regenerate-review`| Regenerate the AI draft at a chosen rating; optional `tone` (`balanced`, `concise`, `enthusiastic`, `critical`; default `balanced`). |
| `POST /api/import-backfill`  | Import "Request My Data" CSV exports (raw `text/csv` or JSON `{files:[...]}`, 50 MB limit). |
| `GET /api/export.csv`        | Flat RFC-4180 CSV of all items (download).                       |
| `GET /api/export.json`       | Full parsed structure incl. review text (download).              |
| `POST /api/sync-now`         | Trigger a headless sync; responds when the run finishes (409 while running). Optional JSON body `{mode, maxOrders, timeBudgetSec, refresh:{prices,votes,details}}` — invalid fields fall back to defaults individually. |
| `GET /api/run-options`       | The in-flight run's effective options, or the last used ones (`{options}`). |
| `GET /api/logs?after=N`      | Daemon log ring buffer `{lines:[{i,t,text}], next}`; no cursor → last 100. |
| `POST /api/logs/clear`       | Empty the log ring buffer (200 `{ok:true}`); the monotonic index keeps running, so cursors stay valid and only new lines stream. |
| `POST /api/sync-stop`        | Abort the active headless run as `cancelled` (409 when idle).  |
| `POST /api/headless-complete`| Completion ping from the headless-run extension (409 when idle). |
| `POST /api/headless-setup`   | Open a visible browser window for the one-time Amazon login (clears the login-block gate, v1.8.2). |
| `GET /api/review-check-queue`| ASINs due for a vote refresh / posted-review detection (cap 20). |
| `POST /api/review-check-result` | Apply one review-check scrape: real review -> note + checkbox + votes, or votes-only. Optional `page_state` (`loaded`/`captcha`/`error`, default `loaded`) drives removed-review detection; non-loaded pages record nothing. |
| `POST /api/dismiss-missing`  | Clear one missing-review flag `{asin}` (re-detection starts fresh; 404 unknown). |
| `POST /api/review-check-now` | Trigger a review check; instant ok on an empty queue, 409 while any run is active. |
| `POST /api/client-log`       | Extension log forwarding: `{lines:[1..100 strings]}`, appended to the daemon log ring buffer with an `[ext] ` prefix. |
| `GET /api/health`            | `{"ok":true,"version":"1.8.2"}`.                                 |

Four supporting routes: `GET /assets/<file>` serves the cached product images
from `Orders/assets/`, `GET /fonts/<file>` serves the bundled typeface, and
`GET /favicon.ico` / `GET /icons/<file>` serve the bundled dashboard icons.

The dashboard is themed with the [Terminal Workbench](https://github.com/Real-Fruit-Snacks/terminal-workbench-suite)
design system (MIT) and bundles the Terminal Workbench Mono typeface (OFL 1.1,
see `server/fonts/OFL.txt`), served from `GET /fonts/<file>`.

### Review auto-fill flow

1. The server drafts a review for each unreviewed, undelivered-filtered item
   when a Gemini key is configured (or write your own in the note, replacing
   the `> *Write your review for this item below...*` placeholder).
2. From the dashboard's "Needs Review" inbox (or the note itself), click the
   **Leave a Review on Amazon** link for the item.
3. The extension detects the Amazon review page, fetches the saved draft from
   the local server, and fills in the title, text, and star rating.
4. Submit on Amazon. No manual bookkeeping: on the next sync, the extension
   checks Amazon's review page for every item not yet marked posted, imports
   any review it finds (title, text, rating), and the server ticks
   `- [x] Posted to Amazon?` itself. Re-syncing never overwrites an existing
   review or a checked box.

## Security model

- **Origin lock:** the server only answers requests carrying an Amazon Origin
  header (`amazon.com`, `amazon.co.uk`, `amazon.de`, `amazon.ca`,
  `amazon.fr`, `amazon.it`, `amazon.es`, `amazon.co.jp`, `amazon.com.au`,
  or any subdomain), same-origin requests from the dashboard itself
  (`127.0.0.1`/`localhost` on the same port), and origin-less local tooling
  such as curl. Every other origin gets `403` before any processing, so no
  other website open in your browser can read your orders or write files.
- **Localhost only:** the server binds `127.0.0.1` and is unreachable from the
  network.
- **Config permissions:** the config file (which may hold your Gemini key) is
  created with mode `0600`.
- **Path safety:** order IDs and ASINs arriving over HTTP are sanitized before
  any filesystem use.
- **Exact route matching (v1.8.0):** every `/api/` route matches the parsed
  URL pathname exactly — lookalike paths such as `/api/checkAnything` get a
  404, not an alias of a real route.
- **Image download guards (v1.8.0):** product images are fetched over http(s)
  only, the response must carry an `image/*` Content-Type from the same
  whitelist the asset server will serve (the saved extension is derived from
  the Content-Type, not the URL), and bodies are capped at 5 MB — anything
  else is skipped.
- Product names, brands, and feature text are sent to the Gemini API only when
  you configure a key. Nothing else ever leaves your machine.

## Uninstall

```bash
./install.sh --uninstall
```

Stops and disables the service and removes the installed server and extension
files. Your notes folder, config file, and the headless browser profile (your
Amazon login) are never touched.

## Development

Run the test suite (spins the real server on an ephemeral port against a tmp
notes folder; needs no network access and no API key):

```bash
cd server && npm test
```

Quality gates:

```bash
node --check server/server.js
bash -n install.sh
```

The server has zero npm dependencies and uses only the Node.js standard
library (`http`, `fs`, `path`, `os`, `crypto`, `child_process`).

## License

MIT
