# evosim documentation index

This tree is the canonical, current-state snapshot of the project. It is
optimized for LLM consumption: each doc owns a single concept, plans are
ephemeral, and superseded history lives in the git log (not here).

If you are a fresh AI chat: read [`prompts/fresh-chat.md`](prompts/fresh-chat.md)
first. It will route you here, then to [`overview.md`](overview.md), then to
whichever subsystem doc matches the user's request.

## How to use this tree

- **Architecture** docs (`architecture/`) describe **what currently IS**. They
  are tenseless; they never say "v1.6 introduced" or "Wave D added". When the
  code changes, the relevant architecture doc gets rewritten in place — the
  history of the change lives in the git log and (briefly, for the in-flight
  pass) in `plans/`.
- **Decisions** docs (`decisions/`) are sectioned by architecture domain
  (not by date). Each entry is declarative: `Decision`, `Why`, `Applies to`,
  with optional `Alternatives considered`, `Tradeoffs`, `Code anchors`,
  `Revisit when`. Only decisions that still apply are recorded; superseded
  rationale stays in git.
- **Agent-context** docs (`agent-context/`) are procedural: "when working on
  X, do Y, don't do Z." Read these only when the task description matches.
- **Prompts** (`prompts/`) are verbatim templates or thin pointers. **Prompts
  own no facts**; if a prompt looks substantive, the substance belongs in
  `architecture/` or `agent-context/` and the prompt should link.
- **Plans** (`plans/`) are mission docs for in-flight work. They are
  ephemeral: when a plan ships, the implementer updates the relevant
  architecture and decisions docs, then the plan is free to be archived or
  deleted.

## Map of the tree

| Concept | Owning doc |
|---|---|
| System at a glance | [`overview.md`](overview.md) |
| Where files live | [`repository-layout.md`](repository-layout.md) |
| World / SoA / tick model / NN / grass mechanic | [`architecture/simulation-core.md`](architecture/simulation-core.md) |
| Web Worker lifecycle, boot handshake, pacing | [`architecture/worker-runtime.md`](architecture/worker-runtime.md) |
| Main-thread UI shell: DOM layout, right-rail tabs, Settings stage-then-apply | [`architecture/app-shell.md`](architecture/app-shell.md) |
| Every main↔worker message kind, SAB layout, snapshot stride | [`architecture/shared-memory-and-protocol.md`](architecture/shared-memory-and-protocol.md) |
| GL renderer, camera, frustum cull, snapshot read | [`architecture/render-pipeline.md`](architecture/render-pipeline.md) |
| Profiler shape, the no-rollup rule, four-tree layout | [`architecture/profiler.md`](architecture/profiler.md) |
| wasm-pack incantation, COOP/COEP, threaded-bundle invariants | [`architecture/build-and-deploy.md`](architecture/build-and-deploy.md) |
| Test layout (cargo lib tests + Playwright e2e) | [`architecture/testing.md`](architecture/testing.md) |
| Why the sim core is shaped this way | [`decisions/sim.md`](decisions/sim.md) |
| Why the renderer is shaped this way | [`decisions/render.md`](decisions/render.md) |
| Why the profiler is shaped this way | [`decisions/profiler.md`](decisions/profiler.md) |
| Why the build is shaped this way | [`decisions/build.md`](decisions/build.md) |
| Decisions that cut across more than one subsystem | [`decisions/cross-cutting.md`](decisions/cross-cutting.md) |
| Rust + TS conventions specific to this repo | [`agent-context/coding-style.md`](agent-context/coding-style.md) |
| Git / commit / "what not to do" rules | [`agent-context/repo-rules.md`](agent-context/repo-rules.md) |
| Inner loop: rebuild wasm, restart server, when to clear caches | [`agent-context/dev-loop.md`](agent-context/dev-loop.md) |
| How to run / add tests | [`agent-context/testing-how-to.md`](agent-context/testing-how-to.md) |
| When you touch X, update Y (doc maintenance rules) | [`agent-context/maintaining-docs.md`](agent-context/maintaining-docs.md) |
| Fresh-chat entry-point template | [`prompts/fresh-chat.md`](prompts/fresh-chat.md) |
| Dev-server prompt (thin pointer) | [`prompts/dev-server.md`](prompts/dev-server.md) |
| Periodic doc-drift check prompt | [`prompts/check-docs.md`](prompts/check-docs.md) |

## Ownership map (single canonical owner per concept)

Two docs gradually duplicate the same content with subtle differences unless
each concept has one named owner. This table is the tie-breaker.

| Concept | Canonical owner | Allowed to reference |
|---|---|---|
| World, CreatureSoA, tick step order, NN topology, grass density field | `architecture/simulation-core.md` | every other arch doc |
| Web Worker spawn/terminate, `Atomics.waitAsync` pacing, slider drain ordering | `architecture/worker-runtime.md` | sim-core, protocol, render |
| Every `SimMessage` / `SimReply` kind, control + snapshot SAB byte layout, snapshot creature stride, header fields | `architecture/shared-memory-and-protocol.md` | every consumer |
| GL renderer, camera, frustum cull, instance pack, highlight ring | `architecture/render-pipeline.md` | protocol (read-only) |
| The four profiler trees (`frame`, `tick`, `nn`, `grass_step`) and the no-rollup rule | `architecture/profiler.md` | every span call site |
| wasm-pack incantation, `.cargo/config.toml` link args, COOP/COEP headers, threaded-bundle invariants | `architecture/build-and-deploy.md` | `prompts/dev-server.md`, `agent-context/dev-loop.md` |
| What test suites exist, what each covers | `architecture/testing.md` | `agent-context/testing-how-to.md` |
| The `MAX_POP_FOR_SIM` constant + cross-language assertion | `decisions/cross-cutting.md` | sim-core, protocol |

When in doubt, edit the owner. Never re-document a concept in a non-owner
doc — link to the owner instead.

## See also

- [`agent-context/maintaining-docs.md`](agent-context/maintaining-docs.md) —
  the rules every agent must follow when their commit touches one of the
  surfaces listed above.
