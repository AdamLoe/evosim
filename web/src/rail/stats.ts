// Stats panel: Canvas2D line chart (population) (E.23)
// + profiler table toggle (perf-timing plan).
// Samples collected per 10 sim-ticks; ring buffer of 500 samples.
//
// v1.6 Wave B: samples read from the snapshot header (no wasm on main thread).
// Sampling guard is now `tick - lastSampledTick >= SAMPLE_INTERVAL_TICKS`
// instead of `tick % 10 === 0` so high-TPS batches that step past multiple
// 10-tick boundaries still produce one sample. (gotcha 8)

import type { SnapshotHeader } from "../sim-bridge";

interface Sample {
  tick: number;
  population: number;
}

const samples: Sample[] = [];
const SAMPLE_CAP = 500;
const SAMPLE_INTERVAL_TICKS = 10;

let lastSampledTick = -1;
let popCanvas: HTMLCanvasElement | null = null;

function getCanvas(): HTMLCanvasElement | null {
  if (!popCanvas) popCanvas = document.getElementById("chart-pop") as HTMLCanvasElement;
  return popCanvas;
}

/** Drop all recorded samples so the chart starts fresh after a world restart. */
export function resetStats(): void {
  samples.length = 0;
  lastSampledTick = -1;
}

export function maybeSampleStats(snapshot: SnapshotHeader): void {
  const tick = snapshot.tick;
  if (tick === lastSampledTick) return;
  // gotcha 8: `>= SAMPLE_INTERVAL_TICKS` survives batches that step past
  // multiple 10-tick boundaries (which `tick % 10 === 0` would skip).
  if (lastSampledTick >= 0 && tick - lastSampledTick < SAMPLE_INTERVAL_TICKS) return;
  samples.push({ tick, population: snapshot.pop });
  if (samples.length > SAMPLE_CAP) samples.shift();
  lastSampledTick = tick;
  drawChart();
}

function drawChart(): void {
  const canvas = getCanvas();
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const W = canvas.width;
  const H = canvas.height;
  ctx.clearRect(0, 0, W, H);
  if (samples.length < 2) {
    ctx.fillStyle = "rgba(255,255,255,0.3)";
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText("pop: waiting…", 4, 14);
    return;
  }
  const ymax = Math.max(1, ...samples.map((s) => s.population));
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
  ctx.strokeStyle = "#7fc4ff";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < samples.length; i++) {
    const px = 20 + ((samples[i].tick - xmin) / xrange) * (W - 22);
    const py = H - 12 - (samples[i].population / ymax) * (H - 18);
    if (i === 0) ctx.moveTo(px, py);
    else ctx.lineTo(px, py);
  }
  ctx.stroke();

  // Label + current value.
  const last = samples[samples.length - 1];
  ctx.fillStyle = "rgba(255,255,255,0.7)";
  ctx.font = "10px ui-monospace, monospace";
  ctx.fillText(`pop: ${last.population.toFixed(0)}`, 4, 12);
  ctx.fillText(ymax.toFixed(0), 4, H - 14);
}
