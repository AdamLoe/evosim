// v1.6 Wave D (Stage 3): sim worker with `Atomics.waitAsync` pacing.
//
// Replaces Wave C's `setTimeout(0)` pacing with an async futex loop:
// `await Atomics.waitAsync(ctrl, CTRL_FUTEX, before, timeoutMs).value`. The
// `await` is what lets the worker event loop run between iterations so
// `onmessage` callbacks fire — i.e. sliders / pause / restart / inspect
// messages drain through `messageQueue` at the top of every iteration.
//
// **Synchronous `Atomics.wait` is forbidden** in this loop: it blocks the
// worker event loop and dark-holes every postMessage from main. See
// `docs/plans/v1.6-plan.md` §"Step D" for the canonical pseudocode.
//
// References:
//   - docs/plans/v1.6-plan.md §"Step D"
//   - docs/plans/v1.6-implementer-guide.md §"Wave D"

import init, {
  WorldHandle,
  max_pop_for_sim,
} from "../wasm/evosim";
import * as _wasmMod from "../wasm/evosim";
import {
  CONTROL_SAB_I32_LEN,
  CREATURE_SOA_BYTES,
  CTRL_CURRENT_SLOT,
  CTRL_FUTEX,
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

// Wave D: rayon thread-count probe. Always exported (returns 1 on non-threaded
// builds), so the cast is just to dodge TS narrowing through wildcard import.
const rayonCurrentNumThreads = (_wasmMod as unknown as Record<string, unknown>)[
  "rayon_current_num_threads"
] as (() => number) | undefined;

// ─── Worker-local state ─────────────────────────────────────────────────────

let world: WorldHandle | null = null;
let paused = false;
let targetTPS = 60;

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

  // Wave D footgun defense: even if `initThreadPool` resolved, rayon may have
  // silently collapsed to one worker (v1.5 shipped this bug — COOP/COEP fine
  // on dev but link-args dropped `--shared-memory` on the build). Probe the
  // actual `rayon::current_num_threads()` value and warn loudly so the
  // single-threaded mode is observable, not silent.
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
    max_pop_for_sim: max_pop_for_sim(),
    snapshot_sab: snapshotSab,
    control_sab: controlSab,
  };
  post(reply);

  // Kick off the sim loop. The async loop body awaits `Atomics.waitAsync`
  // between iterations, which lets the worker event loop run and `onmessage`
  // callbacks fill `messageQueue` — Wave D's central correctness property.
  void simLoop();
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
      return;
    case "set_paused":
      paused = msg.paused;
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

// ─── Async tick loop (Wave D `Atomics.waitAsync` pacing) ───────────────────
//
// THE most important detail in v1.6: this loop body MUST be `async` and the
// pacing primitive MUST be `Atomics.waitAsync` (NOT `Atomics.wait`).
// Synchronous `Atomics.wait` inside a `while` loop blocks the worker event
// loop, which means `onmessage` callbacks never fire — every slider, pause,
// restart, and inspect message dark-holes silently. The `await` on the
// `waitAsync` promise is what allows the event loop to run between
// iterations so messages reach `messageQueue` before the next `drainMessages`.
//
// `messageQueue` is filled by the top-of-file `self.onmessage` handler;
// `drainMessages` runs at the TOP of every iteration so a slider sent at
// tick T takes effect for tick T+1 deterministically (slider drain ordering,
// v1.6-plan.md locked decision 13).

async function simLoop(): Promise<void> {
  // Loop terminates only via `worker.terminate()` (main's restart path).
  // No "terminated" flag — the worker is destroyed wholesale on restart.
  // We must guard on `world` because boot may not have completed before the
  // first message arrives (race-safe: drainMessages returns early then too).
  while (world !== null && ctrlI32 !== null) {
    drainMessages();

    const iterStart = performance.now();
    const ended = world.world_ended;

    if (!paused && !ended) {
      // Mission §"Stage 1": one tick, one snapshot. The renderer always reads
      // the latest SAB slot at RAF rate; per-tick snapshots give it the
      // freshest view it can possibly have. v1.6 Wave D originally shipped a
      // `step_n(floor(budget))` accumulator lifted from main.ts pre-decoupling
      // — but main.ts batched because it shared the render thread, and the
      // worker doesn't, so batching here only hurt snapshot freshness (at
      // 8000 pop the SAB flipped every ~1.2s; the renderer repainted the same
      // stale slot in between).
      world.step_n(1);
      writeSnapshotToSAB();
    }

    // Pacing wait. When paused, sleep indefinitely so we don't burn CPU on
    // snapshot churn nobody's reading — main's `set_paused(false)` notifies
    // the futex via SimBridge.postMessage to wake us. When running, sleep for
    // whatever target-TPS slice remains after this tick. If the tick itself
    // overran the slice (high pop), elapsed already exceeds the budget and
    // timeoutMs falls through at 0 — the next tick fires immediately, and we
    // naturally underrun targetTPS.
    const elapsedThisIter = performance.now() - iterStart;
    const timeoutMs = paused
      ? Infinity
      : Math.max(0, 1000 / targetTPS - elapsedThisIter);
    const before = Atomics.load(ctrlI32, CTRL_FUTEX);
    const r = Atomics.waitAsync(ctrlI32, CTRL_FUTEX, before, timeoutMs);
    if (r.async) {
      // Standard path: park until a notify, a futex mutation, or the timeout
      // elapses. The `await` is the load-bearing event-loop yield — without
      // it, `onmessage` would never fire.
      await r.value;
    }
    // r.async === false ⇒ "not-equal" (main mutated CTRL_FUTEX between our
    // `Atomics.load(before)` and `waitAsync`) — fall straight through to the
    // next iteration; the message that bumped the futex is already in the
    // queue and `drainMessages` will pick it up.
  }
}

// Entrypoint: boot is dispatched via the top-of-file `self.onmessage` handler.
// Wave B restart is implemented via worker.terminate() + new Worker —
// there's no re-boot on a live worker.
