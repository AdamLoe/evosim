// Rail orchestrator: boot + pollRail. Called from main.ts each RAF frame.
// E.23: Stats sampling.
// E.24: Inspector refresh.
//
// v1.6 Wave B: signatures drop the `WorldHandle` param. `pollRail` takes a
// `SnapshotHeader` (for stats sampling) + `SimBridge` (for inspector refresh).

import type { SnapshotHeader, SimBridge } from "../sim-bridge";
import { maybeSampleStats } from "./stats";
import { refreshInspector } from "./inspector";
import { pruneHighlights, highlights } from "./highlight";

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

export function installRail(): RailState {
  return installTabs();
}

// ---- pollRail (called each RAF frame) ----

export function pollRail(
  rail: RailState,
  snapshot: SnapshotHeader,
  simBridge: SimBridge,
): void {
  // 1. Stats sample (E.23). Reads from the snapshot header now.
  maybeSampleStats(snapshot);

  // 2. Inspector refresh (E.24). Sends async `inspect_id` requests via the
  //    SimBridge; replies render the panel when they arrive.
  refreshInspector(simBridge, rail);

  // 3. Highlight prune.
  pruneHighlights(performance.now());
}

export { highlights };
