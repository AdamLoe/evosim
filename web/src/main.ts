import init, { WorldHandle, creature_stride } from "../wasm/evosim";
import { makeCamera, renderWorld } from "./render";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installCanvasClickHandler } from "./rail/inspector";

const status = document.getElementById("status") as HTMLSpanElement;
const canvas = document.getElementById("aquarium") as HTMLCanvasElement;
const ctx = canvas.getContext("2d");
if (!ctx) throw new Error("2d context unavailable");

let viewW = 0;
let viewH = 0;
function resize(): void {
  const dpr = window.devicePixelRatio || 1;
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

async function main(): Promise<void> {
  await init();

  const params = new URLSearchParams(window.location.search);
  const seed = params.get("seed") ?? "";
  const world = new WorldHandle(seed);
  const stride = creature_stride();
  const cam = makeCamera(world.world_size);
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => world.world_size,
  );

  status.textContent = `seed: ${world.seed}  ·  tick 0  ·  pop ${world.population}`;

  // Speed buttons in the top bar.
  installSpeedControls();

  // E.21: install right rail.
  const rail = installRail(world);

  // E.24: canvas click → inspector.
  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), world, rail);

  // Sim loop.
  let lastRender = performance.now();
  function frame(now: number): void {
    const delta = now - lastRender;
    lastRender = now;
    const ticksThisFrame = speed === 0 ? 0 : Math.min(200, Math.max(1, Math.round((speed * delta) / 16.66)));
    if (ticksThisFrame > 0) {
      world.step_n(ticksThisFrame);
    }

    // Fetch ids buffer once per frame (index-aligned with creatures_buffer).
    const ids = world.creature_ids_buffer() as unknown as Float64Array;

    // E.21/E.22/E.23/E.24: poll the rail (events, toasts, highlights, stats, inspector).
    pollRail(rail, world, ids);

    renderWorld(ctx!, cam, viewW, viewH, world, stride, ids, highlights, now);

    const ended = world.world_ended ? "  (world ended)" : "";
    status.textContent = `seed: ${world.seed}  ·  tick ${world.tick}  ·  pop ${world.population}  ·  species ${world.species_count}${ended}`;
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
    btn.textContent = s === 0 ? "❚❚" : `${s}×`;
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
