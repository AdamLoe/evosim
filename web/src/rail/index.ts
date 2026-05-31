// Rail orchestrator: three persistent tabs (Inspector / Monitor / Settings).
// Called from main.ts each RAF via `pollRail`.

import type { SnapshotHeader, SimBridge } from "../sim-bridge";
import { refreshInspector, updateLatestSoA } from "./inspector";
import { pruneHighlights, highlights } from "./highlight";

export type RailTab = "inspector" | "nn" | "settings";

export interface RailState {
  switchTab(name: RailTab): void;
  readonly activeTab: RailTab;
}

function installTabs(): RailState {
  // Default to Settings. Rail starts collapsed (Wave 0) so this is just the
  // tab that's visible when the user first opens the rail via the ⚙ icon.
  let activeTab: RailTab = "settings";

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
  _snapshot: SnapshotHeader,
  simBridge: SimBridge,
  creatures: Float32Array,
  pop: number,
): void {
  // v1.13 Wave 2: population sampling moved to widgets/perf-panel.ts
  // (see `setPanelStatus`). The rail just keeps the inspector + highlight
  // bookkeeping current here.
  updateLatestSoA(creatures, pop);
  refreshInspector(simBridge, rail);
  pruneHighlights(performance.now());
}

export { highlights };
