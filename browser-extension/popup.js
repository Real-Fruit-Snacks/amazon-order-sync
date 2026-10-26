document.addEventListener('DOMContentLoaded', () => {
    const syncBtn = document.getElementById('syncBtn');
    const autoSyncBtn = document.getElementById('autoSyncBtn');
    const stopSyncBtn = document.getElementById('stopSyncBtn');
    const statusBadge = document.getElementById('statusBadge');
    const logOutput = document.getElementById('logOutput');
    const copyLogBtn = document.getElementById('copyLogBtn');

    // v1.8.3 (§23.6): version label comes from the manifest — never hardcode.
    const verSpan = document.getElementById('ver');
    if (verSpan) {
        try {
            verSpan.textContent = 'v' + chrome.runtime.getManifest().version;
        } catch (e) {
            verSpan.textContent = 'v1.8.3'; // pre-JS paint fallback text
        }
    }

    function appendLog(msg) {
        const div = document.createElement('div');
        div.className = 'log-item';
        div.textContent = msg;
        logOutput.appendChild(div);
        logOutput.scrollTop = logOutput.scrollHeight;
    }

    function setUIState(isSyncing, statusText = "Syncing") {
        if (isSyncing) {
            syncBtn.classList.add('hidden');
            autoSyncBtn.classList.add('hidden');
            stopSyncBtn.classList.remove('hidden');
            statusBadge.textContent = statusText;
            statusBadge.className = 'status-badge syncing';
        } else {
            syncBtn.classList.remove('hidden');
            autoSyncBtn.classList.remove('hidden');
            stopSyncBtn.classList.add('hidden');
            statusBadge.textContent = 'Idle';
            statusBadge.className = 'status-badge';
        }
    }

    // v1.7.2 (§20.5): the popup's own click state. A run is considered active
    // only because the user clicked Start here, or because the content
    // script's explicit state fields said so — NEVER because a log line
    // happened to contain a word like "complete"/"finished".
    let popupRunActive = false;
    function beginRun(statusText) {
        popupRunActive = true;
        setUIState(true, statusText);
    }
    function endRun() {
        popupRunActive = false;
        setUIState(false);
    }

    // v1.7.2 (§20.5): end-of-run is signaled ONLY by these precise full log
    // lines (compared after stripping the "[time] " prefix) or the explicit
    // sync_complete message. Substring matching on "complete" is gone.
    function isEndOfRunLine(log) {
        const text = String(log == null ? '' : log)
            .replace(/^\[\d{1,2}:\d{2}:\d{2}(?:\s?[AP]M)?\]\s*/i, '');
        return text === 'Sync stopped by user.'
            || text === 'Sync operation finished on current page.'
            || text === 'No more history found. Auto-sync complete!'
            || text === 'Sync complete'
            || text.startsWith('[Headless] Pass complete;');
    }

    // v1.7.2 (§20.5): gate on a parsed URL, not a substring regex. Hostname
    // must be (or end with) one of the 9 manifest Amazon domains; path must
    // be an orders page.
    const AMAZON_DOMAINS = [
        'amazon.com', 'amazon.co.uk', 'amazon.de', 'amazon.ca', 'amazon.fr',
        'amazon.it', 'amazon.es', 'amazon.co.jp', 'amazon.com.au'
    ];
    function isAmazonOrdersUrl(tabUrl) {
        let u;
        try {
            u = new URL(tabUrl);
        } catch (e) {
            return false;
        }
        const host = u.hostname.toLowerCase();
        const hostOk = AMAZON_DOMAINS.some(d => host === d || host.endsWith('.' + d));
        if (!hostOk) return false;
        // v1.8.6: 'order-history' covers both the classic /gp/css/order-history
        // and the newer /gp/your-account/order-history layouts.
        return u.pathname.includes('your-orders') ||
               u.pathname.includes('order-history');
    }

    // v1.7.2 (§20.5): every sendMessage checks chrome.runtime.lastError and
    // surfaces a user-visible "refresh the page" line. Returns true when the
    // message was delivered (response callback may still fire with undefined
    // if the receiver doesn't answer — that's fine).
    function sendToTab(tabId, message, onResponse) {
        chrome.tabs.sendMessage(tabId, message, function(response) {
            if (chrome.runtime.lastError) {
                appendLog('Could not reach the Amazon page — refresh it and try again.');
                if (onResponse) onResponse(null, chrome.runtime.lastError);
                return;
            }
            if (onResponse) onResponse(response, null);
        });
    }

    // Connect to content script to get current state
    chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
        const tabUrl = tabs[0] && tabs[0].url ? tabs[0].url : "";
        if (isAmazonOrdersUrl(tabUrl)) {
            sendToTab(tabs[0].id, {action: "check_auto_sync_state"}, function(response, err) {
                if (err) return; // sendToTab already logged the refresh hint
                if (response) {
                    // Populate logs
                    if (response.logs && response.logs.length > 0) {
                        response.logs.forEach(log => appendLog(log));
                    } else {
                        appendLog("Ready to sync.");
                    }

                    // v1.7.2 (§20.5): restore UI state from the explicit
                    // response fields only (syncRunning is the v1.7.2 signal;
                    // isSyncing/isAutoSyncing kept for older content scripts).
                    const running = response.syncRunning === true || response.isSyncing === true;
                    if (response.isAutoSyncing) {
                        beginRun("Auto-Syncing");
                    } else if (running) {
                        beginRun("Syncing Page");
                    } else {
                        endRun();
                    }
                }
            });
        } else {
            appendLog("Please navigate to Amazon Orders page to use this extension.");
            syncBtn.disabled = true;
            autoSyncBtn.disabled = true;
        }
    });

    // Listen for live updates from content script. Logs are DISPLAY ONLY —
    // the single exception is the precise end-of-run sentinel lines, matched
    // in full by isEndOfRunLine (never by substring).
    chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
        if (request.action === "status_update") {
            appendLog(request.log);
            if (isEndOfRunLine(request.log)) endRun();
        } else if (request.action === "sync_complete") {
            endRun();
        }
    });

    syncBtn.addEventListener('click', () => {
        chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
            beginRun("Syncing Page"); // popup's own click state
            sendToTab(tabs[0].id, {action: "trigger_sync"}, function(response, err) {
                if (err) endRun(); // never reached the page — not running
                // v1.8.3 (§23.4, audit B3): an explicit {success:false} from
                // the content script also ends the run (was stuck "Syncing").
                else if (response && response.success === false) endRun();
            });
        });
    });

    autoSyncBtn.addEventListener('click', () => {
        chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
            beginRun("Auto-Syncing"); // popup's own click state
            sendToTab(tabs[0].id, {action: "start_auto_sync"}, function(response, err) {
                if (err) endRun();
                // v1.8.3 (§23.4, audit B3): explicit failure ends the run too.
                else if (response && response.success === false) endRun();
            });
        });
    });

    stopSyncBtn.addEventListener('click', () => {
        chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
            sendToTab(tabs[0].id, {action: "stop_auto_sync"}, function(response, err) {
                if (err) return; // state unchanged; hint already logged
                // The content script's "Sync stopped by user." sentinel line
                // ends the run; end here too in case that broadcast is missed.
                endRun();
            });
        });
    });

    // v1.4.0: best-effort "Last sync: …" line under the status badge,
    // fetched from the local server's /api/orders meta; hidden on failure.
    (function reportLastSync() {
        const line = document.getElementById('lastSync');
        if (!line) return;
        fetch('http://127.0.0.1:27483/api/orders').then(res => {
            if (!res.ok) throw new Error('HTTP ' + res.status);
            return res.json();
        }).then(data => {
            const last = typeof data.lastSyncAt === 'number' ? data.lastSyncAt : null;
            let text = 'Last sync: never';
            if (last != null) {
                const days = Math.floor((Date.now() - last) / 86400000);
                text = 'Last sync: ' + (days <= 0 ? 'today' : days + 'd ago');
            }
            line.textContent = text;
            line.hidden = false;
        }).catch(() => {
            line.hidden = true;
        });
    })();

    copyLogBtn.addEventListener('click', () => {
        const logItems = Array.from(logOutput.querySelectorAll('.log-item')).map(item => item.textContent);
        const logText = logItems.join('\n');
        navigator.clipboard.writeText(logText).then(() => {
            const originalText = copyLogBtn.textContent;
            copyLogBtn.textContent = 'Copied!';
            setTimeout(() => {
                copyLogBtn.textContent = originalText;
            }, 2000);
        }).catch(err => {
            appendLog('Failed to copy log: ' + err);
        });
    });
});
