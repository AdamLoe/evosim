# evosim documentation index

This tree is the canonical, current-state snapshot of the project. It is
optimized for LLM consumption: start small, route by task, and load only
the subtree that matches the work.

If you are a fresh AI chat: read [`prompts/fresh-chat.md`](prompts/fresh-chat.md)
first. It will route you here, then to [`overview.md`](overview.md), then
to the smallest matching subtree index.

## How to use this tree

- **System overview** lives in [`overview.md`](overview.md). Read it at
  chat start for the shape of the app.
- **File inventory** lives in [`repository-layout.md`](repository-layout.md).
  Read it only when you need to find where code lives.
- **Ownership rules** live in [`ownership.md`](ownership.md). Read it when
  two docs could own the same fact, or when updating docs after a change.
- **Architecture** docs describe **what currently IS**. Route through
  [`architecture/index.md`](architecture/index.md).
- **Decisions** docs explain **why current design choices exist**. Route
  through [`decisions/index.md`](decisions/index.md).
- **Agent-context** docs are procedural: "when working on X, do Y, don't
  do Z." Route through [`agent-context/index.md`](agent-context/index.md).
- **Prompts** are verbatim templates or thin pointers. Route through
  [`prompts/index.md`](prompts/index.md). Prompts own no facts.
- **Plans** are working coordination docs for multi-step work. Route
  through [`plans/index.md`](plans/index.md).

## Global routing

| Need | Read |
|---|---|
| System at a glance | [`overview.md`](overview.md) |
| Where files live | [`repository-layout.md`](repository-layout.md) |
| Current subsystem facts | [`architecture/index.md`](architecture/index.md) |
| Design rationale | [`decisions/index.md`](decisions/index.md) |
| Workflow / commands / code-edit guardrails | [`agent-context/index.md`](agent-context/index.md) |
| Prompt templates | [`prompts/index.md`](prompts/index.md) |
| Plan status rules or active plans | [`plans/index.md`](plans/index.md) |
| Canonical owner for a concept | [`ownership.md`](ownership.md) |
| Main-thread UI shell (DOM, right-rail tabs, Settings stage-then-apply) | [`architecture/app-shell.md`](architecture/app-shell.md) |

## See also

- [`overview.md`](overview.md)
- [`repository-layout.md`](repository-layout.md)
- [`ownership.md`](ownership.md)
