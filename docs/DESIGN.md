# Design: the amazon-order-sync page (docs/) and README picture

## Concept

Every Amazon parcel comes with paperwork: a slip that says what's in the box, when it was ordered and who it's for. amazon-order-sync turns each order into one Markdown file in a dated folder. So the page is set like parcel paperwork: condensed capitals in ruled boxes, a barcode, a "made-up order" tag, and the folder each note lands in set big like a routing code, `ORDERS / 2026 / 10-OCTOBER`. A visitor should leave knowing that one order becomes one file in a month folder on their own disk, and what that file says.

## Audience and primary action

People who buy a lot on Amazon and keep notes in Markdown (often in Obsidian), on Windows or Linux. They need to see what a note looks like, that nothing leaves their computer, and how to install it. The action is the install command for their platform, then loading the extension.

## Type

- Display, slip and labels: Archivo, variable (width 62 to 125, weight 100 to 900). At 62% width and 800 to 900 it's the heavy condensed grotesque of parcel labels and sort codes: the headline, the routing codes, the slip's status and order number. At 75% and 700, uppercase and tracked, it's the slip's field labels. At 100% and 400 to 650 it's the body text, so the page has one voice.
- Mono: Martian Mono, variable, at 75% width: the notes, commands and file names. Its squared shapes sit well next to a label, and the condensed width fits a frontmatter line in a half-width column.
- Scale: `scale.mjs`, 16 to 18px, ratio 1.2 to 1.333. The headline is `clamp(2.75rem, 0.9rem + 6.6vw, 7.6rem)`, two lines at 1440.
- Fonts are self-hosted in `fonts/` (Google Fonts latin subsets) with their OFL licences. The dashboard and extension keep IBM Plex.

## Color

The RFS Nightshift and Dayshift values and nothing else. The OS setting decides the theme, the Nightshift/Dayshift switch overrides it, and the choice is saved as `rfs-theme` like the other RFS pages.

| Role | Night | Day | Use |
|---|---|---|---|
| bg / surface / raised | `#0C0D10` / `#13151A` / `#1A1D24` | `#F6F5F2` / `#FFFFFF` / `#ECEAE4` | Page; file tabs and inline code (raised) |
| code | `#070809` | `#FFFFFF` | The note window and command boxes |
| paper / paper ink | `#E8E6E1` / `#0C0D10` | `#FFFFFF` / `#1A1B1E` | The slip. It's light in both themes: a paper label on a dark desk at night |
| line / line-strong | `#262A33` / `#636872` | `#D6D3CB` / `#89867F` | Rules, borders |
| ink / mute | `#E8E6E1` / `#8A8F99` | `#1A1B1E` / `#666A73` | Text |
| Ember | `#FF6A3D` | `#C03B0A` | The sync: the scan beam, "Writing", the slashes in a routing code, the Copy button, focus |
| Amber | `#FFC247` | `#925F00` | Read from the order: frontmatter keys, and the highlighter that lights a slip field and the line it became (`#FFC247` as a tint in both themes) |
| Mint | `#3DDC97` | `#15794D` | "Saved", `true`, "Review posted" (`#15794D` on the paper), "Stays here" |
| Raspberry | `#F06292` | `#B0306A` | `false` |

Every colour that means a state comes with a word: Writing, Saved, Review posted, No review yet, Stays here, Sent to Gemini, true and false.

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

- `index.html`, `styles.css`, `script.js`, `fonts/`, the icons and the manifest. The favicon is a paper label with an Ember header and four barcode bars on the Nightshift tile.
- `samples/` is what the server wrote for the sample orders, `files.txt` lists every file it wrote.
- `assets/cover-*`, `assets/social-dark.png`, `assets/social-light.png` and `assets/cover.toml` are the RFS brand cover from rfs-brand's `make-cover.py`. The page and README don't use them, but the rfs-brand site's cover gallery loads `cover-{dark,light}.svg` from here, so they stay. The dashboard screenshots (`assets/dashboard-*.webp`) are from the earlier page and show made-up orders.

## Alternatives considered

- A filing cabinet: each month a drawer tab, orders as index cards that slide out. Warmer, but it hides the file itself, which is the point.
- A receipt printer: the order history printing as one long thermal receipt that tears off into `.md` files. Fun, but a receipt isn't how Amazon orders look, and the long strip fights a phone screen.
