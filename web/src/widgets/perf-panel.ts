// Profiler bottom panel for the left column.
//
// Visibility is owned by the `showProfiler` setting (live-apply via the
// Settings tab's `display` group). When visible, the panel polls a bundled
// report at 1 Hz and renders four stacked tables (frame, tick, nn,
// grass_step). The panel's own ✕ button flips the same setting so the
// checkbox and the close button stay in sync.
//
// Rust-side profile recording is gated by visibility too — turning the panel
// off stops `profile_enable` so the sim doesn't pay the timer cost.

import type { SimBridge } from "../sim-bridge";
import {
  isProfilerEnabled,
  reportJson as tsPerfReport,
  resetFrameTree,
  setProfilerEnabled,
} from "../perf";
import { getSettings, setSetting } from "../settings";

const POLL_INTERVAL_MS = 1000;

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

const TREE_ORDER = ["frame", "tick", "nn", "grass_step"];

let lastBundle: BundledProfile | null = null;
let panelVisible = false;

/** Set whether the profiler panel is visible. Single source of truth for both
 *  the Settings checkbox and the panel's own ✕ button. v1.9.1: visibility-only
 *  — the Rust profiler is always-on (set by the worker at boot), the TS-side
 *  frame tree stays enabled too so samples accumulate even when the panel is
 *  hidden. Toggling visibility just shows/hides the DOM and skips the 1 Hz
 *  poll's tree render. */
export function setProfilerVisible(visible: boolean): void {
  panelVisible = visible;
  const box = document.getElementById("perf-box");
  if (box) box.style.display = visible ? "" : "none";
  // Keep the TS-side frame tree recording at all times so the panel has data
  // to show the moment it becomes visible again. Mirrors the always-on Rust
  // side (see sim-worker.ts handleBoot).
  if (!isProfilerEnabled()) setProfilerEnabled(true);
  if (!visible) {
    clearTrees();
  } else if (lastBundle) {
    pollAndRenderTrees(lastBundle);
  }
}

export function installProfilerPanel(simBridge: SimBridge): void {
  // Initial visibility from persisted setting.
  setProfilerVisible(getSettings().showProfiler);

  clearTrees();

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
    renderTpsJank(lastBundle);
    renderObsCounters(lastBundle);
    if (isProfilerEnabled()) {
      pollAndRenderTrees(lastBundle);
    }
  };

  void poll();
  window.setInterval(() => void poll(), POLL_INTERVAL_MS);

  const closeBtn = document.getElementById("perf-close");
  if (closeBtn) {
    closeBtn.addEventListener("click", () => {
      setSetting("showProfiler", false);
      setProfilerVisible(false);
    });
  }

  const jankReset = document.getElementById("jank-reset") as HTMLButtonElement | null;
  if (jankReset) {
    // v1.9.1: the reset button is no longer "jank-only" — it also wipes the
    // accumulated profile data on both sides so a single click gives the user
    // a fully clean slate. Title updated to reflect the broader scope.
    jankReset.title = "Reset profiler + jank";
    jankReset.addEventListener("click", () => {
      simBridge.postMessage({ kind: "reset_jank" });
      simBridge.postMessage({ kind: "reset_profile" });
      resetFrameTree();
      if (lastBundle) {
        lastBundle.jank_count = 0;
        renderTpsJank(lastBundle);
      }
      clearTrees();
    });
  }
}

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
