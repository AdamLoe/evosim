# Plans

Working coordination docs for multi-step work on evosim. Plans are
**not** canonical architecture: once work ships, the relevant architecture
and decisions docs get updated and the plan is deleted.

The **lifecycle rules, status-frontmatter spec, and ship-time migration
workflow are generic** (agent-docs v1) and live in the kit:
[`~/agent-docs/v1/plan-lifecycle.md`](~/agent-docs/v1/plan-lifecycle.md).
New plans start from the kit skeleton:
[`~/agent-docs/v1/plan-template.md`](~/agent-docs/v1/plan-template.md).

## What lives here

Active and recently-shipped plans live in this directory alongside this
landing doc. This index does **not** maintain an inventory of active plans
(it would rot the moment one is added) — list the directory to see what's
live:

```
ls docs/plans/
```

The long-lived plans (frontmatter `long_lived: true`) are:

- **`open-bugs.md`** — triage list of open or unconfirmed bugs collected
  during plans-cleanup; never deleted until all items are resolved or moved.

Everything else should reach `shipped + okay_to_delete: true` and be deleted.

## Routing

| Need | Read |
|---|---|
| Create a new plan | [`~/agent-docs/v1/plan-template.md`](~/agent-docs/v1/plan-template.md) |
| Plan lifecycle / status-metadata rules | [`~/agent-docs/v1/plan-lifecycle.md`](~/agent-docs/v1/plan-lifecycle.md) |
| The doc-update workflow a shipped plan triggers | [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md) |

## See also

- [`~/agent-docs/v1/plan-lifecycle.md`](~/agent-docs/v1/plan-lifecycle.md)
  — lifecycle and status metadata (generic).
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
  — the rules a shipped plan must satisfy.
- [`../index.md`](../index.md) — global router.
