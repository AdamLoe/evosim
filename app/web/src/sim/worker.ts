// v1.10: SAB-only sim worker — tight synchronous loop.
//
// The control surface is entirely SAB-backed. Every per-tick iteration:
//   1. Reads control SAB (paused, target TPS, sliders if epoch advanced,
//      inspect request if epoch advanced, profile/jank-clear requests).
//   2. If running, runs one tick.
//   3. Writes snapshot SAB. Writes profile-report + NN-stats SAB at fixed
//      tick cadences. Serves the inspect response if one is pending.
//   4. Paces toward target TPS via a synchronous `Atomics.wait`. Legal
//      because there is no postMessage hot path to dark-hole; the only
//      surviving postMessage is `boot`, handled before the loop starts.
//
// The worker still uses `self.onmessage` for the boot handshake — that's the
// one message that needs to arrive before `world` exists. After boot, the
// onmessage handler is left in place but the loop never yields control to
// the event loop, so additional messages can't dispatch. We don't expect
// any to arrive either.

import init, {
  WorldHandle,
  max_pop_for_sim,
} from "../../wasm/evosim";
import * as _wasmMod from "../../wasm/evosim";
import {
  CTRL_CURRENT_SLOT,
  CTRL_FUTEX,
  CTRL_SEQ,
  type SimMessage,
  type SimMessageBoot,
  type SimReply,
} from "./bridge";
import {
  CONTROL_SAB_BYTES,
  CTRL_CAMERA_CX_BITS,
  CTRL_CAMERA_CY_BITS,
  CTRL_CAMERA_VIEWPORT_H,
  CTRL_CAMERA_VIEWPORT_W,
  CTRL_CAMERA_ZOOM_BITS,
  CTRL_CONSUMED_SEQ,
  CTRL_CONTROL_EPOCH,
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
  CTRL_SLIDERS_BASE,
  CTRL_SPECIES_TABLE_EPOCH,
  CTRL_SPECIES_TABLE_LEN,
  CTRL_TARGET_TPS_BITS,
  CTRL_TELEMETRY_REPORT_EPOCH,
  CTRL_TELEMETRY_REPORT_LEN,
  CTRL_TELEMETRY_REPORT_REQ_EPOCH,
  CTRL_TELEMETRY_REQ_EPOCH,
  CTRL_WORLD_ARTIFACT_REQ_EPOCH,
  CTRL_WORLD_ARTIFACT_RESP_EPOCH,
  CTRL_WORLD_ARTIFACT_RESP_LEN,
  CTRL_WORLD_ARTIFACT_RESP_REQ_EPOCH,
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
  WORLD_ARTIFACT_CAP,
  WORLD_ARTIFACT_OFFSET,
} from "../generated/control-sab";
import { SLIDER_COUNT, SLIDER_NAMES } from "../generated/slider-ids";

// initThreadPool is only exported when wasm is built with --features threads.
const initThreadPool = (_wasmMod as unknown as Record<string, unknown>)[
  "initThreadPool"
] as ((n: number) => Promise<void>) | undefined;
const rayonCurrentNumThreads = (_wasmMod as unknown as Record<string, unknown>)[
  "rayon_current_num_threads"
] as (() => number) | undefined;

type WorldHandleBootCtor = {
  newWithConfigJson(configJson: string): WorldHandle;
  newFromArtifactJson(artifactJson: string, loadMode: string): WorldHandle;
};

type TelemetryWorldHandle = WorldHandle & {
  telemetry_report_json(): string;
};
type ArtifactWorldHandle = WorldHandle & {
  world_artifact_json(): string;
};

function sliderMapFromArtifact(artifactJson: string): Record<string, number> {
  const parsed = JSON.parse(artifactJson) as {
    state?: { sliders?: Record<string, unknown> };
  };
  const sliders = parsed.state?.sliders ?? {};
  const out: Record<string, number> = {};
  for (const [key, value] of Object.entries(sliders)) {
    if (typeof value === "number" && Number.isFinite(value)) out[key] = value;
    if (typeof value === "boolean") out[key] = value ? 1 : 0;
  }
  if (typeof sliders["digestion_cooldown_ticks"] === "number") {
    out["digestion_cooldown"] = sliders["digestion_cooldown_ticks"] as number;
  }
  if (typeof sliders["grass_cell_size"] === "number") {
    out["grass_size"] = sliders["grass_cell_size"] as number;
  }
  if (sliders["crossover_mode"] === "Average") out["crossover_mode"] = 0;
  if (sliders["crossover_mode"] === "FiftyFifty") out["crossover_mode"] = 1;
  return out;
}

/** Target rayon worker count (kept from v1.9; see worker-runtime.md). */
const TARGET_RAYON_WORKERS = 12;

/** Tick cadence for emitting the profile-report payload (~1 Hz at 60 TPS). */
const PROFILE_REPORT_EVERY_N_TICKS = 60;
/** Tick cadence for emitting the NN-stats payload (~750 ms at 60 TPS). */
const NN_STATS_EVERY_N_TICKS = 45;
/**
 * Tick cadence for emitting the v2.0 species-table payload (~750 ms at 60 TPS).
 * The Wave-5 Monitor pop-graph + canvas color table consume it; the producer is
 * cadence-written here (mirrors NN-stats). Only emitted in species mode.
 */
const SPECIES_TABLE_EVERY_N_TICKS = 45;

// ─── Worker-local state ─────────────────────────────────────────────────────

let world: WorldHandle | null = null;
let booted = false;
/** v2.0 Wave 4: true when the running world is in species mode (gates the
 * polled species-table report, which is empty/meaningless in single-pool). */
let speciesMode = false;

let controlSab: SharedArrayBuffer | null = null;
let ctrlI32: Int32Array | null = null;
let ctrlF32: Float32Array | null = null;
let ctrlBytes: Uint8Array | null = null;

// v2.0.3 Stream 2b: camera lane values read each tick from the control SAB.
// Default values produce the full-field window (zoom=1.0) before main writes.
let camCx = 0.0;
let camCy = 0.0;
let camZoom = 1.0;
let camViewportW = 0;
let camViewportH = 0;

// Locals refreshed at the top of every tick from the control SAB.
let paused = false;
let targetTPS = 60;

// Epoch counters we observed last time. An advance means "main published
// something we need to apply." `Atomics.load` reads are cheap enough that
// gating on the epoch matters mostly for the per-slider re-read, which would
// otherwise loop through 22 lanes every tick.
let lastControlEpoch = 0;
let lastProfileClearEpoch = 0;
let lastResetJankEpoch = 0;
let lastInspectReqEpoch = 0;
let lastTelemetryReqEpoch = 0;
let lastWorldArtifactReqEpoch = 0;
// Last profile window_ms value we applied to the Rust profiler. The control
// SAB carries the desired window each tick; we forward to wasm only when
// it actually changes so we don't burn a wasm call every iter.
let lastProfileWindowMs = 0;
let lastPublishedSeq = 0;
// Set true when a profile_clear request is observed. Consumed by
// maybeWriteProfileReport on the next loop iteration to force an immediate
// report write that overwrites the pre-reset SAB payload — otherwise the
// panel keeps polling the stale (~60 s of totals) report for up to
// PROFILE_REPORT_EVERY_N_TICKS ticks until the next cadence fires.
let forceNextProfileReport = false;

self.onmessage = (e: MessageEvent<SimMessage>): void => {
  const msg = e.data;
  if (!booted && msg.kind === "boot") {
    booted = true;
    handleBoot(msg).catch((err) => {
      // v1.12 hardening: a silent boot failure (e.g. a bad nn_topology
      // payload) used to leave main awaiting boot_ready forever while the
      // previous session's app shell stayed visible. Make the failure loud:
      // throw on a setTimeout so the unhandled error surfaces in DevTools
      // AND the page-level error handler if one's installed.
      console.error("[sim] boot failed:", err);
      setTimeout(() => {
        throw err instanceof Error ? err : new Error(String(err));
      }, 0);
    });
  }
};

function post(reply: SimReply): void {
  (self as unknown as { postMessage: (m: unknown) => void }).postMessage(reply);
}

// ─── Bootstrap ──────────────────────────────────────────────────────────────

async function handleBoot(boot: SimMessageBoot): Promise<void> {
  const wasmInit = await init();

  const isolated =
    (self as unknown as { crossOriginIsolated?: boolean }).crossOriginIsolated ?? false;
  console.log(`[sim] crossOriginIsolated=${isolated}`);

  let rayonOk = false;
  let threads = 1;
  if (initThreadPool && isolated) {
    try {
      const workers = Math.min(TARGET_RAYON_WORKERS, navigator.hardwareConcurrency);
      await initThreadPool(workers);
      threads = workers;
      rayonOk = true;
      console.log(`[sim] rayon workers: ${workers} (hardware: ${navigator.hardwareConcurrency})`);
    } catch (err) {
      console.warn("[sim] initThreadPool failed; continuing single-threaded:", err);
    }
  } else if (!isolated) {
    console.warn(
      "[sim] not cross-origin isolated; rayon disabled (single-threaded sim).",
    );
  }
  if (rayonCurrentNumThreads) {
    const actual = rayonCurrentNumThreads();
    if (actual <= 1) {
      console.warn(
        "[sim] rayon collapsed to 1 thread — sim will run single-threaded; " +
        "check COOP/COEP and build flags",
      );
    }
    threads = actual;
  }

  const bootCtor = WorldHandle as unknown as WorldHandleBootCtor;
  const savedSliderState = boot.saved_state_json
    ? sliderMapFromArtifact(boot.saved_state_json)
    : null;
  if (boot.saved_state_json) {
    world = bootCtor.newFromArtifactJson(
      boot.saved_state_json,
      boot.saved_state_load_mode ?? "resume",
    );
  } else {
    world = bootCtor.newWithConfigJson(JSON.stringify(boot.world_config));
  }
  speciesMode = !!boot.world_config.species.enabled;
  world.profile_enable(true);

  // Apply persisted slider state by name (initial_sliders is name→value).
  // After this point the canonical values live in the SAB; main writes only
  // through the SAB transport for the rest of the worker's lifetime.
  if (!boot.saved_state_json) {
    for (const [name, value] of Object.entries(boot.initial_sliders)) {
      try {
        world.set_slider(name, value);
      } catch (err) {
        console.warn(`[sim] set_slider("${name}", ${value}) rejected:`, err);
      }
    }
  }

  // Allocate control SAB and seed it with the initial state we just applied.
  // v1.11 (A): snapshot region lives in wasm linear memory — no separate
  // snapshotSab. Main reads via a view over `wasm.memory.buffer` at the
  // offset returned by `world.snapshot_buf_byte_offset`.
  controlSab = new SharedArrayBuffer(CONTROL_SAB_BYTES);
  ctrlI32 = new Int32Array(controlSab);
  ctrlF32 = new Float32Array(controlSab);
  ctrlBytes = new Uint8Array(controlSab);

  paused = boot.initial_paused;
  targetTPS = boot.initial_target_tps;
  Atomics.store(ctrlI32, CTRL_PAUSED, paused ? 1 : 0);
  ctrlF32[CTRL_TARGET_TPS_BITS] = targetTPS;
  // Mirror the canonical slider state into the SAB so main sees it on first
  // read. CRITICAL: seed EVERY lane. The tick loop drains ALL SLIDER_COUNT lanes
  // on every control-epoch advance (any Apply), so any lane left at 0 would be
  // re-applied as 0 on the next Apply. That is how a single slider change could
  // wipe the world: `max_population`'s control lives in the perf panel, not in
  // `initial_sliders`, so its lane stayed 0 and the next Apply drained it →
  // `apply_max_population(0)` clamps to cap=1 → the whole population is culled.
  // Fall back to the Rust slider defaults, which ARE the world's current value
  // for any name absent from initial_sliders (the world was constructed with
  // them), so the drain is always a faithful re-apply of live state.
  const sliderDefaults: Record<string, number> = JSON.parse(world.sliders_defaults_json());
  for (let i = 0; i < SLIDER_COUNT; i++) {
    const name = SLIDER_NAMES[i];
    const v = savedSliderState?.[name] ?? boot.initial_sliders[name] ?? sliderDefaults[name];
    if (v !== undefined) {
      ctrlF32[CTRL_SLIDERS_BASE + i] = v;
    }
  }
  // Stamp the epoch counters so our first read of the loop is a no-op.
  lastControlEpoch = Atomics.load(ctrlI32, CTRL_CONTROL_EPOCH);
  lastProfileClearEpoch = Atomics.load(ctrlI32, CTRL_PROFILE_CLEAR_EPOCH);
  lastResetJankEpoch = Atomics.load(ctrlI32, CTRL_RESET_JANK_EPOCH);
  lastInspectReqEpoch = Atomics.load(ctrlI32, CTRL_INSPECT_REQ_EPOCH);
  lastTelemetryReqEpoch = Atomics.load(ctrlI32, CTRL_TELEMETRY_REQ_EPOCH);
  lastWorldArtifactReqEpoch = Atomics.load(ctrlI32, CTRL_WORLD_ARTIFACT_REQ_EPOCH);

  // First-paint handshake: run one tick + one snapshot before posting
  // boot_ready so main's first RAF sees a live slot.
  world.step_n(1);
  lastPublishedSeq = writeSnapshotToSAB() ?? Atomics.load(ctrlI32, CTRL_SEQ);

  if (boot.debug_fault === "boot_timeout") {
    freezeForE2E();
  }

  // v1.11 (A): hand main the wasm memory + snapshot byte offset/len so it
  // can build views over `wasm.memory.buffer` directly. WebAssembly.Memory
  // with shared memory enabled round-trips through postMessage and the
  // underlying SharedArrayBuffer is observed identically on both threads.
  const reply: SimReply = {
    kind: "boot_ready",
    world_size: world.world_size,
    grass_dim: world.grass_dim,
    // v2.0.4 S2: runtime grass cell size (default 5.0; adjustable via grass_size slider).
    // The renderer uses this for UV transform so it stays correct at non-default sizes.
    grass_cell_size: world.grass_cell_size,
    // v2.0 Wave 1a: torus flag + resolved numeric biome seed.
    wrap_world: world.wrap_world,
    world_seed: world.world_seed,
    master_seed: (world as WorldHandle & { master_seed: number }).master_seed,
    threads,
    rayon_ok: rayonOk,
    max_pop_for_sim: max_pop_for_sim(),
    wasm_memory: wasmInit.memory,
    snapshot_buf_byte_offset: world.snapshot_buf_byte_offset,
    snapshot_buf_byte_len: world.snapshot_buf_byte_len,
    control_sab: controlSab,
    sliders_defaults_json: world.sliders_defaults_json(),
  };
  post(reply);

  if (boot.debug_fault === "crash_after_boot") {
    setTimeout(() => {
      throw new Error("[sim:e2e] simulated worker crash after boot");
    }, 0);
    return;
  }
  if (boot.debug_fault === "freeze_after_boot") {
    freezeForE2E();
    return;
  }

  simLoop();
}

function freezeForE2E(): never {
  const frozen = new Int32Array(new SharedArrayBuffer(4));
  for (;;) {
    Atomics.wait(frozen, 0, 0, 60_000);
  }
}

// ─── Per-tick control read ──────────────────────────────────────────────────

function readControlSab(): void {
  if (!world || !ctrlI32 || !ctrlF32) return;

  // Pause + target TPS: cheap unconditional read every tick.
  paused = Atomics.load(ctrlI32, CTRL_PAUSED) !== 0;
  const tps = ctrlF32[CTRL_TARGET_TPS_BITS];
  if (Number.isFinite(tps) && tps > 0) {
    targetTPS = tps;
  }

  // v2.0.3 Stream 2b: read camera lanes. F32Array reads are non-atomic but
  // that is acceptable — a partial-frame stale value just shifts the window by
  // ≤1 cell, which is within the 25% margin guarantee.
  const cx = ctrlF32[CTRL_CAMERA_CX_BITS];
  const cy = ctrlF32[CTRL_CAMERA_CY_BITS];
  const zoom = ctrlF32[CTRL_CAMERA_ZOOM_BITS];
  if (Number.isFinite(cx)) camCx = cx;
  if (Number.isFinite(cy)) camCy = cy;
  if (Number.isFinite(zoom) && zoom > 0) camZoom = zoom;
  camViewportW = Atomics.load(ctrlI32, CTRL_CAMERA_VIEWPORT_W) >>> 0;
  camViewportH = Atomics.load(ctrlI32, CTRL_CAMERA_VIEWPORT_H) >>> 0;

  // Sliders: gated on epoch advance. 22 lanes is cheap when actually changed,
  // but skipping the loop every tick saves cycles in the steady state.
  const ctrlEpoch = Atomics.load(ctrlI32, CTRL_CONTROL_EPOCH);
  if (ctrlEpoch !== lastControlEpoch) {
    lastControlEpoch = ctrlEpoch;
    for (let i = 0; i < SLIDER_COUNT; i++) {
      const v = ctrlF32[CTRL_SLIDERS_BASE + i];
      world.set_slider_by_index(i, v);
    }
  }

  // Reset-jank request.
  const rj = Atomics.load(ctrlI32, CTRL_RESET_JANK_EPOCH);
  if (rj !== lastResetJankEpoch) {
    lastResetJankEpoch = rj;
    world.reset_jank();
  }

  // Profile-clear request.
  const pc = Atomics.load(ctrlI32, CTRL_PROFILE_CLEAR_EPOCH);
  if (pc !== lastProfileClearEpoch) {
    lastProfileClearEpoch = pc;
    world.profile_clear();
    forceNextProfileReport = true;
  }

  // Profiler rolling-window length. 0 means "leave the default in place"
  // (uninitialized SAB on first boot). The Rust side clamps to a sane range.
  const windowMs = Atomics.load(ctrlI32, CTRL_PROFILE_WINDOW_MS) >>> 0;
  if (windowMs !== 0 && windowMs !== lastProfileWindowMs) {
    lastProfileWindowMs = windowMs;
    world.profile_set_window_ms(windowMs);
  }
}

// ─── Per-tick response writes ───────────────────────────────────────────────

function serveInspectRequest(): void {
  if (!world || !ctrlI32 || !ctrlF32 || !ctrlBytes) return;
  const reqEpoch = Atomics.load(ctrlI32, CTRL_INSPECT_REQ_EPOCH);
  if (reqEpoch === lastInspectReqEpoch) return;
  lastInspectReqEpoch = reqEpoch;

  const kind = Atomics.load(ctrlI32, CTRL_INSPECT_REQ_KIND);
  let idx: number | undefined;
  if (kind === 0) {
    const wx = ctrlF32[CTRL_INSPECT_REQ_WX_BITS];
    const wy = ctrlF32[CTRL_INSPECT_REQ_WY_BITS];
    const tol = ctrlF32[CTRL_INSPECT_REQ_TOL_BITS];
    const id = world.creature_at(wx, wy, tol);
    if (id !== undefined && id !== null) {
      idx = world.creature_idx_by_id(id);
    }
  } else {
    const idLo = Atomics.load(ctrlI32, CTRL_INSPECT_REQ_ID_LO) >>> 0;
    const idHi = Atomics.load(ctrlI32, CTRL_INSPECT_REQ_ID_HI) >>> 0;
    const id = idHi * 0x1_0000_0000 + idLo;
    idx = world.creature_idx_by_id(id);
  }

  let bytesWritten = 0;
  if (idx !== undefined) {
    // kind=2: NN I/O inspect — call creature_nn_inspect_json instead of
    // creature_inspect_json. Same SAB response slot, same epoch protocol.
    const jsonStr = kind === 2
      ? world.creature_nn_inspect_json(idx)
      : world.creature_inspect_json(idx);
    if (jsonStr) {
      const encoded = new TextEncoder().encode(jsonStr);
      const len = Math.min(encoded.length, INSPECT_RESP_CAP);
      ctrlBytes.set(encoded.subarray(0, len), INSPECT_RESP_OFFSET);
      bytesWritten = len;
    }
  }
  Atomics.store(ctrlI32, CTRL_INSPECT_RESP_LEN, bytesWritten);
  Atomics.store(ctrlI32, CTRL_INSPECT_RESP_REQ_EPOCH, reqEpoch);
  // Publish: bump response epoch last so main's poll sees a coherent view.
  Atomics.add(ctrlI32, CTRL_INSPECT_RESP_EPOCH, 1);
}

function maybeWriteProfileReport(tickIdx: number): void {
  if (!world || !ctrlI32 || !ctrlBytes) return;
  const forced = forceNextProfileReport;
  if (!forced && tickIdx % PROFILE_REPORT_EVERY_N_TICKS !== 0) return;
  forceNextProfileReport = false;
  const inner = world.profile_report_json();
  const bundled = JSON.stringify({
    profile: JSON.parse(inner),
    tps: world.tps,
    jank_count: world.jank_count,
    live_grass_cell_count: world.live_grass_cell_count(),
    total_grass_density: world.total_grass_density(),
  });
  const encoded = new TextEncoder().encode(bundled);
  const len = Math.min(encoded.length, PROFILE_REPORT_CAP);
  ctrlBytes.set(encoded.subarray(0, len), PROFILE_REPORT_OFFSET);
  Atomics.store(ctrlI32, CTRL_PROFILE_REPORT_LEN, len);
  Atomics.add(ctrlI32, CTRL_PROFILE_REPORT_EPOCH, 1);
}

function maybeWriteNnStats(tickIdx: number): void {
  if (!world || !ctrlI32 || !ctrlBytes) return;
  if (tickIdx % NN_STATS_EVERY_N_TICKS !== 0) return;
  const json = world.nn_worker_stats_json();
  const encoded = new TextEncoder().encode(json);
  const len = Math.min(encoded.length, NN_STATS_CAP);
  ctrlBytes.set(encoded.subarray(0, len), NN_STATS_OFFSET);
  Atomics.store(ctrlI32, CTRL_NN_STATS_LEN, len);
  Atomics.add(ctrlI32, CTRL_NN_STATS_EPOCH, 1);
}

// v2.0 Wave 4: cadence-write the polled species-table report (species_id →
// {color, name, count}). Mirrors the NN-stats producer. Species mode only — in
// single-pool the table is empty and the Wave-5 consumer never asks. The
// pop-graph + canvas color-table consumers land in Wave 5; this is the producer.
function maybeWriteSpeciesTable(tickIdx: number): void {
  if (!world || !ctrlI32 || !ctrlBytes || !speciesMode) return;
  if (tickIdx % SPECIES_TABLE_EVERY_N_TICKS !== 0) return;
  const json = world.species_table_json();
  const encoded = new TextEncoder().encode(json);
  const len = Math.min(encoded.length, SPECIES_TABLE_CAP);
  ctrlBytes.set(encoded.subarray(0, len), SPECIES_TABLE_OFFSET);
  Atomics.store(ctrlI32, CTRL_SPECIES_TABLE_LEN, len);
  Atomics.add(ctrlI32, CTRL_SPECIES_TABLE_EPOCH, 1);
}

function serveTelemetryRequest(): void {
  if (!world || !ctrlI32 || !ctrlBytes) return;
  const reqEpoch = Atomics.load(ctrlI32, CTRL_TELEMETRY_REQ_EPOCH);
  if (reqEpoch === lastTelemetryReqEpoch) return;
  lastTelemetryReqEpoch = reqEpoch;

  const telemetryWorld = world as TelemetryWorldHandle;
  const json = telemetryWorld.telemetry_report_json();
  let encoded = new TextEncoder().encode(json);
  if (encoded.length > TELEMETRY_REPORT_CAP) {
    const fallback = JSON.stringify({
      schema_version: 1,
      truncated: true,
      error: "telemetry report exceeded control SAB capacity",
      encoded_len: encoded.length,
      cap: TELEMETRY_REPORT_CAP,
    });
    encoded = new TextEncoder().encode(fallback);
  }
  const len = Math.min(encoded.length, TELEMETRY_REPORT_CAP);
  ctrlBytes.set(encoded.subarray(0, len), TELEMETRY_REPORT_OFFSET);
  Atomics.store(ctrlI32, CTRL_TELEMETRY_REPORT_LEN, len);
  Atomics.store(ctrlI32, CTRL_TELEMETRY_REPORT_REQ_EPOCH, reqEpoch);
  Atomics.add(ctrlI32, CTRL_TELEMETRY_REPORT_EPOCH, 1);
}

function serveWorldArtifactRequest(): void {
  if (!world || !ctrlI32 || !ctrlBytes) return;
  const reqEpoch = Atomics.load(ctrlI32, CTRL_WORLD_ARTIFACT_REQ_EPOCH);
  if (reqEpoch === lastWorldArtifactReqEpoch) return;
  lastWorldArtifactReqEpoch = reqEpoch;

  const artifactWorld = world as ArtifactWorldHandle;
  const json = artifactWorld.world_artifact_json();
  let encoded = new TextEncoder().encode(json);
  if (encoded.length > WORLD_ARTIFACT_CAP) {
    const fallback = JSON.stringify({
      kind: "evosim.world",
      schema_version: 1,
      error: "world artifact exceeded control SAB capacity",
      encoded_len: encoded.length,
      cap: WORLD_ARTIFACT_CAP,
      truncated: true,
    });
    encoded = new TextEncoder().encode(fallback);
  }
  const len = Math.min(encoded.length, WORLD_ARTIFACT_CAP);
  ctrlBytes.set(encoded.subarray(0, len), WORLD_ARTIFACT_OFFSET);
  Atomics.store(ctrlI32, CTRL_WORLD_ARTIFACT_RESP_LEN, len);
  Atomics.store(ctrlI32, CTRL_WORLD_ARTIFACT_RESP_REQ_EPOCH, reqEpoch);
  Atomics.add(ctrlI32, CTRL_WORLD_ARTIFACT_RESP_EPOCH, 1);
}

// ─── Snapshot write ─────────────────────────────────────────────────────────
//
// v1.11 (A+D): the snapshot region lives in wasm linear memory. The worker
// just tells Rust which slot to write into; Rust writes f32 grass + creature
// SoA + stats header directly into its own `Vec<u8>` (no JS boundary). Main
// reads via a view over `wasm.memory.buffer` at the offset shared in boot.

function writeSnapshotToSAB(): number | null {
  if (!world || !ctrlI32) return null;
  const current = Atomics.load(ctrlI32, CTRL_CURRENT_SLOT);
  const inactive: 0 | 1 = current === 0 ? 1 : 0;

  // v2.0.3 Stream 2b: pass camera params so Rust can compute the clipmap window.
  // The camera defaults (camCx=0, camCy=0, camZoom=1.0) on the very first tick
  // before main writes produce a safe (full-field at zoom=1) window.
  world.write_snapshot(inactive, camCx, camCy, camZoom, camViewportW, camViewportH);

  // Publish: flip slot, then bump seq (store-before-add).
  Atomics.store(ctrlI32, CTRL_CURRENT_SLOT, inactive);
  return Atomics.add(ctrlI32, CTRL_SEQ, 1) + 1;
}

function maybeWriteSnapshotToSAB(): boolean {
  if (!ctrlI32) return false;
  const consumedSeq = Atomics.load(ctrlI32, CTRL_CONSUMED_SEQ);
  if (consumedSeq !== lastPublishedSeq) return false;
  const seq = writeSnapshotToSAB();
  if (seq === null) return false;
  lastPublishedSeq = seq;
  return true;
}

// ─── Tight synchronous tick loop ────────────────────────────────────────────

function simLoop(): void {
  if (!world || !ctrlI32) return;
  let tickIdx = 0;

  // Loop terminates only via `worker.terminate()` (main's restart path).
  while (world !== null && ctrlI32 !== null) {
    const iterStart = performance.now();

    // ─── sim_worker.read_input_sab ────────────────────────────────────────
    // The worker's TS perf module is a separate instance from main's, so
    // worker-side `span()` calls never reach the perf panel. Route the
    // sim_worker.* spans through the always-on Rust profiler via
    // `world.record_profile_sample(...)`. Measure with `performance.now()`
    // for the wall-clock duration.
    const readStart = performance.now();
    readControlSab();
    const readEnd = performance.now();
    world.record_profile_sample(
      "sim_worker",
      "read_input_sab",
      Math.round((readEnd - readStart) * 1000),
      1,
    );

    // v2: don't park when `world.world_ended`. Once the population dies out
    // the sim drops into a thin grass-only path so the canvas keeps filling
    // while main shows the world-end popup. Only an explicit pause parks.
    if (paused) {
      // v2.1 P1: serve any pending inspect request (including kind=2 NN I/O)
      // before parking. Without this, a requestNnInspectId (or any inspect
      // request) issued while paused is silently dropped — the request epoch
      // advances past lastInspectReqEpoch in readControlSab's next iteration
      // but serveInspectRequest() lives in the running branch and never fires.
      // The futex notify from issueInspect wakes this Atomics.wait, so after
      // serve the loop re-reads control, sees paused still true, and parks again.
      serveInspectRequest();
      serveTelemetryRequest();
      serveWorldArtifactRequest();
      const before = Atomics.load(ctrlI32, CTRL_FUTEX);
      Atomics.wait(ctrlI32, CTRL_FUTEX, before, Infinity);
      continue;
    }

    // ─── sim_worker.tick ──────────────────────────────────────────────────
    const tickStart = performance.now();
    world.step_n(1);
    const tickEnd = performance.now();
    world.record_profile_sample(
      "sim_worker",
      "tick",
      Math.round((tickEnd - tickStart) * 1000),
      1,
    );
    tickIdx++;

    // ─── sim_worker.write_output_sab ──────────────────────────────────────
    const writeStart = performance.now();
    maybeWriteSnapshotToSAB();
    serveInspectRequest();
    maybeWriteProfileReport(tickIdx);
    maybeWriteNnStats(tickIdx);
    maybeWriteSpeciesTable(tickIdx);
    serveTelemetryRequest();
    serveWorldArtifactRequest();
    const writeEnd = performance.now();
    world.record_profile_sample(
      "sim_worker",
      "write_output_sab",
      Math.round((writeEnd - writeStart) * 1000),
      1,
    );

    // sim_worker root bracket: record total iter wall-clock so the parent
    // row has its own measurement (and children can be shown as % of parent).
    // Per the no-rollup rule in profiler.md — every visible row owns its own
    // measurement, not a sum of children.
    world.record_profile_sample(
      "sim_worker",
      "",
      Math.round((performance.now() - iterStart) * 1000),
      1,
    );

    // ─── Pacing ───────────────────────────────────────────────────────────
    const elapsed = performance.now() - iterStart;
    const sliceMs = 1000 / targetTPS;
    const remainingMs = sliceMs - elapsed;
    if (remainingMs > 0.25) {
      const before = Atomics.load(ctrlI32, CTRL_FUTEX);
      // Synchronous Atomics.wait: parks the OS thread for up to remainingMs,
      // or wakes early on Atomics.notify when main posts a slider / pause /
      // inspect SAB write. Burns zero CPU during the park.
      Atomics.wait(ctrlI32, CTRL_FUTEX, before, remainingMs);
    }
    // No else branch — when over budget, just continue. No yield needed:
    // no postMessage hot path → no event loop to feed.
  }
}
