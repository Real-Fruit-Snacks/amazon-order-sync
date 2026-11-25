# AGENTS.md — amazon-order-sync

Repository contract for contributors and coding agents working on `Real-Fruit-Snacks/amazon-order-sync`.

## Brand System — Cyclorama Dawn `v2.1.0`

Source of truth: `Real-Fruit-Snacks/rfs-brand` (`v2.1.0`).

- **Theme Modes**: Matte Dark `LX 263` (`#050507`) is the explicit default (`<meta name="color-scheme" content="dark">`). Alabaster Day `LX 512` (`#F5F7F9`) is an opt-in toggle synchronized across tabs via `localStorage['rfs-theme']` with a radial View Transition.
- **Palette Tokens**:
  - Surfaces (Dark): `#050507` bg, `#0D0E12` surface, `#161820` raised, `#242834` line, `#5D6476` strong line, `#F2F4F8` text, `#8F95A6` mute.
  - Surfaces (Light): `#F5F7F9` bg, `#FFFFFF` surface, `#E9EEF3` raised, `#CBD3DD` line, `#76808F` strong line, `#11141B` text, `#535B69` mute.
  - Accents: Glacial Steel (`#1F547A` / `#74B8E8` / `#164669`), Maritime Beacon Orange (`#F06E2B` / `#BD4811` / `#FF9254` / `#AD3D0A`), Signal Crimson Fault (`#FF5C6C` / `#C92438` dark; `#B91C2E` light), Mint (`#3DDC97` / `#15794D`), Amber (`#FFC247` / `#885800`).
  - Gradients: Cartesian `in oklab` only (never `in oklch`). Zero neon glows, zero fake macOS traffic-light dots, zero `99px` capsule pills.
- **Typography**: Self-hosted variable WOFF2 only (`Big Shoulders Stencil`, `Albert Sans`, `Azeret Mono`, plus `OFL.txt`) in `docs/fonts/`, `server/fonts/`, and `browser-extension/fonts/`. Zero external CDNs.
- **Scroll Shell**: `<body class="rfs-shell">` + pinned `<header class="rfs-shell-header">` + `<div class="rfs-scroll-region">` on `docs/index.html` so the vertical scrollbar never enters the top header bar. Never animate `scrollbar-color`.
- **Voice**: Plain, specific, honest — zero emoji in UI chrome/docs prose, zero exclamation marks in headings or descriptions, zero hype copy.

## Multi-Surface Architecture & CI Invariants

- **Rust Server (`src/`)**: `src/http.rs` embeds `server/dashboard.html`, `server/fonts/*.woff2`, and `server/icons/*`.
- **Browser Extension (`browser-extension/`)**: Chrome Manifest V3 popup (`popup.html`, `popup.css`, `popup.js`, `fonts/`, `icon-{16,48,128}.png`).
- **Tray & Icons (`tools/make-icons.py`)**: Regenerates `server/icons/*`, `browser-extension/icon-*.png`, and `tray/app.ico` from `server/fonts/big-shoulders-stencil-latin.woff2`.
- **Generated Samples & Slip SVG (`tools/make-samples.mjs`, `tools/make-slip.py`)**: Keep `<!-- samples:begin -->` / `<!-- samples:end -->`, `<!-- files:begin -->` / `<!-- files:end -->` in `docs/index.html` and `<!-- sample-files:begin -->` / `<!-- sample-item:begin -->` in `README.md` intact. CI runs both scripts and checks `git diff --exit-code`.

## Verification

Before pushing:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
cargo build --locked --examples
node --check browser-extension/background.js
node --check browser-extension/content.js
node --check browser-extension/popup.js
node --test --test-force-exit tests/server.test.js
node tools/make-samples.mjs
python tools/make-slip.py
git diff --exit-code -- docs/samples docs/assets/slip-dark.svg docs/assets/slip-light.svg docs/index.html README.md
```
