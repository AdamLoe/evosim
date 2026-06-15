# Shared memory and message protocol

The single source of truth for the main <-> sim-worker protocol: which
payloads cross `postMessage`, which state lives in `controlSab`, and how the
worker publishes wasm-memory snapshots for the renderer.

## What it is

The steady-state control path is SAB-only. `web/src/sim/bridge.ts` sends a
`SimMessageBoot` during boot and receives `SimReplyBootReady`.
After boot, every control signal is a write into `controlSab` plus an epoch or
futex wake, and every worker response is either a control-SAB byte buffer or
the wasm-memory snapshot region.

The authoritative control-SAB byte layout is
[`crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs).
The generated TS mirror is
[`web/src/generated/control-sab.ts`](../../app/web/src/generated/control-sab.ts),
emitted by `crates/evosim/src/bin/gen_bindings.rs`. The snapshot layout and
main-side helpers are in
[`web/src/sim/bridge.ts`](../../app/web/src/sim/bridge.ts) and the Rust writer
is `crates/evosim/src/wasm_api/mod.rs` -> `WorldHandle::write_snapshot`.

Literal protocol constants such as `MAX_POP_FOR_SIM`, `CREATURE_STRIDE`,
`SNAPSHOT_HEADER_BYTES`, `GRASS_LOD_BUDGET_AXIS`, `CONTROL_SAB_BYTES`,
`CTRL_*`, and `SLIDER_COUNT` are mirrored or checked by
`cd app/web && pnpm docs:lint`; this doc names the owning symbols instead of
transcribing their current values.

## What it owns

- The `SimMessageBoot` / `SimReplyBootReady` handshake and the rule that no
  post-boot control signal uses `postMessage`.
- The `controlSab` layout, including control lanes, request/response epochs,
  and fixed-capacity byte buffers.
- The slider index protocol: Rust `SLIDER_NAMES`, generated
  `slider-ids.ts`, and the `CTRL_SLIDERS_BASE + index` lanes.
- The wasm-memory snapshot region returned in `boot_ready` as
  `wasm_memory`, `snapshot_buf_byte_offset`, and `snapshot_buf_byte_len`.
- Snapshot publication ordering: write inactive slot, store
  `CTRL_CURRENT_SLOT`, then bump `CTRL_SEQ`; main acks by writing
  `CTRL_CONSUMED_SEQ`.
- The epoch protocol for byte-buffer responses: write payload bytes and length,
  then bump the response epoch; consumers correlate with request epochs when
  the response is request-scoped.
- The f32 and id encoding conventions used in SAB lanes and snapshot fields.
- `SimBridge` as the main-thread runtime wrapper for the protocol.

## What it does NOT own

- **Worker pacing and when snapshots are written** - see
  [`worker-runtime.md`](worker-runtime.md).
- **What the creature, grass, and biome bytes mean** - see
  [`simulation-core.md`](simulation-core.md), [`species.md`](species.md), and
  [`biome.md`](biome.md).
- **Renderer upload, frustum cull, and GPU packing** - see
  [`render-pipeline.md`](render-pipeline.md).
- **Headers and build flags that make SAB and shared wasm memory available** -
  see [`build-and-deploy.md`](build-and-deploy.md).

## Control SAB

`handleBoot` allocates `new SharedArrayBuffer(CONTROL_SAB_BYTES)` and exposes
three views over it: `Int32Array` for atomic control/status lanes,
`Float32Array` for f32-valued lanes, and `Uint8Array` for length-prefixed JSON
buffers. The authoritative lane and buffer names are the `CTRL_*`,
`*_OFFSET`, and `*_CAP` constants in `crates/evosim/src/control_sab.rs`; the
TS import surface comes from `web/src/generated/control-sab.ts`.

Main writes pause, target TPS, profiler window, slider values, camera lanes,
snapshot-consumed acks, inspector requests, telemetry export requests,
saved-world artifact requests, reset-jank, and reset-profile through this SAB.
The worker reads those lanes in `web/src/sim/worker.ts` -> `readControlSab`
and serves responses from `serveInspectRequest`, `serveTelemetryRequest`,
`serveWorldArtifactRequest`, `maybeWriteProfileReport`, `maybeWriteNnStats`,
and `maybeWriteSpeciesTable`.

The futex lane is the wake mechanism. Any main-side write that should interrupt
the worker's pause or target-TPS park bumps `CTRL_FUTEX` and notifies it. This
is part of the runtime invariant checked by `cd app/web && pnpm docs:lint`.

## Boot handshake

`SimMessageBoot` carries a Rust-owned `WorldConfig`, initial live slider values
by name, initial pause/TPS state, optional saved-world artifact JSON with
`resume`/`fork` mode, and e2e-only fault injection. Fresh worlds are
constructed by `WorldHandle::newWithConfigJson`; artifact loads use
`WorldHandle::newFromArtifactJson`. Construction fields must be in
`WorldConfig`; live knobs ride the slider SAB after construction.

`SimReplyBootReady` returns the resolved world metadata, `control_sab`,
`wasm_memory`, snapshot byte offset/length, Rust slider defaults JSON, and the
Rust-reported population cap used by main's boot-time mismatch check. Main
constructs snapshot views directly over `wasm_memory.buffer`; there is no
separate snapshot SAB.

## Snapshot region

The snapshot region is `WorldHandle::snapshot_buf`, a Rust-owned `Vec<u8>` in
shared wasm linear memory. Each slot contains a fixed header, a fixed creature
SoA region sized from `MAX_POP_FOR_SIM` and `SNAPSHOT_CREATURE_STRIDE`, a
runtime-sized grass clipmap window allocation, and a matching biome window
allocation. `SnapshotLayout::from_grass_cell_count` and
`makeSlotLayout(grassDim)` must derive the same slot geometry, including slot
alignment.

The header starts with tick/pop/world-ended/TPS/jank stats and includes window
metadata consumed by `readWindowMetadata`. The grass and biome window regions
are u8 channels. Their allocation is derived from boot-time `grass_dim` and
`GRASS_LOD_BUDGET_AXIS`; the actual uploaded window size for a frame comes from
the snapshot header.

`WorldHandle::write_snapshot` reads camera center, zoom, viewport, wrap mode,
LOD bias, and the static biome pyramid to fill the inactive slot. Toroidal
windows publish signed logical origins while copying from modulo-normalized
source coordinates, so renderer ghost draws can sample the wrapped complement
correctly.

## Creature render word

The creature SoA stride and byte stride are guarded by `cd app/web && pnpm
docs:lint`. The owning declarations are `web/src/sim/bridge.ts` ->
`CREATURE_STRIDE` and `crates/evosim/src/wasm_api/mod.rs` ->
`SNAPSHOT_CREATURE_STRIDE`.

The per-creature snapshot record carries position, radius, display color,
stable id halves, and a packed render word. The exact lane order is owned by
`web/src/sim/bridge.ts` -> `CREATURE_STRIDE` and
`crates/evosim/src/wasm_api/mod.rs` -> `WorldHandle::write_snapshot`; renderer
decoding is in `web/src/render/gl.ts` -> `renderWorldImpl`. `color_u32` is the
common canvas color lane for lineage color in single-pool mode and species mode.
The packed word carries ring-flash state and species id; consumers should use
the bridge/render owners rather than duplicating bit offsets in docs.

Stable ids split into low/high u32 halves on the wire and are reassembled
JS-side using arithmetic, not JS bitwise operators, so ids remain exact within
the supported JS integer range.

## Request/response buffers

Inspector, profile, NN-stats, species table, telemetry export, and saved-world
artifact payloads are UTF-8 JSON byte buffers inside `controlSab`. Inspector,
telemetry, and artifact responses echo the request epoch. Profile, NN stats,
and species table are cadence-written reports observed by the `SimBridge`
poller.

Telemetry and saved-world artifact writers emit explicit JSON error payloads
when encoded output would exceed the fixed buffer capacity. They do not silently
truncate. `SimBridge.decodeBytes` copies SAB bytes into a fresh `Uint8Array`
before `TextDecoder.decode`, because decoding a view backed by
`SharedArrayBuffer` is rejected by browsers.

NN inspect is an inspector request kind handled by
`SimBridge.requestNnInspectId` and `serveInspectRequest`, which calls
`WorldHandle::creature_nn_inspect_json`. The worker serves inspect, telemetry,
and artifact requests in both running and paused branches so a paused world can
still answer UI requests.

## Cross-language drift guards

- `cd app/web && pnpm docs:lint` checks Rust/TS control-SAB constants,
  `MAX_POP_FOR_SIM`, `CREATURE_STRIDE`, `SNAPSHOT_HEADER_BYTES`,
  `GRASS_LOD_BUDGET_AXIS`, `NN_INPUTS`, `STARTING_POP_DEFAULT`, and
  `SLIDER_NAMES` / `SLIDER_COUNT` mirrors.
- `cargo test --lib` includes the Rust binding drift test for generated TS
  mirrors.
- Main's boot path compares `boot_ready.max_pop_for_sim` with the TS
  `MAX_POP_FOR_SIM` and throws with a rebuild hint on mismatch.

## Code anchors

- [`crates/evosim/src/control_sab.rs`](../../app/crates/evosim/src/control_sab.rs)
  -> `CTRL_CURRENT_SLOT`, `CTRL_SEQ`, `CTRL_FUTEX`, `CTRL_PAUSED`,
  `CTRL_TARGET_TPS_BITS`, `CTRL_CONTROL_EPOCH`, `CTRL_SLIDERS_BASE`,
  `CTRL_INSPECT_REQ_EPOCH`, `CTRL_PROFILE_REPORT_EPOCH`,
  `CTRL_NN_STATS_EPOCH`, `CTRL_SPECIES_TABLE_EPOCH`,
  `CTRL_CAMERA_CX_BITS`, `CTRL_CONSUMED_SEQ`,
  `CTRL_TELEMETRY_REQ_EPOCH`, `CTRL_WORLD_ARTIFACT_REQ_EPOCH`,
  `CTRL_I32_REGION_LEN`, `CONTROL_SAB_BYTES`.
- [`crates/evosim/src/wasm_api/mod.rs`](../../app/crates/evosim/src/wasm_api/mod.rs)
  -> `SLIDER_NAMES`, `SLIDER_COUNT`, `SNAPSHOT_HEADER_BYTES`,
  `SNAPSHOT_CREATURE_STRIDE`, `SNAPSHOT_SLOT_ALIGN`,
  `GRASS_LOD_BUDGET_AXIS`, `GRASS_LOD_MARGIN_FACTOR`, `SnapshotLayout`,
  `WorldHandle::write_snapshot`, `WorldHandle::set_slider_by_index`,
  `WorldHandle::creature_nn_inspect_json`,
  `WorldHandle::telemetry_report_json`, `WorldHandle::world_artifact_json`,
  `WorldHandle::species_table_json`, `max_pop_for_sim`.
- [`crates/evosim/src/bin/gen_bindings.rs`](../../app/crates/evosim/src/bin/gen_bindings.rs)
  -> `main`, `render_control_sab`, `render_slider_ids`,
  `render_lod_constants`, `render_world_config`.
- [`web/src/sim/bridge.ts`](../../app/web/src/sim/bridge.ts) ->
  `SimMessageBoot`, `SimReplyBootReady`, `MAX_POP_FOR_SIM`,
  `CREATURE_STRIDE`, `SNAPSHOT_HEADER_BYTES`, `GRASS_LOD_BUDGET_AXIS`,
  `SlotLayout`, `makeSlotLayout`, `slotOffset`, `creatureSoAOffset`,
  `grassOffset`, `biomeWinOffset`, `readWindowMetadata`, `SimBridge`,
  `SimBridge.requestNnInspectId`, `SimBridge.requestTelemetryReport`,
  `SimBridge.requestWorldArtifact`.
- [`web/src/sim/worker.ts`](../../app/web/src/sim/worker.ts) ->
  `handleBoot`, `readControlSab`, `serveInspectRequest`,
  `serveTelemetryRequest`, `serveWorldArtifactRequest`,
  `maybeWriteProfileReport`, `maybeWriteNnStats`,
  `maybeWriteSpeciesTable`, `writeSnapshotToSAB`, `maybeWriteSnapshotToSAB`,
  `simLoop`.
- [`web/src/main.ts`](../../app/web/src/main.ts) -> `spawnSimWorker`,
  `frame`, `restartWorker`, `recoverWorker`.
- [`web/src/render/gl.ts`](../../app/web/src/render/gl.ts) -> `renderWorld`,
  `renderWorldImpl`.
- [`crates/evosim/src/constants.rs`](../../app/crates/evosim/src/constants.rs)
  -> `MAX_POP_FOR_SIM`, `NN_INPUTS`, `STARTING_POP_DEFAULT`, `WorldDims`,
  `Biome`.
- [`crates/evosim/src/grass/mod.rs`](../../app/crates/evosim/src/grass/mod.rs)
  -> `GrassPyramid::viewport_window`.

## Update when

- A `SimMessageBoot` or `SimReplyBootReady` field changes.
- Any `controlSab` lane, response buffer, or generated mirror changes.
- The snapshot header, creature stride, slot layout, grass window, or biome
  window changes.
- Slider names, ordering, or codegen changes.
- The response epoch protocol or snapshot flip/ack ordering changes.
- A new control path moves between SAB and `postMessage`.

## See also

- [`worker-runtime.md`](worker-runtime.md)
- [`simulation-core.md`](simulation-core.md)
- [`species.md`](species.md)
- [`biome.md`](biome.md)
- [`render-pipeline.md`](render-pipeline.md)
- [`profiler.md`](profiler.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
- [Doc authoring rules](~/agent-docs/v1/rules/authoring-rules.md)
