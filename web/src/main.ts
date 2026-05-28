// v1.6 Wave C (Stage 2): main thread holds NO wasm and copies NO snapshot.
// Sim writes the inactive snapshot slot in the SharedArrayBuffer and atomically
// flips `CTRL_CURRENT_SLOT`; main per-RAF reads the live slot via typed-array
// views — zero structured-clone, zero memcpy.
//
// Per v1.6-plan.md §"Step C":
//   - On `boot_ready`, stash `controlSab` / `snapshotSab` handles.
//   - Each RAF: open `frame.snapshot.read` span, atomic-load the live slot,
//     build creature + grass typed-array views, read the stats header, call
//     `renderWorld(...)`.
//   - Restart = worker.terminate() + new Worker; old SAB views are GC'd along
//     with the previous bridge. Render keeps painting the last-good frame
//     during the worker re-init blip (the previous SAB stays GC-rooted while
//     the new boot_ready is in flight).
//   - `initial_sliders` is sourced from `currentSliderState()` (in-memory
//     widget values), NOT `getSettings()` localStorage — so a mid-drag
//     restart carries the dragged value. (Wave B gotcha 4)
//   - `__world` debug hook is gone (Wave B gotcha 6).

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
  currentSliderState,
} from "./widgets/devpanel";
import { installCanvasClickHandler, resetInspectorSelection } from "./rail/inspector";
import { resetStats } from "./rail/stats";
import { span } from "./perf";
import { getSettings, setSetting } from "./settings";
import {
  SimBridge,
  MAX_POP_FOR_SAB,
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

// SAB handles received via `boot_ready`. Per Wave C, the sim worker writes
// the inactive slot of `snapshotSab` and bumps `controlI32[CTRL_CURRENT_SLOT]`;
// main reads the live slot each RAF. Both are non-null after a successful
// boot_ready handshake; main keeps painting the previous frame during the
// brief restart blip while they're still bound to the *previous* worker's
// SABs (the new bridge's boot_ready will replace them atomically).
let controlSab: SharedArrayBuffer | null = null;
let snapshotSab: SharedArrayBuffer | null = null;
let controlI32: Int32Array | null = null;
let snapshotView: DataView | null = null;
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

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop 0`;

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
      // Wave C zero-copy snapshot read. `frame.snapshot.read` wraps the
      // atomic-load + typed-array view construction. Construction itself is
      // ~free (no copy); the span is here so its absence vs presence shows up
      // in the perf-tree without changing the constant cost.
      if (!controlI32 || !snapshotSab || !snapshotView) {
        // boot_ready hasn't landed yet (or a restart is in flight and the
        // previous bridge has been torn down). Skip render this frame.
        return;
      }
      const readSpan = span("frame.snapshot.read");
      const rawSlot = Atomics.load(controlI32, CTRL_CURRENT_SLOT);
      // Defensive: the sim worker only ever stores 0 or 1 here, but clamp so
      // a stale or corrupted control word can't blow up `Float32Array` ctor.
      const slot: 0 | 1 = rawSlot === 1 ? 1 : 0;
      const header = readSnapshotHeader(snapshotView, slotOffset(slot));
      // Build typed-array views over the live slot's regions. These views
      // alias the SAB — no copy. They are scoped to this frame; next frame
      // builds new ones (cheap, ~30 ns each) so a mid-frame slot flip doesn't
      // leak across frames.
      const pop = Math.min(header.pop, MAX_POP_FOR_SAB);
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

      pollRail(rail, header, simBridge);
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

      if (now - lastStatusUpdate > 200) {
        lastStatusUpdate = now;
        const endedSuffix = header.world_ended ? "  (world ended)" : "";
        status.textContent =
          `seed: ${cachedSeed}  ·  tick ${header.tick}  ·  pop ${header.pop}${endedSuffix}`;
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

  // Wave C: snapshots are SAB-backed, not postMessage'd. The Stage-1
  // `snapshot` reply is gone, so we don't register an `onSnapshot` handler;
  // SimBridge silently drops any stale snapshot that arrives (won't happen
  // post-Wave C, but the bridge tolerates it).

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
  // Wave C: SAB handles are now mandatory. boot_ready posts them only after
  // the worker has run one tick + written one snapshot to slot 0, so the very
  // first frame after handshake is guaranteed to see a populated live slot.
  if (!ready.snapshot_sab || !ready.control_sab) {
    throw new Error(
      "[boot] sim worker did not deliver SAB handles (snapshot_sab/control_sab " +
      "null). Wave C requires both — check sim-worker.ts boot path.",
    );
  }
  controlSab = ready.control_sab;
  snapshotSab = ready.snapshot_sab;
  controlI32 = new Int32Array(controlSab);
  snapshotView = new DataView(snapshotSab);
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
