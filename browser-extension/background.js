// Amazon Order Sync service worker (v2.2.0).
//
// The only code in the extension that talks to the local server. Content
// scripts ask it with chrome.runtime.sendMessage({aos: 'fetch', ...}); its
// requests carry this extension's own origin (chrome-extension://<id>, fixed
// by the "key" in manifest.json), which is the only extension origin the
// server accepts. Only /api/ paths on 127.0.0.1:27483 can be reached.
const SERVER = 'http://127.0.0.1:27483';

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (!msg || msg.aos !== 'fetch') return false;
    // Only this extension's own content scripts and pages.
    if (sender.id !== chrome.runtime.id) return false;
    const path = String(msg.path || '');
    if (!/^\/api\/[A-Za-z0-9\-/]+(\?.*)?$/.test(path)) {
        sendResponse({ error: 'not an API path: ' + path });
        return false;
    }
    const method = msg.method === 'POST' ? 'POST' : 'GET';
    const init = { method };
    if (method === 'POST') {
        init.headers = { 'Content-Type': 'application/json' };
        init.body = typeof msg.body === 'string' ? msg.body : '{}';
    }
    fetch(SERVER + path, init)
        .then(async res => sendResponse({ status: res.status, text: await res.text() }))
        .catch(e => sendResponse({ error: e.message }));
    return true; // async sendResponse
});
