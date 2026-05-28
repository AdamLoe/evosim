// ui-perf: free-standing Perf-timing widget (bottom-left, always visible).
//
// v1.6 Wave B: polls the sim worker for the bundled profile report
// (profile + tps + jank + grass counters) instead of calling wasm directly.
// 1 Hz cadence preserved.

import type { SimBridge } from "../sim-bridge";
import { setProfilerEnabled, isProfilerEnabled, reportJson as tsPerfReport } from "../perf";

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

/** Bundled reply from the sim worker's `request_profile_report` handler. */
interface BundledProfile {
  profile: ProfReport;
  tps: number;
  jank_count: number;
  live_grass_cell_count: number;
  total_grass_density: number;
}

// Latest bundle for tps / jank / grass-counter rendering between polls.
let lastBundle: BundledProfile | null = null;

/** Install the profiler checkbox + table. Call once after SimBridge is ready. */
export function installProfilerPanel(simBridge: SimBridge): void {
  const checkbox = document.getElementById("profiler-enable") as HTMLInputElement | null;
  if (!checkbox) return;

  // Default OFF — match D9 (no persistence, unchecked on every page load).
  checkbox.checked = false;
  // D5: initial paint of "enable to record" hint so tbody isn't visually blank.
  clearTable();

  // Poll the bundled report at 1 Hz — TPS + jank + grass counters live in
  // this reply so we don't need a second poller.
  const poll = async (): Promise<void> => {
    const raw = await simBridge.requestProfileReport();
    if (!raw) return;
    try {
      lastBundle = JSON.parse(raw) as BundledProfile;
    } catch (e) {
      console.warn("profiler: failed to parse bundled report", e);
      return;
    }
    renderTpsJank(lastBundle);
    renderObsCounters(lastBundle);
    if (isProfilerEnabled()) {
      pollAndRenderTree(lastBundle);
    }
  };

  // Run once immediately so the panel isn't blank for 1s.
  void poll();
  window.setInterval(() => void poll(), POLL_INTERVAL_MS);

  checkbox.addEventListener("change", () => {
    const on = checkbox.checked;
    setProfilerEnabled(on);
    simBridge.postMessage({ kind: "profile_enable", on });
    if (on) {
      profilerPollHandle = 1; // sentinel: tree rendering happens inside poll()
      if (lastBundle) pollAndRenderTree(lastBundle);
    } else {
      profilerPollHandle = 0;
      clearTable();
    }
  });

  // Wire up the jank reset button if present.
  const jankReset = document.getElementById("jank-reset") as HTMLButtonElement | null;
  if (jankReset) {
    jankReset.addEventListener("click", () => {
      simBridge.postMessage({ kind: "reset_jank" });
      if (lastBundle) {
        lastBundle.jank_count = 0;
        renderTpsJank(lastBundle);
      }
    });
  }
}

/** Render TPS rolling average and jank counter into their DOM elements. */
function renderTpsJank(bundle: BundledProfile): void {
  const tpsEl = document.getElementById("perf-tps");
  const jankEl = document.getElementById("perf-jank");
  if (tpsEl) {
    const tps = bundle.tps;
    tpsEl.textContent = tps > 0 ? `${tps.toFixed(1)} TPS` : "— TPS";
  }
  if (jankEl) {
    jankEl.textContent = `${bundle.jank_count} jank`;
  }
}

/** Render P3d observability counters into their DOM elements (1 Hz cadence). */
function renderObsCounters(bundle: BundledProfile): void {
  const grassCellsEl = document.getElementById("perf-grass-cells");
  const grassTotalEl = document.getElementById("perf-grass-total");
  if (grassCellsEl) {
    grassCellsEl.textContent = `grass cells: ${bundle.live_grass_cell_count}`;
  }
  if (grassTotalEl) {
    grassTotalEl.textContent = `grass total: ${bundle.total_grass_density.toFixed(1)}`;
  }
}

function clearTable(): void {
  const tbody = document.getElementById("profiler-tbody");
  if (tbody) {
    // D5: show hint instead of leaving tbody literally empty.
    tbody.innerHTML =
      `<tr><td colspan="6" class="pf-empty-hint">enable to record</td></tr>`;
  }
  const banner = document.getElementById("profiler-stabilizing");
  if (banner) banner.style.display = "none";
}

function pollAndRenderTree(bundle: BundledProfile): void {
  if (!isProfilerEnabled()) return;
  // Reference `profilerPollHandle` so the unused-locals lint stays quiet — it
  // is a Stage-1 sentinel for "tree rendering is live".
  void profilerPollHandle;

  const rustReport = bundle.profile;
  let tsReport: ProfNode | null = null;
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

  // D5: empty-state guard — show hint when no samples are recorded.
  const allEmpty =
    tree.length === 0 || tree.every((n) => n.call_count === 0);
  if (allEmpty) {
    tbody.innerHTML =
      `<tr><td colspan="6" class="pf-empty-hint">enable to record</td></tr>`;
    return;
  }

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
