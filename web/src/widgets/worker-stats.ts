// Per-worker stats panel for the parallel NN forward pass.
//
// Polls `world.nn_worker_stats_json()` every POLL_MS and renders a small
// dashboard:
//
//   * Pool ceiling vs workers ever-seen + workers-used this tick.
//   * Per-worker table (chunks, creatures, uptime, busy %).
//   * Per-tick sub-phase breakdown (only populated when the profiler is on).
//
// The "lite" counters (chunks/creatures/busy_us/first_seen) are recorded by
// `nn_stats::record_chunk_lite` and are always live. The sub-phase rows
// (build_input / proximity / forward) only fill when the user toggles the
// profiler — there's a button to flip that.
//
// Cost: 1 JSON parse per POLL_MS + DOM updates. Negligible.

import type { WorldHandle } from "../../wasm/evosim";

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
  getWorld: () => WorldHandle,
  container: HTMLElement,
): () => void {
  // Header.
  const hdr = document.createElement("div");
  hdr.className = "devpanel-section-header";
  hdr.textContent = "nn threads";
  container.appendChild(hdr);

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
    getWorld().profile_enable(profileBox.checked);
  });
  profileRow.append(profileBox, document.createTextNode(" sub-phase timing"));
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
  poolLine.textContent = `pool ceiling: ${navigator.hardwareConcurrency} (navigator.hardwareConcurrency)`;
  container.appendChild(poolLine);

  let intervalId: number | null = null;
  const tick = (): void => {
    let stats: NnStatsJson;
    try {
      stats = JSON.parse(getWorld().nn_worker_stats_json()) as NnStatsJson;
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
  tick();
  intervalId = window.setInterval(tick, POLL_MS);

  return () => {
    if (intervalId !== null) {
      window.clearInterval(intervalId);
      intervalId = null;
    }
  };
}
