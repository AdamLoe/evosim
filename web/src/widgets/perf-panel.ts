// Bottom monitor + profiler panel (v1.13 Wave 2).
//
// Replaces both the v1.12 "Profiler panel" and the right-rail Monitor tab.
// Top-to-bottom layout:
//
//   1. Status line — `seed: X · tick N · pop M · TPS · FPS · (world ended)?`
//      Migrated from the old `#status` span in the top bar; main.ts calls
//      `setPanelStatus(...)` once per painted frame.
//   2. Graphs row — combined FPS/TPS chart + population chart. Responsive
//      via CSS container queries; falls back to flex-wrap on browsers
//      without container-query support.
//   3. Selectors row — target TPS pills + numeric input, max-population
//      pills + numeric input.
//   4. CPU Process Monitor — per-worker table (rebuilt from worker-stats.ts).
//   5. Profile system — the existing four-tree tables + window selector +
//      reset button.
//
// Visibility is owned by the `showProfiler` setting (live-apply via the
// floating #perf-open button). When hidden, the 1 Hz profile poll skips
// the tree render but keeps the painted-frame samplers running so the
// graphs are populated the moment the panel reopens.

import type { SimBridge } from "../sim-bridge";
import {
  isProfilerEnabled,
  reportJson as tsPerfReport,
  resetFrameTree,
  setProfilerEnabled,
  setProfilerWindowMs,
} from "../perf";
import { getSettings, setSetting } from "../settings";
import { getTargetTPS, setTpsChangeListener, setExternalTargetTPS } from "../main";
import { installWorkerStatsPanel } from "./worker-stats";

const POLL_INTERVAL_MS = 1000;
const FPS_TPS_SAMPLE_CAP = 240;     // ~4s at 60 fps, ~12s at 20 fps
const POP_SAMPLE_CAP = 500;
const POP_SAMPLE_INTERVAL_TICKS = 10;

// TPS preset pills.
const TPS_PILLS = [10, 30, 60, 180, 500, 1000] as const;
// Max-population preset pills.
const MAX_POP_PILLS = [500, 1000, 2000, 4000, 8000, 16000] as const;

// v1.10: sim_worker is the outer-loop tree (read_input_sab / tick /
// write_output_sab.{snapshot,...}). The Rust-side write_snapshot_to cost
// nests under sim_worker.write_output_sab.snapshot instead of being its own
// top-level tree.
const TREE_ORDER = ["frame", "sim_worker", "tick", "nn", "grass_step"];

// ─── Profile bundle types ─────────────────────────────────────────────────

interface ProfNode {
  name: string;
  total_us: number;
  call_count: number;
  total_call_count: number;
  parent_call_count: number | null;
  effective_window_ms: number;
  children: ProfNode[];
}

interface ProfReport {
  now_ms: number;
  window_ms: number;
  enabled: boolean;
  tree: ProfNode[];
}

interface BundledProfile {
  profile: ProfReport;
  tps: number;
  jank_count: number;
  live_grass_cell_count: number;
  total_grass_density: number;
}

// ─── Module state ─────────────────────────────────────────────────────────

let panelVisible = false;
let lastBundle: BundledProfile | null = null;

// Status line element.
let statusLine: HTMLDivElement | null = null;

// FPS/TPS sample ring (one sample per painted frame).
interface FpsTpsSample { fps: number; tps: number; }
const fpsTpsSamples: FpsTpsSample[] = [];
let fpsTpsCanvas: HTMLCanvasElement | null = null;
let fpsTpsResizeObserver: ResizeObserver | null = null;

// Painted-frame FPS counter — sampled into a "last second" ring so the
// status line shows a stable number even when paint cadence is uneven.
const paintedFrameTimestamps: number[] = [];

// Population sample ring.
interface PopSample { tick: number; population: number; }
const popSamples: PopSample[] = [];
let lastPopSampledTick = -1;
let popCanvas: HTMLCanvasElement | null = null;
let popResizeObserver: ResizeObserver | null = null;
const CHART_HEIGHT_CSS = 120;

// TPS / max-pop selector references (for live syncing).
let tpsNumInput: HTMLInputElement | null = null;
let tpsPillButtons: HTMLButtonElement[] = [];
let maxPopNumInput: HTMLInputElement | null = null;
let maxPopPillButtons: HTMLButtonElement[] = [];

// CPU Process Monitor teardown.
let workerStatsTeardown: (() => void) | null = null;

// ─── Public API ───────────────────────────────────────────────────────────

/**
 * Set whether the perf+monitor panel is visible. Single source of truth for
 * both the persisted setting and the floating #perf-open toggle button.
 * Hidden state: DOM is display:none, samplers still run so the graphs and
 * profile counters fill in the moment the panel reappears.
 */
export function setProfilerVisible(visible: boolean): void {
  panelVisible = visible;
  const box = document.getElementById("perf-box");
  if (box) box.style.display = visible ? "" : "none";
  const openBtn = document.getElementById("perf-open");
  if (openBtn) openBtn.classList.toggle("is-active", visible);
  if (!isProfilerEnabled()) setProfilerEnabled(true);
  if (visible) {
    if (lastBundle) pollAndRenderTrees(lastBundle);
    redrawFpsTpsChart();
    redrawPopChart();
  }
}

/**
 * Called from main.ts on every painted frame. Updates the status line, the
 * FPS counter (painted-frame semantics), the FPS/TPS chart sampler, and
 * the population chart sampler. The pop sampler self-throttles to one
 * sample per `POP_SAMPLE_INTERVAL_TICKS` ticks.
 */
export function setPanelStatus(args: {
  seed: string;
  tick: number;
  pop: number;
  tps: number;
  worldEnded: boolean;
}): void {
  const now = performance.now();
  // Painted-frame FPS — count timestamps in the trailing 1 s window.
  paintedFrameTimestamps.push(now);
  while (paintedFrameTimestamps.length > 0 && now - paintedFrameTimestamps[0] > 1000) {
    paintedFrameTimestamps.shift();
  }
  const fps = paintedFrameTimestamps.length;

  // Status line text.
  if (statusLine) {
    const tpsStr = isFinite(args.tps) && args.tps > 0 ? args.tps.toFixed(0) : "—";
    const fpsStr = fps > 0 ? String(fps) : "—";
    const endedSuffix = args.worldEnded ? "  ·  (world ended)" : "";
    statusLine.textContent =
      `seed: ${args.seed}  ·  tick ${args.tick}  ·  pop ${args.pop}` +
      `  ·  ${tpsStr} TPS  ·  ${fpsStr} FPS${endedSuffix}`;
  }

  // FPS/TPS chart sample (one per painted frame).
  fpsTpsSamples.push({ fps, tps: isFinite(args.tps) && args.tps > 0 ? args.tps : 0 });
  if (fpsTpsSamples.length > FPS_TPS_SAMPLE_CAP) fpsTpsSamples.shift();

  // Pop chart sampler — gated by tick delta so high-TPS batches still get
  // sampled, but we don't spam the buffer with duplicates.
  const tick = args.tick;
  if (tick !== lastPopSampledTick) {
    if (lastPopSampledTick < 0 || tick - lastPopSampledTick >= POP_SAMPLE_INTERVAL_TICKS) {
      popSamples.push({ tick, population: args.pop });
      if (popSamples.length > POP_SAMPLE_CAP) popSamples.shift();
      lastPopSampledTick = tick;
      if (panelVisible) redrawPopChart();
    }
  }
  if (panelVisible) redrawFpsTpsChart();
}

/** Drop all recorded samples so charts start fresh after a world restart. */
export function resetPanelSamples(): void {
  fpsTpsSamples.length = 0;
  popSamples.length = 0;
  lastPopSampledTick = -1;
  paintedFrameTimestamps.length = 0;
  if (panelVisible) {
    redrawFpsTpsChart();
    redrawPopChart();
  }
}

// ─── Installer ────────────────────────────────────────────────────────────

export function installProfilerPanel(simBridge: SimBridge): void {
  const box = document.getElementById("perf-box") as HTMLDivElement | null;
  if (!box) return;

  buildPanelDom(box, simBridge);

  // Apply persisted profiler-window length so the first poll renders against
  // the user's chosen window. Pushed to both halves: Rust profiler (via
  // bridge → control SAB) and the TS-side `frame` tree.
  const initialWindow = getSettings().profilerWindowMs;
  simBridge.setProfileWindowMs(initialWindow);
  setProfilerWindowMs(initialWindow);

  // Initial visibility from persisted setting.
  setProfilerVisible(getSettings().showProfiler);

  const poll = async (): Promise<void> => {
    if (!panelVisible) return;
    const raw = await simBridge.requestProfileReport();
    if (!raw) return;
    try {
      lastBundle = JSON.parse(raw) as BundledProfile;
    } catch (e) {
      console.warn("profiler: failed to parse bundled report", e);
      return;
    }
    // v1.12: publish the Rust profile tree for the NN tab's per-layer perf
    // log (cheap pointer share — same parsed object, no re-poll). Shape
    // matches the consumer in web/src/rail/nn-tab.ts.
    (window as unknown as { __lastProfilerReport?: unknown }).__lastProfilerReport =
      lastBundle.profile;
    if (isProfilerEnabled()) {
      pollAndRenderTrees(lastBundle);
    }
  };

  void poll();
  window.setInterval(() => void poll(), POLL_INTERVAL_MS);

  // Single toggle: clicking #perf-open flips the persisted visibility flag
  // and re-applies.
  const openBtn = document.getElementById("perf-open");
  if (openBtn) {
    openBtn.addEventListener("click", () => {
      const next = !getSettings().showProfiler;
      setSetting("showProfiler", next);
      setProfilerVisible(next);
    });
  }
}

// ─── DOM build ────────────────────────────────────────────────────────────

function buildPanelDom(box: HTMLDivElement, simBridge: SimBridge): void {
  box.innerHTML = "";

  // 1. Status line.
  statusLine = document.createElement("div");
  statusLine.id = "perf-status-line";
  statusLine.className = "perf-status-line";
  statusLine.textContent = "Booting…";
  box.appendChild(statusLine);

  // Responsive band wraps graphs + selectors. Container queries on this
  // element drive small / medium / large breakpoints.
  const band = document.createElement("div");
  band.className = "perf-band";
  box.appendChild(band);

  // 2. Graphs row.
  const graphsRow = document.createElement("div");
  graphsRow.className = "perf-graphs";
  band.appendChild(graphsRow);

  const fpsTpsCard = document.createElement("div");
  fpsTpsCard.className = "perf-graph-card";
  const fpsTpsTitle = document.createElement("div");
  fpsTpsTitle.className = "perf-graph-title";
  fpsTpsTitle.textContent = "FPS / TPS";
  fpsTpsCard.appendChild(fpsTpsTitle);
  fpsTpsCanvas = document.createElement("canvas");
  fpsTpsCanvas.className = "perf-chart";
  fpsTpsCanvas.id = "chart-fps-tps";
  fpsTpsCanvas.width = 380;
  fpsTpsCanvas.height = 120;
  fpsTpsCard.appendChild(fpsTpsCanvas);
  const fpsTpsLegend = document.createElement("div");
  fpsTpsLegend.className = "perf-graph-legend";
  fpsTpsLegend.innerHTML =
    `<span class="perf-legend-item"><span class="perf-legend-swatch perf-legend-fps"></span>FPS</span>` +
    `<span class="perf-legend-item"><span class="perf-legend-swatch perf-legend-tps"></span>TPS</span>`;
  fpsTpsCard.appendChild(fpsTpsLegend);
  graphsRow.appendChild(fpsTpsCard);

  const popCard = document.createElement("div");
  popCard.className = "perf-graph-card";
  const popTitle = document.createElement("div");
  popTitle.className = "perf-graph-title";
  popTitle.textContent = "Population";
  popCard.appendChild(popTitle);
  popCanvas = document.createElement("canvas");
  popCanvas.className = "perf-chart";
  popCanvas.id = "chart-pop";
  popCanvas.width = 380;
  popCanvas.height = 120;
  popCard.appendChild(popCanvas);
  graphsRow.appendChild(popCard);

  // 3. Selectors row.
  const selectorsRow = document.createElement("div");
  selectorsRow.className = "perf-selectors";
  band.appendChild(selectorsRow);
  selectorsRow.appendChild(buildTpsSelector(simBridge));
  selectorsRow.appendChild(buildMaxPopSelector(simBridge));

  // 4. CPU Process Monitor.
  const cpuSec = document.createElement("div");
  cpuSec.className = "perf-section perf-cpu-section";
  const cpuTitle = document.createElement("div");
  cpuTitle.className = "perf-section-title";
  cpuTitle.textContent = "CPU Process Monitor";
  cpuSec.appendChild(cpuTitle);
  const cpuHost = document.createElement("div");
  cpuHost.id = "worker-stats-host";
  cpuSec.appendChild(cpuHost);
  box.appendChild(cpuSec);
  if (workerStatsTeardown) workerStatsTeardown();
  workerStatsTeardown = installWorkerStatsPanel(simBridge, cpuHost);

  // 5. Profile system.
  const profileSec = document.createElement("div");
  profileSec.className = "perf-section perf-profile-section";
  const profileHeader = document.createElement("div");
  profileHeader.className = "perf-section-title perf-profile-header";

  const profileTitle = document.createElement("span");
  profileTitle.textContent = "Profile";
  profileHeader.appendChild(profileTitle);

  // Window selector.
  const windowLabel = document.createElement("label");
  windowLabel.className = "perf-window-label";
  windowLabel.setAttribute("title", "Profiler rolling-window length");
  windowLabel.appendChild(document.createTextNode("window "));
  const windowSelect = document.createElement("select");
  windowSelect.id = "perf-window-select";
  windowSelect.className = "perf-window-select";
  for (const [val, lbl] of [
    [5000, "5s"],
    [10000, "10s"],
    [30000, "30s"],
    [60000, "60s"],
  ] as const) {
    const opt = document.createElement("option");
    opt.value = String(val);
    opt.textContent = lbl;
    windowSelect.appendChild(opt);
  }
  windowSelect.value = String(getSettings().profilerWindowMs);
  windowSelect.addEventListener("change", () => {
    const ms = Number(windowSelect.value) || 10_000;
    setSetting("profilerWindowMs", ms);
    simBridge.setProfileWindowMs(ms);
    setProfilerWindowMs(ms);
  });
  windowLabel.appendChild(windowSelect);
  profileHeader.appendChild(windowLabel);

  // Jank/profile reset.
  const resetBtn = document.createElement("button");
  resetBtn.id = "jank-reset";
  resetBtn.className = "jank-reset-btn";
  resetBtn.type = "button";
  resetBtn.title = "Reset profiler + jank";
  resetBtn.setAttribute("aria-label", "Reset profiler + jank");
  resetBtn.innerHTML =
    `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" ` +
    `stroke="currentColor" stroke-width="1.8" ` +
    `stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">` +
    `<path d="M3 12a9 9 0 1 0 3-6.7"></path>` +
    `<path d="M3 4v5h5"></path>` +
    `</svg>`;
  resetBtn.addEventListener("click", () => {
    simBridge.resetJank();
    simBridge.resetProfile();
    resetFrameTree();
    if (lastBundle) lastBundle.jank_count = 0;
    clearTrees();
  });
  profileHeader.appendChild(resetBtn);

  profileSec.appendChild(profileHeader);

  const stabilizing = document.createElement("div");
  stabilizing.id = "profiler-stabilizing";
  stabilizing.className = "profiler-stabilizing";
  stabilizing.style.display = "none";
  profileSec.appendChild(stabilizing);

  const trees = document.createElement("div");
  trees.id = "profiler-trees";
  profileSec.appendChild(trees);

  box.appendChild(profileSec);

  clearTrees();

  // Resize observers for the two charts.
  if (fpsTpsResizeObserver) fpsTpsResizeObserver.disconnect();
  if (popResizeObserver) popResizeObserver.disconnect();
  fpsTpsResizeObserver = new ResizeObserver(() => {
    resizeChartCanvas(fpsTpsCanvas);
    redrawFpsTpsChart();
  });
  popResizeObserver = new ResizeObserver(() => {
    resizeChartCanvas(popCanvas);
    redrawPopChart();
  });
  fpsTpsResizeObserver.observe(fpsTpsCanvas);
  popResizeObserver.observe(popCanvas);
  resizeChartCanvas(fpsTpsCanvas);
  resizeChartCanvas(popCanvas);
}

// ─── Selector builders ────────────────────────────────────────────────────

function buildTpsSelector(simBridge: SimBridge): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "perf-selector-row";
  const lbl = document.createElement("span");
  lbl.className = "perf-selector-label";
  lbl.textContent = "TPS";
  row.appendChild(lbl);

  const pillGroup = document.createElement("div");
  pillGroup.className = "perf-pill-group";
  tpsPillButtons = [];
  for (const v of TPS_PILLS) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "perf-pill";
    b.textContent = String(v);
    b.dataset.tpsValue = String(v);
    b.addEventListener("click", () => applyTpsValue(v, simBridge, "pill"));
    pillGroup.appendChild(b);
    tpsPillButtons.push(b);
  }
  row.appendChild(pillGroup);

  tpsNumInput = document.createElement("input");
  tpsNumInput.type = "number";
  tpsNumInput.min = "1";
  tpsNumInput.step = "1";
  tpsNumInput.className = "perf-num-input";
  // Stable e2e hook. v1.13 moved the TPS control from a top-bar <select> to this
  // perf-panel numeric input; the suite drives it by id + a "change" event.
  tpsNumInput.id = "target-tps-input";
  tpsNumInput.value = String(getTargetTPS());
  tpsNumInput.addEventListener("change", () => {
    const v = Number(tpsNumInput!.value);
    if (Number.isFinite(v) && v >= 1) applyTpsValue(Math.round(v), simBridge, "input");
  });
  row.appendChild(tpsNumInput);

  // External TPS changes (e.g. URL boot, future keyboard shortcuts) should
  // refresh the widget. main.ts wires this through setTpsChangeListener.
  setTpsChangeListener((v) => syncTpsWidget(v));
  syncTpsWidget(getTargetTPS());
  return row;
}

function applyTpsValue(v: number, simBridge: SimBridge, _source: "pill" | "input"): void {
  if (!Number.isFinite(v) || v < 1) return;
  setExternalTargetTPS(v);
  setSetting("targetTPS", v);
  simBridge.setTargetTps(v);
  syncTpsWidget(v);
}

function syncTpsWidget(v: number): void {
  if (tpsNumInput) tpsNumInput.value = String(v);
  for (const b of tpsPillButtons) {
    const isMatch = Number(b.dataset.tpsValue) === v;
    b.classList.toggle("is-active", isMatch);
  }
}

function buildMaxPopSelector(simBridge: SimBridge): HTMLDivElement {
  const row = document.createElement("div");
  row.className = "perf-selector-row";
  const lbl = document.createElement("span");
  lbl.className = "perf-selector-label";
  lbl.textContent = "Max pop";
  row.appendChild(lbl);

  const pillGroup = document.createElement("div");
  pillGroup.className = "perf-pill-group";
  maxPopPillButtons = [];
  for (const v of MAX_POP_PILLS) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "perf-pill";
    b.textContent = String(v);
    b.dataset.maxPopValue = String(v);
    b.addEventListener("click", () => applyMaxPopValue(v, simBridge));
    pillGroup.appendChild(b);
    maxPopPillButtons.push(b);
  }
  row.appendChild(pillGroup);

  maxPopNumInput = document.createElement("input");
  maxPopNumInput.type = "number";
  maxPopNumInput.min = "1";
  maxPopNumInput.step = "100";
  maxPopNumInput.className = "perf-num-input";
  maxPopNumInput.value = String(getSettings().maxPopulation);
  maxPopNumInput.addEventListener("change", () => {
    const v = Number(maxPopNumInput!.value);
    if (Number.isFinite(v) && v >= 1) applyMaxPopValue(Math.round(v), simBridge);
  });
  row.appendChild(maxPopNumInput);

  syncMaxPopWidget(getSettings().maxPopulation);
  return row;
}

function applyMaxPopValue(v: number, simBridge: SimBridge): void {
  if (!Number.isFinite(v) || v < 1) return;
  setSetting("maxPopulation", v);
  simBridge.debouncedSetSlider("max_population", v);
  syncMaxPopWidget(v);
}

function syncMaxPopWidget(v: number): void {
  if (maxPopNumInput) maxPopNumInput.value = String(v);
  for (const b of maxPopPillButtons) {
    const isMatch = Number(b.dataset.maxPopValue) === v;
    b.classList.toggle("is-active", isMatch);
  }
}

// ─── Chart canvas helpers ────────────────────────────────────────────────

function resizeChartCanvas(canvas: HTMLCanvasElement | null): void {
  if (!canvas) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const cssW = canvas.clientWidth;
  if (cssW <= 0) return;
  canvas.style.height = `${CHART_HEIGHT_CSS}px`;
  canvas.width = Math.floor(cssW * dpr);
  canvas.height = Math.floor(CHART_HEIGHT_CSS * dpr);
}

function readCssVar(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function redrawFpsTpsChart(): void {
  const canvas = fpsTpsCanvas;
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const Wcss = canvas.width / dpr;
  const Hcss = canvas.height / dpr;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.scale(dpr, dpr);

  const gridColor = readCssVar("--chart-grid", "rgba(255,255,255,0.10)");
  const fgFaint = readCssVar("--fg-faint", "rgba(255,255,255,0.5)");
  const fpsColor = readCssVar("--chart-line", "#67b3a9");
  const tpsColor = readCssVar("--accent-2", "#b07ad4");

  if (fpsTpsSamples.length < 2) {
    ctx.fillStyle = fgFaint;
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText("fps/tps: waiting…", 4, 14);
    return;
  }

  // Shared y-axis (max across both series, hard floor at 1).
  let ymax = 1;
  for (const s of fpsTpsSamples) {
    if (s.fps > ymax) ymax = s.fps;
    if (s.tps > ymax) ymax = s.tps;
  }

  const padLeft = 4;
  const padRight = 4;
  const padTop = 16;
  const padBottom = 14;
  const plotW = Math.max(1, Wcss - padLeft - padRight);
  const plotH = Math.max(1, Hcss - padTop - padBottom);
  const n = fpsTpsSamples.length;
  const xStep = plotW / Math.max(1, n - 1);

  // Subtle gridline at midline.
  ctx.strokeStyle = gridColor;
  ctx.lineWidth = 1;
  ctx.beginPath();
  const midY = padTop + plotH * 0.5;
  ctx.moveTo(padLeft, midY);
  ctx.lineTo(Wcss - padRight, midY);
  ctx.stroke();

  // TPS line (drawn first so FPS sits on top).
  drawSeries(ctx, fpsTpsSamples.map((s) => s.tps), tpsColor, padLeft, padTop, xStep, plotH, ymax);
  // FPS line.
  drawSeries(ctx, fpsTpsSamples.map((s) => s.fps), fpsColor, padLeft, padTop, xStep, plotH, ymax);

  // Labels: max top-left, current values top-right.
  const last = fpsTpsSamples[n - 1];
  ctx.fillStyle = fgFaint;
  ctx.font = "10px ui-monospace, monospace";
  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.fillText(`max: ${ymax.toFixed(0)}`, padLeft, 2);
  ctx.textAlign = "right";
  ctx.fillText(`fps ${last.fps.toFixed(0)} · tps ${last.tps.toFixed(0)}`, Wcss - padRight, 2);
}

function drawSeries(
  ctx: CanvasRenderingContext2D,
  vals: number[],
  color: string,
  padLeft: number,
  padTop: number,
  xStep: number,
  plotH: number,
  ymax: number,
): void {
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < vals.length; i++) {
    const sx = padLeft + i * xStep;
    const sy = padTop + (1 - vals[i] / ymax) * plotH;
    if (i === 0) ctx.moveTo(sx, sy);
    else ctx.lineTo(sx, sy);
  }
  ctx.stroke();
}

function redrawPopChart(): void {
  const canvas = popCanvas;
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const Wcss = canvas.width / dpr;
  const Hcss = canvas.height / dpr;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.scale(dpr, dpr);

  const gridColor = readCssVar("--chart-grid", "rgba(255,255,255,0.10)");
  const fgFaint = readCssVar("--fg-faint", "rgba(255,255,255,0.5)");
  const lineColor = readCssVar("--chart-line", "#67b3a9");

  if (popSamples.length < 2) {
    ctx.fillStyle = fgFaint;
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText("pop: waiting…", 4, 14);
    return;
  }

  const ymax = Math.max(1, ...popSamples.map((s) => s.population));
  const xmin = popSamples[0].tick;
  const xmax = popSamples[popSamples.length - 1].tick;
  const xrange = Math.max(1, xmax - xmin);

  const padLeft = 4;
  const padRight = 4;
  const padTop = 16;
  const padBottom = 14;
  const plotW = Math.max(1, Wcss - padLeft - padRight);
  const plotH = Math.max(1, Hcss - padTop - padBottom);

  ctx.strokeStyle = gridColor;
  ctx.lineWidth = 1;
  ctx.beginPath();
  const midY = padTop + plotH * 0.5;
  ctx.moveTo(padLeft, midY);
  ctx.lineTo(Wcss - padRight, midY);
  ctx.stroke();

  ctx.strokeStyle = lineColor;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < popSamples.length; i++) {
    const sx = padLeft + ((popSamples[i].tick - xmin) / xrange) * plotW;
    const sy = padTop + (1 - popSamples[i].population / ymax) * plotH;
    if (i === 0) ctx.moveTo(sx, sy);
    else ctx.lineTo(sx, sy);
  }
  ctx.stroke();

  const last = popSamples[popSamples.length - 1];
  ctx.fillStyle = fgFaint;
  ctx.font = "10px ui-monospace, monospace";
  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.fillText(`max: ${ymax.toFixed(0)}`, padLeft, 2);
  ctx.textAlign = "right";
  ctx.fillText(`now: ${last.population.toFixed(0)}`, Wcss - padRight, 2);
  ctx.textBaseline = "bottom";
  ctx.textAlign = "left";
  ctx.fillText(`t=${xmin}`, padLeft, Hcss - 2);
  ctx.textAlign = "right";
  ctx.fillText(`t=${xmax}`, Wcss - padRight, Hcss - 2);
}

// ─── Profile tree render (kept ~verbatim from v1.12) ─────────────────────

function clearTrees(): void {
  const root = document.getElementById("profiler-trees");
  if (root) {
    root.innerHTML =
      `<div class="pf-empty-hint" style="text-align:center;opacity:0.45;font-style:italic;padding:8px 0">Enable to record</div>`;
  }
  const banner = document.getElementById("profiler-stabilizing");
  if (banner) banner.style.display = "none";
}

function pollAndRenderTrees(bundle: BundledProfile): void {
  if (!isProfilerEnabled()) return;

  const rustReport = bundle.profile;
  let tsReport: ProfNode | null = null;
  try {
    tsReport = JSON.parse(tsPerfReport()) as ProfNode;
  } catch (e) {
    console.warn("profiler: failed to parse TS report", e);
  }

  const treesByName = new Map<string, ProfNode>();
  for (const r of rustReport.tree) {
    treesByName.set(r.name, r);
  }
  if (tsReport) {
    treesByName.set("frame", tsReport);
  }

  const ordered: ProfNode[] = [];
  const seen = new Set<string>();
  for (const name of TREE_ORDER) {
    const t = treesByName.get(name);
    if (t) {
      ordered.push(t);
      seen.add(name);
    }
  }
  for (const r of rustReport.tree) {
    if (!seen.has(r.name)) {
      ordered.push(r);
      seen.add(r.name);
    }
  }

  const windowMs = rustReport.window_ms;
  const threshold = windowMs * 0.9;
  let minEffective = Infinity;
  function checkStabilizing(nodes: ProfNode[]): void {
    for (const n of nodes) {
      if (n.call_count > 0) {
        minEffective = Math.min(minEffective, n.effective_window_ms);
      }
      checkStabilizing(n.children);
    }
  }
  for (const t of ordered) checkStabilizing([t]);

  const banner = document.getElementById("profiler-stabilizing");
  if (banner) {
    if (minEffective < threshold && minEffective !== Infinity) {
      const secs = (minEffective / 1000).toFixed(0);
      banner.textContent = `stabilizing… (${secs}s of data)`;
      banner.style.display = "block";
    } else {
      banner.style.display = "none";
    }
  }

  renderTreesStacked(ordered);
}

function renderTreesStacked(trees: ProfNode[]): void {
  const root = document.getElementById("profiler-trees");
  if (!root) return;

  if (trees.length === 0) {
    root.innerHTML =
      `<div class="pf-empty-hint" style="text-align:center;opacity:0.45;font-style:italic;padding:8px 0">Enable to record</div>`;
    return;
  }

  let maxWindowMs = 0;
  function scanWindow(node: ProfNode): void {
    if (node.call_count > 0 && node.effective_window_ms > maxWindowMs) {
      maxWindowMs = node.effective_window_ms;
    }
    for (const c of node.children) scanWindow(c);
  }
  for (const t of trees) scanWindow(t);
  const windowSec = maxWindowMs / 1000;
  const windowHeader =
    `<div class="profiler-window" style="opacity:0.7;font-size:11px;padding:2px 4px 6px 4px">` +
      `window: ${windowSec.toFixed(1)} s` +
    `</div>`;

  const parts: string[] = [windowHeader];
  for (const tree of trees) {
    parts.push(renderOneTree(tree));
  }
  root.innerHTML = parts.join("");
}

function renderOneTree(treeRoot: ProfNode): string {
  const rootTotalUs = treeRoot.total_us;
  const hasSamples = treeRoot.call_count > 0 || treeRoot.children.some((c) => c.call_count > 0);

  if (!hasSamples) {
    return (
      `<div class="profiler-tree-section">` +
        `<div class="profiler-tree-header">${escHtml(treeRoot.name)}</div>` +
        `<div class="pf-empty-hint" style="text-align:center;opacity:0.45;font-style:italic;padding:4px 0">(no samples yet)</div>` +
      `</div>`
    );
  }

  const rows: string[] = [];

  function renderNode(node: ProfNode, depth: number, pathPrefix: string): void {
    const fullPath =
      pathPrefix === ""
        ? node.name
        : node.name.startsWith(pathPrefix + ".") || node.name === pathPrefix
          ? node.name
          : `${pathPrefix}.${node.name}`;
    const totalMs = node.total_us / 1000;
    const honestCalls = node.total_call_count ?? node.call_count;
    const msPerCall = honestCalls > 0 ? node.total_us / honestCalls / 1000 : null;
    const sharePct =
      depth === 0
        ? 100
        : rootTotalUs > 0
          ? (node.total_us / rootTotalUs) * 100
          : null;

    const fmt = (v: number | null, decimals: number): string =>
      v !== null && isFinite(v) ? v.toFixed(decimals) : "-";

    rows.push(
      `<tr class="pf-depth-${Math.min(depth, 4)}">` +
        `<td class="pf-name">${escHtml(fullPath)}</td>` +
        `<td>${fmt(totalMs, 2)}</td>` +
        `<td>${honestCalls > 0 ? honestCalls : "-"}</td>` +
        `<td>${fmt(msPerCall, 3)}</td>` +
        `<td>${fmt(sharePct, 1)}</td>` +
        `</tr>`,
    );

    for (const child of node.children) {
      renderNode(child, depth + 1, fullPath);
    }
  }

  renderNode(treeRoot, 0, "");

  return (
    `<div class="profiler-tree-section">` +
      `<div class="profiler-tree-header">${escHtml(treeRoot.name)}</div>` +
      `<table class="profiler-table">` +
        `<thead><tr>` +
          `<th>path</th><th>total ms</th><th>calls</th>` +
          `<th>ms/call</th><th>share %</th>` +
        `</tr></thead>` +
        `<tbody>${rows.join("")}</tbody>` +
      `</table>` +
    `</div>`
  );
}

function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
