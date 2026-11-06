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

The app icons are generated with `python3 tools/make-icons.py`, with Real-Fruit-Snacks/rfs-brand cloned next to this repo.

The README picture and the samples on the project page are generated too, and CI fails if they're out of date. After a change to how notes are written, or to `tools/samples.json`, run:

```sh
node tools/make-samples.mjs                         # syncs the made-up orders through the real server
python -m pip install -r tools/requirements-art.txt
python tools/make-slip.py                           # draws docs/assets/slip-*.svg from the notes it wrote
```

The page's design decisions are in [docs/DESIGN.md](docs/DESIGN.md).

## Developing offline

Each release has a dev kit, `amazon-order-sync-<version>-dev-windows-x64.zip`: the source at that tag and an `offline\` folder with every package the checks above need, with their licences. That is the tray app's NuGet packages (the .NET runtime packs for its self-contained build included), the Python wheels for the README picture, and PSScriptAnalyzer. The server and the tests have no npm packages. The kit leaves out the toolchains: the .NET SDK (exactly the version in `tools/versions.json`), Node.js and Python, plus Git, PowerShell 7 and ShellCheck, which are optional. Its `offline\README.md` lists each with its version and installer. Unzip the kit, install those, then run this in the kit's folder:

```powershell
powershell -ExecutionPolicy Bypass -File offline\setup.ps1
```

It needs no network. It checks the toolchains and says what's missing, restores and installs from `offline\` only, and prints the commands that test and build. `tools/build-dev-kit.ps1` builds the kit; `tools/dev-kit/` holds its `setup.ps1` and README. Before it publishes, the release workflow unzips the kit on a fresh Windows runner with the network cut off, runs `setup.ps1` and then these checks; run the workflow by hand (Actions, release, Run workflow) to check without publishing.

Versions are pinned in `tools/versions.json` (Node.js, the .NET SDK, Python, PSScriptAnalyzer, ShellCheck) and `tools/requirements-art.txt` (the wheels). CI, the release and the dev kit read those files, so change a version there and nowhere else.
