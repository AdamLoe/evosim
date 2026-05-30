// Per-worker stats panel for the parallel NN forward pass.
//
// v1.6 Wave B: polls via `SimBridge.requestNnStats()` (request/reply through
// the worker boundary) every POLL_MS and renders a small dashboard:
//
//   * Pool ceiling vs workers ever-seen + workers-used this tick.
//   * Per-worker table (chunks, creatures, uptime, busy %).
//   * Per-tick sub-phase breakdown (only populated when the profiler is on).
//
// Cost: 1 round-trip + JSON parse per POLL_MS + DOM updates. Negligible.

import type { SimBridge } from "../sim-bridge";

const POLL_MS = 750;

interface NnStatsJson {
  world_uptime_us: number;
  tick: {
    build_input_other_us: number;
    proximity_creatures_us: number;
    proximity_grass_us: number;
    forward_us: number;
    chunk_wall_us: number;
    workers_used: number;
  };
  workers: Array<{
    idx: number;
    first_seen_us: number;
    last_seen_us: number;
    uptime_us: number;
    chunks: number;
    creatures: number;
    busy_us: number;
    creatures_per_chunk: number;
  }>;
}

function fmtUs(us: number): string {
  if (us < 1000) return `${us}µs`;
  if (us < 1_000_000) return `${(us / 1000).toFixed(1)}ms`;
  return `${(us / 1_000_000).toFixed(2)}s`;
}

function fmtCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

/** Build the threads-stats panel and append it to `container`. Returns a
 *  teardown fn that stops the poller. */
export function installWorkerStatsPanel(
  simBridge: SimBridge,
  container: HTMLElement,
): () => void {
  // Summary line (pool / seen / used).
  const summary = document.createElement("div");
  summary.className = "worker-stats-summary";
  container.appendChild(summary);

  // Per-tick sub-phase line (only meaningful with profiler on).
  const subphase = document.createElement("div");
  subphase.className = "worker-stats-subphase";
  container.appendChild(subphase);

  // Profile toggle so the user can turn the heavy timing on/off from here.
  const profileRow = document.createElement("label");
  profileRow.className = "worker-stats-profile-toggle";
  const profileBox = document.createElement("input");
  profileBox.type = "checkbox";
  profileBox.addEventListener("change", () => {
    // v1.10: profiler is always-on Rust-side; the visibility toggle lives in
    // perf-panel. This checkbox is a holdover and only changes panel reveal.
  });
  profileRow.append(profileBox, document.createTextNode(" Sub-phase timing"));
  container.appendChild(profileRow);

  // Workers table.
  const table = document.createElement("table");
  table.className = "worker-stats-table";
  const head = document.createElement("thead");
  head.innerHTML =
    "<tr><th>w</th><th>chunks</th><th>creat.</th><th>busy</th><th>uptime</th><th>%busy</th></tr>";
  const body = document.createElement("tbody");
  table.append(head, body);
  container.appendChild(table);

  // Pool ceiling line at the bottom — static, won't change after boot.
  const poolLine = document.createElement("div");
  poolLine.className = "worker-stats-pool";
  poolLine.textContent = `Pool ceiling: ${navigator.hardwareConcurrency} (navigator.hardwareConcurrency)`;
  container.appendChild(poolLine);

  let intervalId: number | null = null;
  const tick = async (): Promise<void> => {
    let raw: string | null;
    try {
      raw = await simBridge.requestNnStats();
    } catch {
      return;
    }
    if (!raw) return;
    let stats: NnStatsJson;
    try {
      stats = JSON.parse(raw) as NnStatsJson;
    } catch {
      return;
    }
    const seen = stats.workers.length;
    summary.textContent = `seen ${seen} · used this tick ${stats.tick.workers_used} · chunk wall ${fmtUs(stats.tick.chunk_wall_us)}`;

    if (
      stats.tick.forward_us > 0 ||
      stats.tick.build_input_other_us > 0 ||
      stats.tick.proximity_creatures_us > 0 ||
      stats.tick.proximity_grass_us > 0
    ) {
      subphase.textContent =
        `fwd ${fmtUs(stats.tick.forward_us)} · ` +
        `prox.cre ${fmtUs(stats.tick.proximity_creatures_us)} · ` +
        `prox.grass ${fmtUs(stats.tick.proximity_grass_us)} · ` +
        `build ${fmtUs(stats.tick.build_input_other_us)}`;
    } else {
      subphase.textContent = "sub-phase timing off (enable above)";
    }

    // Reuse existing rows for stability under DevTools highlight; only resize
    // the tbody when the worker count changes.
    while (body.rows.length < stats.workers.length) body.insertRow();
    while (body.rows.length > stats.workers.length) body.deleteRow(-1);
    stats.workers.forEach((w, i) => {
      const row = body.rows[i];
      const busyPct = w.uptime_us > 0 ? (w.busy_us / w.uptime_us) * 100 : 0;
      const cells = [
        String(w.idx),
        fmtCount(w.chunks),
        fmtCount(w.creatures),
        fmtUs(w.busy_us),
        fmtUs(w.uptime_us),
        `${busyPct.toFixed(1)}%`,
      ];
      while (row.cells.length < cells.length) row.insertCell();
      while (row.cells.length > cells.length) row.deleteCell(-1);
      cells.forEach((c, j) => (row.cells[j].textContent = c));
    });
  };

  // Run once immediately so the panel isn't blank on open, then poll.
  void tick();
  intervalId = window.setInterval(() => void tick(), POLL_MS);

  return () => {
    if (intervalId !== null) {
      window.clearInterval(intervalId);
      intervalId = null;
    }
  };
}
