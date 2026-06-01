# Orchestrating multi-stream work on evosim

## When does this apply

You are acting as the **orchestrator** of a large, multi-stream effort
(audit → plan → implement → verify → doc-migrate) across the evosim
codebase — Rust sim core in `src/`, the TypeScript/WebGL web shell in
`web/`, the worker bridge, and the docs tree — delegating to sub-agents
while keeping your own context lean enough to run for hours. If you're
doing a single focused change yourself, you don't need this doc — just
follow [`coding-style.md`](coding-style.md) and [`repo-rules.md`](repo-rules.md).

Bootstrap an orchestration chat with [`../prompts/fresh-orchestrator.md`](../prompts/fresh-orchestrator.md);
it routes you back here.

## What to do

### Your actual job

1. **Hold the map, not the territory.** Keep a hub plan doc (a mission
   plan under [`../plans/`](../plans/index.md)) with a phase tracker, a
   decisions log, and an open-questions list. Route work; don't do it.
   Your scarcest resource is your own context window — spend it on
   sequencing and judgment, not on file reads and build output.
2. **Verify outcomes, not steps.** Trust an agent's "build green / tests
   pass" when it pasted the command output; spot-check only the high-risk
   bits — and spot-checking means *reading* the pasted output, not
   re-running the gate yourself. A stream is not **done** until: files
   changed are listed, the gate command + its result are pasted, known
   deferrals are named, and **you have recorded the observed outcome in
   the hub doc** (not the agent's optimism).
3. **Keep notes a stranger could resume from.** Every decision, every
   "why", every deferral goes in the hub doc — assume your context will be
   wiped. Durable facts migrate to `architecture/` and `decisions/` at
   ship time per [`maintaining-docs.md`](maintaining-docs.md); the plan is
   scratch and gets deleted.

### How to delegate

Delegate three kinds of work:

- **Investigate** — read-only, returns distilled facts (not raw file
  dumps). Use this instead of reading big source files (`src/world.rs`,
  `src/wasm_api.rs`, `web/src/render-gl.ts`) yourself.
- **Implement** — edits code and self-verifies. Default model: the
  faster mid-tier model (Sonnet/Haiku).
- **Plan / red-team** — returns a plan or a critique. Default model: the
  strongest model (Opus), for planning, audit, and correctness-critical
  work (SAB layout, tick step order, the worker control path).

Also delegate **verification** — don't run the full gate inline; have an
agent run it and paste the result.

### Resuming vs. spawning agents

- **You can't redirect an agent mid-run.** Get the dispatch prompt right
  up front. If a decision changes while an agent is in flight, queue a
  follow-up agent rather than trying to steer the running one.
- **Once an agent returns, prefer resuming it over cold-spawning** when
  your next task overlaps its existing context — it saves both your
  context and the agent's ramp-up. But **don't resume an agent whose
  context is bloated or far from the new task**; a fresh agent is cleaner
  and cheaper there. Resume for continuity; spawn for a clean slate.
- **Background agents are the workhorse for long streams** — launch and
  get re-invoked on completion. Don't poll, and don't read their
  transcript files (they overflow context).

### Standard preamble for every sub-agent prompt

Bake these in so you stop re-litigating them:

- **Name the authoritative source of truth.** The Rust sim core owns the
  wire format and every invariant; the TS shell consumes it. Verify SAB
  layout, strides, slider names, and report shapes against the Rust, never
  against a TS guess or a doc. When sources disagree, this is the
  precedence to state in the prompt:

  ```
  src/ Rust (constants.rs, wasm_api.rs SAB layout) > generated web/wasm/*.d.ts bindings > web/ TS consumers > tests > docs
  ```

- **wasm rebuilds must pass `--features threads`** — a plain `wasm-pack`
  build silently single-threads the sim and invalidates any perf or
  parallel-equivalence check. State this in any prompt that rebuilds wasm
  (see [`dev-loop.md`](dev-loop.md)).
- **New tests go in their own per-feature test file/module**, never a
  shared one — two agents appending to the same `#[cfg(test)] mod tests`
  or the same `sim-bridge.spec.ts` is a merge hazard.
- **State the cheapest sufficient gate** — the exact narrow command for
  this slice (e.g. `cargo test --lib --features threads` for one Rust
  module, or `cd web && pnpm typecheck` for a TS-only change) — and
  require the agent to paste its result. **Explicitly forbid the full
  suite and the Playwright e2e per stream**; those run once, as the
  consolidated end gate, not as a per-stream check.
- **Scope fences:** name the files/dirs the agent must NOT touch (the ones
  another in-flight agent owns).
- **Report format: tight and self-contained** (especially background
  agents) — the key finding, files changed, gate result, anything
  deferred. No big diffs.
- **Honesty clause:** if a decision turns out infeasible, implement the
  documented fallback and flag it — don't fake a value or silently
  descope.

Default dispatch block — fill every field, paste into the agent prompt:

```
Task:                       (the one outcome this agent owns)
Scope:                      (what's in bounds; what's explicitly out)
Files/dirs you MAY touch:
Files/dirs you must NOT touch:  (what another in-flight agent owns)
Authoritative source of truth:  (the Rust handler/const to verify against)
Gate:                       (exact narrow command to run; paste its result)
Report back with:           (key finding, files changed, gate result, deferrals)
Fallback if infeasible:     (the documented alternative; flag it, don't fake)
```

### Sequencing

- **One dev server, one wasm build.** Never run two `wasm-pack` builds
  racing the same `web/wasm/` output, and never two dev servers on the
  same port — they clobber each other. Light per-stream verifies
  (`cargo test`, `pnpm typecheck`) are fine; a browser boot/tick smoke and
  the Playwright e2e are a single consolidated final gate, not a per-stream
  check (see [`testing-how-to.md`](testing-how-to.md) and
  [`dev-loop.md`](dev-loop.md)).
- **Sequence by file overlap, parallelize by disjointness.** Two agents
  editing the same file — or a shared central module like
  `src/constants.rs`, `src/wasm_api.rs`, or `web/src/main.ts` — WILL
  collide. Map the files each stream touches before launching. Rust-core
  streams and `web/` TS streams almost always parallelize safely, **except**
  when a Rust change reshapes the SAB layout or a slider/report export — then
  the TS consumer must follow in the same wave or right after, never in
  parallel.

### When to involve the lead

- **Ask up front, batched**, when a decision changes *what you build* and
  you can't resolve it from code or precedent — a world/sim-rule change, a
  data-layout convention, a UX fork in the web shell.
- **Don't ask** about things with an obvious default or that you can
  verify yourself — pick it, log it in the decisions section, move on.
- **Second-guess deliberately:** if the lead's call has a real downside,
  say so once with the tradeoff. A single well-aimed pushback can change
  the design for the better; don't relitigate past one round.

### Bringing in second opinions

- For correctness-critical changes, spend a strong-model agent on a
  **red-team / second opinion** before or after implementing. This is the
  **exception, not a per-stream default** — a review agent is a full
  cold-context spawn, so don't pay that cost on routine streams.
- The single most valuable second opinion in a refactor that touches the
  sim is: **does it still tick correctly and deterministically?** Make
  cross-feature equivalence (`cargo test --lib` default vs
  `--features threads`, same seed → same output) the gate, and consider a
  dedicated correctness review of any change to the SAB layout
  (`wasm_api.rs`), the tick step order (the simulation core), the parallel
  NN forward, or the worker control path (the `Atomics.waitAsync`
  regression class — see [`testing-how-to.md`](testing-how-to.md)).

### Workflow skeleton

1. **Audit/recon** (one read-only agent) → findings doc on disk.
2. **Hub doc**: a mission plan with phases, a streams table, a decisions
   log, and a questions-for-lead list. The streams table is where you hold
   the map — use these columns so it stays a record of *observed* state,
   not plans:

   ```
   | Stream | Area | Status | Last observed fact | Next action | Blockers |
   ```
3. **Per-stream plan docs** — the strongest model for the meaty ones, or
   fold small streams into the hub.
4. **Implement in waves** grouped by file-disjointness; background agents.
5. **On each completion:** update the streams table + todos, spot-check
   the high-risk bit, decide the next wave. Keep it to a few tool calls.
6. **Final consolidated gate** (`cargo fmt`/`clippy` both feature sets,
   `cargo test --lib` both feature sets, `pnpm typecheck`/`pnpm build`, and
   the Playwright e2e — full list in
   [`testing-how-to.md`](testing-how-to.md)).
7. **Doc-migrate** durable facts into architecture/decisions; mark plans
   `okay_to_delete: true`. Hand the lead a review hub + questions.

## What NOT to do

- **Don't drift into implementer mode.** The moment you're reading source
  files, running build/grep loops, or hand-editing **source code** in your
  own context, stop and dispatch an agent. That work burns the context
  that lets you run long. There is no "it's only a quick edit" exception —
  that rationalization is exactly how the drift starts; delegate even the
  small ones. (Editing the hub doc, trackers, and your own plan notes is
  not implementer mode — that *is* your job.)
- **Don't gate every stream with the full suite.** Per-stream gates stay
  narrow (the relevant `cargo test --lib`/`pnpm typecheck` + the new test);
  the full clippy/fmt, both-feature test runs, the build, and the
  Playwright e2e run *once*, at the end. Re-gating each stream is the
  single biggest avoidable time sink in a multi-stream run.
- **Don't narrate state you haven't observed.** Update the tracker from
  agent results and disk truth, not optimism. Never mark a stream "done"
  before its agent reports.
- **Don't poll background agents or read their transcripts.** Wait for the
  completion re-invoke.
- **Don't assume a read-only (`Plan`/`Explore`-type) agent's output landed
  on disk.** It returns the plan inline; you (or a write-capable agent)
  must persist it.

## Living notes

Append-only field notes from real orchestration runs. Newest first. This
is the **one** section that may carry dated, concrete incidents — the body
above stays general; specifics that would rot live here. Extend it as you
learn.

- *No live orchestration-run incidents recorded yet.* Two seeded cautions
  from the codebase, to be replaced/augmented by real runs:
  - **The `--features threads` footgun.** A sub-agent that rebuilds wasm
    with a plain `wasm-pack` build will silently single-thread the sim;
    any perf or parallel-equivalence claim from that agent is void. Bake
    the flag into the dispatch prompt.
  - **SAB-layout changes cross the Rust↔TS seam.** A Rust change to the
    snapshot layout that isn't mirrored in the TS reader passes
    `cargo test` and `pnpm typecheck` independently yet breaks the live
    sim. Don't parallelize the producer and the consumer of a layout
    change — same wave, Rust first.

## See also

- [`index.md`](index.md) — agent-context router.
- [`../prompts/fresh-orchestrator.md`](../prompts/fresh-orchestrator.md) —
  the bootstrap prompt for an orchestration chat.
- [`maintaining-docs.md`](maintaining-docs.md) — the doc-migration rules
  step 7 of the workflow invokes.
- [`testing-how-to.md`](testing-how-to.md) — the gates and the
  consolidated final suite.
- [`dev-loop.md`](dev-loop.md) — wasm rebuild + dev server, and the
  `--features threads` requirement.
- [`../plans/index.md`](../plans/index.md) — plan lifecycle for the hub doc.
