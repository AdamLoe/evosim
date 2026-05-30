// v1.12 NN tab. Two independent editors share a single panel:
//
//   • Topology (stage-then-apply): hidden-layer list with width + activation.
//     Apply respawns the worker via the injected `restart()` callback.
//
//   • Mutation buckets (live-apply): 8-row table of {weight, rate, sigma}.
//     Each edit writes through to Settings AND pushes a debounced
//     `set_slider` so the bucket draw in Brain::child_from sees the new
//     policy on the next birth.
//
// A per-hidden-layer perf-log section reads `nn.forward.l{k}` rows out of
// the polled profiler report (poll cadence matches the perf panel). The
// section uses the 1-based naming convention already in src/world/nn.rs.

import type { SimBridge } from "../sim-bridge";
import {
  type ActivationName,
  DEFAULT_MUTATION_BUCKETS,
  DEFAULT_NN_TOPOLOGY,
  MUTATION_BUCKET_COUNT,
  getSettings,
  setSetting,
  type MutationBucket,
  type NnTopology,
} from "../settings";

const MIN_WIDTH = 8;
const MAX_WIDTH = 256;
const MAX_HIDDEN_LAYERS = 8;

const ACTIVATIONS: ActivationName[] = ["lrelu", "relu", "tanh", "sigmoid", "linear"];

interface TopologyState {
  layerSizes: number[];
  activations: ActivationName[];
}

/** Install the NN tab. Returns nothing — DOM is mutated under `#nn-tab-host`.
 * `getBridge()` is queried lazily so the same installer survives a worker
 * respawn (the bridge instance is replaced; the installer is not). */
export function installNnTab(getBridge: () => SimBridge, onApplyTopology: () => Promise<void>): void {
  const host = document.getElementById("nn-tab-host");
  if (!host) return;
  host.innerHTML = "";

  // ─── Topology editor ───────────────────────────────────────────────────
  const topoSec = document.createElement("section");
  topoSec.className = "nn-section";
  topoSec.innerHTML = `<div class="rail-section-title">Network structure (next world)</div>`;

  const layerListEl = document.createElement("div");
  layerListEl.className = "nn-layer-list";
  topoSec.appendChild(layerListEl);

  const addLayerBtn = document.createElement("button");
  addLayerBtn.type = "button";
  addLayerBtn.className = "nn-add-layer";
  addLayerBtn.textContent = "+ Add hidden layer";
  topoSec.appendChild(addLayerBtn);

  const topoFooter = document.createElement("div");
  topoFooter.className = "nn-footer";
  topoFooter.innerHTML =
    `<button id="nn-topo-apply" type="button" disabled>Apply (respawn)</button>` +
    `<button id="nn-topo-cancel" type="button" disabled>Cancel</button>` +
    `<button id="nn-topo-reset" type="button">Reset</button>` +
    `<span class="nn-validation" id="nn-topo-validation"></span>`;
  topoSec.appendChild(topoFooter);

  host.appendChild(topoSec);

  // ─── Mutation bucket editor (stage-then-apply) ─────────────────────────
  const bucketSec = document.createElement("section");
  bucketSec.className = "nn-section";
  bucketSec.innerHTML =
    `<div class="rail-section-title">Mutation buckets</div>` +
    `<div class="nn-help">` +
    `At every birth, one bucket is picked by <b>weight</b> (relative share, ` +
    `any value ≥ 0) and that bucket's <b>rate</b> + <b>sigma</b> drive the ` +
    `weight perturbation:` +
    `<ul class="nn-help-list">` +
    `<li><b>weight</b> — relative selection share. All-zero weights = no mutation at all (frozen).</li>` +
    `<li><b>rate</b> ∈ [0, 1] — per-NN-weight chance of being touched. 0.02 ≈ 2% of weights mutated.</li>` +
    `<li><b>sigma</b> ≥ 0 — Gaussian step size added to a touched weight. Typical 0.005–0.05; larger = bigger jumps.</li>` +
    `</ul>` +
    `Changes are staged — click <b>Apply</b> to push to the sim. Bucket edits ` +
    `take effect on the next birth; no restart needed.` +
    `</div>`;

  const bucketRowsEl = document.createElement("div");
  bucketRowsEl.className = "nn-bucket-rows";
  bucketSec.appendChild(bucketRowsEl);

  const bucketFooter = document.createElement("div");
  bucketFooter.className = "nn-footer";
  bucketFooter.innerHTML =
    `<button id="nn-buckets-apply" type="button" disabled>Apply</button>` +
    `<button id="nn-buckets-cancel" type="button" disabled>Cancel</button>` +
    `<button id="nn-buckets-reset" type="button">Reset</button>` +
    `<span class="nn-validation" id="nn-buckets-status"></span>`;
  bucketSec.appendChild(bucketFooter);

  host.appendChild(bucketSec);

  // ─── Perf log ──────────────────────────────────────────────────────────
  const perfSec = document.createElement("section");
  perfSec.className = "nn-section";
  perfSec.innerHTML = `<div class="rail-section-title">Per-layer forward (live)</div>`;
  const perfTableEl = document.createElement("div");
  perfTableEl.className = "nn-perf-rows";
  perfTableEl.innerHTML = `<div class="nn-perf-empty">(no samples yet)</div>`;
  perfSec.appendChild(perfTableEl);
  host.appendChild(perfSec);

  // ─── State + render: topology ──────────────────────────────────────────
  const persistedTopo = getSettings().nnTopology;
  let snapshotTopo: TopologyState = cloneTopo(persistedTopo);
  let liveTopo: TopologyState = cloneTopo(persistedTopo);

  const applyBtn = topoFooter.querySelector("#nn-topo-apply") as HTMLButtonElement;
  const cancelBtn = topoFooter.querySelector("#nn-topo-cancel") as HTMLButtonElement;
  const resetBtn = topoFooter.querySelector("#nn-topo-reset") as HTMLButtonElement;
  const validationEl = topoFooter.querySelector("#nn-topo-validation") as HTMLSpanElement;

  function renderTopo(): void {
    layerListEl.innerHTML = "";
    // Pinned input row (32 inputs, no controls).
    const inputRow = document.createElement("div");
    inputRow.className = "nn-layer-row nn-layer-pinned";
    inputRow.innerHTML = `<span class="nn-layer-label">inputs</span>` +
      `<span class="nn-layer-width">32</span>`;
    layerListEl.appendChild(inputRow);

    liveTopo.layerSizes.forEach((width, idx) => {
      const row = document.createElement("div");
      row.className = "nn-layer-row";
      const widthInput = document.createElement("input");
      widthInput.type = "number";
      widthInput.min = String(MIN_WIDTH);
      widthInput.max = String(MAX_WIDTH);
      widthInput.step = "8";
      widthInput.value = String(width);
      widthInput.className = "nn-layer-width-input";
      widthInput.addEventListener("input", () => {
        const n = Math.round(Number(widthInput.value));
        liveTopo.layerSizes[idx] = isFinite(n) ? n : 0;
        refreshDirty();
      });

      const actSelect = document.createElement("select");
      actSelect.className = "nn-layer-act";
      for (const a of ACTIVATIONS) {
        const opt = document.createElement("option");
        opt.value = a;
        opt.textContent = a;
        if (a === liveTopo.activations[idx]) opt.selected = true;
        actSelect.appendChild(opt);
      }
      actSelect.addEventListener("change", () => {
        liveTopo.activations[idx] = actSelect.value as ActivationName;
        refreshDirty();
      });

      const rmBtn = document.createElement("button");
      rmBtn.type = "button";
      rmBtn.className = "nn-layer-remove";
      rmBtn.textContent = "×";
      rmBtn.title = "Remove this hidden layer";
      rmBtn.disabled = liveTopo.layerSizes.length <= 1;
      rmBtn.addEventListener("click", () => {
        if (liveTopo.layerSizes.length <= 1) return;
        liveTopo.layerSizes.splice(idx, 1);
        liveTopo.activations.splice(idx, 1);
        renderTopo();
        refreshDirty();
      });

      row.innerHTML = `<span class="nn-layer-label">h${idx + 1}</span>`;
      row.appendChild(widthInput);
      row.appendChild(actSelect);
      row.appendChild(rmBtn);
      layerListEl.appendChild(row);
    });

    // Pinned output row.
    const outRow = document.createElement("div");
    outRow.className = "nn-layer-row nn-layer-pinned";
    outRow.innerHTML = `<span class="nn-layer-label">outputs</span>` +
      `<span class="nn-layer-width">5</span>`;
    layerListEl.appendChild(outRow);

    addLayerBtn.disabled = liveTopo.layerSizes.length >= MAX_HIDDEN_LAYERS;
  }

  function validateTopo(t: TopologyState): string | null {
    if (t.layerSizes.length === 0) return "Need at least one hidden layer.";
    if (t.layerSizes.length > MAX_HIDDEN_LAYERS) return `Max ${MAX_HIDDEN_LAYERS} hidden layers.`;
    if (t.layerSizes.length !== t.activations.length) return "Activation count mismatch (internal).";
    for (const w of t.layerSizes) {
      if (!isFinite(w) || w < MIN_WIDTH || w > MAX_WIDTH || w % 8 !== 0) {
        return `Each width must be a multiple of 8 in [${MIN_WIDTH}, ${MAX_WIDTH}].`;
      }
    }
    return null;
  }

  function refreshDirty(): void {
    const dirty = !sameTopo(snapshotTopo, liveTopo);
    const error = validateTopo(liveTopo);
    applyBtn.disabled = !dirty || error !== null;
    cancelBtn.disabled = !dirty;
    validationEl.textContent = error ?? "";
    validationEl.classList.toggle("is-error", error !== null);
  }

  addLayerBtn.addEventListener("click", () => {
    if (liveTopo.layerSizes.length >= MAX_HIDDEN_LAYERS) return;
    liveTopo.layerSizes.push(24);
    liveTopo.activations.push("lrelu");
    renderTopo();
    refreshDirty();
  });

  cancelBtn.addEventListener("click", () => {
    liveTopo = cloneTopo(snapshotTopo);
    renderTopo();
    refreshDirty();
  });

  resetBtn.addEventListener("click", () => {
    liveTopo = cloneTopo(DEFAULT_NN_TOPOLOGY);
    renderTopo();
    refreshDirty();
  });

  applyBtn.addEventListener("click", () => {
    const err = validateTopo(liveTopo);
    if (err) return;
    const toSave: NnTopology = cloneTopo(liveTopo);
    setSetting("nnTopology", toSave);
    snapshotTopo = cloneTopo(liveTopo);
    refreshDirty();
    void onApplyTopology();
  });

  renderTopo();
  refreshDirty();

  // ─── State + render: buckets (stage-then-apply) ────────────────────────
  const defaultBuckets = (): MutationBucket[] =>
    DEFAULT_MUTATION_BUCKETS.map((b) => ({ ...b }));
  const cloneBuckets = (src: MutationBucket[]): MutationBucket[] =>
    src.map((b) => ({ ...b }));

  let snapshotBuckets: MutationBucket[] = cloneBuckets(getSettings().mutationBuckets);
  let liveBuckets: MutationBucket[] = cloneBuckets(snapshotBuckets);
  const bucketBars: HTMLDivElement[] = [];
  const bucketInputs: Record<keyof MutationBucket, HTMLInputElement[]> = {
    weight: [],
    rate: [],
    sigma: [],
  };

  const bucketsApply = bucketFooter.querySelector("#nn-buckets-apply") as HTMLButtonElement;
  const bucketsCancel = bucketFooter.querySelector("#nn-buckets-cancel") as HTMLButtonElement;
  const bucketsReset = bucketFooter.querySelector("#nn-buckets-reset") as HTMLButtonElement;
  const bucketsStatus = bucketFooter.querySelector("#nn-buckets-status") as HTMLSpanElement;

  function totalWeight(): number {
    return liveBuckets.reduce((s, b) => s + Math.max(0, b.weight), 0);
  }
  function refreshBars(): void {
    const total = totalWeight();
    bucketBars.forEach((bar, k) => {
      const w = Math.max(0, liveBuckets[k].weight);
      const pct = total > 0 ? (w / total) * 100 : 0;
      bar.style.width = `${pct.toFixed(1)}%`;
    });
  }
  function sameBuckets(a: MutationBucket[], b: MutationBucket[]): boolean {
    for (let i = 0; i < a.length; i++) {
      if (a[i].weight !== b[i].weight) return false;
      if (a[i].rate !== b[i].rate) return false;
      if (a[i].sigma !== b[i].sigma) return false;
    }
    return true;
  }
  function refreshBucketsDirty(): void {
    const dirty = !sameBuckets(snapshotBuckets, liveBuckets);
    bucketsApply.disabled = !dirty;
    bucketsCancel.disabled = !dirty;
    bucketsStatus.textContent = dirty ? "unsaved changes" : "";
    // Toggle a `.is-dirty` class on each row whose value differs from snapshot.
    for (let k = 0; k < MUTATION_BUCKET_COUNT; k++) {
      const rowEl = bucketRowsEl.children[k] as HTMLElement | undefined;
      if (!rowEl) continue;
      const rowDirty =
        liveBuckets[k].weight !== snapshotBuckets[k].weight ||
        liveBuckets[k].rate !== snapshotBuckets[k].rate ||
        liveBuckets[k].sigma !== snapshotBuckets[k].sigma;
      rowEl.classList.toggle("is-dirty", rowDirty);
    }
  }
  function writeInputsFromLive(): void {
    for (let k = 0; k < MUTATION_BUCKET_COUNT; k++) {
      bucketInputs.weight[k].value = String(liveBuckets[k].weight);
      bucketInputs.rate[k].value = String(liveBuckets[k].rate);
      bucketInputs.sigma[k].value = String(liveBuckets[k].sigma);
    }
    refreshBars();
    refreshBucketsDirty();
  }

  function makeFieldGroup(
    k: number,
    field: keyof MutationBucket,
    label: string,
    step: string,
    max: string | null,
  ): HTMLDivElement {
    const wrap = document.createElement("div");
    wrap.className = "nn-field";
    const lbl = document.createElement("label");
    lbl.className = "nn-field-label";
    lbl.textContent = label;
    const input = document.createElement("input");
    input.type = "number";
    input.className = "nn-field-input";
    input.min = "0";
    input.step = step;
    if (max !== null) input.max = max;
    input.value = String(liveBuckets[k][field]);
    input.addEventListener("input", () => {
      const n = Number(input.value);
      let v = isFinite(n) ? Math.max(0, n) : 0;
      if (field === "rate") v = Math.min(1, v);
      liveBuckets[k][field] = v;
      if (field === "weight") refreshBars();
      refreshBucketsDirty();
    });
    bucketInputs[field].push(input);
    wrap.appendChild(lbl);
    wrap.appendChild(input);
    return wrap;
  }

  for (let k = 0; k < MUTATION_BUCKET_COUNT; k++) {
    const row = document.createElement("div");
    row.className = "nn-bucket-card";

    const head = document.createElement("div");
    head.className = "nn-bucket-head";
    const idx = document.createElement("span");
    idx.className = "nn-bucket-idx";
    idx.textContent = `bucket ${k}`;
    head.appendChild(idx);
    const barWrap = document.createElement("div");
    barWrap.className = "nn-bucket-bar";
    const bar = document.createElement("div");
    bar.className = "nn-bucket-bar-fill";
    barWrap.appendChild(bar);
    bucketBars.push(bar);
    head.appendChild(barWrap);
    row.appendChild(head);

    const fields = document.createElement("div");
    fields.className = "nn-bucket-fields";
    fields.appendChild(makeFieldGroup(k, "weight", "weight", "0.01", null));
    fields.appendChild(makeFieldGroup(k, "rate", "rate", "0.001", "1"));
    fields.appendChild(makeFieldGroup(k, "sigma", "sigma", "0.001", null));
    row.appendChild(fields);

    bucketRowsEl.appendChild(row);
  }
  refreshBars();
  refreshBucketsDirty();

  function pushAllBuckets(): void {
    const bridge = getBridge();
    for (let k = 0; k < MUTATION_BUCKET_COUNT; k++) {
      const b = liveBuckets[k];
      bridge.debouncedSetSlider(`bucket_${k}_weight`, Math.max(0, b.weight));
      bridge.debouncedSetSlider(`bucket_${k}_rate`, Math.min(1, Math.max(0, b.rate)));
      bridge.debouncedSetSlider(`bucket_${k}_sigma`, Math.max(0, b.sigma));
    }
  }

  bucketsApply.addEventListener("click", () => {
    if (sameBuckets(snapshotBuckets, liveBuckets)) return;
    setSetting("mutationBuckets", cloneBuckets(liveBuckets));
    pushAllBuckets();
    snapshotBuckets = cloneBuckets(liveBuckets);
    refreshBucketsDirty();
  });

  bucketsCancel.addEventListener("click", () => {
    liveBuckets = cloneBuckets(snapshotBuckets);
    writeInputsFromLive();
  });

  bucketsReset.addEventListener("click", () => {
    liveBuckets = defaultBuckets();
    writeInputsFromLive();
  });

  // ─── Perf log poll ─────────────────────────────────────────────────────
  setInterval(() => updatePerfRows(perfTableEl), 1000);
}

function cloneTopo<T extends { layerSizes: number[]; activations: ActivationName[] }>(t: T): TopologyState {
  return {
    layerSizes: t.layerSizes.slice(),
    activations: t.activations.slice(),
  };
}
function sameTopo(a: TopologyState, b: TopologyState): boolean {
  if (a.layerSizes.length !== b.layerSizes.length) return false;
  for (let i = 0; i < a.layerSizes.length; i++) {
    if (a.layerSizes[i] !== b.layerSizes[i]) return false;
    if (a.activations[i] !== b.activations[i]) return false;
  }
  return true;
}

// Per-layer perf log: scan the polled profile report for `nn.forward.l*` rows
// and emit one perf row per hidden layer found.
interface ProfilerNode {
  name: string;
  total_us: number;
  call_count: number;
  total_call_count?: number;
  effective_window_ms?: number;
  children?: ProfilerNode[];
}
declare global {
  interface Window {
    __lastProfilerReport?: { tree: ProfilerNode[] };
  }
}
function updatePerfRows(host: HTMLElement): void {
  const report = window.__lastProfilerReport;
  if (!report) return;
  // Walk roots looking for `nn` → `forward` → l* children.
  let nnForward: ProfilerNode | null = null;
  for (const root of report.tree ?? []) {
    if (root.name !== "nn") continue;
    const buildOrForward = root.children ?? [];
    for (const c of buildOrForward) {
      if (c.name === "nn.forward") {
        nnForward = c;
        break;
      }
    }
  }
  if (!nnForward || !(nnForward.children?.length ?? 0)) {
    return;
  }
  const rows: string[] = [];
  for (const layer of nnForward.children!) {
    // Names look like `nn.forward.l1`. Strip the prefix.
    const label = layer.name.replace(/^nn\.forward\./, "");
    const totalMs = layer.total_us / 1000;
    const calls = (layer.total_call_count ?? layer.call_count) || 0;
    const usPerCall = calls > 0 ? layer.total_us / calls : 0;
    rows.push(
      `<div class="nn-perf-row">` +
      `<span class="nn-perf-label">${label}</span>` +
      `<span class="nn-perf-cell">${usPerCall.toFixed(2)} µs/call</span>` +
      `<span class="nn-perf-cell">${totalMs.toFixed(2)} ms total</span>` +
      `</div>`
    );
  }
  host.innerHTML = rows.length
    ? rows.join("")
    : `<div class="nn-perf-empty">(no samples yet)</div>`;
}
