# HTTP API

The server listens on `http://127.0.0.1:27483` (the `port` setting). It answers only requests addressed to `127.0.0.1:<port>` or `localhost:<port>`, and of those only the dashboard's own page, the extension's service worker and origin-less local tools such as curl and the tray app. See [Security model](guide.md#security-model).


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
| `GET /api/notifications?after=N` | Notification ring buffer that the Windows tray app shows as toasts. |
| `POST /api/shutdown`         | Clean exit: stops any headless browser and releases the notes-folder lock (tray Quit, installer). |
| `GET /api/gemini-usage`      | Today's Gemini request count and limit. |
| `GET /api/health`            | `{ok, version, pid, platform, notesPath, browserFound, headlessRunning}`; the tray app polls it. |

Four supporting routes: `GET /assets/<file>` serves the cached product images
from `Orders/assets/`, `GET /fonts/<file>` serves the bundled typeface, and
`GET /favicon.ico` / `GET /icons/<file>` serve the bundled dashboard icons.

The dashboard uses the RFS colours ([rfs-brand](https://github.com/Real-Fruit-Snacks/rfs-brand)) and bundles IBM Plex Sans and Mono (OFL 1.1, `server/fonts/OFL.txt`), served from `GET /fonts/<file>`.
