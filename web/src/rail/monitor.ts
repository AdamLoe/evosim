// Monitor tab installer. Wires up the worker-stats panel beneath the
// pre-rendered population graph. The pop graph itself is driven by
// `maybeSampleStats` in rail/stats.ts, which paints to `#chart-pop` on every
// rail poll — no per-tab init needed for it.

import type { SimBridge } from "../sim-bridge";
import { installWorkerStatsPanel } from "../widgets/worker-stats";
import { installPopChart } from "./stats";

export function installMonitorTab(simBridge: SimBridge): void {
  const host = document.getElementById("worker-stats-host");
  if (!host) return;
  installWorkerStatsPanel(simBridge, host);
  // v1.9.1: hook up the DPR-aware ResizeObserver for the population chart.
  installPopChart();
}
