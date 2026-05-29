# Shared memory and message protocol

The single source of truth for every byte and every message that crosses
the main ↔ sim-worker boundary.

## What it is

Two SharedArrayBuffers (`controlSab` 16 B + `snapshotSab` ~3.9 MB) plus
two discriminated unions of postMessage payloads (`SimMessage` for
main→worker, `SimReply` for worker→main). All canonical shapes live in
`web/src/sim-bridge.ts`. The Rust constant `MAX_POP_FOR_SIM` and the TS
constant are asserted equal at boot — drift is a thrown error pointing
at "rebuild wasm".

## What it owns

- The 10 main→worker message kinds and the 5 worker→main reply kinds.
- The `controlSab` 4-word layout (`CTRL_CURRENT_SLOT`, `CTRL_SEQ`,
  `CTRL_FUTEX`, reserved).
- The `snapshotSab` two-slot layout: per-slot 20-byte stats header + 12-byte
  alignment padding + creature SoA region + grass density region.
- The creature SoA stride: `CREATURE_STRIDE = 8` (= 32 bytes).
- The atomic-flip ordering (write inactive slot → `Atomics.store` flip →
  `Atomics.add` seq bump).
- The `f32::to_bits` round-trip convention for the `tps` field in the
  stats header.
- The `id` interleave convention: per-creature `id_lo` / `id_hi` are two
  raw u32s placed at byte offset +24 / +28 within each 32-byte stride,
  written via `f32::from_bits` on the Rust side and read via a
  `Uint32Array` view on the JS side. No float→int conversion either way.
- The `SimBridge` runtime class (correlation map for request/reply,
  per-name slider debouncer, futex wake on every `postMessage`).

## What it does NOT own

- **What the snapshot bytes *mean*** — see
  [`simulation-core.md`](simulation-core.md). This doc owns the layout;
  sim core owns the data.
- **When the worker writes / main reads** — see
  [`worker-runtime.md`](worker-runtime.md).
- **JS-side frustum cull, GPU instance pack** — see
  [`render-pipeline.md`](render-pipeline.md).
- **The COOP/COEP headers** that make `SharedArrayBuffer` available — see
  [`build-and-deploy.md`](build-and-deploy.md).

## Control SAB (16 bytes)

`new Int32Array(controlSab)` → 4 i32 words:

| Word | Symbol | Use |
|---|---|---|
| 0 | `CTRL_CURRENT_SLOT` | u32 atomic. Which snapshot slot is live (0 or 1). Worker writes inactive, then `Atomics.store(ctrl, 0, new)`. |
| 1 | `CTRL_SEQ` | u32 atomic, monotone. Bumped via `Atomics.add(ctrl, 1, 1)` after every flip. |
| 2 | `CTRL_FUTEX` | u32 atomic. Futex word the worker `Atomics.waitAsync`s on. Main `Atomics.add` + `Atomics.notify` after every `postMessage` to wake. |
| 3 | reserved | — |

## Snapshot SAB (per-slot)

Per-slot byte layout (consult `web/src/sim-bridge.ts` for the constants):

| Bytes | Length | Content |
|---|---|---|
| `0..20` | 20 | Stats header — `[tick: u32, pop: u32, world_ended: u32, tps_bits: u32, jank_count: u32]`, all little-endian. |
| `20..32` | 12 | **Padding**. Do not trim — `new Float32Array(buf, offset, len)` requires the offset to be element-stride-aligned, and Chrome/Firefox enforce it. Creature stride is 32 B, so the SoA must start 32 B aligned. |
| `32..(32 + MAX_POP_FOR_SIM*32)` | up to `MAX_POP_FOR_SIM × 32` | Creature SoA at stride 32 bytes. |
| trailing | `GRASS_CELL_COUNT` | Grass density quantized to u8 (`(d * 255).clamp(0,255) as u8`). |

`tps_bits` is the raw bit pattern of an `f32` written by Rust's
`f32::to_bits` and read JS-side via `DataView.getFloat32(byteOffset+12,
/* littleEndian */ true)`. Same 4 bytes, exact round-trip, no encoding
decision to document.

Two slots are allocated: `SNAPSHOT_SAB_BYTES = 2 × SLOT_BYTES`. With the
current constants this is roughly 3.9 MB total; nothing resizes after
boot.

## Creature SoA (per-creature, 32 bytes)

| Bytes | f32 lane | Field |
|---|---|---|
| `0..4` | `[i*8 + 0]` | `x` (world units) |
| `4..8` | `[i*8 + 1]` | `y` (world units) |
| `8..12` | `[i*8 + 2]` | `body_radius` (world units) |
| `12..16` | `[i*8 + 3]` | `color_r` (action-EMA, [0,1] with 0.15 display floor) |
| `16..20` | `[i*8 + 4]` | `color_g` |
| `20..24` | `[i*8 + 5]` | `color_b` |
| `24..28` | u32 `idView[i*8 + 6]` | `id_lo` — low 32 bits of the stable u64 creature id |
| `28..32` | u32 `idView[i*8 + 7]` | `id_hi` — high 32 bits |

`id` reassembly JS-side:
`const id = idView[i*8 + 7] * 4294967296 + idView[i*8 + 6]`. The f64
mantissa is exact up to 2^53 — well above any v1 session's id count.

## Main → worker messages (`SimMessage`)

| `kind` | Payload | Freq |
|---|---|---|
| `boot` | `{ seed, initial_grass_seed_count, energy_max, founder_count, initial_sliders }` | once per worker lifetime |
| `set_slider` | `{ name: string, value: number }` (bools as `0|1`) | per slider input event (debounced 16 ms) |
| `set_target_tps` | `{ tps: number }` | per TPS dropdown change |
| `set_paused` | `{ paused: boolean }` | per space / play-pause click |
| `inspect_at` | `{ wx, wy, tolerance_world, request_id }` | per canvas click |
| `inspect_id` | `{ id, request_id }` | per inspector refresh |
| `request_nn_stats` | `{ request_id }` | ~750 ms poll |
| `request_profile_report` | `{ request_id }` | ~1000 ms poll |
| `profile_enable` | `{ on: boolean }` | per checkbox toggle |
| `reset_jank` | `{}` | per button click |

Every `postMessage` from main is followed by `Atomics.add(controlI32,
CTRL_FUTEX, 1)` + `Atomics.notify(controlI32, CTRL_FUTEX, 1)` so the
worker's `Atomics.waitAsync` wakes immediately (the `add` returns
"not-equal" synchronously; the `notify` covers the already-parked case).

## Worker → main replies (`SimReply`)

| `kind` | Payload | Notes |
|---|---|---|
| `boot_ready` | `{ world_size, grass_dim, threads, rayon_ok, max_pop_for_sim, snapshot_sab, control_sab }` | Posted **after** the worker runs one tick + writes one snapshot to slot 0, guaranteeing main a valid first frame. |
| `inspect_reply` | `{ request_id, json: string | null }` | `json === null` means no creature at that point / id. |
| `nn_stats_reply` | `{ request_id, json }` | |
| `profile_reply` | `{ request_id, json }` | Bundles `{profile, tps, jank_count, live_grass_cell_count, total_grass_density}` so main does not need four round-trips per second. |

The Stage-1 `snapshot` reply shape (`creatures`, `grass`, `ids` as
typed arrays in a postMessage payload) is **dead** post-Wave-C. Its type
is still declared in `sim-bridge.ts` for posterity; `SimBridge.dispatch`
will route an arriving `snapshot` reply to the optional handler if one
is set, but the worker no longer posts them.

## Atomic-flip ordering

The worker's `writeSnapshotToSAB()` follows a strict order:

```ts
const current = Atomics.load(ctrl, CTRL_CURRENT_SLOT);
const inactive: 0 | 1 = current === 0 ? 1 : 0;

// Write the full inactive slot (creatures + grass + stats) via three Uint8Array
// subarray views over the SAB. Rust copies through the views.
world.write_snapshot_to(creaturesView, grassView, statsView);

Atomics.store(ctrl, CTRL_CURRENT_SLOT, inactive);   // FLIP (store-before-add)
Atomics.add(ctrl, CTRL_SEQ, 1);                     // BUMP
```

**Store before add.** If main observes the seq bump it is guaranteed to
load the new `CTRL_CURRENT_SLOT` on the next `Atomics.load`. The
inactive slot is owned exclusively by the worker between flips — no
reader sees a torn write.

## Cross-language drift assert

`MAX_POP_FOR_SIM` lives twice — once in `src/constants.rs` and once in
`web/src/sim-bridge.ts`. The worker passes the Rust value through
`boot_ready.max_pop_for_sim`; main throws if the TS constant disagrees.
If the assert fires, the fix is "rebuild wasm" (the TS const was
edited and the wasm bundle is stale).

`CREATURE_STRIDE = 8` is checked by the renderer with `if (stride !== 8)
throw`. Today the renderer reads the constant from `sim-bridge.ts`
directly so this is a defensive guard; if the constant ever moves, keep
the guard.

## Code anchors

- `web/src/sim-bridge.ts` → `MAX_POP_FOR_SIM`, `CREATURE_STRIDE`,
  `SNAPSHOT_HEADER_BYTES`, `CREATURE_SOA_BYTES`, `GRASS_BYTES`,
  `SLOT_BYTES`, `SNAPSHOT_SAB_BYTES`, `CONTROL_SAB_I32_LEN`,
  `CTRL_CURRENT_SLOT`, `CTRL_SEQ`, `CTRL_FUTEX`, `slotOffset`,
  `creatureSoAOffset`, `grassOffset`, `readSnapshotHeader`,
  `SimMessage`, `SimReply`, `SimBridge`.
- `web/src/sim-worker.ts` → `writeSnapshotToSAB`, the SAB allocation in
  `handleBoot`.
- `web/src/main.ts` → `spawnSimWorker` (the `max_pop_for_sim` assert and
  the SAB view construction).
- `src/wasm_api.rs` → `WorldHandle::write_snapshot_to`,
  `WorldHandle::write_creatures_each`, `max_pop_for_sim` free function.
- `src/constants.rs` → `MAX_POP_FOR_SIM`, `GRASS_CELL_COUNT`,
  `GRASS_GRID_DIM`.

## Update when

- Any field changes type, size, or offset inside any SAB region. Update
  the byte-layout tables here AND the `slotOffset` / `creatureSoAOffset`
  / `grassOffset` helpers AND every reader (main, renderer, inspector).
- A new `SimMessage` / `SimReply` kind is added or removed. The
  discriminated unions in `sim-bridge.ts` must round-trip with the
  worker's `handle()` switch and main's `SimBridge.dispatch`.
- `CREATURE_STRIDE` or `MAX_POP_FOR_SIM` changes value in EITHER
  language — both must change, then rebuild wasm, then verify the boot
  handshake passes.
- The atomic-flip ordering changes (it currently relies on
  store-before-add).
- A new wasm-bindgen export becomes part of the protocol surface (e.g.,
  if a future feature adds a new typed reply).

## Why is it shaped this way

See [`decisions/sim.md`](../decisions/sim.md) for the SAB-not-postMessage
choice, the double-buffer-not-triple-buffer choice, and the `set_slider`
name-dispatch decision. See
[`decisions/cross-cutting.md`](../decisions/cross-cutting.md) for the
`MAX_POP_FOR_SIM` cross-language assert and the 32-byte stride alignment
rationale.

## See also

- [`worker-runtime.md`](worker-runtime.md)
- [`simulation-core.md`](simulation-core.md)
- [`render-pipeline.md`](render-pipeline.md)
- [`../decisions/sim.md`](../decisions/sim.md)
- [`../decisions/cross-cutting.md`](../decisions/cross-cutting.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
