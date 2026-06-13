// Main thread entry. The renderer holds the only WebGL context and reads
// snapshots from the SAB the sim worker writes; main never holds a wasm
// instance. Slider mutations / pause / TPS / inspect-requests go through
// the SimBridge.

import { makeCamera } from "./render/scene";
import { renderWorld } from "./render/gl";
import { attachCameraControls } from "./render/camera";
import { installRail, pollRail, highlights, type RailState } from "./rail/index";
import {
  installProfilerPanel,
  setPanelStatus,
  resetPanelSamples,
  setPanelBridge,
} from "./widgets/perf-panel";
import {
  installDevPanel,
  getWorldSeed,
  currentWorldConfig,
  currentSliderState,
} from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection, installInspectorE2EHook, refreshInspector } from "./rail/inspector";
import { installNnTab } from "./rail/nn-tab";
import { span } from "./perf";
import { getSettings, setSetting, hasStoredSetting } from "./settings";
import { applyTheme } from "./themes";
import { showToast } from "./toast";
import {
  latestWorldSave,
  metadataFromArtifact,
  putAutosave,
  putNamedSave,
  withAppMetadata,
} from "./storage/world-saves";
import packageJson from "../package.json";
import {
  SimBridge,
  MAX_POP_FOR_SIM,
  CTRL_CONSUMED_SEQ,
  CTRL_CURRENT_SLOT,
  CTRL_NN_STATS_EPOCH,
  CTRL_PROFILE_REPORT_EPOCH,
  CTRL_SEQ,
  CREATURE_STRIDE,
  makeSlotLayout,
  creatureSoAOffset,
  grassOffset,
  biomeWinOffset,
  slotOffset,
  readSnapshotHeader,
  readWindowMetadata,
  CTRL_CAMERA_CX_BITS,
  CTRL_CAMERA_CY_BITS,
  CTRL_CAMERA_ZOOM_BITS,
  CTRL_CAMERA_VIEWPORT_W,
  CTRL_CAMERA_VIEWPORT_H,
  type SlotLayout,
  type SimReplyBootReady,
  type WorkerDebugFault,
  type WindowMetadata,
} from "./sim/bridge";

const APP_VERSION = packageJson.version;
const WORKER_BOOT_TIMEOUT_MS = 15_000;
const WORKER_STALL_TIMEOUT_MS = 3_500;
const MAX_AUTO_RECOVERY_ATTEMPTS = 2;
const RECOVERY_ATTEMPT_STABLE_MS = 2_000;
const AUTOSAVE_INTERVAL_MS = 30_000;

type WorkerHealthState =
  | "booting"
  | "running"
  | "paused"
  | "stalled"
  | "crashed"
  | "restarting"
  | "failed";

type WorkerFaultKind = "boot_timeout" | "error" | "messageerror" | "stall";

interface SpawnWorkerOptions {
  debugFault?: WorkerDebugFault | null;
  savedStateJson?: string | null;
  savedStateLoadMode?: "resume" | "fork";
  bootTimeoutMs?: number;
  onWorkerFault?: (kind: Exclude<WorkerFaultKind, "boot_timeout">, detail: string) => void;
}

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

// v1.13: TPS selector lives only in the perf panel (top-bar dropdown gone
// in Wave 1). Devpanel's upkeep-per-second readout also listens so it can
// scale its display with the current TPS.
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
 * The perf panel's TPS selector calls this to update main's mirrored
 * target-TPS and fan out to listeners.
 */
export function setExternalTargetTPS(tps: number): void {
  if (!Number.isFinite(tps) || tps < 1) return;
  targetTPS = tps;
  fireTpsChange(tps);
}

let controlSab: SharedArrayBuffer | null = null;
let controlI32: Int32Array | null = null;
// v2.0.3 Stream 2b: Float32Array view over the same control SAB for writing
// camera lanes as f32-bits (same pattern as slider writes).
let controlF32: Float32Array | null = null;
// v2.0.3 Stream 2b: latest window metadata read from the consumed snapshot slot.
// Stored here; the renderer (2c) will read it to drive windowed texture upload.
let latestWindowMetadata: WindowMetadata | null = null;
/** v2.0.3 Stream 2b: accessor for the renderer (2c) to read the latest window
 *  metadata without importing the full frame state. Returns null before the
 *  first snapshot is consumed. */
export function getLatestWindowMetadata(): WindowMetadata | null {
  return latestWindowMetadata;
}
/**
 * v1.11 (A): snapshot region lives in wasm linear memory. These are
 * (re)constructed from the `WebAssembly.Memory.buffer` returned in
 * `boot_ready`. Each restart spawns a new worker with new wasm memory, so
 * we re-attach on every successful boot.
 */
let snapshotBuffer: ArrayBufferLike | null = null;
let snapshotBaseOffset = 0;
let snapshotView: DataView | null = null;
// v2.0 Wave 1a: runtime slot geometry derived from the boot-time grass_dim.
// Rebuilt on every boot/restart since world dims can change between runs.
let slotLayout: SlotLayout | null = null;
// v2.0.3 Stream 2d: biome tint now comes from the per-slot biome window channel
// (mode-downsampled, same win_w × win_h as the grass window). The old static
// biomeView + biomeDirty approach is superseded — biomeWin is read from the
// snapshot slot each frame using biomeWinOffset(). No module-level state needed.
let cachedSeed = "";
let latestWrapWorld = true;

async function main(): Promise<void> {
  // v1.9.1: apply the persisted theme before any UI installer runs so the
  // first paint uses the user's chosen palette instead of flashing the
  // :root fallback (charcoal).
  applyTheme(getSettings().theme);

  // v1.13 Wave 3: apply persisted layout sizes to their CSS vars before any
  // UI installer runs so the grid uses the user's chosen widths from the
  // first paint (no flash of the 420 / 240 defaults).
  applyLayoutSizes();

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

  // Settings keeps the historical `worldSeed` key, but it now stores the
  // WorldConfig master seed. Zero means Rust resolves a random master seed and
  // reports it back for plain restarts.
  pendingMasterSeed = getWorldSeed();

  let simBridge: SimBridge;
  let workerState: WorkerHealthState = "booting";
  let workerGeneration = 0;
  let nextDebugFault: WorkerDebugFault | null = null;
  let recoveryInFlight = false;
  let automaticRecoveryAttempts = 0;
  let lastRecoveryStartedAtMs = -Infinity;
  let lastWorkerProgressAtMs = performance.now();
  let lastProgressSeq = -1;
  let lastProgressProfileEpoch = -1;
  let lastProgressNnEpoch = -1;

  const workerStatusUi = installWorkerStatusUi(() => {
    if (workerState !== "failed") return;
    automaticRecoveryAttempts = 0;
    void recoverWorker("boot_timeout", "manual retry");
  });
  const rail = installRail(setRailOpen);

  function setWorkerState(state: WorkerHealthState, reason = ""): void {
    workerState = state;
    document.body.dataset.workerState = state;
    workerStatusUi.update(state, reason);
  }

  function readProgressSignature(): [number, number, number] | null {
    if (!controlI32) return null;
    return [
      Atomics.load(controlI32, CTRL_SEQ),
      Atomics.load(controlI32, CTRL_PROFILE_REPORT_EPOCH),
      Atomics.load(controlI32, CTRL_NN_STATS_EPOCH),
    ];
  }

  function resetWatchdogProgress(now: number): void {
    const sig = readProgressSignature();
    lastProgressSeq = sig?.[0] ?? -1;
    lastProgressProfileEpoch = sig?.[1] ?? -1;
    lastProgressNnEpoch = sig?.[2] ?? -1;
    lastWorkerProgressAtMs = now;
  }

  function observeWorkerProgress(now: number): void {
    const sig = readProgressSignature();
    if (!sig) return;
    const [seq, profileEpoch, nnEpoch] = sig;
    if (
      seq !== lastProgressSeq ||
      profileEpoch !== lastProgressProfileEpoch ||
      nnEpoch !== lastProgressNnEpoch
    ) {
      lastProgressSeq = seq;
      lastProgressProfileEpoch = profileEpoch;
      lastProgressNnEpoch = nnEpoch;
      lastWorkerProgressAtMs = now;
      if (
        automaticRecoveryAttempts > 0 &&
        now - lastRecoveryStartedAtMs >= RECOVERY_ATTEMPT_STABLE_MS
      ) {
        automaticRecoveryAttempts = 0;
      }
    }
  }

  function checkWorkerWatchdog(now: number): void {
    if (!controlI32 || recoveryInFlight || workerState === "failed") return;
    observeWorkerProgress(now);
    if (paused) {
      if (workerState === "running") setWorkerState("paused");
      lastWorkerProgressAtMs = now;
      return;
    }
    if (workerState === "paused") {
      resetWatchdogProgress(now);
      setWorkerState("running");
      return;
    }
    if (workerState !== "running") return;
    if (now - lastWorkerProgressAtMs > WORKER_STALL_TIMEOUT_MS) {
      void recoverWorker("stall", "worker stopped publishing snapshots or reports");
    }
  }

  function bindWorkerFault(generation: number): SpawnWorkerOptions["onWorkerFault"] {
    return (kind, detail) => {
      if (generation !== workerGeneration || recoveryInFlight || workerState === "failed") return;
      void recoverWorker(kind, detail);
    };
  }

  async function spawnTrackedWorker(
    seed: string,
    savedStateJson: string | null = null,
    loadMode: "resume" | "fork" = "resume",
  ): Promise<SimBridge> {
    const generation = ++workerGeneration;
    const fault = nextDebugFault;
    nextDebugFault = null;
    setWorkerState(generation === 1 ? "booting" : "restarting");
    const bridge = await spawnSimWorker(seed, {
      debugFault: fault,
      savedStateJson,
      savedStateLoadMode: loadMode,
      bootTimeoutMs: WORKER_BOOT_TIMEOUT_MS,
      onWorkerFault: bindWorkerFault(generation),
    });
    if (generation !== workerGeneration) {
      bridge.terminate();
      throw new Error("[worker] stale boot_ready from superseded worker");
    }
    resetWatchdogProgress(performance.now());
    setWorkerState(paused ? "paused" : "running");
    return bridge;
  }

  function cleanupAfterWorkerSwap(rail: RailState): void {
    setPanelBridge(simBridge);
    resetPanelSamples();
    resetInspectorSelection(rail);
    highlights.clear();
    lastPaintedSeq = -1;
    lastPaintedAtMs = -Infinity;
    lastPaintedCamX = NaN;
    lastPaintedCamY = NaN;
    lastPaintedCamZoom = NaN;
    latestWindowMetadata = null;
    hideWorldEndOverlay();
  }

  async function restartWorker(rail: RailState, automatic: boolean): Promise<void> {
    let freshSeed = (Math.floor(Math.random() * 0xffff_fffe) + 1) >>> 0;
    if (freshSeed === 0) freshSeed = 1;
    pendingMasterSeed = freshSeed;
    const oldBridge = simBridge;
    const replacement = await spawnTrackedWorker("");
    simBridge = replacement;
    oldBridge.terminate();
    cleanupAfterWorkerSwap(rail);
    if (!automatic) automaticRecoveryAttempts = 0;
  }

  async function loadWorldArtifact(artifactJson: string, mode: "resume" | "fork"): Promise<void> {
    metadataFromArtifact(artifactJson);
    const oldBridge = simBridge;
    const replacement = await spawnTrackedWorker("", artifactJson, mode);
    simBridge = replacement;
    oldBridge.terminate();
    cleanupAfterWorkerSwap(rail);
    automaticRecoveryAttempts = 0;
    showToast(mode === "fork" ? "Forked saved world." : "Resumed saved world.", 2600);
  }

  async function recoverWorker(kind: WorkerFaultKind, detail: string): Promise<void> {
    if (recoveryInFlight || workerState === "failed") return;
    automaticRecoveryAttempts++;
    if (automaticRecoveryAttempts > MAX_AUTO_RECOVERY_ATTEMPTS) {
      setWorkerState("failed", detail);
      showToast("Simulation worker failed. Use Retry to start a new worker.", 6000);
      return;
    }

    recoveryInFlight = true;
    lastRecoveryStartedAtMs = performance.now();
    setWorkerState(kind === "stall" ? "stalled" : "crashed", detail);
    showToast("Simulation worker stopped. Restarting the simulation.", 3600);
    try {
      await restartWorker(rail, true);
      showToast("Simulation worker recovered.", 2600);
    } catch (err) {
      const nextDetail = err instanceof Error ? err.message : String(err);
      recoveryInFlight = false;
      void recoverWorker("boot_timeout", nextDetail);
      return;
    }
    recoveryInFlight = false;
  }

  setWorkerState("booting");
  simBridge = await spawnTrackedWorker(urlSeed);

  const cam = makeCamera(latestSnapshotWorldSize());
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => latestSnapshotWorldSize(),
  );

  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), simBridge, rail);
  // v2.1 P1 e2e: expose window.__evosimE2E.selectFirstCreature() for headless
  // specs that cannot reliably hit a creature with a blind canvas click.
  installInspectorE2EHook(rail, simBridge);

  installProfilerPanel(simBridge);
  // v1.13 Wave 2: the right-rail Monitor tab is gone. Its population graph
  // and per-worker stats now live in the bottom perf panel.
  // v1.12: NN tab. Topology Apply respawns the worker via restart(); bucket
  // edits are live-applied through getBridge() inside the installer.
  installNnTab(() => simBridge, () => restart());

  // v1.13 Wave 1: media-player top-bar buttons (play/pause, restart,
  // auto-restart, settings, perf). All share the `.iconbtn` CSS class.
  // Order in DOM matches left-to-right visual order.
  const persistenceUi = installPersistenceUi(
    () => simBridge,
    (artifact, mode) => loadWorldArtifact(artifact, mode),
  );

  installTopBarButtons(() => simBridge, () => restart(), rail, () => {
    if (workerState === "running" || workerState === "paused") {
      resetWatchdogProgress(performance.now());
      setWorkerState(paused ? "paused" : "running");
    }
  });

  // v1.9.1: apply persisted rail open/closed state on boot. The class drives
  // the grid track collapse + #right-rail display:none (see styles.css).
  applyRailOpen(getSettings().railOpen);

  // v1.13 Wave 3: drag-to-resize handles for the right rail width and the
  // bottom profiler panel height. Installed after applyRailOpen so the
  // initial visibility state is correct.
  installResizeHandles();

  // v2.0.5 S5: #seed-reroll-btn removed from DOM (status strip removed).
  // Restart always rerolls — see restart() below.

  async function restart(): Promise<void> {
    try {
      await restartWorker(rail, false);
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      setWorkerState("failed", detail);
      showToast("Simulation restart failed. Use Retry to try again.", 6000);
    }
  }

  {
    const ns =
      (window as unknown as { __evosimE2E?: Record<string, unknown> }).__evosimE2E ??
      ((window as unknown as { __evosimE2E: Record<string, unknown> }).__evosimE2E = {});
    ns["getWorkerState"] = (): WorkerHealthState => workerState;
    ns["simulateWorkerCrash"] = (): void => {
      nextDebugFault = "crash_after_boot";
      void restart();
    };
    ns["simulateWorkerFreeze"] = (): void => {
      nextDebugFault = "freeze_after_boot";
      void restart();
    };
    ns["simulateWorkerBootTimeout"] = (): void => {
      nextDebugFault = "boot_timeout";
      void restart();
    };
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

  // v1.13 Wave 4: Escape clears the inspector selection. The clear emits
  // the visibility event that hides the Inspector tab + falls back to NN
  // when Inspector was active.
  window.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    resetInspectorSelection(rail);
  });

  let autoRestartPending = false;
  let lastAutosaveAtMs = performance.now();
  let lastAutosaveTick = -1;
  let autosaveInFlight = false;

  function maybeAutosave(now: number, tick: number): void {
    if (autosaveInFlight || tick === lastAutosaveTick) return;
    if (now - lastAutosaveAtMs < AUTOSAVE_INTERVAL_MS) return;
    autosaveInFlight = true;
    lastAutosaveAtMs = now;
    lastAutosaveTick = tick;
    void simBridge.requestWorldArtifact()
      .then(async (artifact) => {
        if (!artifact) throw new Error("save request timed out");
        const record = await putAutosave(artifact, APP_VERSION);
        persistenceUi.setStatus(`autosaved t${record.tick}`);
      })
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        persistenceUi.setStatus(`autosave failed: ${message}`);
      })
      .finally(() => {
        autosaveInFlight = false;
      });
  }

  // v1.13 Wave 2: render-loop seq-gate. The renderer must never paint the
  // same snapshot twice — if `seq === lastPaintedSeq`, the RAF callback
  // reschedules itself and returns before doing any work. This keeps the
  // invariant FPS ≤ TPS at all times. The FPS counter (painted-frame
  // semantics) is owned by the perf panel via setPanelStatus(); we no
  // longer count one FPS per RAF.
  let lastPaintedSeq = -1;
  let lastPaintedAtMs = -Infinity;
  // Camera snapshot used to detect pans/zooms that need a repaint while
  // paused (seq frozen). Initialised to values that will never match the
  // real camera so the very first frame always paints.
  let lastPaintedCamX = NaN;
  let lastPaintedCamY = NaN;
  let lastPaintedCamZoom = NaN;
  function frame(now: number): void {
    if (!controlI32 || !snapshotBuffer || !snapshotView || !slotLayout) {
      requestAnimationFrame(frame);
      return;
    }
    checkWorkerWatchdog(now);

    // v2.0.3 Stream 2b: write camera SAB lanes each RAF so the worker has an
    // up-to-date view of camera state when it calls write_snapshot.
    // f32-bits for cx/cy/zoom (same pattern as slider writes in SimBridge);
    // u32 for viewport width/height.
    if (controlF32) {
      controlF32[CTRL_CAMERA_CX_BITS]   = cam.cx;
      controlF32[CTRL_CAMERA_CY_BITS]   = cam.cy;
      controlF32[CTRL_CAMERA_ZOOM_BITS] = cam.zoom;
      Atomics.store(controlI32, CTRL_CAMERA_VIEWPORT_W, viewW >>> 0);
      Atomics.store(controlI32, CTRL_CAMERA_VIEWPORT_H, viewH >>> 0);
    }

    const seq = Atomics.load(controlI32, CTRL_SEQ);
    const appFPS = getSettings().appFPS;
    const appFrameIntervalMs = 1000 / appFPS;
    const dueForAppFrame = now - lastPaintedAtMs >= appFrameIntervalMs - 0.5;
    if (seq === lastPaintedSeq) {
      // No new snapshot since the last paint. Re-render only when the camera
      // moved (pan/zoom while paused) so the canvas reflects the new view.
      // v2.1 P1: always call refreshInspector even when seq is frozen so the
      // NN I/O fetch can fire while paused (it's serialised inside
      // refreshInspector and would otherwise never run on a paused world).
      refreshInspector(simBridge, rail, paused);
      const camMoved =
        cam.cx !== lastPaintedCamX ||
        cam.cy !== lastPaintedCamY ||
        cam.zoom !== lastPaintedCamZoom;
      if (paused && camMoved && slotLayout) {
        const layout = slotLayout;
        const rawSlot = Atomics.load(controlI32, CTRL_CURRENT_SLOT);
        const slot: 0 | 1 = rawSlot === 1 ? 1 : 0;
        const slotBaseOff = slotOffset(layout, slot);
        const header = readSnapshotHeader(snapshotView, slotBaseOff);
        // Also refresh window metadata on camera-pan repaint.
        latestWindowMetadata = readWindowMetadata(snapshotView, slotBaseOff);
        const pop = Math.min(header.pop, MAX_POP_FOR_SIM);
        const creatures = pop > 0
          ? new Float32Array(
              snapshotBuffer,
              snapshotBaseOffset + creatureSoAOffset(layout, slot),
              pop * CREATURE_STRIDE,
            )
          : new Float32Array(0);
        const grass = new Uint8Array(
          snapshotBuffer,
          snapshotBaseOffset + grassOffset(layout, slot),
          layout.grassRegionBytes,
        );
        // v2.0.3 Stream 2d: read the biome window from the slot (after grass).
        const biomeWin = new Uint8Array(
          snapshotBuffer,
          snapshotBaseOffset + biomeWinOffset(layout, slot),
          layout.biomeWinBytes,
        );
        renderWorld(
          gl!,
          cam,
          viewW,
          viewH,
          creatures,
          grass,
          // v2.0.3 Stream 2d: biome window from the slot (mode-downsampled).
          biomeWin,
          pop,
          latestWorldSize,
          latestGrassDim,
          // v2.0.4 S2: runtime grass cell size for UV transform.
          latestGrassCellSize,
          latestWrapWorld,
          highlights,
          // v2.0.3 Stream 2c: pass latest window metadata for UV transform.
          latestWindowMetadata,
        );
        Atomics.store(controlI32, CTRL_CONSUMED_SEQ, seq);
        lastPaintedAtMs = now;
        lastPaintedCamX = cam.cx;
        lastPaintedCamY = cam.cy;
        lastPaintedCamZoom = cam.zoom;
        setPanelStatus({
          seed: cachedSeed,
          tick: header.tick,
          pop: header.pop,
          tps: header.tps,
          worldEnded: !!header.world_ended,
        });
      }
      requestAnimationFrame(frame);
      return;
    }
    if (!dueForAppFrame) {
      requestAnimationFrame(frame);
      return;
    }
    const layout = slotLayout;

    const frameSpan = span("frame");
    try {
      const readSpan = span("frame.snapshot.read");
      const rawSlot = Atomics.load(controlI32, CTRL_CURRENT_SLOT);
      const slot: 0 | 1 = rawSlot === 1 ? 1 : 0;
      const slotBase = slotOffset(layout, slot);
      const header = readSnapshotHeader(snapshotView, slotBase);
      // v2.0.3 Stream 2b: read the window metadata from the slot header [32..64).
      // Stored for use by the renderer (2c wires it into the GPU upload path).
      latestWindowMetadata = readWindowMetadata(snapshotView, slotBase);
      const pop = Math.min(header.pop, MAX_POP_FOR_SIM);
      // v2.0 Wave 1a / v2.0.3 Stream 2b: snapshot region lives inside
      // wasm.memory.buffer at snapshotBaseOffset. Grass is a u8 clipmap window
      // of `grassRegionBytes` bytes (= min(grass_dim, 2048)² at default scale
      // equals grassCellCount — byte-identical to the pre-2b layout).
      const creatures = pop > 0
        ? new Float32Array(
            snapshotBuffer,
            snapshotBaseOffset + creatureSoAOffset(layout, slot),
            pop * CREATURE_STRIDE,
          )
        : new Float32Array(0);
      const grass = new Uint8Array(
        snapshotBuffer,
        snapshotBaseOffset + grassOffset(layout, slot),
        layout.grassRegionBytes,
      );
      // v2.0.3 Stream 2d: read the biome window from the slot (after grass).
      const biomeWin = new Uint8Array(
        snapshotBuffer,
        snapshotBaseOffset + biomeWinOffset(layout, slot),
        layout.biomeWinBytes,
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

      pollRail(rail, header, simBridge, creatures, pop, paused);
      renderWorld(
        gl!,
        cam,
        viewW,
        viewH,
        creatures,
        grass,
        // v2.0.3 Stream 2d: biome window from the snapshot slot (mode-downsampled,
        // same win_w × win_h as grass). Replaces the static biomeView path.
        biomeWin,
        pop,
        latestWorldSize,
        latestGrassDim,
        // v2.0.4 S2: runtime grass cell size for UV transform.
        latestGrassCellSize,
        latestWrapWorld,
        highlights,
        // v2.0.3 Stream 2c: pass latest window metadata for UV transform.
        latestWindowMetadata,
      );

      lastPaintedSeq = seq;
      Atomics.store(controlI32, CTRL_CONSUMED_SEQ, seq);
      lastPaintedAtMs = now;
      lastPaintedCamX = cam.cx;
      lastPaintedCamY = cam.cy;
      lastPaintedCamZoom = cam.zoom;

      setPanelStatus({
        seed: cachedSeed,
        tick: header.tick,
        pop: header.pop,
        tps: header.tps,
        worldEnded: !!header.world_ended,
      });
      maybeAutosave(now, header.tick);
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
// v2.0.4 S2: runtime grass cell size (default 5.0; non-default when grass_size slider is moved).
let latestGrassCellSize = 5.0;
// Master seed for the next world. Zero means Rust resolves one at construction.
let pendingMasterSeed = 0;
function latestSnapshotWorldSize(): number {
  return latestWorldSize;
}

function hashSeedString(seed: string): number {
  let h = 2166136261 >>> 0;
  for (let i = 0; i < seed.length; i++) {
    h ^= seed.charCodeAt(i);
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h === 0 ? 1 : h;
}

async function spawnSimWorker(
  seed: string,
  options: SpawnWorkerOptions = {},
): Promise<SimBridge> {
  const w = new Worker(new URL("./sim/worker.ts", import.meta.url), { type: "module" });
  const bridge = new SimBridge(w);

  let bootSettled = false;
  let bootTimer: ReturnType<typeof window.setTimeout> | null = null;
  const bootReady = new Promise<SimReplyBootReady>((resolve, reject) => {
    const failBoot = (err: Error): void => {
      if (bootSettled) return;
      bootSettled = true;
      if (bootTimer !== null) window.clearTimeout(bootTimer);
      w.terminate();
      reject(err);
    };
    bootTimer = window.setTimeout(() => {
      failBoot(new Error(`[boot] sim worker did not become ready within ${options.bootTimeoutMs ?? WORKER_BOOT_TIMEOUT_MS} ms`));
    }, options.bootTimeoutMs ?? WORKER_BOOT_TIMEOUT_MS);
    bridge.onBootReady((reply) => {
      if (bootSettled) return;
      bootSettled = true;
      if (bootTimer !== null) window.clearTimeout(bootTimer);
      resolve(reply);
    });
    w.addEventListener("error", (event) => {
      event.preventDefault();
      const message = event.message || "worker error";
      if (!bootSettled) {
        failBoot(new Error(`[worker] ${message}`));
        return;
      }
      options.onWorkerFault?.("error", message);
    });
    w.addEventListener("messageerror", () => {
      if (!bootSettled) {
        failBoot(new Error("[worker] messageerror during boot"));
        return;
      }
      options.onWorkerFault?.("messageerror", "worker messageerror");
    });
  });

  const pinnedSettingSeed = getWorldSeed();
  const masterSeed =
    seed !== ""
      ? hashSeedString(seed)
      : pinnedSettingSeed !== 0
        ? pinnedSettingSeed
        : pendingMasterSeed;
  cachedSeed = masterSeed === 0 ? "(random)" : String(masterSeed);
  bridge.sendBoot({
    kind: "boot",
    world_config: currentWorldConfig(masterSeed),
    initial_sliders: currentSliderState(),
    initial_target_tps: targetTPS,
    initial_paused: paused,
    saved_state_json: options.savedStateJson ?? undefined,
    saved_state_load_mode: options.savedStateLoadMode,
    debug_fault: options.debugFault ?? undefined,
  });

  const ready = await bootReady;
  if (ready.max_pop_for_sim !== MAX_POP_FOR_SIM) {
    throw new Error(
      `[boot] max_pop_for_sim mismatch: worker reported ${ready.max_pop_for_sim}, ` +
      `main expects ${MAX_POP_FOR_SIM}. Rebuild wasm (rustup run nightly wasm-pack ` +
      `build --target web --out-dir src/web/wasm --dev --features threads) — ` +
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
  // v2.0.3 Stream 2b: Float32Array view for camera lane writes (f32-bits).
  controlF32 = controlSab ? new Float32Array(controlSab) : null;
  // Fix minor #7: pre-seed the SAB camera lanes to world-center / zoom=1.0
  // so that the first snapshot the worker writes uses a sensible window rather
  // than cx=cy=0, zoom=0 (the SAB default). Without this, any world larger
  // than the viewport or at non-default zoom shows a one-frame artifact
  // (anchored top-left corner instead of world-center). We write before the
  // first RAF fires, using the real world_size from the boot reply.
  if (controlF32) {
    controlF32[CTRL_CAMERA_CX_BITS]   = ready.world_size / 2;
    controlF32[CTRL_CAMERA_CY_BITS]   = ready.world_size / 2;
    controlF32[CTRL_CAMERA_ZOOM_BITS] = 1.0;
  }
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
  // v2.0.4 S2: store the construction-time cell size so the renderer UV transform is correct.
  latestGrassCellSize = ready.grass_cell_size;
  latestWrapWorld = ready.wrap_world;
  pendingMasterSeed = ready.master_seed;
  cachedSeed = String(ready.master_seed);
  // v2.0 Wave 1a/2d: build the runtime slot geometry from the reported grass_dim
  // (the single source of truth). Biome tint comes from the per-slot biome
  // window appended after grass in each snapshot slot.
  slotLayout = makeSlotLayout(ready.grass_dim);
  // Stash the Rust-side slider defaults for the Wave D drift-guard e2e to
  // read. Cheap, only the test consumes it.
  (window as unknown as { __rustSlidersDefaults?: string }).__rustSlidersDefaults =
    ready.sliders_defaults_json;

  bridge.attachControlSab(controlSab);
  {
    const ns =
      (window as unknown as { __evosimE2E?: Record<string, unknown> }).__evosimE2E ??
      ((window as unknown as { __evosimE2E: Record<string, unknown> }).__evosimE2E = {});
    ns["getSnapshotSeq"] = (): number => controlI32 ? Atomics.load(controlI32, CTRL_SEQ) : -1;
    ns["getConsumedSeq"] = (): number =>
      controlI32 ? Atomics.load(controlI32, CTRL_CONSUMED_SEQ) : -1;
    ns["getAppFPS"] = (): number => getSettings().appFPS;
  }
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
// v2.1 P4: only the settings ⚙ icon is needed in the top bar (NN/Inspector/perf
// openers removed). Keep ICON_SETTINGS for the ⚙ rail toggle button.
const ICON_SETTINGS = `<svg ${SVG_ATTRS}><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>`;

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

function makeTextBtn(id: string, label: string, title: string): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.id = id;
  btn.className = "topbar-btn";
  btn.title = title;
  btn.textContent = label;
  return btn;
}

function downloadText(filename: string, mime: string, text: string): void {
  const blob = new Blob([text], { type: `${mime};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

function installPersistenceUi(
  getBridge: () => SimBridge,
  loadArtifact: (artifactJson: string, mode: "resume" | "fork") => Promise<void>,
): { setStatus: (message: string) => void } {
  const bar = document.getElementById("top-bar");
  const status = document.createElement("span");
  status.id = "save-status";
  status.className = "topbar-btn";
  status.textContent = "save: idle";
  status.title = "World save/autosave status";

  const setStatus = (message: string): void => {
    status.textContent = `save: ${message}`;
    status.title = status.textContent;
  };

  const requestArtifact = async (): Promise<string> => {
    setStatus("saving...");
    const artifact = await getBridge().requestWorldArtifact();
    if (!artifact) throw new Error("save request timed out");
    metadataFromArtifact(artifact);
    return withAppMetadata(artifact, APP_VERSION);
  };

  const saveBtn = makeTextBtn("world-save-btn", "Save", "Save named world state");
  saveBtn.addEventListener("click", () => {
    void requestArtifact()
      .then((artifact) => putNamedSave(artifact, APP_VERSION))
      .then((record) => {
        setStatus(`saved t${record.tick}`);
        showToast("World saved.", 2200);
      })
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        setStatus(`save failed: ${message}`);
      });
  });

  const resumeBtn = makeTextBtn("world-resume-btn", "Resume", "Resume latest saved world");
  resumeBtn.addEventListener("click", () => {
    void latestWorldSave()
      .then((record) => {
        if (!record) throw new Error("no saved world");
        setStatus(`loading t${record.tick}...`);
        return loadArtifact(record.artifactJson, "resume");
      })
      .then(() => setStatus("resumed"))
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        setStatus(`resume failed: ${message}`);
      });
  });

  const forkBtn = makeTextBtn("world-fork-btn", "Fork", "Fork latest saved world");
  forkBtn.addEventListener("click", () => {
    void latestWorldSave()
      .then((record) => {
        if (!record) throw new Error("no saved world");
        setStatus(`forking t${record.tick}...`);
        return loadArtifact(record.artifactJson, "fork");
      })
      .then(() => setStatus("forked"))
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        setStatus(`fork failed: ${message}`);
      });
  });

  const exportBtn = makeTextBtn("world-export-btn", "Export", "Export current world artifact");
  exportBtn.addEventListener("click", () => {
    void requestArtifact()
      .then((artifact) => {
        const meta = metadataFromArtifact(artifact);
        downloadText(`evosim-world-t${meta.tick}.json`, "application/json", artifact);
        setStatus(`exported t${meta.tick}`);
      })
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        setStatus(`export failed: ${message}`);
      });
  });

  const importInput = document.createElement("input");
  importInput.id = "world-import-input";
  importInput.type = "file";
  importInput.accept = "application/json,.json";
  importInput.hidden = true;
  const importBtn = makeTextBtn("world-import-btn", "Import", "Import world artifact");
  importBtn.addEventListener("click", () => importInput.click());
  importInput.addEventListener("change", () => {
    const file = importInput.files?.[0];
    importInput.value = "";
    if (!file) return;
    void file.text()
      .then(async (text) => {
        const artifact = withAppMetadata(text, APP_VERSION);
        const meta = metadataFromArtifact(artifact);
        setStatus(`loading t${meta.tick}...`);
        await loadArtifact(artifact, "resume");
        try {
          await putNamedSave(artifact, APP_VERSION, `Imported t${meta.tick}`, "imported");
          setStatus(`imported t${meta.tick}`);
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          setStatus(`import loaded; save failed: ${message}`);
        }
      })
      .catch((err) => {
        const message = err instanceof Error ? err.message : String(err);
        setStatus(`import failed: ${message}`);
      });
  });

  if (bar) bar.append(saveBtn, resumeBtn, forkBtn, exportBtn, importBtn, status, importInput);
  return { setStatus };
}

function installWorkerStatusUi(onRetry: () => void): {
  update: (state: WorkerHealthState, reason: string) => void;
} {
  const setShown = (el: HTMLElement, shown: boolean): void => {
    el.hidden = !shown;
    el.style.display = shown ? "" : "none";
  };
  const bar = document.getElementById("top-bar");
  const status = document.createElement("span");
  status.id = "worker-status";
  status.className = "topbar-btn";
  setShown(status, false);

  const retry = makeTextBtn("worker-retry-btn", "Retry worker", "Retry simulation worker");
  setShown(retry, false);
  retry.addEventListener("click", onRetry);

  if (bar) bar.append(status, retry);

  return {
    update(state, reason) {
      if (state === "running" || state === "paused") {
        setShown(status, false);
        setShown(retry, false);
        return;
      }
      const labels: Record<Exclude<WorkerHealthState, "running" | "paused">, string> = {
        booting: "Worker: booting",
        stalled: "Worker: stalled",
        crashed: "Worker: crashed",
        restarting: "Worker: recovering",
        failed: "Worker: failed",
      };
      status.textContent = labels[state as Exclude<WorkerHealthState, "running" | "paused">];
      status.title = reason || status.textContent;
      setShown(status, true);
      setShown(retry, state === "failed");
    },
  };
}

function installAppBadge(): void {
  const wrap = document.getElementById("canvas-wrap");
  if (!wrap || document.getElementById("app-badge")) return;
  const badge = document.createElement("div");
  badge.id = "app-badge";
  badge.className = "topbar-btn app-badge";
  badge.textContent = `evosim v${APP_VERSION}`;
  wrap.appendChild(badge);
}

// v2.1 P4: Top bar trimmed to exactly four controls:
//   1. Play/Pause (pacing)
//   2. Restart (rerolls seed)
//   3. Auto-restart toggle
//   4. ⚙ Settings rail toggle (opens/closes the right rail)
//
// Removed from top bar: NN opener, Inspector opener, perf-toggle opener.
// NN is now a Settings category; Inspector stays click-to-open (creature click);
// Profiler is now a Settings category (no standalone toggle button needed).
function installTopBarButtons(
  getBridge: () => SimBridge,
  onRestart: () => void,
  rail: RailState,
  onPausedChange: () => void,
): void {
  const bar = document.getElementById("top-bar");
  if (!bar) return;
  installAppBadge();

  // 1. Play / pause — text swaps based on state.
  const playBtn = makeTextBtn("playpause-btn", paused ? "Play" : "Pause", "Play / pause (space)");
  const refreshPlayLabel = (): void => {
    playBtn.textContent = paused ? "Play" : "Pause";
    playBtn.classList.toggle("is-active", paused);
  };
  playBtn.addEventListener("click", () => {
    paused = !paused;
    getBridge().setPaused(paused);
    onPausedChange();
    refreshPlayLabel();
  });
  window.addEventListener("keydown", (e) => {
    if (e.key !== " " && e.code !== "Space") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    paused = !paused;
    getBridge().setPaused(paused);
    onPausedChange();
    refreshPlayLabel();
  });

  // 2. Restart.
  const restartBtn = makeTextBtn("restart-btn", "Restart", "Restart simulation (r)");
  restartBtn.addEventListener("click", onRestart);

  // 3. Auto-restart toggle.
  const autoBtn = makeTextBtn("auto-restart-btn", "Auto-restart", "Auto-restart on world end");
  autoBtn.addEventListener("click", () => {
    const next = !getSettings().autoRun;
    setSetting("autoRun", next);
    autoBtn.classList.toggle("is-active", next);
  });

  // 4. ⚙ Settings rail toggle. Toggles the rail open/closed (same as `~`
  //    hotkey). When the rail is open on the Settings tab a second click
  //    collapses it; otherwise it opens the rail and switches to Settings.
  const settingsBtn = makeIconBtn("settings-rail-btn", "Settings (~)", ICON_SETTINGS);
  settingsBtn.addEventListener("click", () => {
    if (getSettings().railOpen && rail.activeTab === "settings") {
      setRailOpen(false);
    } else {
      setRailOpen(true);
      rail.switchTab("settings");
    }
  });

  bar.append(playBtn, restartBtn, autoBtn, settingsBtn);

  const refreshHighlights = (): void => {
    const railOpen = getSettings().railOpen;
    autoBtn.classList.toggle("is-active", getSettings().autoRun);
    settingsBtn.classList.toggle("is-active", railOpen && rail.activeTab === "settings");
  };
  refreshPlayLabel();
  refreshHighlights();
  window.setInterval(refreshHighlights, 250);
}

// v1.13 Wave 3: layout-size CSS-var helpers. The two persisted Settings
// keys (railW, profilerH) mirror the CSS vars --rail-w and --profiler-h.
// applyLayoutSizes() runs at boot before any UI installer so the first
// paint uses the persisted widths; installResizeHandles() wires the drag
// strips that live-update the vars and persist on pointer-up.
const RAIL_W_MIN = 280;
const RAIL_W_MAX = 720;
const PROFILER_H_MIN = 160;
const PROFILER_H_MAX_FRAC = 0.6;

function applyLayoutSizes(): void {
  const s = getSettings();
  const railW = clamp(s.railW, RAIL_W_MIN, RAIL_W_MAX);
  const profilerH = clamp(s.profilerH, PROFILER_H_MIN, profilerHMax());
  document.documentElement.style.setProperty("--rail-w", `${railW}px`);
  document.documentElement.style.setProperty("--profiler-h", `${profilerH}px`);
}

function clamp(v: number, lo: number, hi: number): number {
  if (!isFinite(v)) return lo;
  return Math.min(hi, Math.max(lo, v));
}

function profilerHMax(): number {
  return Math.max(PROFILER_H_MIN, Math.floor(window.innerHeight * PROFILER_H_MAX_FRAC));
}

function installResizeHandles(): void {
  const railHandle = document.getElementById("rail-resize-handle");
  const perfHandle = document.getElementById("perf-resize-handle");
  const rail = document.getElementById("right-rail");

  // Right-rail width handle. Drag-left increases width (rail grows toward
  // the canvas). The rail's right edge is glued to the viewport; the live
  // width is `viewport_right - pointer_x`. Persist on pointer-up only.
  if (railHandle && rail) {
    let dragging = false;
    railHandle.addEventListener("pointerdown", (e) => {
      dragging = true;
      railHandle.classList.add("is-dragging");
      railHandle.setPointerCapture(e.pointerId);
      e.preventDefault();
    });
    railHandle.addEventListener("pointermove", (e) => {
      if (!dragging) return;
      const w = clamp(window.innerWidth - e.clientX, RAIL_W_MIN, RAIL_W_MAX);
      document.documentElement.style.setProperty("--rail-w", `${w}px`);
    });
    const finish = (e: PointerEvent): void => {
      if (!dragging) return;
      dragging = false;
      railHandle.classList.remove("is-dragging");
      try { railHandle.releasePointerCapture(e.pointerId); } catch { /* already released */ }
      const w = clamp(window.innerWidth - e.clientX, RAIL_W_MIN, RAIL_W_MAX);
      setSetting("railW", w);
    };
    railHandle.addEventListener("pointerup", finish);
    railHandle.addEventListener("pointercancel", finish);
  }

  // Bottom-panel height handle. Drag-up grows the profiler (the panel's
  // bottom is glued to the viewport bottom; the live height is
  // `viewport_bottom - pointer_y`). Max clamps to 60% of innerHeight so a
  // mid-drag window resize can't trap the panel covering the canvas.
  if (perfHandle) {
    let dragging = false;
    perfHandle.addEventListener("pointerdown", (e) => {
      dragging = true;
      perfHandle.classList.add("is-dragging");
      perfHandle.setPointerCapture(e.pointerId);
      e.preventDefault();
    });
    perfHandle.addEventListener("pointermove", (e) => {
      if (!dragging) return;
      const h = clamp(window.innerHeight - e.clientY, PROFILER_H_MIN, profilerHMax());
      document.documentElement.style.setProperty("--profiler-h", `${h}px`);
    });
    const finish = (e: PointerEvent): void => {
      if (!dragging) return;
      dragging = false;
      perfHandle.classList.remove("is-dragging");
      try { perfHandle.releasePointerCapture(e.pointerId); } catch { /* already released */ }
      const h = clamp(window.innerHeight - e.clientY, PROFILER_H_MIN, profilerHMax());
      setSetting("profilerH", h);
    };
    perfHandle.addEventListener("pointerup", finish);
    perfHandle.addEventListener("pointercancel", finish);
  }
}

main().catch((err) => {
  console.error(err);
  // Surface in the perf panel's status line if it's already wired up;
  // otherwise users see the error in DevTools.
  try {
    document.body.dataset.workerState = "failed";
    const workerStatus = document.getElementById("worker-status");
    if (workerStatus) {
      workerStatus.textContent = "Worker: failed";
      workerStatus.setAttribute("title", String(err));
      workerStatus.hidden = false;
      workerStatus.style.display = "";
    }
    const workerRetry = document.getElementById("worker-retry-btn") as HTMLButtonElement | null;
    if (workerRetry) {
      workerRetry.hidden = true;
      workerRetry.style.display = "none";
    }
    const statusLine = document.getElementById("perf-status-line");
    if (statusLine) statusLine.textContent = `Boot failed: ${err}`;
  } catch { /* ignore */ }
});
