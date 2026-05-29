# Render pipeline

The WebGL2 renderer, the camera, and the per-frame SAB snapshot read.

## What it is

A single instanced WebGL2 draw call for every creature body (plus a
second instanced pass for the rare highlight rings), one full-screen
quad sampling an R8 grass density texture, and one `LINE_LOOP` for the
world-bounds frame. The render loop runs on the main thread, owns no
wasm handle, and reads the live snapshot SAB slot directly via typed
arrays — no copy, no postMessage.

## What it owns

- The GL programs (`disc`, `grass`, `frame`), VAOs, instance buffers,
  the R8 grass texture, the instance scratch buffer.
- JS-side **frustum culling**. Per-creature x/y/radius is read from the
  SoA; off-screen creatures are skipped before they touch the instance
  buffer.
- The **JS-side instance pack** (the data the GPU draws). Replaces the
  deleted Rust `pack_render_buffer` path.
- The per-frame `frame.snapshot.read` span (`Atomics.load` +
  typed-array view construction). Cheap (~free) but visible in the
  profiler so its absence vs presence shows up if a future change starts
  copying.
- The outer `frame` RAII span and the inner `frame.render_world`,
  `frame.render_world.grass`, `frame.render_world.creatures` spans.
- The camera helpers (`makeCamera`, `clampCamera`, `worldToScreen`,
  `PX_PER_SIZE`) and pointer-driven pan/zoom controls.
- The highlight-ring decode: per-frame extraction of stable creature ids
  from the SAB's interleaved id_lo/id_hi u32 pair via a `Uint32Array`
  view over the same backing buffer.

## What it does NOT own

- **The SAB layout or the snapshot byte stride** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md). The
  renderer reads `CREATURE_STRIDE` and the slot-offset helpers from
  `sim-bridge.ts`.
- **The sim tick rate or any sim mutation** — owned by
  [`worker-runtime.md`](worker-runtime.md). The render rate is just RAF.
- **The profile-tree shape** — owned by [`profiler.md`](profiler.md).
  The renderer only opens spans by name.
- **The dev-panel UI, the rail tabs, the inspector** — those live in
  `web/src/widgets/` and `web/src/rail/` and only border the renderer
  via the camera + the snapshot views passed into `renderWorld`.

## Per-frame shape (in `main.ts`)

```text
RAF callback (wrapped in `span("frame")`):
  1. early-return if SAB handles aren't bound yet (e.g., during restart)
  2. open `frame.snapshot.read` span
     2a. slot = Atomics.load(controlI32, CTRL_CURRENT_SLOT)
     2b. header = readSnapshotHeader(snapshotView, slotOffset(slot))
     2c. creatures = new Float32Array(snapshotSab, creatureSoAOffset(slot), pop * CREATURE_STRIDE)
     2d. grass     = new Uint8Array(snapshotSab, grassOffset(slot), GRASS_BYTES)
  3. close `frame.snapshot.read`
  4. (auto-restart on world_ended if enabled)
  5. pollRail(rail, header, simBridge, creatures, pop)
  6. renderWorld(gl, cam, viewW, viewH, creatures, grass, pop,
                 worldSize, grassDim, highlights, now)
  7. update status bar (every RAF — TPS from header, FPS from a
     1 s rolling main-side frame counter)
```

`renderWorld` opens `frame.render_world` and inside it
`frame.render_world.grass` and `frame.render_world.creatures` children.
`frame` is a real RAII span (not a rollup), so any unaccounted-for RAF
overhead shows up as `frame.total - sum(frame.*.total)`.

## Top-left status bar

The `#status` div is rewritten every RAF with the freshest snapshot
header. The format is:

```text
seed: <cachedSeed>  ·  tick <N>  ·  pop <N>  ·  <TPS> TPS  ·  <FPS> FPS[  (world ended)]
```

- **TPS** is rounded from `header.tps` (the worker's rolling-avg TPS
  written into the SAB header each snapshot). Shows `—` until the
  first positive non-NaN value lands.
- **FPS** is a "frames in the last 1 s" counter maintained entirely on
  main: `framesThisSecond++` each RAF; when
  `now - fpsWindowStart >= 1000`, sample → `lastFps`, reset counter
  and window. Shows `—` until the first 1 s window closes; naturally
  trends toward 0 while paused or world-ended without a misleading
  transient.

The `#perf-tps` readout inside the bottom-left perf widget is kept
deliberately as belt-and-suspenders — the always-visible top bar is
the discoverable surface, the perf widget is for heavy debugging.

## Per-creature instance pack

Inside `renderWorld`, the JS frustum cull walks `pop` creatures from the
Float32Array view in stride-8 chunks:

```text
for i in 0..pop:
  base = i * 8
  x  = creatures[base + 0]
  y  = creatures[base + 1]
  r  = creatures[base + 2]
  // cull if (x, y) ± (r + margin) is outside the camera viewport
  // emit one 8-float record into the instance scratch buffer:
  //   [x, y, radius_px, 0, color_r, color_g, color_b, alpha]
```

Then one `gl.drawArraysInstanced(TRIANGLE_STRIP, 0, 4, bodyCount)` call
covers every visible body. Bodies are filled discs; the `disc` fragment
shader switches to an annulus when `v_inner_px > 0`, which the
highlight pass uses.

## Highlight pass

Triggered when `highlightMap.size > 0` and `pop > 0`. Builds a
`Uint32Array` view over the same backing buffer as `creatures` and
reads `id_lo` / `id_hi` from each creature's 32-byte stride:

```ts
const idView = new Uint32Array(creatures.buffer, creatures.byteOffset,
                                pop * stride * (creatures.BYTES_PER_ELEMENT / 4));
const idLo = idView[i*8 + 6];
const idHi = idView[i*8 + 7];
const cid  = idHi * 4294967296 + idLo;       // exact in f64 up to 2^53
const exp  = highlightMap.get(cid);
if (exp === undefined || nowMs >= exp) continue;
// emit one ring instance: outer = radiusPx + 4, inner = radiusPx + 2
```

The pass is rare (≤ 1–2 rings/frame typically) so it does not need to
fit in the body-pack loop.

## Grass

R8 texture, `GRASS_GRID_DIM × GRASS_GRID_DIM` (960 × 960 = ~921 KB per
upload). Sub-image update each frame when the dim hasn't changed;
full image upload on first use or after a worker swap. The fragment
shader fades from white at `density=1.0` toward the dark clear color as
density → 0; `discard` at density=0; alpha scaled by
`settings.grassOpacity` so the user can dial grass back without
changing density semantics.

If `settings.showGrass === false` the upload + draw is skipped entirely
— at high cell counts this is a real perf win.

## Code anchors

- `web/src/render-gl.ts` → `renderWorld`, `renderWorldImpl`,
  `initRenderer`, `DISC_VS`/`DISC_FS`, `GRASS_VS`/`GRASS_FS`,
  `FRAME_VS`/`FRAME_FS`, `FLOATS_PER_INSTANCE`, `PERMANENT_EXP`,
  `HIGHLIGHT_FADE_MS`.
- `web/src/render.ts` → `Camera`, `PX_PER_SIZE`, `makeCamera`,
  `clampCamera`, `worldToScreen`.
- `web/src/camera.ts` → `attachCameraControls`.
- `web/src/main.ts` → the RAF `frame` callback and `spawnSimWorker`'s
  SAB binding.
- `web/src/rail/highlight.ts` → `highlights` map, TTL semantics.

## Update when

- The creature SoA stride changes (renderer's stride guard, instance
  pack reads, id extraction offsets all break).
- The grass texture format or dimension changes (the `gl.R8` /
  `gl.RED` / `gl.UNSIGNED_BYTE` triple here and the upload path).
- A new per-frame span is added (must nest under `frame` to avoid
  orphaning).
- The id encoding changes (currently raw u32 pair via `f32::from_bits`
  on the Rust side).
- The renderer starts copying the SAB instead of viewing it (the
  `frame.snapshot.read` span will inflate and the
  `grass.upload.read` ghost will reappear; both are noted in the
  decisions/render doc).
- The status-bar format string or its update cadence changes (currently
  every RAF, with TPS from `header.tps` and a 1 s rolling FPS counter
  on main).

## Why is it shaped this way

See [`decisions/render.md`](../decisions/render.md) — JS-side cull
(zero-copy from the SAB beats wasm boundary), single instanced draw
call for bodies, R8 grass texture (vs R32F), no-rollup span shape so
unaccounted RAF overhead is visible.

## See also

- [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md)
- [`worker-runtime.md`](worker-runtime.md)
- [`profiler.md`](profiler.md)
- [`../decisions/render.md`](../decisions/render.md)
- [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
