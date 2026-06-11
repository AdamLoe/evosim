// Rail orchestrator: two persistent tabs (Settings / Inspector).
// v2.1 P4: NN is no longer a top-level tab; it lives in the Settings rail's
// "NN" category pane. Inspector stays click-to-open.
// Called from main.ts each RAF via `pollRail`.

import type { SnapshotHeader, SimBridge } from "../sim/bridge";
import { refreshInspector, updateLatestSoA } from "./inspector";
import { pruneHighlights, highlights } from "./highlight";
import { getSettings } from "../settings";

export type RailTab = "inspector" | "settings";

export interface RailState {
  switchTab(name: RailTab): void;
  readonly activeTab: RailTab;
}

function installTabs(setRailOpen: (open: boolean) => void): RailState {
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
    b.addEventListener("click", () => {
      const tab = b.dataset.tab as RailTab;
      // v2.0.5 S6: clicking the currently-active tab while the rail is open
      // collapses the rail (second click on active tab = toggle closed).
      // A click on an inactive tab opens the rail (if closed) and switches
      // to that tab.
      const railCurrentlyOpen = getSettings().railOpen;
      if (railCurrentlyOpen && activeTab === tab) {
        setRailOpen(false);
      } else {
        setRailOpen(true);
        switchTab(tab);
      }
    });
  });

  return {
    get activeTab() { return activeTab; },
    switchTab,
  };
}

export function installRail(setRailOpen: (open: boolean) => void): RailState {
  return installTabs(setRailOpen);
}

export function pollRail(
  rail: RailState,
  _snapshot: SnapshotHeader,
  simBridge: SimBridge,
  creatures: Float32Array,
  pop: number,
  /** v2.1 P1: true when the sim is paused. Forwarded to refreshInspector so
   *  the NN I/O fetch is only issued while paused. */
  isPaused: boolean,
): void {
  // v1.13 Wave 2: population sampling moved to widgets/perf-panel.ts
  // (see `setPanelStatus`). The rail just keeps the inspector + highlight
  // bookkeeping current here.
  updateLatestSoA(creatures, pop);
  refreshInspector(simBridge, rail, isPaused);
  pruneHighlights(performance.now());
}

export { highlights };
