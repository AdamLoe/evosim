// v1.6 Wave B (Stage 1): sim worker.
//
// Holds the wasm `WorldHandle` and the tick loop. Main thread holds no wasm
// after this commit — it only renders snapshots posted from here.
//
// Stage 1 pacing is `setTimeout(0)`: simple, correct, caps high-TPS accuracy
// at ~250 TPS due to Chrome's 4 ms minimum (Wave D replaces with
// `Atomics.waitAsync` to fix that).
//
// References:
//   - docs/plans/v1.6-plan.md §"Step B"
//   - docs/plans/v1.6-implementer-guide.md §"Wave B"

import init, {
  WorldHandle,
  max_pop_for_sab,
} from "../wasm/evosim";
import * as _wasmMod from "../wasm/evosim";
import type {
  SimMessage,
  SimMessageBoot,
  SimReply,
  SimReplySnapshot,
} from "./sim-bridge";

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
  // §"Step B" — `set_slider(name, value)` is the sole entry point post-cutover,
  // no per-typed `set_*` calls (including auto_curriculum, which rides this
  // path as 0|1 via the new `try_set_slider` arm landed in this commit).
  for (const [name, value] of Object.entries(boot.initial_sliders)) {
    try {
      world.set_slider(name, value);
    } catch (err) {
      // An unknown name throws; surface it but keep booting so a stale
      // localStorage key doesn't brick the sim.
      console.warn(`[sim] set_slider("${name}", ${value}) rejected:`, err);
    }
  }

  // First-paint handshake (gotcha 1): run one tick + post one snapshot BEFORE
  // boot_ready so main's first RAF sees a valid snapshot. Skipping this makes
  // the first frame see `latestSnapshot === undefined` → crash or pop=0 ghost.
  world.step_n(1);
  postSnapshot();

  const reply: SimReply = {
    kind: "boot_ready",
    world_size: world.world_size,
    grass_dim: world.grass_dim,
    threads,
    rayon_ok: rayonOk,
    max_pop_for_sab: max_pop_for_sab(),
    snapshot_sab: null, // Stage 1: postMessage; Wave C populates.
    control_sab: null,
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

// ─── Tick loop ──────────────────────────────────────────────────────────────

function postSnapshot(): void {
  if (!world) return;
  // Note: world.creatures_buffer() / creature_ids_buffer() return typed-array
  // VIEWS over wasm linear memory (which is a SharedArrayBuffer under the
  // threaded build). Structured clone shares the underlying SAB, so we
  // explicitly copy into a fresh (non-shared) ArrayBuffer first — otherwise
  // a Stage 1 render-vs-next-tick race would corrupt mid-frame reads.
  // `grass_buffer_u8` already returns a fresh `Uint8Array::from(&[...])`
  // (i.e. a copy) on the Rust side, so it can be sent as-is.
  // (gotcha 11)
  const creaturesView = world.creatures_buffer();
  const creaturesBytes = new Uint8Array(creaturesView.byteLength);
  creaturesBytes.set(
    new Uint8Array(creaturesView.buffer, creaturesView.byteOffset, creaturesView.byteLength),
  );
  const idsView = world.creature_ids_buffer();
  const idsCopy = new Float64Array(idsView.length);
  idsCopy.set(idsView);
  const snapshot: SimReplySnapshot = {
    kind: "snapshot",
    tick: world.tick,
    pop: world.population,
    tps: world.tps,
    world_ended: world.world_ended,
    jank_count: world.jank_count,
    creatures: creaturesBytes,
    grass: world.grass_buffer_u8(),
    ids: idsCopy,
  };
  post(snapshot);
}

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
    postSnapshot();
  } else if (paused || ended) {
    // No tick this iter, but we still want main to see fresh paused/ended
    // state if it just toggled. Cheap; only fires when ticksThisIter === 0.
    postSnapshot();
  }

  scheduleNext();
}

function scheduleNext(): void {
  // Stage 1 pacing: setTimeout(0). Wave D replaces with `Atomics.waitAsync`.
  setTimeout(step, 0);
}

// Entrypoint: boot is dispatched via the top-of-file `self.onmessage` handler.
// Wave B restart is implemented via worker.terminate() + new Worker —
// there's no re-boot on a live worker.
