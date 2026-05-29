# Fresh-chat entry-point prompt

Use this prompt (verbatim) at the top of any new conversation about this
project. It loads the deterministic minimum context — about 2k tokens —
and lets you route from there.

---

You are working on **evosim**, a Rust → WebAssembly browser evolution
sandbox. The project documentation lives under `docs/`. Read these
three files first, in order:

1. `docs/index.md` — the table of contents and the ownership map.
2. `docs/overview.md` — the system at a glance.
3. `docs/repository-layout.md` — one-line purpose for every directory.

Do not read the architecture or decisions docs proactively. Wait for the
user to describe what they want to do, then load the subsystem doc
that matches:

- Touching the simulation engine, NN, grass, or tick step →
  `docs/architecture/simulation-core.md` (+ `docs/decisions/sim.md`).
- Touching the sim worker, pacing, restart, message dispatch →
  `docs/architecture/worker-runtime.md` (+ `docs/decisions/sim.md`).
- Touching the SAB layout or any main↔worker message →
  `docs/architecture/shared-memory-and-protocol.md`.
- Touching the renderer, camera, GL programs, frustum cull →
  `docs/architecture/render-pipeline.md` (+ `docs/decisions/render.md`).
- Touching the profiler, span call sites, perf panel →
  `docs/architecture/profiler.md` (+ `docs/decisions/profiler.md`).
- Touching the build, wasm-pack flags, COOP/COEP, deploy →
  `docs/architecture/build-and-deploy.md` (+ `docs/decisions/build.md`).
- Adding/running tests → `docs/architecture/testing.md` +
  `docs/agent-context/testing-how-to.md`.
- Iterating live in the browser → `docs/agent-context/dev-loop.md`.
- Committing, conventional commits, what not to do →
  `docs/agent-context/repo-rules.md`.
- Editing the docs themselves →
  `docs/agent-context/maintaining-docs.md`.

The user's first message is below.
