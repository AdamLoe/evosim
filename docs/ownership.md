# Documentation ownership

Canonical owner map for facts that appear in more than one doc.

## How to use this file

Two docs gradually duplicate the same content with subtle differences
unless each concept has one named owner. This file is the tie-breaker.
When in doubt, edit the owner. Never re-document a concept in a non-owner
doc; link to the owner instead.

Fresh chats do not need to read this by default. Load it when updating
docs, resolving conflicting docs, or deciding where a new fact belongs.

## Owner map

| Concept | Canonical owner | Allowed to reference |
|---|---|---|
| World, CreatureSoA, tick step order, NN topology, grass density field | `architecture/simulation-core.md` | every other arch doc |
| Web Worker spawn/terminate, `Atomics.waitAsync` pacing, slider drain ordering | `architecture/worker-runtime.md` | sim-core, protocol, render |
| Every `SimMessage` / `SimReply` kind, control + snapshot SAB byte layout, snapshot creature stride, header fields | `architecture/shared-memory-and-protocol.md` | every consumer |
| GL renderer, camera, frustum cull, instance pack, highlight ring | `architecture/render-pipeline.md` | protocol (read-only) |
| Main-thread DOM layout, right-rail tab routing, Settings stage-then-apply, panel-installer assignment, `showProfiler` source-of-truth | `architecture/app-shell.md` | every non-canvas widget |
| The four profiler trees (`frame`, `tick`, `nn`, `grass_step`) and the no-rollup rule | `architecture/profiler.md` | every span call site |
| wasm-pack incantation, `.cargo/config.toml` link args, COOP/COEP headers, threaded-bundle invariants | `architecture/build-and-deploy.md` | `prompts/dev-server.md`, `agent-context/dev-loop.md` |
| What test suites exist, what each covers | `architecture/testing.md` | `agent-context/testing-how-to.md` |
| The `MAX_POP_FOR_SIM` constant + cross-language assertion | `decisions/cross-cutting.md` | sim-core, protocol |
| Which procedural doc an agent should read for a workflow | `agent-context/index.md` | every agent-context doc |
| Plan status metadata and lifecycle rules | `plans/index.md` | maintaining-docs |
| Plan creation template | `plans/template.md` | plans index |

## See also

- [`index.md`](index.md) — global docs router.
- [`agent-context/maintaining-docs.md`](agent-context/maintaining-docs.md)
  — update rules for agents shipping changes.
