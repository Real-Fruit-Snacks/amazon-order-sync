# Amazon Order Sync guide

Everything the README leaves out: configuration, the dashboard, headless sync, review drafts, Windows and Linux specifics, and troubleshooting. The HTTP API is in [api.md](api.md).

## Windows and Linux

| Linux                                   | Windows                                                        |
| --------------------------------------- | -------------------------------------------------------------- |
| `systemctl --user` service              | Tray app starts/stops `node server\server.js` (hidden window)  |
| `notify-send` toasts                    | Server queues toasts at `/api/notifications`; tray shows them  |
| `~/.config/amazon-order-sync/config.json` | `%APPDATA%\amazon-order-sync\config.json`                    |
| `~/.local/share/amazon-order-sync/`     | `%LOCALAPPDATA%\amazon-order-sync\` (browser profile, logs)    |
| Chromium, Brave, Edge, then Chrome on `PATH` | Brave → Edge → Chromium → Chrome, PATH then install folders |
| `journalctl --user -u …`                | `%LOCALAPPDATA%\amazon-order-sync\server.log` (+ `tray.log`)   |

## Installing

The README has the install commands. What the installers do:

**Windows, `install.ps1`** (Windows PowerShell 5.1 or PowerShell 7, run from the folder the release zip was expanded into):

1. Checks for Node.js 18 or newer and offers to install the LTS with winget.
2. Creates `%APPDATA%\amazon-order-sync\config.json` on first run, with notes in `%USERPROFILE%\AmazonOrders`. An existing config is never changed.
3. Registers `AmazonOrderSync.exe` to start with Windows (the `AmazonOrderSync` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) and puts a shortcut on the desktop.
4. Stops any running server and tray app, starts the new tray app and waits for `/api/health`.

Options: `-NotesPath D:\Somewhere` (new configs only), `-NoAutostart`, `-Uninstall`. Rerun it after expanding a newer release over the same folder.

**Linux, `install.sh`**: copies `server/` and `browser-extension/` to `~/.local/share/amazon-order-sync/`, installs and starts the systemd user service `amazon-order-sync.service`, creates `~/.config/amazon-order-sync/config.json` (mode 0600) if missing, and checks `/api/health`. It is idempotent; rerun it to upgrade. Load the extension from `~/.local/share/amazon-order-sync/browser-extension/` so upgrades reach it.

**The extension** has a fixed ID, `cikcnmangaegpkglmcfgbfbmknjmdifh`, set by the `key` in its manifest: the server accepts requests only from that ID. Load it unpacked, and press its reload button on the extensions page after an upgrade.

### Building from source

The server and extension need no build. The tray app needs the .NET 8 SDK or newer:

```powershell
dotnet publish tray -c Release -o .
```

That writes `AmazonOrderSync.exe` to the repo root, next to `server\` and `browser-extension\`, which is where it looks for them (or set `AOS_SERVER_DIR`). Tests: `node --test tests/server.test.js`. Icons: `python3 tools/make-icons.py`, with a clone of Real-Fruit-Snacks/rfs-brand next to this repo.

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
| `geminiModel`           | Optional. Gemini model id for drafts. Default `gemini-3.1-flash-lite`; try `gemini-3.1-flash` for more specific drafts at higher cost per request. |
| `geminiTemperature`     | Optional. 0–2, default `0.9`. Higher = more varied phrasing. |
| `styleExemplars`        | Optional. Default `true`: your 3 most-helpful posted reviews are fed to Gemini as voice samples so drafts sound like you. `false` disables. |

### Single-instance lock

One daemon per notes folder. At startup the server writes
`<notesPath>/.daemon.lock` (`{pid, port, startedAt}`); if a lock exists and
its pid is still alive, the new daemon refuses to start
(`another daemon (pid N) is already serving this notes folder`, exit 1)
instead of racing the first one's writes. A lock left behind by a crashed or
killed daemon is stale (dead pid) and is taken over silently; a clean
SIGTERM/SIGINT exit removes the file. Different notes folders (different
configs) can still run side by side. Set `AOS_SKIP_INSTANCE_LOCK=1` to
bypass the lock entirely (dev/tests).

### Performance at scale

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
  renders in batches of 50 (respecting the filter/search) with a
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
- A headless run walks **all orders pages**, not just the first
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

#### Login-block gate

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

#### Troubleshooting headless stalls

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
  `browser-stalled — no extension activity after 45s`. Only
  *successful* work disarms this watchdog — a completed (2xx) order write or
  extension log lines; a sync POST that fails (400/500) no longer counts as
  activity. Fix, in order:
  1. run **SETUP LOGIN** and confirm the orders page loads while logged in,
  2. if it persists, delete `~/.local/share/amazon-order-sync/browser-profile`
     and run **SETUP LOGIN** again,
  3. make sure Brave Shields are down for amazon.com in the setup window.

#### Headless run progress logging

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

#### Stuck "already running"

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
  and **CLEAR** (`POST /api/logs/clear` empties the daemon's ring
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

**Removed-review detection.** Each visited review page also reports
a `page_state`. When a page that provably LOADED shows an empty review form
for an item your notes say is posted (with review content), the server
records a sighting; a captcha wall or error page is inconclusive and records
nothing. **Two consecutive empty sightings** confirm the removal: you get
one desktop notification, the dashboard shows a red `REVIEW MISSING` badge
(Orders rows and Needs Review cards), and `/api/orders` meta lists the item
under `missingReviews`. A found review clears the flag, as does the badge's
dismiss (×) action (`POST /api/dismiss-missing`). Your note is never
modified — the review text stays, ready to repost.

### Review drafts and auto-fill

A draft is a starting point. Nothing is posted for you: you rewrite it from
your own experience of the product, set the rating you'd give it, and submit
it on Amazon yourself.

1. With a Gemini key configured, the server drafts a review for each
   delivered item that has none. Without a key, or to start from scratch,
   type under the `> *Write your review for this item below...*` line in the
   note; the placeholder line is dropped on the next sync and your text kept.
2. Rewrite the draft in the dashboard's Needs Review tab (title, text and
   star rating) and press **SAVE DRAFT**, or edit the note.
3. Press **Leave review on Amazon**. On Amazon's review page the extension
   fills in your saved title, text and rating; check them, then submit.
4. On the next sync the extension finds the posted review, imports what you
   actually posted (title, text, rating) and ticks `- [x] Posted to Amazon?`.
   Re-syncing never overwrites an existing review or a checked box.

## Windows tray menu

- **Open dashboard** (also double-click the icon) — `http://127.0.0.1:27483/`
- **Sync now** — headless run; completion/failure arrives as a toast
- **Review your purchases on Amazon** — Amazon's review page, where the extension fills in your saved drafts
- **Set up Amazon login…** — visible browser window on the dedicated profile
- **Open notes folder / Edit config.json / View server log**
- **Open browser-extension folder** — for *Load unpacked*
- **Start with Windows** — toggles the `HKCU\…\Run` entry
- **Restart server** — after editing `config.json`
- **Quit** — asks the server to shut down cleanly (`POST /api/shutdown`),
  which also kills any headless browser and releases the `.daemon.lock`

If the tray app starts and a server is already answering on the port (say an
orphan from a crashed tray) it attaches to it instead of starting a second
daemon on the same notes folder; **Quit** stops that server too. If the server
crashes, the tray restarts it, at most twice in five minutes; after that it
shows a notification and leaves it stopped until you fix the cause (see
`server.log`) and use **Restart server**.

## Troubleshooting on Windows

- **"Node.js was not found"** — install it (`winget install OpenJS.NodeJS.LTS`),
  then **Restart server** from the tray.
- **Server exits immediately** — read `server.log`. Usually the port is in use
  or `config.json` has a typo.
- **"No browser found" toast** — Brave/Edge/Chromium isn't in a standard place;
  set `"browserPath": "C:\\Users\\<you>\\AppData\\Local\\BraveSoftware\\Brave-Browser\\Application\\brave.exe"`.
- **Headless sync stalls / login errors** — tray → **Set up Amazon login…**
  again. To start over, quit the tray and delete
  `%LOCALAPPDATA%\amazon-order-sync\browser-profile`.
- **Toasts not showing** — Windows Focus Assist / notification settings for
  "AmazonOrderSync"; the balloon tips use the standard Windows toast channel.

## Security model

- **Host check:** a request whose `Host` header isn't `127.0.0.1:<port>` or
  `localhost:<port>` gets `403`. This stops DNS rebinding, where a website
  re-points its own name at 127.0.0.1 to read the API as a "same-origin" page.
- **Origin lock:** of the rest, the server answers only the dashboard's own
  page (an Origin of `http://127.0.0.1:<port>` or `http://localhost:<port>`),
  the extension (`chrome-extension://cikcnmangaegpkglmcfgbfbmknjmdifh`), and
  origin-less local tools such as curl and the tray app. The extension makes
  every request from its service worker, so pages on Amazon can't use the
  extension's access: their scripts carry an Amazon Origin, which gets `403`.
- **Localhost only:** the server binds `127.0.0.1` and is unreachable from the
  network.
- **Config permissions:** the config file (which may hold your Gemini key) is
  created with mode `0600`.
- **Path safety:** order IDs and ASINs arriving over HTTP are sanitized before
  any filesystem use.
- **Exact route matching:** every `/api/` route matches the parsed
  URL pathname exactly — lookalike paths such as `/api/checkAnything` get a
  404, not an alias of a real route.
- **Image download guards:** product images are fetched over http(s)
  only, the response must carry an `image/*` Content-Type from the same
  whitelist the asset server will serve (the saved extension is derived from
  the Content-Type, not the URL), and bodies are capped at 5 MB — anything
  else is skipped.
- Product names, brands, and feature text are sent to the Gemini API only when
  you configure a key. Nothing else ever leaves your machine.

## Uninstall

Windows: `install.ps1 -Uninstall`, then delete the install folder. Linux: `./install.sh --uninstall` stops and disables the service and removes the installed server and extension. Neither touches your notes, the config file or the headless browser profile (your Amazon login); delete those yourself if you want them gone.
