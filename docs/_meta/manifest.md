# Agent-docs manifest — evosim

App-specific bindings for the global agent-docs kit. The generic skills
and rules in `~/agent-docs/<agent_docs_version>/` read the slots
below; everything app-specific lives here, nothing generic does.

```yaml
agent_docs_version: v1
repo_name: evosim — Rust → WebAssembly browser evolution sandbox (sim worker + WebGL2 shell)
code_root: app/
```

> **Roots.** Agent-docs v1 fixes the docs root at `docs/` (repo top
> level). `code_root` is `app/`: the Rust crate source lives in
> `app/crates/evosim/src/` (`world/tick.rs`, `wasm_api/mod.rs`, `bin/gen_bindings.rs`), and the
> TypeScript/Vite shell lives in `app/web/` (`app/web/src/main.ts`,
> `app/web/src/sim/worker.ts`). Every code path written in the docs is relative
> to `app/`.

## Slot: decisions-domains

`sim`, `render`, `profiler`, `build`, `app-shell`, `perf`, `cross-cutting`.
(Authoritative list: `ls docs/decisions/`.)

## Slot: drift-gates

Per-commit gates (full commands + triage in
[`agent-context/testing-how-to.md`](../agent-context/testing-how-to.md);
gate philosophy in [`architecture/testing.md`](../architecture/testing.md)):

- `cargo test --lib` **and** `cargo test --lib --features threads` — both
  must pass (cross-feature equivalence: sequential fallback vs rayon NN +
  parallel grass).
- `cargo fmt --all --check`.
- `cargo clippy --all-targets -- -D warnings` **and**
  `cargo clippy --all-targets --features threads -- -D warnings`.
- `cargo bench --no-run` — compile-gate the benches (NOT
  `cargo build --benches`: `panic=abort` vs criterion's unwind).
- `cd app/web && pnpm docs:lint` — cheap mechanical docs drift check
  (local links/paths, ownership/index routing, generated constant mirrors,
  worker pacing invariants, profiler tree/span drift).
- `cd app/web && pnpm typecheck` and `pnpm build` (tsc + vite build).
- `cd app/web && pnpm test:e2e` — Playwright worker-control-path smoke.
- A threaded wasm rebuild always uses `--features threads`:
  `cd app && rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --dev --features threads`
  (omitting `--features threads` silently single-threads the sim).

## Slot: change-to-doc

The table to consult before declaring a commit "done." (Lifted from the
pre-migration `agent-context/maintaining-docs.md`; `web/` paths updated to
`app/web/` and the prompt/plan rows repointed at the kit.)

| If you changed… | Update… |
|---|---|
| `app/crates/evosim/src/world/mod.rs` or `tick.rs` (tick step, SoA layout, NN, graze/attack/energy) | `architecture/simulation-core.md` (tick step order, NN topology, code anchors) |
| `app/crates/evosim/src/world/species.rs` or mating/seeding in `mod.rs` | `architecture/species.md` (registry, seeding, Mate mechanic, crossover) |
| `app/crates/evosim/src/world/biome.rs` or `movement_penalty_for`/`biome_at` in `mod.rs` | `architecture/biome.md` (blob generation, static grid, effects) |
| `app/crates/evosim/src/wasm_api/mod.rs` (added/removed/renamed an export) | `architecture/simulation-core.md` (interaction-with-neighbours block) AND `architecture/shared-memory-and-protocol.md` if it's part of the message protocol |
| `app/crates/evosim/src/constants.rs` (any value quoted in a doc, esp. `MAX_POP_FOR_SIM`, `GRASS_CELL_COUNT`, `NN_*`, `FOUNDER_COUNT_DEFAULT`) | Wherever the constant is quoted; `decisions/cross-cutting.md` for `MAX_POP_FOR_SIM` |
| `app/crates/evosim/src/profiler.rs` API | `architecture/profiler.md`, `decisions/profiler.md` |
| `app/crates/evosim/src/grass/mod.rs` step shape or per-row bitset semantics | `architecture/simulation-core.md`, `architecture/profiler.md` (grass_step tree) |
| `app/crates/evosim/src/bin/gen_bindings.rs` (codegen of the TS mirror) | the generated files under `app/web/src/generated/`; `architecture/shared-memory-and-protocol.md` if the layout changes |
| `app/web/src/sim/worker.ts` loop shape, pacing, message handling | `architecture/worker-runtime.md`, possibly `decisions/sim.md` |
| `app/web/src/sim/bridge.ts` (any message kind, SAB constant, layout helper) | `architecture/shared-memory-and-protocol.md` (definitive) |
| `app/web/src/main.ts` (boot, render-loop shape, restart) | `architecture/worker-runtime.md`, `architecture/render-pipeline.md` |
| `app/web/src/render/gl.ts` (GL program, instance pack, frustum cull, span names) | `architecture/render-pipeline.md`, `architecture/profiler.md` if span names change |
| `app/web/src/perf.ts` API | `architecture/profiler.md` |
| `app/web/src/widgets/perf-panel.ts` (`TREE_ORDER`, poll cadence) | `architecture/profiler.md`, `decisions/profiler.md` |
| `app/web/src/widgets/devpanel.ts` (`currentSliderState`, slider list) | `architecture/worker-runtime.md` (boot payload section) |
| `app/web/vite.config.ts`, `app/web/public/_headers`, `Cargo.toml` profile, `.cargo/config.toml` | `architecture/build-and-deploy.md`, `decisions/build.md` |
| `app/web/tests/e2e/*` | `architecture/testing.md`, `agent-context/testing-how-to.md` |
| Repository directory layout (new dir, renamed dir) | `repository-layout.md` |
| Anything that introduces a new cross-language constant | `decisions/cross-cutting.md` (add a "constant duplicated in X + Y, asserted at Z" entry) |
| A new/removed/re-routed architecture doc | `architecture/index.md`, and `_meta/ownership.json` if ownership changes |
| A new/removed/re-routed decisions domain | `decisions/index.md`, and `_meta/ownership.json` if ownership changes |
| A new procedural workflow doc, or a changed condition for when one applies | `agent-context/index.md` and `docs/index.md` |
| A workflow command's behaviour (a global skill) changes | the global skill in `~/.claude/skills/`, and `agent-context/index.md` if routing changes |
| Plan lifecycle or status-metadata shape | `~/agent-docs/v1/plan-lifecycle.md` + `plan-template.md` (generic, in the kit); `plans/index.md` only if the app's landing/routing changes |
| A concept gets a new canonical owner, or a new cross-doc ownership conflict appears | `_meta/ownership.json` |

## Slot: drift-verification (high-risk surfaces for fix-docs-drift-all)

The doc-fix sweep verifies code-path pointers still resolve and
spot-checks these high-risk facts against code. evosim's checks are more
*behavioral* than a variant-set check — verify the invariant, not just the
symbol's existence:

- **Cross-language constants** match Rust ↔ TS (the doc quotes both, or
  says "derived from"): `MAX_POP_FOR_SIM`, `CREATURE_STRIDE`, `GRASS_BYTES`,
  `SNAPSHOT_HEADER_BYTES`, `CONTROL_SAB_I32_LEN`, the `CTRL_*` indices.
  Also `GRASS_CELL_COUNT`, `NN_INPUTS`, `FOUNDER_COUNT_DEFAULT`.
- **Profiler spans:** `cd app/web && pnpm docs:lint` checks the profiler
  doc's top-level tree order against `app/web/src/widgets/perf-panel.ts`
  and verifies the documented `frame.*` / `sim_worker.*` / `tick.*` /
  `nn.*` / `grass_step.*` spans have producers in code.
- **Worker control path** (`app/web/src/sim/worker.ts → simLoop`): pacing
  is synchronous `Atomics.wait` (not `Atomics.waitAsync`); the paused path
  waits with `Infinity`; the target-TPS path waits on `remainingMs` only
  when `remainingMs > 0.25`; `simLoop` stays synchronous with no
  `await`/`Promise.resolve()`/`setTimeout` yield path. `pnpm docs:lint`
  checks those invariants.
- **Build config:** `Cargo.toml` `[profile.dev]` + `[profile.release]`
  both `panic = "abort"`; `.cargo/config.toml` still has `--shared-memory`,
  `--max-memory`, `--import-memory`, TLS exports; COOP **and** COEP set in
  both `app/web/vite.config.ts` and `app/web/public/_headers`;
  `worker: { format: "es" }` in the vite config.

## Notes

- The generic agent-docs kit (authoring rules, coding-style, repo-rules,
  orchestrating rules) lives at `~/agent-docs/v1/rules/`. The
  workflow commands are global skills in `~/.claude/skills/`. This
  manifest is the only app-specific binding the kit reads.
- `agent-context/maintaining-docs.md` and `ownership.md` are thin in-repo
  stubs kept so existing `See also` links resolve; the rules are global
  and the ownership data is `_meta/ownership.json`.
