# v1.7.2 Plan — Hardening release (full-review fixes)

Triaged from the 3-reviewer audit. All MAJOR findings + cheap MINORs in scope; deferred: no-Origin form
POST (browsers send Origin), downloadImage SSRF (defense-in-depth), stop() 409 micro-window, cosmetic nits.

## Server + ops (Coder A, branch v172-server)
- S1: parseAlignedArray for ALL index-aligned frontmatter fields (rating, brand, price, url, reviewed) in
  parseNote, analyzeOrder (/api/check), and every re-sync preservation path. Filtering parser only for asin
  existence lookups. THE data-corruption fix.
- S2: setFrontmatterArrayValue uses aligned parse for helpful_votes (descending-order writes no longer erase).
- S3: escape ALL interpolated frontmatter values (quotes + CR/LF strip), not just brand.
- S4: sync-time ASIN dedupe within one order payload (first occurrence wins, log line) — kills the
  duplicate-ASIN wrong-item-write class at the source.
- S5: install.sh `systemctl --user try-restart` after copy (upgrades actually take effect).
- S6: legacy notify() routed through notificationsEnabled config.
- S7: gemini quota — don't flag a REAL generated draft as fallback when the request succeeded at the limit edge.
- S8: CSV export — prefix formula-injection chars (=,+,-,@) with '.
- S10: headless meta gains runStartedAt (dashboard already reads it).
- Tests: descending-order vote write (S2), aligned rating/brand/url parse (S1), YAML escaping (S3), ASIN
  dedupe (S4), re-sync review+checkbox preservation roundtrip, /api/check transitions, gemini 429→success
  retry + quota counting, log-buffer 500-cap wraparound. README: add client-log + favicon/icons routes,
  3 tabs→4, health example 1.7.2. CHANGELOG 1.7.2.

## Client (Coder B, branch v172-ui)
- E1: popup state from explicit signaling — content.js sends explicit run-state (message response field +
  precise end-of-run sentinel, no "complete" substring matching); add isSyncing re-entry guard.
- E2: regional scraping — NEVER default date to today (send null/"Unknown", server puts note in root rather
  than corrupting); total regex extended to £,€,¥ + currency captured.
- E3: dashboard dirty-tracks Review Studio edits per ASIN (state.edits) and restores them across ALL
  re-render paths; cleared on successful save/regenerate/post.
- E4: log poller single-chain via generation counter.
- E5: extension log forwarding stops at run end; final flush drains >100 lines (loop).
- E6: popup gate via URL hostname/pathname parsing; sendMessage lastError → user-visible log line.
- E7: review-check redirect — hash present but wrong pathname → completion ping {ok:false,error:'redirected'}.
- E8: SYNC tab clarity — relabel panel "HEADLESS RUN STATUS", add "LAST BROWSER SYNC: X ago" line from
  lastSyncAt so manual syncs don't look invisible.
- E9: reviewCheckNow 409 notice text fix ("Review check already running").
- E10: popup.html label → 1.7.2. manifest 1.7.2. dashboard label 1.7.2.

## Stage 2 (orchestrator)
Merge, full tests, live smoke incl. misalignment regression data, screenshots, zip, final report.
