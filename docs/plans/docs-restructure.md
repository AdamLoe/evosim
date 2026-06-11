---
status:        shipped
owner:         adamg
last_updated:  2026-06-05
okay_to_delete: false
long_lived:    false
owning_docs:
  - architecture/simulation-core.md
  - architecture/species.md
  - architecture/biome.md
  - architecture/index.md
  - decisions/app-shell.md
  - decisions/perf.md
  - decisions/index.md
  - _meta/ownership.json
  - _meta/manifest.md
---

# Docs structural restructure (post-review)

## Mission

The `/review-docs` editorial pass found the tree's bones are sound but four
structural problems are accruing debt: (1) pervasive changelog/version
framing across architecture docs ("v2.0 Wave 2b", "v1.10:", "previously…")
instead of rewrite-in-place; (2) `simulation-core.md` has become a ~4.8k-token
junk drawer that owns three subsystems at once; (3) `app-shell` has an
architecture doc and ownership concept but no decisions home; (4) the project
is heading deep into sim-perf with no perf decisions home and durable bench
evidence trapped in soon-to-be-deleted plans. Plus the v2 `app/` monorepo
migration left stale `web/` and "crate at repo root" paths behind. Done = the
tree reads as a current-state snapshot a fresh chat can navigate without plan
history, with clean ownership and altitude.

## Scope

**In scope:** doc structure, altitude, ownership, routing, and the path-drift
from the `app/` migration. Creating `architecture/species.md`,
`architecture/biome.md`, `decisions/app-shell.md`, `decisions/perf.md`.
Migrating durable plan context into architecture/decisions, then retiring the
shipped plans backlog and the stale `recon/` artifacts.

**Out of scope:** re-verifying every code claim against source (that's
`check-docs-consistency-some`); any code change. The in-flight
`v2.0.8-grid-rebuild-perf.md` plan and the two long-lived plans stay.

## Approach

Dependency-ordered waves. Shared "registry" files (ownership.json, manifest,
the indexes) are each owned by exactly one wave to avoid edit conflicts.

**Wave 1 (parallel — disjoint file sets):**
- **1A — split simulation-core.** Extract `architecture/species.md` (registry,
  N-anchor seeding, Mate/crossover, same-species gate) and
  `architecture/biome.md` (blob generator, movement penalties, NN modulation).
  Rewrite `simulation-core.md` in place to ~1.5–2k tokens, strip its changelog
  framing, replace the `constants.rs`/`grass.rs` field-dump transcription with
  `path → symbol` pointers. Update `_meta/ownership.json` (add `species`,
  `biome` concepts), `architecture/index.md` (two new rows + authoring-rules
  See-also), `_meta/manifest.md` (change-to-doc rows + decisions-domains slot).
  *Owns:* simulation-core.md, species.md, biome.md, ownership.json,
  architecture/index.md, manifest.md.
- **1B — decisions restructure.** Create `decisions/app-shell.md` (migrate the
  four app-shell entries out of `cross-cutting.md`); create `decisions/perf.md`
  (migrate the s0 bench-attribution data + the S2R snapshot-worker NO-GO
  rationale + grass_step cadence deferral from the plan bodies). Fix
  finding-7 nits: `render.md` `**Decision (b)**` → `**Decision**`; dedup
  `build.md` See-also; move code-paths out of `sim.md` `Applies to` into
  `Code anchors`; refresh `decisions/index.md` routing for the v2.0 entries.
  *Owns:* decisions/app-shell.md, perf.md, cross-cutting.md, sim.md, render.md,
  build.md, decisions/index.md.
- **1C — path drift.** Fix the `app/` migration drift in the procedural +
  top-level docs: `repository-layout.md` top-level table, `overview.md` "crate
  at repo root", bare `web/` anchors in `coding-style.md`,
  `testing-how-to.md`, `repo-rules.md`, `orchestrating.md`. Add the missing
  "when does this apply" header to `repo-rules.md`; dedup `orchestrating.md`
  Living-notes. *Owns:* overview.md, repository-layout.md, agent-context/*.

**Wave 2 (parallel — one agent per architecture doc):** strip changelog
framing + transcription + stale `web/` paths from the remaining architecture
docs: `app-shell.md`, `render-pipeline.md`, `profiler.md`, `worker-runtime.md`,
`shared-memory-and-protocol.md`, `build-and-deploy.md`, `testing.md`.
Cross-refs to the new `species.md`/`biome.md` are wired here.

**Wave 3 (after Wave 1B):** plans cleanup — verify durable content landed,
then migrate-then-delete the shipped backlog, delete `recon/`, fix the
incomplete frontmatter. Keep `index.md`, `perf-optimization-ideas.md`,
`v2.0.5-bench-recipe.md`, `v2.0.8-grid-rebuild-perf.md`, and this plan.

**Wave 4:** validation — relative links resolve, no surviving bare `web/src`
anchors, ownership.json matches the doc set, indexes route to every doc.

## Exit gate

- `architecture/species.md` and `architecture/biome.md` exist; `simulation-core.md`
  is ≤ ~2k tokens and carries no `vX.Y` / `Wave N` / "previously" framing.
- `decisions/app-shell.md` and `decisions/perf.md` exist; `decisions-domains`
  slot in the manifest lists them; `decisions/index.md` routes to them.
- No architecture doc contains version-flavored framing (grep for
  `Wave |v2\.0|v1\.1|previously|was removed` returns only legitimate prose).
- No doc references a bare `web/src` / `web/wasm` / repo-root `Cargo.toml`.
- `docs/plans/` contains only: this plan, `index.md`, the two long-lived
  plans, `v2.0.8-grid-rebuild-perf.md`. `recon/` is gone.
- Every relative doc link resolves (Wave 4 check).

## Migration notes (shipped 2026-06-05)

**New docs created:** `architecture/species.md`, `architecture/biome.md`
(split out of `simulation-core.md`, which dropped from ~4.8k→~2.5k tokens);
`decisions/app-shell.md`, `decisions/perf.md`; `plans/open-bugs.md`.

**Ownership / routing:** `_meta/ownership.json` gained `species` + `biome`
concepts (12 total); `_meta/manifest.md` `decisions-domains` slot gained
`app-shell` + `perf`, and the change-to-doc table now routes species/biome and
the renamed code paths; `architecture/index.md` + `decisions/index.md` route to
the new docs.

**Decisions migrated:** the four app-shell entries moved from `cross-cutting.md`
→ `decisions/app-shell.md`; grass-scatter bench attribution, the v2.0.7
snapshot-worker NO-GO + unpark bar, and the grass_step cadence deferral moved
from plan bodies → `decisions/perf.md`.

**Altitude:** all architecture docs stripped of changelog/version framing;
decisions headings stripped of `(v2.0 Wave X)` date-suffixes; transcription
(constant dumps, the simLoop body, duplicated SoA/FlashTag/camera-lane tables)
replaced with `path → symbol` pointers.

**Code-anchor reconciliation (drift the review didn't scope):** the web shell
moved flat→subdirs (`sim/`, `render/`) and three Rust modules became
directories (`brain/`, `grass/`, `wasm_api/` → `mod.rs`). Every canonical doc's
anchors + `repository-layout.md` were reconciled to the real layout. `plans/`
anchors were intentionally left (transient).

**Plans retired:** 15 shipped/superseded plans deleted (3 already
`okay_to_delete`, the v2.0.5/6/7 missions + grass-perf trilogy + closeouts +
recon snapshots). 7 unconfirmed open bugs from `grass-stage3-review.md` were
preserved in `plans/open-bugs.md` (`long_lived`) before deletion — **these
still need triage**. Kept: `perf-optimization-ideas.md`, `v2.0.5-bench-recipe.md`
(long-lived), `v2.0.8-grid-rebuild-perf.md` (active).

**Validation:** all relative doc links resolve; no stale code anchors outside
`plans/`; `ownership.json` valid.

## See also

- [`index.md`](index.md) — where live plans land.
- [`~/agent-docs/v1/plan-lifecycle.md`](~/agent-docs/v1/plan-lifecycle.md)
- `~/agent-docs/v1/rules/authoring-rules.md` — the standards this
  restructure is bringing the tree back to.
