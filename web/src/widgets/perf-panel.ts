// ui-perf: free-standing Perf-timing widget (bottom-left, always visible).
//
// v1.6 Wave B: polls the sim worker for the bundled profile report
// (profile + tps + jank + grass counters) instead of calling wasm directly.
// 1 Hz cadence preserved.
//
// v1.7 profiler overhaul: renders four stacked tables (frame, tick, nn,
// grass_step) as independent top-level trees instead of one merged table.
// Display order is fixed (tree by definition, rows within a tree by Rust/TS
// insertion order). No sort, no rollup — every row's `total ms` is its own
// real measurement.

import type { SimBridge } from "../sim-bridge";
import { setProfilerEnabled, isProfilerEnabled, reportJson as tsPerfReport } from "../perf";

// ─── Profiler panel ───────────────────────────────────────────────────────────
// Perf-timing plan: toggle checkbox + 1Hz polling + table rendering.

const POLL_INTERVAL_MS = 1000;

/** JSON node shape from profile_report_json() / reportJson() */
interface ProfNode {
  name: string;
  total_us: number;
  /** Sample count: number of distinct ring-buffer entries for this node. */
  call_count: number;
  /**
   * v1.7.2: sum of per-sample call counts. For RAII spans this equals
   * `call_count`; for sum-of-workers drains it's the real per-invocation
   * count (e.g. creature_count × ticks for `nn.forward.l1`). The panel
   * divides by this to get honest `ms/call`.
   */
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

/** Bundled reply from the sim worker's `request_profile_report` handler. */
interface BundledProfile {
  profile: ProfReport;
  tps: number;
  jank_count: number;
  live_grass_cell_count: number;
  total_grass_density: number;
}

// v1.7: order in which top-level trees render in the panel. Trees not in this
// list still render at the bottom in JSON-insertion order so a future Rust
// addition (e.g. `gpu`) doesn't drop on the floor — they just don't get a
// pinned slot.
const TREE_ORDER = ["frame", "tick", "nn", "grass_step"];

// Latest bundle for tps / jank / grass-counter rendering between polls.
let lastBundle: BundledProfile | null = null;

/** Install the profiler checkbox + table. Call once after SimBridge is ready. */
export function installProfilerPanel(simBridge: SimBridge): void {
  const checkbox = document.getElementById("profiler-enable") as HTMLInputElement | null;
  if (!checkbox) return;

  // Default OFF — match D9 (no persistence, unchecked on every page load).
  checkbox.checked = false;
  // D5: initial paint of "enable to record" hint so the panel area isn't blank.
  clearTrees();

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
      pollAndRenderTrees(lastBundle);
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
      if (lastBundle) pollAndRenderTrees(lastBundle);
    } else {
      clearTrees();
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

function clearTrees(): void {
  const root = document.getElementById("profiler-trees");
  if (root) {
    root.innerHTML =
      `<div class="pf-empty-hint" style="text-align:center;opacity:0.45;font-style:italic;padding:8px 0">enable to record</div>`;
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

  // Merge tree list: Rust roots from the report + live TS `frame` overrides
  // the Rust-side `frame` placeholder (which is always empty — the TS panel
  // owns the frame tree). Rust may emit a `frame` root with zero samples; we
  // unconditionally swap it for the TS-side one when available.
  const treesByName = new Map<string, ProfNode>();
  for (const r of rustReport.tree) {
    treesByName.set(r.name, r);
  }
  if (tsReport) {
    treesByName.set("frame", tsReport);
  }

  // Stable order: pinned trees first (frame, tick, nn, grass_step), then any
  // others in JSON insertion order.
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
  // (TS-side frame, if not in TREE_ORDER for some reason, was already inserted above.)

  // Stabilizing banner — driven by the smallest effective_window across any
  // tree's populated nodes.
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
      banner.textContent = `stabilizing... (${secs}s of data)`;
      banner.style.display = "block";
    } else {
      banner.style.display = "none";
    }
  }

  renderTreesStacked(ordered);
}

/**
 * v1.7: render each top-level tree as its own table, stacked vertically.
 * Columns: path · total ms · calls · ms/call · share %.
 * - `total ms` is taken DIRECTLY from the node's `total_us`. No rollup. A
 *   parent row that out-measures the sum of its children is showing real
 *   overhead time and that visibility is the diagnostic point.
 * - `share %` is `node.total_us / root.total_us` so siblings inside a tree
 *   are comparable to each other. The root row shows 100%.
 * - `path` indents one level per dot beyond the root name.
 */
function renderTreesStacked(trees: ProfNode[]): void {
  const root = document.getElementById("profiler-trees");
  if (!root) return;

  if (trees.length === 0) {
    root.innerHTML =
      `<div class="pf-empty-hint" style="text-align:center;opacity:0.45;font-style:italic;padding:8px 0">enable to record</div>`;
    return;
  }

  // v1.7.2: render the actual sample window length at the top of the panel
  // so "tick.total_ms" is interpretable without guessing whether the window
  // is 17 s, 40 s, or 60 s. Per-tick aggregation makes the window uniform
  // across nodes, so taking the max effective_window_ms over all populated
  // nodes is a good proxy for "how much data is the panel showing".
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
    // Two profiler subsystems use different conventions for node.name:
    //   - Rust profiler: leaf-only ("forward", "l1", "build_input", etc.)
    //   - TS profiler (`web/src/perf.ts`): caller passes the literal full
    //     dotted path ("frame.render_world.grass") which we then store as the
    //     node name.
    // Normalize: if the node name already contains the prefix, use it as-is
    // (TS shape); otherwise append it (Rust shape).
    const fullPath =
      pathPrefix === ""
        ? node.name
        : node.name.startsWith(pathPrefix + ".") || node.name === pathPrefix
          ? node.name
          : `${pathPrefix}.${node.name}`;
    const totalMs = node.total_us / 1000;
    // v1.7.2: use `total_call_count` (honest per-work-unit count) for the
    // calls column and the ms/call divisor. Falls back to `call_count` when
    // missing (older JSON shape) so a stale wasm doesn't blank the panel.
    const honestCalls = node.total_call_count ?? node.call_count;
    const msPerCall = honestCalls > 0 ? node.total_us / honestCalls / 1000 : null;
    // share % is relative to the top-level tree root so siblings within a
    // tree are directly comparable. depth-0 (the root row) is pinned to 100%.
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
