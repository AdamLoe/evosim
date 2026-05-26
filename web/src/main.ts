import init, { WorldHandle, creature_stride, initThreadPool } from "../wasm/evosim";
import { makeCamera, renderWorld } from "./render";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installProfilerPanel } from "./widgets/perf-panel";
import { installDevPanel, getInitialGrassSeedCount } from "./widgets/devpanel";
import { installCanvasClickHandler } from "./rail/inspector";
import { attachProfiler, timed, span } from "./perf";

const status = document.getElementById("status") as HTMLSpanElement;
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const ctx = canvas.getContext("2d");
if (!ctx) throw new Error("2d context unavailable");

let viewW = 0;
let viewH = 0;
function resize(): void {
  // S22: cap DPR at 2 to avoid 3× pixel overdraw on 3× displays.
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  viewW = window.innerWidth;
  viewH = window.innerHeight;
  canvas.width = Math.floor(viewW * dpr);
  canvas.height = Math.floor(viewH * dpr);
  canvas.style.width = `${viewW}px`;
  canvas.style.height = `${viewH}px`;
  ctx!.setTransform(dpr, 0, 0, dpr, 0, 0);
}
window.addEventListener("resize", resize);
resize();

type Speed = 0 | 1 | 10 | 100;
let speed: Speed = 1;


// F.27: seed display + copy button.
function installSeedDisplay(seed: string): void {
  const valueEl = document.getElementById("seed-value");
  if (valueEl) valueEl.textContent = seed;
  const btn = document.getElementById("seed-copy-btn") as HTMLButtonElement | null;
  if (btn) {
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(seed);
        btn.textContent = "copied";
        setTimeout(() => (btn.textContent = "copy"), 1200);
      } catch (e) {
        console.warn("clipboard copy failed", e);
      }
    });
  }
}

async function main(): Promise<void> {
  await init();

  // Threads: spin up the rayon worker pool BEFORE any WorldHandle is
  // constructed so the first tick gets parallelism. SAB feature-detect:
  // browsers without SharedArrayBuffer skip initThreadPool — the wasm
  // still works, just sequentially (rayon falls back to the calling
  // thread when no workers are registered). See docs/plans/perf-4-threads.md.
  if (typeof SharedArrayBuffer !== "undefined") {
    try {
      await initThreadPool(navigator.hardwareConcurrency);
    } catch (e) {
      console.warn("initThreadPool failed; continuing single-threaded:", e);
    }
  } else {
    console.warn("SharedArrayBuffer not available; running single-threaded");
  }

  const params = new URLSearchParams(window.location.search);
  // Cap seed param length to prevent oversized inputs (S15).
  const urlSeed = (params.get("seed") ?? "").slice(0, 128) || null;

  const world: WorldHandle = WorldHandle.newWithGrassSeed(urlSeed ?? "", getInitialGrassSeedCount());

  // S22: cache seed once per world lifetime (world.seed is a getter that
  // allocates a new String each call; no need to call it per frame).
  const cachedSeed = world.seed;

  // F.27: seed display.
  installSeedDisplay(cachedSeed);

  const stride = creature_stride();
  const cam = makeCamera(world.world_size);
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => world.world_size,
  );

  status.textContent = `seed: ${cachedSeed}  ·  tick 0  ·  pop ${world.population}`;

  // Speed buttons in the top bar.
  installSpeedControls();

  // E.21: install right rail.
  const rail = installRail(world);

  // E.24: canvas click → inspector.
  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), world, rail);

  // Profiler: attach world after construction so the TS profiler can
  // forward enable/disable calls to the Rust side (D1, D9).
  attachProfiler(world);

  // Perf-timing: install the Stats-panel toggle + 1Hz polling loop.
  installProfilerPanel(world);

  // P3b: dev panel overlay (7 sliders, ~ hotkey, ⚙ button).
  installDevPanel(world);

  // Sim + render loop.
  let lastRender = performance.now();
  // S22: throttle status DOM updates to 5 Hz (200 ms gate).
  let lastStatusUpdate = 0;
  function frame(now: number): void {
    const frameSpan = span("frame");
    try {
      const delta = now - lastRender;
      lastRender = now;
      const ticksThisFrame =
        speed === 0 ? 0 : Math.min(200, Math.max(1, Math.round((speed * delta) / 16.66)));

      // S22: hoist world_ended once per RAF frame (was called 3× per frame).
      const ended = world.world_ended;

      if (ticksThisFrame > 0 && !ended) {
        timed("step_n", () => world.step_n(ticksThisFrame));
      }

      // Fetch ids buffer once per frame (index-aligned with creatures_buffer).
      const ids = world.creature_ids_buffer() as unknown as Float64Array;

      // E.21/E.22/E.23/E.24: poll the rail (events, toasts, highlights, stats, inspector).
      // S18: ids no longer passed to pollRail (inspector uses creature_idx_by_id instead).
      timed("pollRail", () => pollRail(rail, world));

      timed("renderWorld", () =>
        renderWorld(ctx!, cam, viewW, viewH, world, stride, ids, highlights, now));

      // S22: throttle status DOM updates to 5 Hz (200 ms gate).
      if (now - lastStatusUpdate > 200) {
        lastStatusUpdate = now;
        const endedSuffix = ended ? "  (world ended)" : "";
        status.textContent =
          `seed: ${cachedSeed}  ·  tick ${world.tick}  ·  pop ${world.population}  ·  species ${world.species_count}${endedSuffix}`;
      }
    } finally {
      frameSpan.close();
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

function installSpeedControls(): void {
  const bar = document.getElementById("top-bar")!;
  const wrap = document.createElement("span");
  wrap.style.marginLeft = "auto";
  for (const s of [0, 1, 10, 100] as const) {
    const btn = document.createElement("button");
    btn.textContent = s === 0 ? "pause" : `${s}x`;
    btn.style.marginLeft = "4px";
    btn.style.background = "rgba(255,255,255,0.08)";
    btn.style.color = "var(--fg)";
    btn.style.border = "1px solid rgba(255,255,255,0.15)";
    btn.style.padding = "2px 8px";
    btn.style.borderRadius = "3px";
    btn.style.cursor = "pointer";
    btn.style.font = "inherit";
    btn.onclick = () => {
      speed = s;
    };
    wrap.appendChild(btn);
  }
  bar.appendChild(wrap);
}

main().catch((err) => {
  status.textContent = `boot failed: ${err}`;
  console.error(err);
});
