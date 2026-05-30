# Plan Template

Copy this skeleton when creating a new mission plan.

```md
---
status: draft
owner: mixed
last_updated: YYYY-MM-DD
okay_to_delete: false
long_lived: false
owning_docs:
  - docs/architecture/<doc>.md
  - docs/decisions/<doc>.md
---

# Mission: <name>

## Goal

<One paragraph describing the user-visible or maintainer-visible outcome.>

## Scope

- <Included work>
- <Included work>

## Non-goals

- <Explicitly excluded work>

## Plan

1. <Wave or step>
2. <Wave or step>
3. <Wave or step>

## Verification

- <Commands, checks, screenshots, or review criteria>

## Status

- <Current progress, blockers, or handoff notes>
```

For a long-lived plan, add a short `## Long-lived purpose` section after
`## Goal`. Do not add that section to ordinary implementation plans.

## See also

- [`index.md`](index.md) — plan lifecycle and status metadata rules.
