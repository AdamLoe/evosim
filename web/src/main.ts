import init, { WorldHandle, creature_stride } from "../wasm/evosim";
// initThreadPool is only exported when wasm is built with --features threads.
// Cast via unknown to avoid TS2614 on non-threaded builds.
import * as _wasmMod from "../wasm/evosim";
const initThreadPool = (_wasmMod as unknown as Record<string, unknown>)["initThreadPool"] as
  | ((n: number) => Promise<void>)
  | undefined;
import { makeCamera } from "./render";
import { renderWorld } from "./render-gl";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installProfilerPanel } from "./widgets/perf-panel";
import { installDevPanel, getInitialGrassSeedCount, getEnergyMax, reapplyDevSliders } from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection } from "./rail/inspector";
import { resetStats } from "./rail/stats";
import { attachProfiler, timed, span } from "./perf";
import { getSettings, setSetting } from "./settings";

const status = document.getElementById("status") as HTMLSpanElement;
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const gl = canvas.getContext("webgl2", { antialias: true, alpha: false });
if (!gl) throw new Error("WebGL2 context unavailable");

let viewW = 0;
let viewH = 0;
function resize(): void {
  // S22: cap DPR at 2 to avoid 3× pixel overdraw on 3× displays.
  // WebGL2 uses gl.viewport against canvas.width/.height (physical px);
  // viewW/viewH stay in CSS px so camera/cursor math is consistent.
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  viewW = window.innerWidth;
  viewH = window.innerHeight;
  canvas.width = Math.floor(viewW * dpr);
  canvas.height = Math.floor(viewH * dpr);
  canvas.style.width = `${viewW}px`;
  canvas.style.height = `${viewH}px`;
}
window.addEventListener("resize", resize);
resize();

// Sim pacing: a play/pause toggle plus a target ticks/sec input. The frame
// loop accumulates a fractional tick budget so ticks are spread evenly
// across rAF frames instead of bursting at the start of a wall-clock second.
// targetTPS is hydrated from persisted settings so user choice survives reload.
let paused = false;
let targetTPS = getSettings().targetTPS;
let tickBudget = 0;
const MAX_TICKS_PER_FRAME = 2000;
const MAX_FRAME_DELTA_MS = 100; // cap so tab-blur doesn't queue a tick tsunami.

// The upkeep slider's /sec readout depends on the targetTPS, so the TPS
// input notifies the dev panel to refresh that readout when it changes.
let onTpsChange: ((tps: number) => void) | null = null;
export function setTpsChangeListener(fn: (tps: number) => void): void {
  onTpsChange = fn;
}
export function getTargetTPS(): number {
  return targetTPS;
}


// F.27: seed display + copy button. The getter form lets the copy button
// (installed once) always copy the *current* world's seed across restarts.
function installSeedDisplay(getSeed: () => string): void {
  const valueEl = document.getElementById("seed-value");
  if (valueEl) valueEl.textContent = getSeed();
  const btn = document.getElementById("seed-copy-btn") as HTMLButtonElement | null;
  if (btn) {
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(getSeed());
        btn.textContent = "copied";
        setTimeout(() => (btn.textContent = "copy"), 1200);
      } catch (e) {
        console.warn("clipboard copy failed", e);
      }
    });
  }
}

function updateSeedDisplay(seed: string): void {
  const valueEl = document.getElementById("seed-value");
  if (valueEl) valueEl.textContent = seed;
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

  const params = new URLSearchParams(window.location.search);
  // Cap seed param length to prevent oversized inputs (S15).
  const urlSeed = (params.get("seed") ?? "").slice(0, 128) || null;

  // `world` is mutable so the restart button can swap in a fresh WorldHandle
  // without re-installing UI. All subsystems that need the current world
  // either get it as a parameter each frame (pollRail, renderWorld) or
  // capture the `getWorld` getter below (which always returns the latest).
  let world: WorldHandle = WorldHandle.newWithGrassSeed(
    urlSeed ?? "",
    getInitialGrassSeedCount(),
    getEnergyMax(),
  );
  const getWorld = (): WorldHandle => world;
  // Debug hook: expose the live world on `window.__world` so headless probes
  // (and the JS console) can poke at the sim state directly.
  (window as unknown as { __world: WorldHandle }).__world = world;

  // S22: cache seed once per world lifetime (world.seed is a getter that
  // allocates a new String each call; no need to call it per frame).
  let cachedSeed = world.seed;

  // F.27: seed display. The copy button (installed once) reads the current
  // seed via the getter so it stays correct across restarts.
  installSeedDisplay(() => cachedSeed);

  const stride = creature_stride();
  const cam = makeCamera(world.world_size);
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => world.world_size,
  );

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop ${world.population}`;

  // Top-bar pacing controls + restart.
  installPacingControls();
  installRestartButton(() => restart());

  // E.21: install right rail.
  const rail = installRail(world);

  // E.24: canvas click → inspector.
  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), getWorld, rail);

  // Profiler: attach world after construction so the TS profiler can
  // forward enable/disable calls to the Rust side (D1, D9).
  attachProfiler(getWorld);

  // Perf-timing: install the Stats-panel toggle + 1Hz polling loop.
  installProfilerPanel(getWorld);

  // P3b: dev panel overlay (6 sliders, ~ hotkey, ⚙ button).
  installDevPanel(getWorld);

  function restart(): void {
    const oldWorld = world;
    // New random seed each restart (empty string → random per build).
    world = WorldHandle.newWithGrassSeed("", getInitialGrassSeedCount(), getEnergyMax());
    (window as unknown as { __world: WorldHandle }).__world = world;
    cachedSeed = world.seed;
    // Re-apply the user's current dev-slider tweaks; `initialGrassSeedCount`
    // is already baked into world construction above.
    reapplyDevSliders(world);
    // Clear UI state that referenced creatures from the old world.
    resetStats();
    resetInspectorSelection(rail);
    highlights.clear();
    updateSeedDisplay(cachedSeed);
    // Free the old world's wasm memory.
    oldWorld.free();
  }

  // Restart hotkey: "r" (ignored when focus is in an input/textarea).
  window.addEventListener("keydown", (e) => {
    if (e.key !== "r" && e.key !== "R") return;
    if (e.ctrlKey || e.metaKey || e.altKey) return; // don't hijack Ctrl-R
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    restart();
  });

  // Sim + render loop.
  let lastRender = performance.now();
  // S22: throttle status DOM updates to 5 Hz (200 ms gate).
  let lastStatusUpdate = 0;
  function frame(now: number): void {
    const frameSpan = span("frame");
    try {
      const rawDelta = now - lastRender;
      lastRender = now;
      // Clamp delta so a hidden/blurred tab doesn't queue thousands of ticks
      // on resume. Drop any pent-up budget on resume from paused.
      const delta = Math.min(rawDelta, MAX_FRAME_DELTA_MS);

      // S22: hoist world_ended once per RAF frame (was called 3× per frame).
      let ended = world.world_ended;

      // Auto-restart: when the world has ended and the user has auto-run on,
      // spawn a fresh world right away so the sim never stalls. The new
      // world inherits the user's slider state (via reapplyDevSliders).
      if (ended && !paused && getSettings().autoRun) {
        restart();
        ended = world.world_ended; // false on a fresh world
      }

      let ticksThisFrame = 0;
      if (paused || ended) {
        tickBudget = 0;
      } else {
        tickBudget += targetTPS * (delta / 1000);
        if (tickBudget > MAX_TICKS_PER_FRAME) {
          // Cap one frame's worth; the rest of the backlog is dropped (we
          // don't try to "catch up" if the target rate exceeds what the
          // browser can deliver in real time).
          tickBudget = MAX_TICKS_PER_FRAME;
        }
        ticksThisFrame = Math.floor(tickBudget);
        tickBudget -= ticksThisFrame;
      }

      if (ticksThisFrame > 0) {
        timed("step_n", () => world.step_n(ticksThisFrame));
      }

      // Fetch ids buffer once per frame (index-aligned with creatures_buffer).
      const ids = world.creature_ids_buffer() as unknown as Float64Array;

      // E.23/E.24: poll the rail (highlights, stats, inspector).
      // S18: ids no longer passed to pollRail (inspector uses creature_idx_by_id instead).
      timed("pollRail", () => pollRail(rail, world));

      timed("renderWorld", () =>
        renderWorld(gl!, cam, viewW, viewH, world, stride, ids, highlights, now));

      // S22: throttle status DOM updates to 5 Hz (200 ms gate).
      if (now - lastStatusUpdate > 200) {
        lastStatusUpdate = now;
        const endedSuffix = ended ? "  (world ended)" : "";
        status.textContent =
          `seed: ${cachedSeed}  ·  tick ${world.tick}  ·  pop ${world.population}${endedSuffix}`;
      }
    } finally {
      frameSpan.close();
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

function installPacingControls(): void {
  const bar = document.getElementById("top-bar")!;
  const wrap = document.createElement("span");
  wrap.style.marginLeft = "auto";
  wrap.style.display = "inline-flex";
  wrap.style.alignItems = "center";
  wrap.style.gap = "6px";

  // Play/pause toggle.
  const toggle = document.createElement("button");
  toggle.id = "playpause-btn";
  toggle.title = "Play / pause (space)";
  applyTopbarBtnStyle(toggle);
  const refreshToggleLabel = (): void => {
    toggle.textContent = paused ? "▶ play" : "⏸ pause";
  };
  refreshToggleLabel();
  toggle.onclick = () => {
    paused = !paused;
    tickBudget = 0;
    refreshToggleLabel();
  };
  wrap.appendChild(toggle);

  // Target TPS dropdown (fixed set of options for predictable pacing).
  const tpsLabel = document.createElement("label");
  tpsLabel.textContent = "target TPS";
  tpsLabel.style.color = "var(--fg)";
  tpsLabel.style.fontSize = "12px";
  tpsLabel.style.opacity = "0.8";
  const tpsSelect = document.createElement("select");
  tpsSelect.id = "target-tps-input";
  tpsSelect.style.background = "rgba(255,255,255,0.08)";
  tpsSelect.style.color = "var(--fg)";
  tpsSelect.style.border = "1px solid rgba(255,255,255,0.15)";
  tpsSelect.style.padding = "2px 4px";
  tpsSelect.style.borderRadius = "3px";
  tpsSelect.style.font = "inherit";
  const tpsOptions = [10, 30, 60, 180, 500, 1000];
  for (const v of tpsOptions) {
    const opt = document.createElement("option");
    opt.value = String(v);
    opt.textContent = String(v);
    tpsSelect.appendChild(opt);
  }
  // Snap initial value to the nearest option so persisted values from older
  // schemas (or odd numbers) still match a dropdown choice.
  const nearest = tpsOptions.reduce((best, v) =>
    Math.abs(v - targetTPS) < Math.abs(best - targetTPS) ? v : best, tpsOptions[2]);
  targetTPS = nearest;
  tpsSelect.value = String(nearest);
  tpsSelect.addEventListener("change", () => {
    const v = Number(tpsSelect.value);
    if (!Number.isFinite(v) || v < 1) return;
    targetTPS = v;
    tickBudget = 0;
    setSetting("targetTPS", targetTPS);
    if (onTpsChange) onTpsChange(targetTPS);
  });
  wrap.appendChild(tpsLabel);
  wrap.appendChild(tpsSelect);

  bar.appendChild(wrap);

  // Spacebar toggles play/pause (skip when typing in inputs).
  window.addEventListener("keydown", (e) => {
    if (e.key !== " " && e.code !== "Space") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    paused = !paused;
    tickBudget = 0;
    refreshToggleLabel();
  });
}

function applyTopbarBtnStyle(btn: HTMLButtonElement): void {
  btn.style.background = "rgba(255,255,255,0.08)";
  btn.style.color = "var(--fg)";
  btn.style.border = "1px solid rgba(255,255,255,0.15)";
  btn.style.padding = "2px 8px";
  btn.style.borderRadius = "3px";
  btn.style.cursor = "pointer";
  btn.style.font = "inherit";
}

function installRestartButton(onClick: () => void): void {
  const bar = document.getElementById("top-bar")!;
  const btn = document.createElement("button");
  btn.id = "restart-btn";
  btn.textContent = "↺ restart";
  btn.title = "Restart simulation with new seed (r)";
  btn.style.marginLeft = "8px";
  applyTopbarBtnStyle(btn);
  btn.onclick = onClick;
  bar.appendChild(btn);
}

main().catch((err) => {
  status.textContent = `boot failed: ${err}`;
  console.error(err);
});
