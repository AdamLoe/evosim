import init, { WorldHandle, creature_stride } from "../wasm/evosim";
// initThreadPool is only exported when wasm is built with --features threads.
// Cast via unknown to avoid TS2614 on non-threaded builds.
import * as _wasmMod from "../wasm/evosim";
const initThreadPool = (_wasmMod as unknown as Record<string, unknown>)["initThreadPool"] as
  | ((n: number) => Promise<void>)
  | undefined;
import { makeCamera, renderWorld } from "./render";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installProfilerPanel } from "./widgets/perf-panel";
import { installDevPanel, getInitialGrassSeedCount } from "./widgets/devpanel";
import { installCanvasClickHandler } from "./rail/inspector";
import { PersistenceClient } from "./persistence";
import { showResumePrompt, showSchemaMismatchModal } from "./persistence/ui";
import { showAutosaveFailureToast } from "./persistence/toast";
import { showEulogyCard } from "./eulogy";
import { attachProfiler, timed, span } from "./perf";

const status = document.getElementById("status") as HTMLSpanElement;
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const ctx = canvas.getContext("2d");
if (!ctx) throw new Error("2d context unavailable");

let viewW = 0;
let viewH = 0;
function resize(): void {
  // S22: cap DPR at 2 to avoid 3× pixel overdraw on 3× displays.
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  viewW = window.innerWidth;
  viewH = window.innerHeight;
  canvas.width = Math.floor(viewW * dpr);
  canvas.height = Math.floor(viewH * dpr);
  canvas.style.width = `${viewW}px`;
  canvas.style.height = `${viewH}px`;
  ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
}
window.addEventListener("resize", resize);
resize();

type Speed = 0 | 1 | 10 | 100;
let speed: Speed = 1;

// F.26: autosave parameters (v5 §13, F.26 DECISIONS).
// AUTOSAVE_WALL_INTERVAL_MS is the primary throttle — one save per 5 minutes.
// AUTOSAVE_TICK_INTERVAL is a minor guard that prevents saving twice in the
// same sim tick (e.g. if the frame loop fires very fast in tests).
const AUTOSAVE_TICK_INTERVAL = 100;
const AUTOSAVE_WALL_INTERVAL_MS = 5 * 60 * 1000; // 5 minutes
let lastSavedTick = -1;
let lastSavedWallMs = 0;

// F.26 + v6 §C: camera state is persisted alongside the wasm save so resume
// restores zoom + pan. We pass camera through to the worker as a separate
// structured-cloned field rather than wrapping the (large) wasm JSON in an
// envelope — wrapping previously cost a JSON.parse + JSON.stringify of the
// full save (brains × creatures) on the main thread every autosave tick,
// producing a ~hundreds-of-ms pulse every ~3 s.
//
// Legacy saves written before this change used `{ wasm, camera }` envelope
// JSON in `row.json`; unwrapLegacySaveEnvelope below preserves load-side
// backward compatibility for those rows.
type CameraState = { zoom: number; cx: number; cy: number };

function unwrapLegacySaveEnvelope(json: string): {
  wasmJson: string;
  camera: CameraState | null;
} {
  // Cheap pre-check: a bare SaveV1 starts with `{"schema_version":` so we
  // avoid parsing the (potentially MB-scale) JSON unless it really looks
  // like the old envelope (`{"wasm":…}`).
  if (!json.startsWith("{\"wasm\"") && !json.startsWith("{ \"wasm\"")) {
    return { wasmJson: json, camera: null };
  }
  try {
    const parsed = JSON.parse(json) as { wasm?: unknown; camera?: CameraState };
    if (parsed && "wasm" in parsed && parsed.wasm) {
      return { wasmJson: JSON.stringify(parsed.wasm), camera: parsed.camera ?? null };
    }
    return { wasmJson: json, camera: null };
  } catch {
    return { wasmJson: json, camera: null };
  }
}

function maybeAutosave(
  world: WorldHandle,
  cam: CameraState,
  nowMs: number,
  persistence: PersistenceClient,
): void {
  const t = world.tick;
  // Primary gate: wall-clock interval (5 min). Tick guard only prevents a
  // double-save at the exact same tick (e.g. rapid frame bursts in tests).
  if (t === lastSavedTick) return;
  if (lastSavedTick >= 0 && t - lastSavedTick < AUTOSAVE_TICK_INTERVAL) return;
  if (nowMs - lastSavedWallMs < AUTOSAVE_WALL_INTERVAL_MS) return;
  // snapshot_json() is the unavoidable wasm-side serde cost (~tens of ms);
  // passing the string straight to the worker keeps the main thread off the
  // parse/stringify path entirely.
  persistence.save(world.snapshot_json(), world.seed, t, {
    zoom: cam.zoom,
    cx: cam.cx,
    cy: cam.cy,
  });
  lastSavedTick = t;
  lastSavedWallMs = nowMs;
}

// F.28: eulogy gate — shown exactly once per session.
let eulogyShown = false;

// F.27: seed display + copy button.
function installSeedDisplay(seed: string): void {
  const valueEl = document.getElementById("seed-value");
  if (valueEl) valueEl.textContent = seed;
  const btn = document.getElementById("seed-copy-btn") as HTMLButtonElement | null;
  if (btn) {
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(seed);
        btn.textContent = "copied";
        setTimeout(() => (btn.textContent = "copy"), 1200);
      } catch (e) {
        console.warn("clipboard copy failed", e);
      }
    });
  }
}

async function main(): Promise<void> {
  await init();

  // Threads: spin up the rayon worker pool BEFORE any WorldHandle is
  // constructed so the first tick gets parallelism. SAB feature-detect:
  // browsers without SharedArrayBuffer skip initThreadPool — the wasm
  // still works, just sequentially (rayon falls back to the calling
  // thread when no workers are registered). See docs/plans/perf-4-threads.md.
  if (typeof SharedArrayBuffer !== "undefined" && initThreadPool) {
    try {
      await initThreadPool(navigator.hardwareConcurrency);
    } catch (e) {
      console.warn("initThreadPool failed; continuing single-threaded:", e);
    }
  } else {
    console.warn("SharedArrayBuffer not available; running single-threaded");
  }

  // F.26: persistence client.
  const persistence = new PersistenceClient();

  // Saving indicator: shown while the worker round-trip is in flight.
  const saveIndicator = document.getElementById("save-indicator") as HTMLSpanElement | null;
  function showSaveIndicator(): void {
    saveIndicator?.classList.add("is-saving");
  }
  function hideSaveIndicator(): void {
    saveIndicator?.classList.remove("is-saving");
  }

  persistence.setHandlers({
    onSaving: showSaveIndicator,
    onSaved: () => hideSaveIndicator(),
    onError: (_code, _msg) => {
      hideSaveIndicator();
      showAutosaveFailureToast();
    },
  });

  const params = new URLSearchParams(window.location.search);
  // Cap seed param length to prevent oversized inputs (S15).
  const urlSeed = (params.get("seed") ?? "").slice(0, 128) || null;

  let world: WorldHandle | null = null;
  let restoredCamera: { zoom: number; cx: number; cy: number } | null = null;

  // F.26: boot-path resume logic.
  if (!urlSeed) {
    // No explicit seed → probe for a saved world.
    try {
      const row = await persistence.loadCurrent();
      if (row) {
        const DAY_TICKS = 1000;
        const day = Math.floor(row.tick / DAY_TICKS);
        const userChose = await showResumePrompt({
          day,
          seed: row.seed,
          savedAtMs: row.saved_at_ms,
        });
        if (userChose === "resume") {
          try {
            // New saves: row.json is the raw wasm save string and row.camera
            // is a separate field. Legacy saves: row.camera is undefined and
            // row.json may still be the `{wasm,camera}` envelope — unwrap it.
            let wasmJson = row.json;
            let camera = row.camera ?? null;
            if (!camera) {
              const unwrapped = unwrapLegacySaveEnvelope(row.json);
              wasmJson = unwrapped.wasmJson;
              camera = unwrapped.camera;
            }
            world = WorldHandle.fromJson(wasmJson);
            restoredCamera = camera;
          } catch (e) {
            const msg = String(e);
            if (msg.startsWith("schema-mismatch:")) {
              await showSchemaMismatchModal();
            } else {
              console.warn("load failed (treating as unusable save):", msg);
              await showSchemaMismatchModal();
            }
            // Clear stale IDB record so the next boot goes straight to fresh.
            await persistence.deleteCurrent();
            // Fall through to fresh world.
          }
        }
        // userChose === "new" → fall through to fresh world.
      }
    } catch (e) {
      // Worker error at load time — log and proceed fresh (no toast; user wasn't expecting feedback).
      console.warn("resume probe failed:", e);
    }
  }

  if (!world) {
    world = WorldHandle.newWithGrassSeed(urlSeed ?? "", getInitialGrassSeedCount());
  }

  // S22: cache seed once per world lifetime (world.seed is a getter that
  // allocates a new String each call; no need to call it per frame).
  const cachedSeed = world.seed;

  // F.27: seed display.
  installSeedDisplay(cachedSeed);

  // F.26: flush a save when the user navigates away so we don't lose up to
  // 5 minutes of sim. The postMessage to the worker is fire-and-forget —
  // modern browsers give service workers ~a few seconds on beforeunload but
  // a dedicated worker gets no such grace period guarantee. We attempt it
  // anyway; partial success is better than none. We do NOT call
  // event.preventDefault() / returnValue so we never block navigation.
  window.addEventListener("beforeunload", () => {
    if (world && !world.world_ended) {
      persistence.save(world.snapshot_json(), cachedSeed, world.tick, {
        zoom: cam.zoom,
        cx: cam.cx,
        cy: cam.cy,
      });
    }
  });

  const stride = creature_stride();
  const cam = makeCamera(world.world_size);
  if (restoredCamera) {
    cam.zoom = restoredCamera.zoom;
    cam.cx = restoredCamera.cx;
    cam.cy = restoredCamera.cy;
  }
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => world!.world_size,
  );

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop ${world.population}`;

  // Speed buttons in the top bar.
  installSpeedControls();

  // E.21: install right rail.
  const rail = installRail(world);

  // E.24: canvas click → inspector.
  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), world, rail);

  // Profiler: attach world after construction so the TS profiler can
  // forward enable/disable calls to the Rust side (D1, D9).
  attachProfiler(world);

  // Perf-timing: install the Stats-panel toggle + 1Hz polling loop.
  installProfilerPanel(world);

  // P3b: dev panel overlay (7 sliders, ~ hotkey, ⚙ button).
  installDevPanel(world);

  // Sim + render loop.
  let lastRender = performance.now();
  // S22: throttle status DOM updates to 5 Hz (200 ms gate).
  let lastStatusUpdate = 0;
  function frame(now: number): void {
    const frameSpan = span("frame");
    try {
      const delta = now - lastRender;
      lastRender = now;
      const ticksThisFrame =
        speed === 0 ? 0 : Math.min(200, Math.max(1, Math.round((speed * delta) / 16.66)));

      // S22: hoist world_ended once per RAF frame (was called 3× per frame).
      const ended = world!.world_ended;

      if (ticksThisFrame > 0 && !ended) {
        timed("step_n", () => world!.step_n(ticksThisFrame));
      }

      // F.26: autosave after each frame batch.
      if (!ended) {
        timed("maybeAutosave", () => maybeAutosave(world!, cam, now, persistence));
      }

      // F.28: eulogy — fire once when world ends.
      if (ended && !eulogyShown) {
        eulogyShown = true;
        showEulogyCard(world!, persistence);
      }

      // Fetch ids buffer once per frame (index-aligned with creatures_buffer).
      const ids = world!.creature_ids_buffer() as unknown as Float64Array;

      // E.21/E.22/E.23/E.24: poll the rail (events, toasts, highlights, stats, inspector).
      // S18: ids no longer passed to pollRail (inspector uses creature_idx_by_id instead).
      timed("pollRail", () => pollRail(rail, world!));

      timed("renderWorld", () =>
        renderWorld(ctx!, cam, viewW, viewH, world!, stride, ids, highlights, now));

      // S22: throttle status DOM updates to 5 Hz (200 ms gate).
      if (now - lastStatusUpdate > 200) {
        lastStatusUpdate = now;
        const endedSuffix = ended ? "  (world ended)" : "";
        status.textContent =
          `seed: ${cachedSeed}  ·  tick ${world!.tick}  ·  pop ${world!.population}${endedSuffix}`;
      }
    } finally {
      frameSpan.close();
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

function installSpeedControls(): void {
  const bar = document.getElementById("top-bar")!;
  const wrap = document.createElement("span");
  wrap.style.marginLeft = "auto";
  for (const s of [0, 1, 10, 100] as const) {
    const btn = document.createElement("button");
    btn.textContent = s === 0 ? "pause" : `${s}x`;
    btn.style.marginLeft = "4px";
    btn.style.background = "rgba(255,255,255,0.08)";
    btn.style.color = "var(--fg)";
    btn.style.border = "1px solid rgba(255,255,255,0.15)";
    btn.style.padding = "2px 8px";
    btn.style.borderRadius = "3px";
    btn.style.cursor = "pointer";
    btn.style.font = "inherit";
    btn.onclick = () => {
      speed = s;
    };
    wrap.appendChild(btn);
  }
  bar.appendChild(wrap);
}

main().catch((err) => {
  status.textContent = `boot failed: ${err}`;
  console.error(err);
});
