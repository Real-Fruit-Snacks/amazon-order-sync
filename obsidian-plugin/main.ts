import { Plugin, Notice, normalizePath, requestUrl, App } from 'obsidian';
import * as http from 'http';

interface SyncPayload {
    order_id: string;
    date: string;
    total: string;
    status: string;
    ship_to: string;
    items: Array<{
        asin: string;
        name: string;
        link: string;
        image: string;
        is_reviewed: boolean;
        brand: string;
        current_price: string;
        features: string;
        review_rating: string;
        review_title: string;
    }>;
}

export default class AmazonOrderSyncPlugin extends Plugin {
    server: http.Server | null = null;
    readonly PORT = 27483;

    async onload() {
        console.log('Loading Amazon Order Sync plugin');
        await this.ensureFolderExists('Orders');
        this.startServer();
    }

    onunload() {
        console.log('Unloading Amazon Order Sync plugin');
        this.stopServer();
    }

    startServer() {
        this.server = http.createServer(async (req, res) => {
            res.setHeader('Access-Control-Allow-Origin', '*');
            res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
            res.setHeader('Access-Control-Allow-Headers', 'Content-Type');

            if (req.method === 'OPTIONS') {
                res.writeHead(204);
                res.end();
                return;
            }

            if (req.method === 'GET' && req.url?.startsWith('/api/check')) {
                try {
                    const url = new URL(req.url, `http://${req.headers.host}`);
                    const orderId = url.searchParams.get('order_id');
                    
                    if (!orderId) {
                        res.writeHead(400, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ error: 'Missing order_id' }));
                        return;
                    }

                    const files = this.app.vault.getFiles();
                    const file = files.find(f => f.name === `Order-${orderId}.md`);
                    
                    if (!file) {
                        res.writeHead(200, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ complete: false, reason: 'File does not exist' }));
                        return;
                    }

                    const content = await this.app.vault.read(file as any);
                    
                    const needsReview = content.includes('> *Write your review for this item below...*');
                    
                    const statusMatch = content.match(/status: "(.*?)"/);
                    let isUndelivered = false;
                    if (statusMatch) {
                        const status = statusMatch[1].toLowerCase();
                        isUndelivered = !status.includes('delivered') && !status.includes('return') && !status.includes('cancel');
                    }

                    const notPosted = content.includes('- [ ] Posted to Amazon?');
                    
                    if (needsReview || isUndelivered || notPosted) {
                        res.writeHead(200, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ complete: false, reason: 'Needs update or undelivered' }));
                        return;
                    }

                    res.writeHead(200, { 'Content-Type': 'application/json' });
                    res.end(JSON.stringify({ complete: true }));
                } catch (e) {
                    console.error('Error checking order:', e);
                    res.writeHead(500, { 'Content-Type': 'application/json' });
                    res.end(JSON.stringify({ error: e.message }));
                }
            } else if (req.method === 'GET' && req.url?.startsWith('/api/get-review')) {
                try {
                    const url = new URL(req.url, `http://${req.headers.host}`);
                    const asin = url.searchParams.get('asin');
                    
                    if (!asin) {
                        res.writeHead(400, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ error: 'Missing asin' }));
                        return;
                    }

                    const files = this.app.vault.getFiles().filter(f => f.path.startsWith("Orders/"));
                    let foundTitle = "";
                    let foundText = "";

                    for (const file of files) {
                        const content = await this.app.vault.read(file as any);
                        const fmMatch = content.match(/---[\s\S]*?---/);
                        if (!fmMatch) continue;
                        
                        const fm = fmMatch[0];
                        if (fm.includes(`"${asin}"`)) {
                            const asinMatch = fm.match(/asin:\s*\[(.*?)\]/);
                            if (asinMatch) {
                                const asinsList = asinMatch[1].split(',').map(s => s.trim().replace(/"/g, ''));
                                const index = asinsList.indexOf(asin);
                                
                                if (index !== -1) {
                                    const reviewBlocks = content.split('#### 📝 My Review');
                                    if (reviewBlocks.length > index + 1) {
                                        const block = reviewBlocks[index + 1];
                                        const titleMatch = block.match(/\*\*Title:\*\*\s*(.+)/);
                                        const reviewMatch = block.match(/\*\*Review:\*\*\s*([\s\S]*?)(?=\n- \[|\n---|$)/);
                                        
                                        if (titleMatch && reviewMatch) {
                                            const t = titleMatch[1].trim();
                                            const r = reviewMatch[1].trim();
                                            if (t && r && !r.includes("Write your review for this item below")) {
                                                foundTitle = t;
                                                foundText = r;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if (foundTitle && foundText) {
                        res.writeHead(200, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ title: foundTitle, text: foundText }));
                    } else {
                        res.writeHead(404, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ error: 'Review not found or not generated yet' }));
                    }
                } catch (e) {
                    console.error('Error fetching review:', e);
                    res.writeHead(500, { 'Content-Type': 'application/json' });
                    res.end(JSON.stringify({ error: e.message }));
                }
            } else if (req.method === 'POST' && req.url === '/api/debug') {
                let body = '';
                req.on('data', chunk => { body += chunk.toString(); });
                req.on('end', async () => {
                    try {
                        const filePath = normalizePath('debug-review.html');
                        const file = this.app.vault.getAbstractFileByPath(filePath);
                        if (file) {
                            await this.app.vault.modify(file as any, body);
                        } else {
                            await this.app.vault.create(filePath, body);
                        }
                        res.writeHead(200);
                        res.end();
                    } catch (e) {
                        res.writeHead(500);
                        res.end();
                    }
                });
            } else if (req.method === 'POST' && req.url === '/api/sync') {
                let body = '';
                req.on('data', chunk => {
                    body += chunk.toString();
                });
                req.on('end', async () => {
                    try {
                        const payload: SyncPayload = JSON.parse(body);
                        await this.processPayload(payload);
                        
                        res.writeHead(200, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ success: true }));
                    } catch (e) {
                        console.error('Error processing payload:', e);
                        res.writeHead(500, { 'Content-Type': 'application/json' });
                        res.end(JSON.stringify({ error: e.message }));
                    }
                });
            } else {
                res.writeHead(404);
                res.end();
            }
        });

        this.server.listen(this.PORT, '127.0.0.1', () => {
            new Notice(`Amazon Sync Server running on port ${this.PORT}`);
            console.log(`Amazon Sync Server running on port ${this.PORT}`);
        });
        
        this.server.on('error', (e) => {
            console.error('Server error:', e);
            new Notice('Failed to start Amazon Sync Server. Port might be in use.');
        });
    }

    stopServer() {
        if (this.server) {
            this.server.close();
            this.server = null;
            console.log('Amazon Sync Server stopped');
        }
    }

    async ensureFolderExists(folderName: string) {
        const parts = folderName.split('/');
        let currentPath = '';
        for (const part of parts) {
            if (part === '') continue;
            currentPath = currentPath === '' ? part : `${currentPath}/${part}`;
            const folderPath = normalizePath(currentPath);
            const folder = this.app.vault.getAbstractFileByPath(folderPath);
            if (!folder) {
                try {
                    await this.app.vault.createFolder(folderPath);
                } catch (e) {
                    if (e.message && !e.message.includes('already exists')) {
                        console.error('Failed to create folder:', e);
                    }
                }
            }
        }
    }

    async downloadImage(url: string, asin: string): Promise<string> {
        if (!url || !url.startsWith('http')) return url;
        try {
            const assetsPath = 'Orders/assets';
            await this.ensureFolderExists(assetsPath);
            
            // Extract file extension or default to jpg
            const match = url.match(/\.([a-zA-Z0-9]+)(?:[?#]|$)/);
            const ext = match ? match[1] : 'jpg';
            const fileName = `${asin}.${ext}`;
            const filePath = normalizePath(`${assetsPath}/${fileName}`);
            
            const file = this.app.vault.getAbstractFileByPath(filePath);
            if (file) {
                return `assets/${fileName}`; // Already downloaded
            }
            
            const response = await requestUrl({ url: url });
            await this.app.vault.createBinary(filePath, response.arrayBuffer);
            
            return `assets/${fileName}`;
        } catch (e) {
            console.error('Failed to download image', e);
            return url; // Fallback to original URL
        }
    }

    async generateReview(item: any): Promise<{title: string, text: string}> {
        const apiKey = this.app.secretStorage.getSecret("gemini");
        if (!apiKey) {
            return { title: "", text: "" };
        }
        
        try {
            new Notice(`Generating AI review for ${item.name.substring(0, 30)}...`);
            
            const prompt = `You are a specialized writing assistant that produces Amazon Vine-style product reviews. Your goals are: (1) comply with Amazon Vine Guidelines, (2) sound like a real customer, not AI (Never use any "—"  when writing), (3) be specific and helpful, (4) never mislead, (5) suggest a title, and (6) never put URLs or links in the output.

# Hard rules (Vine-compliant)
Be unbiased & honest. Reflect pros and cons even on 5-star requests; include at least one "could be better" note on 4–5★ when reasonable.
Be product-focused. Do not discuss seller, shipping, delivery speed, pricing, coupons, packaging condition, or promotions.
Be specific. Mention concrete features, specs, or use-cases drawn from the listing (capacity, size, materials, key features).
No unverified claims. Do not claim personal longevity testing, lab measurements, medical/health outcomes, safety compliance, or certifications unless shown on the page.
No copying. Paraphrase listing text in your own words; don't quote marketing copy verbatim.
Photos/videos. You may encourage the user to add real-life media, but do not fabricate or imply images exist.
Grammar & clarity. Keep clean, readable prose. Avoid repetitive phrasing and clichéd AI telltales ("overall, overall"/"in conclusion").

# Voice & style (human, not AI)
First-person singular ("I") with natural cadence and varied sentence lengths.
Use sensory/experiential details when supported by the listing or user context (e.g., "matte finish," "noticeably quieter fan").
Prefer concrete nouns and numbers over adjectives.
Avoid boilerplate and over-formal tone. Use contractions. No grandiose language.

# Keep length adaptive:
Default: ~140–220 words.

# Rating calibration
5★: Clearly strong product; include 1 minor nit or usage tip.
If the user's requested rating conflicts with obvious listing limitations, still honor the rating but keep wording honest (e.g., "For my needs..." / "In my testing..."). Never fabricate experience; use neutral framing when evidence is thin.

# Category notes (use when applicable)
Beauty/Personal Care: Mention scent strength, texture, residue, size (oz/ml), suitability (e.g., fine/curly hair) only if indicated. Avoid medical claims.
Electronics/Appliances: Note setup, compatibility, ports, battery life, noise level, wattage—only if listed. Avoid benchmarking unless provided.
Home/Kitchen: Capacity, materials, care/cleaning, footprint (dimensions), assembly notes from listing.
Toys/Kids/Pets: Age range, durability, materials, safety notes only if explicitly stated.

# Safe evidence handling
Extract product title, key specs, and features from the listing page. Paraphrase.
If a spec is unclear, omit it rather than guess.
Do not invent time-in-use.

# Structure template (adapt as needed; do not label sections if it feels forced)
Context (1–2 lines): How/why someone would use it, or fit for a scenario.
What stands out (3–5 lines): Specific features or results tied to the listing.
What could be better (1–3 lines): Concrete, non-dramatic drawbacks.
Bottom line (1–2 lines): Who it's for, with the requested star level spelled out.
Avoid bullet overload; a short, scannable paragraph style is fine.

# Phrases to avoid (AI tells)
"Overall overall," "as an AI," "game-changer," "I was skeptical at first," "works as expected" (without detail), "I highly recommend this product to everyone," excessive synonyms, repeating the product title verbatim multiple times.

# Inputs
Requested star rating: 5 stars
Product Name: ${item.name}
Brand: ${item.brand}
Features: ${item.features}

# Output format
Deliver a single review in plain text paragraphs. DO NOT include a star rating line or any meta commentary at the end.
CRITICAL: You MUST return a valid JSON object in this exact format:
{"title": "your generated title", "text": "your complete generated review text"}
`;

            // Prevent rate limits by waiting 2 seconds between generations
            await new Promise(resolve => setTimeout(resolve, 2000));

            let response;
            let retryCount = 0;
            
            while (retryCount < 3) {
                response = await fetch(`https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite:generateContent?key=${apiKey.trim()}`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        contents: [{ parts: [{ text: prompt }] }],
                        generationConfig: { responseMimeType: "application/json" }
                    })
                });

                if (response.status === 429) {
                    console.log("Rate limited! Waiting 5 seconds before retrying...");
                    await new Promise(resolve => setTimeout(resolve, 5000));
                    retryCount++;
                } else if (!response.ok) {
                    throw new Error(`Request failed with status ${response.status}`);
                } else {
                    break;
                }
            }

            if (!response || !response.ok) {
                throw new Error("Failed to generate review after retries");
            }

            const data = await response.json();

            if (data.candidates && data.candidates.length > 0) {
                try {
                    const jsonText = data.candidates[0].content.parts[0].text;
                    const parsed = JSON.parse(jsonText);
                    return { 
                        title: parsed.title || "Great product", 
                        text: parsed.text || "" 
                    };
                } catch (e) {
                    console.error("Failed to parse JSON", e);
                }
            }
        } catch (e) {
            console.error('Failed to generate review', e);
            new Notice(`Failed to generate review: ${e.message}`);
        }
        
        return { title: "", text: "" };
    }

    async processPayload(payload: SyncPayload) {
        new Notice(`Syncing order ${payload.order_id}...`);
        await this.createOrUpdateOrder(payload);
        new Notice(`Order ${payload.order_id} synced!`);
    }

    async createOrUpdateOrder(payload: SyncPayload) {
        let folderPath = 'Orders';
        if (payload.date) {
            const d = new Date(payload.date.replace("Ordered on ", ""));
            if (!isNaN(d.getTime())) {
                const year = d.getFullYear();
                const month = (d.getMonth() + 1).toString().padStart(2, '0');
                const monthName = d.toLocaleString('en-US', { month: 'long' });
                folderPath = `Orders/${year}/${month}-${monthName}`;
            }
        }
        await this.ensureFolderExists(folderPath);
        
        const filePath = normalizePath(`${folderPath}/Order-${payload.order_id}.md`);
        
        let existingContent = "";
        const existingFile = this.app.vault.getFiles().find(f => f.name === `Order-${payload.order_id}.md`);
        if (existingFile) {
            existingContent = await this.app.vault.read(existingFile as any);
        }
        
        let brands = [];
        let prices = [];
        let asins = [];
        let urls = [];
        let revieweds = [];
        let ratings = [];
        let itemsContent = "";
        
        for (const item of payload.items) {
            const cleanBrand = item.brand.replace(/"/g, '\\"');
            
            brands.push(`"${cleanBrand}"`);
            prices.push(`"${item.current_price}"`);
            asins.push(`"${item.asin}"`);
            urls.push(`"${item.link}"`);
            
            const localImagePath = await this.downloadImage(item.image, item.asin);
            
            let aiReviewContent = "";
            let alreadyPosted = false;
            
            if (existingContent) {
                // Split the content by horizontal rules separating the items
                const blocks = existingContent.split(/\r?\n---\r?\n/);
                
                // Find the item block that contains this item's ASIN and has a review section
                const itemBlock = blocks.find(block => block.includes(item.asin) && block.includes('#### 📝 My Review'));
                
                if (itemBlock) {
                    const reviewHeader = `#### 📝 My Review`;
                    const reviewHeaderIndex = itemBlock.indexOf(reviewHeader);
                    
                    if (reviewHeaderIndex !== -1) {
                        let searchStartIndex = reviewHeaderIndex + reviewHeader.length;
                        
                        const asinLink = `[✍️ Leave a Review on Amazon]`;
                        const linkIndex = itemBlock.indexOf(asinLink, reviewHeaderIndex);
                        if (linkIndex !== -1) {
                            // Skip past the line containing the review link
                            const endOfLinkLine = itemBlock.indexOf('\n', linkIndex);
                            if (endOfLinkLine !== -1) {
                                searchStartIndex = endOfLinkLine + 1;
                            } else {
                                searchStartIndex = linkIndex + asinLink.length;
                            }
                        }
                        
                        // Locate the end of the review content before the check box
                        const postedIndex = itemBlock.indexOf('Posted to Amazon?', searchStartIndex);
                        let endOfReview = -1;
                        if (postedIndex !== -1) {
                            const checkboxStart = itemBlock.lastIndexOf('- [', postedIndex);
                            if (checkboxStart !== -1 && checkboxStart >= searchStartIndex) {
                                endOfReview = checkboxStart;
                            } else {
                                endOfReview = postedIndex;
                            }
                        }
                        
                        if (endOfReview !== -1) {
                            const existingReviewBlock = itemBlock.substring(searchStartIndex, endOfReview).trim();
                            if (existingReviewBlock && !existingReviewBlock.includes('> *Write your review for this item below...*')) {
                                aiReviewContent = existingReviewBlock;
                            }
                        }
                        
                        // Check if the review has been marked as posted in Obsidian
                        const checkboxMatch = itemBlock.match(/- \[\s*([xX])\s*\]\s*Posted to Amazon\?/);
                        if (checkboxMatch) {
                            alreadyPosted = true;
                        }
                    }
                }
            }
            
            const isPosted = item.is_reviewed || alreadyPosted;
            revieweds.push(isPosted);
            ratings.push(`"${item.review_rating}"`);

            const apiKey = this.app.secretStorage.getSecret("gemini");
            if (!isPosted && !aiReviewContent && apiKey) {
                const draft = await this.generateReview(item);
                if (draft.text) {
                    aiReviewContent = `**Rating:** 5\n**Title:** ${draft.title}\n**Review:** ${draft.text}`;
                }
            }
            
            let finalReviewText = "";
            if (item.is_reviewed && item.review_title && item.review_text) {
                 finalReviewText = `**Title:** ${item.review_title}\n**Review:** ${item.review_text}`;
            } else if (aiReviewContent) {
                 finalReviewText = aiReviewContent;
            } else {
                 finalReviewText = `> *Write your review for this item below...*\n**Rating:** \n**Review:** `;
            }
            
            itemsContent += `
### ${item.name}

![image](${localImagePath})

#### Features
${item.features}

#### 📝 My Review
[✍️ Leave a Review on Amazon](https://www.amazon.com/review/create-review?asin=${item.asin})

${finalReviewText}
- [${isPosted ? "x" : " "}] Posted to Amazon?

---
`;
        }
        
        const orderContent = `---
type: order
order_id: "${payload.order_id}"
date: "${payload.date}"
total: "${payload.total}"
status: "${payload.status}"
ship_to: "${payload.ship_to}"
brand: [${brands.join(", ")}]
price: [${prices.join(", ")}]
asin: [${asins.join(", ")}]
url: [${urls.join(", ")}]
reviewed: [${revieweds.join(", ")}]
rating: [${ratings.join(", ")}]
---
# Order ${payload.order_id}

**Order Date:** ${payload.date}
**Total:** ${payload.total}
**Status:** ${payload.status}
**Ship To:** ${payload.ship_to}

---

## 📦 Items
${itemsContent}
`;


        if (existingFile) {
            if (existingFile.path !== filePath) {
                try {
                    await this.app.vault.rename(existingFile, filePath);
                } catch (e) {
                    console.error("Failed to move file to new folder:", e);
                }
            }
            await this.app.vault.modify(existingFile as any, orderContent);
        } else {
            await this.app.vault.create(filePath, orderContent);
        }
    }
}
