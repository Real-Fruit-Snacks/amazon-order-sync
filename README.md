# amazon-order-sync

Syncs your Amazon orders into local Markdown notes

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/slip-dark.svg">
  <img alt="A packing slip for a made-up order, 113-0927751-3104428, is scanned top to bottom. As each field lights up, the matching frontmatter line prints in the note beside it: order_id, date, total, status, ship_to, then brand, price, asin, url, reviewed and rating. The note is saved as Orders/2026/09-September/Order-113-0927751-3104428.md, and the October order comes back the same way." src="docs/assets/slip-light.svg" width="100%">
</picture>

```powershell
$zip = "$env:TEMP\amazon-order-sync-windows.zip"
Invoke-WebRequest https://github.com/Real-Fruit-Snacks/amazon-order-sync/releases/latest/download/amazon-order-sync-windows.zip -OutFile $zip
Expand-Archive $zip "$env:LOCALAPPDATA\Programs\amazon-order-sync" -Force
Set-ExecutionPolicy -Scope Process Bypass; & "$env:LOCALAPPDATA\Programs\amazon-order-sync\install.ps1"
```

- **Local Markdown archive:** Saves one Markdown note per order filed by month (`Orders/YYYY/MM-Month/Order-<id>.md`) with cached product images next to them; re-syncing preserves your reviews and drafts.
- **Single Rust binary + extension:** Browser extension reads your Amazon order history and posts to `127.0.0.1:27483`; includes a Windows tray app (or Linux `systemd` user service) and embedded local dashboard.
- **Zero external servers:** Everything stays on your machine unless you opt into Gemini review drafts with your own API key.
- **Optional headless sync:** Scheduled browser runs keep notes updated automatically after a one-time login (Brave, Edge, or Chromium).

## Documentation

- [Project Page & Screenshots](https://real-fruit-snacks.github.io/amazon-order-sync/)
- [Usage, Linux/Offline Install, Sample Output & Known Issues](docs/USAGE.md)
- [Full Guide & Configuration](docs/guide.md) · [HTTP API](docs/api.md)
- [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

## License

[MIT](LICENSE). Not affiliated with Amazon. Bundled fonts (`Big Shoulders Stencil`, `Albert Sans`, `Azeret Mono`) are SIL OFL 1.1 (`server/fonts/OFL.txt`, `docs/fonts/OFL.txt`).
