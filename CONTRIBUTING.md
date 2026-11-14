# Contributing

- The program is Rust (`src/`): the server everywhere, and on Windows the tray app around it, in one binary. Keep its dependencies few; each one ships to every user and its licence goes into `THIRD-PARTY-LICENSES.txt` (`about.toml` lists the accepted ones). The extension and dashboard stay plain JavaScript with no build step.
- The notes, the sidecar files and every API response match what the Node.js server (2.3.0) produced, byte for byte: `src/js.rs` and `src/jsdate.rs` hold the JavaScript rules that depends on (number formatting, `toFixed`, `Date` parsing, regular expressions, object key order). `tests/parity.test.js` checks it against `server/` at the v2.3.0 tag; change behaviour on purpose, and adjust that test with it.
- Every call to the server from the extension goes through `browser-extension/background.js`. Don't add a `fetch` to `127.0.0.1` in `content.js`; the server refuses Amazon page origins.
- The extension's `key` in `manifest.json` fixes its ID, and `EXTENSION_ID` in `src/http.rs` must match it (a test checks). Don't change one without the other.
- Notes are user data. A change to how notes are written must keep existing notes readable and never drop text the user wrote; add a test that re-syncs a note with the user's edits in it.
- `install.ps1` must run on Windows PowerShell 5.1: ASCII only, `Get-Content -Encoding UTF8` for anything that reads JSON.
- Bump the version in every place for a change that ships: `Cargo.toml` (and `Cargo.lock`, by building), `browser-extension/manifest.json`, `tray/app.manifest`, the dashboard's `v2.x.y` label, the tests that check it, and the top of `CHANGELOG.md`.

Run the checks before opening a PR:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                                 # the internals: note format, store, Gemini, headless runner
cargo build --release
node --test tests/server.test.js           # the HTTP API, against target/release/amazon-order-sync
shellcheck install.sh
```

The parity test against the Node.js server (needs git with the v2.3.0 tag):

```sh
cargo build --examples
AOS_SERVER_BIN=target/release/amazon-order-sync AOS_FAKE_BROWSER_BIN=target/debug/examples/fake_browser \
  node --test --test-concurrency=1 tests/parity.test.js
```

The app icons are generated with `python3 tools/make-icons.py`, with Real-Fruit-Snacks/rfs-brand cloned next to this repo.

The README picture and the samples on the project page are generated too, and CI fails if they're out of date. After a change to how notes are written, or to `tools/samples.json`, run:

```sh
cargo build --release
node tools/make-samples.mjs                         # syncs the made-up orders through the real server
python -m pip install -r tools/requirements-art.txt
python tools/make-slip.py                           # draws docs/assets/slip-*.svg from the notes it wrote
```

The page's design decisions are in [docs/DESIGN.md](docs/DESIGN.md).

## Developing offline

Each release has a dev kit, `amazon-order-sync-<version>-dev-windows-x64.zip`: the source at that tag and an `offline\` folder with every package the checks above need, with their licences. That is every Rust crate `Cargo.lock` names (`cargo vendor`), the Python wheels for the README picture, and PSScriptAnalyzer. The tests have no npm packages. The kit leaves out the toolchains: Rust (the version in `tools/versions.json`) with the Visual Studio Build Tools for its linker, Node.js and Python, plus Git, PowerShell 7 and ShellCheck, which are optional. Its `offline\README.md` lists each with its version and installer. Unzip the kit, install those, then run this in the kit's folder:

```powershell
powershell -ExecutionPolicy Bypass -File offline\setup.ps1
```

It needs no network. It checks the toolchains and says what's missing, points cargo at the kit's crates and installs from `offline\` only, and prints the commands that test and build. `tools/build-dev-kit.ps1` builds the kit; `tools/dev-kit/` holds its `setup.ps1` and README. Before it publishes, the release workflow unzips the kit on a fresh Windows runner with the network cut off and cargo's cache emptied, runs `setup.ps1` and then these checks; run the workflow by hand (Actions, release, Run workflow) to check without publishing.

Versions are pinned in `tools/versions.json` (Rust, Node.js, Python, PSScriptAnalyzer, ShellCheck) and `tools/requirements-art.txt` (the wheels). CI, the release and the dev kit read those files, so change a version there and nowhere else.
