# Design: the amazon-order-sync page (docs/) and README picture

## Concept

Every Amazon parcel comes with paperwork: a slip that says what's in the box, when it was ordered and who it's for. amazon-order-sync turns each order into one Markdown file in a dated folder. So the page is set like parcel paperwork: condensed capitals in ruled boxes, a barcode, a "made-up order" tag, and the folder each note lands in set big like a routing code, `ORDERS / 2026 / 10-OCTOBER`. A visitor should leave knowing that one order becomes one file in a month folder on their own disk, and what that file says.

## Audience and primary action

People who buy a lot on Amazon and keep notes in Markdown (often in Obsidian), on Windows or Linux. They need to see what a note looks like, that nothing leaves their computer, and how to install it. The action is the install command for their platform, then loading the extension.

## Type

- Display & routing codes: **Big Shoulders Stencil** (variable `wght` 100–900), self-hosted (`docs/fonts/`, `server/fonts/`, `browser-extension/fonts/`). Used for the headline, section headings, routing codes (`ORDERS / 2026 / 10-OCTOBER`), and slip order/status stamps.
- Body: **Albert Sans** (variable `wght` 100–900), self-hosted. Clean geometric sans for body prose and item labels.
- Mono: **Azeret Mono** (variable `wght` 100–900), self-hosted. Used for notes, frontmatter, commands, file names, field labels, and dashboard tables (`font-variant-ligatures: none`).
- All fonts carry their SIL Open Font License (`OFL.txt`) with zero external CDNs.

## Color

Canonical **Cyclorama Dawn `v2.1.0`** tokens from `Real-Fruit-Snacks/rfs-brand` (`rfs.css` + `rfs-components.css`). Matte Dark `LX 263` (`#050507`) is the explicit default (`<meta name="color-scheme" content="dark">`); Alabaster Day `LX 512` (`#F5F7F9`) is an opt-in toggle synchronized across tabs via `localStorage['rfs-theme']` with a radial View Transition.

| Role | LX 263 Dark | LX 512 Light | Use |
|---|---|---|---|
| bg / surface / raised | `#050507` / `#0D0E12` / `#161820` | `#F5F7F9` / `#FFFFFF` / `#E9EEF3` | Page, pinned header, file tabs and inline code |
| code | `#07080B` | `#FFFFFF` | Note window and command boxes |
| paper / paper ink | `#F2F4F8` / `#050507` | `#FFFFFF` / `#11141B` | Packing slip (light stock on dark desk at night) |
| line / line-strong | `#242834` / `#5D6476` | `#CBD3DD` / `#76808F` | Rules, borders |
| ink / mute | `#F2F4F8` / `#8F95A6` | `#11141B` / `#535B69` | Text |
| Glacial Steel / Beacon Orange | `#1F547A` / `#74B8E8` · `#F06E2B` / `#FF9254` | `#164669` · `#BD4811` / `#AD3D0A` | Scan beam, "Writing", routing slashes, Cartesian `in oklab` Copy gradient, links |
| Amber | `#FFC247` | `#885800` | Frontmatter keys and slip-to-note highlight |
| Mint | `#3DDC97` | `#15794D` | "Saved", `true`, "Review posted", "Stays here" |
| Signal Crimson | `#FF5C6C` / `#C92438` | `#B91C2E` | `false` tokens, extension Stop Sync destructive button |

Every colour that means a state comes with a word: Writing, Saved, Review posted, No review yet, Stays here, Sent to Gemini, true and false. The pinned header (`.rfs-shell-header`) sits above `.rfs-scroll-region` so the vertical scrollbar never enters the top bar.

## Layout

A 1240px wrap on a 12-column grid. The headline runs across the top. Under it, the sub and the install box take four columns and the bench takes eight: the routing code, then the slip beside the note. On a phone: headline, sub, routing code, slip, note, install. The sections below each take the form their content needs: the folder listing as rows of routing codes, a packing list for the parts, the two dashboard screens, a two-column "what leaves your computer", the config keys, and tested-on and known issues.

## Signature

Folder paths set as routing codes, in heavy condensed capitals with Ember slashes: above the note, in the folder listing, and on the social card. And the slip: ruled fields, the order number as a Code 39 barcode, and a "Made-up order" tag.

## Wow moment

On load the bench shows the October order's slip beside an empty file. A scan line runs down the slip in 1.9 s. As it reaches each field, the field lights like a highlighter and the frontmatter lines that came from it print in the note with the same highlight: `order_id`, `date`, `total`, `status`, `ship_to`, then `brand`, `price`, `asin`, `url`, `reviewed` and `rating` from the items. The rest of the note follows, the tab changes from Writing to Saved, and `ORDERS / 2026 / 10-OCTOBER` stamps in above. "Sync the next order" brings in the September and August orders the same way.

The slips are drawn from `tools/samples.json` and the notes are what the real server wrote for those orders (`tools/make-samples.mjs` runs it and writes them into the page), so the demo can't drift from the tool. The caption says it's a simulation with made-up orders.

- **No JS:** the October slip and its whole note, complete.
- **Reduced motion:** nothing runs on its own; the button swaps orders without the scan.
- **Phone:** the same bench, stacked. The first run starts when the note scrolls into view.
- A CSS failsafe shows the first note after 6 s if the script never runs.

## README picture

`tools/make-slip.py` draws the bench as `assets/slip-{dark,light}.svg`, text turned to paths from `fonts/` (an `<img>` can't load fonts), in a 14 s loop: the September order is scanned and written, then the October order again. Its base styles are the first frame, the October order complete. CI runs `make-samples.mjs` and `make-slip.py` and fails if anything changed. `card.html` puts that first frame next to the name for the 1280 x 640 social preview (`assets/social.png`).

Don't wrap the README's `<picture>` in a link: GitHub then shows the light image in dark mode.

## Motion

The scan is linear. Highlights snap on and fade over 700 to 900ms. Slips arrive with a 420ms drop and a slight turn, the routing code with a 380ms stamp. Hovers are 150ms. Nothing loops on the page and nothing flashes.

## Voice

Plain and exact, as in the README: real commands, real file names, real frontmatter. Limits are stated near the top of their section. The only invented things are the three orders, and they're labelled on the slip and in the caption.

## Files

- `index.html`, `styles.css`, `script.js`, `fonts/`, the icons and the manifest. The favicon is the Cyclorama Dawn RFS monogram tile with the Steel-to-Beacon horizon rule.
- `samples/` is what the server wrote for the sample orders, `files.txt` lists every file it wrote.
- `assets/cover-*`, `assets/social-dark.png`, `assets/social-light.png` and `assets/cover.toml` are the RFS brand cover from rfs-brand's `make-cover.py`. The page and README don't use them, but the rfs-brand site's cover gallery loads `cover-{dark,light}.svg` from here, so they stay. The dashboard screenshots (`assets/dashboard-*.webp`) are from the earlier page and show made-up orders.

## Alternatives considered

- A filing cabinet: each month a drawer tab, orders as index cards that slide out. Warmer, but it hides the file itself, which is the point.
- A receipt printer: the order history printing as one long thermal receipt that tears off into `.md` files. Fun, but a receipt isn't how Amazon orders look, and the long strip fights a phone screen.
