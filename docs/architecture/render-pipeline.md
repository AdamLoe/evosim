# Render pipeline

The WebGL2 renderer, the camera, and the per-frame SAB snapshot read.

## What it is

A single instanced WebGL2 draw call for every creature body (plus a
second instanced pass for the rare highlight rings), one full-screen
quad sampling an **R8 grass density** texture, one full-screen quad
sampling an **R8 biome-id** texture drawn UNDER the grass, and one
`LINE_LOOP` for the world-bounds frame. The render loop runs on the main
thread, owns no wasm handle, and reads the live snapshot SAB slot directly
via typed arrays — no copy, no postMessage.

Both the grass and biome textures are fixed **2048×2048 R8** allocations
(`GRASS_LOD_BUDGET_AXIS = 2048`). Each frame the renderer uploads only the
sim-chosen clipmap window (`win_w × win_h` bytes) into the `(0, 0)` corner
via `texSubImage2D`, then applies a UV transform (`u_uv_scale`/`u_uv_offset`)
to map the world-space quad into that window. The sim worker selects the
pyramid LOD level and window origin and publishes them in the snapshot header
(bytes [32..64)); the renderer reads them as `WindowMetadata` each frame. At
the default scale (`grass_dim = 1920 < 2048`) the window equals the full
field and the upload is byte-identical to a full-field path.

## What it owns

- The GL programs (`disc`, `grass`, `biome`, `frame`), VAOs, instance
  buffers, the fixed 2048² R8 grass texture, the fixed 2048² R8 biome
  texture, and the instance scratch buffer.
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
     2a. seq  = Atomics.load(controlI32, CTRL_SEQ)
     2b. slot = Atomics.load(controlI32, CTRL_CURRENT_SLOT)
     2c. header = readSnapshotHeader(snapshotView, slotOffset(layout, slot))
     2d. creatures = new Float32Array(snapshotBuffer, creatureSoAOffset(layout, slot), pop * CREATURE_STRIDE)
     2e. grass    = new Uint8Array(snapshotBuffer, grassOffset(layout, slot),    layout.grassRegionBytes)
         biomeWin = new Uint8Array(snapshotBuffer, biomeWinOffset(layout, slot), layout.biomeWinBytes)
         // grass: u8 clipmap window, min(grass_dim,2048)² bytes per slot
         // biomeWin: mode-downsampled biome window, same allocation size
         // windowMeta = readWindowMetadata(snapshotView, slotBaseOff)
         // snapshotBuffer is wasm.memory.buffer (wasm linear memory, not a separate SAB)
  3. close `frame.snapshot.read`
  4. If seq === lastPaintedSeq AND camera unchanged → skip paint (seq-gated: FPS ≤ TPS).
     Exception: if the camera moved (pan/zoom while paused), repaint with the
     existing snapshot so the canvas reflects the new view without needing a new tick.
  5. (auto-restart on world_ended if enabled)
  6. pollRail(rail, header, simBridge, creatures, pop)
  7. renderWorld(gl, cam, viewW, viewH, creatures, grass,
                 biomeWin, pop, worldSize, grassDim,
                 wrapWorld, highlights, windowMeta)
     // biomeWin is the per-slot windowed biome bytes from the snapshot
     // (mode-downsampled, same win_w×win_h as the grass window).
     // windowMeta carries mip_level, win_origin_x/y, win_w/h, tex dims,
     // wrap mode; consumed each frame to drive the UV transform + texSubImage2D.
     // Also writes camera SAB lanes (cx/cy/zoom as f32-bits at slots 120-122;
     // viewport_w/h as u32 at slots 123-124) so the worker knows the view.
  8. update top-left status strip (world_seed) + perf-panel status line
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
Float32Array view in stride-8 chunks. **v2.0 Wave 2a/2b repacked the
creature lanes** (stride/byte-size unchanged: 8 lanes / 32 B):

```text
// SoA lane layout (Float32Array view, plus a Uint32Array view over the
// same backing buffer for the raw-u32 lanes 3/4/5/6):
//   [0] x (f32)   [1] y (f32)   [2] radius (f32, body_size-derived)
//   [3] color_u32 (RGBA8 LE, A=255)   [4] id_lo (u32)   [5] id_hi (u32)
//   [6] packed_u32   [7] pad
for i in 0..pop:
  base = i * 8
  x       = f32[base + 0]
  y       = f32[base + 1]
  radius  = f32[base + 2]                 // body_size-driven (genome 0.5..1.5)
  packed  = u32[base + 3]                 // display color, packed RGBA8
  r = (packed       ) & 0xFF;  g = (packed >> 8) & 0xFF;  b = (packed >> 16) & 0xFF
  idLo    = u32[base + 4];  idHi = u32[base + 5];  cid = idHi*2^32 + idLo
  flags   = u32[base + 6]                 // ring-flash + reserved species_id
  flashTag   =  flags        & 0x7        // FlashTag 0..5 (src/creature.rs)
  flashTicks = (flags >> 3)  & 0xF        // 0..FLASH_TICKS(5) countdown
  // v2.0 Wave 1c: for each wrap offset (see below):
  //   cull if (x+ox, y+oy) ± (r + margin) is outside the camera viewport
  //   emit one 8-float instance record into the instance scratch buffer:
  //     [x, y, radius_px, inner_px, r, g, b, alpha]
```

The `color_u32` lane **replaces** the old three `color_r/g/b` f32 lanes
(the action-EMA display color is gone). The Rust side now derives the
display color from the genome via an HSV mapping (hue ← diet, saturation
← body_size, value ← max_speed) and packs it RGBA8 little-endian with
A=255, so the old 125/255 brightness floor was dropped — creatures stay
legible by construction.

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
  center, 15% darker at the rim. Pure multiplication so the genome
  display color (v2.0 Wave 2a; was the action-EMA color) stays readable.
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

## Action ring-flash pass (v2.0 Wave 2b)

Drawn **on top of the bodies**, inside the body-pack loop. For each
creature with `flashTicks > 0` (decoded from `packed_u32` lane 6), the
pack emits a transient outer annulus instance (`outer = radiusPx + 5`,
`inner = radiusPx + 2`, the `inner_px > 0` annulus branch of `DISC_FS`)
in the flash tag's color, with the per-instance alpha set to
`flashTicks / FLASH_TICKS(5)` so the ring fades over its ~5-tick life.
The ring follows the same wrap-aware ghost-copy offsets as the body, so
a flashing creature near a seam rings at every visible ghost position.
The flash instances are accumulated into a **separate** `flashScratch`
buffer (the highlight pass below reuses the body `instanceScratch`, so a
shared buffer would clobber it) and drawn with a second
`drawArraysInstanced` after the body draw.

**Flash LOD guard.** The flash annulus is skipped when the creature's
raw screen radius is below `MID_THRESHOLD_PX` (4 px) — at that size the
annulus is not legible.

**Body-ring LOD.** The outline ring (`u_ring_inner` / `u_ring_outer`
uniforms) is a per-draw-call zoom-scaled uniform, not per-creature. A
representative radius (`1.0 × PX_PER_SIZE × zoom`) determines the tier:
far zoom (`< MID_THRESHOLD_PX`) → ring suppressed (`inner == outer ==
1.0`); mid zoom (`MID` to `NEAR_THRESHOLD_PX` = 12 px) → slim 1.5%-wide
ring (`0.985..1.0`); near zoom (`≥ NEAR`) → full 4%-wide ring
(`0.96..1.0`). Thresholds are `const` in `renderWorldImpl`.

**FlashTag → color legend** (RGBs owned TS-side in `render-gl.ts`,
matching the `FlashTag` enum order in `src/creature.rs`):

| tag | name | meaning | color |
|----:|------|---------|-------|
| 1 | Born | just born | teal |
| 2 | Grazed | grazed | green |
| 3 | Attacked | attacked | yellow |
| 4 | CreatedChild | created a child (split initiator) | blue |
| 5 | Killed | killed another creature | red |

Tag 0 (`None`) is never drawn. The action ring-flash is **distinct from
and coexists with** the inspector selection ring (the yellow highlight
pass below) — a selected, flashing creature shows both rings at once.

## Highlight pass

Triggered when `highlightMap.size > 0` and `pop > 0`. Builds a
`Uint32Array` view over the same backing buffer as `creatures` and
reads `id_lo` / `id_hi` from each creature's 32-byte stride. **v2.0
Wave 2b moved the id halves to lanes 4/5** (were 6/7):

```ts
const idView = new Uint32Array(creatures.buffer, creatures.byteOffset,
                                pop * stride * (creatures.BYTES_PER_ELEMENT / 4));
const idLo = idView[i*8 + 4];
const idHi = idView[i*8 + 5];
const cid  = idHi * 4294967296 + idLo;       // exact in f64 up to 2^53
const exp  = highlightMap.get(cid);
if (exp === undefined || nowMs >= exp) continue;
// emit one ring instance: outer = radiusPx + 4, inner = radiusPx + 2,
// color (255,200,50) — the distinct yellow selection ring.
```

The pass is rare (≤ 1–2 rings/frame typically) so it does not need to
fit in the body-pack loop.

## Grass and biome textures

Both the grass density and biome layers share a fixed **2048×2048 R8
texture** allocated once in `initRenderer` (`GRASS_LOD_BUDGET_AXIS = 2048`).
The texture is never resized; only the window sub-region changes per frame.

**Grass.** The snapshot grass region is a u8 clipmap window:
`min(grass_dim, 2048)²` bytes per slot, carrying exactly `win_w × win_h`
meaningful bytes at the sim-chosen LOD level. The renderer calls
`texSubImage2D` each painted frame to upload only the `win_w × win_h`
window into the `(0, 0)` corner of the 2048² texture (subarray-guarded to
guard against `UNPACK_ROW_LENGTH` latent footguns). `UNPACK_ALIGNMENT` is
set to 1 at init; R8 is a normalized unorm format so the shader's
`texture(...).r` returns 0..1 with no shader rescale.

**Biome.** The per-slot biome window (mode-downsampled, same `win_w × win_h`
as grass) is appended immediately after the grass region in the slot
(`biomeWinOffset`). It is uploaded the same way — `texSubImage2D` of
`win_w × win_h` bytes into the `(0, 0)` corner of its own 2048² R8 texture
— every frame. The biome texture uses `NEAREST` filtering so each cell
renders as a flat color.

**LOD level.** The sim's Rust worker computes the pyramid level and window
origin each tick (`write_snapshot` reads the camera SAB lanes and publishes
the window via `budget_axis = 2048`, margin 1.5×,
`level = floor(log2(visible_cell_span / budget_axis))`). At default scale
(`grass_dim = 1920`) the window equals the full field and the upload is
byte-identical to a full-field path (1920² bytes ≈ 3.7 MB). The LOD level
is carried in `windowMeta.mipLevel` (snapshot header bytes [32..36)).

**UV transform.** Both grass and biome programs receive the same two uniforms
to map the world-space quad UV into the window sub-region of the 2048² texture:

```
u_uv_scale  = world_size / (GRASS_CELL_SIZE × 2^mipLevel × BUDGET_AXIS)
u_uv_offset = -vec2(winOriginX, winOriginY) / BUDGET_AXIS
```

At default scale (mip=0, origin=(0,0), win=1920):
`scale = 9600 / (5 × 1 × 2048) ≈ 0.9375` and `offset = (0,0)`,
giving `v_uv ∈ [0, 0.9375]` — exactly the 1920 data pixels in the 2048
texture. This collapses to the identity mapping at default scale.
`u_lod_blend` is reserved for future trilinear blending and is always `0.0`.

**Toroidal tiling on zoom-out.** When `wrap_world` is on and the camera
frustum extends past a world edge, `renderWorldImpl` issues extra grass
and biome draw calls with a shifted camera position (the per-axis offset
set `{-W, 0, +W}` shared with creatures), so both layers tile seamlessly.

**NEAREST mip filter.** Both textures use `gl.NEAREST` for min and mag
filtering. Trilinear blending is reserved for a future pass (one
`texParameteri` change enables it when `u_lod_blend` is wired).

> **Known limitation — toroidal wrap seam.** A clipmap window straddling
> the world wrap boundary (grass_dim > 2048, non-default zoom, toroidal
> world) shows the wrong grass/biome region because the window is a
> rectangular crop that cannot span the seam. This is a documented TODO in
> the code; it does not affect the default (grass_dim=1920) configuration.

If `settings.showGrass === false` the grass upload + draw is skipped
entirely — at high cell counts this is a real perf win.

## Biome layer

A second full-screen world-quad drawn **UNDER the grass**, before it, so
grass density brightens green on top. It samples an **R8 biome-id texture**
(one u8 `Biome` tag per cell: 0=Plains, 1=Water, 2=Desert) with **NEAREST**
filtering so each cell is a flat color. The biome data is carried as a
windowed clipmap in the per-slot biome window (mode-downsampled, same
`win_w × win_h` as the grass window); the renderer uploads it via
`texSubImage2D` every painted frame using the same UV transform as grass —
biome tint therefore tracks the grass window at every LOD level. The
fragment shader recovers the integer tag (`floor(.r * 255 + 0.5)`) and maps
it to a flat color:

| Biome  | id | color (rgb)        |
|--------|----|--------------------|
| Plains | 0  | `74, 106, 58`      |
| Water  | 1  | `40, 92, 168`      |
| Desert | 2  | `196, 178, 128`    |

These are **visual-identity defaults** (Wave 1c) — adjust in `BIOME_FS`'s
`u_plains` / `u_water` / `u_desert` uniforms.

### Grass shading

- Texture filter is `NEAREST` (min + mag); trilinear blending is reserved
  via the `u_lod_blend` uniform (always `0.0`) and can be enabled later by
  changing one `texParameteri` call.
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
  `GRASS_CELL_SIZE`, the wrap-offset ghost-copy loop, `PERMANENT_EXP`,
  `HIGHLIGHT_FADE_MS`.
- `web/src/sim-bridge.ts` → `GRASS_LOD_BUDGET_AXIS`, `WindowMetadata`,
  `readWindowMetadata`, `makeSlotLayout`, `grassOffset`, `biomeWinOffset`,
  `SNAPSHOT_HEADER_BYTES`, `CTRL_CAMERA_CX_BITS`/`CTRL_CAMERA_CY_BITS`/
  `CTRL_CAMERA_ZOOM_BITS`/`CTRL_CAMERA_VIEWPORT_W`/`CTRL_CAMERA_VIEWPORT_H`.
- `web/src/render.ts` → `Camera`, `PX_PER_SIZE`, `MIN_ZOOM`/`MAX_ZOOM`,
  `makeCamera`, `clampCamera`, `worldToScreen`.
- `web/src/camera.ts` → `attachCameraControls`.
- `web/src/main.ts` → the RAF `frame` callback, camera SAB lane writes,
  `getLatestWindowMetadata`, and `spawnSimWorker`'s SAB binding.
- `web/src/rail/highlight.ts` → `highlights` map, TTL semantics.

## Update when

- The creature SoA stride changes (renderer's stride guard, instance
  pack reads, id extraction offsets all break).
- The grass/biome texture format changes (the `gl.R8` / `gl.RED` /
  `gl.UNSIGNED_BYTE` triple and the upload path). The texture dimensions
  are now fixed at `GRASS_LOD_BUDGET_AXIS²` and do not change per boot.
- The clipmap window metadata layout changes (snapshot header bytes
  [32..64); `readWindowMetadata`; the `u_uv_scale`/`u_uv_offset` derivation
  in `renderWorldImpl`; the `GRASS_CELL_SIZE` constant; or the camera SAB
  lanes at control-SAB slots 120–124).
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
