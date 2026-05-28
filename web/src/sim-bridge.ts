// Sim-bridge message protocol + SAB layout types.
//
// This file is the single source of truth for the main-thread ↔ sim-worker
// boundary. Wave A2 ships the types only — the runtime `SimBridge` class
// lands in Wave B. Wave C extends the SAB layout helpers.
//
// References:
//   - docs/plans/v1.6-plan.md §"Sim-bridge message protocol" (canonical).
//   - docs/plans/v1.6-plan.md §"Step A2" (this file's spec).
//
// Cross-language sync:
//   `MAX_POP_FOR_SAB` must equal the Rust constant `src/constants.rs`.
//   Wave B's `boot_ready` reply carries `max_pop_for_sab: u32` sourced from
//   Rust; main asserts equality with this TS constant at handshake time and
//   throws on mismatch. Drift is fatal — rebuild wasm + restart pnpm dev.

/** Protocol version. Bump on a breaking change to either message union. */
export const SIM_BRIDGE_VERSION = 1;

/**
 * Maximum population the SAB creature SoA slot can hold.
 *
 * Matches the Rust constant `MAX_POP_FOR_SAB` in `src/constants.rs`.
 * Pop exceeding this cap is log-warned and truncated by the snapshot writer
 * (deterministic — dead creatures simply aren't rendered that frame).
 */
export const MAX_POP_FOR_SAB = 8000;

/**
 * Number of f32 lanes per creature in the snapshot SoA.
 *
 * Layout: `[x, y, body_radius, color_r, color_g, color_b, id_lo, id_hi]`
 * where `id_lo` / `id_hi` are the u32 halves of the creature id reinterpreted
 * as f32 via `f32::from_bits` (Rust side) and `Uint32Array` view (JS side).
 *
 * Matches `creature_stride()` post-Wave A1.
 */
export const CREATURE_STRIDE = 8;

/**
 * Bytes per snapshot stats header — 20 bytes of stats + 12 bytes of padding
 * to 32-byte-align the creature SoA that follows.
 *
 * Stats layout (LE):
 *   off  0: `tick`         u32
 *   off  4: `pop`          u32
 *   off  8: `world_ended`  u32 (0/1)
 *   off 12: `tps_bits`     u32 (= `f32::to_bits(tps)`)
 *   off 16: `jank_count`   u32
 *   off 20..32: padding (do NOT trim — creature SoA stride is 32B and
 *               `new Float32Array(buf, offset, len)` requires `offset` to be
 *               element-stride-aligned; Chrome/Firefox enforce this).
 */
export const SNAPSHOT_HEADER_BYTES = 32;

/** Bytes per creature SoA region in one snapshot slot (= 256_000). */
export const CREATURE_SOA_BYTES = MAX_POP_FOR_SAB * CREATURE_STRIDE * 4;

/** Bytes per grass density region in one snapshot slot. Matches `GRASS_CELL_COUNT`. */
export const GRASS_BYTES = 230_400;

/** Bytes per snapshot slot (header + creatures + grass). */
export const SLOT_BYTES = SNAPSHOT_HEADER_BYTES + CREATURE_SOA_BYTES + GRASS_BYTES;

/** Total snapshot SAB size — two double-buffered slots. */
export const SNAPSHOT_SAB_BYTES = SLOT_BYTES * 2;

/** Control SAB length in i32 words (16 bytes). */
export const CONTROL_SAB_I32_LEN = 4;

/** Control SAB word index: current live snapshot slot (0 or 1), atomic. */
export const CTRL_CURRENT_SLOT = 0;

/** Control SAB word index: monotone seq counter, incremented after every slot flip. */
export const CTRL_SEQ = 1;

/** Control SAB word index: futex word the sim worker `Atomics.waitAsync`s on. */
export const CTRL_FUTEX = 2;

/** Byte offset of snapshot slot `slot` (0 or 1) within the snapshot SAB. */
export function slotOffset(slot: 0 | 1): number {
  return slot * SLOT_BYTES;
}

/** Byte offset of the creature SoA within snapshot slot `slot`. */
export function creatureSoAOffset(slot: 0 | 1): number {
  return slotOffset(slot) + SNAPSHOT_HEADER_BYTES;
}

/** Byte offset of the grass density region within snapshot slot `slot`. */
export function grassOffset(slot: 0 | 1): number {
  return slotOffset(slot) + SNAPSHOT_HEADER_BYTES + CREATURE_SOA_BYTES;
}

// ---------------------------------------------------------------------------
// Snapshot stats header
// ---------------------------------------------------------------------------

/** Decoded view of the 20-byte stats header at the start of a snapshot slot. */
export interface SnapshotHeader {
  tick: number;
  pop: number;
  world_ended: boolean;
  tps: number;
  jank_count: number;
}

/**
 * Decode a snapshot stats header at `byteOffset` within `view`.
 *
 * `tick`/`pop`/`world_ended`/`jank_count` are decoded as little-endian u32.
 * `tps` is decoded as a little-endian f32, matching Rust's `f32::to_bits`
 * write (same 4 bytes, exact round-trip).
 */
export function readSnapshotHeader(view: DataView, byteOffset: number): SnapshotHeader {
  return {
    tick: view.getUint32(byteOffset + 0, true),
    pop: view.getUint32(byteOffset + 4, true),
    world_ended: view.getUint32(byteOffset + 8, true) !== 0,
    tps: view.getFloat32(byteOffset + 12, true),
    jank_count: view.getUint32(byteOffset + 16, true),
  };
}

// ---------------------------------------------------------------------------
// main → worker messages
// ---------------------------------------------------------------------------

/**
 * Boot payload sent once per worker lifetime, immediately after the worker is
 * spawned. The worker initializes wasm + rayon, runs one tick + writes one
 * snapshot, then replies with `boot_ready` — guaranteeing main a valid first
 * frame.
 *
 * `initial_sliders` is a name→value map (bools encoded as 0|1) drawn from the
 * in-memory dev-panel widget state (NOT localStorage) so a mid-drag restart
 * carries the dragged value.
 */
export interface SimMessageBoot {
  kind: "boot";
  seed: string;
  initial_grass_seed_count: number;
  energy_max: number;
  founder_count: number;
  initial_sliders: Record<string, number>;
}

/** Set one named slider. Bools ride this as `value: 0 | 1`. */
export interface SimMessageSetSlider {
  kind: "set_slider";
  name: string;
  value: number;
}

/** Set the target ticks-per-second pacing budget. */
export interface SimMessageSetTargetTps {
  kind: "set_target_tps";
  tps: number;
}

/** Pause / resume sim stepping. Render keeps painting the last-good frame. */
export interface SimMessageSetPaused {
  kind: "set_paused";
  paused: boolean;
}

/** Locate the creature nearest `(wx, wy)` in world space within tolerance. */
export interface SimMessageInspectAt {
  kind: "inspect_at";
  wx: number;
  wy: number;
  tolerance_world: number;
  request_id: number;
}

/** Refresh inspector JSON for a known creature id. */
export interface SimMessageInspectId {
  kind: "inspect_id";
  id: number;
  request_id: number;
}

/** Request a JSON dump of the NN worker stats. Polled at ~750 ms cadence. */
export interface SimMessageRequestNnStats {
  kind: "request_nn_stats";
  request_id: number;
}

/** Request a JSON profile report. Polled at ~1 s cadence. */
export interface SimMessageRequestProfileReport {
  kind: "request_profile_report";
  request_id: number;
}

/** Enable or disable profile sampling. */
export interface SimMessageProfileEnable {
  kind: "profile_enable";
  on: boolean;
}

/** Reset the jank counter in the snapshot header to zero. */
export interface SimMessageResetJank {
  kind: "reset_jank";
}

/** Discriminated union of every main → worker message shape. */
export type SimMessage =
  | SimMessageBoot
  | SimMessageSetSlider
  | SimMessageSetTargetTps
  | SimMessageSetPaused
  | SimMessageInspectAt
  | SimMessageInspectId
  | SimMessageRequestNnStats
  | SimMessageRequestProfileReport
  | SimMessageProfileEnable
  | SimMessageResetJank;

// ---------------------------------------------------------------------------
// worker → main replies
// ---------------------------------------------------------------------------

/**
 * Sent once per worker lifetime, **after** the worker has run one tick and
 * written one snapshot. Stage 1 leaves `snapshot_sab` and `control_sab` as
 * `null` — they are populated from Stage 2 onward (Wave C).
 *
 * `max_pop_for_sab` is sourced from the Rust constant; main asserts it equals
 * the TS `MAX_POP_FOR_SAB` constant and throws on mismatch.
 */
export interface SimReplyBootReady {
  kind: "boot_ready";
  world_size: number;
  grass_dim: number;
  threads: number;
  rayon_ok: boolean;
  max_pop_for_sab: number;
  snapshot_sab: SharedArrayBuffer | null;
  control_sab: SharedArrayBuffer | null;
}

/**
 * Stage 1 only: snapshot payload posted once per batch (NOT once per tick).
 * Replaced by SAB writes in Stage 2 — the message goes away in Wave C.
 *
 * The typed arrays alias wasm memory; structured-clone is eager in
 * Chrome + Firefox, so it's safe to mutate wasm memory on the next iteration.
 */
export interface SimReplySnapshot {
  kind: "snapshot";
  tick: number;
  pop: number;
  tps: number;
  world_ended: boolean;
  jank_count: number;
  creatures: Uint8Array;
  grass: Uint8Array;
  ids: Float64Array;
}

/** Reply to `inspect_at` / `inspect_id`. `json` is `null` if no creature matched. */
export interface SimReplyInspectReply {
  kind: "inspect_reply";
  request_id: number;
  json: string | null;
}

/** Reply to `request_nn_stats`. */
export interface SimReplyNnStatsReply {
  kind: "nn_stats_reply";
  request_id: number;
  json: string;
}

/**
 * Reply to `request_profile_report`. Worker bundles `tps`, `jank_count`,
 * `live_grass_cell_count`, `total_grass_density` into the same JSON to avoid
 * round-tripping each separately.
 */
export interface SimReplyProfileReply {
  kind: "profile_reply";
  request_id: number;
  json: string;
}

/** Discriminated union of every worker → main reply shape. */
export type SimReply =
  | SimReplyBootReady
  | SimReplySnapshot
  | SimReplyInspectReply
  | SimReplyNnStatsReply
  | SimReplyProfileReply;
