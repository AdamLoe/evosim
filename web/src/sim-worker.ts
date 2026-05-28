// v1.6 Wave C (Stage 2): sim worker with SharedArrayBuffer snapshots.
//
// Replaces Wave B's per-batch `snapshot` postMessage (and the explicit
// non-shared-ArrayBuffer copy workaround that postMessage required) with a
// double-buffered SAB write. The sim worker writes the inactive slot, flips
// `CTRL_CURRENT_SLOT`, then bumps `CTRL_SEQ` so main's render loop can detect
// a fresh snapshot. Zero structured-clone, zero copy.
//
// Pacing remains `setTimeout(0)` for Wave C — Wave D introduces
// `Atomics.waitAsync` for accurate high-TPS pacing.
//
// References:
//   - docs/plans/v1.6-plan.md §"Step C"
//   - docs/plans/v1.6-implementer-guide.md §"Wave C"

import init, {
  WorldHandle,
  max_pop_for_sab,
} from "../wasm/evosim";
import * as _wasmMod from "../wasm/evosim";
import {
  CONTROL_SAB_I32_LEN,
  CREATURE_SOA_BYTES,
  CTRL_CURRENT_SLOT,
  CTRL_SEQ,
  GRASS_BYTES,
  SNAPSHOT_HEADER_BYTES,
  SNAPSHOT_SAB_BYTES,
  creatureSoAOffset,
  grassOffset,
  slotOffset,
  type SimMessage,
  type SimMessageBoot,
  type SimReply,
} from "./sim-bridge";
import { span } from "./perf";

// initThreadPool is only exported when wasm is built with --features threads.
// Cast via unknown to avoid TS2614 on non-threaded builds.
const initThreadPool = (_wasmMod as unknown as Record<string, unknown>)[
  "initThreadPool"
] as ((n: number) => Promise<void>) | undefined;

// ─── Worker-local state ─────────────────────────────────────────────────────

let world: WorldHandle | null = null;
let paused = false;
let targetTPS = 60;
let tickBudget = 0;
let lastLoopMs = 0;

const MAX_TICKS_PER_BATCH = 2000;
const MAX_FRAME_DELTA_MS = 100;

// Pending main → worker messages drained at the top of each loop iteration.
const messageQueue: SimMessage[] = [];
let booted = false;

// SAB handles allocated at boot. Sized once and shared with main via
// `boot_ready`. `ctrlI32` is the Int32Array view over `controlSab` used for
// the slot-flip atomics. The snapshot SAB is reused across both slots.
let controlSab: SharedArrayBuffer | null = null;
let snapshotSab: SharedArrayBuffer | null = null;
let ctrlI32: Int32Array | null = null;

self.onmessage = (e: MessageEvent<SimMessage>): void => {
  const msg = e.data;
  if (!booted && msg.kind === "boot") {
    booted = true;
    handleBoot(msg).catch((err) => {
      console.error("[sim] boot failed:", err);
    });
    return;
  }
  // Either the boot is already complete (handle in loop) or non-boot arrived
  // before boot did — queue either way; drainMessages handles the no-world
  // case by returning early.
  messageQueue.push(msg);
};

function post(reply: SimReply): void {
  // Cast through unknown because `SharedArrayBuffer | null` upsets the lib
  // dom's WindowPostMessageOptions overload typing for Worker.postMessage.
  (self as unknown as { postMessage: (m: unknown) => void }).postMessage(reply);
}

// ─── Bootstrap ──────────────────────────────────────────────────────────────

async function handleBoot(boot: SimMessageBoot): Promise<void> {
  await init();

  // Read isolation directly from the JS global — no Rust export.
  // Per v1.6-plan.md plan-review C3 + implementer guide gotcha 2.
  const isolated =
    (self as unknown as { crossOriginIsolated?: boolean }).crossOriginIsolated ?? false;
  console.log(`[sim] crossOriginIsolated=${isolated}`);

  let rayonOk = false;
  let threads = 1;
  if (initThreadPool && isolated) {
    try {
      const hwConc = navigator.hardwareConcurrency;
      await initThreadPool(hwConc);
      threads = hwConc;
      rayonOk = true;
      console.log(`[sim] rayon workers: ${hwConc}`);
    } catch (err) {
      console.warn("[sim] initThreadPool failed; continuing single-threaded:", err);
    }
  } else if (!isolated) {
    console.warn(
      "[sim] not cross-origin isolated; rayon disabled (single-threaded sim).",
    );
  }

  world = WorldHandle.newWithFounderCount(
    boot.seed,
    boot.initial_grass_seed_count,
    boot.energy_max,
    boot.founder_count,
  );

  // Apply every persisted slider via the name-dispatcher. Per v1.6-plan.md
  // §"Step C": per-typed `set_*` wasm exports are gone; `set_slider(name,
  // value)` is the sole entry point. Bools ride the same path as 0|1 via the
  // `"auto_curriculum"` arm in `try_set_slider`.
  for (const [name, value] of Object.entries(boot.initial_sliders)) {
    try {
      world.set_slider(name, value);
    } catch (err) {
      // An unknown name throws; surface it but keep booting so a stale
      // localStorage key doesn't brick the sim.
      console.warn(`[sim] set_slider("${name}", ${value}) rejected:`, err);
    }
  }

  // Allocate the two SABs once at boot. They live for the lifetime of this
  // worker; main keeps the handles in closures so they're GC-rooted until the
  // worker (and its closure references) are dropped on restart.
  controlSab = new SharedArrayBuffer(CONTROL_SAB_I32_LEN * 4);
  snapshotSab = new SharedArrayBuffer(SNAPSHOT_SAB_BYTES);
  ctrlI32 = new Int32Array(controlSab);

  // First-paint handshake (gotcha 1, retained from Wave B): run one tick and
  // write one snapshot to slot 0 BEFORE posting boot_ready, so main's first
  // RAF sees a valid live slot. The control word stays 0 so that initial
  // snapshot is the live slot.
  world.step_n(1);
  writeSnapshotToSAB();

  const reply: SimReply = {
    kind: "boot_ready",
    world_size: world.world_size,
    grass_dim: world.grass_dim,
    threads,
    rayon_ok: rayonOk,
    max_pop_for_sab: max_pop_for_sab(),
    snapshot_sab: snapshotSab,
    control_sab: controlSab,
  };
  post(reply);

  // Kick off the sim loop.
  lastLoopMs = performance.now();
  scheduleNext();
}

// ─── Message dispatch ───────────────────────────────────────────────────────

function handle(msg: SimMessage): void {
  if (!world) return; // boot must arrive first; ignore anything else.
  switch (msg.kind) {
    case "boot":
      // Already booted; spurious reboot shouldn't happen (main respawns the
      // whole worker on restart). Log and ignore.
      console.warn("[sim] received boot after boot — ignoring.");
      return;
    case "set_slider":
      try {
        world.set_slider(msg.name, msg.value);
      } catch (err) {
        console.warn(`[sim] set_slider("${msg.name}", ${msg.value}) rejected:`, err);
      }
      return;
    case "set_target_tps":
      targetTPS = msg.tps;
      tickBudget = 0;
      return;
    case "set_paused":
      paused = msg.paused;
      tickBudget = 0;
      return;
    case "profile_enable":
      world.profile_enable(msg.on);
      return;
    case "reset_jank":
      world.reset_jank();
      return;
    case "inspect_at": {
      const id = world.creature_at(msg.wx, msg.wy, msg.tolerance_world);
      let json: string | null = null;
      if (id !== undefined && id !== null) {
        const idx = world.creature_idx_by_id(id);
        if (idx !== undefined) {
          const j = world.creature_inspect_json(idx);
          json = j ?? null;
        }
      }
      post({ kind: "inspect_reply", request_id: msg.request_id, json });
      return;
    }
    case "inspect_id": {
      const idx = world.creature_idx_by_id(msg.id);
      let json: string | null = null;
      if (idx !== undefined) {
        const j = world.creature_inspect_json(idx);
        json = j ?? null;
      }
      post({ kind: "inspect_reply", request_id: msg.request_id, json });
      return;
    }
    case "request_nn_stats": {
      const json = world.nn_worker_stats_json();
      post({ kind: "nn_stats_reply", request_id: msg.request_id, json });
      return;
    }
    case "request_profile_report": {
      // Bundle into one reply so main does not need 4 round-trips per second.
      const inner = world.profile_report_json();
      const bundled = JSON.stringify({
        profile: JSON.parse(inner),
        tps: world.tps,
        jank_count: world.jank_count,
        live_grass_cell_count: world.live_grass_cell_count(),
        total_grass_density: world.total_grass_density(),
      });
      post({ kind: "profile_reply", request_id: msg.request_id, json: bundled });
      return;
    }
  }
}

function drainMessages(): void {
  while (messageQueue.length > 0) {
    const next = messageQueue.shift()!;
    handle(next);
  }
}

// ─── Snapshot write (Wave C SAB path) ───────────────────────────────────────

/**
 * Atomic-flip ordering, per implementer-guide gotcha 2:
 *   1. read `current` (live slot)
 *   2. write inactive slot fully (creatures + grass + stats)
 *   3. `Atomics.store(ctrl, CTRL_CURRENT_SLOT, 1 - current)`  ← flip
 *   4. `Atomics.add(ctrl, CTRL_SEQ, 1)`                       ← seq bump
 *
 * Store-before-add gives main's render loop a clean read: if seq changed,
 * current_slot is already coherent. The whole region the inactive slot
 * occupies is owned by the worker between the previous flip and the next, so
 * no reader sees a torn write.
 *
 * The TS-side `worker.snapshot.write` span wraps only the `write_snapshot_to`
 * wasm call — that's the cost the orchestrator is measuring. The flip itself
 * is two atomic ops, well below the noise floor of any span.
 */
function writeSnapshotToSAB(): void {
  if (!world || !ctrlI32 || !snapshotSab) return;
  const current = Atomics.load(ctrlI32, CTRL_CURRENT_SLOT);
  const inactive: 0 | 1 = current === 0 ? 1 : 0;

  // Build three Uint8Array views over the inactive slot's regions. Each view
  // aliases a fixed byte range inside the shared snapshot SAB — Rust writes
  // straight through them via `Uint8Array::copy_from`.
  const headerOff = slotOffset(inactive);
  const creaturesOff = creatureSoAOffset(inactive);
  const grassOff = grassOffset(inactive);

  const statsView = new Uint8Array(snapshotSab, headerOff, SNAPSHOT_HEADER_BYTES);
  const creaturesView = new Uint8Array(snapshotSab, creaturesOff, CREATURE_SOA_BYTES);
  const grassView = new Uint8Array(snapshotSab, grassOff, GRASS_BYTES);

  const writeSpan = span("worker.snapshot.write");
  try {
    world.write_snapshot_to(creaturesView, grassView, statsView);
  } finally {
    writeSpan.close();
  }

  // Publish: flip slot, then bump seq (store-before-add per gotcha 2).
  Atomics.store(ctrlI32, CTRL_CURRENT_SLOT, inactive);
  Atomics.add(ctrlI32, CTRL_SEQ, 1);
}

// ─── Tick loop ──────────────────────────────────────────────────────────────

function step(): void {
  if (!world) return;
  drainMessages();

  const now = performance.now();
  const rawDelta = now - lastLoopMs;
  lastLoopMs = now;
  const delta = Math.min(rawDelta, MAX_FRAME_DELTA_MS);

  let ticksThisIter = 0;
  const ended = world.world_ended;
  if (paused || ended) {
    tickBudget = 0;
  } else {
    tickBudget += targetTPS * (delta / 1000);
    if (tickBudget > MAX_TICKS_PER_BATCH) tickBudget = MAX_TICKS_PER_BATCH;
    ticksThisIter = Math.floor(tickBudget);
    tickBudget -= ticksThisIter;
  }

  if (ticksThisIter > 0) {
    world.step_n(ticksThisIter);
    writeSnapshotToSAB();
  } else if (paused || ended) {
    // No tick this iter, but write a fresh snapshot anyway so main sees the
    // most recent paused/ended state without staring at stale data. Cheap;
    // only fires when ticksThisIter === 0.
    writeSnapshotToSAB();
  }

  scheduleNext();
}

function scheduleNext(): void {
  // Stage 2 pacing: setTimeout(0). Wave D replaces with `Atomics.waitAsync`.
  setTimeout(step, 0);
}

// Entrypoint: boot is dispatched via the top-of-file `self.onmessage` handler.
// Wave B restart is implemented via worker.terminate() + new Worker —
// there's no re-boot on a live worker.
