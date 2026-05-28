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
export const GRASS_BYTES = 921_600;

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

// ---------------------------------------------------------------------------
// Runtime SimBridge (Wave B)
// ---------------------------------------------------------------------------

/**
 * Wave B runtime: owns the Web Worker, the request_id correlation table for
 * async replies, and an emitter for the latest snapshot. Lives in this file
 * (next to the discriminated unions) per v1.6-plan.md §4 ambiguity 4.
 *
 * Stage-1 contract: `postMessage` is fire-and-forget for set_slider /
 * set_paused / set_target_tps / profile_enable / reset_jank; `request*`
 * methods return a Promise that resolves when the matching reply
 * arrives (correlated by `request_id`). Entries older than 5 s are dropped so
 * the map can't leak when the worker is busy.
 *
 * Wave D adds a futex-notify on top of postMessage; Wave B does not.
 */

const REQUEST_TIMEOUT_MS = 5_000;

interface PendingRequest {
  resolve: (json: string | null) => void;
  deadlineMs: number;
}

/** Wave D: per-name slider debounce trailing-edge delay (ms). */
const SLIDER_DEBOUNCE_MS = 16;

interface DebouncedSliderEntry {
  timer: ReturnType<typeof setTimeout>;
  value: number;
}

export class SimBridge {
  private worker: Worker;
  private nextRequestId = 1;
  private pending = new Map<number, PendingRequest>();
  private snapshotHandler: ((snap: SimReplySnapshot) => void) | null = null;
  private bootReadyHandler: ((reply: SimReplyBootReady) => void) | null = null;
  private gcTimer: ReturnType<typeof setInterval> | null = null;
  /**
   * Wave D: per-name trailing-edge debouncer for `set_slider` writes. A 100 Hz
   * `pointermove` on a slider track would otherwise flood the worker with
   * postMessages (and futex wakes). 16 ms trailing-edge collapses bursts to
   * roughly one per RAF tick; "last value wins" so the released value is the
   * one that lands.
   */
  private sliderDebounceTimers = new Map<string, DebouncedSliderEntry>();
  /**
   * Wave D: control SAB futex word view, populated when main wires up
   * `attachControlSab` after `boot_ready`. Used by `postMessage` to wake the
   * sim worker out of `Atomics.waitAsync(ctrl, CTRL_FUTEX, before, ...)`.
   * Null until the SAB handshake completes; postMessage falls back to
   * fire-and-forget while null (worker still wakes via the JS event loop on
   * each `setTimeout(0)` iteration during Wave B/C, and Wave D's loop drains
   * incoming messages at the top of every iteration regardless).
   */
  private controlI32: Int32Array | null = null;

  constructor(worker: Worker) {
    this.worker = worker;
    this.worker.onmessage = (e: MessageEvent<SimReply>) => {
      this.dispatch(e.data);
    };
    // Periodic GC of stale request entries. Cheap (≤ 100 entries typically).
    this.gcTimer = setInterval(() => this.gcPending(), 1_000);
  }

  /**
   * Wave D: attach the control SAB so subsequent `postMessage` calls also
   * notify the worker's futex. Called by main from `boot_ready` once the
   * control SAB is available; safe to call again on restart.
   */
  attachControlSab(controlSab: SharedArrayBuffer): void {
    this.controlI32 = new Int32Array(controlSab);
  }

  /** Issue a fresh u32 request id. */
  mintRequestId(): number {
    const id = this.nextRequestId;
    this.nextRequestId = (this.nextRequestId + 1) >>> 0;
    if (this.nextRequestId === 0) this.nextRequestId = 1;
    return id;
  }

  /** Fire-and-forget send. Used for set_slider / set_paused / set_target_tps. */
  postMessage(msg: SimMessage): void {
    this.worker.postMessage(msg);
    // Wave D: futex wake. The `add` mutates `CTRL_FUTEX` so the worker's
    // `Atomics.waitAsync(ctrl, CTRL_FUTEX, before, timeoutMs)` resolves
    // synchronously with `"not-equal"` if it was about to park; the `notify`
    // covers the case where the worker is already parked. Wraparound at u32
    // max is harmless (the futex value is opaque — only equality matters).
    if (this.controlI32 !== null) {
      Atomics.add(this.controlI32, CTRL_FUTEX, 1);
      Atomics.notify(this.controlI32, CTRL_FUTEX, 1);
    }
  }

  /**
   * Wave D: debounced `set_slider` write. Per-name 16 ms trailing-edge
   * debouncer; last value wins. Prevents pointermove flooding the worker
   * during slider drags. Final value lands within `SLIDER_DEBOUNCE_MS` of the
   * last input event.
   *
   * Use this for high-frequency dev-panel slider drags. Boot's
   * `initial_sliders` map is NOT routed through here — those are applied
   * synchronously by the worker before its first tick.
   */
  debouncedSetSlider(name: string, value: number): void {
    const existing = this.sliderDebounceTimers.get(name);
    if (existing !== undefined) {
      clearTimeout(existing.timer);
    }
    const timer = setTimeout(() => {
      this.sliderDebounceTimers.delete(name);
      this.postMessage({ kind: "set_slider", name, value });
    }, SLIDER_DEBOUNCE_MS);
    this.sliderDebounceTimers.set(name, { timer, value });
  }

  /**
   * Wave D: flush any pending debounced slider writes immediately. Called
   * before tearing the bridge down (restart) so the last in-flight slider
   * value isn't dropped on the floor.
   */
  flushDebouncedSliders(): void {
    for (const [name, entry] of this.sliderDebounceTimers) {
      clearTimeout(entry.timer);
      this.postMessage({ kind: "set_slider", name, value: entry.value });
    }
    this.sliderDebounceTimers.clear();
  }

  /** Register the snapshot listener (called every batch by the worker). */
  onSnapshot(fn: (snap: SimReplySnapshot) => void): void {
    this.snapshotHandler = fn;
  }

  /** Register the one-shot boot_ready listener. */
  onBootReady(fn: (reply: SimReplyBootReady) => void): void {
    this.bootReadyHandler = fn;
  }

  /** Inspector click → nearest creature in world space. */
  requestInspectAt(wx: number, wy: number, toleranceWorld: number): Promise<string | null> {
    const request_id = this.mintRequestId();
    const p = this.makePending(request_id);
    this.worker.postMessage({ kind: "inspect_at", wx, wy, tolerance_world: toleranceWorld, request_id });
    return p;
  }

  /** Inspector per-frame refresh by stable creature id. */
  requestInspectId(id: number): Promise<string | null> {
    const request_id = this.mintRequestId();
    const p = this.makePending(request_id);
    this.worker.postMessage({ kind: "inspect_id", id, request_id });
    return p;
  }

  /** 750 ms poll → NN worker stats JSON. */
  requestNnStats(): Promise<string | null> {
    const request_id = this.mintRequestId();
    const p = this.makePending(request_id);
    this.worker.postMessage({ kind: "request_nn_stats", request_id });
    return p;
  }

  /** 1 s poll → profile report JSON (bundled with tps/jank/grass counters). */
  requestProfileReport(): Promise<string | null> {
    const request_id = this.mintRequestId();
    const p = this.makePending(request_id);
    this.worker.postMessage({ kind: "request_profile_report", request_id });
    return p;
  }

  /** Tear down the worker and clear pending state. Called on restart. */
  terminate(): void {
    if (this.gcTimer !== null) {
      clearInterval(this.gcTimer);
      this.gcTimer = null;
    }
    // Resolve any in-flight promises with null so awaiters don't hang.
    for (const { resolve } of this.pending.values()) resolve(null);
    this.pending.clear();
    // Drop any pending slider debounce timers — the bridge is dying; the new
    // bridge will receive the current slider state via `boot.initial_sliders`.
    for (const entry of this.sliderDebounceTimers.values()) {
      clearTimeout(entry.timer);
    }
    this.sliderDebounceTimers.clear();
    this.controlI32 = null;
    this.worker.terminate();
  }

  private makePending(request_id: number): Promise<string | null> {
    return new Promise<string | null>((resolve) => {
      this.pending.set(request_id, {
        resolve,
        deadlineMs: performance.now() + REQUEST_TIMEOUT_MS,
      });
    });
  }

  private dispatch(reply: SimReply): void {
    switch (reply.kind) {
      case "boot_ready":
        if (this.bootReadyHandler) this.bootReadyHandler(reply);
        return;
      case "snapshot":
        if (this.snapshotHandler) this.snapshotHandler(reply);
        return;
      case "inspect_reply":
      case "nn_stats_reply":
      case "profile_reply": {
        const entry = this.pending.get(reply.request_id);
        if (!entry) return; // stale (TTL'd or never registered) — ignore.
        this.pending.delete(reply.request_id);
        if (reply.kind === "inspect_reply") {
          entry.resolve(reply.json);
        } else {
          entry.resolve(reply.json);
        }
        return;
      }
    }
  }

  private gcPending(): void {
    const now = performance.now();
    for (const [id, entry] of this.pending) {
      if (entry.deadlineMs < now) {
        entry.resolve(null);
        this.pending.delete(id);
      }
    }
  }
}
