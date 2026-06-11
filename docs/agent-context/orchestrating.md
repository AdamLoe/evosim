# Orchestrating multi-stream work — evosim

Generic orchestration discipline (hold the map, delegate by type, dispatch
block, sequencing, workflow skeleton, what NOT to do) lives in the global
kit at `~/agent-docs/v1/rules/orchestrating.md`. Read that first.

To bootstrap an orchestration chat for this repo, use the
`/fresh-orchestrator` skill.

## When does this apply

You are acting as the **orchestrator** of a large, multi-stream effort
(audit → plan → implement → verify → doc-migrate) across the evosim
codebase — Rust sim core in `app/crates/evosim/src/`, the TypeScript/WebGL web shell in
`app/web/`, the worker bridge, and the docs tree — delegating to sub-agents
while keeping your own context lean enough to run for hours. If you're
doing a single focused change yourself, you don't need this doc — just
follow [`coding-style.md`](coding-style.md) and [`repo-rules.md`](repo-rules.md).

## App-specific notes

**Scarce shared resource: one wasm build, one dev server.** Never run two
`wasm-pack` builds racing the same `app/web/wasm/` output, and never two
dev servers on the same port — they clobber each other. Light per-stream
verifies (`cargo test`, `pnpm typecheck`) are fine; a browser boot/tick
smoke and the Playwright e2e are a single consolidated final gate, not a
per-stream check (see [`testing-how-to.md`](testing-how-to.md) and
[`dev-loop.md`](dev-loop.md)).

**The `--features threads` footgun.** Any sub-agent that rebuilds wasm
with a plain `wasm-pack build` (no `--features threads`) will silently
single-thread the sim; any perf or parallel-equivalence claim from that
agent is void. Bake `--features threads` into the dispatch prompt for
every stream that touches Rust or rebuilds wasm.

**SAB-layout changes cross the Rust↔TS seam.** A Rust change to the
snapshot layout that isn't mirrored in the TS reader passes `cargo test`
and `pnpm typecheck` independently yet breaks the live sim. Don't
parallelize the producer and the consumer of a layout change — same wave,
Rust first.

**Source of truth precedence.** State this in every sub-agent prompt that
touches the wire format:

```
app/crates/evosim/src/ Rust (constants.rs, wasm_api/mod.rs SAB layout) > generated app/web/wasm/*.d.ts bindings > app/web/ TS consumers > tests > docs
```

**Gate commands for dispatch prompts:**

- Rust only: `cargo test --lib` + `cargo test --lib --features threads`
- TS only: `cd app/web && pnpm typecheck` (from repo root)
- Full consolidated: see [`testing-how-to.md`](testing-how-to.md)

## Living notes

Append-only field notes from real orchestration runs. Newest first. This
is the **one** section that may carry dated, concrete incidents — the body
above stays general; specifics that would rot live here. Extend it as you
learn.

*No live orchestration-run incidents recorded yet.*

## See also

- [`index.md`](index.md) — agent-context router.
- `/fresh-orchestrator` skill — bootstrap prompt for an orchestration chat.
- [`maintaining-docs.md`](maintaining-docs.md) — the doc-migration rules
  step 7 of the workflow invokes.
- [`testing-how-to.md`](testing-how-to.md) — the gates and the
  consolidated final suite.
- [`dev-loop.md`](dev-loop.md) — wasm rebuild + dev server, and the
  `--features threads` requirement.
- [`../plans/index.md`](../plans/index.md) — plan lifecycle for the hub doc.
