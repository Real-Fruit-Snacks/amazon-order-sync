# Changelog

All notable changes to this project are documented here.
Format based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [2.2.0] — RFS refactor

The standalone app replaces the Obsidian plugin in this repository (the plugin's last release was 1.0.7; its code stays in the history). The repository is public from this release, and installs come from release assets instead of a copied project folder.

**Security**

- The server refuses requests whose `Host` isn't `127.0.0.1:<port>` or `localhost:<port>`. A website could otherwise use DNS rebinding to read `/api/orders` and `/api/export.json` as a same-origin page.
- Amazon page origins are no longer accepted. The extension now makes every server call from a service worker (`browser-extension/background.js`), and the server accepts only that extension's origin. Its ID is fixed by a `key` in the manifest. Before, any script on any Amazon subdomain could read your order history or drive the API.

**Fixes**

- A review typed under the `Write your review for this item below...` placeholder was replaced by a new draft (or the empty placeholder) on the next sync. It is now kept, and the placeholder line is dropped.
- A saved review containing a `---` line lost everything after that line on the next sync. Such lines are now written as `- - -`.
- The backfill import could write the same ASIN twice in one note. The duplicate check now covers every write path, not just `/api/sync`.
- The star rating you picked in Review Studio was dropped by **SAVE DRAFT** (only Regenerate kept one), and review auto-fill always clicked 5 stars. Save now stores your rating, `/api/get-review` returns it, and auto-fill clicks it. Auto-fill also sets the form fields with the native value setter, which Amazon's React form doesn't ignore.
- On Windows, quitting left the headless browser's helper processes running. Shutdown now ends the whole process tree.
- Google Chrome ignores `--load-extension` since version 137. It is now tried last, a warning is logged when it is picked, and the guidance for stalled runs says so.
- Note writes retry briefly when Windows reports the file busy (indexer, antivirus, an editor), and clean up their temp file if they still fail.
- Tray app: a server that kept crashing was restarted every few seconds forever. It is now restarted at most twice in five minutes. A restart could also start two servers; it no longer can. **Quit** now stops a server the tray attached to. A slow server no longer shows each notification twice.
- `install.ps1`: no longer recreates the `HKCU\...\Run` key, which could delete other programs' startup entries. Its Node.js fallback lookup works. It reads `config.json` as UTF-8 on Windows PowerShell 5.1, and stops any running server before an upgrade, even without the tray app. New configs keep notes in `%USERPROFILE%\AmazonOrders`.
- `import-old-notes.ps1` reads its JSON as UTF-8 on Windows PowerShell 5.1.
- `install.sh` checks health on the configured port, not always 27483.
- `npm test` works on Node.js 22.
- The extension popup could not load the last-sync time: the server refused its origin.

**Look**

- Dashboard, popup and icons use the RFS colours (Nightshift and Dayshift; Ember accent, Mint for success only) and IBM Plex Sans and Mono, bundled. The theme button reads NIGHTSHIFT / DAYSHIFT / AUTO.
- New icon: "aos" on a dark tile with an Ember rule, from `tools/make-icons.py`.

**Repository**

- README rewritten. The detail moved to `docs/guide.md` and `docs/api.md`, the design spec to `docs/spec.md`, and `README-WINDOWS.md` was folded into the guide.
- CI on Ubuntu and Windows with Node.js 18 and 22, a tray build, and installer checks. A release workflow builds `amazon-order-sync-windows.zip` (with `AmazonOrderSync.exe`) and `amazon-order-sync-linux.tar.gz`.
- SECURITY, CONTRIBUTING, issue forms, PR template, LICENSE.
- Tests: 91, up from 85. On Windows the 25 headless-run tests skip themselves (their fake browser is a shell script); the other 66 run there too.

## [2.1.0] — Windows port

- **Native Windows tray app** (`tray/`, C#/.NET 8 WinForms, self-contained
  `AmazonOrderSync.exe`): starts/stops the Node server hidden, shows Windows
  toasts, menu for dashboard / sync now / setup login / notes / config / logs,
  start-with-Windows toggle, clean shutdown via `POST /api/shutdown`.
- **Server**: platform-aware paths (`%APPDATA%` config, `%LOCALAPPDATA%`
  data; XDG unchanged on Linux); Brave/Chrome/Edge/Chromium autodetect on
  Windows (PATH `.exe` names, then standard install folders, Brave first);
  `taskkill /T /F` replaces process-group signals on Windows; `/proc` liveness
  check replaced by a cross-platform `process.kill(pid, 0)` probe.
- **New routes**: `GET /api/notifications?after=N` (toast ring buffer fed by
  every `tryNotify`), `POST /api/shutdown` (clean exit, releases lock), and a
  richer `GET /api/health` (`pid`, `platform`, `notesPath`, `browserFound`,
  `headlessRunning`). Headless meta now carries `profileDir`/`browserBinary`.
- `install.ps1` + `README-WINDOWS.md`. Linux `install.sh`/systemd unit unchanged.
- **Draft quality**: Gemini 503/502/504 retried with backoff (was 429 only);
  re-sync regenerates over placeholders and unedited canned templates (keeps
  the template if the retry fails; edited text always preserved); your own
  posted reviews (top 3 by helpful votes) are fed back as voice samples;
  prompt bans the stock constructions drafts kept converging on and asks for
  varied structure/openers/titles; new config keys `geminiModel`,
  `geminiTemperature`, `styleExemplars`.
- Extension: ship-to read from the visible recipient link (the card's hidden
  address popover doubled the name and leaked the street address).
- Tests: cases 79–85. 83/83 pass.

## [2.0.1]

Live Gemini usage panel (user report: usage didn't seem to update).

- New lightweight `GET /api/gemini-usage` (usage sidecar only — /api/orders
  costs a full notes scan at thousands of items).
- The SYNC tab's 2s log poller now also refreshes the usage number/bar in
  place, so background syncs' draft generation shows up within seconds
  instead of after the next tab switch.

## [2.0.0]

The Vine feature set is removed entirely (user decision: Vine detection was
always a `$0.00`-price proxy that cannot distinguish real Vine items from
gifts or promos — too inaccurate to keep).

### Removed

- Item `vine` flag and the `isVinePrice` helper (`parsePriceNumber` stays —
  price watch and spend math still use it).
- `/api/orders` meta `vineSummary` and `vineWindowDays`.
- Vine escalation: `checkVineEscalation`, its scheduler call, the persisted
  `vineNotified` state map, and its notification path.
- export.csv `vine` column (header is now
  `order_id,order_date,item_name,brand,asin,price,url,rating,reviewed,posted,helpful_votes`);
  export.json items no longer carry a `vine` field.
- Dashboard: VINE badges, the `ALL | VINE | PAID` chip rows (filtering is
  search-only again), the VINE QUEUE panel, and the VINE COVERAGE stat card.

### Compatibility

- `vineWindowDays`/`vineEscalationDays` in existing `config.json` files are
  inert — simply not read anymore (unknown keys were always ignored), so old
  configs boot unchanged.

## [1.9.2]

Honest counts for stopped/failed runs (user request).

- A manually stopped sync now reports the orders that actually landed
  before the stop (`CANCELLED · 7 orders`) instead of a hardcoded 0 —
  the daemon already counted successful writes for the stall watchdog;
  cancelled, timeout, browser-exited, and catch-path outcomes now use it.
- Error history rows also show the order count when it is non-zero.

## [1.9.1]

Multi-page headless sync (user evidence: maxOrders=100 stopped at 10 —
Amazon shows 10 cards per page and the headless pass never paginated,
because clicking Next dropped the `#aos-autosync` trigger hash).

- The daemon's autosync launch URL now carries a fresh per-run token
  (`#aos-autosync-<epochMs>`, mirroring the v1.8.4 review-check token).
- The extension persists cross-page run state in sessionStorage
  (`aos_headless_sync`: token, sentCount, runStartMs, pageNum). After a
  page's cards are exhausted — and neither maxOrders nor the time budget
  has been hit — it navigates to the next orders page with the same token
  in the hash; the next load restores the counters and continues as the
  same run. maxOrders and timeBudgetSec therefore span pages.
- Loop cap: 20 orders pages per run. The completion ping fires exactly
  once, on the final page, with the accumulated count. A legacy tokenless
  `#aos-autosync` hash still works — treated as always-fresh (single page).

## [1.9.0]

Catch-up sync mode — time-boxed runs that resume at the frontier (user
request: "10-minute syncs that continue where I left off").

- New third mode, `Catch-up — only new orders`: skips ANY order the daemon
  already has a note for (complete or not), so a time-boxed run spends its
  whole budget on never-seen orders instead of re-scraping the same
  incomplete ones. Each run starts fast at the frontier and moves it back
  in time. Quick mode remains the daily-maintenance mode.
- `/api/check` responses gain `exists` (note file present) to support it.

## [1.8.11]

Bot-protection pacing + sync-form robustness (user requests).

- All fixed scraping delays replaced with randomized jitter: per-item waits
  3–6s (was exactly 2s), review-retry 2–3.5s, page settle 2–4s, pagination/
  year navigation 4–7s. Metronome-precise delays are the easiest automation
  fingerprint; syncs are intentionally slower now — that is the point.
- Fixed "maxOrders stuck at previous value": the SYNC tab number inputs
  committed only on blur, so an edit could be lost to a re-render between
  typing and clicking Start. They now commit on every keystroke AND are
  re-read live at submit time.

## [1.8.10]

Clearable SYNC HEALTH history (user request).

- New `POST /api/sync-history/clear` wipes the headless run history; a
  `Clear` button now sits in the SYNC HEALTH panel header. `lastRunAt`
  (freshness/scheduler) is preserved, and an active login block is never
  cleared by it — otherwise `lastResult`/`lastError` reset too so stale
  error guidance disappears.

## [1.8.9]

Stuck-run recovery: a wedged "sync already running" can always be cleared
without a restart (spec §27).

### Added

- **Force recovery (server, §27.1):** `POST /api/sync-stop` with
  `running === true` but no pending completion wait (an inconsistent state
  no finally-path can settle) now force-clears the flag — logged as
  `Cleared a stuck run flag via sync-stop` — and answers
  `200 {ok:true, recovered:true}` instead of 409ing forever. The normal
  stop path (`200 {ok:true}`) and the idle 409 are unchanged.
- **Consistency sweep (server, §27.1):** the scheduler tick also clears
  `running && !_pending && !_activeChild` on its own, logged as
  `Cleared a stuck run flag (consistency sweep)`.
- **Dashboard busy-flag hygiene (§27.2):** the SYNC NOW / REVIEW CHECK NOW
  busy flags reset in BOTH `catch` and `finally` (an aborted in-flight
  request can never wedge the button); STOP SYNC always POSTs
  `/api/sync-stop` and reloads — even when only the local busy flag says
  "running" — and drops stale local busy flags on success; ALL 409
  responses from the trigger endpoints display the server's `error` text
  verbatim (generic "already running" only when the body has none), so a
  login-block 409 can never masquerade as "already running" again.

## [1.8.8]

Scale fix for thousands of notes, review cutoff, and log UX (spec §26).

### Added

- **Incremental per-file parse cache (server, §26.1):** the note scan cache
  is now per file (`path → {mtimeMs, size, parsed}`) instead of one
  whole-tree signature. Each scan walks the tree (cheap readdir+stat),
  re-parses only new/changed notes, drops deleted ones, and assembles the
  orders list from the cached parses — a single write no longer re-parses
  every note. With thousands of backfilled notes this removes the
  event-loop stall that could starve the extension's mid-sync check-ins.
  Batch parsing yields periodically; correctness invariant unchanged (after
  any write, `/api/orders` reflects it). New test seam `__lastScanStats()`.
- **Review cutoff (§26.2):** new config key `reviewCutoffDays` (positive
  int, default `60`; `0` disables). Unposted items from orders older than
  the cutoff gain `beyondCutoff: true` in `/api/orders` (posted items never
  do; meta echoes `reviewCutoffDays`), are hidden in the dashboard's Needs
  Review behind a quiet **SHOW OLDER (N)** chip (badge and pending-review
  counts ignore them), and are skipped by the review-check queue — posted
  items still get their vote refreshes.
- **Log UX (§26.3):** `POST /api/logs/clear` empties the daemon log ring
  buffer while keeping the monotonic index running (200 `{ok:true}`,
  origin-locked); the dashboard LIVE LOG gains **COPY** (clipboard copy of
  the visible log) and **CLEAR** buttons, and the GUIDANCE box a
  per-session dismiss (×).

## [1.8.7]

Headless run progress logging + Needs Review pagination (spec §25).

### Added

- **Early browser exit (server, §25.1):** if the sync browser process exits
  before the run settles (any code/signal), the daemon logs
  `Browser exited early (code N) — crash or profile issue` and settles the
  run as `browser-exited — the sync browser closed unexpectedly` within
  seconds (instead of waiting out the 45s watchdog), with history, one
  failure notification, and mapped guidance — silent on the daemon's own
  group-kill after settle.
- **Waiting ticks (server, §25.1):** every 15s while a headless run waits
  for the extension, the daemon logs
  `…waiting for extension: N client lines, M orders (Ts)` (unref'd
  interval, cleared on settle, permanently disarmed once any activity is
  seen — same condition as the stall watchdog). `AOS_WAIT_TICK_MS`
  overrides the interval (tests).
- **Extension-alive line (server, §25.1):** the first client-log line of a
  run logs `Extension alive in the sync browser.` (once per run).
- **Needs Review pagination (dashboard, §25.2):** the tab renders in
  batches of 50 (respecting the ALL/VINE/PAID filter and search) with a
  `LOAD MORE (N REMAINING)` button — thousands of backfilled items no
  longer render thousands of Review Studio cards at once. Selection stays
  data-driven: select-all and bulk actions cover all matching items,
  rendered or not.

## [1.8.6]

Support Amazon's newer orders-page URL (user report: popup said "need to be
on the orders page" while on one).

- `/gp/your-account/order-history` added to the manifest's .com content
  matches — the content script now actually injects there (regional domains
  were already domain-wide).
- Popup gate and the headless autosync page check generalize to
  `order-history` (covers classic and new layouts).

## [1.8.5]

Duplicate-extension hardening for the sync browser profile (user report).

- Both browser spawn paths (headless runs + SETUP LOGIN) now pass
  `--disable-extensions-except=<installed extension dir>`, so only the
  daemon-managed copy of the extension can run in the sync profile — a
  second unpacked copy persisted in the profile (e.g. loaded by hand from
  another folder) is disabled instead of double-scraping every page.

## [1.8.4]

Residual-nit cleanup closing out every known issue from both audits (spec
§24). Selector fragility and the .com redirect gap are accepted as designed
(watchdog backstop) — not bugs.

### Fixed

- **Watchdog strictness (server, §24.1):** only SUCCESSFUL work disarms the
  headless stall watchdog — a 2xx-completed `/api/sync` order write and
  `/api/client-log` lines. A sync POST that 400/500s no longer counts as
  extension activity (client-log lines stay a liveness signal: they only
  exist while the content script is alive).
- **Single-instance lock (server, §24.1):** at startup the daemon writes
  `<notesPath>/.daemon.lock` `{pid, port, startedAt}`. A second daemon
  pointed at the same notes folder while the lock holder is alive exits 1
  with `another daemon (pid N) is already serving this notes folder`; a
  stale lock (dead pid) is taken over silently; a clean SIGTERM/SIGINT exit
  removes the file. `AOS_SKIP_INSTANCE_LOCK=1` bypasses (tests/dev).
  `OrderStore`/`HeadlessRunner` constructors stay lock-free.
- **Browser-resolve TTL (server, §24.1):** a cached `resolveBrowser` result
  older than 60s is re-validated with one cheap `accessSync(X_OK)`; the full
  PATH scan only runs on a miss or an entry that no longer validates.
- **Review-check processed-set (extension, §24.2):** cleared at run START as
  well as at exhaustion — a tab reused after a crashed/killed run can't skip
  items. The daemon stamps each review-check launch with a per-run token
  (`#aos-reviewcheck-<ms>`) so the first page of a new run is told apart
  from a continuation page of the same run.
- **Stale auto-sync flag (extension, §24.2):** the hash-triggered headless
  branch removes any leftover `amazon_sync_auto` sessionStorage flag before
  scheduling, so manual-browsing state can't double-schedule a headless tab.
- **extLogBuffer cap (extension, §24.2):** the pending log-forward buffer is
  hard-capped at 1000 lines, drop-oldest (one console note when trimming
  starts) — bounded even if the daemon is down for a whole run.
- **manifest (extension, §24.2):** the `.com` content matches gain
  `*://*.amazon.com/your-orders*` (no trailing slash) alongside the existing
  patterns.

## [1.8.3]

Second-audit fix release: setup-browser/profile-lock collision guard, a
server correctness batch (parser quote-awareness, race hardening, small
route fixes), and ops hardening.

### Fixed

- **Setup-browser vs profile-lock collision (server, spec §23.2):** the
  HeadlessRunner now tracks the visible setup browser child (pid + exit
  listener). While it lives, `POST /api/sync-now` and
  `POST /api/review-check-now` return 409
  `{error:'setup browser open — close it first'}`, the stale Singleton*
  lock cleanup skips its LIVE locks, and `POST /api/headless-setup`
  returns 409 while a run is active. Belt: before deleting `SingletonLock`
  its symlink target pid is checked against `/proc` — a live owner skips
  all deletions (best-effort, Linux-only).
- **Server correctness batch (§23.5):**
  - S-a: the inline frontmatter array matcher is quote-aware — a `]`
    inside a quoted value (brand `Foo]Bar`) no longer terminates the
    array and misaligns every later entry.
  - S-b: `reviewMissing` read-modify-save is serialized per ASIN — a
    dismiss-missing during a confirming sighting's write window sticks
    instead of being overwritten back onto disk.
  - S-c: the browser child's `error` listener attaches unconditionally
    right after spawn (no unhandled-error crash in the settled window).
  - S-d: `downloadImage`'s buffered fallback skips when there is no
    Content-Length and no stream API (unbounded read), and an over-cap
    Content-Length skips before any buffering.
  - S-e/S-f: inline-value unescaping is single-pass (`\\\"` round-trips),
    and `setFrontmatterArrayValue` rewrites siblings with full backslash +
    quote escaping.
  - S-g: the Gemini quota path returns an explicit `fallback:true` marker
    that regenerate-review trusts instead of text emptiness.
  - S-h: malformed JSON on `/api/sync` and `/api/mark-posted` is a 400
    with a static message (was a 500 leaking the parser error).
  - S-i: the HTML/icon routes match the parsed pathname — `/?tab=sync`
    and `/favicon.ico?…` no longer 404.
  - S-j: `/api/check` status extraction uses the escape-aware scalarField.
  - S-k: the CSV formula-injection guard also fires behind leading
    whitespace.
  - S-l: a browser spawn failing ENOENT invalidates the resolveBrowser
    cache so the next call re-scans.
- **install.sh (§23.6):** the install-path `systemctl` calls are guarded
  with `command -v` (systemd-less installs), and `daemon-reload` runs
  before `try-restart`.
- Tests 56–62.

## [1.8.2]

Login-block gate: when the last headless run failed with a login error
(expired Amazon session), sync can no longer be started — manually or on the
schedule — until the user re-authenticates via SETUP LOGIN.

### Added

- **Login-block gate (server, spec §22.1):** a persisted predicate,
  `loginBlocked = lastResult === 'error' && /login/i.test(lastError)`, read
  from the runner state file on every check (survives daemon restarts; any
  fresh result takes effect immediately). While blocked, `POST /api/sync-now`
  and `POST /api/review-check-now` return 409
  `{error:'login-required — run SETUP LOGIN first', guidance}` with no
  browser spawn, and both scheduler due predicates (`isHeadlessDue`,
  `isReviewCheckDue`) are forced false via a threaded `loginBlocked` flag —
  no automatic retries and no repeated failure notifications. Skips are
  logged at most once per calendar day. The `/api/orders` meta always
  carries `headless.loginBlocked`.
- **Cleared by re-authentication:** `POST /api/headless-setup` now nulls
  `lastResult`/`lastError` on successful launch — before the 200 is sent —
  so the next run gets a chance while the user logs in. Any run that
  completes `ok` also clears the gate (existing state overwrite).
- **Dashboard locked state (§22.2):** while `headless.loginBlocked`, START
  SYNC, REVIEW CHECK NOW, and the Overview SYNC NOW are disabled (greyed,
  with explaining `title` hints), an amber `LOG IN REQUIRED — RUN SETUP
  LOGIN FIRST` hint replaces the SYNC tab's idle button row, and SETUP LOGIN
  gets amber emphasis as the required action. After SETUP LOGIN returns 200
  the dashboard reloads state and the buttons unlock. STOP SYNC is
  unaffected.
- Test 55.

## [1.8.1]

Headless-stall hardening: repeated headless runs that produced zero extension
log lines and zero orders (ending only via manual stop or the 6-minute
timeout) now fail fast with a specific error and guidance.

### Added

- **Sign-in page detection (fast login-required):** the extension's content
  matches now include `*://*.amazon.com/ap/signin*` (the regional TLD entries
  are domain-wide and already matched). On any `/ap/signin*` page carrying
  the `#aos-autosync` or `#aos-reviewcheck` fragment (fragments survive the
  session-expiry redirect), the content script settles after load, POSTs
  `/api/headless-complete {ok:false, error:'login-required'}` once, and
  stops — instead of stalling silently until the daemon timeout. Normal
  sign-in browsing is untouched (hash gate).
- **Stale profile lock cleanup (server):** before spawning the headless
  browser, the runner deletes `SingletonLock`, `SingletonSocket`, and
  `SingletonCookie` from the profile dir when present (symlink/lock artifacts
  left behind by SIGTERM/SIGKILLed browsers that make the next launch stall).
  Exactly one log line when any were removed; removal is best-effort and
  only happens when no run is active (guaranteed by the in-progress guard).
- **No-activity watchdog (server):** when a run starts, a 45s unref'd
  watchdog arms. If ZERO client-log lines arrived (`/api/client-log`) AND
  zero orders were POSTed (`/api/sync`) since run start, the run aborts as
  `lastResult:'error'`, `lastError:'browser-stalled — no extension activity
  after 45s'` through the same settle/kill/history path as other failures
  (one failure notification, no spam). Any client-log line or order POST
  disarms it permanently for that run; counters reset per run. The delay is
  injectable via `AOS_STALL_WATCHDOG_MS` and the profile dir via
  `AOS_PROFILE_DIR` (tests/smoke).
- **Guidance:** `guidanceFor('browser-stalled…')` maps to "The sync browser
  started but the extension never responded." with three next steps (SETUP
  LOGIN, delete the browser profile, Brave Shields down).
- Tests 53–54.

## [1.8.0]

Removed-review detection (spec §21.1) plus the deferred audit-hardening
batch (§21.2 H1–H7).

### Added

- **Removed-review detection (§21.1):** the review-checker now reports a
  `page_state` (`loaded`/`captcha`/`error`; absent = `loaded`) with every
  `/api/review-check-result`. When a LOADED create-review page shows an
  empty form for an item the notes say is posted WITH review content, the
  server records a sighting in the state file
  (`reviewMissing[asin] = {since, sightings, confirmed, notified}`). Two
  CONSECUTIVE empty sightings confirm the removal: one desktop notification
  ("Amazon review missing — your posted review is gone"), a
  `missingReviews: [{asin, name, orderId, since}]` block in the `/api/orders`
  meta, and `reviewMissing: true` on the flagged items. A found review
  clears the entry (streak reset); a captcha/error page records nothing at
  all (no sighting, no vote-timestamp bump — the item just re-queues).
  Unconfirmed suspects jump to the front of the review-check queue. The
  note itself is never modified — the review text stays, ready to repost.
- **`POST /api/dismiss-missing {asin}` (§21.1):** clears one missing-review
  entry (re-detection starts fresh; 404 for an unknown ASIN). Origin-locked
  like every other route.
- Dashboard: red `REVIEW MISSING` badge on Orders rows and Needs Review
  cards, with a dismiss (×) action.
- Tests 48–52 (§21.3).

### Fixed (hardening batch, §21.2)

- **H1 downloadImage:** fetches http(s) only, requires an `image/*`
  response Content-Type that maps onto the `/assets/` serving whitelist
  (the saved file's extension now comes from the Content-Type, not the
  URL), and streams with a hard 5 MB cap — any violation skips the download
  (`null`). The fetch is injectable for tests (`setImageFetchForTests`,
  same pattern as `setGeminiFetchForTests`).
- **H2 resolveBrowser:** the PATH scan result is cached (per configured
  `browserPath`, `resetBrowserCacheForTests` hook) — `/api/orders` no
  longer re-scans PATH per request.
- **H3 parsePriceNumber:** European formats — when both separators are
  present the rightmost is decimal (`1.234,56` → 1234.56, `$1,234.56` →
  1234.56); a lone comma followed by 1–2 digits is a decimal comma
  (`12,99` → 12.99), otherwise a thousands separator. US behavior
  unchanged.
- **H4 voteTimestamps:** entries whose ASIN no longer appears in any note
  are pruned on each scheduler tick (cached scan, map stays bounded).
- **H5 routes:** all `/api/` routes match the parsed pathname exactly —
  `/api/checkAnything` is now a 404 instead of an `/api/check` alias.
- **H6 stop-window:** the headless run's pending record is registered
  synchronously with `running = true`, before the async mkdir/spawn
  sequence — `/api/sync-stop` and `/api/headless-complete` can no longer
  409 during the spawn window.
- **H7 (extension):** `syncLogs` capped at 500 entries; the
  `isSyncStopped` early-return no longer skips `isSyncing = false`; the
  `start_auto_sync` branch has its missing `.catch`.

## [1.7.2]

Hardening release from a full three-lens code audit (spec §20).

### Fixed

- **Index-aligned frontmatter parsing everywhere (§20.1, audit S1/S2 — data
  corruption):** every index-aligned field (`brand`, `price`, `url`,
  `rating`, `reviewed`, `helpful_votes`, `tone`) is now read with the
  alignment-preserving parser in `parseNote`, `analyzeOrder` (`/api/check`),
  the regenerate-review brand lookup, and all re-sync preservation paths.
  Previously the filtering parser dropped `""` entries, so e.g. a note with
  `rating: ["", "4"]` assigned the 4 stars to the FIRST item, and writing
  helpful votes for index 1 then index 0 erased the first write.
  `setFrontmatterArrayValue` now reads existing values with the aligned
  parser for ALL fields. The filtering `parseArrayField` survives only for
  existence lookups and asin-driven index derivation (asin arrays never have
  empty entries — documented at each call site).
- **Frontmatter escaping (§20.2, audit S3):** one `yamlEscape()` helper
  (double-quote wrap, escape inner `"` and `\`, strip CR/LF) now guards EVERY
  interpolated frontmatter value in `buildNote` — order_id, date, total,
  status, ship_to, brand, price, asin, url (plus rating and tone) — so a
  scraped value containing quotes or a newline can no longer corrupt the
  frontmatter or inject keys. `scalarField` reads the escaped form back
  (escape-aware, unescapes on read).
- **Sync-time ASIN dedupe (§20.3, audit C1):** duplicate ASINs within one
  `/api/sync` payload are dropped (first occurrence wins) with one
  `[Dedupe] Dropped duplicate ASIN … in order …` log line each — same-ASIN
  notes made every ASIN-keyed writer ambiguous.
- **Regenerate quota-edge misreport (§20.4):** a REAL draft whose own
  request pushed the Gemini counter to the daily limit was misreported as
  `fallback: true` and discarded. The quota-skip is now decided up front;
  only actual fallback drafts are flagged.
- **Notifications config honored (§20.4):** the legacy per-order and
  per-generation `notify()` calls now respect `notificationsEnabled`
  (tryNotify paths were already gated).
- **export.csv formula-injection guard (§20.4):** fields beginning with
  `=`, `+`, `-` or `@` are prefixed with `'` so spreadsheets can't evaluate
  them as formulas.
- **install.sh (§20.4):** `systemctl --user try-restart amazon-order-sync`
  after the file copy, so upgrades restart an already-running service
  immediately (no-op when not running; first install still uses
  `enable --now`).

### Added

- Headless meta gains `runStartedAt` (§20.4): ms epoch while a run is
  active, `null` otherwise — the dashboard RUN STATUS panel uses it for the
  "started X ago" line.
- Tests 40–47 (§20.7): aligned-parse regression (S1), descending vote
  writes (S2), YAML-escaping roundtrip (S3), ASIN dedupe (C1), re-sync
  preservation roundtrip, /api/check matrix, Gemini 429→success quota
  accounting, and log-buffer wraparound.

## [1.7.1]

### Added

- Extension log forwarding (§19): during both headless branches
  (`aos-autosync`, `aos-reviewcheck`) the extension buffers every
  `logStatus(...)` line and flushes it to the new `POST /api/client-log`
  endpoint every ~3 s and once at run end (right before the completion
  ping). The daemon validates `{lines:[1..100 strings]}`, trims each line
  to 500 chars, and appends the lines to the existing §18.3 ring buffer
  with an `[ext] ` prefix — the dashboard LIVE LOG now shows full scrape
  detail during headless runs. Flush failures are ignored (lines are only
  cleared on confirmed success); manual/popup syncs are unaffected.
- Run lifecycle lines (§19.3): the daemon now logs an explicit headless
  run-start line (mode + options summary) and a run-finish line
  (result, orders, duration) alongside the existing per-order and
  generation lines.

## [1.7.0]

### Added

- Run options (§18.1): `POST /api/sync-now` accepts an optional JSON body
  `{mode:'quick'|'full', maxOrders:int>=0, timeBudgetSec:int>=0,
  refresh:{prices,votes,details}}` — every field validates independently
  (defaults: full / 0=all / 0=none / all true; missing or unparsable body
  means full defaults). The runner stores the effective options as the
  pending run's options (review-check runs ignore them), `GET
  /api/run-options` returns `{options}` — the in-flight run's options while
  running, the last used options otherwise — and the `/api/orders`
  `headless` meta gains `lastOptions` (persisted in
  `<notesPath>/.headless-sync.json`).
- Daemon log buffer (§18.3): a 500-entry ring buffer `{i, t, text}`
  (monotonic index, ms time); `main()` wraps `console.log`/`console.error`
  so every daemon line lands in the buffer while stdout/stderr output is
  unchanged. `GET /api/logs?after=N` returns `{lines, next}` — only newer
  lines with the cursor, the last 100 without.
- Friendly guidance (§18.4): exported `guidanceFor(errText)` maps
  `login-required`, `no-browser`, `timeout…`, and `gemini-limit` to a
  plain-language `{message, steps[]}` (unknown errors pass through raw with
  no steps). Headless run results and the `headless` meta gain `guidance`
  whenever `lastError` is set.
- Gemini quota tracking (§18.5): every actual HTTP attempt to the Gemini
  endpoint (429 retries included) increments `{day, used}` persisted in
  `<notesPath>/.gemini-usage.json` (day-rollover reset, atomic-ish write;
  exported `recordGeminiAttempt`/`loadGeminiUsage`/`setGeminiFetchForTests`
  keep it testable without network). New optional config `geminiDailyLimit`
  (positive int, default 1000): at `used >= limit` `generateReview` skips
  the API entirely and returns the fallback draft (one log line per run);
  at >=80% one warning log line per run. `/api/orders` meta gains
  `gemini {used, limit, remaining, day}`.
- Dashboard (§18.6): new SYNC tab with run controls, live log, run status +
  history, guidance box, and Gemini usage panel.
- Tests: cases 33–38 (§18.7); suite is now 38 tests.

## [1.6.1]

### Added

- Stoppable headless runs (§17.1): `POST /api/sync-stop` aborts the active
  headless run (orders sync or review-check) — 409 `{error:'no run in
  progress'}` when idle, otherwise the browser process group is killed via
  the existing SIGTERM→SIGKILL path, the run resolves as
  `lastResult:'cancelled'` (`lastError:null`, `ordersSynced:0`, recorded in
  the persisted run history with `result:'cancelled'`), pending
  `/api/sync-now` and `/api/review-check-now` callers resolve with the
  cancelled result, and no desktop notification fires. A settled flag
  guards the 6-minute timeout / completion-ping / abort paths against
  double-resolution.
- Shutdown hygiene (§17.2): on SIGTERM/SIGINT the daemon kills the active
  run's browser process group before exiting — no orphan headless browsers
  after `systemctl --user restart`.
- Dashboard (§17.3): the SYNC NOW button becomes STOP SYNC (amber/red
  emphasis) while `headless.running` or a request is locally in flight and
  posts to `/api/sync-stop` (notice `Sync stopped`); cancelled results
  render as an amber `Sync cancelled` notice, and SYNC HEALTH shows
  `CANCELLED` rows in amber.

## [1.6.0]

### Added

- Price-drop watch (§16.2): sync items may carry an optional `listing_price`;
  when it and the paid price both parse, the server upserts
  `<notesPath>/.price-history.json` (`{[asin]: {paid, history:[{t, price}]}}`,
  same-day collapse, 90-entry cap per ASIN, validated-ASIN keys, atomic-ish
  writes). `/api/orders` items gain `listingPrice` and the meta gains
  `priceDrops` (`{asin, name, paid, listing, savings, pct}`, paid>0 and
  latest listing < paid, savings to 2dp, sorted savings desc, cap 10).
- Desktop notifications (§16.3): `tryNotify(title, body)` locates
  `notify-send` on PATH once (cached, injectable via `AOS_NOTIFY_SEND` or
  `setNotifyBinaryForTests`) and spawns it detached, fire-and-forget. Fires
  on headless completion with `ordersSynced > 0`, on headless failure, and
  on Vine escalation. New optional config key `notificationsEnabled`
  (default `true`; only literal `false` disables).
- Regions (§16.4): the origin lock now also allows `amazon.fr`, `amazon.it`,
  `amazon.es`, `amazon.co.jp`, and `amazon.com.au`.
- Repeat-purchase flag (§16.5): `/api/orders` items gain `repeatCount`, the
  number of distinct orders containing that ASIN (>=1).
- Sync health history (§16.6): `<notesPath>/.headless-sync.json` gains
  `history` (`{t, result, error, ordersSynced, durationMs}`, every run,
  newest first, cap 20); `/api/orders` meta `headless.history` serves the
  last 10.
- Headless review-checker (§16.7): refreshes helpful votes and detects
  reviews posted from other devices without a full order sync. New
  endpoints `GET /api/review-check-queue` (posted items with votes older
  than `reviewCheckIntervalDays` — vote timestamps tracked in the state
  file's `voteTimestamps`, bumped on every votes write — plus unposted
  items from orders >2 days old; cap 20, oldest-stale first),
  `POST /api/review-check-result` (real review -> note review block +
  posted checkbox + votes via the imported-review write path; votes-only
  path; always bumps `voteTimestamps`; 400 on bad payloads), and
  `POST /api/review-check-now` (409 while any headless run is active; an
  empty queue resolves immediately without spawning; otherwise launches the
  shared HeadlessRunner at the first queued ASIN's create-review page with
  `#aos-reviewcheck`). The scheduler also fires a review check when due
  (`autoSyncEnabled && browserFound && now − lastReviewCheckAt >
  reviewCheckIntervalDays`, persisted in the state file); meta
  `headless.lastReviewCheckAt` added. New optional config key
  `reviewCheckIntervalDays` (default `7`).
- Vine escalation (§16.8): on each scheduler tick, unposted Vine items with
  `daysLeft <= vineEscalationDays` (days computed with `vineWindowDays` like
  the dashboard queue) trigger a `Vine review due soon` notification at
  most once per item per calendar day (state file `vineNotified`). New
  optional config key `vineEscalationDays` (default `7`).
- Tests 23–30 (price watch, notifications, regions, repeatCount, run
  history, review-check queue/results, Vine escalation, review-check-now).

## [1.5.0]

### Added

- Headless auto-sync: the daemon drives headless Chromium with the real
  extension loaded — no cookie/HTTP scraping. New optional config keys
  `autoSyncEnabled` (default `false`), `autoSyncIntervalHours` (default `24`),
  and `browserPath` (default `''` = autodetect `chromium`/`chromium-browser`/
  `google-chrome`/`google-chrome-stable`/`brave-browser`/`microsoft-edge` on
  `PATH`). A dedicated profile at
  `~/.local/share/amazon-order-sync/browser-profile/` holds the Amazon login.
- `runHeadlessSync()` lifecycle: in-progress guard, detached spawn in its own
  process group, completion-ping await with a 6-minute timeout, and
  SIGTERM→SIGKILL group kill on every exit path (complete, timeout, error) —
  no zombie chromiums. State persists to `<notesPath>/.headless-sync.json`
  (`{lastRunAt, lastResult, lastError, ordersSynced}`), loaded at boot;
  `running` is in-memory only.
- New endpoints: `POST /api/sync-now` (409 while running; responds when the
  run finishes), `POST /api/headless-complete` (extension ping; 409 when
  idle), `POST /api/headless-setup` (detached visible login window, 409
  without a browser) and the CLI equivalent `node server.js --setup-headless`.
  `/api/orders` meta gains `headless` (`enabled`, `running`, `lastRunAt`,
  `lastResult`, `lastError`, `ordersSynced`, `intervalHours`, `browserFound`).
- Scheduler: an unref'd 30-minute interval fires a run when due
  (`enabled && browserFound && !running && now−lastRunAt > intervalHours`;
  never-run counts as due), fire-and-forget with error logging.
- `install.sh` also installs `browser-extension/` to
  `~/.local/share/amazon-order-sync/browser-extension/` (idempotent);
  `--uninstall` now preserves the `browser-profile/` login alongside notes
  and config.
- Tests 19–22 (binary autodetect, fake-browser end-to-end cycle with process
  reaping, 409 discipline, scheduler due-predicate math).

## [1.4.0]

### Added

- Vine deadline tracker: `/api/orders` meta gains `vineSummary`
  (`{total, posted, pending, coveragePct}` over all `$0.00` Vine items) and
  `vineWindowDays` (new optional config key, default 30); the dashboard shows
  a VINE COVERAGE stat card and a deadline-sorted VINE QUEUE panel.
- Review-performance analytics: per-item `tone` is persisted as a frontmatter
  array aligned with `asin` — written by `/api/regenerate-review` (the tone
  actually used, including `balanced`), preserved by `/api/save-review` and
  by re-syncs, omitted entirely when nothing is known. `parseNote` exposes it
  (default `''`) and `/api/orders` + export.json include it (CSV unchanged).
  The dashboard charts helpful votes per posted review, tinted by tone, and
  surfaces most-helpful-tone/rating insights.
- Sync freshness: `/api/orders` meta gains `lastSyncAt` (max mtime across
  order notes, `null` when none) and `syncReminderDays` (new optional config
  key, default 7); the dashboard shows a stale-sync banner and the extension
  popup a last-sync line.

## [1.3.0]

### Added

- Backfill importer: `POST /api/import-backfill` accepts Amazon "Request My
  Data" order-history CSV files (raw `text/csv` or JSON `{files:[...]}`, 50 MB
  cap for this route only), groups rows by order, writes one note per order
  with `backfill: true` frontmatter, and never overwrites existing notes.
- RFC-4180 `parseCsv` (quoted fields, embedded commas/newlines, doubled
  quotes, case-insensitive header lookup) and `parsePriceNumber`, both
  exported for tests.
- Vine ($0.00) item detection: `/api/orders` items gain `vine`, and
  export.csv/export.json gain `vine` and `helpful_votes` columns/fields.
- Tone presets on `/api/regenerate-review`: optional `tone` in
  `{balanced, concise, enthusiastic, critical}` (default `balanced`), echoed
  in the response and threaded into the Gemini prompt.
- Helpful-vote tracking: sync accepts per-item `helpful_votes`, persisted as a
  frontmatter array aligned with `asin` (preserved on re-sync when omitted),
  exposed as `helpfulVotes` via `/api/orders` and exports.

## [1.2.0]

### Added

- Review Studio endpoints: `POST /api/save-review`, `POST /api/regenerate-review`.
- Export endpoints: `GET /api/export.csv` (RFC-4180) and `GET /api/export.json`.
- Smart default ratings: deterministic per-ASIN `defaultRatingForAsin` (~40%
  4-star) replaces the hardcoded 5-star draft rating; Gemini prompt is now
  rating-aware (1-5).
- Voice-of-customer context: optional `top_reviews` per sync item, persisted
  to `<notesPath>/.review-context.json` and quoted to Gemini as untrusted
  reference material.
- `scanNotes` parse cache keyed by tree signature, invalidated on every write.
- Regional Amazon origins: `amazon.co.uk`, `amazon.de`, `amazon.ca`.
- Test suite `tests/server.test.js` (`npm test`, node:test, no network needed).

## [1.1.2]

### Added

- Bundled Terminal Workbench favicon and icons.

## [1.1.1]

### Added

- Terminal Workbench theme (dark default, light via `prefers-color-scheme` or
  toggle), bundled Mono typeface served at `/fonts/`.

## [1.1.0]

### Added

- Client-rendered dashboard (`server/dashboard.html`) with `GET /api/orders`,
  `GET /assets/<file>`, and `POST /api/mark-posted`.
- Same-origin dashboard requests allowed by the origin lock.

## [1.0.9]

### Fixed

- Review detection on re-sync: the `is_reviewed` heuristic no longer strands
  items; posted reviews are imported and checked boxes are preserved.

## [1.0.0]

### Added

- Standalone Node.js port of the Obsidian plugin: local HTTP daemon, Markdown
  note writer, image caching, Gemini review drafts, installer + systemd unit.
