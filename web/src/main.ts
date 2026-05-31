// Main thread entry. The renderer holds the only WebGL context and reads
// snapshots from the SAB the sim worker writes; main never holds a wasm
// instance. Slider mutations / pause / TPS / inspect-requests go through
// the SimBridge.

import { makeCamera } from "./render";
import { renderWorld, resetInterpolation } from "./render-gl";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights, type RailState } from "./rail/index";
import { installProfilerPanel, setProfilerVisible } from "./widgets/perf-panel";
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
import { installNnTab } from "./rail/nn-tab";
import { span } from "./perf";
import { getSettings, setSetting } from "./settings";
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

const status = document.getElementById("status") as HTMLSpanElement;
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

// v1.13 Wave 1: the top-bar TPS dropdown is gone (Wave 2 puts the new
// selector in the perf panel). `setTpsChangeListener` stays exported so
// the devpanel's upkeep-per-second readout keeps wiring its callback;
// nothing currently invokes the listener until Wave 2 lands a new TPS
// control. The listener is held in a module-level slot so a future
// caller (Wave 2 TPS pill row) can fire it without re-plumbing.
let onTpsChange: ((tps: number) => void) | null = null;
export function setTpsChangeListener(fn: (tps: number) => void): void {
  onTpsChange = fn;
}
export function getTargetTPS(): number {
  return targetTPS;
}
// Keep `onTpsChange` reachable to the linter; the no-op call path
// matters once Wave 2 wires the new selector.
export function fireTpsChangeListener(tps: number): void {
  if (onTpsChange) onTpsChange(tps);
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

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop 0`;

  const rail = installRail();

  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), simBridge, rail);

  installProfilerPanel(simBridge);
  installMonitorTab(simBridge);
  // v1.12: NN tab. Topology Apply respawns the worker via restart(); bucket
  // edits are live-applied through getBridge() inside the installer.
  installNnTab(() => simBridge, () => restart());

  // v1.13 Wave 1: media-player top-bar buttons (play/pause, restart,
  // auto-restart, settings, perf). All share the `.iconbtn` CSS class.
  // Order in DOM matches left-to-right visual order.
  installTopBarButtons(() => simBridge, () => restart(), rail);

  // v1.9.1: apply persisted rail open/closed state on boot. The class drives
  // the grid track collapse + #right-rail display:none (see styles.css).
  applyRailOpen(getSettings().railOpen);

  async function restart(): Promise<void> {
    const oldBridge = simBridge;
    simBridge = await spawnSimWorker("");
    oldBridge.terminate();
    resetStats();
    resetInspectorSelection(rail);
    highlights.clear();
    // v1.9.2 Wave 3: clear interpolation maps so the first frame after
    // restart doesn't lerp from positions in the dead worker's last
    // snapshot.
    resetInterpolation();
  }

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

  let framesThisSecond = 0;
  let fpsWindowStart = performance.now();
  let lastFps = -1;
  function frame(now: number): void {
    const frameSpan = span("frame");
    try {
      if (!controlI32 || !snapshotBuffer || !snapshotView) return;
      const readSpan = span("frame.snapshot.read");
      const rawSlot = Atomics.load(controlI32, CTRL_CURRENT_SLOT);
      const slot: 0 | 1 = rawSlot === 1 ? 1 : 0;
      const seq = Atomics.load(controlI32, CTRL_SEQ);
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
        seq,
        targetTPS,
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

// v1.13 Wave 1: small inline SVG glyphs for the top-bar icon buttons.
// Material/Heroicons-style line work, currentColor stroke so the
// `.iconbtn` hover/active rules tint them. Sized 18×18 inside a 36×36
// button.
const SVG_ATTRS =
  'viewBox="0 0 24 24" width="18" height="18" fill="none" ' +
  'stroke="currentColor" stroke-width="1.8" stroke-linecap="round" ' +
  'stroke-linejoin="round" aria-hidden="true"';
const ICON_PLAY = `<svg ${SVG_ATTRS}><path d="M7 5l12 7-12 7V5z" fill="currentColor" stroke="none"/></svg>`;
const ICON_PAUSE = `<svg ${SVG_ATTRS}><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>`;
const ICON_RESTART = `<svg ${SVG_ATTRS}><path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/></svg>`;
const ICON_AUTO_RESTART = `<svg ${SVG_ATTRS}><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 4v5h-5"/><circle cx="12" cy="12" r="1.6" fill="currentColor" stroke="none"/></svg>`;
const ICON_SETTINGS = `<svg ${SVG_ATTRS}><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>`;
const ICON_PERF = `<svg ${SVG_ATTRS}><rect x="2.5" y="3.5" width="19" height="13" rx="1.5"/><path d="M8 20.5h8M12 16.5v4"/><path d="M5.5 13.5l3-3 2.5 2.5 3.5-5 3.5 3.5"/></svg>`;

function makeIconBtn(id: string, title: string, html: string): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.id = id;
  btn.className = "iconbtn";
  btn.title = title;
  btn.setAttribute("aria-label", title);
  btn.innerHTML = html;
  return btn;
}

// v1.13 Wave 1: install all five top-bar icon buttons in one pass and
// register the keyboard shortcuts (space → pause/play). Highlight state
// for the three "toggleable" buttons (auto-restart, settings, perf) is
// refreshed by a low-rate interval — cheaper than wiring a subscription
// into perf-panel / devpanel and good enough for visual sync.
function installTopBarButtons(
  getBridge: () => SimBridge,
  onRestart: () => void,
  rail: RailState,
): void {
  const bar = document.getElementById("top-bar");
  if (!bar) return;

  // 1. Play / pause — single button, glyph swaps with state.
  const playBtn = makeIconBtn("playpause-btn", "Play / pause (space)", paused ? ICON_PLAY : ICON_PAUSE);
  const refreshPlayGlyph = (): void => {
    playBtn.innerHTML = paused ? ICON_PLAY : ICON_PAUSE;
    playBtn.classList.toggle("is-active", paused);
  };
  playBtn.addEventListener("click", () => {
    paused = !paused;
    getBridge().setPaused(paused);
    refreshPlayGlyph();
  });
  window.addEventListener("keydown", (e) => {
    if (e.key !== " " && e.code !== "Space") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    paused = !paused;
    getBridge().setPaused(paused);
    refreshPlayGlyph();
  });

  // 2. Restart — fires the same restart() the `r` hotkey wires to.
  const restartBtn = makeIconBtn("restart-btn", "Restart simulation (r)", ICON_RESTART);
  restartBtn.addEventListener("click", onRestart);

  // 3. Auto-restart — toggles Settings.autoRun. Highlight follows the
  //    persisted value (which the devpanel can also flip).
  const autoBtn = makeIconBtn("auto-restart-btn", "Auto-restart on world end", ICON_AUTO_RESTART);
  autoBtn.addEventListener("click", () => {
    const next = !getSettings().autoRun;
    setSetting("autoRun", next);
    autoBtn.classList.toggle("is-active", next);
  });

  // 4. Settings — toggles the rail open/closed (same as `~` hotkey).
  //    Highlighted only when the rail is open AND showing the settings
  //    tab — otherwise the rail is acting as Inspector or NN.
  const settingsBtn = makeIconBtn("settings-btn", "Toggle settings rail (~)", ICON_SETTINGS);
  settingsBtn.addEventListener("click", () => toggleRailOpen());

  // 5. Perf — flips Settings.showProfiler and re-applies via the
  //    perf-panel's single source-of-truth setter.
  const perfBtn = makeIconBtn("perf-btn", "Toggle profiler panel", ICON_PERF);
  perfBtn.addEventListener("click", () => {
    const next = !getSettings().showProfiler;
    setSetting("showProfiler", next);
    setProfilerVisible(next);
    perfBtn.classList.toggle("is-active", next);
  });

  bar.append(playBtn, restartBtn, autoBtn, settingsBtn, perfBtn);

  // Initial highlight pass + low-rate sync. Polling at 4 Hz keeps the
  // three reactive buttons in step with state changes coming from
  // elsewhere (devpanel autoRun row, rail tab click, etc.) without
  // forcing us to plumb subscriptions through the perf-panel / devpanel
  // / rail modules.
  const refreshHighlights = (): void => {
    autoBtn.classList.toggle("is-active", getSettings().autoRun);
    settingsBtn.classList.toggle(
      "is-active",
      getSettings().railOpen && rail.activeTab === "settings",
    );
    perfBtn.classList.toggle("is-active", getSettings().showProfiler);
  };
  refreshPlayGlyph();
  refreshHighlights();
  window.setInterval(refreshHighlights, 250);
}

main().catch((err) => {
  status.textContent = `Boot failed: ${err}`;
  console.error(err);
});
