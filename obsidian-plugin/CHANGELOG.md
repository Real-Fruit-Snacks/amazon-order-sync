# Changelog

## 1.0.7 — 2026-07-19

### Security
- The local sync server now rejects requests from any origin other than amazon.com. Previously it answered with `Access-Control-Allow-Origin: *` and no origin check, which let any website open in your browser read your synced order history and reviews — and trigger vault writes — by talking to `127.0.0.1:27483`. Requests are now refused server-side (not merely blocked from reading the response), and only the companion extension's amazon.com content script is honored. Requests with no `Origin` header (non-browser local tooling) are unaffected.
- Removed the `/api/debug` endpoint, which wrote an arbitrary request body to a file in the vault. It was unused by the extension and, combined with the wildcard CORS above, allowed any website to write a file into your vault.

### Changed
- Pinned the Obsidian API development dependency for reproducible builds.
- Added a unit test for the new origin check and continuous integration (build, tests, manifest/versions validation for the plugin; syntax checks for the extension).
