// Eulogy card (F.28). Shown once when world.world_ended becomes true.
// Shows 4-image HoF grid, Copy Share Text, Download Save, New World.
// v5 §11, v6 §I.

import { renderCreaturePortrait } from "./render";
import type { PersistenceClient } from "./persistence";
import type { WorldHandle } from "../wasm/evosim";

const SHARE_BASE_URL = window.location.origin + window.location.pathname;

export interface GenomeJson {
  size: number;
  pigment_r: number;
  pigment_g: number;
  pigment_b: number;
  eye_count: number;
  move_speed: number;
  eat_efficiency: number;
  scavenge_efficiency: number;
  armor: number;
}

export interface HoFEntry {
  creature_id: number;
  genome: GenomeJson;
  species_name: string;
  captured_tick: number;
  captured_size: number;
  captured_age: number;
}

export interface HoFJson {
  first_mover: HoFEntry | null;
  biggest: HoFEntry | null;
  weirdest: HoFEntry | null;
  last_survivor: HoFEntry | null;
  day_count: number;
  peak_population: number;
  peak_species: number;
  seed: string;
}

export function showEulogyCard(world: WorldHandle, persistence: PersistenceClient): void {
  const hof: HoFJson = JSON.parse(world.hof_json());

  // Dim overlay over the aquarium.
  const backdrop = document.createElement("div");
  backdrop.className = "modal-backdrop eulogy-backdrop";

  const card = document.createElement("div");
  card.className = "modal-card eulogy-card";

  card.innerHTML = `
    <h2>The world has ended.</h2>
    <p class="eulogy-headline">
      Your world lived <strong>${hof.day_count}</strong> days,
      peaked at <strong>${hof.peak_population}</strong> creatures
      across <strong>${hof.peak_species}</strong> species.
    </p>
    <p class="eulogy-seed">Seed: <code>${escapeHtml(hof.seed)}</code></p>
    <div class="eulogy-grid">
      <div class="eulogy-cell" data-slot="first_mover">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">First to move</div>
      </div>
      <div class="eulogy-cell" data-slot="biggest">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">Biggest</div>
      </div>
      <div class="eulogy-cell" data-slot="weirdest">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">Weirdest</div>
      </div>
      <div class="eulogy-cell" data-slot="last_survivor">
        <div class="eulogy-cell-canvas"></div>
        <div class="eulogy-cell-caption">Last survivor</div>
      </div>
    </div>
    <div class="modal-actions">
      <button data-action="copy-share">Copy Share Text</button>
      <button data-action="download-save">Download Save</button>
      <button data-action="new-world" class="primary">New World</button>
    </div>
  `;

  // Populate the 4 portraits.
  populateCell(card, "first_mover", hof.first_mover);
  populateCell(card, "biggest", hof.biggest);
  populateCell(card, "weirdest", hof.weirdest);
  populateCell(card, "last_survivor", hof.last_survivor);

  // Wire buttons.
  card.querySelector('[data-action="copy-share"]')!.addEventListener("click", async () => {
    const url = `${SHARE_BASE_URL}?seed=${encodeURIComponent(hof.seed)}`;
    const text = `evosim seed=${hof.seed} — lived ${hof.day_count} days, peaked ${hof.peak_species} species, see ${url}`;
    await navigator.clipboard.writeText(text).catch(() => {});
    flashButton(card, '[data-action="copy-share"]', "Copied");
  });

  card.querySelector('[data-action="download-save"]')!.addEventListener("click", () => {
    const json = world.snapshot_json();
    const filename = `evosim-${hof.seed}-day${hof.day_count}.json`;
    triggerDownload(json, filename);
    flashButton(card, '[data-action="download-save"]', "Downloaded");
  });

  card.querySelector('[data-action="new-world"]')!.addEventListener("click", async () => {
    // F.28 PIN (B): delete saved world, then reload without ?seed= so the
    // resume prompt does not re-offer the just-finished world.
    try {
      await persistence.deleteCurrent();
    } catch (e) {
      console.warn("deleteCurrent failed (non-blocking):", e);
    }
    window.location.href = window.location.origin + window.location.pathname;
  });

  backdrop.appendChild(card);
  document.body.appendChild(backdrop);
}

function populateCell(card: HTMLElement, slot: string, entry: HoFEntry | null): void {
  const cell = card.querySelector(`[data-slot="${slot}"]`)!;
  const canvasHolder = cell.querySelector(".eulogy-cell-canvas")!;
  const captionEl = cell.querySelector(".eulogy-cell-caption")!;
  if (!entry) {
    captionEl.textContent = `${captionEl.textContent} — (none)`;
    return;
  }
  const c = renderCreaturePortrait(entry.genome, 96);
  c.style.borderRadius = "4px";
  canvasHolder.appendChild(c);
  captionEl.textContent = `${captionEl.textContent} · ${entry.species_name} · t=${entry.captured_tick}`;
}

function triggerDownload(text: string, filename: string): void {
  const blob = new Blob([text], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  setTimeout(() => {
    URL.revokeObjectURL(url);
    a.remove();
  }, 0);
}

function flashButton(card: HTMLElement, selector: string, label: string): void {
  const btn = card.querySelector(selector) as HTMLButtonElement;
  const original = btn.textContent ?? "";
  btn.textContent = label;
  btn.disabled = true;
  setTimeout(() => {
    btn.textContent = original;
    btn.disabled = false;
  }, 1200);
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
