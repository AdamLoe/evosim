// Sim-bridge: main↔worker transport.
//
// v1.10 makes the control surface SAB-only. The Web Worker hot loop never
// reads `onmessage` during steady state. Only `boot` round-trips through
// postMessage; every other control message — sliders, pause, target TPS,
// inspector requests, profile/NN stats requests, reset-jank, reset-profile —
// is a Shared-Array-Buffer write on the main thread and a read at the top of
// each tick on the worker. Responses (inspector / profile / NN stats) are
// length-prefixed byte buffers in the same SAB; main reads via epoch advance.
//
// Why: `postMessage` requires the worker's event loop to run, which means
// every tick must `await` to drain the macrotask queue. The historical
// `Atomics.waitAsync(..., 1ms)` floor capped real TPS at ≈ 1000/(tick_ms+1).
// With control on SAB, the worker loop is synchronous; `Atomics.wait` is
// legal again because there is no postMessage to dark-hole.
//
// Cross-language sync: the SAB byte layout lives in `src/control_sab.rs` and
// is mirrored verbatim into `web/src/generated/control-sab.ts` by
// `cargo run --bin gen-bindings`. The slider name → index table is in
// `web/src/generated/slider-ids.ts` from the same source. A Rust unit test
// fails CI on drift.

import {
  CONTROL_SAB_BYTES,
  CTRL_CONTROL_EPOCH,
  CTRL_CURRENT_SLOT,
  CTRL_FUTEX,
  CTRL_INSPECT_REQ_EPOCH,
  CTRL_INSPECT_REQ_ID_HI,
  CTRL_INSPECT_REQ_ID_LO,
  CTRL_INSPECT_REQ_KIND,
  CTRL_INSPECT_REQ_TOL_BITS,
  CTRL_INSPECT_REQ_WX_BITS,
  CTRL_INSPECT_REQ_WY_BITS,
  CTRL_INSPECT_RESP_EPOCH,
  CTRL_INSPECT_RESP_LEN,
  CTRL_INSPECT_RESP_REQ_EPOCH,
  CTRL_NN_STATS_EPOCH,
  CTRL_NN_STATS_LEN,
  CTRL_PAUSED,
  CTRL_PROFILE_CLEAR_EPOCH,
  CTRL_PROFILE_REPORT_EPOCH,
  CTRL_PROFILE_REPORT_LEN,
  CTRL_PROFILE_WINDOW_MS,
  CTRL_RESET_JANK_EPOCH,
  CTRL_SEQ,
  CTRL_SLIDERS_BASE,
  CTRL_TARGET_TPS_BITS,
  INSPECT_RESP_CAP,
  INSPECT_RESP_OFFSET,
  NN_STATS_CAP,
  NN_STATS_OFFSET,
  PROFILE_REPORT_CAP,
  PROFILE_REPORT_OFFSET,
} from "./generated/control-sab";
import { SLIDER_INDEX } from "./generated/slider-ids";

// Re-export hot SAB constants for the snapshot read path in main.ts.
export {
  CONTROL_SAB_BYTES,
  CTRL_CURRENT_SLOT,
  CTRL_FUTEX,
  CTRL_SEQ,
};

/** Protocol version. Bump on a breaking change to either message union.
 *  v3 (v2.0 Wave 1c): boot carries world_size/wrap_world/world_seed and
 *  boot_ready adds wrap_world/world_seed + the biome buffer geometry; the
 *  snapshot grass region is now u8 (was f32). */
export const SIM_BRIDGE_VERSION = 3;

/**
 * Maximum simulation population.
 *
 * Matches the Rust constant `MAX_POP_FOR_SIM` in `src/constants.rs`. The cap
 * is enforced sim-side — `World::handle_births` randomly culls back to this
 * number after every birth phase, so the snapshot SAB never has to truncate.
 * The two-slot snapshot SAB sizes its creature region from this constant
 * (`MAX_POP_FOR_SIM × 32 B` per slot); change the value here AND in the Rust
 * constant, then rebuild wasm — the boot handshake throws on mismatch.
 */
export const MAX_POP_FOR_SIM = 32_000;

/**
 * Number of f32 lanes per creature in the snapshot SoA.
 *
 * Layout: `[x, y, body_radius, color_r, color_g, color_b, id_lo, id_hi]`
 * where `id_lo` / `id_hi` are the u32 halves of the creature id reinterpreted
 * as f32 via `f32::from_bits` (Rust side) and `Uint32Array` view (JS side).
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

/** Bytes per creature SoA region in one snapshot slot. UNCHANGED in v2.0. */
export const CREATURE_SOA_BYTES = MAX_POP_FOR_SIM * CREATURE_STRIDE * 4;

/**
 * Runtime snapshot-slot geometry.
 *
 * v2.0 Wave 1a: the world is runtime-sized, so the grass region byte length
 * (and therefore the slot/buffer totals) is no longer a compile-time
 * constant. The grass region is now **u8** (one byte per cell, quantized
 * Rust-side in `quantize_grass_into`) of `grass_cell_count` bytes — NOT the
 * old f32 `921_600 × 4`. The renderer uploads it as `R8`.
 *
 * The single source of truth is the `grass_dim` reported in `boot_ready`
 * (mirrored from `WorldHandle.grass_dim`); `grass_cell_count = grass_dim²`.
 * Every per-frame view length and slot offset is derived from a `SlotLayout`
 * built once at boot from that value. Getting this wrong silently
 * over/under-runs the SAB slot, so it lives in ONE place.
 */
export interface SlotLayout {
  /** Grass cells per axis (`grass_dim`). */
  grassDim: number;
  /** Grass cells per slot (`grass_dim²`); === u8 grass-region byte length. */
  grassCellCount: number;
  /** Header + creatures + grass, in bytes. */
  slotBytes: number;
}

/**
 * Build the runtime slot geometry from the boot-time `grass_dim`. The grass
 * region is `grass_dim²` u8 bytes (one per cell); the creature region and
 * header are world-size-independent.
 */
export function makeSlotLayout(grassDim: number): SlotLayout {
  const grassCellCount = grassDim * grassDim;
  return {
    grassDim,
    grassCellCount,
    // u8 grass: 1 byte per cell (no ×4 — that was the f32 era).
    slotBytes: SNAPSHOT_HEADER_BYTES + CREATURE_SOA_BYTES + grassCellCount,
  };
}

/** Byte offset of snapshot slot `slot` (0 or 1) within the snapshot region. */
export function slotOffset(layout: SlotLayout, slot: 0 | 1): number {
  return slot * layout.slotBytes;
}

/** Byte offset of the creature SoA within snapshot slot `slot`. */
export function creatureSoAOffset(layout: SlotLayout, slot: 0 | 1): number {
  return slotOffset(layout, slot) + SNAPSHOT_HEADER_BYTES;
}

/** Byte offset of the u8 grass region within snapshot slot `slot`. */
export function grassOffset(layout: SlotLayout, slot: 0 | 1): number {
  return slotOffset(layout, slot) + SNAPSHOT_HEADER_BYTES + CREATURE_SOA_BYTES;
}

// ---------------------------------------------------------------------------
// Snapshot stats header
// ---------------------------------------------------------------------------

export interface SnapshotHeader {
  tick: number;
  pop: number;
  world_ended: boolean;
  tps: number;
  jank_count: number;
}

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
// Boot handshake (the only surviving postMessage path)
// ---------------------------------------------------------------------------

/**
 * Boot payload sent once per worker lifetime. Initial slider values are
 * delivered by name so the SAB layout doesn't have to leak into the boot
 * message (and so a stale localStorage key doesn't desync the array index).
 * The worker applies each via `set_slider(name, value)` before its first
 * tick, then writes the canonical values into the control SAB so main's
 * post-boot view of `CTRL_SLIDERS` matches the actually-applied state.
 */
export interface SimMessageBoot {
  kind: "boot";
  seed: string;
  initial_grass_seed_count: number;
  energy_max: number;
  founder_count: number;
  full_grass_on_init: boolean;
  initial_sliders: Record<string, number>;
  initial_target_tps: number;
  initial_paused: boolean;
  /** v1.12: JSON-encoded `{hidden_sizes, activations}`. Empty string means
   * "use Rust-side legacy default" (32→48→24→5). The worker passes this
   * verbatim into `WorldHandle.newWithFounderCount`. */
  nn_topology_json: string;
  /** v2.0 Wave 1a: construction-only world shape. `world_size` is the world
   * extent in world-units (default 9600). `wrap_world` selects torus vs
   * walled. `world_seed` seeds the biome generator (0 ⇒ Rust picks a random
   * one and reports it back). All three feed `newWithFounderCount`'s new
   * trailing args. */
  world_size: number;
  wrap_world: boolean;
  world_seed: number;
}

/** Discriminated union of every main → worker message shape. v1.10: just boot. */
export type SimMessage = SimMessageBoot;

/**
 * Worker → main boot acknowledgment. Carries the two SABs main needs to
 * attach. After this, the worker never sends another postMessage — every
 * worker→main signal is a SAB epoch bump.
 */
export interface SimReplyBootReady {
  kind: "boot_ready";
  world_size: number;
  /** v2.0 Wave 1a: runtime grass cells per axis (1920 at the 9600 default).
   * Sizes the grass + biome views as `grass_dim²` bytes each — the single
   * source of truth for the SAB grass-region geometry. */
  grass_dim: number;
  /** v2.0 Wave 1a: torus (wrap) vs walled world. Mirrors `WorldHandle.wrap_world`. */
  wrap_world: boolean;
  /** v2.0 Wave 1a: numeric biome seed actually used (Rust resolves 0 → random
   * and reports the resolved value so a restart can reuse it). */
  world_seed: number;
  threads: number;
  rayon_ok: boolean;
  max_pop_for_sim: number;
  /**
   * v1.11 (A): the wasm `WebAssembly.Memory` handle. Main builds a
   * `Uint8Array` view over `wasm_memory.buffer` at the
   * `snapshot_buf_byte_offset` returned below. With shared memory enabled,
   * `WebAssembly.Memory` round-trips via `postMessage` and the underlying
   * SharedArrayBuffer is observed identically by both threads.
   */
  wasm_memory: WebAssembly.Memory;
  /** Byte offset of the snapshot region within `wasm_memory.buffer`. */
  snapshot_buf_byte_offset: number;
  /** Byte length of the snapshot region (runtime; grass region is u8). */
  snapshot_buf_byte_len: number;
  /** v2.0 Wave 1a: byte offset of the biome buffer (one u8 `Biome` per grass
   * cell) within `wasm_memory.buffer`. Static for the worker's lifetime. */
  biome_buf_byte_offset: number;
  /** v2.0 Wave 1a: biome buffer length in bytes (== `grass_dim²`). */
  biome_buf_byte_len: number;
  control_sab: SharedArrayBuffer | null;
  /** JSON map of every slider name → Rust default. Drift-guard input. */
  sliders_defaults_json: string;
}

export type SimReply = SimReplyBootReady;

// ---------------------------------------------------------------------------
// SimBridge — owns the worker + the control SAB writer/reader views
// ---------------------------------------------------------------------------

/** Per-name trailing-edge slider debounce delay (ms). */
const SLIDER_DEBOUNCE_MS = 16;

interface DebouncedSliderEntry {
  timer: ReturnType<typeof setTimeout>;
  value: number;
}

/** Resolves to a JSON string (the inspector response) or null on timeout. */
type PendingInspect = (json: string | null) => void;

/** Max time main will wait for a worker response before resolving to null. */
const REQUEST_TIMEOUT_MS = 5_000;

export class SimBridge {
  private worker: Worker;

  // Boot handshake.
  private bootReadyHandler: ((reply: SimReplyBootReady) => void) | null = null;

  // Control SAB views (attached post-boot).
  private ctrlI32: Int32Array | null = null;
  private ctrlF32: Float32Array | null = null;
  private ctrlBytes: Uint8Array | null = null;

  // Per-name 16ms trailing-edge debounce for slider writes — prevents a
  // 100 Hz pointermove drag from flooding the SAB + epoch counter.
  private sliderDebounceTimers = new Map<string, DebouncedSliderEntry>();

  // Inspector request correlation. Each inspect call advances
  // CTRL_INSPECT_REQ_EPOCH; the resolver is parked here. The next response
  // epoch advance whose CTRL_INSPECT_RESP_REQ_EPOCH matches our last request
  // resolves the promise. We only keep the latest pending request — superseded
  // ones resolve to null immediately.
  private inspectPending: { reqEpoch: number; resolve: PendingInspect; deadlineMs: number } | null = null;

  // Polled response trackers — we remember the last epoch we read from each
  // response slot so we can detect advance without re-reading the same bytes.
  private lastInspectRespEpoch = 0;
  private lastProfileReportEpoch = 0;
  private lastNnStatsEpoch = 0;

  // Cached latest payload bytes per response slot. Filled by the response poller.
  private latestProfileReportJson: string | null = null;
  private latestNnStatsJson: string | null = null;

  // Drives the per-frame poll of all SAB response slots. Started on attach,
  // stopped on terminate. Uses `setInterval` at 60 Hz so inspector responses
  // feel snappy without burning CPU.
  private responsePoller: ReturnType<typeof setInterval> | null = null;

  constructor(worker: Worker) {
    this.worker = worker;
    this.worker.onmessage = (e: MessageEvent<SimReply>) => {
      // v1.10: the only worker→main message is `boot_ready`.
      if (e.data.kind === "boot_ready" && this.bootReadyHandler) {
        this.bootReadyHandler(e.data);
      }
    };
  }

  /** Send the boot message. The only postMessage that survives v1.10. */
  sendBoot(boot: SimMessageBoot): void {
    this.worker.postMessage(boot);
  }

  /** Register the one-shot boot_ready listener. */
  onBootReady(fn: (reply: SimReplyBootReady) => void): void {
    this.bootReadyHandler = fn;
  }

  /**
   * Attach to the control SAB sent in `boot_ready`. After this point the
   * bridge is fully wired and every control method works.
   */
  attachControlSab(controlSab: SharedArrayBuffer): void {
    this.ctrlI32 = new Int32Array(controlSab);
    this.ctrlF32 = new Float32Array(controlSab);
    this.ctrlBytes = new Uint8Array(controlSab);
    // Seed our "last seen" epochs from the current SAB state so we don't
    // re-deliver pre-boot payloads as new ones.
    this.lastInspectRespEpoch = Atomics.load(this.ctrlI32, CTRL_INSPECT_RESP_EPOCH);
    this.lastProfileReportEpoch = Atomics.load(this.ctrlI32, CTRL_PROFILE_REPORT_EPOCH);
    this.lastNnStatsEpoch = Atomics.load(this.ctrlI32, CTRL_NN_STATS_EPOCH);
    this.responsePoller = setInterval(() => this.pollResponses(), 1000 / 60);
  }

  // ─── Slider / pause / target-TPS writes (SAB-only) ──────────────────────

  /** Per-name 16 ms debounced slider write. Latest value wins. */
  debouncedSetSlider(name: string, value: number): void {
    const existing = this.sliderDebounceTimers.get(name);
    if (existing !== undefined) {
      clearTimeout(existing.timer);
    }
    const timer = setTimeout(() => {
      this.sliderDebounceTimers.delete(name);
      this.writeSliderImmediate(name, value);
    }, SLIDER_DEBOUNCE_MS);
    this.sliderDebounceTimers.set(name, { timer, value });
  }

  /** Flush any pending debounced slider writes immediately. Restart safety. */
  flushDebouncedSliders(): void {
    for (const [name, entry] of this.sliderDebounceTimers) {
      clearTimeout(entry.timer);
      this.writeSliderImmediate(name, entry.value);
    }
    this.sliderDebounceTimers.clear();
  }

  private writeSliderImmediate(name: string, value: number): void {
    if (!this.ctrlF32 || !this.ctrlI32) return;
    const idx = SLIDER_INDEX[name];
    if (idx === undefined) {
      console.warn(`[bridge] unknown slider "${name}" — drop`);
      return;
    }
    // Float32Array write is atomic for aligned 32-bit lanes per the JS spec;
    // we don't need Atomics.store for the value itself. The epoch bump is
    // the release fence that publishes the write to the worker.
    this.ctrlF32[CTRL_SLIDERS_BASE + idx] = value;
    Atomics.add(this.ctrlI32, CTRL_CONTROL_EPOCH, 1);
  }

  setPaused(paused: boolean): void {
    if (!this.ctrlI32) return;
    Atomics.store(this.ctrlI32, CTRL_PAUSED, paused ? 1 : 0);
    // Bump futex + notify so a paused worker parked on Atomics.wait wakes.
    Atomics.add(this.ctrlI32, CTRL_FUTEX, 1);
    Atomics.notify(this.ctrlI32, CTRL_FUTEX, 1);
  }

  setTargetTps(tps: number): void {
    if (!this.ctrlI32 || !this.ctrlF32) return;
    this.ctrlF32[CTRL_TARGET_TPS_BITS] = tps;
    // No epoch bump — worker reads target TPS at top of every tick. Wake any
    // futex park so the new pacing slice takes effect immediately.
    Atomics.add(this.ctrlI32, CTRL_FUTEX, 1);
    Atomics.notify(this.ctrlI32, CTRL_FUTEX, 1);
  }

  resetJank(): void {
    if (!this.ctrlI32) return;
    Atomics.add(this.ctrlI32, CTRL_RESET_JANK_EPOCH, 1);
  }

  resetProfile(): void {
    if (!this.ctrlI32) return;
    Atomics.add(this.ctrlI32, CTRL_PROFILE_CLEAR_EPOCH, 1);
    // Drop the cached pre-reset report so the next `requestProfileReport`
    // returns null until the worker writes a fresh one. Pairs with the
    // worker-side `forceNextProfileReport` flag that bypasses the 60-tick
    // cadence — together they make reset visually instant.
    this.latestProfileReportJson = null;
  }

  /** Set the profiler's rolling-window length in milliseconds. Carried via
   *  CTRL_PROFILE_WINDOW_MS; the worker reads it once per tick and forwards
   *  to `world.profile_set_window_ms` when it changes. */
  setProfileWindowMs(ms: number): void {
    if (!this.ctrlI32) return;
    Atomics.store(this.ctrlI32, CTRL_PROFILE_WINDOW_MS, Math.max(0, Math.round(ms)));
    Atomics.add(this.ctrlI32, CTRL_FUTEX, 1);
    Atomics.notify(this.ctrlI32, CTRL_FUTEX, 1);
  }

  // ─── Inspector request/response ─────────────────────────────────────────

  requestInspectAt(wx: number, wy: number, toleranceWorld: number): Promise<string | null> {
    return this.issueInspect((reqEpoch) => {
      if (!this.ctrlI32 || !this.ctrlF32) return reqEpoch;
      this.ctrlF32[CTRL_INSPECT_REQ_WX_BITS] = wx;
      this.ctrlF32[CTRL_INSPECT_REQ_WY_BITS] = wy;
      this.ctrlF32[CTRL_INSPECT_REQ_TOL_BITS] = toleranceWorld;
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_KIND, 0); // 0 = by coord
      return reqEpoch;
    });
  }

  requestInspectId(id: number): Promise<string | null> {
    return this.issueInspect((reqEpoch) => {
      if (!this.ctrlI32) return reqEpoch;
      // id is a positive integer up to 2^53 — split via division/modulo since
      // JS bitwise ops are 32-bit signed.
      const idLo = (id >>> 0) | 0; // low 32 bits
      const idHi = Math.floor(id / 0x1_0000_0000) | 0; // high 32 bits
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_ID_LO, idLo);
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_ID_HI, idHi);
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_KIND, 1); // 1 = by id
      return reqEpoch;
    });
  }

  private issueInspect(writeParams: (reqEpoch: number) => number): Promise<string | null> {
    if (!this.ctrlI32) return Promise.resolve(null);
    // Supersede any pending request — only the latest is delivered.
    if (this.inspectPending !== null) {
      this.inspectPending.resolve(null);
      this.inspectPending = null;
    }
    // Compute the new request epoch BEFORE writing params so the writer
    // closure can see it. (We bump after writeParams returns to publish.)
    const reqEpoch = Atomics.load(this.ctrlI32, CTRL_INSPECT_REQ_EPOCH) + 1;
    writeParams(reqEpoch);
    Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_EPOCH, reqEpoch);
    Atomics.add(this.ctrlI32, CTRL_FUTEX, 1);
    Atomics.notify(this.ctrlI32, CTRL_FUTEX, 1);
    return new Promise<string | null>((resolve) => {
      this.inspectPending = {
        reqEpoch,
        resolve,
        deadlineMs: performance.now() + REQUEST_TIMEOUT_MS,
      };
    });
  }

  // ─── Profile / NN-stats poll-style readers ──────────────────────────────

  /**
   * Returns the latest profile report JSON written to SAB, or null if none
   * has been observed yet. The worker writes this every ~1 s; main is
   * encouraged to call this from a `setInterval` of similar cadence.
   */
  requestProfileReport(): Promise<string | null> {
    // Read-on-demand: the response poller already mirrored the freshest
    // payload to `latestProfileReportJson`. Returning a resolved Promise
    // preserves the old caller shape; callers can `await` to keep their
    // existing code path unchanged.
    return Promise.resolve(this.latestProfileReportJson);
  }

  requestNnStats(): Promise<string | null> {
    return Promise.resolve(this.latestNnStatsJson);
  }

  // ─── Tear-down ──────────────────────────────────────────────────────────

  terminate(): void {
    if (this.responsePoller !== null) {
      clearInterval(this.responsePoller);
      this.responsePoller = null;
    }
    for (const entry of this.sliderDebounceTimers.values()) {
      clearTimeout(entry.timer);
    }
    this.sliderDebounceTimers.clear();
    if (this.inspectPending !== null) {
      this.inspectPending.resolve(null);
      this.inspectPending = null;
    }
    this.ctrlI32 = null;
    this.ctrlF32 = null;
    this.ctrlBytes = null;
    this.worker.terminate();
  }

  // ─── Internal: 60 Hz response polling ───────────────────────────────────

  private pollResponses(): void {
    if (!this.ctrlI32 || !this.ctrlBytes) return;

    // Inspector response.
    const inspEpoch = Atomics.load(this.ctrlI32, CTRL_INSPECT_RESP_EPOCH);
    if (inspEpoch !== this.lastInspectRespEpoch) {
      const respReqEpoch = Atomics.load(this.ctrlI32, CTRL_INSPECT_RESP_REQ_EPOCH);
      const len = Atomics.load(this.ctrlI32, CTRL_INSPECT_RESP_LEN) >>> 0;
      const json = this.decodeBytes(INSPECT_RESP_OFFSET, INSPECT_RESP_CAP, len);
      this.lastInspectRespEpoch = inspEpoch;
      const pending = this.inspectPending;
      if (pending !== null && pending.reqEpoch === respReqEpoch) {
        this.inspectPending = null;
        pending.resolve(len === 0 ? null : json);
      }
    }

    // Profile report.
    const profEpoch = Atomics.load(this.ctrlI32, CTRL_PROFILE_REPORT_EPOCH);
    if (profEpoch !== this.lastProfileReportEpoch) {
      const len = Atomics.load(this.ctrlI32, CTRL_PROFILE_REPORT_LEN) >>> 0;
      this.latestProfileReportJson = this.decodeBytes(PROFILE_REPORT_OFFSET, PROFILE_REPORT_CAP, len);
      this.lastProfileReportEpoch = profEpoch;
    }

    // NN stats.
    const nnEpoch = Atomics.load(this.ctrlI32, CTRL_NN_STATS_EPOCH);
    if (nnEpoch !== this.lastNnStatsEpoch) {
      const len = Atomics.load(this.ctrlI32, CTRL_NN_STATS_LEN) >>> 0;
      this.latestNnStatsJson = this.decodeBytes(NN_STATS_OFFSET, NN_STATS_CAP, len);
      this.lastNnStatsEpoch = nnEpoch;
    }

    // GC stale inspect request.
    if (this.inspectPending !== null && this.inspectPending.deadlineMs < performance.now()) {
      this.inspectPending.resolve(null);
      this.inspectPending = null;
    }
  }

  private decodeBytes(offset: number, cap: number, len: number): string {
    if (!this.ctrlBytes) return "";
    const safeLen = Math.min(len, cap);
    // TextDecoder spec rejects views into SharedArrayBuffer
    // ("The provided ArrayBufferView value must not be shared.")
    // Copy into a non-shared Uint8Array before decoding.
    const buf = new Uint8Array(safeLen);
    buf.set(this.ctrlBytes.subarray(offset, offset + safeLen));
    return new TextDecoder().decode(buf);
  }
}
