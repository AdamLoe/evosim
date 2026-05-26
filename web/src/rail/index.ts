// Rail orchestrator: boot + pollRail. Called from main.ts each RAF frame.
// E.22: Highlight management (toasts suppressed for v1.1 revisit).
// E.23: Stats sampling.
// E.24: Inspector refresh.

import type { WorldHandle } from "../../wasm/evosim";
import { maybeSampleStats } from "./stats";
import { refreshInspector } from "./inspector";
import { tickToasts } from "./toast";
import { addHighlight, pruneHighlights, highlights } from "./highlight";

// ---- Rail state (opaque to main.ts) ----

export interface RailState {
  switchTab(name: string): void;
  activeTab: string;
}

// ---- Tab switching ----

function installTabs(): RailState {
  let activeTab = "stats"; // Events tab hidden; Stats is default for v1.1 revisit

  function switchTab(name: string): void {
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
    b.addEventListener("click", () => switchTab(b.dataset.tab!));
  });

  return {
    get activeTab() { return activeTab; },
    switchTab,
  };
}

// ---- installRail ----

export function installRail(_world: WorldHandle): RailState {
  return installTabs();
}

// ---- pollRail (called each RAF frame) ----

export function pollRail(
  rail: RailState,
  world: WorldHandle,
): void {
  // 1. Stats sample (E.23).
  maybeSampleStats(world);

  // 2. Inspector refresh (E.24). S18: no longer needs idsBuffer (uses creature_idx_by_id).
  refreshInspector(world, rail);

  // 3. Toast tick + highlight prune.
  tickToasts(performance.now());
  pruneHighlights(performance.now());
}

export { highlights, addHighlight };
