# Maintaining docs

The authoring rules for `docs/`. Every architecture doc's `See also`
points here so an agent shipping a change knows which doc to update.

## When does this apply

You shipped a code change that touched any of the surfaces named in the
ownership map ([`../index.md`](../index.md)). Or you're orchestrating a
plan and need to update the snapshot. Or you found yourself wanting to
write a "v1.X introduced …" sentence into an architecture doc — stop
and read this first.

## The rules

1. **Every doc has explicit ownership.** The ownership map in
   [`../index.md`](../index.md) names one canonical owner per concept.
   Edit the owner. Non-owners may *reference* the concept by linking;
   they must not redefine it. If a non-owner doc starts to grow
   substantive content on a concept it doesn't own, that's drift —
   move the content to the owner and link back.

2. **Architecture docs describe what IS, not what changed.** No
   "v1.6 introduced…" / "Wave D added…" / "as of v1.7…" framing. When
   a subsystem changes, the architecture doc gets **rewritten in
   place** — the history of the change lives in the git log and (for
   the in-flight pass) in `plans/`. Decisions about *why* the new
   shape exists go in `decisions/<domain>.md`.

3. **Reference code by path; do not embed implementation code.
   Small interface snippets are allowed.** Code is authoritative for
   behaviour; docs are authoritative for what's where and why.
   Implementation excerpts rot the moment the code changes — banned.
   Interface snippets (a function signature, a discriminated union
   shape, a byte layout table) are contracts that change rarely and
   are useful inline.

4. **Each architecture doc is ~1–2k tokens, one file per subsystem.**
   Bigger burns context; smaller is a navigation tax. If a subsystem
   doc grows past ~2k tokens, split it.

5. **`decisions/` is sectioned by architecture domain, not by date.**
   Five files: `sim.md`, `render.md`, `profiler.md`, `build.md`,
   `cross-cutting.md`. Each entry has three **mandatory** fields:
   - `Decision` — one sentence stating what we believe.
   - `Why` — the reason. Even one line is fine.
   - `Applies to` — which architecture doc(s) this constrains.

   And four **optional** fields, included only when they add real
   value:
   - `Alternatives considered`
   - `Tradeoffs`
   - `Code anchors` — `path → symbol_name` (NOT `path:line` — line
     numbers drift; symbol names are grep-stable).
   - `Revisit when` — the trigger condition.

   **No `Date:` field** — the git log is the date authority.
   **Only carry forward decisions that still apply.** Superseded
   rationale stays in git; don't re-litigate.

6. **Cross-link liberally.** Each architecture doc has a `See also`
   section pointing to related architecture docs, the relevant
   `decisions/<domain>.md`, and any `agent-context/` doc with
   procedural overlap. **Every architecture doc must reference this
   doc (`agent-context/maintaining-docs.md`) in its `See also`** so a
   fresh agent always finds the maintenance rules.

7. **Plans live in `docs/plans/`. They are ephemeral.** When a plan
   ships, the implementer (or the orchestrator) **must update the
   relevant architecture doc(s) and `decisions/<domain>.md` to reflect
   the new current state** before the plan can be marked done. After
   that, the plan is free to be archived to `docs/plans/archive/` or
   deleted; nothing depends on it.

8. **No `CLAUDE.md`, no `AGENTS.md`, no global `.claude/` instructions
   about this project's architecture.** All such context goes in
   `docs/`. Settings unrelated to project knowledge (permissions,
   hooks) stay where they are.

## What changes trigger a doc update

This is the table to consult before declaring a commit "done".

| If you changed… | Update… |
|---|---|
| `src/world/*.rs` (tick step, NN, grass mechanic, SoA layout) | `architecture/simulation-core.md` (esp. tick step order, NN topology, code anchors) |
| `src/wasm_api.rs` (added/removed/renamed an export) | `architecture/simulation-core.md` (interaction-with-neighbours block) AND `architecture/shared-memory-and-protocol.md` if it's part of the message protocol |
| `src/constants.rs` (any value that appears in a doc, esp. `MAX_POP_FOR_SIM`, `GRASS_CELL_COUNT`, `NN_*`, `FOUNDER_COUNT_DEFAULT`) | Wherever the constant is quoted; `decisions/cross-cutting.md` for `MAX_POP_FOR_SIM` |
| `src/profiler.rs` API | `architecture/profiler.md`, `decisions/profiler.md` |
| `src/grass.rs` step shape or per-row bitset semantics | `architecture/simulation-core.md`, `architecture/profiler.md` (grass_step tree) |
| `web/src/sim-worker.ts` loop shape, pacing, message handling | `architecture/worker-runtime.md`, possibly `decisions/sim.md` |
| `web/src/sim-bridge.ts` (any message kind, SAB constant, layout helper) | `architecture/shared-memory-and-protocol.md` (definitive) |
| `web/src/main.ts` (boot, render-loop shape, restart) | `architecture/worker-runtime.md`, `architecture/render-pipeline.md` |
| `web/src/render-gl.ts` (GL program, instance pack, frustum cull, span names) | `architecture/render-pipeline.md`, `architecture/profiler.md` if span names change |
| `web/src/perf.ts` API | `architecture/profiler.md` |
| `web/src/widgets/perf-panel.ts` (`TREE_ORDER`, poll cadence) | `architecture/profiler.md`, `decisions/profiler.md` |
| `web/src/widgets/devpanel.ts` (`currentSliderState`, slider list) | `architecture/worker-runtime.md` (boot payload section) |
| `web/vite.config.ts`, `web/public/_headers`, `Cargo.toml` profile, `.cargo/config.toml` | `architecture/build-and-deploy.md`, `decisions/build.md` |
| `web/tests/e2e/*` | `architecture/testing.md`, `agent-context/testing-how-to.md` |
| `web/tests/README.md` | `architecture/testing.md`, `agent-context/testing-how-to.md` (kept in sync) |
| Repository directory layout (new dir, renamed dir) | `repository-layout.md` |
| Anything that introduces a new cross-language constant | `decisions/cross-cutting.md` (add a "constant duplicated in X + Y, asserted at Z" entry) |

If your change touches a surface and you're not sure which doc owns
it, consult the ownership map in [`../index.md`](../index.md).

## Workflow when shipping a plan

1. Make the code change.
2. Run the per-commit gates (see [`testing-how-to.md`](testing-how-to.md)).
3. **Update the architecture doc(s)** that own the touched surfaces.
   Rewrite in place — don't append a "v1.X" section.
4. **Update `decisions/<domain>.md`** if the change introduces a new
   decision worth recording. Use the three mandatory + four optional
   fields; route to the right domain file.
5. Commit code + doc updates together (or as two adjacent commits in
   the same PR — whichever is cleaner).
6. Archive or delete the plan from `docs/plans/`.

## Anti-patterns to refuse

- **"Just add a quick note in DECISIONS.md".** That file is gone.
  Route the entry to the right `decisions/<domain>.md`.
- **"Add a `## v1.X` section to the architecture doc".** Rewrite the
  doc in place. The version-flavoured framing is what made the old
  tree unreadable.
- **"It's faster to put the wasm-pack incantation in this prompt
  too".** Prompts own no facts. Link to
  [`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md).
- **"Let me drop a one-liner about this in CLAUDE.md".** No CLAUDE.md.
  Put it in `agent-context/<doc>` if it's procedural, in
  `architecture/<doc>` if it's a fact about the system, in
  `decisions/<domain>` if it's a choice we made.
- **"This decision is superseded but I'll keep the old text for
  context".** The git log is the context. Delete superseded decisions
  from `decisions/`; the rationale already lives in the commit that
  changed the code.

## See also

- [`../index.md`](../index.md) (ownership map)
- [`coding-style.md`](coding-style.md)
- [`repo-rules.md`](repo-rules.md)
- [`../prompts/check-docs.md`](../prompts/check-docs.md) (the periodic
  drift-check prompt)
