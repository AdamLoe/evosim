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
} from "../wasm/evosim";
import * as _wasmMod from "../wasm/evosim";
import {
  CTRL_CURRENT_SLOT,
  CTRL_FUTEX,
  CTRL_SEQ,
  type SimMessage,
  type SimMessageBoot,
  type SimReply,
} from "./sim-bridge";
import {
  CONTROL_SAB_BYTES,
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
  CTRL_TARGET_TPS_BITS,
  INSPECT_RESP_CAP,
  INSPECT_RESP_OFFSET,
  NN_STATS_CAP,
  NN_STATS_OFFSET,
  PROFILE_REPORT_CAP,
  PROFILE_REPORT_OFFSET,
} from "./generated/control-sab";
import { SLIDER_COUNT, SLIDER_NAMES } from "./generated/slider-ids";

// initThreadPool is only exported when wasm is built with --features threads.
const initThreadPool = (_wasmMod as unknown as Record<string, unknown>)[
  "initThreadPool"
] as ((n: number) => Promise<void>) | undefined;
const rayonCurrentNumThreads = (_wasmMod as unknown as Record<string, unknown>)[
  "rayon_current_num_threads"
] as (() => number) | undefined;

/** Target rayon worker count (kept from v1.9; see worker-runtime.md). */
const TARGET_RAYON_WORKERS = 8;

/** Tick cadence for emitting the profile-report payload (~1 Hz at 60 TPS). */
const PROFILE_REPORT_EVERY_N_TICKS = 60;
/** Tick cadence for emitting the NN-stats payload (~750 ms at 60 TPS). */
const NN_STATS_EVERY_N_TICKS = 45;

// ─── Worker-local state ─────────────────────────────────────────────────────

let world: WorldHandle | null = null;
let booted = false;

let controlSab: SharedArrayBuffer | null = null;
let ctrlI32: Int32Array | null = null;
let ctrlF32: Float32Array | null = null;
let ctrlBytes: Uint8Array | null = null;

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
// Last profile window_ms value we applied to the Rust profiler. The control
// SAB carries the desired window each tick; we forward to wasm only when
// it actually changes so we don't burn a wasm call every iter.
let lastProfileWindowMs = 0;
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
      console.error("[sim] boot failed:", err);
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

  world = WorldHandle.newWithFounderCount(
    boot.seed,
    boot.initial_grass_seed_count,
    boot.energy_max,
    boot.founder_count,
    boot.full_grass_on_init,
  );
  world.profile_enable(true);

  // Apply persisted slider state by name (initial_sliders is name→value).
  // After this point the canonical values live in the SAB; main writes only
  // through the SAB transport for the rest of the worker's lifetime.
  for (const [name, value] of Object.entries(boot.initial_sliders)) {
    try {
      world.set_slider(name, value);
    } catch (err) {
      console.warn(`[sim] set_slider("${name}", ${value}) rejected:`, err);
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
  // Mirror the just-applied slider values into the SAB so main sees the
  // canonical state on first read.
  for (let i = 0; i < SLIDER_COUNT; i++) {
    const name = SLIDER_NAMES[i];
    const v = boot.initial_sliders[name];
    if (v !== undefined) {
      ctrlF32[CTRL_SLIDERS_BASE + i] = v;
    }
  }
  // Stamp the epoch counters so our first read of the loop is a no-op.
  lastControlEpoch = Atomics.load(ctrlI32, CTRL_CONTROL_EPOCH);
  lastProfileClearEpoch = Atomics.load(ctrlI32, CTRL_PROFILE_CLEAR_EPOCH);
  lastResetJankEpoch = Atomics.load(ctrlI32, CTRL_RESET_JANK_EPOCH);
  lastInspectReqEpoch = Atomics.load(ctrlI32, CTRL_INSPECT_REQ_EPOCH);

  // First-paint handshake: run one tick + one snapshot before posting
  // boot_ready so main's first RAF sees a live slot.
  world.step_n(1);
  writeSnapshotToSAB();

  // v1.11 (A): hand main the wasm memory + snapshot byte offset/len so it
  // can build views over `wasm.memory.buffer` directly. WebAssembly.Memory
  // with shared memory enabled round-trips through postMessage and the
  // underlying SharedArrayBuffer is observed identically on both threads.
  const reply: SimReply = {
    kind: "boot_ready",
    world_size: world.world_size,
    grass_dim: world.grass_dim,
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

  simLoop();
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
    const jsonStr = world.creature_inspect_json(idx);
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

// ─── Snapshot write ─────────────────────────────────────────────────────────
//
// v1.11 (A+D): the snapshot region lives in wasm linear memory. The worker
// just tells Rust which slot to write into; Rust writes f32 grass + creature
// SoA + stats header directly into its own `Vec<u8>` (no JS boundary). Main
// reads via a view over `wasm.memory.buffer` at the offset shared in boot.

function writeSnapshotToSAB(): void {
  if (!world || !ctrlI32) return;
  const current = Atomics.load(ctrlI32, CTRL_CURRENT_SLOT);
  const inactive: 0 | 1 = current === 0 ? 1 : 0;

  world.write_snapshot(inactive);

  // Publish: flip slot, then bump seq (store-before-add).
  Atomics.store(ctrlI32, CTRL_CURRENT_SLOT, inactive);
  Atomics.add(ctrlI32, CTRL_SEQ, 1);
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

    const ended = world.world_ended;

    if (paused || ended) {
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
    writeSnapshotToSAB();
    serveInspectRequest();
    maybeWriteProfileReport(tickIdx);
    maybeWriteNnStats(tickIdx);
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
