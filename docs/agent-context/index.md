# Agent context index

Procedural docs: when working on X, do Y, don't do Z.

## How to use this folder

Load only the procedural doc that matches the situation you're in. Each
doc opens with a "when does this apply" framing so you can decide
quickly whether to read it.

Under **agent-docs v1**, the generic workflow discipline lives in the
global kit (`~/agent-docs/v1/rules/`); the in-repo docs below
hold what's specific to **evosim** and link up to the matching global
rule. App-independent commands and templates are exposed as global
**skills** (`/fresh-chat`, `/grand-orchestrator`, `/fix-docs-drift-all`,
`/check-docs-consistency-some`, `/wrap-up-current-chat`), not as in-repo
prompt files.

## Routing

| Situation | Read | Generic rule (global kit) |
|---|---|---|
| Editing Rust in `app/crates/evosim/src/` or TypeScript in `app/web/` | [`coding-style.md`](coding-style.md) | `rules/coding-style.md` |
| Running the app, rebuilding wasm, checking the dev server, hard-reloading | [`dev-loop.md`](dev-loop.md) | — (app-specific) |
| Running or adding tests, triaging test failures | [`testing-how-to.md`](testing-how-to.md) | — (app-specific) |
| Git commits, staging, drift gates, things to never do | [`repo-rules.md`](repo-rules.md) | `rules/repo-rules.md` |
| Updating docs after a code change | [`maintaining-docs.md`](maintaining-docs.md) | `rules/authoring-rules.md` |
| Orchestrating multi-stream work via sub-agents | [`orchestrating.md`](orchestrating.md) | `rules/orchestrating.md` |
| Creating, updating, or shipping a plan | [`../plans/index.md`](../plans/index.md) (landing); [`~/agent-docs/v1/plan-lifecycle.md`](~/agent-docs/v1/plan-lifecycle.md) (lifecycle) + [`~/agent-docs/v1/plan-template.md`](~/agent-docs/v1/plan-template.md) (skeleton); [`maintaining-docs.md`](maintaining-docs.md) | `rules/authoring-rules.md` §workflow |

For a multi-stream effort driven via sub-agents (audit → plan →
implement → verify → doc-migrate), bootstrap with the `/grand-orchestrator`
skill and read `orchestrating.md`. A single focused change does not need it.

## See also

- [`../index.md`](../index.md) — global docs router.
- [`../architecture/index.md`](../architecture/index.md) — the
  current-state facts these procedural docs reference.
- [`../_meta/manifest.md`](../_meta/manifest.md) — app slot-data the
  global rules plug into.
- [`../ownership.md`](../ownership.md) — ownership map for static facts.
