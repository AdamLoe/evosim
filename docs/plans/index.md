# Plans

Working mission docs for multi-step changes.

**Plans are scratch and get deleted post-ship.** `long_lived: true` is the
explicit exception, not the norm. Anything in a plan that needs to survive
launch — current-state architecture, design rationale, invariants —
**must** live in `architecture/` or `decisions/`. If it's only in a plan,
treat it as already gone.

## What belongs here

Plans are coordination artifacts: scope, waves, checklists, status, and
open questions for work that has not fully landed. They are not canonical
architecture or decision references. When code lands, rewrite the owning
`architecture/` and `decisions/` docs in place so the current-state
snapshot stays accurate — then the plan can be deleted.

Do not add lifecycle boilerplate to the top of ordinary plans. This file
owns the lifecycle rules. Individual plans should carry compact metadata
at the top, then start the mission content.

Only call out long-lived plan status in prose when the plan is intentionally
long-lived, for example a roadmap, benchmark campaign, or umbrella project
that will remain useful after individual implementation waves ship.

## Status Metadata

Start each plan with YAML frontmatter:

```yaml
---
status: draft | active | blocked | shipped | parked
owner: user | codex | mixed
last_updated: YYYY-MM-DD
okay_to_delete: false
long_lived: false
owning_docs:
  - docs/architecture/<doc>.md
  - docs/decisions/<doc>.md
---
```

- `status`: current state of the plan.
- `owner`: who is expected to drive the next update.
- `last_updated`: date of the latest meaningful plan edit.
- `okay_to_delete`: `true` only after the owning architecture and decision
  docs have been updated and the plan has no remaining coordination value.
  The user decides when to delete it.
- `long_lived`: `true` only for plans that are meant to stay useful after
  their first implementation pass.
- `owning_docs`: docs that must be rewritten in place as the work lands.

## Creating A Plan

Use [`template.md`](template.md) when creating a new plan. An agent
reviewing or updating an existing plan usually only needs this index plus
the plan itself.

## Active plans

- [`v2.0.4-grass-tuning-perf.md`](v2.0.4-grass-tuning-perf.md) — **Active wave, ready
  for orchestration.** LOD config (budget=4096 + `lod_bias` + aspect fix),
  `grass_size` slider, per-cell scatter perf (alloc/instrumentation/RNG-fusion/
  geometric-skip), density-weighted "fertility" spread, far-grass multi-band NN
  sight. Decisions resolved; carries the **execution hub** (S0 recon-first +
  fallback, 7 streams, DAG, gates, worktree map).
- [`perf-optimization-ideas.md`](perf-optimization-ideas.md) — **long-lived** perf
  backlog (cadence/visibility gating, event-sampling, mip-skip throttle, far NN
  sight, deterministic-scatter). Menu of deferred ideas; never deleted.
- [`grass-perf-closeout.md`](grass-perf-closeout.md) — **Lead handoff / review surface**
  for the grass-perf effort. Code-complete + fully gated green (both feature sets, e2e
  11/11); durable facts migrated. Headlines the **ratification items** (threaded run-to-run
  non-reproducibility, blur deletion, feel-tune) + the toroidal wrap-seam limitation.
  **Review** — start here.
- [`grass-perf-hub.md`](grass-perf-hub.md) — **Orchestration hub**: the full build-first
  log, every decision, and the carried-forward debt (each item fixed/confirmed/deferred).
  **Review** — the detailed record behind the close-out.
- [`grass-perf-recon.md`](grass-perf-recon.md) · [`grass-perf-recon-stage2.md`](grass-perf-recon-stage2.md)
  · [`grass-stage3-review.md`](grass-stage3-review.md) — recon maps (Stage 1 / Stage 2) +
  the Stage-3 code review & perf numbers. Scratch; delete with the effort.
- [`v2.0.2-grass-scatter.md`](v2.0.2-grass-scatter.md) — Stage-1 design spec (stochastic u8
  scatter). **Shipped** — durable facts migrated to `architecture/` + `decisions/`;
  `okay_to_delete: true`.
- [`v2.0.3-grass-lod-pyramid.md`](v2.0.3-grass-lod-pyramid.md) — Stage-2 design spec (u8 LOD
  mip pyramid + windowed snapshot). **Shipped** — durable facts migrated; `okay_to_delete: true`.
- [`v2.0.1-mission.md`](v2.0.1-mission.md) — Patch-wave hub for v2.0.1:
  triage + fix the problems the lead surfaces against the shipped v2.0.0
  build. **Draft** — intake open, no streams launched yet.
- [`v2.0.0-mission.md`](v2.0.0-mission.md) — World, body, species:
  editable runtime `world_size` (default 8× linear / 64× area),
  optional toroidal wrap, 3 biomes, evolving body genome, opt-in
  species + sexual mating mode. **Shipped** on `feat/v2.0.0`; durable
  facts migrated to `architecture/` + `decisions/`.
- [`v2.0.0-decisions.md`](v2.0.0-decisions.md) — Companion rationale
  for v2.0.0: every choice settled in plan drafting, with the
  alternatives that were rejected. **Shipped**: rationale folded into
  `decisions/{sim,render,cross-cutting}.md`; `okay_to_delete: true`.
- [`v2-possible-next-steps.md`](v2-possible-next-steps.md) —
  **long-lived** backlog for the v2 family: deferred ideas, expected
  problems, and design directions discussed but not committed (brain
  inheritance under sexual reproduction, mating cold-start levers,
  the 1920² optimization pass, survey-scale visibility, dynamic
  species). Never deleted.

## See also

- [`template.md`](template.md) — copy-paste skeleton for new plans.
- [`../index.md`](../index.md) — global docs router.
- [`../ownership.md`](../ownership.md) — documentation ownership map.
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
  — doc update rules when plans ship.
