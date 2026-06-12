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
  CTRL_CONSUMED_SEQ,
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
  CTRL_SPECIES_TABLE_EPOCH,
  CTRL_SPECIES_TABLE_LEN,
  CTRL_TARGET_TPS_BITS,
  CTRL_TELEMETRY_REPORT_EPOCH,
  CTRL_TELEMETRY_REPORT_LEN,
  CTRL_TELEMETRY_REPORT_REQ_EPOCH,
  CTRL_TELEMETRY_REQ_EPOCH,
  INSPECT_RESP_CAP,
  INSPECT_RESP_OFFSET,
  NN_STATS_CAP,
  NN_STATS_OFFSET,
  PROFILE_REPORT_CAP,
  PROFILE_REPORT_OFFSET,
  SPECIES_TABLE_CAP,
  SPECIES_TABLE_OFFSET,
  TELEMETRY_REPORT_CAP,
  TELEMETRY_REPORT_OFFSET,
} from "../generated/control-sab";
// v2.0.3 Stream 2b: re-export camera lane constants so worker + main can import
// them from sim-bridge without going through the generated file directly.
export {
  CTRL_CAMERA_CX_BITS,
  CTRL_CAMERA_CY_BITS,
  CTRL_CAMERA_ZOOM_BITS,
  CTRL_CAMERA_VIEWPORT_W,
  CTRL_CAMERA_VIEWPORT_H,
} from "../generated/control-sab";
import { SLIDER_INDEX } from "../generated/slider-ids";
// v2.0.4 S1: GRASS_LOD_BUDGET_AXIS is now generated from the Rust constant via
// gen-bindings → lod-constants.ts. Import here and re-export so consumers of
// sim-bridge get it without touching the generated file directly.
import { GRASS_LOD_BUDGET_AXIS as GRASS_LOD_BUDGET_AXIS_GENERATED } from "../generated/lod-constants";
import type { WorldConfig } from "../generated/world-config";

// Re-export hot SAB constants for the snapshot read path in main.ts.
export {
  CONTROL_SAB_BYTES,
  CTRL_CONSUMED_SEQ,
  CTRL_CURRENT_SLOT,
  CTRL_FUTEX,
  CTRL_NN_STATS_EPOCH,
  CTRL_PROFILE_REPORT_EPOCH,
  CTRL_SEQ,
};

/** Protocol version. Bump on a breaking change to either message union. */
export const SIM_BRIDGE_VERSION = 4;

export type WorkerDebugFault =
  | "crash_after_boot"
  | "freeze_after_boot"
  | "boot_timeout";

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
 * v2.0 Wave 2a/2b layout (stride unchanged at 8 lanes / 32 B):
 * `[x, y, radius, color_u32, id_lo, id_hi, packed_u32, pad]`.
 * `color_u32` is a packed RGBA8 (LE, A=255) display color; `id_lo` / `id_hi`
 * are the u32 halves of the creature id; `packed_u32` bit-packs the ring-flash
 * tag (bits 0..2), flash-ticks countdown (bits 3..6), and a reserved species_id
 * (bits 7..22). The raw-u32 lanes are read via a `Uint32Array` view (JS side).
 */
export const CREATURE_STRIDE = 8;

/**
 * Bytes per snapshot stats header — v2.0.3 Stream 2b: bumped 32 → 64.
 * Adds 32 bytes of window-metadata fields at [32..64) while keeping the
 * creature SoA 64-byte aligned (64 is a multiple of the 32B stride).
 *
 * Stats layout (LE):
 *   off  0: `tick`         u32
 *   off  4: `pop`          u32
 *   off  8: `world_ended`  u32 (0/1)
 *   off 12: `tps_bits`     u32 (= `f32::to_bits(tps)`)
 *   off 16: `jank_count`   u32
 *   off 20..32: reserved / padding (unchanged)
 *   off 32: `mip_level`    u32
 *   off 36: `win_origin_x` i32
 *   off 40: `win_origin_y` i32
 *   off 44: `win_w`        u32
 *   off 48: `win_h`        u32
 *   off 52: `tex_dim_w`    u32
 *   off 56: `tex_dim_h`    u32
 *   off 60: `wrap_mode`    u32  (0 = clamp/walled, 1 = wrap/torus)
 */
export const SNAPSHOT_HEADER_BYTES = 64;

/**
 * v2.0.4 S1: clipmap budget axis (cells per axis). The worker publishes at most
 * `GRASS_LOD_BUDGET_AXIS²` bytes per slot grass region. This is the single
 * source of truth: the value is generated from the Rust constant via
 * `src/bin/gen_bindings.rs` → `web/src/generated/lod-constants.ts`.
 * Do NOT edit lod-constants.ts manually; run `cargo run --bin gen-bindings`.
 */
export const GRASS_LOD_BUDGET_AXIS = GRASS_LOD_BUDGET_AXIS_GENERATED;

/** Bytes per creature SoA region in one snapshot slot. UNCHANGED in v2.0. */
export const CREATURE_SOA_BYTES = MAX_POP_FOR_SIM * CREATURE_STRIDE * 4;

/**
 * Runtime snapshot-slot geometry.
 *
 * v2.0 Wave 1a: the world is runtime-sized, so the grass region byte length
 * (and therefore the slot/buffer totals) is no longer a compile-time constant.
 *
 * v2.0.3 Stream 2b: the grass region is now a **clipmap window** of at most
 * `GRASS_LOD_BUDGET_AXIS²` bytes. At default grass_dim=1920 (< 2048) the
 * window equals the full field and the slot size is byte-identical to the
 * pre-2b layout. For larger worlds the slot grass region is capped at
 * `budget_axis²`. The ACTUAL window dims for a given tick are in the
 * window metadata (header bytes [32..64)).
 *
 * v2.0.3 Stream 2d: the slot now also carries a biome window (mode-downsampled,
 * same allocation size as the grass region) appended immediately after the
 * grass region. The biome window uses the same UV transform as the grass window.
 *
 * The single source of truth is the `grass_dim` reported in `boot_ready`
 * (mirrored from `WorldHandle.grass_dim`). Getting this wrong silently
 * over/under-runs the SAB slot, so the geometry lives in ONE place.
 */
export interface SlotLayout {
  /** Grass cells per axis (`grass_dim`). */
  grassDim: number;
  /**
   * Full grass cells per slot (`grass_dim²`). Kept for reference and for
   * the biome SAB sizing; NOT the grass-region byte count since 2b.
   */
  grassCellCount: number;
  /**
   * Slot grass region byte count = `min(grassDim, GRASS_LOD_BUDGET_AXIS)²`.
   * At default scale equals `grassCellCount`. This is the ALLOCATION size;
   * use window metadata (readWindowMetadata) for the actual window dims.
   */
  grassRegionBytes: number;
  /**
   * v2.0.3 Stream 2d: biome window region byte count = same as grassRegionBytes.
   * The actual written bytes per tick are win_w × win_h (from window metadata).
   * The biome window is appended immediately after the grass region in the slot.
   */
  biomeWinBytes: number;
  /** Header + creatures + grass + biome_win, in bytes. */
  slotBytes: number;
}

/**
 * Build the runtime slot geometry from the boot-time `grass_dim`.
 *
 * v2.0.3 Stream 2b: grass region bytes = `min(grassDim, GRASS_LOD_BUDGET_AXIS)²`
 * so the slot can hold the largest possible window. At default scale (1920 < 2048)
 * this equals `grassDim²` — byte-identical to the pre-2b layout.
 *
 * v2.0.3 Stream 2d: biome window = same allocation as grass region, appended after.
 */
export function makeSlotLayout(grassDim: number): SlotLayout {
  const grassCellCount = grassDim * grassDim;
  const winAxis = Math.min(grassDim, GRASS_LOD_BUDGET_AXIS);
  const grassRegionBytes = winAxis * winAxis;
  // v2.0.3 Stream 2d: biome window allocation = same budget as grass.
  const biomeWinBytes = grassRegionBytes;
  // v2.0.x alignment fix: pad each slot to a 4-byte multiple so slot 1's f32
  // creature SoA stays 4-aligned for `new Float32Array(...)` even when grass_dim
  // is odd (grass + biome = 2·win_axis² is ≡2 mod 4 for odd win_axis). MUST match
  // the Rust rounding in `SnapshotLayout::from_grass_cell_count`
  // (`SNAPSHOT_SLOT_ALIGN = 4`). Even grass_dim is already aligned → no-op.
  const SLOT_ALIGN = 4;
  const rawSlotBytes =
    SNAPSHOT_HEADER_BYTES + CREATURE_SOA_BYTES + grassRegionBytes + biomeWinBytes;
  const slotBytes = Math.ceil(rawSlotBytes / SLOT_ALIGN) * SLOT_ALIGN;
  return {
    grassDim,
    grassCellCount,
    grassRegionBytes,
    biomeWinBytes,
    slotBytes,
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

/**
 * v2.0.3 Stream 2d: byte offset of the u8 biome window within snapshot slot `slot`.
 * The biome window immediately follows the grass region. Same allocation size.
 * Actual written bytes are win_w × win_h per tick (from window metadata).
 */
export function biomeWinOffset(layout: SlotLayout, slot: 0 | 1): number {
  return grassOffset(layout, slot) + layout.grassRegionBytes;
}

// ---------------------------------------------------------------------------
// v2.0.3 Stream 2b — Window metadata
// ---------------------------------------------------------------------------

/**
 * Window metadata packed into snapshot header bytes [32..64) by the Rust worker.
 * Describes the clipmap window written into the slot's grass region.
 */
export interface WindowMetadata {
  /** Pyramid LOD level (0 = full resolution). */
  mipLevel: number;
  /** Logical window origin X in level-k cells. Signed for toroidal seam windows. */
  winOriginX: number;
  /** Logical window origin Y in level-k cells. Signed for toroidal seam windows. */
  winOriginY: number;
  /** Window width in level-k cells. */
  winW: number;
  /** Window height in level-k cells. */
  winH: number;
  /** Texture upload width (= winW for now). */
  texDimW: number;
  /** Texture upload height (= winH for now). */
  texDimH: number;
  /** 0 = clamp/walled, 1 = wrap/torus. */
  wrapMode: number;
}

/**
 * Read the window metadata from the snapshot header of `slot`.
 * `view` is a DataView over the entire snapshot buffer (wasm memory);
 * `slotByteBase` is the byte offset of the slot start within `view`
 * (i.e. `snapshotBaseOffset + slotOffset(layout, slot)`).
 */
export function readWindowMetadata(view: DataView, slotByteBase: number): WindowMetadata {
  const base = slotByteBase + 32; // window metadata starts at header offset 32
  return {
    mipLevel:    view.getUint32(base +  0, true),
    winOriginX:  view.getInt32(base +  4, true),
    winOriginY:  view.getInt32(base +  8, true),
    winW:        view.getUint32(base + 12, true),
    winH:        view.getUint32(base + 16, true),
    texDimW:     view.getUint32(base + 20, true),
    texDimH:     view.getUint32(base + 24, true),
    wrapMode:    view.getUint32(base + 28, true),
  };
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
  world_config: WorldConfig;
  initial_sliders: Record<string, number>;
  initial_target_tps: number;
  initial_paused: boolean;
  /** Test-only fault injection. Main only sets this from window.__evosimE2E. */
  debug_fault?: WorkerDebugFault;
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
   * Single source of truth for the snapshot slot grass/window geometry. */
  grass_dim: number;
  /** v2.0.4 S2: grass cell size in world-units used at construction. Default 5.0.
   * The renderer uses this for UV transform: cellSizeL = grass_cell_size × 2^mip.
   * Single source of truth: Rust `WorldHandle.grass_cell_size` (runtime field). */
  grass_cell_size: number;
  /** v2.0 Wave 1a: torus (wrap) vs walled world. Mirrors `WorldHandle.wrap_world`. */
  wrap_world: boolean;
  /** v2.0 Wave 1a: numeric biome seed actually used (Rust resolves 0 → random
   * and reports the resolved value so a restart can reuse it). */
  world_seed: number;
  /** Resolved master seed used to derive construction-time streams. */
  master_seed: number;
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
type PendingTelemetry = (json: string | null) => void;

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
  private lastSpeciesTableEpoch = 0;
  private lastTelemetryReportEpoch = 0;

  // Cached latest payload bytes per response slot. Filled by the response poller.
  private latestProfileReportJson: string | null = null;
  private latestNnStatsJson: string | null = null;
  private latestSpeciesTableJson: string | null = null;
  private telemetryPending: {
    reqEpoch: number;
    resolve: PendingTelemetry;
    deadlineMs: number;
  } | null = null;

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
    this.lastSpeciesTableEpoch = Atomics.load(this.ctrlI32, CTRL_SPECIES_TABLE_EPOCH);
    this.lastTelemetryReportEpoch = Atomics.load(this.ctrlI32, CTRL_TELEMETRY_REPORT_EPOCH);
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

  /**
   * Request the NN I/O inspection JSON for the creature with the given stable id.
   * Uses kind=2 on the inspect request SAB lane; the worker calls
   * `creature_nn_inspect_json(idx)` instead of `creature_inspect_json(idx)`.
   * Same response slot + epoch protocol as the regular inspect path.
   */
  requestNnInspectId(id: number): Promise<string | null> {
    return this.issueInspect((reqEpoch) => {
      if (!this.ctrlI32) return reqEpoch;
      const idLo = (id >>> 0) | 0;
      const idHi = Math.floor(id / 0x1_0000_0000) | 0;
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_ID_LO, idLo);
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_ID_HI, idHi);
      Atomics.store(this.ctrlI32, CTRL_INSPECT_REQ_KIND, 2); // 2 = NN inspect by id
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

  /**
   * Latest species-table report JSON mirrored from SAB, or null if none has
   * been observed yet. Synchronous (no Promise) so the pop-graph RAF sampler
   * can read it allocation-free on the hot path. The worker writes this every
   * 45 ticks in species mode only — single-pool never writes it, so this stays
   * null and the Wave-5 pop graph keeps its single-pool line. Shape:
   * `{ tick, species: [{ id, color_u32, name, count }, …] }`.
   */
  latestSpeciesTable(): string | null {
    return this.latestSpeciesTableJson;
  }

  requestTelemetryReport(): Promise<string | null> {
    if (!this.ctrlI32) return Promise.resolve(null);
    if (this.telemetryPending !== null) {
      this.telemetryPending.resolve(null);
      this.telemetryPending = null;
    }
    const reqEpoch = Atomics.load(this.ctrlI32, CTRL_TELEMETRY_REQ_EPOCH) + 1;
    Atomics.store(this.ctrlI32, CTRL_TELEMETRY_REQ_EPOCH, reqEpoch);
    Atomics.add(this.ctrlI32, CTRL_FUTEX, 1);
    Atomics.notify(this.ctrlI32, CTRL_FUTEX, 1);
    return new Promise<string | null>((resolve) => {
      this.telemetryPending = {
        reqEpoch,
        resolve,
        deadlineMs: performance.now() + REQUEST_TIMEOUT_MS,
      };
    });
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
    if (this.telemetryPending !== null) {
      this.telemetryPending.resolve(null);
      this.telemetryPending = null;
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

    // v2.0 Wave 5: per-species table (species mode only; epoch never advances
    // in single-pool so this stays untouched).
    const speciesEpoch = Atomics.load(this.ctrlI32, CTRL_SPECIES_TABLE_EPOCH);
    if (speciesEpoch !== this.lastSpeciesTableEpoch) {
      const len = Atomics.load(this.ctrlI32, CTRL_SPECIES_TABLE_LEN) >>> 0;
      this.latestSpeciesTableJson = this.decodeBytes(SPECIES_TABLE_OFFSET, SPECIES_TABLE_CAP, len);
      this.lastSpeciesTableEpoch = speciesEpoch;
    }

    const telemetryEpoch = Atomics.load(this.ctrlI32, CTRL_TELEMETRY_REPORT_EPOCH);
    if (telemetryEpoch !== this.lastTelemetryReportEpoch) {
      const respReqEpoch = Atomics.load(this.ctrlI32, CTRL_TELEMETRY_REPORT_REQ_EPOCH);
      const len = Atomics.load(this.ctrlI32, CTRL_TELEMETRY_REPORT_LEN) >>> 0;
      const json = this.decodeBytes(
        TELEMETRY_REPORT_OFFSET,
        TELEMETRY_REPORT_CAP,
        len,
      );
      this.lastTelemetryReportEpoch = telemetryEpoch;
      const pending = this.telemetryPending;
      if (pending !== null && pending.reqEpoch === respReqEpoch) {
        this.telemetryPending = null;
        pending.resolve(len === 0 ? null : json);
      }
    }

    // GC stale inspect request.
    if (this.inspectPending !== null && this.inspectPending.deadlineMs < performance.now()) {
      this.inspectPending.resolve(null);
      this.inspectPending = null;
    }
    if (this.telemetryPending !== null && this.telemetryPending.deadlineMs < performance.now()) {
      this.telemetryPending.resolve(null);
      this.telemetryPending = null;
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
