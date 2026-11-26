# amazon-order-sync Usage & Sample Output

Detailed installation options, live-verified sample notes, architecture summary, configuration quick reference, tested platforms, and known issues for `amazon-order-sync`. For the full dashboard, headless sync, review drafts, and troubleshooting guide see [guide.md](guide.md); for the HTTP API see [api.md](api.md).

## Install & Browser Extension

Windows (PowerShell):

```powershell
$zip = "$env:TEMP\amazon-order-sync-windows.zip"
Invoke-WebRequest https://github.com/Real-Fruit-Snacks/amazon-order-sync/releases/latest/download/amazon-order-sync-windows.zip -OutFile $zip
Expand-Archive $zip "$env:LOCALAPPDATA\Programs\amazon-order-sync" -Force
Set-ExecutionPolicy -Scope Process Bypass; & "$env:LOCALAPPDATA\Programs\amazon-order-sync\install.ps1"
```

Linux (x86-64, systemd user service):

```sh
curl -fsSL https://github.com/Real-Fruit-Snacks/amazon-order-sync/releases/latest/download/amazon-order-sync-linux.tar.gz | tar xz && ./amazon-order-sync/install.sh
```

With no network: copy the zip or tarball over, expand it, and run `install.ps1` or `install.sh` from that folder; nothing is downloaded and nothing else needs to be installed. The Linux binary is static, so it runs on any x86-64 distribution.

Then load the extension: open `brave://extensions` (or `chrome://extensions`, `edge://extensions`), turn on Developer mode, choose **Load unpacked**, and pick the `browser-extension` folder the installer prints. Open your Amazon order history and press **Sync Current Page**.

## Where the Notes Go

The orders in the cover graphic are made up. The notes below are what the server wrote for them, unedited, in [docs/samples](samples); CI runs the server on those orders again on every push and fails if its output changes.

Every file the server wrote for the three sample orders:

<!-- sample-files:begin -->
```text
~/AmazonOrders/Orders/2026/08-August/Order-114-6203318-0071592.md
~/AmazonOrders/Orders/2026/09-September/Order-113-0927751-3104428.md
~/AmazonOrders/Orders/2026/10-October/Order-112-4418203-6650917.md
~/AmazonOrders/Orders/assets/B0DCBL6062.jpg
~/AmazonOrders/Orders/assets/B0DLMP4207.jpg
~/AmazonOrders/Orders/assets/B0DNTB2213.jpg
~/AmazonOrders/Orders/assets/B0DSKL1035.jpg
```
<!-- sample-files:end -->

The frontmatter is one array per field, in item order: the second brand, price and ASIN belong to the second item. Below it, each item gets its own section. Here is the cable from the October order. Its review was already posted on Amazon, so the note holds what was posted:

<!-- sample-item:begin -->
```markdown
### Braided USB-C Cable, 6 ft, 2-Pack

![image](assets/B0DCBL6062.jpg)

#### Features
Nylon braid, aluminum ends
- 60 W charging

#### 📝 My Review
[✍️ Leave a Review on Amazon](https://www.amazon.com/review/create-review?asin=B0DCBL6062)

**Title:** Long enough to reach the sofa
**Review:** Both still work after three months of being stepped on. The ends are a little bulky for my phone case.
- [x] Posted to Amazon?

---
```
<!-- sample-item:end -->

Syncing an order again rewrites its details and keeps what you wrote: a review typed under the placeholder, a draft you saved, a box you ticked.

## What's In It

| Part | Does |
|---|---|
| `src/` | The program, in Rust: the server (writes the notes, caches images, serves the dashboard and the API at `http://127.0.0.1:27483`) and, on Windows, the tray app around it (notifications, menu for sync, login, notes and logs) |
| `server/dashboard.html` | Dashboard: spend by month, orders, price drops, repeat purchases, review drafts, sync status, CSV/JSON export, backfill import. Built into the program, with its fonts and icons |
| `browser-extension/` | Reads your Amazon order pages and sends them to the server; fills Amazon's review form from a saved draft for you to edit and post |
| `install.ps1`, `install.sh` | Installers for Windows (tray app, start with Windows) and Linux (systemd user service) |
| Headless sync | On a schedule the server runs your browser headless with the extension, after a one-time login |
| Review drafts | Optional. With a Gemini key, a draft review per item that you rewrite in your own words before posting |

## Configuration

The config file is `%APPDATA%\amazon-order-sync\config.json` on Windows and `~/.config/amazon-order-sync/config.json` on Linux; set `AMAZON_SYNC_CONFIG` to use another. The installer creates it on first run and never overwrites it.

| Key | Default | Meaning |
|---|---|---|
| `notesPath` | `~/AmazonOrders` | Folder for the notes and images |
| `port` | `27483` | The extension expects this port; change both or neither |
| `autoSyncEnabled` | `false` | Headless sync on a schedule, every `autoSyncIntervalHours` (24) |
| `browserPath` | autodetect | Browser for headless runs; Brave, Edge and Chromium are tried before Chrome |
| `geminiApiKey` | empty | Turns on review drafts; `GEMINI_API_KEY` also works |

Every key, including notifications, review checks and Gemini limits, is in [guide.md#configuration](guide.md#configuration). On Windows, use the tray menu's **Edit config.json**, then **Restart server**.

## Tested On

- Windows with Brave and the tray app: daily setup
- CI on every push, on Ubuntu and Windows: the Rust tests, the HTTP tests against the built program (headless-run tests need a POSIX shell and are skipped on Windows), and a parity test comparing responses, notes, and Gemini prompts against the last Node.js server (2.3.0). Also Windows PowerShell 5.1, PSScriptAnalyzer, shellcheck, and sample orders synced through the real server
- *Not* tested: macOS, Firefox, Amazon sites other than amazon.com in day-to-day use

## Known Issues

- Google Chrome ignores `--load-extension` since version 137, so headless sync can't load the extension there. Use Brave, Edge or Chromium for headless runs; manual sync from the popup works in Chrome.
- The extension reads Amazon's pages as they are today. When Amazon changes its order or review pages, sync or auto-fill can break until the extension is updated.
- Each item in a note ends with a line of `---`. If you edit a note by hand, don't add a `---` line inside a review; the dashboard's editor rewrites one as `- - -` for you.
- A note links its images as `assets/<ASIN>.jpg`, but the images sit in `Orders/assets/`, not in the note's month folder. The dashboard finds them by ASIN; a Markdown editor that resolves the link from the note's own folder shows a broken image.
- The headless browser profile holds a signed-in Amazon session. It stays on your computer, in `%LOCALAPPDATA%\amazon-order-sync\browser-profile` or `~/.local/share/amazon-order-sync/browser-profile`; delete it to sign out.
