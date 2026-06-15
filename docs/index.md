# evosim documentation index

This tree is the canonical, current-state snapshot of the project. It is
optimized for LLM consumption: start small, route by task, and load only
the subtree that matches the work. It follows **agent-docs v1** — the
generic doc-authoring rules and the workflow/maintenance commands live in
the global kit (`~/agent-docs/v1/` and `~/.claude/skills/`); this
tree holds only what's specific to **this** app.

If you are a fresh AI chat: run the **`/fresh-chat`** skill. It routes you
here, then to [`overview.md`](overview.md), then to the smallest matching
subtree index.

## How to use this tree

- **System overview** lives in [`overview.md`](overview.md). Read it at
  chat start for the shape of the app.
- **File inventory** lives in [`repository-layout.md`](repository-layout.md).
  Read it only when you need to find where code lives.
- **Ownership** is data in [`_meta/ownership.json`](_meta/ownership.json)
  (concept → owner doc); [`ownership.md`](ownership.md) explains how to
  use it. Consult it when two docs could own the same fact.
- **App bindings** for the global kit live in
  [`_meta/manifest.md`](_meta/manifest.md) — `code_root`, the change→doc
  table, drift gates, decisions domains, drift-verification surfaces.
- **Architecture** docs describe **what currently IS**. Route through
  [`architecture/index.md`](architecture/index.md).
- **Decisions** docs explain **why current design choices exist**. Route
  through [`decisions/index.md`](decisions/index.md).
- **Agent-context** docs are procedural: "when working on X, do Y, don't
  do Z." Each links up to its generic global rule. Route through
  [`agent-context/index.md`](agent-context/index.md).
- **Plans** are working coordination docs for multi-step work. Route
  through [`plans/index.md`](plans/index.md).

> The maintenance/chat commands (`/fresh-chat`, `/grand-orchestrator`,
> `/wrap-up-current-chat`, `/clear-plans`, `/fix-docs-drift-all`,
> `/check-docs-consistency-some`, `/review-docs`) are global skills, not
> in-repo files. There is no `prompts/` tree anymore.

## Global routing

| Need | Read |
|---|---|
| System at a glance | [`overview.md`](overview.md) |
| Where files live | [`repository-layout.md`](repository-layout.md) |
| Current subsystem facts | [`architecture/index.md`](architecture/index.md) |
| Design rationale | [`decisions/index.md`](decisions/index.md) |
| Workflow / commands / code-edit guardrails | [`agent-context/index.md`](agent-context/index.md) |
| App bindings for the global kit | [`_meta/manifest.md`](_meta/manifest.md) |
| Plan status rules or active plans | [`plans/index.md`](plans/index.md) |
| Canonical owner for a concept | [`_meta/ownership.json`](_meta/ownership.json) |
| Building / running the app, rebuilding wasm, dev loop | [`agent-context/dev-loop.md`](agent-context/dev-loop.md) |
| Main-thread UI shell (DOM, right-rail tabs, Settings stage-then-apply) | [`architecture/app-shell.md`](architecture/app-shell.md) |

## See also

- [`overview.md`](overview.md)
- [`repository-layout.md`](repository-layout.md)
- [`ownership.md`](ownership.md)
