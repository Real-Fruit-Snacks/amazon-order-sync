let isSyncStopped = false;
let isSyncing = false;
let syncLogs = [];

// ---------------------------------------------------------------------------
// v1.7.1 (SPEC §19.2): headless log forwarding. Inert unless one of the two
// headless branches (aos-autosync / aos-reviewcheck) flips aosForwardLogs on;
// manual/popup syncs never touch this path and are byte-for-byte unaffected.
// Pending lines flush to the daemon every ~3s and once at run end (right
// before the completion ping). Failures are ignored and lines are only
// cleared on a confirmed success — logging must never break a sync.
// ---------------------------------------------------------------------------
let aosForwardLogs = false;
let aosLogFlushInterval = null; // v1.7.2 (§20.6): handle so we can shut down cleanly
const extLogBuffer = [];
// v1.8.4 (§24.2): hard cap — drop-oldest once the buffer hits 1000 pending
// lines so a run with the daemon down can't grow it without bound. One
// console.info the first time trimming kicks in (not logStatus — that would
// recurse into the buffer).
const EXT_LOG_BUFFER_CAP = 1000;
let extLogTrimNoted = false;

// v2.2.0: every call to the local server goes through the extension's
// service worker (background.js). A content-script fetch carries the Amazon
// page's Origin, so the server could not tell this extension from any script
// on an Amazon page; the service worker's requests carry this extension's own
// origin, which is the only extension origin the server accepts.
function aosFetch(path, init = {}) {
    return new Promise((resolve, reject) => {
        chrome.runtime.sendMessage({ aos: 'fetch', path, method: init.method || 'GET', body: init.body }, reply => {
            const err = chrome.runtime.lastError;
            if (err || !reply) return reject(new Error(err ? err.message : 'no reply from the extension'));
            if (reply.error) return reject(new Error(reply.error));
            resolve({
                ok: reply.status >= 200 && reply.status < 300,
                status: reply.status,
                text: async () => reply.text,
                json: async () => JSON.parse(reply.text)
            });
        });
    });
}

function setNativeValue(el, value) {
    const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
}

function flushClientLogs() {
    if (extLogBuffer.length === 0) return Promise.resolve(true);
    const lines = extLogBuffer.slice(0, 100); // server accepts at most 100 per post
    return aosFetch('/api/client-log', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ lines })
    }).then(res => {
        if (res.ok) extLogBuffer.splice(0, lines.length); // clear-on-success only
        return res.ok;
    }).catch(() => false /* daemon unreachable etc. — retry on the next flush */);
}

function startClientLogForwarding() {
    if (aosForwardLogs) return;
    aosForwardLogs = true;
    aosLogFlushInterval = setInterval(flushClientLogs, 3000);
}

// v1.7.2 (§20.6): run-end shutdown. Called after the final completion ping:
// stop the 3s interval, then flush in a loop until the buffer is fully
// drained (multiple 100-line posts if needed). Failure-tolerant: a failed
// post stops the loop — logging must never hang a headless run.
async function stopClientLogForwarding() {
    if (aosLogFlushInterval) {
        clearInterval(aosLogFlushInterval);
        aosLogFlushInterval = null;
    }
    while (extLogBuffer.length > 0) {
        const ok = await flushClientLogs();
        if (!ok) break;
    }
    aosForwardLogs = false;
}

function logStatus(message) {
    console.log(message);
    const time = new Date().toLocaleTimeString();
    const logEntry = `[${time}] ${message}`;
    syncLogs.push(logEntry);
    // v1.8.0 (§21.2 H7): cap the in-page log buffer at 500 entries (drop
    // oldest) so a long auto-sync can't grow it without bound.
    if (syncLogs.length > 500) syncLogs.shift();
    if (aosForwardLogs) { // v1.7.1 (§19.2)
        extLogBuffer.push(logEntry);
        // v1.8.4 (§24.2): bounded buffer — drop the oldest pending line past
        // the cap, with a single console note when trimming starts.
        if (extLogBuffer.length > EXT_LOG_BUFFER_CAP) {
            extLogBuffer.shift();
            if (!extLogTrimNoted) {
                extLogTrimNoted = true;
                console.info(`[AOS] Log buffer reached ${EXT_LOG_BUFFER_CAP} unflushed lines; dropping oldest until the daemon drains it.`);
            }
        }
    }

    // Broadcast to popup if open
    chrome.runtime.sendMessage({
        action: "status_update", 
        log: logEntry,
        isAutoSyncing: sessionStorage.getItem('amazon_sync_auto') === 'true'
    }).catch(() => {});
}

logStatus("Amazon Order Sync content script loaded.");

// Auto-start if flag is set
if (sessionStorage.getItem('amazon_sync_auto') === 'true') {
    logStatus("Auto-sync is enabled. Starting extraction in 2 seconds...");
    setTimeout(() => {
        extractAndSyncOrders(true);
    }, 2000);
}

// ---------------------------------------------------------------------------
// v1.5.0: headless auto-sync trigger (SPEC §15.6). When the page was opened
// by the local daemon with `#aos-autosync`, run ONE single-pass sync (the
// same routine the popup's Start Sync button triggers, isAuto=false) and
// report the outcome to POST /api/headless-complete. Everything in this block
// is gated on the hash; normal browsing is completely unaffected.
// ---------------------------------------------------------------------------
// v1.8.6: Amazon also serves the orders page at the newer
// /gp/your-account/order-history path — 'order-history' covers both layouts.
const AOS_HEADLESS_AUTOSYNC = window.location.hash.includes('aos-autosync') &&
    (window.location.pathname.includes('/your-orders') ||
     window.location.pathname.includes('order-history'));
let aosHeadlessFired = false; // fires exactly once per page load

// ---------------------------------------------------------------------------
// v1.9.1 (§28.2): multi-page headless sync. The daemon's launch URL carries
// a per-run token (`#aos-autosync-<epochMs>`, mirroring the v1.8.4
// review-check token); a pagination hop re-navigates with the SAME token so
// the next page load can tell "continuation of this run" apart from "a new
// run". Cross-page counters live in sessionStorage under `aos_headless_sync`
// ({token, sentCount, runStartMs, pageNum}) so maxOrders/timeBudgetSec span
// pages. A bare `#aos-autosync` hash (older daemon) is legacy/tokenless:
// every page load is then treated as a FRESH run (no pagination possible).
// ---------------------------------------------------------------------------
const AOS_HEADLESS_STATE_KEY = 'aos_headless_sync';
const AOS_HEADLESS_MAX_PAGES = 20; // loop cap per run (§28.2)
let aosHeadlessRunState = null;    // this run's counters (headless branch only)
let aosHeadlessPaginating = false; // true while a pagination hop is in flight

function aosParseRunToken() {
    const m = window.location.hash.match(/aos-autosync-(\d+)/);
    return m ? m[1] : null; // null = legacy tokenless hash → always fresh
}

function aosReadHeadlessState() {
    try {
        const raw = sessionStorage.getItem(AOS_HEADLESS_STATE_KEY);
        if (!raw) return null;
        const s = JSON.parse(raw);
        if (!s || typeof s !== 'object') return null;
        return s;
    } catch (e) {
        return null;
    }
}

// ---------------------------------------------------------------------------
// v1.7.0 (SPEC §18.2): headless run options. Fetched ONCE at the start of a
// headless #aos-autosync run from GET /api/run-options; absent server/older
// daemon/network failure → full defaults. Manual popup syncs NEVER fetch
// run-options: aosRunOptions stays null and activeRunOptions() returns the
// defaults, which are identical to pre-1.7.0 behavior (full mode, no caps,
// all refreshes on).
// ---------------------------------------------------------------------------
const AOS_DEFAULT_RUN_OPTIONS = {
    mode: 'full',
    maxOrders: 0,        // 0 = all
    timeBudgetSec: 0,    // 0 = no budget
    refresh: { prices: true, votes: true, details: true }
};
let aosRunOptions = null; // set only by the headless autosync branch

function normalizeRunOptions(raw) {
    const o = (raw && typeof raw === 'object') ? raw : {};
    const r = (o.refresh && typeof o.refresh === 'object') ? o.refresh : {};
    return {
        mode: (o.mode === 'quick' || o.mode === 'catchup') ? o.mode : 'full',
        maxOrders: (Number.isInteger(o.maxOrders) && o.maxOrders > 0) ? o.maxOrders : 0,
        timeBudgetSec: (Number.isInteger(o.timeBudgetSec) && o.timeBudgetSec > 0) ? o.timeBudgetSec : 0,
        refresh: {
            prices: r.prices !== false,
            votes: r.votes !== false,
            details: r.details !== false
        }
    };
}

function activeRunOptions() {
    return aosRunOptions || AOS_DEFAULT_RUN_OPTIONS;
}

async function fetchRunOptions() {
    try {
        const res = await aosFetch('/api/run-options');
        if (!res.ok) throw new Error('HTTP ' + res.status);
        const data = await res.json();
        aosRunOptions = normalizeRunOptions(data && data.options);
    } catch (e) {
        aosRunOptions = null; // full defaults
        logStatus(`[Options] Run options unavailable (${e.message}); using full defaults.`);
    }
    const o = activeRunOptions();
    logStatus(`[Options] mode=${o.mode} maxOrders=${o.maxOrders || 'all'} ` +
        `timeBudgetSec=${o.timeBudgetSec || 'none'} refresh=` +
        `prices:${o.refresh.prices} votes:${o.refresh.votes} details:${o.refresh.details}`);
}

async function postHeadlessComplete(body) {
    // v1.7.1 (§19.2): final log flush right before the completion ping.
    await flushClientLogs();
    await aosFetch('/api/headless-complete', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body)
    }).then(res => {
        logStatus(`[Headless] Completion ping sent (HTTP ${res.status}).`);
    }).catch(e => {
        logStatus(`[Headless] Failed to send completion ping: ${e.message}`);
    });
    // v1.7.2 (§20.6): shut log forwarding down — clear the 3s interval, drain
    // the buffer completely (looping posts), reset the forwarding flag.
    if (aosForwardLogs) await stopClientLogForwarding();
}

async function runHeadlessAutosyncPass() {
    if (aosHeadlessFired) return;
    aosHeadlessFired = true;
    try {
        // Amazon's sign-in wall: report and stop instead of scraping a login page.
        if (document.querySelector('form[name="signIn"], #ap_email, #authportal-center-section')) {
            sessionStorage.removeItem(AOS_HEADLESS_STATE_KEY); // v1.9.1: run over — no resume
            logStatus('[Headless] Sign-in page detected; reporting login-required.');
            await postHeadlessComplete({ ok: false, error: 'login-required' });
            return;
        }
        isSyncStopped = false;
        logStatus('--- Headless auto-sync pass started (#aos-autosync) ---');
        // v1.9.1 (§28.2): resolve the cross-page run state BEFORE processing.
        // Token match → this load is a pagination hop of the same run:
        // restore the counters. Mismatch (or no stored state, or a legacy
        // tokenless hash) → fresh run: clear stale state, zero the counters.
        const runToken = aosParseRunToken();
        const stored = aosReadHeadlessState();
        if (runToken && stored && stored.token === runToken) {
            aosHeadlessRunState = {
                token: runToken,
                sentCount: stored.sentCount || 0,
                runStartMs: stored.runStartMs || Date.now(),
                pageNum: stored.pageNum || 1
            };
            logStatus(`[Page] Resumed on orders page ${aosHeadlessRunState.pageNum} — ${aosHeadlessRunState.sentCount} orders sent so far`);
        } else {
            sessionStorage.removeItem(AOS_HEADLESS_STATE_KEY);
            aosHeadlessRunState = { token: runToken, sentCount: 0, runStartMs: Date.now(), pageNum: 1 };
        }
        // v1.7.0 (§18.2): fetch this run's options once, before processing.
        // (Per page load — the same run's options; nothing is double-counted
        // because sentCount/runStartMs are restored, not re-accumulated.)
        await fetchRunOptions();
        const sentCount = await extractAndSyncOrders(false, aosHeadlessRunState);
        if (aosHeadlessPaginating) {
            // Pagination hop in flight — the completion ping fires exactly
            // once, on the final page's finish path, never here.
            logStatus('[Headless] Moving to the next orders page; completion ping deferred.');
            return;
        }
        logStatus(`[Headless] Pass complete; ${sentCount || 0} order(s) sent to local server.`);
        await postHeadlessComplete({ ok: true, processed: sentCount || 0 });
    } catch (e) {
        sessionStorage.removeItem(AOS_HEADLESS_STATE_KEY); // run over — no resume
        logStatus('[Headless] Pass failed: ' + e.message);
        await postHeadlessComplete({ ok: false, error: e.message || 'unknown' });
    } finally {
        aosHeadlessRunState = null;
    }
}

if (AOS_HEADLESS_AUTOSYNC) {
    // v1.8.4 (§24.2): drop any stale manual auto-sync flag BEFORE scheduling
    // — a leftover 'amazon_sync_auto' from manual browsing in a reused tab
    // would otherwise double-schedule a second pass alongside this run.
    sessionStorage.removeItem('amazon_sync_auto');
    startClientLogForwarding(); // v1.7.1 (§19.2)
    logStatus('Headless auto-sync hash detected; will start after page settles (2s).');
    const scheduleHeadless = () => setTimeout(runHeadlessAutosyncPass, Math.round(2000 + Math.random() * 2000));
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', scheduleHeadless);
    } else {
        scheduleHeadless();
    }
}

// ---------------------------------------------------------------------------
// v1.8.1: fast login-required detection on the sign-in page itself. When a
// headless run's Amazon session has expired, Amazon redirects the run URL to
// /ap/signin — the URL fragment survives the redirect, but none of the run
// branches above match that path, so the run previously stalled in silence
// until the daemon's 6-minute timeout. Now, on any /ap/signin* page carrying
// one of the headless hashes, we settle after page load, report
// login-required ONCE, and stop. Everything is gated on the hash — normal
// sign-in browsing is completely untouched.
// ---------------------------------------------------------------------------
const AOS_SIGNIN_PAGE = window.location.pathname.startsWith('/ap/signin');
const AOS_HEADLESS_HASH = window.location.hash.includes('aos-autosync') ||
    window.location.hash.includes('aos-reviewcheck');
let aosSigninReported = false; // reports exactly once per page load

async function reportSigninLoginRequired() {
    if (aosSigninReported) return;
    aosSigninReported = true;
    logStatus('[Headless] On the Amazon sign-in page; session expired — reporting login-required.');
    await postHeadlessComplete({ ok: false, error: 'login-required' });
}

if (AOS_SIGNIN_PAGE && AOS_HEADLESS_HASH) {
    startClientLogForwarding();
    const scheduleSigninReport = () => setTimeout(reportSigninLoginRequired, 2000);
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', scheduleSigninReport);
    } else {
        scheduleSigninReport();
    }
}

// ---------------------------------------------------------------------------
// v1.6.0: headless review-checker trigger (SPEC §16.7). When a create-review
// page was opened by the local daemon with `#aos-reviewcheck`, read the LIVE
// DOM with the same selectors/regexes fetchReviewDetails uses, POST the
// result to /api/review-check-result, then advance to the next ASIN in
// /api/review-check-queue (or ping completion when the queue is exhausted).
// Everything is gated on the hash; normal browsing is completely unaffected.
// ---------------------------------------------------------------------------
const AOS_REVIEW_CHECK_HASH = window.location.hash.includes('aos-reviewcheck');
const AOS_REVIEW_CHECK = AOS_REVIEW_CHECK_HASH &&
    window.location.pathname.includes('/review/create-review');
let aosReviewCheckFired = false; // fires exactly once per page load
const AOS_RV_PROCESSED_KEY = 'aos_reviewcheck_processed';
// v1.8.4 (§24.2): the daemon stamps every review-check launch with a fresh
// per-run token (#aos-reviewcheck-<ms>); the token of the run this tab is
// serving lives under AOS_RV_RUN_KEY so a NEW run's first page can be told
// apart from a continuation page of the same run. '' = tokenless hash
// (older daemon): keep the pre-1.8.4 same-tab behavior.
const AOS_RV_RUN_KEY = 'aos_reviewcheck_run';
const AOS_RV_RUN_TOKEN = (window.location.hash.match(/aos-reviewcheck-(\d+)/) || [])[1] || '';

function readLiveReviewForm() {
    // Same selectors/regexes as fetchReviewDetailsOnce, against the live DOM
    // of the current page instead of a fetched document.
    const html = document.documentElement ? document.documentElement.outerHTML : '';

    let rating = "";
    let title = "";
    let text = "";

    const ratingInput = document.querySelector('input[name="rating"]:checked') || document.querySelector('.ryp-star-rating-selected');
    if (ratingInput && ratingInput.value) rating = ratingInput.value;
    else if (ratingInput && ratingInput.className) {
        const classMatch = ratingInput.className.match(/star-(\d)/);
        if (classMatch) rating = classMatch[1];
    }

    const titleInput = document.querySelector('input[name="reviewTitle"], input[id*="review-title"], .ryp-review-title-input');
    if (titleInput) title = titleInput.value || "";

    const textInput = document.querySelector('textarea[name="reviewText"], textarea[id*="review-text"], .ryp-review-text-input');
    if (textInput) text = textInput.value || textInput.textContent || "";

    // Regex fallbacks for React/SPA rendered pages (same blobs as fetchReviewDetailsOnce).
    if (!rating || rating === "Unknown") {
        const rMatch = html.match(/"initialRating"\s*:\s*(\d)/) || html.match(/"customerReview":\{.*?"rating":(\d)/);
        if (rMatch) rating = rMatch[1];
    }
    if (!title || title === "Unknown") {
        const tMatch = html.match(/"initialTitle"\s*:\s*"((?:\\.|[^"\\])*)"/);
        if (tMatch) title = tMatch[1].replace(/\\n/g, '\n').replace(/\\"/g, '"');
    }
    if (!text || text === "Unknown") {
        const txtMatch = html.match(/"initialBody"\s*:\s*"((?:\\.|[^"\\])*)"/)
                      || html.match(/"initialReviewText"\s*:\s*"((?:\\.|[^"\\])*)"/)
                      || html.match(/"reviewText"\s*:\s*"((?:\\.|[^"\\])*)"/);
        if (txtMatch) text = txtMatch[1].replace(/\\n/g, '\n').replace(/\\"/g, '"');
    }

    // Helpful-vote count (same regexes as fetchReviewDetailsOnce).
    let helpful_votes = null;
    const hvMatch = html.match(/"helpfulVotes"\s*:\s*(\d+)/)
                 || html.match(/(\d+)\s+people found this helpful/i);
    if (hvMatch) {
        helpful_votes = parseInt(hvMatch[1], 10);
    } else if (/One person found this helpful/i.test(html)) {
        helpful_votes = 1;
    }
    if (!Number.isInteger(helpful_votes) || helpful_votes < 0) helpful_votes = null;

    return { rating: rating || "Unknown", title: title || "Unknown", text: text || "Unknown", helpful_votes };
}

// v1.8.0 (§21.1): page-state classification for the review-check branch.
// Computed BEFORE the review form is read — a captcha wall or an error page
// also shows an "empty form", and must not count as a missing-review
// sighting. 'captcha' wins over 'error' (a captcha page can have a
// near-empty body too); anything else with real content is 'loaded'. The
// server treats a non-'loaded' page_state as inconclusive (no sighting).
function classifyReviewPageState() {
    const bodyText = document.body ? (document.body.innerText || document.body.textContent || '') : '';
    if (document.querySelector('form[action*="validateCaptcha"], #captchacharacters') ||
        bodyText.includes('Enter the characters you see below')) {
        return 'captcha';
    }
    const title = document.title || '';
    if (title.includes('Page Not Found') || title.includes('Sorry!') || bodyText.trim().length < 50) {
        return 'error';
    }
    return 'loaded';
}

function getReviewCheckProcessed() {
    try {
        const arr = JSON.parse(sessionStorage.getItem(AOS_RV_PROCESSED_KEY) || '[]');
        return Array.isArray(arr) ? arr : [];
    } catch (e) {
        return [];
    }
}

async function runReviewCheckPass() {
    if (aosReviewCheckFired) return;
    aosReviewCheckFired = true;
    try {
        // v1.8.4 (§24.2): RUN START clear of the processed-set (in addition
        // to the exhaustion clear below). The first page of a run — its
        // launch token differs from the token stored in this tab — drops any
        // processed-set a crashed/killed run left in sessionStorage, so a
        // reused tab can't skip items. Continuation pages of the same run
        // carry the same token and keep the set. Skipped for tokenless
        // hashes (can't distinguish run start from a continuation there).
        if (AOS_RV_RUN_TOKEN !== '' && sessionStorage.getItem(AOS_RV_RUN_KEY) !== AOS_RV_RUN_TOKEN) {
            sessionStorage.removeItem(AOS_RV_PROCESSED_KEY);
            sessionStorage.setItem(AOS_RV_RUN_KEY, AOS_RV_RUN_TOKEN);
        }
        // Amazon's sign-in wall: report and stop instead of scraping a login page.
        if (document.querySelector('form[name="signIn"], #ap_email, #authportal-center-section')) {
            logStatus('[ReviewCheck] Sign-in page detected; reporting login-required.');
            await postHeadlessComplete({ ok: false, error: 'login-required' });
            return;
        }
        const asin = new URLSearchParams(window.location.search).get('asin');
        logStatus(`--- Headless review-check pass started (#aos-reviewcheck, ASIN ${asin || '?'}) ---`);

        // v1.8.0 (§21.1): classify the page BEFORE reading the form so the
        // server can tell a truly-empty review form from a page that never
        // loaded (captcha/error → inconclusive, no missing-review sighting).
        const pageState = classifyReviewPageState();
        if (pageState !== 'loaded') {
            logStatus(`[ReviewCheck] Page state is '${pageState}'; form contents are not trustworthy.`);
        }

        const details = readLiveReviewForm();
        const reviewed = !!(details.title && details.text &&
            details.title !== "Unknown" && details.text !== "Unknown");
        const resultBody = {
            asin,
            reviewed,
            title: reviewed ? details.title : "",
            text: reviewed ? details.text : "",
            rating: details.rating || "",
            helpful_votes: Number.isInteger(details.helpful_votes) ? details.helpful_votes : null,
            page_state: pageState
        };
        const resRes = await aosFetch('/api/review-check-result', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(resultBody)
        });
        logStatus(`[ReviewCheck] Result for ASIN ${asin} sent (HTTP ${resRes.status}, reviewed=${reviewed}, page_state=${pageState}).`);

        // Track processed ASINs for this run so a reload loop can't repeat one.
        const processed = getReviewCheckProcessed();
        if (asin && !processed.includes(asin)) processed.push(asin);
        sessionStorage.setItem(AOS_RV_PROCESSED_KEY, JSON.stringify(processed));

        logStatus('Waiting (bot protection)...');
        await BOT_WAIT();

        const queueRes = await aosFetch('/api/review-check-queue');
        const queue = await queueRes.json();
        const asins = Array.isArray(queue.asins) ? queue.asins : [];
        const next = asins.find(a => !processed.includes(a));
        if (next) {
            logStatus(`[ReviewCheck] Advancing to next ASIN: ${next}.`);
            // v1.8.4 (§24.2): keep the CURRENT hash (including the v1.8.4
            // per-run token) so the next page is recognized as a
            // continuation of this run, not a new run start.
            location.href = '/review/create-review?asin=' + next + window.location.hash;
            return;
        }
        logStatus(`[ReviewCheck] Queue exhausted; ${processed.length} ASIN(s) processed.`);
        // v1.8.3 (§23.4, audit B4): the run is over — clear the processed-set
        // so the NEXT review-check run starts fresh (sessionStorage would
        // otherwise carry stale ASINs across runs in the same tab session).
        sessionStorage.removeItem(AOS_RV_PROCESSED_KEY);
        await postHeadlessComplete({ ok: true, processed: processed.length });
    } catch (e) {
        logStatus('[ReviewCheck] Pass failed: ' + e.message);
        await postHeadlessComplete({ ok: false, error: e.message || 'unknown' });
    }
}

if (AOS_REVIEW_CHECK) {
    startClientLogForwarding(); // v1.7.1 (§19.2)
    logStatus('Review-check hash detected; will start after page settles (2s).');
    const scheduleReviewCheck = () => setTimeout(runReviewCheckPass, Math.round(2000 + Math.random() * 2000));
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', scheduleReviewCheck);
    } else {
        scheduleReviewCheck();
    }
} else if (AOS_REVIEW_CHECK_HASH && !AOS_SIGNIN_PAGE) {
    // v1.7.2 (§20.5): the hash is present but Amazon redirected us away from
    // /review/create-review (interstitial, moved URL, etc.). Report the
    // redirect ONCE and stop instead of hanging until the daemon's timeout.
    startClientLogForwarding();
    logStatus('[ReviewCheck] Hash present but this is not the create-review page (redirected).');
    const reportReviewRedirect = async () => {
        if (aosReviewCheckFired) return;
        aosReviewCheckFired = true;
        await postHeadlessComplete({ ok: false, error: 'redirected — Amazon moved the review page' });
    };
    const scheduleRedirectReport = () => setTimeout(reportReviewRedirect, 2000);
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', scheduleRedirectReport);
    } else {
        scheduleRedirectReport();
    }
}

// Auto-Fill Review Logic
// v1.8.3 (§23.1, audit B1 — HIGH): skip the ENTIRE auto-fill block during
// headless passes (#aos-autosync / #aos-reviewcheck). Headless passes READ the
// page (readLiveReviewForm); injecting the note's own draft into the form
// before that read makes items falsely report as reviewed and breaks
// missing-review detection. Without a headless hash this gate is always true
// and normal browsing auto-fill is byte-for-byte unaffected.
if (!AOS_HEADLESS_HASH && !AOS_REVIEW_CHECK_HASH &&
    (window.location.href.includes('/review/create-review') || window.location.href.includes('/review/review-your-purchases'))) {
    logStatus("On Amazon Review Page. Attempting Auto-Fill from local server...");
    const params = new URLSearchParams(window.location.search);
    const asin = params.get('asin');
    if (asin) {
        aosFetch(`/api/get-review?asin=${encodeURIComponent(asin)}`)
            .then(res => res.json())
            .then(data => {
                if (data.title && data.text) {
                    logStatus(`Found AI Review in local server for ASIN ${asin}! Injecting...`);
                    
                    const tryInject = setInterval(() => {
                        const titleInput = document.querySelector('input[name="reviewTitle"], input[id*="review-title"], .ryp-review-title-input');
                        const textInput = document.querySelector('textarea[name="reviewText"], textarea[id*="review-text"], .ryp-review-text-input');
                        // The draft's own rating, never a fixed 5.
                        const stars = /^[1-5]$/.test(String(data.rating)) ? String(data.rating) : '';
                        const ratingBtn = stars ? document.querySelector(`button[data-hook="ryp-star-rating-${stars}"], img[alt="${stars} stars"], .ryp-star-rating-button[data-hook*="rating-${stars}"]`) : null;
                        
                        if (titleInput && textInput) {
                            clearInterval(tryInject);
                            
                            // Amazon's form is React-controlled: assigning .value
                            // doesn't reach React's state, so use the native
                            // setter and then fire input/change.
                            setNativeValue(titleInput, data.title);
                            setNativeValue(textInput, data.text);

                            if (ratingBtn) ratingBtn.click();
                            
                            logStatus("Review successfully auto-filled!");
                        }
                    }, 500);
                    
                    setTimeout(() => clearInterval(tryInject), 10000);
                } else {
                    logStatus("No AI review found in local server for this ASIN.");
                }
            }).catch(e => logStatus("Failed to contact local server for Auto-Fill: " + e.message));
    }
}

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
    if (request.action === "trigger_sync") {
        isSyncStopped = false;
        logStatus("--- Started Manual Sync ---");
        logStatus("Starting manual sync on current page...");
        extractAndSyncOrders(false).then(() => {
            sendResponse({success: true});
        }).catch(err => {
            logStatus("Error: " + err.message);
            sendResponse({success: false, error: err.message});
        });
        return true;
    } else if (request.action === "start_auto_sync") {
        isSyncStopped = false;
        logStatus("--- Started Auto-Sync ---");
        logStatus("Starting auto-sync...");
        sessionStorage.setItem('amazon_sync_auto', 'true');
        // v1.8.0 (§21.2 H7): the missing .catch — a rejected pass previously
        // left sendResponse hanging forever (and the error unlogged).
        extractAndSyncOrders(true).then(() => {
            sendResponse({success: true});
        }).catch(err => {
            logStatus("Error: " + err.message);
            sendResponse({success: false, error: err.message});
        });
        return true;
    } else if (request.action === "stop_auto_sync") {
        // v1.8.3 (§23.3, audit B2): set ONLY isSyncStopped here — isSyncing is
        // owned by the extractAndSyncOrders wrapper's finally on every exit
        // path. Writing it here opened a stop→quick-restart double-pass
        // window (restart accepted while the old pass was still unwinding).
        isSyncStopped = true;
        logStatus("Sync stopped by user.");
        sessionStorage.removeItem('amazon_sync_auto');
        sendResponse({success: true});
        return true;
    } else if (request.action === "check_auto_sync_state") {
        sendResponse({
            isAutoSyncing: sessionStorage.getItem('amazon_sync_auto') === 'true',
            isSyncing: isSyncing,
            // v1.7.2 (§20.5): explicit run-state signal so the popup never has
            // to infer state from log text. True during a single OR auto sync.
            syncRunning: isSyncing,
            logs: syncLogs
        });
        return true;
    }
});

const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));
// v1.8.11: bot protection pacing — randomized waits (jitter) instead of
// metronome-fixed delays, and longer overall. Predictable 2.000s gaps are
// the easiest automation fingerprint Amazon has; humans don't metronome.
const jitterSleep = (minMs, maxMs) =>
    sleep(Math.round(minMs + Math.random() * (maxMs - minMs)));
const BOT_WAIT = () => jitterSleep(3000, 6000);      // between per-item scrapes
const BOT_RETRY = () => jitterSleep(2000, 3500);     // inconclusive review retry
const BOT_SETTLE = () => jitterSleep(2000, 4000);    // page-load settle
const BOT_NAV = () => jitterSleep(4000, 7000);       // pagination / year nav

async function fetchProductDetails(url) {
    try {
        logStatus(`Fetching deep product details for: ${url.split('/dp/')[1] || url}`);
        const response = await fetch(url);
        if (!response.ok) return null;
        const html = await response.text();
        const doc = new DOMParser().parseFromString(html, "text/html");
        
        let brand = doc.querySelector('#bylineInfo, #brand')?.textContent.trim() || 'Unknown';
        if (brand.startsWith('Visit the ') || brand.startsWith('Brand: ')) {
            brand = brand.replace('Visit the ', '').replace(' Store', '').replace('Brand: ', '').trim();
        }
        
        const price = doc.querySelector('.a-price .a-offscreen')?.textContent.trim() || 'Unknown';
        
        const featureBullets = Array.from(doc.querySelectorAll('#feature-bullets li span.a-list-item'))
                                   .map(el => el.textContent.trim())
                                   .join('\n- ');

        // Voice-of-customer context: up to 3 top customer review bodies from the
        // product page (trimmed, non-empty, <=500 chars each). Sent to the local
        // server as `top_reviews` so generated drafts can pick up authentic
        // usage details. The content is untrusted; the server treats it as
        // reference material only.
        const topReviews = [];
        const reviewNodes = doc.querySelectorAll('[data-hook="review-body"] span, .review-text-content span');
        for (const node of reviewNodes) {
            const body = node.textContent.trim();
            if (!body) continue;
            topReviews.push(body.length > 500 ? body.slice(0, 500) : body);
            if (topReviews.length >= 3) break;
        }

        return { brand, current_price: price, features: featureBullets, top_reviews: topReviews };
    } catch (e) {
        logStatus("Error fetching product page: " + e.message);
        return null;
    }
}

// Single attempt at reading the review form for an ASIN. Uses a same-origin
// relative URL so it works on any regional Amazon domain (amazon.com,
// amazon.co.uk, amazon.de, amazon.ca, ...).
async function fetchReviewDetailsOnce(asin) {
    try {
        logStatus(`Fetching review details for ASIN: ${asin}...`);
        const response = await fetch(`/review/create-review?asin=${asin}`);
        if (!response.ok) return null;
        const html = await response.text();
        const doc = new DOMParser().parseFromString(html, "text/html");
        
        let rating = "";
        let title = "";
        let text = "";
        
        const ratingInput = doc.querySelector('input[name="rating"]:checked') || doc.querySelector('.ryp-star-rating-selected');
        if (ratingInput && ratingInput.value) rating = ratingInput.value;
        else if (ratingInput && ratingInput.className) {
            const classMatch = ratingInput.className.match(/star-(\d)/);
            if (classMatch) rating = classMatch[1];
        }

        const titleInput = doc.querySelector('input[name="reviewTitle"], input[id*="review-title"], .ryp-review-title-input');
        if (titleInput) title = titleInput.value || "";

        const textInput = doc.querySelector('textarea[name="reviewText"], textarea[id*="review-text"], .ryp-review-text-input');
        if (textInput) text = textInput.value || textInput.textContent || "";

        // 2. Try Regex fallback (for React/SPA rendered pages)
        if (!rating || rating === "Unknown") {
            const rMatch = html.match(/"initialRating"\s*:\s*(\d)/) || html.match(/"customerReview":\{.*?"rating":(\d)/);
            if (rMatch) rating = rMatch[1];
        }
        if (!title || title === "Unknown") {
            const tMatch = html.match(/"initialTitle"\s*:\s*"((?:\\.|[^"\\])*)"/);
            if (tMatch) title = tMatch[1].replace(/\\n/g, '\n').replace(/\\"/g, '"');
        }
        if (!text || text === "Unknown") {
            const txtMatch = html.match(/"initialBody"\s*:\s*"((?:\\.|[^"\\])*)"/)
                          || html.match(/"initialReviewText"\s*:\s*"((?:\\.|[^"\\])*)"/)
                          || html.match(/"reviewText"\s*:\s*"((?:\\.|[^"\\])*)"/);
            if (txtMatch) text = txtMatch[1].replace(/\\n/g, '\n').replace(/\\"/g, '"');
        }

        // 3. Try parsing the new window.P.initialPageState JSON (used in Review Your Purchases redirect)
        if (!title || title === "Unknown" || title.includes("DEBUG")) {
            const pageStateMatch = html.match(/window\.P\.initialPageState\s*=\s*(\{.*?\});/);
            if (pageStateMatch) {
                try {
                    const state = JSON.parse(pageStateMatch[1]);
                    if (state && state.reviewForm) {
                        if (state.reviewForm.rating) rating = String(state.reviewForm.rating);
                        if (state.reviewForm.title) title = state.reviewForm.title;
                        if (state.reviewForm.text) text = state.reviewForm.text;
                    }
                } catch (e) {
                    console.error("Failed to parse initialPageState", e);
                }
            }
        }

        // v1.3.0: helpful-vote count from the same review page. Omitted when
        // absent/unparseable so older servers see an unchanged payload.
        let helpful_votes = null;
        const hvMatch = html.match(/"helpfulVotes"\s*:\s*(\d+)/)
                     || html.match(/(\d+)\s+people found this helpful/i);
        if (hvMatch) {
            helpful_votes = parseInt(hvMatch[1], 10);
        } else if (/One person found this helpful/i.test(html)) {
            helpful_votes = 1;
        }
        if (!Number.isInteger(helpful_votes) || helpful_votes < 0) helpful_votes = null;

        const result = { rating: rating || "Unknown", title: title || "Unknown", text: text || "Unknown" };
        if (helpful_votes !== null) result.helpful_votes = helpful_votes;
        return result;
    } catch (e) {
        logStatus(`Error fetching review for ${asin}: ${e.message}`);
        return null;
    }
}

// Hardened wrapper: if the first attempt comes back null or ambiguous (no
// real title+text), the page may still have been rendering its React/SPA
// state — wait 1500 ms and retry ONCE before concluding "not reviewed".
// Semantics are unchanged: only a real title+text counts as reviewed.
async function fetchReviewDetails(asin) {
    let result = await fetchReviewDetailsOnce(asin);
    const ambiguous = !result || result.title === "Unknown" || result.text === "Unknown";
    if (ambiguous) {
        logStatus(`Review details for ASIN ${asin} inconclusive; retrying once shortly...`);
        await BOT_RETRY();
        const retry = await fetchReviewDetailsOnce(asin);
        if (retry) result = retry;
    }
    return result;
}

async function checkOrderComplete(orderId) {
    logStatus(`[Server] Checking status for Order ${orderId}...`);
    try {
        const response = await aosFetch(`/api/check?order_id=${encodeURIComponent(orderId)}`);
        if (!response.ok) {
            logStatus(`[Server] Failed to check status for Order ${orderId} (HTTP status ${response.status}).`);
            return { complete: false, reason: `HTTP error ${response.status}` };
        }
        const data = await response.json();
        if (data.complete) {
            logStatus(`[Server] Order ${orderId} status: Complete.`);
        } else {
            logStatus(`[Server] Order ${orderId} status: Incomplete. Reason: ${data.reason || 'Needs update'}`);
        }
        return data;
    } catch (e) {
        logStatus(`[Server] Could not connect to local server for Order ${orderId}: ${e.message}`);
        return { complete: false, reason: `Connection error: ${e.message}` };
    }
}

// v1.8.0 (§21.2 H7): the re-entry guard and the isSyncing flag live in this
// wrapper; the finally block resets the flag on EVERY exit path of the pass.
// (The old isSyncStopped early-return inside the item loop skipped the
// reset, leaving every later sync permanently blocked.)
async function extractAndSyncOrders(isAuto, restored) {
    // v1.7.2 (§20.5): re-entry guard — a second Start while a sync is running
    // is ignored (logged), never a concurrent pass over the same page.
    if (isSyncing) {
        logStatus("Sync already running; ignoring duplicate start request.");
        return 0;
    }
    isSyncing = true;
    try {
        return await extractAndSyncOrdersPass(isAuto, restored);
    } finally {
        isSyncing = false;
    }
}

async function extractAndSyncOrdersPass(isAuto, restored) {
    logStatus("Scanning for order cards...");

    const orderCards = document.querySelectorAll('.order-card, .yohtmlc-order');
    logStatus(`Found ${orderCards.length} orders on this page.`);

    // v1.7.0 (§18.2): run options. Defaults for manual/popup syncs are
    // full/zero/all-true — behavior identical to pre-1.7.0.
    const runOpts = activeRunOptions();
    // v1.9.1 (§28.2): on a headless pagination continuation the restored
    // counters are threaded INTO the same runStartMs/sentCount variables, so
    // every existing maxOrders/timeBudgetSec check below spans pages
    // untouched. Manual/popup syncs pass no `restored` → fresh counters.
    const runStartMs = (restored && restored.runStartMs) ? restored.runStartMs : Date.now();

    let processedCount = 0;
    let sentCount = (restored && restored.sentCount) ? restored.sentCount : 0; // orders actually POSTed to /api/sync (v1.5.0 headless reporting)

    for (const card of orderCards) {
        if (isSyncStopped) {
            logStatus("Aborting sync loop.");
            return; // v1.8.0 (§21.2 H7): isSyncing resets in the wrapper's finally
        }
        // v1.7.0 (§18.2): time budget — checked before each NEW order.
        if (runOpts.timeBudgetSec > 0 &&
            Date.now() - runStartMs > runOpts.timeBudgetSec * 1000) {
            logStatus(`[Budget] Time budget of ${runOpts.timeBudgetSec}s exceeded; stopping pass cleanly.`);
            break;
        }
        try {
            const cardText = card.textContent.replace(/\s+/g, ' ');
            
            // Find Order ID using strict Regex
            let orderId = null;
            const match = cardText.match(/\d{3}-\d{7}-\d{7}/);
            if (match) {
                orderId = match[0];
            } else {
                let orderIdMatch = cardText.match(/ORDER #\s*([0-9-]+)/i) || cardText.match(/Order [Nn]umber\s*([0-9-]+)/i);
                if (orderIdMatch) orderId = orderIdMatch[1].trim();
            }

            if (!orderId || orderId.length < 10) {
                orderId = "Unknown";
            }

            if (orderId === "Unknown") {
                logStatus("Could not find order ID. Skipping...");
                continue;
            }

            processedCount++;
            logStatus(`[${processedCount}/${orderCards.length}] Processing Order ${orderId}...`);
            
            // v1.7.2 (§20.5): unmatched date → "" (NEVER today's date — the
            // server files unparseable dates at the Orders/ root instead of
            // silently corrupting the note with a wrong date).
            let dateMatch = cardText.match(/ORDER PLACED\s+([A-Za-z]+\s+\d{1,2},\s+\d{4})/i);
            let date = dateMatch ? dateMatch[1].trim() : "";

            // SMART PRE-CHECK
            const checkResult = await checkOrderComplete(orderId);
            // v1.9.0: catch-up mode — skip ANY order the daemon already has
            // a note for (complete or not). Time-boxed runs then spend their
            // whole budget on never-seen orders instead of re-scraping the
            // same incomplete ones; the frontier moves back each run.
            if (runOpts.mode === 'catchup' && checkResult && checkResult.exists === true) {
                logStatus(`[Skip] Order ${orderId} seen before (catch-up mode)`);
                continue;
            }
            if (checkResult && checkResult.complete === true) {
                // v1.7.0 (§18.2): quick mode's signature log line; either way
                // the order is skipped entirely (no detail/vote fetches).
                if (runOpts.mode === 'quick') {
                    logStatus(`[Skip] Order ${orderId} already complete`);
                } else {
                    logStatus(`[Action] Skipping Order ${orderId} (already up-to-date in local server).`);
                }
                continue;
            }

            logStatus(`[Action] Syncing Order ${orderId} (extracting details)...`);
            
            // v1.7.2 (§20.5): regional currencies $ £ € ¥, symbol kept in the
            // string; unmatched → "" (NEVER a fake "$0.00").
            let totalMatch = cardText.match(/(?:TOTAL|ORDER TOTAL)\s*([$£€¥]\s?[0-9,.]+)/i);
            let total = totalMatch ? totalMatch[1].replace(/\s+/, '').trim() : "";
            
            // v2.1.0: the recipient is a popover trigger whose hidden preload
            // repeats the name plus the full street address — textContent
            // therefore reads "Name Name 123 Street ...". Prefer the visible
            // trigger link; keep the text regex as a layout-change fallback.
            let shipTo = "Unknown";
            const recipientLink = card.querySelector('.yohtmlc-recipient a.a-popover-trigger, .yohtmlc-recipient .a-popover-trigger');
            if (recipientLink && recipientLink.textContent.trim()) {
                shipTo = recipientLink.textContent.replace(/\s+/g, ' ').trim();
            } else {
                let shipToMatch = cardText.match(/SHIP TO\s+(.+?)\s+(?:ORDER #|TOTAL|View order details|Order Details)/i);
                if (shipToMatch) shipTo = shipToMatch[1].trim();
            }

            let status = "Unknown";
            const statusElements = Array.from(card.querySelectorAll('.a-text-bold, h2, h3, .a-size-medium, .js-shipment-info-container span, .a-color-success, .a-color-state'));
            const statusCandidate = statusElements.find(el => el.textContent.match(/Arriving|Delivered|Shipped|Expected|Out for delivery|Now expected|Return window/i));
            if (statusCandidate) {
                status = statusCandidate.textContent.trim();
            }

            const items = [];
            let links = Array.from(card.querySelectorAll('a.a-link-normal'));
            links = links.filter(l => l.href.includes('/dp/') || l.href.includes('/product/'));
            
            const seenAsins = new Set();
            for (const link of links) {
                if (isSyncStopped) return; // v1.8.0 (§21.2 H7): finally resets isSyncing (was the leak)
                const url = new URL(link.href);
                const match = url.pathname.match(/\/(dp|product)\/([A-Z0-9]{10})/);
                if (match) {
                    const asin = match[2];
                    if (!seenAsins.has(asin)) {
                        seenAsins.add(asin);
                        const img = link.querySelector('img');
                        const name = img ? img.alt : link.textContent.trim() || `Product ${asin}`;
                        const image = img ? img.src : '';
                        
                        let is_reviewed = false;
                        const itemContainer = link.closest('.a-fixed-left-grid, .yohtmlc-item, .a-row.a-spacing-base') || card;
                        const itemText = itemContainer.textContent.toLowerCase();
                        
                        // If there's no prompt to write a review, it MIGHT be reviewed (or it might be non-reviewable)
                        if (!itemText.includes("write a product review") && !itemText.includes("review your purchases")) {
                            is_reviewed = true;
                        }
                        
                        // Retrieve existing item details to skip redundant scrapes and API queries
                        let brand = "Unknown";
                        let current_price = "Unknown";
                        let features = "";
                        let topReviews = [];
                        let hasDeepDetails = false;
                        let listingPrice = null; // v1.6.0: live product-page price (SPEC §16.2)
                        
                        const existing = checkResult?.existingItems?.[asin];
                        if (existing && existing.brand && existing.brand !== "Unknown") {
                            brand = existing.brand;
                            current_price = existing.current_price;
                            features = existing.features;
                            hasDeepDetails = true;
                            logStatus(`[Skip] Product page scrape skipped for ASIN ${asin} (already in local server).`);
                        }

                        // v1.7.0 (§18.2): the product page is a SINGLE fetch
                        // serving brand/features/listing_price/top_reviews, so
                        // it is fetched fully when EITHER refresh.prices or
                        // refresh.details is on; skipped only when BOTH are off
                        // (server preservation keeps the stored values).
                        const productFetchOn = runOpts.refresh.prices || runOpts.refresh.details;
                        if (!hasDeepDetails && !productFetchOn) {
                            logStatus(`[Skip] Product page fetch skipped for ASIN ${asin} (prices+details refresh off).`);
                        }
                        if (!hasDeepDetails && productFetchOn) {
                            // Fetch Deep Details
                            const cleanUrl = link.href.split('?')[0];
                            logStatus(`[Scrape] Fetching product page details for ASIN ${asin}...`);
                            const deepDetails = await fetchProductDetails(cleanUrl);
                            if (deepDetails) {
                                brand = deepDetails.brand || "Unknown";
                                current_price = deepDetails.current_price || "Unknown";
                                features = deepDetails.features || "";
                                topReviews = Array.isArray(deepDetails.top_reviews) ? deepDetails.top_reviews : [];
                                // v1.6.0: the product page's current price is the
                                // live LISTING price. `current_price` stays the
                                // order-card price (what was paid) — never mixed.
                                if (deepDetails.current_price && deepDetails.current_price !== "Unknown") {
                                    listingPrice = deepDetails.current_price;
                                }
                            }
                            logStatus(`Waiting (bot protection)...`);
                            await BOT_WAIT();
                        }
                        
                        let reviewDetails = null;
                        // For any item not already marked Posted, ask Amazon directly whether a
                        // review exists. The order-card heuristic above is only a hint and is
                        // wrong in both directions; the review page is authoritative. This is
                        // what makes "review on Amazon, then re-sync" tick the item off with
                        // no manual marking.
                        if (!runOpts.refresh.votes) {
                            // v1.7.0 (§18.2): votes refresh off → skip the
                            // review-status fetch entirely and fall back to the
                            // stored-state path (alreadyPosted / card hint).
                            logStatus(`[Skip] Review status check skipped for ASIN ${asin} (votes refresh off); using stored state.`);
                            if (existing && existing.alreadyPosted) is_reviewed = true;
                        } else if (!existing || !existing.alreadyPosted) {
                            logStatus(`[Scrape] Checking Amazon review status for ASIN ${asin}...`);
                            reviewDetails = await fetchReviewDetails(asin);

                            if (reviewDetails && reviewDetails.title !== "Unknown" && reviewDetails.text !== "Unknown") {
                                // A real posted review found on Amazon: import it and let the
                                // local server mark the item as posted automatically.
                                is_reviewed = true;
                                logStatus(`[Found] Existing Amazon review detected for ASIN ${asin}.`);
                            } else {
                                // No review on Amazon (empty form) or the check page failed
                                // to load. Treat as not reviewed.
                                is_reviewed = false;
                                reviewDetails = null;
                            }
                            logStatus(`Waiting (bot protection)...`);
                            await BOT_WAIT();
                        } else if (existing && existing.alreadyPosted) {
                            logStatus(`[Skip] Review status check skipped for ASIN ${asin} (already marked as Posted).`);
                            is_reviewed = true;
                        }
                        
                        const itemPayload = {
                            asin,
                            name,
                            link: link.href.split('?')[0],
                            image,
                            is_reviewed,
                            brand,
                            current_price,
                            features,
                            review_rating: reviewDetails?.rating || "",
                            review_title: reviewDetails?.title || "",
                            review_text: reviewDetails?.text || ""
                        };
                        // Optional voice-of-customer context (omitted when empty
                        // so older servers see an unchanged payload).
                        if (topReviews.length > 0) itemPayload.top_reviews = topReviews;
                        // v1.6.0: live listing price, only when freshly scraped
                        // from the product page this run (omitted otherwise).
                        if (listingPrice) itemPayload.listing_price = listingPrice;
                        // v1.3.0: helpful votes, only when a posted review was
                        // actually found and a count was parsed (reviewDetails
                        // is null otherwise). Older servers ignore the field.
                        if (reviewDetails && Number.isInteger(reviewDetails.helpful_votes) && reviewDetails.helpful_votes >= 0) {
                            itemPayload.helpful_votes = reviewDetails.helpful_votes;
                        }
                        items.push(itemPayload);
                    }
                }
            }

            const payload = {
                order_id: orderId,
                date,
                total,
                status,
                ship_to: shipTo,
                items
            };

            logStatus(`[Server] Sending payload for Order ${orderId} to local server...`);
            const syncResponse = await aosFetch('/api/sync', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify(payload)
            });
            // v1.8.3 (§23.4, audit B7): count only orders the server actually
            // accepted (2xx) — a failed POST no longer inflates the headless
            // completion ping's processed count / maxOrders cap accounting.
            if (syncResponse.ok) {
                sentCount++;
                logStatus(`[Server] Successfully synced Order ${orderId}!`);
            } else {
                logStatus(`[Server] Failed to sync Order ${orderId} (HTTP status ${syncResponse.status}).`);
            }

            // v1.7.0 (§18.2): maxOrders cap — stop after N orders POSTed; the
            // headless completion ping still reports the actual count.
            if (runOpts.maxOrders > 0 && sentCount >= runOpts.maxOrders) {
                logStatus(`[Limit] maxOrders=${runOpts.maxOrders} reached (${sentCount} order(s) sent); stopping pass.`);
                break;
            }

        } catch (e) {
            logStatus(`Error extracting order: ${e.message}`);
        }
    }
    
    if (isAuto && !isSyncStopped) {
        logStatus("Auto-sync: Checking for next page...");
        const nextBtn = document.querySelector('ul.a-pagination li.a-last a');
        if (nextBtn) {
            logStatus("Next page found! Navigating shortly...");
            await BOT_NAV();
            nextBtn.click();
        } else {
            logStatus("No next page found. Checking for previous year...");
            const timeFilter = document.querySelector('select[name="timeFilter"], #time-filter');
            if (timeFilter) {
                const nextOption = timeFilter.options[timeFilter.selectedIndex + 1];
                if (nextOption && nextOption.value.startsWith('year-')) {
                    logStatus(`Navigating to previous year: ${nextOption.text} shortly...`);
                    await BOT_NAV();
                    timeFilter.value = nextOption.value;
                    timeFilter.dispatchEvent(new Event('change'));
                    
                    // Fallback redirect if Amazon's JS fails
                    setTimeout(() => {
                        window.location.href = `/your-orders/orders?timeFilter=${nextOption.value}`;
                    }, 1000);
                    return;
                }
            }
            logStatus("No more history found. Auto-sync complete!");
            sessionStorage.removeItem('amazon_sync_auto');
            chrome.runtime.sendMessage({ action: "sync_complete" }).catch(() => {});
        }
        return sentCount;
    } else {
        // v1.9.1 (§28.2): multi-page headless sync. Before finishing, walk to
        // the next orders page when (a) this is a headless autosync run with
        // a per-run token (a legacy tokenless hash can never resume, so it
        // must finish here), (b) neither maxOrders nor timeBudgetSec has
        // been hit, (c) a next-page link exists, and (d) the 20-page loop
        // cap hasn't been reached. The next page load restores the counters
        // from sessionStorage and continues as the same run.
        if (AOS_HEADLESS_AUTOSYNC && aosHeadlessRunState && aosHeadlessRunState.token &&
                !isSyncStopped) {
            const limitsHit =
                (runOpts.maxOrders > 0 && sentCount >= runOpts.maxOrders) ||
                (runOpts.timeBudgetSec > 0 && Date.now() - runStartMs > runOpts.timeBudgetSec * 1000);
            const nextLink = document.querySelector('ul.a-pagination li.a-last a');
            const rawHref = nextLink ? (nextLink.getAttribute('href') || '') : '';
            if (!limitsHit && rawHref && aosHeadlessRunState.pageNum < AOS_HEADLESS_MAX_PAGES) {
                const nextPage = aosHeadlessRunState.pageNum + 1;
                sessionStorage.setItem(AOS_HEADLESS_STATE_KEY, JSON.stringify({
                    token: aosHeadlessRunState.token,
                    sentCount,
                    runStartMs,
                    pageNum: nextPage
                }));
                logStatus(`[Page] Moving to orders page ${nextPage}…`);
                aosHeadlessPaginating = true; // suppresses the completion ping
                await flushClientLogs();      // don't lose the last lines on unload
                await BOT_NAV();
                // The href may be relative — resolve against the origin, then
                // carry the run token in the hash so the next load resumes.
                const nextUrl = new URL(rawHref, location.origin);
                nextUrl.hash = 'aos-autosync-' + aosHeadlessRunState.token;
                window.location.href = nextUrl.href;
                return sentCount;
            }
            // Finish path: clear the cross-page state; the headless wrapper
            // sends the completion ping with the ACCUMULATED sentCount.
            sessionStorage.removeItem(AOS_HEADLESS_STATE_KEY);
        }
        logStatus("Sync operation finished on current page.");
        return sentCount;
    }
}
