// Stats panel: Canvas2D line chart (population) (E.23)
// + profiler table toggle (perf-timing plan).
// Samples collected per 10 sim-ticks; ring buffer of 500 samples.
//
// v1.6 Wave B: samples read from the snapshot header (no wasm on main thread).
// Sampling guard is now `tick - lastSampledTick >= SAMPLE_INTERVAL_TICKS`
// instead of `tick % 10 === 0` so high-TPS batches that step past multiple
// 10-tick boundaries still produce one sample. (gotcha 8)
//
// v1.9.1: DPR-aware sizing via a ResizeObserver on `#chart-pop`. Y-axis is
// hard-floored at 0; max label top-left, current population top-right; a
// single subtle gridline at y=ymax/2 in `var(--border)`. No tooltip.

import type { SnapshotHeader } from "../sim-bridge";

interface Sample {
  tick: number;
  population: number;
}

const samples: Sample[] = [];
const SAMPLE_CAP = 500;
const SAMPLE_INTERVAL_TICKS = 10;
const CHART_HEIGHT_CSS = 120;

let lastSampledTick = -1;
let popCanvas: HTMLCanvasElement | null = null;
let popResizeObserver: ResizeObserver | null = null;

function getCanvas(): HTMLCanvasElement | null {
  if (!popCanvas) popCanvas = document.getElementById("chart-pop") as HTMLCanvasElement;
  return popCanvas;
}

/**
 * Set up the population chart's ResizeObserver. Called from the Monitor tab
 * installer. Re-resizes the canvas backing buffer to clientWidth × DPR on
 * every layout change and re-renders the current sample buffer.
 */
export function installPopChart(): void {
  const canvas = getCanvas();
  if (!canvas) return;
  if (popResizeObserver) popResizeObserver.disconnect();
  resizePopCanvas();
  popResizeObserver = new ResizeObserver(() => {
    resizePopCanvas();
    drawChart();
  });
  popResizeObserver.observe(canvas);
  drawChart();
}

function resizePopCanvas(): void {
  const canvas = getCanvas();
  if (!canvas) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const cssW = canvas.clientWidth;
  if (cssW <= 0) return; // layout not flushed yet — ResizeObserver will re-fire.
  // Pin CSS height so the row stays a stable 120px regardless of DPR; only
  // the backing buffer scales up.
  canvas.style.height = `${CHART_HEIGHT_CSS}px`;
  canvas.width = Math.floor(cssW * dpr);
  canvas.height = Math.floor(CHART_HEIGHT_CSS * dpr);
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

function readCssVar(name: string, fallback: string): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

function drawChart(): void {
  const canvas = getCanvas();
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const Wcss = canvas.width / dpr;
  const Hcss = canvas.height / dpr;
  // Reset the transform first so re-renders don't accumulate scale.
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.scale(dpr, dpr);

  const borderColor = readCssVar("--border", "rgba(255,255,255,0.15)");
  const fgFaint = readCssVar("--fg-faint", "rgba(255,255,255,0.5)");
  const accent = readCssVar("--accent", "#7fc4ff");

  if (samples.length < 2) {
    ctx.fillStyle = fgFaint;
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText("pop: waiting…", 4, 14);
    return;
  }

  // v1.9.1: Y axis pinned at 0; the current sample range determines ymax.
  const ymax = Math.max(1, ...samples.map((s) => s.population));
  const xmin = samples[0].tick;
  const xmax = samples[samples.length - 1].tick;
  const xrange = Math.max(1, xmax - xmin);

  const padLeft = 4;
  const padRight = 4;
  const padTop = 16; // leaves room for the max/now labels
  const padBottom = 4;
  const plotW = Math.max(1, Wcss - padLeft - padRight);
  const plotH = Math.max(1, Hcss - padTop - padBottom);

  // Subtle gridline at y = ymax / 2.
  ctx.strokeStyle = borderColor;
  ctx.lineWidth = 1;
  ctx.beginPath();
  const midY = padTop + plotH * 0.5;
  ctx.moveTo(padLeft, midY);
  ctx.lineTo(Wcss - padRight, midY);
  ctx.stroke();

  // Data line.
  ctx.strokeStyle = accent;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  for (let i = 0; i < samples.length; i++) {
    const sx = padLeft + ((samples[i].tick - xmin) / xrange) * plotW;
    const sy = padTop + (1 - samples[i].population / ymax) * plotH;
    if (i === 0) ctx.moveTo(sx, sy);
    else ctx.lineTo(sx, sy);
  }
  ctx.stroke();

  // Labels: max top-left, current (now) top-right.
  const last = samples[samples.length - 1];
  ctx.fillStyle = fgFaint;
  ctx.font = "10px ui-monospace, monospace";
  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.fillText(`max: ${ymax.toFixed(0)}`, padLeft, 2);
  ctx.textAlign = "right";
  ctx.fillText(`now: ${last.population.toFixed(0)}`, Wcss - padRight, 2);
}
