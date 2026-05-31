// Main thread entry. The renderer holds the only WebGL context and reads
// snapshots from the SAB the sim worker writes; main never holds a wasm
// instance. Slider mutations / pause / TPS / inspect-requests go through
// the SimBridge.

import { makeCamera } from "./render";
import { renderWorld } from "./render-gl";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import {
  installProfilerPanel,
  setPanelStatus,
  resetPanelSamples,
} from "./widgets/perf-panel";
import {
  installDevPanel,
  getInitialGrassSeedCount,
  getEnergyMax,
  getFounderCount,
  getFullGrassOnInit,
  currentSliderState,
} from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection } from "./rail/inspector";
import { installNnTab } from "./rail/nn-tab";
import { span } from "./perf";
import { getSettings, setSetting, hasStoredSetting } from "./settings";
import { applyTheme } from "./themes";
import {
  SimBridge,
  MAX_POP_FOR_SIM,
  CTRL_CURRENT_SLOT,
  CTRL_SEQ,
  CREATURE_STRIDE,
  GRASS_CELL_COUNT,
  creatureSoAOffset,
  grassOffset,
  slotOffset,
  readSnapshotHeader,
  type SimReplyBootReady,
} from "./sim-bridge";

// v1.13 Wave 2: the `#status` span in the top bar is gone; the status line
// lives inside the bottom perf panel and is updated via setPanelStatus()
// (web/src/widgets/perf-panel.ts) each painted frame.
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const gl = canvas.getContext("webgl2", { antialias: true, alpha: false });
if (!gl) throw new Error("WebGL2 context unavailable");

let viewW = 0;
let viewH = 0;
function resize(): void {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  // If layout hasn't flushed yet (clientWidth=0), don't paint garbage. The
  // ResizeObserver will fire again once the box has real dimensions.
  if (w === 0 || h === 0) return;
  viewW = w;
  viewH = h;
  canvas.width = Math.floor(viewW * dpr);
  canvas.height = Math.floor(viewH * dpr);
}
const canvasWrap = document.getElementById("canvas-wrap");
if (canvasWrap) {
  const observer = new ResizeObserver(resize);
  observer.observe(canvasWrap);
}
resize();

// Pacing controls — TPS dropdown + play/pause toggle. Both forward to the
// sim worker via SimBridge messages.
let paused = false;
let targetTPS = getSettings().targetTPS;

// v1.13 Wave 2: the TPS selector lives in two places — the legacy top-bar
// dropdown (this file's `installPacingControls`) and the new perf panel's
// pill selector (`widgets/perf-panel.ts`). Each registers a listener so a
// change in one syncs the other; devpanel.ts also listens so the upkeep
// /s readout updates with TPS.
const tpsChangeListeners: Array<(tps: number) => void> = [];
export function setTpsChangeListener(fn: (tps: number) => void): void {
  tpsChangeListeners.push(fn);
}
export function getTargetTPS(): number {
  return targetTPS;
}
function fireTpsChange(tps: number): void {
  for (const fn of tpsChangeListeners) {
    try { fn(tps); } catch (e) { console.warn("tps listener threw", e); }
  }
}

/**
 * v1.13 Wave 2: the perf panel's TPS selector calls this to update main's
 * mirrored target-TPS without bouncing through its own listener. Mirrors
 * the legacy dropdown's onChange handler.
 */
export function setExternalTargetTPS(tps: number): void {
  if (!Number.isFinite(tps) || tps < 1) return;
  targetTPS = tps;
  fireTpsChange(tps);
}

let controlSab: SharedArrayBuffer | null = null;
let controlI32: Int32Array | null = null;
/**
 * v1.11 (A): snapshot region lives in wasm linear memory. These are
 * (re)constructed from the `WebAssembly.Memory.buffer` returned in
 * `boot_ready`. Each restart spawns a new worker with new wasm memory, so
 * we re-attach on every successful boot.
 */
let snapshotBuffer: ArrayBufferLike | null = null;
let snapshotBaseOffset = 0;
let snapshotView: DataView | null = null;
let cachedSeed = "";

async function main(): Promise<void> {
  // v1.9.1: apply the persisted theme before any UI installer runs so the
  // first paint uses the user's chosen palette instead of flashing the
  // :root fallback (charcoal).
  applyTheme(getSettings().theme);

  // First-run adaptive: low-core machines start with a tighter population cap.
  // Only fires when the user has never persisted their own value.
  if (!hasStoredSetting("maxPopulation")) {
    const cores = navigator.hardwareConcurrency ?? 0;
    if (cores > 0 && cores < 8) {
      setSetting("maxPopulation", 2000);
    }
  }

  const sabAvail = typeof SharedArrayBuffer !== "undefined";
  const isolated =
    (globalThis as unknown as { crossOriginIsolated?: boolean }).crossOriginIsolated ?? false;
  console.log(
    `[threads] main thread: SharedArrayBuffer=${sabAvail} crossOriginIsolated=${isolated}`,
  );

  const params = new URLSearchParams(window.location.search);
  const urlSeed = (params.get("seed") ?? "").slice(0, 128) || "";

  // v1.9.2 follow-up: install the dev panel BEFORE spawning the worker so
  // its staged-slider widget readers are registered when
  // `currentSliderState()` is queried inside `spawnSimWorker` for the
  // boot payload. Otherwise `initial_sliders` is `{}` on first boot, the
  // worker uses Rust defaults, and persisted settings only take effect
  // after a manual restart.
  installDevPanel(() => simBridge);

  let simBridge = await spawnSimWorker(urlSeed);

  const cam = makeCamera(latestSnapshotWorldSize());
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => latestSnapshotWorldSize(),
  );

  installPacingControls(() => simBridge);
  installRestartButton(() => restart());

  const rail = installRail();

  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), simBridge, rail);

  installProfilerPanel(simBridge);
  // v1.13 Wave 2: the right-rail Monitor tab is gone. Its population graph
  // and per-worker stats now live in the bottom perf panel.
  // v1.12: NN tab. Topology Apply respawns the worker via restart(); bucket
  // edits are live-applied through getBridge() inside the installer.
  installNnTab(() => simBridge, () => restart());
  installSettingsButton(rail);

  // v1.9.1: apply persisted rail open/closed state on boot. The class drives
  // the grid track collapse + #right-rail display:none (see styles.css).
  applyRailOpen(getSettings().railOpen);

  async function restart(): Promise<void> {
    const oldBridge = simBridge;
    simBridge = await spawnSimWorker("");
    oldBridge.terminate();
    resetPanelSamples();
    resetInspectorSelection(rail);
    highlights.clear();
    // v1.13 Wave 2: position interpolation between snapshots was removed —
    // the seq-gate now forbids painting the same snapshot twice, and lerping
    // requires painting duplicates by definition. Nothing to reset here.
    hideWorldEndOverlay();
  }

  // World-end overlay wiring. Shown by the frame loop the first time a
  // snapshot reports `world_ended`; cleared by restart() or by the user's
  // Keep-watching click. The sim itself keeps grass-only ticks running so
  // the canvas underneath the dim layer still fills in.
  const overlay = document.getElementById("world-end-overlay") as HTMLDivElement | null;
  const overlayRestart = document.getElementById("world-end-restart") as HTMLButtonElement | null;
  const overlayDismiss = document.getElementById("world-end-dismiss") as HTMLButtonElement | null;
  let overlayDismissed = false;
  function showWorldEndOverlay(): void {
    if (overlay && !overlayDismissed) overlay.hidden = false;
  }
  function hideWorldEndOverlay(): void {
    if (overlay) overlay.hidden = true;
    overlayDismissed = false;
  }
  overlayRestart?.addEventListener("click", () => { void restart(); });
  overlayDismiss?.addEventListener("click", () => {
    overlayDismissed = true;
    if (overlay) overlay.hidden = true;
  });

  window.addEventListener("keydown", (e) => {
    if (e.key !== "r" && e.key !== "R") return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    void restart();
  });

  // ~ hotkey → toggle the right rail open/closed (v1.9.1; previously
  // switched to the Settings tab).
  window.addEventListener("keydown", (e) => {
    if (e.key !== "~") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    toggleRailOpen();
  });

  let autoRestartPending = false;

  // v1.13 Wave 2: render-loop seq-gate. The renderer must never paint the
  // same snapshot twice — if `seq === lastPaintedSeq`, the RAF callback
  // reschedules itself and returns before doing any work. This keeps the
  // invariant FPS ≤ TPS at all times. The FPS counter (painted-frame
  // semantics) is owned by the perf panel via setPanelStatus(); we no
  // longer count one FPS per RAF.
  let lastPaintedSeq = -1;
  function frame(_now: number): void {
    if (!controlI32 || !snapshotBuffer || !snapshotView) {
      requestAnimationFrame(frame);
      return;
    }
    const seq = Atomics.load(controlI32, CTRL_SEQ);
    if (seq === lastPaintedSeq) {
      // No new snapshot since the last paint — skip this RAF entirely so
      // the painted-frame FPS counter stays bounded by TPS.
      requestAnimationFrame(frame);
      return;
    }

    const frameSpan = span("frame");
    try {
      const readSpan = span("frame.snapshot.read");
      const rawSlot = Atomics.load(controlI32, CTRL_CURRENT_SLOT);
      const slot: 0 | 1 = rawSlot === 1 ? 1 : 0;
      const header = readSnapshotHeader(snapshotView, slotOffset(slot));
      const pop = Math.min(header.pop, MAX_POP_FOR_SIM);
      // v1.11 (A+D): snapshot region lives inside wasm.memory.buffer at
      // snapshotBaseOffset. Grass is now f32 per cell (no quantize) — view
      // length is GRASS_CELL_COUNT, not GRASS_BYTES.
      const creatures = pop > 0
        ? new Float32Array(
            snapshotBuffer,
            snapshotBaseOffset + creatureSoAOffset(slot),
            pop * CREATURE_STRIDE,
          )
        : new Float32Array(0);
      const grass = new Float32Array(
        snapshotBuffer,
        snapshotBaseOffset + grassOffset(slot),
        GRASS_CELL_COUNT,
      );
      readSpan.close();

      if (header.world_ended) {
        if (!paused && getSettings().autoRun && !autoRestartPending) {
          autoRestartPending = true;
          void restart().then(() => {
            autoRestartPending = false;
          });
        } else {
          showWorldEndOverlay();
        }
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
      );

      lastPaintedSeq = seq;

      setPanelStatus({
        seed: cachedSeed,
        tick: header.tick,
        pop: header.pop,
        tps: header.tps,
        worldEnded: !!header.world_ended,
      });
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

  // v1.12: hand the persisted NN topology to the worker as a JSON string.
  // Rust side parses `{hidden_sizes, activations}`; "" falls back to legacy.
  const t = getSettings().nnTopology;
  const nn_topology_json = JSON.stringify({
    hidden_sizes: t.layerSizes,
    activations: t.activations,
  });
  bridge.sendBoot({
    kind: "boot",
    seed,
    initial_grass_seed_count: getInitialGrassSeedCount(),
    energy_max: getEnergyMax(),
    founder_count: getFounderCount(),
    full_grass_on_init: getFullGrassOnInit(),
    initial_sliders: currentSliderState(),
    initial_target_tps: targetTPS,
    initial_paused: paused,
    nn_topology_json,
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
  if (!ready.control_sab) {
    throw new Error(
      "[boot] sim worker did not deliver control SAB handle. " +
      "Check sim-worker.ts boot path.",
    );
  }
  if (!ready.wasm_memory) {
    throw new Error(
      "[boot] sim worker did not deliver wasm.memory handle. " +
      "Check sim-worker.ts boot path.",
    );
  }
  controlSab = ready.control_sab;
  controlI32 = new Int32Array(controlSab);
  // v1.11 (A): the snapshot region lives at a fixed offset inside the
  // worker's wasm linear memory. With shared memory enabled,
  // `wasm.memory.buffer` is a SharedArrayBuffer-compatible view both
  // threads see identically. Build a DataView once at boot; the byte
  // offsets returned by slotOffset/creatureSoAOffset/grassOffset are
  // RELATIVE TO the snapshot base inside wasm memory, so we add
  // `snapshotBaseOffset` everywhere we use them.
  snapshotBuffer = ready.wasm_memory.buffer;
  snapshotBaseOffset = ready.snapshot_buf_byte_offset;
  snapshotView = new DataView(snapshotBuffer, snapshotBaseOffset, ready.snapshot_buf_byte_len);
  latestWorldSize = ready.world_size;
  latestGrassDim = ready.grass_dim;
  // Stash the Rust-side slider defaults for the Wave D drift-guard e2e to
  // read. Cheap, only the test consumes it.
  (window as unknown as { __rustSlidersDefaults?: string }).__rustSlidersDefaults =
    ready.sliders_defaults_json;

  bridge.attachControlSab(controlSab);
  // Boot seeded initial paused / target TPS / sliders into the control SAB;
  // no post-boot mirror writes needed.

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
    getBridge().setPaused(paused);
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
    getBridge().setTargetTps(targetTPS);
    fireTpsChange(targetTPS);
  });

  // Keep the dropdown in sync if some other widget (perf panel) sets the
  // TPS. Match against the dropdown options; off-list values clear the
  // selection so the user sees there's no exact preset match.
  setTpsChangeListener((v) => {
    const opt = tpsOptions.find((o) => o === v);
    tpsSelect.value = opt !== undefined ? String(opt) : "";
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
    getBridge().setPaused(paused);
    refreshToggleLabel();
  });

  function refreshToggleLabel(): void {
    toggle.textContent = paused ? "▶ Play" : "⏸ Pause";
  }
}

// v1.9.1: rail open/closed helpers. Both the ⚙ button and the `~` hotkey
// route through `toggleRailOpen`; the canvas click handler calls
// `applyRailOpen(true)` to force the rail open before switching to Inspector.
function applyRailOpen(open: boolean): void {
  const shell = document.getElementById("app-shell");
  if (!shell) return;
  shell.classList.toggle("rail-collapsed", !open);
}

export function setRailOpen(open: boolean): void {
  setSetting("railOpen", open);
  applyRailOpen(open);
}

function toggleRailOpen(): void {
  setRailOpen(!getSettings().railOpen);
}

function installSettingsButton(_rail: { switchTab(name: "settings"): void }): void {
  const bar = document.getElementById("top-bar");
  if (!bar) return;
  const btn = document.createElement("button");
  btn.id = "settings-btn";
  btn.className = "topbar-btn";
  btn.textContent = "⚙";
  btn.title = "Toggle rail (~ hotkey)";
  // v1.9.1: ⚙ now toggles the rail open/closed instead of switching to the
  // Settings tab; users open Settings via the in-rail tab bar.
  btn.addEventListener("click", () => toggleRailOpen());
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
  console.error(err);
  // Surface in the perf panel's status line if it's already wired up;
  // otherwise users see the error in DevTools.
  try {
    const statusLine = document.getElementById("perf-status-line");
    if (statusLine) statusLine.textContent = `Boot failed: ${err}`;
  } catch { /* ignore */ }
});
