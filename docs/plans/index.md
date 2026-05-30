# Plans

Working mission docs for multi-step changes.

## What belongs here

Plans are coordination artifacts: scope, waves, checklists, status, and
open questions for work that has not fully landed. They are not canonical
architecture or decision references. When code lands, update the owning
`architecture/` and `decisions/` docs so the current-state snapshot stays
accurate.

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

## See also

- [`template.md`](template.md) — copy-paste skeleton for new plans.
- [`../index.md`](../index.md) — global docs router.
- [`../ownership.md`](../ownership.md) — documentation ownership map.
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
  — doc update rules when plans ship.
