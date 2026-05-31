// Rail orchestrator: three persistent tabs (Inspector / Monitor / Settings).
// Called from main.ts each RAF via `pollRail`.

import type { SnapshotHeader, SimBridge } from "../sim-bridge";
import {
  refreshInspector,
  subscribeInspectorVisibility,
  updateLatestSoA,
} from "./inspector";
import { pruneHighlights, highlights } from "./highlight";

export type RailTab = "inspector" | "nn" | "settings";

export interface RailState {
  switchTab(name: RailTab): void;
  readonly activeTab: RailTab;
}

function installTabs(): RailState {
  // v1.13: default to "nn" — Inspector starts hidden (Wave 4) so its panel
  // shouldn't be the boot-active tab, and Wave 0 collapsed the rail by
  // default anyway.
  let activeTab: RailTab = "nn";

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

// v1.13 Wave 4: the Inspector tab + panel are only present in the strip
// while a creature is selected. The inspector module is the source of truth
// for selection state; it pushes show/hide events here via the visibility
// subscription set up in installRail().
function setInspectorTabVisible(rail: RailState, visible: boolean): void {
  const tabBtn = document.querySelector<HTMLButtonElement>(
    '.rail-tab[data-tab="inspector"]',
  );
  if (tabBtn) tabBtn.classList.toggle("is-hidden", !visible);
  // Panel visibility is implied: the inactive .rail-panel rule is
  // display:none, and we always switch away from "inspector" below when
  // hiding, so the panel stops painting without an explicit class.
  if (!visible && rail.activeTab === "inspector") {
    rail.switchTab("nn");
  }
}

export function installRail(): RailState {
  const rail = installTabs();
  subscribeInspectorVisibility((selected) =>
    setInspectorTabVisible(rail, selected),
  );
  return rail;
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
