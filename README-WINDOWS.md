# Amazon Order Sync on Windows

The Windows build is the same zero-dependency Node server and Brave/Chrome
extension as the Linux version, wrapped in a small native tray app
(`AmazonOrderSync.exe`, C#/.NET 8 WinForms, self-contained — no .NET install
needed). The tray app replaces the systemd user service and `notify-send`:

| Linux                                   | Windows                                                        |
| --------------------------------------- | -------------------------------------------------------------- |
| `systemctl --user` service              | Tray app starts/stops `node server\server.js` (hidden window)  |
| `notify-send` toasts                    | Server queues toasts at `/api/notifications`; tray shows them  |
| `~/.config/amazon-order-sync/config.json` | `%APPDATA%\amazon-order-sync\config.json`                    |
| `~/.local/share/amazon-order-sync/`     | `%LOCALAPPDATA%\amazon-order-sync\` (browser profile, logs)    |
| Chromium found on `PATH`                | Brave → Chrome → Edge → Chromium, PATH then install folders    |
| `journalctl --user -u …`                | `%LOCALAPPDATA%\amazon-order-sync\server.log` (+ `tray.log`)   |

## Layout

```
C:\Users\<you>\Projects\amazon\
├── AmazonOrderSync.exe      ← tray app (built from tray\)
├── install.ps1              ← installer / upgrader
├── server\                  ← Node daemon + dashboard
├── browser-extension\       ← load this folder in Brave
├── tray\                    ← C# source for the tray app
├── tests\
└── AmazonOrders\            ← your notes (default notesPath)
```

The tray app finds `server\server.js` relative to its own location, so keep
the exe at the project root (or set `AOS_SERVER_DIR`).

## Install

1. Open PowerShell in the project folder and run:

   ```powershell
   Set-ExecutionPolicy -Scope Process Bypass
   .\install.ps1
   ```

   It installs Node.js LTS via winget if missing, writes a default
   `config.json` pointing notes at `.\AmazonOrders`, registers the tray app
   to start with Windows, launches it and checks `/api/health`.

   Options: `-NotesPath D:\Somewhere` (only used when creating a new config),
   `-NoAutostart`, `-Uninstall`.

2. Load the extension in Brave: `brave://extensions` → **Developer mode** →
   **Load unpacked** → pick `browser-extension\`. Pin it, open your Amazon
   orders page, click **Sync Current Page**.

3. Optional — AI review drafts: put your Gemini key in `config.json`
   (tray menu → **Edit config.json**), then **Restart server**.

4. Optional — headless auto-sync: tray menu → **Set up Amazon login…**, sign
   in to Amazon in the window that opens (turn Brave Shields down for
   amazon.com), close it. Set `"autoSyncEnabled": true` in `config.json` and
   restart the server. The daemon then runs Brave headless with the extension
   every `autoSyncIntervalHours`.

## Tray menu

- **Open dashboard** (also double-click the icon) — `http://127.0.0.1:27483/`
- **Sync now** — headless run; completion/failure arrives as a toast
- **Set up Amazon login…** — visible Brave window on the dedicated profile
- **Open notes folder / Edit config.json / View server log**
- **Open browser-extension folder** — for Brave's *Load unpacked*
- **Start with Windows** — toggles the `HKCU\…\Run` entry
- **Restart server** — after editing `config.json`
- **Quit** — asks the server to shut down cleanly (`POST /api/shutdown`),
  which also kills any headless browser and releases the `.daemon.lock`

If the tray app starts and a server is already answering on the port (say an
orphan from a crashed tray) it attaches to it instead of starting a second
daemon on the same notes folder.

## Rebuilding the tray app

Needs the .NET 8 SDK (`winget install Microsoft.DotNet.SDK.8`). From `tray\`:

```powershell
dotnet publish -c Release -o ..\
```

That drops a fresh `AmazonOrderSync.exe` at the project root. The server
needs no build step; the tests run with `cd server; node --test ..\tests\server.test.js`.

## Troubleshooting

- **"Node.js was not found"** — install it (`winget install OpenJS.NodeJS.LTS`),
  then **Restart server** from the tray.
- **Server exits immediately** — read `server.log`. Usually the port is in use
  or `config.json` has a typo.
- **"No browser found" toast** — Brave/Chrome/Edge isn't in a standard place;
  set `"browserPath": "C:\\Users\\<you>\\AppData\\Local\\BraveSoftware\\Brave-Browser\\Application\\brave.exe"`.
- **Headless sync stalls / login errors** — tray → **Set up Amazon login…**
  again. To start over, quit the tray and delete
  `%LOCALAPPDATA%\amazon-order-sync\browser-profile`.
- **Toasts not showing** — Windows Focus Assist / notification settings for
  "AmazonOrderSync"; the balloon tips use the standard Windows toast channel.
