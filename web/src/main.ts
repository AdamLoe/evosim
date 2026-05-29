// Main thread entry. The renderer holds the only WebGL context and reads
// snapshots from the SAB the sim worker writes; main never holds a wasm
// instance. Slider mutations / pause / TPS / inspect-requests go through
// the SimBridge.

import { makeCamera } from "./render";
import { renderWorld } from "./render-gl";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installProfilerPanel } from "./widgets/perf-panel";
import {
  installDevPanel,
  getInitialGrassSeedCount,
  getEnergyMax,
  getFounderCount,
  getFullGrassOnInit,
  currentSliderState,
} from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection } from "./rail/inspector";
import { resetStats } from "./rail/stats";
import { installMonitorTab } from "./rail/monitor";
import { span } from "./perf";
import { getSettings, setSetting } from "./settings";
import {
  SimBridge,
  MAX_POP_FOR_SIM,
  CTRL_CURRENT_SLOT,
  CREATURE_STRIDE,
  GRASS_BYTES,
  creatureSoAOffset,
  grassOffset,
  slotOffset,
  readSnapshotHeader,
  type SimReplyBootReady,
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

// Pacing controls — TPS dropdown + play/pause toggle. Both forward to the
// sim worker via SimBridge messages.
let paused = false;
let targetTPS = getSettings().targetTPS;

let onTpsChange: ((tps: number) => void) | null = null;
export function setTpsChangeListener(fn: (tps: number) => void): void {
  onTpsChange = fn;
}
export function getTargetTPS(): number {
  return targetTPS;
}

let controlSab: SharedArrayBuffer | null = null;
let snapshotSab: SharedArrayBuffer | null = null;
let controlI32: Int32Array | null = null;
let snapshotView: DataView | null = null;
let cachedSeed = "";

async function main(): Promise<void> {
  const sabAvail = typeof SharedArrayBuffer !== "undefined";
  const isolated =
    (globalThis as unknown as { crossOriginIsolated?: boolean }).crossOriginIsolated ?? false;
  console.log(
    `[threads] main thread: SharedArrayBuffer=${sabAvail} crossOriginIsolated=${isolated}`,
  );

  const params = new URLSearchParams(window.location.search);
  const urlSeed = (params.get("seed") ?? "").slice(0, 128) || "";

  let simBridge = await spawnSimWorker(urlSeed);

  const cam = makeCamera(latestSnapshotWorldSize());
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => latestSnapshotWorldSize(),
  );

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop 0`;

  installPacingControls(() => simBridge);
  installRestartButton(() => restart());

  const rail = installRail();

  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), simBridge, rail);

  installProfilerPanel(simBridge);
  installDevPanel(simBridge);
  installMonitorTab(simBridge);
  installSettingsButton(rail);

  async function restart(): Promise<void> {
    const oldBridge = simBridge;
    simBridge = await spawnSimWorker("");
    oldBridge.terminate();
    resetStats();
    resetInspectorSelection(rail);
    highlights.clear();
  }

  window.addEventListener("keydown", (e) => {
    if (e.key !== "r" && e.key !== "R") return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    void restart();
  });

  // ~ hotkey → focus rail on Settings tab.
  window.addEventListener("keydown", (e) => {
    if (e.key !== "~") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    rail.switchTab("settings");
  });

  let autoRestartPending = false;

  let framesThisSecond = 0;
  let fpsWindowStart = performance.now();
  let lastFps = -1;
  function frame(now: number): void {
    const frameSpan = span("frame");
    try {
      if (!controlI32 || !snapshotSab || !snapshotView) return;
      const readSpan = span("frame.snapshot.read");
      const rawSlot = Atomics.load(controlI32, CTRL_CURRENT_SLOT);
      const slot: 0 | 1 = rawSlot === 1 ? 1 : 0;
      const header = readSnapshotHeader(snapshotView, slotOffset(slot));
      const pop = Math.min(header.pop, MAX_POP_FOR_SIM);
      const creatures = pop > 0
        ? new Float32Array(snapshotSab, creatureSoAOffset(slot), pop * CREATURE_STRIDE)
        : new Float32Array(0);
      const grass = new Uint8Array(snapshotSab, grassOffset(slot), GRASS_BYTES);
      readSpan.close();

      if (header.world_ended && !paused && getSettings().autoRun && !autoRestartPending) {
        autoRestartPending = true;
        void restart().then(() => {
          autoRestartPending = false;
        });
      }

      pollRail(rail, header, simBridge, creatures, pop);
      renderWorld(
        gl!,
        cam,
        viewW,
        viewH,
        creatures,
        grass,
        pop,
        latestWorldSize,
        latestGrassDim,
        highlights,
        now,
      );

      framesThisSecond++;
      if (now - fpsWindowStart >= 1000) {
        lastFps = framesThisSecond;
        framesThisSecond = 0;
        fpsWindowStart = now;
      }
      const endedSuffix = header.world_ended ? "  (world ended)" : "";
      const tpsStr = isFinite(header.tps) && header.tps > 0
        ? header.tps.toFixed(0)
        : "—";
      const fpsStr = lastFps >= 0 ? lastFps.toString() : "—";
      status.textContent =
        `seed: ${cachedSeed}  ·  tick ${header.tick}  ·  pop ${header.pop}` +
        `  ·  ${tpsStr} TPS  ·  ${fpsStr} FPS${endedSuffix}`;
    } finally {
      frameSpan.close();
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

// ─── Worker spawn / boot handshake ──────────────────────────────────────────

let latestWorldSize = 0;
let latestGrassDim = 0;
function latestSnapshotWorldSize(): number {
  return latestWorldSize;
}

async function spawnSimWorker(seed: string): Promise<SimBridge> {
  const w = new Worker(new URL("./sim-worker.ts", import.meta.url), { type: "module" });
  const bridge = new SimBridge(w);

  const bootReady = new Promise<SimReplyBootReady>((resolve) => {
    bridge.onBootReady((reply) => resolve(reply));
  });

  cachedSeed = seed === "" ? "(random)" : seed;

  bridge.postMessage({
    kind: "boot",
    seed,
    initial_grass_seed_count: getInitialGrassSeedCount(),
    energy_max: getEnergyMax(),
    founder_count: getFounderCount(),
    full_grass_on_init: getFullGrassOnInit(),
    initial_sliders: currentSliderState(),
  });

  const ready = await bootReady;
  if (ready.max_pop_for_sim !== MAX_POP_FOR_SIM) {
    throw new Error(
      `[boot] max_pop_for_sim mismatch: worker reported ${ready.max_pop_for_sim}, ` +
      `main expects ${MAX_POP_FOR_SIM}. Rebuild wasm (rustup run nightly wasm-pack ` +
      `build --target web --out-dir web/wasm --dev --features threads) — ` +
      `Rust/TS const drift.`,
    );
  }
  if (!ready.snapshot_sab || !ready.control_sab) {
    throw new Error(
      "[boot] sim worker did not deliver SAB handles (snapshot_sab/control_sab " +
      "null). Check sim-worker.ts boot path.",
    );
  }
  controlSab = ready.control_sab;
  snapshotSab = ready.snapshot_sab;
  controlI32 = new Int32Array(controlSab);
  snapshotView = new DataView(snapshotSab);
  latestWorldSize = ready.world_size;
  latestGrassDim = ready.grass_dim;
  // Stash the Rust-side slider defaults for the Wave D drift-guard e2e to
  // read. Cheap, only the test consumes it.
  (window as unknown as { __rustSlidersDefaults?: string }).__rustSlidersDefaults =
    ready.sliders_defaults_json;

  bridge.attachControlSab(controlSab);

  bridge.postMessage({ kind: "set_target_tps", tps: targetTPS });
  bridge.postMessage({ kind: "set_paused", paused });

  return bridge;
}

function installPacingControls(getBridge: () => SimBridge): void {
  const bar = document.getElementById("top-bar")!;

  // Play/pause toggle.
  const toggle = document.createElement("button");
  toggle.id = "playpause-btn";
  toggle.className = "topbar-btn";
  toggle.title = "Play / pause (space)";
  refreshToggleLabel();
  toggle.onclick = () => {
    paused = !paused;
    getBridge().postMessage({ kind: "set_paused", paused });
    refreshToggleLabel();
  };

  // Target TPS dropdown.
  const tpsLabel = document.createElement("span");
  tpsLabel.className = "topbar-label";
  tpsLabel.textContent = "Target TPS";

  const tpsSelect = document.createElement("select");
  tpsSelect.id = "target-tps-input";
  tpsSelect.className = "topbar-select";
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

  // Spacer pushes pacing controls to the right.
  const spacer = document.createElement("span");
  spacer.className = "topbar-spacer";

  bar.append(spacer, toggle, tpsLabel, tpsSelect);

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
    toggle.textContent = paused ? "▶ Play" : "⏸ Pause";
  }
}

function installSettingsButton(rail: { switchTab(name: "settings"): void }): void {
  const bar = document.getElementById("top-bar");
  if (!bar) return;
  const btn = document.createElement("button");
  btn.id = "settings-btn";
  btn.className = "topbar-btn";
  btn.textContent = "⚙";
  btn.title = "Settings (~ hotkey)";
  btn.addEventListener("click", () => rail.switchTab("settings"));
  bar.appendChild(btn);
}

function installRestartButton(onClick: () => void): void {
  const bar = document.getElementById("top-bar")!;
  const btn = document.createElement("button");
  btn.id = "restart-btn";
  btn.className = "topbar-btn";
  btn.textContent = "↺ Restart";
  btn.title = "Restart simulation with new seed (r)";
  btn.onclick = onClick;
  bar.appendChild(btn);
}

main().catch((err) => {
  status.textContent = `Boot failed: ${err}`;
  console.error(err);
});
