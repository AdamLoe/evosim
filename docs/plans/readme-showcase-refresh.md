---
status:        shipped
owner:         unassigned
last_updated:  2026-06-23
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/app-shell.md
  - decisions/app-shell.md
---

# README showcase refresh

## Mission

Rebuild `README.md` as a short GitHub / portfolio showcase for recruiters and
quick glancers after the reliability gate and app UI cleanup land. The finished
README should make the live demo obvious, explain the project in plain language,
show current screenshots or GIFs, and route real developers or agents into
`docs/` instead of duplicating deep subsystem documentation.

## Scope

In scope:

- Rewrite `README.md` around the current live demo and current UI.
- Replace or add showcase assets under `app/assets/`.
- Capture fresh screenshots after UI cleanup, including a running world,
  Settings/NN, Inspector or Profiler, and the General demo cockpit if it looks
  portfolio-worthy.
- Keep a short technical section: Rust/Wasm worker, WebGL2 renderer, threaded
  wasm/rayon, and plain TypeScript shell.
- Include a short developer/agent pointer to `docs/index.md`,
  `docs/overview.md`, and the fresh-chat workflow.

Out of scope:

- Deep developer setup docs beyond a concise pointer to `docs/`.
- Architecture explanations that belong in `docs/architecture/*`.
- Performance claims unless rechecked or clearly phrased as approximate /
  current-machine.
- README screenshots before the UI cleanup is visually final.

## Approach

1. Source-truth pass after UI cleanup.
   - Confirm [`live-demo-reliability-release-gate.md`](live-demo-reliability-release-gate.md)
     has shipped or that the live link is not advertised as ready yet.
   - Re-read `docs/overview.md`, `docs/architecture/app-shell.md`,
     `app/web/index.html`, and `app/web/src/main.ts`.
   - Use app source for visible UI labels and docs for system-level facts.
   - Remove stale claims from the current README: no saves, Monitor top-level
     tab, NN top-level rail tab, and fixed-only `1200x1200` world framing.
   - Be precise about visible persistence controls. Current docs describe
     autosave/named-save/resume/fork/export/import capability, but README copy
     should not overpromise controls that are not visible in the final UI.

2. Asset capture pass.
   - Start the web app with `cd app/web && pnpm dev`.
   - Capture current assets only after the UI readiness plan has shipped.
   - Prefer replacing or superseding the current assets:
     `app/assets/demo2.gif`, `app/assets/app+nn.png`, and
     `app/assets/app+settings+perf.png`.
   - Use stable viewport sizes and avoid half-finished panels, debug clutter,
     or local-only browser chrome.
   - Keep assets reasonably compressed for GitHub README load time.

3. README rewrite.
   - Lead with project identity, live demo link, and one strong visual.
   - Make the first paragraphs answer: what is this, why is it interesting,
     what can I poke at in 30 seconds?
   - Use current terms: General, Inspector, Settings, Profiler; NN as a
     Settings category unless the UI plan changes that.
   - Keep "How it is built" compact and recruiter-readable.
   - End with a short "For developers / agents" section that routes to
     `docs/index.md` and `docs/overview.md`.

## Exit gate

- `README.md` no longer contains stale claims about no saves, Monitor tab,
  NN top-level tab, or fixed-only world size.
- README image references resolve to existing `app/assets/*` files.
- Fresh screenshots reflect the post-cleanup UI.
- The reliability/release gate has passed before final live-link wording is
  treated as ready for portfolio use.
- `cd app/web && pnpm docs:lint`
- `cd app/web && pnpm typecheck`
- Manual GitHub-render check of README image sizing and Mermaid rendering.

## Open decisions

- Should README describe named Save/Resume/Fork as user-visible product
  capability, or only mention autosave/export/import unless the final General
  tab exposes those actions?
- Should existing asset filenames be overwritten for stable README links, or
  should new descriptive filenames be added?
- Should the performance section remain? Recommendation: keep it only if the
  claim is either re-benchmarked or softened enough that it will not rot
  quickly.

## Implementer brief

Goal:
Refresh README and showcase assets for a polished GitHub Pages / portfolio
impression.

Non-goals:
Do not expand README into developer docs; do not capture final screenshots
before UI cleanup is done.

Authoritative docs:
`docs/overview.md`, `docs/index.md`, `docs/architecture/app-shell.md`,
`app/web/index.html`, and this plan.

Likely source areas:
`README.md`, `app/assets/demo2.gif`, `app/assets/app+nn.png`,
`app/assets/app+settings+perf.png`, and possibly new `app/assets/*.png` if
replacement filenames would make review harder.

Expected behavior:
README is short, current, visually compelling, and points developers/agents to
the docs router instead of trying to become full contributor documentation.

Implementation notes:
Do this after the UI readiness plan so screenshots and visible control names do
not churn. If source and docs disagree, use visible app source for user-facing
copy and update docs during ship-time migration if needed.

Cheapest sufficient checks:
`pnpm docs:lint`, `pnpm typecheck`, asset path check, and manual rendered README
review.

Stop and report if:
The UI cleanup is not done, the live demo cannot run locally, or visible UI
contradicts docs in a way that affects public copy.

## Migration notes (filled in at ship time)

Shipped 2026-06-23. No stale facts found in `docs/overview.md` or
`docs/architecture/app-shell.md` — both already reflected the post-Wave-2 UI
(General / Inspector / Settings / Profiler tabs; NN under Settings; no Monitor
tab; autosave + Export/Import present).

Changes made:
- `README.md` fully rewritten: removed stale GitHub Pages live link (deferred
  to "Live demo coming soon"), removed "Monitor" tab reference, removed "no
  save" claim, removed NN as a top-level tab, updated persistence description to
  Export/Import + autosave, added General tab description, softened perf section
  to "on a recent desktop / results vary."
- `app/assets/demo2.gif` superseded by `app/assets/demo2.png` (static PNG,
  276 KB vs 9.2 MB; headless GIF capture would require a video encoder pipeline
  and the plan permitted PNG substitution when clean GIF was impractical).
- `app/assets/app+nn.png` replaced with post-Wave-2 Settings/NN screenshot (396 KB vs 1.6 MB).
- `app/assets/app+settings+perf.png` replaced with post-Wave-2 Profiler screenshot (436 KB vs 1.3 MB).
- `app/assets/app+general.png` added (new — General tab/demo cockpit, 324 KB).

Most README copy is not durable architecture. No migration into
architecture/decisions was needed beyond confirming the existing docs are
already current.

## See also

- [`docs/plans/index.md`](index.md)
- [`../overview.md`](../overview.md)
- [`../architecture/app-shell.md`](../architecture/app-shell.md)
