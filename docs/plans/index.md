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

- [`v2.0.3-grass-lod-pyramid.md`](v2.0.3-grass-lod-pyramid.md) — A u8
  box-filter grass mip pyramid (clipmap) serving render LOD, snapshot, and
  scale-invariant NN sensing from one structure. Removes the 1B-scale output
  walls (upload bandwidth, texture size, density-dependent sense cost).
  **Draft** — depends on v2.0.2 u8 density; absorbs v2.0.2's Phase 2.
- [`v2.0.2-grass-scatter.md`](v2.0.2-grass-scatter.md) — Replace the grass
  separable-blur with stochastic u8 scatter (lossy relaxed-atomic writes,
  per-tile frozen source, ring-1/2/3 spread bias, decay sliders, tile-based
  active set). The propagation perf fix at default scale; 1B-scale
  render/snapshot work split to v2.0.3. Targets ~50–100× on grass compute,
  ~20× on the tick. **Draft** — design agreed, supersedes the perf handoff.
- [`v2.0.2-grass-perf-handoff.md`](v2.0.2-grass-perf-handoff.md) —
  Earlier brief that proposed *materializing* the blur. **Superseded** by
  the scatter plan above (we drop the blur instead); kept for context.
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
