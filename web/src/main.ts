// v1.6 Wave B: main thread holds NO wasm. The sim runs in a Web Worker
// (`web/src/sim-worker.ts`); main owns rendering, UI, and the message bridge.
//
// Per v1.6-plan.md §"Step B":
//   - Spawn the sim worker, send `boot`, await `boot_ready`.
//   - Assert `boot_ready.max_pop_for_sab === MAX_POP_FOR_SAB` (Rust/TS const
//     drift is fatal — rebuild wasm).
//   - Render loop reads the most recent snapshot received via postMessage.
//   - Restart = worker.terminate() + new Worker; old SAB views are GC'd by
//     the closure swap.
//   - `initial_sliders` is sourced from `currentSliderState()` (in-memory
//     widget values), NOT `getSettings()` localStorage — so a mid-drag
//     restart carries the dragged value. (gotcha 4)
//   - `__world` debug hook is gone (gotcha 6).

import { makeCamera } from "./render";
import { renderWorld, type SimSnapshot } from "./render-gl";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installProfilerPanel } from "./widgets/perf-panel";
import {
  installDevPanel,
  getInitialGrassSeedCount,
  getEnergyMax,
  getFounderCount,
  currentSliderState,
} from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection } from "./rail/inspector";
import { resetStats } from "./rail/stats";
import { span } from "./perf";
import { getSettings, setSetting } from "./settings";
import {
  SimBridge,
  MAX_POP_FOR_SAB,
  type SimReplyBootReady,
  type SimReplySnapshot,
} from "./sim-bridge";

const status = document.getElementById("status") as HTMLSpanElement;
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const gl = canvas.getContext("webgl2", { antialias: true, alpha: false });
if (!gl) throw new Error("WebGL2 context unavailable");

let viewW = 0;
let viewH = 0;
function resize(): void {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  viewW = canvas.clientWidth || window.innerWidth;
  viewH = canvas.clientHeight || window.innerHeight;
  canvas.width = Math.floor(viewW * dpr);
  canvas.height = Math.floor(viewH * dpr);
}
window.addEventListener("resize", resize);
resize();

/** Open or close the right-edge settings overlay. */
export function setSettingsOpen(open: boolean): void {
  const overlay = document.getElementById("settings-overlay") as HTMLElement | null;
  if (!overlay) return;
  overlay.style.display = open ? "block" : "none";
  document.body.classList.toggle("settings-open", open);
  requestAnimationFrame(resize);
}
export function isSettingsOpen(): boolean {
  return document.body.classList.contains("settings-open");
}

// Pacing controls — TPS dropdown + play/pause toggle. Both forward to the
// sim worker via SimBridge messages; the worker owns the actual tick loop.
let paused = false;
let targetTPS = getSettings().targetTPS;

let onTpsChange: ((tps: number) => void) | null = null;
export function setTpsChangeListener(fn: (tps: number) => void): void {
  onTpsChange = fn;
}
export function getTargetTPS(): number {
  return targetTPS;
}

// Latest snapshot received from the sim worker. The first one lands as part
// of the boot handshake (worker runs one tick before posting boot_ready), so
// the first RAF after boot is guaranteed to see a valid snapshot. (gotcha 1)
let latestSnapshot: SimReplySnapshot | null = null;
let cachedSeed = "";

async function main(): Promise<void> {
  // crossOriginIsolated drives the threaded-build decision on the worker side;
  // we still log it on main so the existing diagnostic line survives.
  const sabAvail = typeof SharedArrayBuffer !== "undefined";
  const isolated =
    (globalThis as unknown as { crossOriginIsolated?: boolean }).crossOriginIsolated ?? false;
  console.log(
    `[threads] main thread: SharedArrayBuffer=${sabAvail} crossOriginIsolated=${isolated}`,
  );

  const params = new URLSearchParams(window.location.search);
  const urlSeed = (params.get("seed") ?? "").slice(0, 128) || "";

  // Spawn the sim worker. v1.6 Wave B: `worker: { format: "es" }` is already
  // set in vite.config.ts (originally for wasm-bindgen-rayon); we piggyback.
  let simBridge = await spawnSimWorker(urlSeed);

  // Camera is sized by world_size received in boot_ready; cachedSeed is set
  // from the same handshake. Both are set inside spawnSimWorker → bootReady.
  const cam = makeCamera(latestSnapshotWorldSize());
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => latestSnapshotWorldSize(),
  );

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop ${latestSnapshot?.pop ?? 0}`;

  installPacingControls(() => simBridge);
  installRestartButton(() => restart());

  const rail = installRail();

  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), simBridge, rail);

  // The TS-side profiler can no longer toggle the Rust profiler via a
  // WorldHandle — main holds none. `attachProfiler` is now a no-op since the
  // sim worker drives `profile_enable` directly when the checkbox toggles.
  // Skip attaching; perf-panel.ts wires the toggle to the SimBridge.

  installProfilerPanel(simBridge);

  installDevPanel(simBridge);

  installSettingsToggle();

  async function restart(): Promise<void> {
    const oldBridge = simBridge;
    simBridge = await spawnSimWorker("");
    // After the new worker's first snapshot lands, drop the old one.
    oldBridge.terminate();
    // Reset per-world UI state so stale ids don't linger across worlds.
    resetStats();
    resetInspectorSelection(rail);
    highlights.clear();
  }

  // Restart hotkey: "r" (ignored when focus is in an input/textarea).
  window.addEventListener("keydown", (e) => {
    if (e.key !== "r" && e.key !== "R") return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    void restart();
  });

  // Auto-restart trigger latch — guards against the brief gap between
  // observing `world_ended` and the new worker's boot_ready landing.
  let autoRestartPending = false;

  // Render loop.
  let lastStatusUpdate = 0;
  function frame(now: number): void {
    const frameSpan = span("frame");
    try {
      const snap = latestSnapshot;
      if (!snap) {
        // boot_ready hasn't landed yet (or restart in flight). Skip render
        // this frame; the previous backbuffer remains visible.
        return;
      }

      if (snap.world_ended && !paused && getSettings().autoRun && !autoRestartPending) {
        autoRestartPending = true;
        void restart().then(() => {
          autoRestartPending = false;
        });
      }

      const simSnap: SimSnapshot = {
        creatures: snap.creatures as unknown as Float32Array,
        ids: snap.ids,
        grass: snap.grass,
        pop: snap.pop,
        world_size: latestSnapshotWorldSize(),
        grass_dim: latestGrassDim,
      };

      pollRail(rail, snap, simBridge);
      renderWorld(gl!, cam, viewW, viewH, simSnap, highlights, now);

      if (now - lastStatusUpdate > 200) {
        lastStatusUpdate = now;
        const endedSuffix = snap.world_ended ? "  (world ended)" : "";
        status.textContent =
          `seed: ${cachedSeed}  ·  tick ${snap.tick}  ·  pop ${snap.pop}${endedSuffix}`;
      }
    } finally {
      frameSpan.close();
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

// ─── Worker spawn / boot handshake ──────────────────────────────────────────

// Held outside spawnSimWorker so render loop can read after handshake.
let latestWorldSize = 0;
let latestGrassDim = 0;
function latestSnapshotWorldSize(): number {
  return latestWorldSize;
}

async function spawnSimWorker(seed: string): Promise<SimBridge> {
  const w = new Worker(new URL("./sim-worker.ts", import.meta.url), { type: "module" });
  const bridge = new SimBridge(w);

  bridge.onSnapshot((snap) => {
    // Reinterpret `creatures` (delivered as Uint8Array because of the wasm
    // Float32Array.view cast at the boundary) back to its native typed-array.
    // Structured clone preserves the underlying ArrayBuffer; we wrap it.
    const cu = snap.creatures as unknown as Uint8Array;
    const cf = new Float32Array(cu.buffer, cu.byteOffset, cu.byteLength / 4);
    (snap as unknown as { creatures: Float32Array }).creatures = cf;
    latestSnapshot = snap;
  });

  const bootReady = new Promise<SimReplyBootReady>((resolve) => {
    bridge.onBootReady((reply) => resolve(reply));
  });

  // Wave B uses the seed echoed back from the worker for the displayed
  // `seed:` line. We pass the URL/restart seed in; the worker resolves "" to
  // a random seed. Main displays whatever it sent; the worker's seed is
  // captured for parity if it ever differs.
  // For now we don't have a `seed` field in boot_ready (not in protocol);
  // fall back to the request seed for the status bar.
  cachedSeed = seed === "" ? "(random)" : seed;

  bridge.postMessage({
    kind: "boot",
    seed,
    initial_grass_seed_count: getInitialGrassSeedCount(),
    energy_max: getEnergyMax(),
    founder_count: getFounderCount(),
    initial_sliders: currentSliderState(),
  });

  const ready = await bootReady;
  // gotcha 5: handshake assertion. Mismatch means Rust constant drifted from
  // the TS const; only recovery is to rebuild wasm.
  if (ready.max_pop_for_sab !== MAX_POP_FOR_SAB) {
    throw new Error(
      `[boot] max_pop_for_sab mismatch: worker reported ${ready.max_pop_for_sab}, ` +
      `main expects ${MAX_POP_FOR_SAB}. Rebuild wasm (rustup run nightly wasm-pack ` +
      `build --target web --out-dir web/wasm --dev --features threads) — ` +
      `Rust/TS const drift.`,
    );
  }
  latestWorldSize = ready.world_size;
  latestGrassDim = ready.grass_dim;

  // Push current pacing state to the freshly-booted worker so it matches the
  // user's last-known dropdown / play-pause state across restarts.
  bridge.postMessage({ kind: "set_target_tps", tps: targetTPS });
  bridge.postMessage({ kind: "set_paused", paused });

  return bridge;
}

function installPacingControls(getBridge: () => SimBridge): void {
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
  refreshToggleLabel();
  toggle.onclick = () => {
    paused = !paused;
    getBridge().postMessage({ kind: "set_paused", paused });
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
  const nearest = tpsOptions.reduce((best, v) =>
    Math.abs(v - targetTPS) < Math.abs(best - targetTPS) ? v : best, tpsOptions[2]);
  targetTPS = nearest;
  tpsSelect.value = String(nearest);
  tpsSelect.addEventListener("change", () => {
    const v = Number(tpsSelect.value);
    if (!Number.isFinite(v) || v < 1) return;
    targetTPS = v;
    setSetting("targetTPS", targetTPS);
    getBridge().postMessage({ kind: "set_target_tps", tps: targetTPS });
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
    getBridge().postMessage({ kind: "set_paused", paused });
    refreshToggleLabel();
  });

  function refreshToggleLabel(): void {
    toggle.textContent = paused ? "▶ play" : "⏸ pause";
  }
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

function installSettingsToggle(): void {
  const overlay = document.getElementById("settings-overlay") as HTMLElement | null;
  if (!overlay) return;
  const bar = document.getElementById("top-bar");
  if (bar) {
    const btn = document.createElement("button");
    btn.id = "devpanel-toggle";
    btn.textContent = "⚙";
    btn.title = "Settings (~ hotkey)";
    applyTopbarBtnStyle(btn);
    btn.style.marginLeft = "4px";
    btn.addEventListener("click", () => setSettingsOpen(!isSettingsOpen()));
    bar.appendChild(btn);
  }
  const close = document.getElementById("settings-close") as HTMLButtonElement | null;
  if (close) {
    close.addEventListener("click", () => setSettingsOpen(false));
  }
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
