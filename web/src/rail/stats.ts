// Stats panel: two Canvas2D line charts (population + species count) (E.23)
// + profiler table toggle (perf-timing plan).
// Samples collected per 10 sim-ticks; ring buffer of 500 samples.

import type { WorldHandle } from "../../wasm/evosim";
import { setProfilerEnabled, isProfilerEnabled, reportJson as tsPerfReport } from "../perf";

interface Sample {
  tick: number;
  population: number;
  species: number;
}

const samples: Sample[] = [];
const SAMPLE_CAP = 500;
const SAMPLE_INTERVAL_TICKS = 10;

let lastSampledTick = -1;
let popCanvas: HTMLCanvasElement | null = null;
let spcCanvas: HTMLCanvasElement | null = null;

function getCanvases(): { pop: HTMLCanvasElement; spc: HTMLCanvasElement } | null {
  if (!popCanvas) popCanvas = document.getElementById("chart-pop") as HTMLCanvasElement;
  if (!spcCanvas) spcCanvas = document.getElementById("chart-species") as HTMLCanvasElement;
  if (!popCanvas || !spcCanvas) return null;
  return { pop: popCanvas, spc: spcCanvas };
}

export function maybeSampleStats(world: WorldHandle): void {
  const tick = world.tick;
  if (tick === lastSampledTick) return;
  if (tick % SAMPLE_INTERVAL_TICKS !== 0) return;
  const raw = world.stats_sample() as unknown as Float32Array;
  const t = raw[0];
  const pop = raw[1];
  const sp = raw[2];
  samples.push({ tick: t, population: pop, species: sp });
  if (samples.length > SAMPLE_CAP) samples.shift();
  lastSampledTick = tick;
  drawCharts();
}

function drawChart(
  canvas: HTMLCanvasElement,
  pick: (s: Sample) => number,
  color: string,
  label: string,
): void {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const W = canvas.width;
  const H = canvas.height;
  ctx.clearRect(0, 0, W, H);
  if (samples.length < 2) {
    // Draw empty placeholder text.
    ctx.fillStyle = "rgba(255,255,255,0.3)";
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText(`${label}: waiting…`, 4, 14);
    return;
  }
  const ymax = Math.max(1, ...samples.map(pick));
  const xmin = samples[0].tick;
  const xmax = samples[samples.length - 1].tick;
  const xrange = Math.max(1, xmax - xmin);

  // X-axis line.
  ctx.strokeStyle = "rgba(255,255,255,0.15)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(20, H - 12);
  ctx.lineTo(W, H - 12);
  ctx.stroke();

  // Data line.
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < samples.length; i++) {
    const px = 20 + ((samples[i].tick - xmin) / xrange) * (W - 22);
    const py = H - 12 - (pick(samples[i]) / ymax) * (H - 18);
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();

  // Label + current value.
  const last = samples[samples.length - 1];
  ctx.fillStyle = "rgba(255,255,255,0.7)";
  ctx.font = "10px ui-monospace, monospace";
  ctx.fillText(`${label}: ${pick(last).toFixed(0)}`, 4, 12);
  ctx.fillText(ymax.toFixed(0), 4, H - 14);
}

function drawCharts(): void {
  const c = getCanvases();
  if (!c) return;
  drawChart(c.pop, (s) => s.population, "#7fc4ff", "pop");
  drawChart(c.spc, (s) => s.species, "#ffb96b", "species");
}

// ─── Profiler panel ───────────────────────────────────────────────────────────
// Perf-timing plan: toggle checkbox + 1Hz polling + table rendering.

const POLL_INTERVAL_MS = 1000;
let profilerPollHandle = 0;

/** JSON node shape from profile_report_json() / reportJson() */
interface ProfNode {
  name: string;
  total_us: number;
  call_count: number;
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

/** Install the profiler checkbox + table. Call once after world is ready. */
export function installProfilerPanel(world: WorldHandle): void {
  const checkbox = document.getElementById("profiler-enable") as HTMLInputElement | null;
  if (!checkbox) return;

  // Default OFF — match D9 (no persistence, unchecked on every page load).
  checkbox.checked = false;

  checkbox.addEventListener("change", () => {
    const on = checkbox.checked;
    setProfilerEnabled(on);
    if (on) {
      startPolling(world);
    } else {
      stopPolling();
      clearTable();
    }
  });
}

function startPolling(world: WorldHandle): void {
  if (profilerPollHandle !== 0) return; // already running
  profilerPollHandle = window.setInterval(() => {
    pollAndRender(world);
  }, POLL_INTERVAL_MS);
  // Render once immediately so the table isn't blank for 1s.
  pollAndRender(world);
}

function stopPolling(): void {
  if (profilerPollHandle !== 0) {
    clearInterval(profilerPollHandle);
    profilerPollHandle = 0;
  }
}

function clearTable(): void {
  const tbody = document.getElementById("profiler-tbody");
  if (tbody) tbody.innerHTML = "";
  const banner = document.getElementById("profiler-stabilizing");
  if (banner) banner.style.display = "none";
}

function pollAndRender(world: WorldHandle): void {
  if (!isProfilerEnabled()) return;

  let rustReport: ProfReport | null = null;
  let tsReport: ProfNode | null = null;
  try {
    rustReport = JSON.parse(world.profile_report_json()) as ProfReport;
  } catch (e) {
    console.warn("profiler: failed to parse Rust report", e);
    return;
  }
  try {
    tsReport = JSON.parse(tsPerfReport()) as ProfNode;
  } catch (e) {
    console.warn("profiler: failed to parse TS report", e);
  }

  // Build merged tree: start with Rust tree, append TS frame root if available.
  const tree: ProfNode[] = [...rustReport.tree];
  if (tsReport) {
    // Replace the Rust-side "frame" placeholder (call_count=0) with the live TS report.
    const frameIdx = tree.findIndex((n) => n.name === "frame");
    if (frameIdx >= 0) {
      tree[frameIdx] = tsReport;
    } else {
      tree.push(tsReport);
    }
  }

  // Determine if any node is still stabilizing (effective_window < 90% of window_ms).
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
  checkStabilizing(tree);

  const banner = document.getElementById("profiler-stabilizing");
  if (banner) {
    if (minEffective < threshold && minEffective !== Infinity) {
      const secs = (minEffective / 1000).toFixed(0);
      banner.textContent = `stabilizing... (${secs}s of data)`;
      banner.style.display = "block";
    } else {
      banner.style.display = "none";
    }
  }

  renderProfilerTable(tree);
}

function renderProfilerTable(tree: ProfNode[]): void {
  const tbody = document.getElementById("profiler-tbody");
  if (!tbody) return;

  const rows: string[] = [];

  function renderNode(node: ProfNode, depth: number, parentTotalUs: number | null): void {
    const totalMs = node.total_us / 1000;
    const perCallUs = node.call_count > 0 ? node.total_us / node.call_count : null;
    const callsPerParent =
      node.parent_call_count !== null && node.parent_call_count > 0
        ? node.call_count / node.parent_call_count
        : null;
    const sharePct =
      parentTotalUs !== null && parentTotalUs > 0
        ? (node.total_us / parentTotalUs) * 100
        : (depth === 0 ? 100 : null);

    const fmt = (v: number | null, decimals: number, suffix = ""): string =>
      v !== null && isFinite(v) ? v.toFixed(decimals) + suffix : "-";

    rows.push(
      `<tr class="pf-depth-${Math.min(depth, 4)}">` +
        `<td class="pf-name">${escHtml(node.name)}</td>` +
        `<td>${fmt(totalMs, 2)}</td>` +
        `<td>${node.call_count > 0 ? node.call_count : "-"}</td>` +
        `<td>${fmt(perCallUs, 1)}</td>` +
        `<td>${fmt(callsPerParent, 2)}</td>` +
        `<td>${fmt(sharePct, 1)}</td>` +
        `</tr>`,
    );

    for (const child of node.children) {
      renderNode(child, depth + 1, node.total_us);
    }
  }

  for (const root of tree) {
    renderNode(root, 0, null);
  }

  tbody.innerHTML = rows.join("");
}

function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
