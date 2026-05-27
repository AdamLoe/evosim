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
import { installDevPanel, getInitialGrassSeedCount, reapplyDevSliders } from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection } from "./rail/inspector";
import { resetStats } from "./rail/stats";
import { attachProfiler, timed, span } from "./perf";

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

type Speed = 0 | 1 | 10 | 100;
let speed: Speed = 1;


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
  let world: WorldHandle = WorldHandle.newWithGrassSeed(urlSeed ?? "", getInitialGrassSeedCount());
  const getWorld = (): WorldHandle => world;

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

  // Speed buttons + restart in the top bar.
  installSpeedControls();
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
    world = WorldHandle.newWithGrassSeed("", getInitialGrassSeedCount());
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
      const delta = now - lastRender;
      lastRender = now;
      const ticksThisFrame =
        speed === 0 ? 0 : Math.min(200, Math.max(1, Math.round((speed * delta) / 16.66)));

      // S22: hoist world_ended once per RAF frame (was called 3× per frame).
      const ended = world.world_ended;

      if (ticksThisFrame > 0 && !ended) {
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

function installRestartButton(onClick: () => void): void {
  const bar = document.getElementById("top-bar")!;
  const btn = document.createElement("button");
  btn.id = "restart-btn";
  btn.textContent = "↺ restart";
  btn.title = "Restart simulation with new seed (r)";
  btn.style.marginLeft = "8px";
  btn.style.background = "rgba(255,255,255,0.08)";
  btn.style.color = "var(--fg)";
  btn.style.border = "1px solid rgba(255,255,255,0.15)";
  btn.style.padding = "2px 8px";
  btn.style.borderRadius = "3px";
  btn.style.cursor = "pointer";
  btn.style.font = "inherit";
  btn.onclick = onClick;
  bar.appendChild(btn);
}

main().catch((err) => {
  status.textContent = `boot failed: ${err}`;
  console.error(err);
});
