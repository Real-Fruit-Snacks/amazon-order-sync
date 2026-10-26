# Contributing

- The server stays one file with no npm dependencies (Node.js 18 standard library only). The extension and dashboard stay plain JavaScript with no build step.
- Every call to the server from the extension goes through `browser-extension/background.js`. Don't add a `fetch` to `127.0.0.1` in `content.js`; the server refuses Amazon page origins.
- The extension's `key` in `manifest.json` fixes its ID, and `EXTENSION_ID` in `server/server.js` must match it (a test checks). Don't change one without the other.
- Notes are user data. A change to how notes are written must keep existing notes readable and never drop text the user wrote; add a test that re-syncs a note with the user's edits in it.
- `install.ps1` must run on Windows PowerShell 5.1: ASCII only, `Get-Content -Encoding UTF8` for anything that reads JSON.
- Bump the version in every place for a change that ships: `VERSION` in `server/server.js`, `server/package.json`, `browser-extension/manifest.json`, `tray/AmazonOrderSync.Tray.csproj`, the dashboard's `v2.x.y` label, the tests that check it, and the top of `CHANGELOG.md`.

Run the checks before opening a PR:

```sh
node --check server/server.js
node --test tests/server.test.js
shellcheck install.sh
dotnet build tray -c Release    # Windows, .NET 8 SDK or newer
```

Icons and the README cover are generated: `python3 tools/make-icons.py` and `python3 ../rfs-brand/tools/make-cover.py docs/assets/cover.toml`, with Real-Fruit-Snacks/rfs-brand cloned next to this repo.
