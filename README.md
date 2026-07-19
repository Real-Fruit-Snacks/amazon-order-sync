# Amazon Order Sync for Obsidian

![Amazon Order Sync](https://img.shields.io/badge/Obsidian-Amazon%20Order%20Sync-7C3AED?style=for-the-badge&logo=obsidian)
![Version](https://img.shields.io/badge/Version-1.0.7-blue?style=for-the-badge)
![License](https://img.shields.io/badge/License-MIT-green?style=for-the-badge)

A powerful, fully-autonomous integration bridging your Amazon purchase history directly into your local Obsidian vault. Designed for heavy Amazon shoppers and Amazon Vine reviewers, this tool securely extracts your entire order history, downloads product images locally, generates AI-powered reviews, and creates beautiful dashboards—all without relying on third-party cloud services.

By [Real-Fruit-Snacks](https://github.com/Real-Fruit-Snacks)

---

## Features

*   **Autonomous Time Machine Sync:** Seamlessly scrape and extract your complete Amazon order history. The Chrome extension automatically handles pagination and navigates backwards through your historical years on autopilot.
*   **Dynamic Folder Routing:** Say goodbye to cluttered vaults. New orders are automatically sorted into clean, date-based directories (e.g., `Orders/2026/06-June/`).
*   **Local Image Caching:** Product thumbnails are downloaded directly to your vault so your notes never suffer from dead web links.
*   **AI Review Generation:** Need to write a review? The system uses Gemini (configured with strict Amazon Vine guidelines) to automatically generate high-quality, insightful, and authentic product reviews in the background.
*   **Review Auto-Filler:** Click the "Leave a Review on Amazon" link inside Obsidian, and the Chrome extension will instantly auto-fill the Amazon review form with your AI-generated text.
*   **Advanced Spending Analytics:** Includes a native, pure HTML/CSS dashboard powered by DataviewJS. Visualize your monthly spending directly inside Obsidian without requiring heavy charting plugins.

## Architecture

This project consists of two interlocking components:
1.  **Obsidian Plugin:** Runs a lightweight local server (`127.0.0.1:27483`) inside your vault to receive incoming data, save Markdown files, and provide the AI-review API endpoint. The server only answers requests that originate from an `amazon.com` page — the companion extension — and rejects every other origin, so no other website open in your browser can read from or write to your vault through it.
2.  **Chrome Extension:** Runs strictly on Amazon domains. It extracts order data, detects when you open a review page, and communicates natively with your local Obsidian server.

## Installation

### 1. Obsidian Plugin
1. Download the latest `obsidian-plugin.zip` from the **[Releases](https://github.com/Real-Fruit-Snacks/amazon-order-sync/releases)** page.
2. Unzip the contents into your `.obsidian/plugins/` directory.
3. Open Obsidian **Settings > Community Plugins** and enable **Amazon Order Sync**.
4. In the plugin settings, add your Gemini API key to enable AI review generation.

### 2. Chrome Extension
1. Download the latest `browser-extension.zip` from the **[Releases](https://github.com/Real-Fruit-Snacks/amazon-order-sync/releases)** page.
2. Extract the folder to a safe location on your computer.
3. Open Google Chrome and navigate to `chrome://extensions/`.
4. Enable **Developer mode** in the top right corner.
5. Click **Load unpacked** and select the extracted `browser-extension` folder.

## Usage

1. Open your Amazon **Your Orders** page.
2. Click the **Amazon Order Sync** icon in your browser toolbar.
3. Click **Auto-Sync All Pages** and watch as your entire purchase history populates your Obsidian vault in real-time.
4. Open the `Amazon Dashboard.md` note in Obsidian to view your Analytics Chart and your "Needs Review" inbox!

## Privacy & Security

Your data is yours. This integration extracts your order history directly from your local browser session and sends it to your local Obsidian vault via `localhost`. No purchase history, tracking cookies, or personal data is ever sent to an external server (other than the product descriptions sent to Gemini for AI review generation).

The local server is origin-locked: it only accepts requests from `amazon.com` pages (the companion extension) and refuses every other origin outright, so no other website you have open can reach your synced orders or reviews or write files into your vault. Your Gemini API key is held in Obsidian's secret storage, not in the plugin's plaintext data file.

## License

This project is licensed under the MIT License.
