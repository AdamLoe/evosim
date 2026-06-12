---
status:        draft
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: false
long_lived:    false
owning_docs:
  - architecture/simulation-core.md
  - architecture/worker-runtime.md
  - architecture/shared-memory-and-protocol.md
  - architecture/app-shell.md
  - architecture/testing.md
  - decisions/sim.md
  - decisions/app-shell.md
  - decisions/cross-cutting.md
---

# World Persistence, Autosave, Forking, and Artifacts

## Mission

Make worlds durable and reproducible enough for users and regression tests.
Done means the app can serialize a world, autosave it to IndexedDB, resume it
after reload, fork from a saved state, and import/export shareable artifacts
that can become regression fixtures.

## Scope

In scope:

- Versioned serialization for the sim state needed to resume the world.
- IndexedDB storage for autosaves and named saved states.
- Import/export of saved-state artifacts as local files.
- Fork semantics: load an existing state, choose whether to preserve or replace
  run identity/seed metadata, and continue as a new run.
- Regression fixture path that loads a saved state and validates deterministic
  or bounded outcomes.

Out of scope:

- Do not implement cloud sync or share-server URLs.
- Do not serialize every telemetry sample by default unless explicitly chosen.
- Do not make threaded scatter deterministic in this plan. Saved states in
  normal mode can resume without promising bit-for-bit future replay.
- Do not migrate constructor/default complexity here; depend on `WorldConfig`
  for new boot metadata instead.

## Dependencies

Design can start while `05-world-config-schema-presets.md` is being reviewed,
but implementation should follow the `WorldConfig` schema decision so the
artifact can include a stable config block. This plan also benefits from
`03-telemetry-history-export.md` if exports should bundle run history.

## Context Routes

Docs to load:

- `docs/overview.md` - current "no save/load" baseline.
- `docs/architecture/simulation-core.md`
- `docs/architecture/worker-runtime.md`
- `docs/architecture/shared-memory-and-protocol.md`
- `docs/architecture/app-shell.md`
- `docs/architecture/testing.md`
- `docs/decisions/sim.md`
- `docs/decisions/app-shell.md`
- `docs/decisions/cross-cutting.md`

Code routes:

- `app/crates/evosim/src/world/mod.rs`, `world/tick.rs`, `creature.rs`,
  `brain/`, `grass/`, `rng.rs` - state that must round-trip.
- `app/crates/evosim/src/wasm_api/mod.rs` - wasm export boundary and snapshot
  helpers.
- `app/web/src/sim/worker.ts` and `app/web/src/sim/bridge.ts` - save/load
  requests and boot/restart path.
- `app/web/src/main.ts` - restart/load orchestration.
- `app/web/src/settings.ts` - existing localStorage schema, not a replacement
  for world state.
- New likely route: `app/web/src/storage/` for IndexedDB wrapper.
- `app/web/tests/e2e/*` and Rust unit tests for round-trip fixtures.

## Workstreams

1. Serialization boundary and format.

   Define what belongs in the artifact: schema version, app/build version,
   world config, tick, dimensions, RNG states, creature SoA, grass density,
   biome/source seed metadata, species registry if active, profiler/telemetry
   inclusion policy, and compatibility flags. Prefer explicit versioned structs
   over ad hoc JSON assembled in TypeScript.

2. Wasm API and worker transport.

   Add exports/requests for serialize and deserialize/load. Loading a world
   should create a new worker/world lifetime rather than mutating every field of
   a running `World` in place unless the implementer can prove in-place load is
   simpler and safer.

3. IndexedDB autosave.

   Add autosave cadence that is coarse enough to avoid jank and respects the
   app FPS/snapshot decimation work. Autosave should be visible in UI and
   recoverable after reload. Include quota/error handling and a way to clear old
   saves.

4. Fork and artifact UX.

   Add import/export buttons and a saved-state list. Fork should make the run
   identity explicit so users can tell original, resumed, and forked runs apart.

5. Regression fixtures.

   Add at least one saved-state fixture path that can be loaded in tests. Use it
   for round-trip assertions and future regression reproduction.

## Acceptance / Verification

- Save -> reload page -> resume restores tick/pop/world dimensions/visible
  state within explicit tolerances.
- Export -> import in a fresh tab/session restores the same saved state.
- Fork creates an independent run and does not overwrite the source artifact
  unless the user explicitly saves over it.
- Corrupt or unsupported artifacts fail with a clear error and no half-loaded
  worker state.
- Autosave does not write every tick and does not dominate profiler output.
- Expected gates:
  - Rust round-trip unit tests, both default and `--features threads`
  - `cd app/web && pnpm typecheck`
  - Playwright save/resume/import smoke tests

## Handoff Notes

- Persistence should not depend on current localStorage settings schema except
  for UI preferences. World state gets its own versioned storage namespace.
- Artifact format should include enough metadata for deterministic science mode
  later, but normal mode does not have to promise deterministic future replay.
- Coordinate with telemetry export so users do not get two incompatible
  "download run" concepts.

## Migration Notes

At ship time, remove the "no save/load" statement from `overview.md`, add
architecture for persistence/storage, document protocol additions, and record
format/versioning rationale in `decisions/sim.md` or a new decision entry.
