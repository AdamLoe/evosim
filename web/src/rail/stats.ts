// Stats panel: two Canvas2D line charts (population + species count) (E.23)
// + profiler table toggle (perf-timing plan).
// Samples collected per 10 sim-ticks; ring buffer of 500 samples.

import type { WorldHandle } from "../../wasm/evosim";

interface Sample {
  tick: number;
  population: number;
  species: number;
}

const samples: Sample[] = [];
const SAMPLE_CAP = 500;
const SAMPLE_INTERVAL_TICKS = 10;

let lastSampledTick = -1;
let popCanvas: HTMLCanvasElement | null = null;
let spcCanvas: HTMLCanvasElement | null = null;

function getCanvases(): { pop: HTMLCanvasElement; spc: HTMLCanvasElement } | null {
  if (!popCanvas) popCanvas = document.getElementById("chart-pop") as HTMLCanvasElement;
  if (!spcCanvas) spcCanvas = document.getElementById("chart-species") as HTMLCanvasElement;
  if (!popCanvas || !spcCanvas) return null;
  return { pop: popCanvas, spc: spcCanvas };
}

export function maybeSampleStats(world: WorldHandle): void {
  const tick = world.tick;
  if (tick === lastSampledTick) return;
  if (tick % SAMPLE_INTERVAL_TICKS !== 0) return;
  const raw = world.stats_sample() as unknown as Float32Array;
  const t = raw[0];
  const pop = raw[1];
  const sp = raw[2];
  samples.push({ tick: t, population: pop, species: sp });
  if (samples.length > SAMPLE_CAP) samples.shift();
  lastSampledTick = tick;
  drawCharts();
}

function drawChart(
  canvas: HTMLCanvasElement,
  pick: (s: Sample) => number,
  color: string,
  label: string,
): void {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const W = canvas.width;
  const H = canvas.height;
  ctx.clearRect(0, 0, W, H);
  if (samples.length < 2) {
    // Draw empty placeholder text.
    ctx.fillStyle = "rgba(255,255,255,0.3)";
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText(`${label}: waiting…`, 4, 14);
    return;
  }
  const ymax = Math.max(1, ...samples.map(pick));
  const xmin = samples[0].tick;
  const xmax = samples[samples.length - 1].tick;
  const xrange = Math.max(1, xmax - xmin);

  // X-axis line.
  ctx.strokeStyle = "rgba(255,255,255,0.15)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(20, H - 12);
  ctx.lineTo(W, H - 12);
  ctx.stroke();

  // Data line.
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < samples.length; i++) {
    const px = 20 + ((samples[i].tick - xmin) / xrange) * (W - 22);
    const py = H - 12 - (pick(samples[i]) / ymax) * (H - 18);
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();

  // Label + current value.
  const last = samples[samples.length - 1];
  ctx.fillStyle = "rgba(255,255,255,0.7)";
  ctx.font = "10px ui-monospace, monospace";
  ctx.fillText(`${label}: ${pick(last).toFixed(0)}`, 4, 12);
  ctx.fillText(ymax.toFixed(0), 4, H - 14);
}

function drawCharts(): void {
  const c = getCanvases();
  if (!c) return;
  drawChart(c.pop, (s) => s.population, "#7fc4ff", "pop");
  drawChart(c.spc, (s) => s.species, "#ffb96b", "species");
}

