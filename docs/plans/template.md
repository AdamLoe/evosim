# Plan Template

Copy this skeleton when creating a new mission plan.

**Before you start filling this in: plans are scratch and get deleted
post-ship (unless explicitly `long_lived: true`). Anything that needs to
survive past launch — current-state architecture, design rationale —
must end up in `architecture/` or `decisions/`, not here. The
`owning_docs:` field below is the list of files you owe an update to
when this plan ships. The plan body is for coordination only.**

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
<Remember: durable context goes in owning_docs, not in this body.>

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
