// Main thread entry. The renderer holds the only WebGL context and reads
// snapshots from the SAB the sim worker writes; main never holds a wasm
// instance. Slider mutations / pause / TPS / inspect-requests go through
// the SimBridge.

import { makeCamera } from "./render";
import { renderWorld } from "./render-gl";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights, type RailState } from "./rail/index";
import {
  installProfilerPanel,
  setProfilerVisible,
  setPanelStatus,
  resetPanelSamples,
  setPanelBridge,
} from "./widgets/perf-panel";
import {
  installDevPanel,
  getInitialGrassSeedCount,
  getEnergyMax,
  getFounderCount,
  getFullGrassOnInit,
  getWorldSize,
  getWrapWorld,
  getWorldSeed,
  getSpeciesMode,
  getCrossoverMode,
  getStartingSpeciesCount,
  getStartingSpeciesMemberCount,
  getStartingSpeciesMemberVariance,
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
  type WindowMetadata,
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
// v2.0 Wave 1a: the numeric biome world_seed actually in use (resolved by
// Rust when the user leaves it at 0). Surfaced in the top-left status strip
// and reused verbatim on restart unless the user rerolls.
let latestWorldSeed = 0;
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

  // v2.0 Wave 1a: seed the next-world biome seed from the persisted Settings
  // value (0 ⇒ Rust randomizes on first boot and reports it back, which we
  // then capture so restarts reuse the same biome until the user rerolls).
  pendingWorldSeed = getWorldSeed();

  let simBridge = await spawnSimWorker(urlSeed);

  const cam = makeCamera(latestSnapshotWorldSize());
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => latestSnapshotWorldSize(),
  );

  const rail = installRail();

  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), simBridge, rail);

  installProfilerPanel(simBridge);
  // v1.13 Wave 2: the right-rail Monitor tab is gone. Its population graph
  // and per-worker stats now live in the bottom perf panel.
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

  // v1.13 Wave 3: drag-to-resize handles for the right rail width and the
  // bottom profiler panel height. Installed after applyRailOpen so the
  // initial visibility state is correct.
  installResizeHandles();

  // v2.0 Wave 1a: top-left status-strip reroll → fresh biome seed + restart.
  // A plain restart keeps `pendingWorldSeed` (same biome); reroll randomizes it.
  const seedRerollBtn = document.getElementById("seed-reroll-btn");
  seedRerollBtn?.addEventListener("click", () => {
    // Pick a fresh non-zero u32 (0 means "let Rust randomize", which we want
    // to avoid here so the displayed seed is the one actually used).
    let s = (Math.floor(Math.random() * 0xffff_fffe) + 1) >>> 0;
    if (s === 0) s = 1;
    pendingWorldSeed = s;
    void restart();
  });

  async function restart(): Promise<void> {
    const oldBridge = simBridge;
    simBridge = await spawnSimWorker("");
    oldBridge.terminate();
    // v2.0 Wave 5: re-point the perf panel's species-table poll at the new
    // bridge (the old one is now terminated, its species report frozen).
    setPanelBridge(simBridge);
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

  // v1.13 Wave 2: render-loop seq-gate. The renderer must never paint the
  // same snapshot twice — if `seq === lastPaintedSeq`, the RAF callback
  // reschedules itself and returns before doing any work. This keeps the
  // invariant FPS ≤ TPS at all times. The FPS counter (painted-frame
  // semantics) is owned by the perf panel via setPanelStatus(); we no
  // longer count one FPS per RAF.
  let lastPaintedSeq = -1;
  // Camera snapshot used to detect pans/zooms that need a repaint while
  // paused (seq frozen). Initialised to values that will never match the
  // real camera so the very first frame always paints.
  let lastPaintedCamX = NaN;
  let lastPaintedCamY = NaN;
  let lastPaintedCamZoom = NaN;
  function frame(_now: number): void {
    if (!controlI32 || !snapshotBuffer || !snapshotView || !slotLayout) {
      requestAnimationFrame(frame);
      return;
    }

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
    if (seq === lastPaintedSeq) {
      // No new snapshot since the last paint. Re-render only when the camera
      // moved (pan/zoom while paused) so the canvas reflects the new view.
      const camMoved =
        cam.cx !== lastPaintedCamX ||
        cam.cy !== lastPaintedCamY ||
        cam.zoom !== lastPaintedCamZoom;
      if (camMoved && slotLayout) {
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
        lastPaintedCamX = cam.cx;
        lastPaintedCamY = cam.cy;
        lastPaintedCamZoom = cam.zoom;
      }
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

      pollRail(rail, header, simBridge, creatures, pop);
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
      lastPaintedCamX = cam.cx;
      lastPaintedCamY = cam.cy;
      lastPaintedCamZoom = cam.zoom;

      setStatusStrip();
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
// v2.0.4 S2: runtime grass cell size (default 5.0; non-default when grass_size slider is moved).
let latestGrassCellSize = 5.0;
// v2.0 Wave 1a: the numeric biome seed to construct the NEXT world with. The
// status strip's reroll button and the Settings world_seed row write this;
// boot reads it (0 ⇒ Rust picks + reports a fresh one, which we then capture
// so subsequent restarts reuse the same biome unless the user rerolls).
let pendingWorldSeed = 0;
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
    // v2.0 Wave 1a: construction-only world shape. A non-zero `world_seed` in
    // Settings means the user pinned a specific biome seed — honor it. A 0
    // means "auto": reuse the last resolved/rerolled seed (`pendingWorldSeed`)
    // so the same biome returns across plain restarts, or 0 on the very first
    // boot (Rust then picks one and reports it back, which we capture).
    world_size: getWorldSize(),
    wrap_world: getWrapWorld(),
    world_seed: getWorldSeed() !== 0 ? getWorldSeed() : pendingWorldSeed,
    // v2.0 Wave 3b: species + sexual-mating construction settings (the 5 new
    // trailing `newWithFounderCount` args). `mating_cooldown_ticks` is a live
    // slider and rides `initial_sliders`, not this construction path.
    species_mode: getSpeciesMode(),
    crossover_mode: getCrossoverMode(),
    starting_species_count: getStartingSpeciesCount(),
    starting_species_member_count: getStartingSpeciesMemberCount(),
    starting_species_member_variance: getStartingSpeciesMemberVariance(),
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
  latestWorldSeed = ready.world_seed;
  // Reuse the resolved numeric seed on the next restart so the same biome
  // layout returns; a reroll/edit in Settings overrides `pendingWorldSeed`.
  pendingWorldSeed = ready.world_seed;
  // v2.0 Wave 1a: build the runtime slot geometry from the reported grass_dim
  // (the single source of truth) and bind the static biome layer view. The
  // biome buffer is `grass_dim²` u8 bytes in wasm linear memory.
  slotLayout = makeSlotLayout(ready.grass_dim);
  // v2.0.3 Stream 2d: biome tint now comes from the per-slot biome window
  // channel (mode-downsampled, appended after the grass region in each slot).
  // The biome_buf (full-field static, biome_buf_byte_offset/len) is no longer
  // read by the renderer — the slot carries the windowed biome data instead.
  // Stash the Rust-side slider defaults for the Wave D drift-guard e2e to
  // read. Cheap, only the test consumes it.
  (window as unknown as { __rustSlidersDefaults?: string }).__rustSlidersDefaults =
    ready.sliders_defaults_json;

  bridge.attachControlSab(controlSab);
  // Boot seeded initial paused / target TPS / sliders into the control SAB;
  // no post-boot mirror writes needed.

  return bridge;
}

// v2.0 Wave 1a: top-left status strip — surfaces the numeric biome world_seed
// without opening the dev panel. Updated each painted frame; cheap (only
// touches the DOM when the displayed seed actually changes).
let statusStripSeedShown = -1;
function setStatusStrip(): void {
  if (latestWorldSeed === statusStripSeedShown) return;
  statusStripSeedShown = latestWorldSeed;
  const el = document.getElementById("status-seed");
  if (el) el.textContent = `seed ${latestWorldSeed >>> 0}`;
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
const ICON_PERF = `<svg ${SVG_ATTRS}><rect x="2.5" y="3.5" width="19" height="13" rx="1.5"/><path d="M8 20.5h8M12 16.5v4"/><path d="M5.5 13.5l3-3 2.5 2.5 3.5-5 3.5 3.5"/></svg>`;
// Rail-tab icons — copied verbatim from the rail-tab SVGs in index.html so the
// top-bar buttons use the exact same visual language as the tabs they target.
const ICON_SETTINGS = `<svg ${SVG_ATTRS}><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>`;
const ICON_NN = `<svg ${SVG_ATTRS}><path d="M9.5 3.5c-1.4 0-2.5 1.1-2.5 2.5 0 .3.05.6.14.86C6 7.3 5 8.5 5 10c0 .5.1 1 .3 1.4C4.5 12 4 13 4 14c0 1.4 1 2.6 2.3 2.9.1 1.2 1.1 2.1 2.4 2.1.4 0 .8-.1 1.1-.3.3 1.1 1.3 1.8 2.5 1.8.1-1.1 1.1-2 2.2-2 0-1.4-1.1-2.5-2.5-2.5"></path><path d="M14.5 3.5c1.4 0 2.5 1.1 2.5 2.5 0 .3-.05.6-.14.86C18 7.3 19 8.5 19 10c0 .5-.1 1-.3 1.4.8.6 1.3 1.6 1.3 2.6 0 1.4-1 2.6-2.3 2.9-.1 1.2-1.1 2.1-2.4 2.1-.4 0-.8-.1-1.1-.3-.3 1.1-1.3 1.8-2.5 1.8V3.5c.7 0 1.4.3 1.8.8"></path><path d="M12 4v17"></path></svg>`;
const ICON_INSPECTOR = `<svg ${SVG_ATTRS}><circle cx="11" cy="11" r="7"></circle><path d="M21 21l-5-5"></path></svg>`;

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

// Top bar lives in the top-right corner: text labels for the three
// primary actions (Play/Pause, Restart, Auto-restart) + icon buttons for the
// bottom-panel toggle and three right-rail openers (Settings, NN, Inspector).
// Highlight state for the toggleable buttons (auto-restart, perf, rail) is
// refreshed by a low-rate interval.
function installTopBarButtons(
  getBridge: () => SimBridge,
  onRestart: () => void,
  rail: RailState,
): void {
  const bar = document.getElementById("top-bar");
  if (!bar) return;

  // 1. Play / pause — text swaps based on state.
  const playBtn = makeTextBtn("playpause-btn", paused ? "Play" : "Pause", "Play / pause (space)");
  const refreshPlayLabel = (): void => {
    playBtn.textContent = paused ? "Play" : "Pause";
    playBtn.classList.toggle("is-active", paused);
  };
  playBtn.addEventListener("click", () => {
    paused = !paused;
    getBridge().setPaused(paused);
    refreshPlayLabel();
  });
  window.addEventListener("keydown", (e) => {
    if (e.key !== " " && e.code !== "Space") return;
    if (e.target instanceof HTMLInputElement) return;
    if (e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    paused = !paused;
    getBridge().setPaused(paused);
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

  // 4. Perf — bottom panel toggle.
  const perfBtn = makeIconBtn("perf-btn", "Toggle profiler panel", ICON_PERF);
  perfBtn.addEventListener("click", () => {
    const next = !getSettings().showProfiler;
    setSetting("showProfiler", next);
    setProfilerVisible(next);
    perfBtn.classList.toggle("is-active", next);
  });

  // 5-7. Three rail-panel openers: Settings, NN, Inspector.
  //   Each button opens the rail (if collapsed) AND switches to its tab.
  //   Active style (.iconbtn.is-active) is shown when the rail is open AND
  //   that button's tab is the current active one. The 250 ms refreshHighlights
  //   interval keeps this accurate after external changes (~ hotkey, canvas
  //   inspector click).
  const openRailTab = (tab: import("./rail/index").RailTab): void => {
    setRailOpen(true);
    rail.switchTab(tab);
  };

  const settingsBtn = makeIconBtn("settings-rail-btn", "Settings (~)", ICON_SETTINGS);
  settingsBtn.addEventListener("click", () => openRailTab("settings"));

  const nnBtn = makeIconBtn("nn-rail-btn", "Neural network", ICON_NN);
  nnBtn.addEventListener("click", () => openRailTab("nn"));

  const inspectorBtn = makeIconBtn("inspector-rail-btn", "Inspector", ICON_INSPECTOR);
  inspectorBtn.addEventListener("click", () => openRailTab("inspector"));

  bar.append(playBtn, restartBtn, autoBtn, perfBtn, settingsBtn, nnBtn, inspectorBtn);

  const refreshHighlights = (): void => {
    const railOpen = getSettings().railOpen;
    autoBtn.classList.toggle("is-active", getSettings().autoRun);
    perfBtn.classList.toggle("is-active", getSettings().showProfiler);
    settingsBtn.classList.toggle("is-active", railOpen && rail.activeTab === "settings");
    nnBtn.classList.toggle("is-active", railOpen && rail.activeTab === "nn");
    inspectorBtn.classList.toggle("is-active", railOpen && rail.activeTab === "inspector");
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
    const statusLine = document.getElementById("perf-status-line");
    if (statusLine) statusLine.textContent = `Boot failed: ${err}`;
  } catch { /* ignore */ }
});
