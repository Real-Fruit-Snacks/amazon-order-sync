# AGENTS.md — amazon-order-sync

Repository contract for contributors and coding agents working on `Real-Fruit-Snacks/amazon-order-sync`.

## Brand System — Cyclorama Dawn `v2.1.0`

Source of truth: `Real-Fruit-Snacks/rfs-brand` (`v2.1.0`).

- **Theme Modes**: Matte Dark `LX 263` (`#050507`) is the explicit default (`<meta name="color-scheme" content="dark">`). Alabaster Day `LX 512` (`#F5F7F9`) is an opt-in toggle synchronized across tabs via `localStorage['rfs-theme']` with a radial View Transition.
- **Palette Tokens**:
  - Surfaces (Dark): `#050507` bg, `#070709` rail, `#0C0C10` surface, `#14141A` raised, `rgba(255, 255, 255, 0.14)` line, `#62646D` strong line, `#F4F5F8` text, `#B3B5C0` mute, `#08080B` code-bg.
  - Surfaces (Light): `#F5F7F9` bg, `#EAEEF2` rail, `#FFFFFF` surface, `#E3E8EE` raised, `rgba(13, 16, 22, 0.16)` line, `#6E7682` strong line, `#0D1016` text, `#474D59` mute, `#EEF2F5` code-bg.
  - Accents: Glacial Steel (`#1F547A` / `#18496B` / `#74B8E8` / `#164669`), Maritime Beacon Orange (`#F06E2B` / `#BD4811` / `#FF9254` / `#AD3D0A`), Signal Crimson Fault (`#FF5C6C` / `#C92438` / `#E02E44` dark; `#B91C2E` / `#991525` light; `#FFFFFF` on-fault), Mint (`#3DDC97` / `#127247`), Amber (`#FFC247` / `#885800`).
  - Gradients: Cartesian `in oklab` only (never `in oklch`). Zero neon glows, zero fake macOS traffic-light dots, zero `99px` capsule pills.
- **Typography**: Self-hosted variable WOFF2 only (`Big Shoulders Stencil`, `Albert Sans`, `Azeret Mono`, plus `OFL.txt`) in `docs/fonts/`, `server/fonts/`, and `browser-extension/fonts/`. Zero external CDNs.
- **Scroll Shell**: `<body class="rfs-shell">` + pinned `<header class="rfs-shell-header">` + `<div class="rfs-scroll-region">` on `docs/index.html` so the vertical scrollbar never enters the top header bar. Never animate `scrollbar-color`.
- **Voice**: Plain, specific, honest — zero emoji in UI chrome/docs prose, zero exclamation marks in headings or descriptions, zero hype copy.
- **README & Docs (`BRAND.md` §14)**: Keep `README.md` ultra-minimal (40–60 lines); put sample notes, install details, config tables, and known issues in `docs/USAGE.md` and `docs/guide.md` (never GitHub Wiki).

## Multi-Surface Architecture & CI Invariants

- **Rust Server (`src/`)**: `src/http.rs` embeds `server/dashboard.html`, `server/fonts/*.woff2`, and `server/icons/*`.
- **Browser Extension (`browser-extension/`)**: Chrome Manifest V3 popup (`popup.html`, `popup.css`, `popup.js`, `fonts/`, `icon-{16,48,128}.png`).
- **Tray & Icons (`tools/make-icons.py`)**: Regenerates `server/icons/*`, `browser-extension/icon-*.png`, and `tray/app.ico` from `server/fonts/big-shoulders-stencil-latin.woff2`.
- **Generated Samples & Slip SVG (`tools/make-samples.mjs`, `tools/make-slip.py`)**: Keep `<!-- samples:begin -->` / `<!-- samples:end -->`, `<!-- files:begin -->` / `<!-- files:end -->` in `docs/index.html` and `<!-- sample-files:begin -->` / `<!-- sample-item:begin -->` in `docs/USAGE.md` intact. CI runs both scripts and checks `git diff --exit-code`.

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
