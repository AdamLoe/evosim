# Decisions — cross-cutting

Decisions that bind more than one architecture doc.

---

### `MAX_POP_FOR_SIM` is duplicated in Rust + TS and asserted at boot

- **Decision**: The constant lives in `crates/evosim/src/constants.rs` AND
  `web/src/sim/bridge.ts`. The worker passes the Rust value through
  `boot_ready.max_pop_for_sim`; main asserts the TS const matches and
  throws on mismatch.
- **Why**: The SAB region sizes are derived from this constant — they
  must agree across languages or the typed-array views go out of
  bounds. Asserting at boot makes drift fatal and immediate; a comment
  on each constant would rot.
- **Tradeoffs**: Two-place duplication. The boot assert is the
  load-bearing safety net. The error message points the reader at
  "rebuild wasm" — the most common cause of drift.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `crates/evosim/src/constants.rs → MAX_POP_FOR_SIM`,
  `web/src/sim/bridge.ts → MAX_POP_FOR_SIM`,
  `crates/evosim/src/wasm_api/mod.rs → max_pop_for_sim`,
  `web/src/main.ts → spawnSimWorker` (the assert).

### Snapshot SAB header is 64 bytes to preserve typed-array alignment

- **Decision**: `SNAPSHOT_HEADER_BYTES` is a fixed 64-byte header. The stats
  fields occupy the front of the header and the window-metadata fields occupy
  the later half; the creature SoA begins immediately after the header.
- **Why**: `new Float32Array(buf, offset, len)` and friends throw if
  `offset` is not aligned for the element type. A 64-byte header keeps the
  creature SoA aligned to the Rust byte stride and leaves room for the
  snapshot-window metadata without changing the creature region layout.
- **Applies to**: `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → SNAPSHOT_HEADER_BYTES`,
  `SNAPSHOT_CREATURE_STRIDE`; `web/src/sim/bridge.ts → SNAPSHOT_HEADER_BYTES`,
  `CREATURE_STRIDE`.

### `tps` is round-tripped via `f32::to_bits`, not as JSON or a separate atomic

- **Decision**: The stats header stores `tps` as the raw 4-byte
  little-endian bit pattern of the `f32`. JS decodes via
  `DataView.getFloat32(off + 12, true)`.
- **Why**: Same 4 bytes either way, exact round-trip, no encoding
  decision to document. Treating it as a u32 + a JS coercion would
  introduce precision questions.
- **Applies to**: `architecture/shared-memory-and-protocol.md`,
  `architecture/simulation-core.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → WorldHandle::write_snapshot`
  (the `tps().to_bits()` write), `web/src/sim/bridge.ts →
  readSnapshotHeader`.

### Worker→main poll cadences: 1 Hz for profile, 750 ms for NN stats

- **Decision**: `request_profile_report` polls at ~1000 ms;
  `request_nn_stats` at ~750 ms. Per-frame polling is forbidden.
- **Why**: Per-frame would flood the message protocol with traffic the
  user can't read at 60 Hz anyway. The asymmetry between the two is
  historical (different panels, different refresh comfort levels);
  one could collapse them if a unifying poll system arrived.
- **Applies to**: `architecture/shared-memory-and-protocol.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/widgets/perf-panel.ts → POLL_INTERVAL_MS`,
  `web/src/widgets/worker-stats.ts → POLL_MS`.

### Worker-control e2e tests force `targetTPS = 1000` before interacting

- **Decision**: Worker-control Playwright tests under `web/tests/e2e/`
  set target TPS to 1000 before exercising the control path they cover.
- **Why**: High target TPS stresses pacing overshoot, futex wake handling,
  SAB request delivery, and snapshot back-pressure. Tests at default TPS=60
  can pass while high-throughput control is broken.
- **Applies to**: `architecture/testing.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `web/tests/e2e/sim-bridge.spec.ts`,
  `web/tests/e2e/app-fps.spec.ts`,
  `web/tests/README.md`.
- **Revisit when**: a fundamentally different pacing primitive lands
  that no longer has the 0-timeout pathology; until then, the rule
  stays mandatory.

### Determinism guard: forbid `HashMap` / `HashSet` `iter*` in sim-critical files

- **Decision**: `clippy.toml` lists `HashMap`/`HashSet` `iter` /
  `iter_mut` / `into_iter` in `disallowed-methods`. Per-tick paths
  must use `BTreeMap` / `BTreeSet` or sorted `Vec` if they need
  iteration order.
- **Why**: Non-deterministic iteration order would silently make
  parallel-vs-sequential equivalence tests flaky and would break any
  future determinism guarantee. The guard catches the issue at clippy
  time, not at the first cross-platform flake.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/testing.md`.
- **Code anchors**: `clippy.toml`.

### Creature id is `u64`; surfaces to JS as `f64` (lossless up to 2^53)

- **Decision**: Stable creature ids are `u64` in Rust. They cross the
  wasm-bindgen boundary as `f64` (or as a `u32` pair via
  `f32::from_bits` in the SAB SoA). No `BigInt`.
- **Why**: `wasm-bindgen` does not auto-bridge `u64`. `f64`'s 53-bit
  mantissa is exact for any id the sim could ever mint in a single
  session. `BigInt` would force every consumer to think about
  `Number | BigInt`.
- **Tradeoffs**: A future session that minted more than 2^53 ids would
  break silently. Not a concern at any realistic playthrough.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/shared-memory-and-protocol.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → creature_at`,
  `creature_idx_by_id`, `fill_creature_bytes` (id_lo/id_hi split),
  `web/src/render/gl.ts → renderWorldImpl` (the `idView` decode).

### Sim worker first-paint handshake: tick once + snapshot once before `boot_ready`

- **Decision**: `handleBoot` runs `world.step_n(1)` and
  `writeSnapshotToSAB()` *before* posting `boot_ready`.
- **Why**: Main's first RAF after `boot_ready` is guaranteed a populated
  live slot. Without the pre-snapshot, there would be a one-frame
  window where the renderer pulls a slot full of zeros and the canvas
  flashes empty.
- **Applies to**: `architecture/worker-runtime.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/sim/worker.ts → handleBoot`.

### Rust owns WorldConfig and live defaults; TypeScript consumes generated mirrors

- **Decision**: Rust owns the versioned `WorldConfig` construction schema,
  complete config presets, and `DevSliders::default()`. `cargo run --bin
  gen-bindings` emits `web/src/generated/world-config.ts` with
  `DEFAULT_WORLD_CONFIG`, `WORLD_CONFIG_PRESETS`, and
  `DEFAULT_LIVE_SLIDER_VALUES`. `settings.ts → DEFAULTS` derives sim defaults
  from those generated values instead of carrying an independent table.
  `WorldHandle::sliders_defaults_json()` remains the runtime Rust map; the
  Playwright defaults-drift spec asserts it matches the generated live defaults.
  Additive config blocks, such as `WorldConfig.science.deterministic`, use
  serde defaults so older saved artifacts and settings payloads resolve to the
  shipped default when the field is absent.
- **Why**: Rust tests construct `World` directly and need canonical defaults
  without the web shell. Generating TS mirrors removes the silent-drift path
  while preserving a localStorage default object before the worker exists.
- **Applies to**: `architecture/simulation-core.md`,
  `architecture/app-shell.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/world/mod.rs → DevSliders::default`,
  `crates/evosim/src/wasm_api/mod.rs → WorldConfig`,
  `crates/evosim/src/bin/gen_bindings.rs → render_world_config`,
  `web/src/generated/world-config.ts`,
  `crates/evosim/src/wasm_api/mod.rs → sliders_defaults_json`,
  `web/src/settings.ts → DEFAULTS`,
  `web/tests/e2e/defaults-drift.spec.ts`.

### Hammer-restart is allowed; old rayon workers GC with the terminated parent

- **Decision**: 5× restart in 5 seconds must not leak threads or fail
  the next boot. The Playwright restart smoke test does not measure
  this, but the design supports it.
- **Why**: `worker.terminate()` is unconditional; the rayon child
  workers are GC'd with the parent. SAB views on main remain valid
  while the previous bridge's closures keep the SAB rooted, so the
  renderer keeps painting until the new boot lands.
- **Applies to**: `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/main.ts → restart`.

### Runtime `world_size` ⇒ computed-dims-equality SAB safety model

- **Decision**: `world_size` (default 9600u) is a **runtime** construction
  setting, not a compile-time constant. Consequently grass-grid dims and the
  snapshot grass/biome window regions are all **derived from settings at boot**.
  There is no Rust ↔ TS `GRASS_CELL_COUNT` constant contract; every web view is
  sized from the **boot-reported** `grass_dim`. `boot_ready` carries the runtime
  `grass_dim`, the snapshot grass and biome window allocations are computed from
  that value, and the TS side never uses a hardcoded grass-cell count.
  `MAX_POP_FOR_SIM` / `CREATURE_STRIDE` stay constant-asserted because the
  creature region is world-size-independent.
  The app-shell SAB-view binding side (how `makeSlotLayout` is built from
  `boot_ready.grass_dim`) is recorded in `decisions/app-shell.md`.
- **Why**: The user explicitly wants worlds **editable across a wide range**
  from one binary (a tiny fast asexual screensaver up to a grand toroidal
  multi-species map), which forces runtime SAB sizing. Sizing both grass and
  biome windows from the same `grass_dim` and budget cap makes them provably
  equal without a second constant to drift. Getting a view length wrong silently
  over/under-runs the snapshot slot, so the boot-time `grass_dim` is the single source of truth (one
  layout object, rebuilt per boot).
- **Applies to**: `architecture/shared-memory-and-protocol.md`
  (computed-dims-equality), `architecture/simulation-core.md` (`WorldDims`).
- **Code anchors**: `crates/evosim/src/constants.rs → WorldDims::from_world_size_with_cell_size`,
  `crates/evosim/src/wasm_api/mod.rs → SnapshotLayout::from_grass_cell_count` (the
  `grass_bytes == biome_bytes` assert), `web/src/sim/bridge.ts → SimReplyBootReady`.
- **Revisit when**: a feature needs the world to resize *after* boot (today
  nothing resizes after boot — restart rebuilds the world).

### Master seed derives construction-time seeds; internal world_seed remains shared

- **Decision**: The external construction payload uses `WorldConfig.master_seed`.
  A zero master seed resolves to a random nonzero seed at wasm construction and
  is reported in `boot_ready.master_seed`. Rust derives the sim RNG seed string
  and the internal numeric `world_seed` via `derive_world_seed`. The internal
  `world_seed` still feeds biome generation, grass clump boot/scatter, and
  species/founder setup inside `World`.
- **Why**: One external seed makes fresh-world construction reproducible and
  gives persistence/share artifacts a single config field to carry. The current
  `World` constructor still accepts one numeric `world_seed`; splitting biome,
  grass, and species into separate internal sub-seed fields would require a
  broader sim-core constructor migration. Keeping the internal shared lane
  preserves behavior while making the external contract coherent.
- **Tradeoffs**: Biome, grass clumps/scatter, and species/founders are all
  controlled by the master seed, but not yet independent internal streams.
  `WorldSeedStream` names the intended stream split for a later sim-core change.
- **Applies to**: `architecture/simulation-core.md` (biome gen + species
  seeding), `architecture/app-shell.md` (settings master seed),
  `decisions/sim.md` (biome / seeding entries).
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → WorldConfig`,
  `WorldHandle::new_with_config_json`,
  `crates/evosim/src/constants.rs → derive_world_seed`, `WorldSeedStream`,
  `crates/evosim/src/world/mod.rs → World::new_with_sliders_topology`
  (internal `world_seed` consumers).

### NN input layout is derived from construction settings → SAB/topology implications

- **Decision**: The NN input width is **runtime/settings-derived**, not a
  compile-time constant. `NnInputLayout::for_settings(wrap_world, species_mode,
  grass_multisight)` composes the active input groups and computes the total
  width (a multiple of 8 capped by `MAX_NN_INPUTS`); it feeds the first
  matmul's `fan_in`. Wrap, species mode, and grass multisight together select
  the layout; saved-world artifact load validates the embedded topology against
  the construction config instead of migrating arbitrary settings changes onto
  old brains.
- **Why**: Inputs constant for a brain's whole run waste weights and train
  against a dishonest surface; a settings-derived layout always trains against
  the honest input surface for the current world (see the fuller rationale in
  `decisions/sim.md`). This is cross-cutting because the width flows into the
  brain topology (`fan_in`), the boot `nn_topology` payload, and the brain/nn
  drift-guard tests, which exercise the runtime-width constraints.
- **Applies to**: `architecture/simulation-core.md` (the width table + topology),
  `architecture/shared-memory-and-protocol.md` (the `nn_topology` boot payload),
  `decisions/sim.md` (the settings-derived-layout decision).
- **Code anchors**: `crates/evosim/src/world/nn.rs → NnInputLayout`,
  `crates/evosim/src/brain/mod.rs → NnTopology::with_input_width`,
  `crates/evosim/src/constants.rs → MAX_NN_INPUTS`,
  `crates/evosim/src/brain/tests/width.rs → input_width_validation_rejects_bad_widths`.

### Saved-world artifacts use SAB request/response with a fixed cap

- **Decision**: The worker serves saved-world artifact exports over the control
  SAB request/response protocol, using `WORLD_ARTIFACT_OFFSET` and a fixed
  `WORLD_ARTIFACT_CAP` of 64 MiB. Oversized artifacts return an explicit JSON
  error payload rather than clipped bytes.
- **Why**: The app already routes long-running worker control through SAB, and
  artifact requests must be served while running or paused without adding a
  second postMessage control plane. A fixed cap keeps the worker transport
  bounded and makes quota/size failure visible to the UI.
- **Applies to**: `architecture/shared-memory-and-protocol.md`,
  `architecture/worker-runtime.md`, `architecture/app-shell.md`.
- **Code anchors**: `crates/evosim/src/control_sab.rs →
  WORLD_ARTIFACT_OFFSET`, `WORLD_ARTIFACT_CAP`, `CTRL_I32_REGION_LEN`;
  `web/src/generated/control-sab.ts → WORLD_ARTIFACT_OFFSET`, `WORLD_ARTIFACT_CAP`;
  `web/src/sim/worker.ts → serveWorldArtifactRequest`;
  `web/src/sim/bridge.ts → SimBridge`.

## See also

- [`sim.md`](sim.md)
- [`render.md`](render.md)
- [`profiler.md`](profiler.md)
- [`build.md`](build.md)
- [`app-shell.md`](app-shell.md) — app-shell-only decisions (settings schema, staged sliders, SAB-view binding)
- [`../architecture/simulation-core.md`](../architecture/simulation-core.md)
- [`../architecture/shared-memory-and-protocol.md`](../architecture/shared-memory-and-protocol.md)
