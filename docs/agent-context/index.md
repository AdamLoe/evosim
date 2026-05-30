# Agent context index

Procedural routing for agents working in this repo.

## When does this apply

Read this after `docs/index.md` and `docs/overview.md` when you are about
to edit code, run commands, start the app, test, commit, or update docs.
This file owns routing among the procedural docs; the child docs own the
actual instructions.

## How to use this layer

Load only the procedural doc that matches the task. If the task also
touches a subsystem, load the relevant architecture doc from
`docs/architecture/index.md` as the static source of truth.

| Situation | Read |
|---|---|
| Editing Rust in `src/` or TypeScript in `web/` | [`coding-style.md`](coding-style.md) |
| Running the app, rebuilding wasm, checking the dev server, hard-reloading | [`dev-loop.md`](dev-loop.md) |
| Running or adding tests, triaging test failures | [`testing-how-to.md`](testing-how-to.md) |
| Making a commit, staging files, touching lockfiles/generated outputs/settings | [`repo-rules.md`](repo-rules.md) |
| Editing docs, shipping a plan, deciding which doc owns a fact | [`maintaining-docs.md`](maintaining-docs.md) |
| Creating a mission plan | [`../plans/index.md`](../plans/index.md) + [`../plans/template.md`](../plans/template.md) |
| Updating or shipping a mission plan | [`../plans/index.md`](../plans/index.md) + [`maintaining-docs.md`](maintaining-docs.md) |

## Routing rules

- For any `src/` or `web/` edit, read `coding-style.md` before editing.
- For browser iteration after a Rust change, read both `coding-style.md`
  and `dev-loop.md`.
- For test work, read `testing-how-to.md`; if the test describes system
  coverage rather than command procedure, also read
  `../architecture/testing.md`.
- For docs work, read `maintaining-docs.md`; if a command recipe appears
  in an agent-context doc, keep it in sync with its canonical architecture
  owner.
- For plan review or updates, read `../plans/index.md`; ordinary plans
  use status metadata instead of lifecycle prose at the top. Open
  `../plans/template.md` only when creating a new plan.
- For file discovery, use `../repository-layout.md`; this agent-context
  layer is about workflow, not source-tree inventory.

## See also

- [`../index.md`](../index.md) — global docs router.
- [`../ownership.md`](../ownership.md) — ownership map for static facts.
- [`../overview.md`](../overview.md) — system shape.
- [`../repository-layout.md`](../repository-layout.md) — file inventory.
