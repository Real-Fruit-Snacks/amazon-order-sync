document.addEventListener('DOMContentLoaded', () => {
    const syncBtn = document.getElementById('syncBtn');
    const autoSyncBtn = document.getElementById('autoSyncBtn');
    const stopSyncBtn = document.getElementById('stopSyncBtn');
    const statusBadge = document.getElementById('statusBadge');
    const logOutput = document.getElementById('logOutput');
    const copyLogBtn = document.getElementById('copyLogBtn');

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

    // Connect to content script to get current state
    chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
        if (tabs[0] && tabs[0].url.includes("amazon.com/your-orders") || tabs[0].url.includes("amazon.com/gp/css/order-history")) {
            chrome.tabs.sendMessage(tabs[0].id, {action: "check_auto_sync_state"}, function(response) {
                if (chrome.runtime.lastError) {
                    appendLog("Error: Could not connect to Amazon page. Please refresh the page.");
                    return;
                }
                if (response) {
                    // Populate logs
                    if (response.logs && response.logs.length > 0) {
                        response.logs.forEach(log => appendLog(log));
                    } else {
                        appendLog("Ready to sync.");
                    }
                    
                    // Restore UI state
                    if (response.isAutoSyncing) {
                        setUIState(true, "Auto-Syncing");
                    } else if (response.isSyncing) {
                        setUIState(true, "Syncing Page");
                    }
                }
            });
        } else {
            appendLog("Please navigate to Amazon Orders page to use this extension.");
            syncBtn.disabled = true;
            autoSyncBtn.disabled = true;
        }
    });

    // Listen for live updates from content script
    chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
        if (request.action === "status_update") {
            appendLog(request.log);
            
            if (request.log.includes("Sync stopped by user") || request.log.includes("complete") || request.log.includes("finished")) {
                setUIState(false);
            } else if (!statusBadge.classList.contains('syncing')) {
                setUIState(true, request.isAutoSyncing ? "Auto-Syncing" : "Syncing Page");
            }
        } else if (request.action === "sync_complete") {
            setUIState(false);
        }
    });

    syncBtn.addEventListener('click', () => {
        chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
            setUIState(true, "Syncing Page");
            chrome.tabs.sendMessage(tabs[0].id, {action: "trigger_sync"});
        });
    });

    autoSyncBtn.addEventListener('click', () => {
        chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
            setUIState(true, "Auto-Syncing");
            chrome.tabs.sendMessage(tabs[0].id, {action: "start_auto_sync"});
        });
    });

    stopSyncBtn.addEventListener('click', () => {
        chrome.tabs.query({active: true, currentWindow: true}, function(tabs) {
            chrome.tabs.sendMessage(tabs[0].id, {action: "stop_auto_sync"});
        });
    });

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
