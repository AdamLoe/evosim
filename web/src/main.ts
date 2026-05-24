import init, { WorldHandle, creature_stride } from "../wasm/evosim";
import { makeCamera, renderWorld } from "./render";
import { attachCameraControls } from "./camera";
import { installRail, pollRail, highlights } from "./rail/index";
import { installCanvasClickHandler } from "./rail/inspector";
import { PersistenceClient } from "./persistence";
import { showResumePrompt, showSchemaMismatchModal } from "./persistence/ui";
import { showAutosaveFailureToast } from "./persistence/toast";
import { showEulogyCard } from "./eulogy";

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

// F.26: autosave parameters (v5 §13, F.26 DECISIONS).
const AUTOSAVE_TICK_INTERVAL = 300; // 10 sim-seconds at 30-tick/s baseline
const AUTOSAVE_WALL_FLOOR_MS = 3000; // never save more often than 3s wall-clock
let lastSavedTick = -1;
let lastSavedWallMs = 0;

// F.26 + v6 §C: camera state is persisted in the autosave envelope so resume
// restores zoom + pan. The wasm save string is wrapped in `{ wasm, camera }`
// JSON; older saves without the wrapper load with camera = default (handled in
// unwrapSaveEnvelope).
interface SaveEnvelope {
  wasm: unknown; // JSON.parse(world.snapshot_json())
  camera: { zoom: number; cx: number; cy: number };
}

function buildSaveEnvelope(world: WorldHandle, cam: { zoom: number; cx: number; cy: number }): string {
  const env: SaveEnvelope = {
    wasm: JSON.parse(world.snapshot_json()),
    camera: { zoom: cam.zoom, cx: cam.cx, cy: cam.cy },
  };
  return JSON.stringify(env);
}

function unwrapSaveEnvelope(json: string): { wasmJson: string; camera: SaveEnvelope["camera"] | null } {
  try {
    const parsed = JSON.parse(json) as Partial<SaveEnvelope> & { schema_version?: number };
    if (parsed && "wasm" in parsed && parsed.wasm) {
      const camera = parsed.camera ?? null;
      return { wasmJson: JSON.stringify(parsed.wasm), camera };
    }
    // Bare wasm save (no envelope) — backwards compatible.
    return { wasmJson: json, camera: null };
  } catch {
    return { wasmJson: json, camera: null };
  }
}

function maybeAutosave(
  world: WorldHandle,
  cam: { zoom: number; cx: number; cy: number },
  nowMs: number,
  persistence: PersistenceClient,
): void {
  const t = world.tick;
  if (t === lastSavedTick) return;
  if (lastSavedTick >= 0 && t - lastSavedTick < AUTOSAVE_TICK_INTERVAL) return;
  if (nowMs - lastSavedWallMs < AUTOSAVE_WALL_FLOOR_MS) return;
  persistence.save(buildSaveEnvelope(world, cam), world.seed, t);
  lastSavedTick = t;
  lastSavedWallMs = nowMs;
}

// F.28: eulogy gate — shown exactly once per session.
let eulogyShown = false;

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

  // F.26: persistence client.
  const persistence = new PersistenceClient();
  persistence.setHandlers({
    onError: (_code, _msg) => showAutosaveFailureToast(),
  });

  const params = new URLSearchParams(window.location.search);
  const urlSeed = params.get("seed");

  let world: WorldHandle | null = null;
  let restoredCamera: { zoom: number; cx: number; cy: number } | null = null;

  // F.26: boot-path resume logic.
  if (!urlSeed) {
    // No explicit seed → probe for a saved world.
    try {
      const row = await persistence.loadCurrent();
      if (row) {
        const DAY_TICKS = 1000;
        const day = Math.floor(row.tick / DAY_TICKS);
        const userChose = await showResumePrompt({
          day,
          seed: row.seed,
          savedAtMs: row.saved_at_ms,
        });
        if (userChose === "resume") {
          try {
            const { wasmJson, camera } = unwrapSaveEnvelope(row.json);
            world = WorldHandle.fromJson(wasmJson);
            restoredCamera = camera;
          } catch (e) {
            const msg = String(e);
            if (msg.startsWith("schema-mismatch:")) {
              await showSchemaMismatchModal();
            } else {
              console.warn("load failed (treating as unusable save):", msg);
              await showSchemaMismatchModal();
            }
            // Fall through to fresh world.
          }
        }
        // userChose === "new" → fall through to fresh world.
      }
    } catch (e) {
      // Worker error at load time — log and proceed fresh (no toast; user wasn't expecting feedback).
      console.warn("resume probe failed:", e);
    }
  }

  if (!world) {
    world = new WorldHandle(urlSeed ?? "");
  }

  // F.27: seed display.
  installSeedDisplay(world.seed);

  const stride = creature_stride();
  const cam = makeCamera(world.world_size);
  if (restoredCamera) {
    cam.zoom = restoredCamera.zoom;
    cam.cx = restoredCamera.cx;
    cam.cy = restoredCamera.cy;
  }
  attachCameraControls(
    canvas,
    cam,
    () => ({ w: viewW, h: viewH }),
    () => world!.world_size,
  );

  status.textContent = `seed: ${world.seed}  ·  tick 0  ·  pop ${world.population}`;

  // Speed buttons in the top bar.
  installSpeedControls();

  // E.21: install right rail.
  const rail = installRail(world);

  // E.24: canvas click → inspector.
  installCanvasClickHandler(canvas, cam, () => ({ w: viewW, h: viewH }), world, rail);

  // Sim + render loop.
  let lastRender = performance.now();
  function frame(now: number): void {
    const delta = now - lastRender;
    lastRender = now;
    const ticksThisFrame =
      speed === 0 ? 0 : Math.min(200, Math.max(1, Math.round((speed * delta) / 16.66)));

    if (ticksThisFrame > 0 && !world!.world_ended) {
      world!.step_n(ticksThisFrame);
    }

    // F.26: autosave after each frame batch.
    if (!world!.world_ended) {
      maybeAutosave(world!, cam, now, persistence);
    }

    // F.28: eulogy — fire once when world ends.
    if (world!.world_ended && !eulogyShown) {
      eulogyShown = true;
      showEulogyCard(world!, persistence);
    }

    // Fetch ids buffer once per frame (index-aligned with creatures_buffer).
    const ids = world!.creature_ids_buffer() as unknown as Float64Array;

    // E.21/E.22/E.23/E.24: poll the rail (events, toasts, highlights, stats, inspector).
    pollRail(rail, world!, ids);

    renderWorld(ctx!, cam, viewW, viewH, world!, stride, ids, highlights, now);

    const ended = world!.world_ended ? "  (world ended)" : "";
    status.textContent = `seed: ${world!.seed}  ·  tick ${world!.tick}  ·  pop ${world!.population}  ·  species ${world!.species_count}${ended}`;
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
