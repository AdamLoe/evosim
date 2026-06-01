# Render pipeline

The WebGL2 renderer, the camera, and the per-frame SAB snapshot read.

## What it is

A single instanced WebGL2 draw call for every creature body (plus a
second instanced pass for the rare highlight rings), one full-screen
quad sampling an **R8 grass density** texture, one full-screen quad
sampling an **R8 biome-id** texture drawn UNDER the grass (v2.0 Wave 1b),
and one `LINE_LOOP` for the world-bounds frame. The render loop runs on
the main thread, owns no wasm handle, and reads the live snapshot SAB
slot directly via typed arrays — no copy, no postMessage.

> **v2.0 Wave 1a:** the world is runtime-sized (default 9600u → a 1920²
> grass grid). The grass + biome texture dimensions and every snapshot
> view length derive from the `grass_dim` reported in `boot_ready`, not
> from a compile-time constant. The snapshot grass region is **u8**
> (`grass_dim²` bytes, quantized Rust-side), uploaded as `gl.R8`.

## What it owns

- The GL programs (`disc`, `grass`, `biome`, `frame`), VAOs, instance
  buffers, the R8 grass texture, the **static R8 biome texture**, and the
  instance scratch buffer.
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
  1. early-return if SAB handles aren't bound yet OR slotLayout is null
     (e.g., during restart). `slotLayout` is built once per boot from the
     boot-time `grass_dim` (see shared-memory-and-protocol.md).
  2. open `frame.snapshot.read` span
     2a. slot = Atomics.load(controlI32, CTRL_CURRENT_SLOT)
     2b. header = readSnapshotHeader(snapshotView, slotOffset(layout, slot))
     2c. creatures = new Float32Array(snapshotSab, creatureSoAOffset(layout, slot), pop * CREATURE_STRIDE)
     2d. grass     = new Uint8Array(snapshotSab, grassOffset(layout, slot), layout.grassCellCount)
         // u8, one byte per cell; length = grass_dim² (NOT a constant)
  3. close `frame.snapshot.read`
  4. (auto-restart on world_ended if enabled)
  5. pollRail(rail, header, simBridge, creatures, pop)
  6. renderWorld(gl, cam, viewW, viewH, creatures, grass,
                 biomeView, biomeDirty, pop, worldSize, grassDim,
                 wrapWorld, highlights)
     // biomeView is a static Uint8Array over wasm memory at
     // biome_buf_byte_offset; biomeDirty is true only on a worker swap so
     // the biome texture re-uploads once, not per frame.
  7. update top-left status strip (world_seed) + perf-panel status line
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
  // v2.0 Wave 1c: for each wrap offset (see below):
  //   cull if (x+ox, y+oy) ± (r + margin) is outside the camera viewport
  //   emit one 8-float record into the instance scratch buffer:
  //     [x, y, radius_px, inner_px, color_r, color_g, color_b, alpha]
```

Then one `gl.drawArraysInstanced(TRIANGLE_STRIP, 0, 4, bodyCount)` call
covers every visible body (including wrapped ghost copies). Bodies are
filled discs; the `disc` fragment shader switches to an annulus when
`v_inner_px > 0` (highlight pass), and to a **flat ~1px point** when
`v_inner_px < 0`.

### v2.0 Wave 1c: survey camera, sub-pixel points, wrap-aware draw

- **Sub-pixel → flat point.** At survey zoom a body's on-screen radius
  drops below 1px. When `radiusWorld × PX_PER_SIZE × zoom < 1` the pack
  emits `radiusPx = 1` and the `inner_px = -1` sentinel; the `disc` FS
  short-circuits to a flat-color fill (no AA / shading / ring) so faction
  territory color stays legible when zoomed all the way out.
- **Wrap-aware cull + draw.** When `wrap_world` is on and the camera
  frustum straddles a seam (`minX < 0`, `maxX > world_size`, and the y
  equivalents), the pack builds the small per-axis offset set
  `{-W, 0, +W}` and emits each in-frustum ghost copy. A creature near
  `x = 0` thus also renders at `x = world_size + δ`. The trail pass uses
  the same offsets and skips any segment whose displacement exceeds
  `world_size / 2` (a wrapped step) so a seam crossing doesn't streak the
  whole world. The highlight-ring pass mirrors the same offset logic.
- **Min-zoom + default zoom.** `clampCamera` floors zoom at `MIN_ZOOM =
  0.04` (px/world-unit) so a ~384px viewport frames the entire 9600u
  world from one view; `makeCamera` opens at `zoom = 1.0` (≈1500u across
  a typical ~1500px viewport), centered on `world_size / 2`.

## Creature appearance (v1.9.2)

The body fragment shader (`DISC_FS`) keeps its annulus shortcut for
trait rings and the highlight ring — those produce a flat fill (UI
elements, not creatures). The filled-body branch (when `inner_px = 0`)
runs the new shading:

- **AA edge.** `alpha = 1 - smoothstep(1 - aa, 1, r)` against a per-
  fragment `aa = fwidth(r) * 1.5`. Soft at low zoom, crisp at high.
- **Radial shade.** `shade = mix(1.15, 0.85, r)` — 15% brighter at
  center, 15% darker at the rim. Pure multiplication so action-EMA
  color identity stays readable.
- **Outline ring.** Two `smoothstep`s carve a soft annulus between
  `u_ring_inner = 0.84` and `u_ring_outer = 0.92`. Stops are constants
  set once at link time (not theme-owned — thickness doesn't need to
  be themed). The color is theme-driven via the `--creature-ring`
  rgba CSS var.

A separate halo program (`HALO_VS` / `HALO_FS`) issues a second
instanced draw against the SAME body instance buffer through its own
VAO. The vertex shader scales the quad by 2.0 around the body center
(a 4.0 corner multiplier in NDC space). Blend mode is
`(SRC_ALPHA, ONE)` during the draw and restored to
`(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)` immediately after.

The fragment shader is a **rim-light**, not a cloud: a narrow Gaussian
band `exp(-(r - 0.55)² · 90)` centered just outside the body silhouette
(body occupies r ∈ [0, 0.5] of the 2×-scaled quad), gated by an
`outside = smoothstep(0.48, 0.53, r)` term so the glow doesn't fill the
body interior, and an `outerFade = 1 - smoothstep(0.80, 1.0, r)` term
so the discard at r = 1 never reads as a hard ring. The glow color is
the creature color mixed 25% toward `vec3(0.55, 0.75, 1.0)` so it
reads as bioluminescent rather than just translucent body color.
There is no pulse animation — at high pop it presented as flicker.
Per-frame alpha (peak rim brightness) comes from the
`--creature-halo` rgba CSS var. The pass is skipped entirely when
`pop > 8000` (perf hygiene).

**Draw order per frame:** grass → world-bounds frame → halo → trails →
bodies → highlight ring. Halo runs first so each body paints over its
own glow; trails run between halo and body so the body's leading edge
always covers the trail's lead-in pixel.

## Snapshot interpolation (v1.9.2)

The renderer interpolates body positions between sim snapshot flips so
motion is smooth at any TPS. State is module-level in `render-gl.ts`:

- `prevById: Map<id, {x, y}>` — positions at the previous snapshot.
- `currById: Map<id, {x, y}>` — positions at the current snapshot.
- `flipTimeMs: number` — `performance.now()` when the seq counter last
  bumped.
- `lastSeenSeq: number` — for flip detection. Starts at `-1`.

Per RAF: main reads `seq = Atomics.load(controlI32, CTRL_SEQ)` and the
configured `targetTPS`, both passed to `renderWorld`. Inside
`renderWorld`:

1. If `seq !== lastSeenSeq`: pointer-swap `prevById ↔ currById`, clear
   the new curr, then walk the SoA once to populate it (id-decode +
   `Map.set`). On the very first flip (`lastSeenSeq === -1`), also
   clear prev so the first frame after boot renders curr verbatim
   instead of lerping from zero.
2. `expectedIntervalMs = clamp(1000 / targetTPS, 16, 250)`. The lower
   bound keeps a `targetTPS = 1000` setting from collapsing alpha to
   zero between RAFs; the upper bound keeps very low TPS from gliding
   for seconds at a time.
3. `alpha = clamp((now - flipTimeMs) / expectedIntervalMs, 0, 1)`.
4. Per creature: `prev = prevById.get(id)`. If present, render at
   `lerp(prev, curr, alpha)`; else render at curr (newborn). Dead
   creatures drop automatically because the loop iterates the current
   SoA.

Cost: the per-flip Map rebuild is ~1–2 ms at 32 k entries; the per-RAF
`Map.get` lookup is ~0.5 ms at 32 k. Both maps are reused across flips
(clear, don't recreate) so the heap doesn't churn.

`resetInterpolation()` is exported from `render-gl.ts` and called by
`main.ts → restart()` after `resetStats()`. It clears both maps,
resets `lastSeenSeq` to `-1`, and stamps `flipTimeMs = now()` so the
first post-restart frame doesn't lerp from positions in the dead
worker's last snapshot.

### Velocity trails

A thin quad-as-line per moving creature draws from `prev` (one full
tick ago) to the current interpolated body position. The fragment
fades alpha from 0 at the back to `v_color.a` at the front, so the
body looks like it's dragging a comet tail one tick long. The earlier
`lerp(prev, curr, alpha - 0.4)` start point was dropped — at any
plausible TPS the resulting segment was sub-pixel. Implementation: a
dedicated `TRAIL_VS` / `TRAIL_FS` program with its own VAO and
instance buffer (per-instance: start xy, end xy, color rgba, thickness
in px). The instance buffer is pre-allocated to the
`MAX_POP_FOR_SIM` ceiling so no per-frame growth. Thickness is
`max(2, radiusPx · 0.6)` per creature so big creatures get visible
tails and small ones stay legible at low zoom. Trails are skipped
when `dx² + dy² < 0.04` (stationary creatures) and the whole pass is
disabled when `pop > 12000`.

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

## Grass (v2.0 Wave 1a)

**R8** texture, `grass_dim × grass_dim` (1920 × 1920 ≈ 3.7 MB per upload
at the 9600u default). The texture dimension is the runtime `grass_dim`
from `boot_ready`, compared against a cached `grassTexDim`: a **full
`texImage2D`** runs on first use / dim change, a `texSubImage2D` on every
subsequent painted frame. The snapshot grass region is u8 (quantized
Rust-side in `quantize_grass_into`, density/`GRASS_MAX` → 0..255); R8 is a
normalized unorm format so the shader's `texture(...).r` returns 0..1
directly — no float-linear extension, no shader rescale.

> **Full upload every painted frame.** Dirty-rect upload is deliberately
> deferred (v2.0). At 1920² this is a ~3.7 MB `texSubImage2D` per paint —
> the obvious next perf lever if a larger world is targeted.

`UNPACK_ALIGNMENT` is set to 1 at init (each R8 row is `dim` bytes, not a
multiple of 4). The fragment shader tints the field a theme-driven green
(`var(--grass-tint)` parsed as a vec3 uniform), with alpha tracking the
sampled density so under-blend leaves the layer beneath (biome / clear
color) showing through where grass is thin; `discard` at density=0 stays
for the empty-cell fast path. The overall alpha is scaled by
`settings.grassOpacity`.

If `settings.showGrass === false` the upload + draw is skipped entirely
— at high cell counts this is a real perf win.

## Biome layer (v2.0 Wave 1b)

A second full-screen world-quad drawn **UNDER the grass**, before it, so
grass density brightens green on top. It samples a **static R8 biome-id
texture** (`grass_dim × grass_dim`, one u8 `Biome` tag per grass cell:
0=Plains, 1=Water, 2=Desert) with **NEAREST** filtering so each cell is a
flat color. The biome buffer is a `Uint8Array` over wasm linear memory at
`biome_buf_byte_offset` (`grass_dim²` bytes); the renderer uploads it
**once per worker swap** — `main.ts` passes a `biomeDirty` flag that is
true only on boot/restart, so the static texture is not re-uploaded per
frame. The fragment shader recovers the integer tag
(`floor(.r * 255 + 0.5)`) and maps it to a flat color:

| Biome  | id | color (rgb)        |
|--------|----|--------------------|
| Plains | 0  | `74, 106, 58`      |
| Water  | 1  | `40, 92, 168`      |
| Desert | 2  | `196, 178, 128`    |

These are **visual-identity defaults** (Wave 1c) — adjust in `BIOME_FS`'s
`u_plains` / `u_water` / `u_desert` uniforms.

### Grass shading (v1.9.2)

- Texture filter is plain `LINEAR` (min + mag); no mipmapping (R8 unorm,
  full re-upload each frame).
- `u_grass_tint` (vec3) is parsed from the `--grass-tint` CSS var via
  `getComputedStyle` and cached against the previous string so we
  re-parse only on theme swap.
- `u_time` (float) = `performance.now() / 1000`. Drives a procedural
  sin-wave term `wave = 0.08 * sin(uv.x * 18 + t * 0.6) + 0.06 *
  sin(uv.y * 22 - t * 0.4)`. The wave multiplies the sampled density
  before the alpha output (`d2 = clamp(d + wave * d, 0, 1)`) so empty
  cells stay empty — the discard early-out and the multiplication
  guarantee the wave term can't paint outside grass.

## Code anchors

- `web/src/render-gl.ts` → `renderWorld`, `renderWorldImpl`,
  `initRenderer`, `DISC_VS`/`DISC_FS`, `GRASS_VS`/`GRASS_FS`,
  `BIOME_VS`/`BIOME_FS`, `FRAME_VS`/`FRAME_FS`, `FLOATS_PER_INSTANCE`,
  the wrap-offset ghost-copy loop, `PERMANENT_EXP`, `HIGHLIGHT_FADE_MS`.
- `web/src/render.ts` → `Camera`, `PX_PER_SIZE`, `MIN_ZOOM`/`MAX_ZOOM`,
  `makeCamera`, `clampCamera`, `worldToScreen`.
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
