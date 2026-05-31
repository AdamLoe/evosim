// Rail orchestrator: three persistent tabs (Inspector / Monitor / Settings).
// Called from main.ts each RAF via `pollRail`.

import type { SnapshotHeader, SimBridge } from "../sim-bridge";
import { maybeSampleStats } from "./stats";
import { refreshInspector, updateLatestSoA } from "./inspector";
import { pruneHighlights, highlights } from "./highlight";

export type RailTab = "inspector" | "monitor" | "nn" | "settings";

export interface RailState {
  switchTab(name: RailTab): void;
  readonly activeTab: RailTab;
}

function installTabs(): RailState {
  let activeTab: RailTab = "inspector";

  function switchTab(name: RailTab): void {
    activeTab = name;
    document.querySelectorAll(".rail-tab").forEach((btn) => {
      const b = btn as HTMLButtonElement;
      b.classList.toggle("is-active", b.dataset.tab === name);
    });
    document.querySelectorAll(".rail-panel").forEach((panel) => {
      const p = panel as HTMLElement;
      const panelName = p.id.replace("rail-", "");
      p.classList.toggle("is-active", panelName === name);
    });
  }

  document.querySelectorAll(".rail-tab").forEach((btn) => {
    const b = btn as HTMLButtonElement;
    b.addEventListener("click", () => switchTab(b.dataset.tab as RailTab));
  });

  return {
    get activeTab() { return activeTab; },
    switchTab,
  };
}

export function installRail(): RailState {
  return installTabs();
}

export function pollRail(
  rail: RailState,
  snapshot: SnapshotHeader,
  simBridge: SimBridge,
  creatures: Float32Array,
  pop: number,
): void {
  maybeSampleStats(snapshot);
  updateLatestSoA(creatures, pop);
  refreshInspector(simBridge, rail);
  pruneHighlights(performance.now());
}

export { highlights };
