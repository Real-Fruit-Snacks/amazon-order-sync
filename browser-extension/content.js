let isSyncStopped = false;
let isSyncing = false;
let syncLogs = [];

function logStatus(message) {
    console.log(message);
    const time = new Date().toLocaleTimeString();
    const logEntry = `[${time}] ${message}`;
    syncLogs.push(logEntry);
    
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

// Auto-Fill Review Logic
if (window.location.href.includes('/review/create-review') || window.location.href.includes('/review/review-your-purchases')) {
    logStatus("On Amazon Review Page. Attempting Auto-Fill from Obsidian...");
    const params = new URLSearchParams(window.location.search);
    const asin = params.get('asin');
    if (asin) {
        fetch(`http://127.0.0.1:27483/api/get-review?asin=${asin}`)
            .then(res => res.json())
            .then(data => {
                if (data.title && data.text) {
                    logStatus(`Found AI Review in Obsidian for ASIN ${asin}! Injecting...`);
                    
                    const tryInject = setInterval(() => {
                        const titleInput = document.querySelector('input[name="reviewTitle"], input[id*="review-title"], .ryp-review-title-input');
                        const textInput = document.querySelector('textarea[name="reviewText"], textarea[id*="review-text"], .ryp-review-text-input');
                        const ratingBtn = document.querySelector('button[data-hook="ryp-star-rating-5"], img[alt="5 stars"], .ryp-star-rating-button[data-hook*="rating-5"]');
                        
                        if (titleInput && textInput) {
                            clearInterval(tryInject);
                            
                            titleInput.value = data.title;
                            titleInput.dispatchEvent(new Event('input', { bubbles: true }));
                            titleInput.dispatchEvent(new Event('change', { bubbles: true }));
                            
                            textInput.value = data.text;
                            textInput.dispatchEvent(new Event('input', { bubbles: true }));
                            textInput.dispatchEvent(new Event('change', { bubbles: true }));
                            
                            if (ratingBtn) ratingBtn.click();
                            
                            logStatus("Review successfully auto-filled!");
                        }
                    }, 500);
                    
                    setTimeout(() => clearInterval(tryInject), 10000);
                } else {
                    logStatus("No AI review found in Obsidian for this ASIN.");
                }
            }).catch(e => logStatus("Failed to contact local Obsidian server for Auto-Fill: " + e.message));
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
        extractAndSyncOrders(true).then(() => sendResponse({success: true}));
        return true;
    } else if (request.action === "stop_auto_sync") {
        isSyncStopped = true;
        isSyncing = false;
        logStatus("Sync stopped by user.");
        sessionStorage.removeItem('amazon_sync_auto');
        sendResponse({success: true});
        return true;
    } else if (request.action === "check_auto_sync_state") {
        sendResponse({
            isAutoSyncing: sessionStorage.getItem('amazon_sync_auto') === 'true',
            isSyncing: isSyncing,
            logs: syncLogs
        });
        return true;
    }
});

const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));

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
                                   
        return { brand, current_price: price, features: featureBullets };
    } catch (e) {
        logStatus("Error fetching product page: " + e.message);
        return null;
    }
}

async function fetchReviewDetails(asin) {
    try {
        logStatus(`Fetching review details for ASIN: ${asin}...`);
        const response = await fetch(`https://www.amazon.com/review/create-review?asin=${asin}`);
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
            const txtMatch = html.match(/"initialBody"\s*:\s*"((?:\\.|[^"\\])*)"/);
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

        return { rating: rating || "Unknown", title: title || "Unknown", text: text || "Unknown" };
    } catch (e) {
        logStatus(`Error fetching review for ${asin}: ${e.message}`);
        return null;
    }
}

async function checkOrderComplete(orderId) {
    logStatus(`[Obsidian] Checking status for Order ${orderId}...`);
    try {
        const response = await fetch(`http://127.0.0.1:27483/api/check?order_id=${orderId}`);
        if (!response.ok) {
            logStatus(`[Obsidian] Failed to check status for Order ${orderId} (HTTP status ${response.status}).`);
            return { complete: false, reason: `HTTP error ${response.status}` };
        }
        const data = await response.json();
        if (data.complete) {
            logStatus(`[Obsidian] Order ${orderId} status: Complete.`);
        } else {
            logStatus(`[Obsidian] Order ${orderId} status: Incomplete. Reason: ${data.reason || 'Needs update'}`);
        }
        return data;
    } catch (e) {
        logStatus(`[Obsidian] Could not connect to local server for Order ${orderId}: ${e.message}`);
        return { complete: false, reason: `Connection error: ${e.message}` };
    }
}

async function extractAndSyncOrders(isAuto) {
    isSyncing = true;
    logStatus("Scanning for order cards...");
    
    const orderCards = document.querySelectorAll('.order-card, .yohtmlc-order');
    logStatus(`Found ${orderCards.length} orders on this page.`);
    
    let processedCount = 0;

    for (const card of orderCards) {
        if (isSyncStopped) {
            logStatus("Aborting sync loop.");
            isSyncing = false;
            return;
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
            
            let dateMatch = cardText.match(/ORDER PLACED\s+([A-Za-z]+\s+\d{1,2},\s+\d{4})/i);
            let date = dateMatch ? dateMatch[1].trim() : new Date().toISOString().split('T')[0];

            // SMART PRE-CHECK
            const checkResult = await checkOrderComplete(orderId);
            if (checkResult && checkResult.complete === true) {
                logStatus(`[Action] Skipping Order ${orderId} (already up-to-date in Obsidian).`);
                continue;
            }

            logStatus(`[Action] Syncing Order ${orderId} (extracting details)...`);
            
            let totalMatch = cardText.match(/(?:TOTAL|ORDER TOTAL)\s*(\$[0-9,.]+)/i);
            let total = totalMatch ? totalMatch[1].trim() : "$0.00";
            
            let shipToMatch = cardText.match(/SHIP TO\s+(.+?)\s+(?:ORDER #|TOTAL|View order details|Order Details)/i);
            let shipTo = shipToMatch ? shipToMatch[1].trim() : "Unknown";

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
                if (isSyncStopped) return;
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
                        let hasDeepDetails = false;
                        
                        const existing = checkResult?.existingItems?.[asin];
                        if (existing && existing.brand && existing.brand !== "Unknown") {
                            brand = existing.brand;
                            current_price = existing.current_price;
                            features = existing.features;
                            hasDeepDetails = true;
                            logStatus(`[Skip] Product page scrape skipped for ASIN ${asin} (already in Obsidian).`);
                        }

                        if (!hasDeepDetails) {
                            // Fetch Deep Details
                            const cleanUrl = link.href.split('?')[0];
                            logStatus(`[Scrape] Fetching product page details for ASIN ${asin}...`);
                            const deepDetails = await fetchProductDetails(cleanUrl);
                            if (deepDetails) {
                                brand = deepDetails.brand || "Unknown";
                                current_price = deepDetails.current_price || "Unknown";
                                features = deepDetails.features || "";
                            }
                            logStatus(`Waiting 2s (bot protection)...`);
                            await sleep(2000); 
                        }
                        
                        let reviewDetails = null;
                        // Fetch review details only if marked reviewed on card AND it's not marked posted in Obsidian yet
                        if (is_reviewed && (!existing || !existing.alreadyPosted)) {
                            logStatus(`[Scrape] Fetching existing Amazon review status/details for ASIN ${asin}...`);
                            reviewDetails = await fetchReviewDetails(asin);
                            
                            // Check if it was a false positive (no actual review found)
                            if (reviewDetails && reviewDetails.title === "Unknown" && reviewDetails.text === "Unknown") {
                                is_reviewed = false;
                                reviewDetails = null;
                            } else {
                                logStatus(`Waiting 2s (bot protection)...`);
                                await sleep(2000);
                            }
                        } else if (existing && existing.alreadyPosted) {
                            logStatus(`[Skip] Review status check skipped for ASIN ${asin} (already marked as Posted).`);
                            is_reviewed = true;
                        }
                        
                        items.push({
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
                        });
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

            logStatus(`[Obsidian] Sending payload for Order ${orderId} to local server...`);
            const syncResponse = await fetch('http://127.0.0.1:27483/api/sync', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify(payload)
            });
            if (syncResponse.ok) {
                logStatus(`[Obsidian] Successfully synced Order ${orderId}!`);
            } else {
                logStatus(`[Obsidian] Failed to sync Order ${orderId} (HTTP status ${syncResponse.status}).`);
            }
            
        } catch (e) {
            logStatus(`Error extracting order: ${e.message}`);
        }
    }
    
    if (isAuto && !isSyncStopped) {
        logStatus("Auto-sync: Checking for next page...");
        const nextBtn = document.querySelector('ul.a-pagination li.a-last a');
        if (nextBtn) {
            logStatus("Next page found! Navigating in 3 seconds...");
            await sleep(3000);
            nextBtn.click();
        } else {
            logStatus("No next page found. Checking for previous year...");
            const timeFilter = document.querySelector('select[name="timeFilter"], #time-filter');
            if (timeFilter) {
                const nextOption = timeFilter.options[timeFilter.selectedIndex + 1];
                if (nextOption && nextOption.value.startsWith('year-')) {
                    logStatus(`Navigating to previous year: ${nextOption.text} in 3 seconds...`);
                    await sleep(3000);
                    timeFilter.value = nextOption.value;
                    timeFilter.dispatchEvent(new Event('change'));
                    
                    // Fallback redirect if Amazon's JS fails
                    setTimeout(() => {
                        window.location.href = `https://www.amazon.com/your-orders/orders?timeFilter=${nextOption.value}`;
                    }, 1000);
                    return;
                }
            }
            logStatus("No more history found. Auto-sync complete!");
            sessionStorage.removeItem('amazon_sync_auto');
            isSyncing = false;
            chrome.runtime.sendMessage({ action: "sync_complete" }).catch(() => {});
        }
    } else {
        logStatus("Sync operation finished on current page.");
        isSyncing = false;
    }
}
