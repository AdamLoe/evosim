// Per-worker stats panel for the parallel NN forward pass.
//
// v1.13 Wave 2: collapsed to just the per-worker table — no summary lines,
// no sub-phase row, no profile toggle, no pool ceiling line. The panel
// lives inside the bottom perf panel ("CPU Process Monitor" section) and
// the table renders 1-indexed worker numbers under the "#" column.
//
// Cost: 1 round-trip + JSON parse per POLL_MS + DOM updates. Negligible.

import type { SimBridge } from "../sim-bridge";

const POLL_MS = 750;

interface NnStatsJson {
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

/**
 * Build the per-worker table inside `container` and start polling. Returns a
 * teardown fn that stops the poller. The first column is `#` (1-indexed
 * worker index — Rust reports 0-indexed but we display 1-indexed for
 * human-friendliness).
 */
export function installWorkerStatsPanel(
  simBridge: SimBridge,
  container: HTMLElement,
): () => void {
  // Workers table.
  const table = document.createElement("table");
  table.className = "worker-stats-table";
  const head = document.createElement("thead");
  head.innerHTML =
    "<tr><th>#</th><th>chunks</th><th>creat.</th><th>busy</th><th>uptime</th><th>%busy</th></tr>";
  const body = document.createElement("tbody");
  table.append(head, body);
  container.appendChild(table);

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
    // Reuse existing rows for stability under DevTools highlight; only resize
    // the tbody when the worker count changes.
    while (body.rows.length < stats.workers.length) body.insertRow();
    while (body.rows.length > stats.workers.length) body.deleteRow(-1);
    stats.workers.forEach((w, i) => {
      const row = body.rows[i];
      const busyPct = w.uptime_us > 0 ? (w.busy_us / w.uptime_us) * 100 : 0;
      const cells = [
        // 1-indexed worker number for display (Rust reports 0-indexed).
        String(w.idx + 1),
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
