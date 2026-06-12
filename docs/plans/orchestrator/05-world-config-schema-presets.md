---
status:        shipped
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: true
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

# WorldConfig Schema, Presets, Defaults, and Master Seed

## Mission

Replace the implicit boot payload made of flat sliders, TS defaults, and growing
constructor arguments with a versioned `WorldConfig` contract. Done means world
construction is driven by a schema with generated TypeScript defaults, explicit
construction-only/live boundaries, presets, and coherent derivation from a
master seed.

## Scope

In scope:

- Versioned Rust-owned `WorldConfig` construction payload.
- Generated TypeScript defaults/types so Rust and TS cannot drift silently.
- Clear separation of construction-only config, live sliders, and pure app/render
  settings.
- Presets that produce complete configs, not partial slider patches.
- Master seed derivation for biome, species/founders, grass clumps, and sim RNG
  streams.
- Migration from current settings/localStorage into the new config shape.

Out of scope:

- Do not remove the live slider SAB lanes until there is a replacement for live
  mutation. This plan is primarily about construction and defaults.
- Do not implement save/load artifacts here, though artifact plans should embed
  `WorldConfig` once this exists.
- Do not change sim mechanics just to fit the schema.

## Dependencies

This should precede world persistence implementation and deterministic science
mode. It can follow Wave 1 runtime work to avoid simultaneous broad edits to
`bridge.ts`, `worker.ts`, and generated bindings.

## Context Routes

Docs to load:

- `docs/architecture/simulation-core.md`
- `docs/architecture/worker-runtime.md`
- `docs/architecture/shared-memory-and-protocol.md`
- `docs/architecture/app-shell.md`
- `docs/architecture/testing.md`
- `docs/decisions/sim.md`
- `docs/decisions/app-shell.md`
- `docs/decisions/cross-cutting.md`

Code routes:

- `app/crates/evosim/src/wasm_api/mod.rs` - `SLIDER_NAMES`,
  `WorldHandle::newWithFounderCount`, defaults JSON.
- `app/crates/evosim/src/constants.rs` - Rust default constants and seed
  derivation helpers.
- `app/crates/evosim/src/bin/gen_bindings.rs` - generated TS contracts.
- `app/web/src/generated/*` - generated mirrors.
- `app/web/src/settings.ts` - persisted settings schema and defaults.
- `app/web/src/widgets/devpanel.ts` - construction-only set and staged apply.
- `app/web/src/main.ts`, `app/web/src/sim/worker.ts`, `app/web/src/sim/bridge.ts`
  - boot payload construction and worker creation.
- `app/web/tests/e2e/defaults-drift.spec.ts`,
  `settings-persistence.spec.ts`.

## Workstreams

1. Schema design.

   Define `WorldConfig` with version, dimensions, wrap, master seed, grass
   initialization, species/mating, initial population, and any construction-only
   settings. Define a separate live slider/default block for values that can
   still change during a run. Name app/render-only settings separately.

2. Code generation and defaults.

   Generate TypeScript types/defaults from Rust. Keep existing drift tests, but
   evolve them to assert generated config defaults and live slider defaults
   rather than hand-maintained TS copies.

3. Boot payload migration.

   Replace positional constructor arguments with a config object at the
   main/worker boundary. Rust may still map into internal constructors, but the
   external wasm/TS contract should stop growing positional tails.

4. Settings and presets.

   Make presets produce complete `WorldConfig` objects. Settings UI should stage
   construction changes against the next config, while live sliders remain live.
   LocalStorage migration should preserve existing user values where names and
   semantics still match.

5. Master seed derivation.

   Define deterministic substreams from one master seed for biome, grass clumps,
   species/founders, and sim RNG. Preserve intentional independence between
   streams.

## Acceptance / Verification

- Fresh boot uses generated TS defaults and matches Rust defaults without a
  hand-maintained duplicate table.
- Construction-only settings apply on next world through `WorldConfig`; live
  sliders still update a running world through the existing live path.
- Existing localStorage blobs migrate or reset according to documented schema
  rules.
- Presets are round-trippable and complete.
- Master seed controls all construction-time streams in a documented,
  reproducible way.
- Expected gates:
  - `cargo test --lib`
  - `cargo test --lib --features threads`
  - `cargo run --bin gen-bindings` or repo-equivalent binding generation check
  - `cd app/web && pnpm typecheck`
  - `cd app/web && pnpm test:e2e`

## Handoff Notes

- Avoid editing persistence artifacts in the same implementation branch unless
  this plan has already landed; otherwise two agents will invent competing
  config formats.
- Keep `WorldConfig` version separate from app settings schema version.
- Any new cross-language constant or generated file needs manifest/decision doc
  updates at ship time.

## Migration Notes

Shipped in the current implementation branch:

- `architecture/simulation-core.md` now documents Rust-owned `WorldConfig`,
  generated TypeScript defaults/presets, construction/live/app setting
  boundaries, and master-seed derivation.
- `architecture/worker-runtime.md` and
  `architecture/shared-memory-and-protocol.md` now describe the boot
  `world_config` payload, `WorldHandle.newWithConfigJson`, generated config
  mirrors, and `boot_ready.master_seed`.
- `architecture/app-shell.md` now documents settings staging into
  `WorldConfig`, live slider fallback injection, generated defaults, and the
  compatibility migration of the historical `worldSeed` setting into
  `WorldConfig.master_seed`.
- `architecture/testing.md` now records generated default/config drift tests
  and complete-preset coverage.
- `decisions/cross-cutting.md`, `decisions/app-shell.md`, and
  `decisions/sim.md` now capture the Rust-owned config/default source of
  truth, staged construction settings, and current live `set_slider` examples.

One intentional legacy boundary remains: the external TS/wasm boot path now
uses `WorldConfig`, but Rust `World` internals still accept a single internal
`world_seed`. The master seed derives the sim RNG string and that internal
world seed; biome, grass, and species construction therefore still share the
legacy internal lane until a future `World` constructor split can consume the
named stream derivations directly.
