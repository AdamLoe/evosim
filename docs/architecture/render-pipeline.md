# Render pipeline

The WebGL2 renderer, the camera, and the per-frame SAB snapshot read.

## What it is

A single instanced WebGL2 draw call for every creature body (plus a
second instanced pass for the rare highlight rings), one full-screen
quad sampling an **R8 grass density** texture, one full-screen quad
sampling an **R8 no-grass-zone id** texture drawn UNDER the grass, and
triangle-rectangle map borders for every visible repeat tile. The render
loop runs on the main thread, owns no wasm handle, and reads the live
snapshot SAB slot directly via typed arrays — no copy, no postMessage.

Both the grass and biome textures are fixed **4096×4096 R8** allocations
(`GRASS_LOD_BUDGET_AXIS = 4096`, single-sourced in Rust via `gen_bindings` →
`web/src/generated/lod-constants.ts`). Each frame the renderer uploads only the
sim-chosen clipmap window (`win_w × win_h` bytes) into the `(0, 0)` corner
via `texSubImage2D`, then applies a UV transform (`u_uv_scale`/`u_uv_offset`)
to map the world-space quad into that window. The sim worker selects the
pyramid LOD level and window origin and publishes them in the snapshot header
(bytes [32..64)); the renderer reads them as `WindowMetadata` each frame. At
the default construction scale (`grass_dim = 480 < 4096`) the window equals
the full field and the upload is byte-identical to a full-field path.

**LOD selection in `write_snapshot`** happens in one of two paths keyed on
`grass_lod_step` (slider idx 63, live, default 0 = Auto):
- **Auto path** (`grass_lod_step = 0`): auto-formula `floor(log2(visible_span /
  4096))`, then `lod_bias` (slider idx 60) is subtracted for finer-detail nudging.
  `lod_bias` is clamped ≥ 0 and is active **only in Auto mode**. If the bias
  requests finer than the budget, the window clamps at `GRASS_LOD_BUDGET_AXIS`
  and **crops viewport edges** — a "nudge within budget."
- **Stepper path** (`grass_lod_step ≥ 1`): forces an absolute mip level
  (1→L0/Full, 2→L1/Half, 3→L2/Quarter, 4→L3/Eighth, 5→L4/Sixteenth). Overrides
  the auto formula AND `lod_bias` entirely. No restart required (LIVE setting).

The **`lod_bias`** slider (idx 60) is therefore an Auto-path-only internal knob
when the stepper is non-zero.

**Aspect-correct window height:** `vis_cells_y` is derived from `viewport_h`
independently via `visible_cell_span_y = visible_cell_span_x × (viewport_h /
viewport_w)`. The full-field invariant holds for `win_w` (horizontal) only; `win_h`
is correctly aspect-matched.

## What it owns

- The GL programs (`disc`, `grass`, `biome`, `frame`), VAOs, instance
  buffers, the fixed 4096² R8 grass texture, the fixed 4096² R8 biome
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
- The camera helpers (`makeCamera`, `clampCamera`, `visibleRepeatTiles`,
  `wrapWorldCoord`, `worldToScreen`, `PX_PER_SIZE`) and pointer-driven
  pan/zoom controls.
- The `grass_cell_size` parameter in the UV transform (`cellSizeL =
  grass_cell_size × 2^mipLevel`). This value comes from `boot_ready` so the
  renderer correctly handles non-default `grass_size` settings without
  hard-coding 5.0.
- The highlight-ring decode: per-frame extraction of stable creature ids
  from the SAB's interleaved id_lo/id_hi u32 pair via a `Uint32Array`
  view over the same backing buffer.

## What it does NOT own

- **The SAB layout or the snapshot byte stride** — owned by
  [`shared-memory-and-protocol.md`](shared-memory-and-protocol.md). The
  renderer reads `CREATURE_STRIDE` and the slot-offset helpers from
  `sim/bridge.ts`.
- **The sim tick rate or any sim mutation** — owned by
  [`worker-runtime.md`](worker-runtime.md). The render loop is main-thread
  RAF capped by the persisted App FPS setting.
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
  2. clamp the camera for the current Display repeat mode, write camera SAB
     lanes, then read `CTRL_SEQ`. When visual repeats are enabled, main writes
     the camera center folded into `[0, world_size)` so the worker's clipmap
     window tracks the base map while the renderer keeps the absolute camera
     center for tile drawing.
     If no new seq, repaint only for paused camera movement.
     If there is a new seq but the configured App FPS interval has not elapsed,
     skip snapshot read/render and do not ack the seq yet.
  3. open `frame.snapshot.read` span
     2a. seq  = Atomics.load(controlI32, CTRL_SEQ)
     2b. slot = Atomics.load(controlI32, CTRL_CURRENT_SLOT)
     2c. header = readSnapshotHeader(snapshotView, slotOffset(layout, slot))
     2d. creatures = new Float32Array(snapshotBuffer, creatureSoAOffset(layout, slot), pop * CREATURE_STRIDE)
     2e. grass    = new Uint8Array(snapshotBuffer, grassOffset(layout, slot),    layout.grassRegionBytes)
         biomeWin = new Uint8Array(snapshotBuffer, biomeWinOffset(layout, slot), layout.biomeWinBytes)
         // grass: u8 clipmap window, min(grass_dim,4096)² bytes per slot
         // biomeWin: mode-downsampled biome window, same allocation size
         // windowMeta = readWindowMetadata(snapshotView, slotBaseOff)
         // snapshotBuffer is wasm.memory.buffer (wasm linear memory, not a separate SAB)
  4. close `frame.snapshot.read`
  5. (auto-restart on world_ended if enabled)
  6. pollRail(rail, header, simBridge, creatures, pop)
  7. renderWorld(gl, cam, viewW, viewH, creatures, grass,
                 biomeWin, pop, worldSize, grassDim,
                 wrapWorld, highlights, windowMeta)
     // biomeWin is the per-slot windowed biome bytes from the snapshot
     // (mode-downsampled, same win_w×win_h as the grass window).
     // windowMeta carries mip_level, win_origin_x/y, win_w/h, tex dims,
     // wrap mode; consumed each frame to drive the UV transform + texSubImage2D.
  8. Atomics.store(CTRL_CONSUMED_SEQ, seq), update perf-panel status line
```

`renderWorld` opens `frame.render_world` and inside it
`frame.render_world.grass` and `frame.render_world.creatures` children;
the creature path also records `frame.render_world.creatures.trail_state`
around the id-keyed trail map rebuild.
`frame` is a real RAII span (not a rollup), so any unaccounted-for RAF
overhead shows up as `frame.total - sum(frame.*.total)`.

## Perf status line and App FPS

The bottom perf-panel status line is updated on every painted frame with
the freshest consumed snapshot header. The format is:

```text
seed: <cachedSeed>  ·  tick <N>  ·  pop <N>  ·  <TPS> TPS  ·  <FPS> FPS[  (world ended)]
```

- **TPS** is rounded from `header.tps` (the worker's rolling-avg TPS
  written into the SAB header each snapshot). Shows `—` until the
  first positive non-NaN value lands.
- **FPS** is the count of painted-frame timestamps in the trailing 1 s
  window, maintained entirely on main. It naturally follows the App FPS
  cap and can be lower when render work is slower than the selected cap.

The App FPS setting is a Display setting with exactly `15`, `30`, `60`,
and `120` choices (default `60`). It caps expensive snapshot read,
render, and upload work; the RAF callback still runs at browser cadence
to write camera lanes and handle paused camera repaint.

## Per-creature instance pack

Inside `renderWorld`, the JS frustum cull walks `pop` creatures from the
Float32Array view in stride-8 chunks. Creature instance lanes are decoded
per [architecture/shared-memory-and-protocol.md → Creature SoA](shared-memory-and-protocol.md);
the GL pack reads them in `render/gl.ts` → `renderWorldImpl`.

**Creature color source.** Lane 3 (`color_u32`) carries a packed RGBA8-LE
word. In single-pool mode (no species), this word is produced by
`lineage_color_u32(hue)` in `wasm_api/mod.rs`: the hue is an inherited
per-creature lineage field that mutates slightly at each split, so
descendants share a color family and lineage blooms are visible on canvas.
In species mode the same lane carries the species palette color (unchanged).
**The packed RGBA8-LE lane-3 format and the renderer's lane-3 read in
`render/gl.ts → renderWorldImpl` are both unchanged** — only the Rust
source that fills the word changed (from genome-derived to
lineage-hue-derived). No renderer change is needed or expected.

Then one `gl.drawArraysInstanced(TRIANGLE_STRIP, 0, 4, bodyCount)` call
covers every visible body instance, including repeated visual tile copies.
Bodies are
filled discs; the `disc` fragment shader switches to an annulus when
`v_inner_px > 0` (highlight pass), and to a **radial-shaded ~1px point** when
`v_inner_px < 0`.

### Repeat camera, survey points, tile-aware draw

- **Survey dots.** At survey zoom the pack emits radial-shaded dots with
  `inner_px = -1`. The formula is
  `max(creatureSurveyDotMinPx, rawRadiusPx × creatureSurveyDotScale)`,
  clamped in the renderer to the Settings ranges (`0.1..8px` and `1..32x`).
  Defaults (`1.0px`, `1.0x`) preserve the old 1px bottom-band behavior. From
  raw radius 1px to 4px, the rendered radius blends back to true body radius
  before the full body-disc branch takes over, so zooming through the
  point/disc boundary has no hard size jump. Action-tint blends the dot color
  toward the active flash action color when `flashTicks > 0`; bottom-band dots
  have no halo (`HALO_ALPHA_BOTTOM = 0.0`).
- **Frustum-derived visual tiles.** The Display setting `visualRepeats`
  controls repeated rendering independently of the sim-side `wrapWorld`
  physics setting. When it is on, `visibleRepeatTiles` derives every map tile
  offset intersecting the current camera frustum; there is no fixed 7x7 crop.
  When it is off, the tile list is only `{0,0}` and `clampCamera` keeps the
  center inside the runtime map.
- **Shared tile offsets.** Biome, grass, map borders, bodies, trails, action
  flashes, and inspector highlight rings all consume the same tile list.
  The terrain passes draw the same world quad per tile; creature/trail/ring
  passes add the tile offset to each instance. Trail streak suppression still
  keys off sim-side `wrap_world` so a wrapped physics step does not draw a
  full-world trail line.
- **Initial fit + min zoom.** `makeCamera` starts centered on the runtime map
  with one full map occupying about 80% of the limiting viewport axis. With
  visual repeats enabled, this leaves adjacent tiles visible at first paint.
  The live Display setting `maxZoomOutMaps` controls the zoom-out floor as the
  number of map widths visible on the limiting viewport axis (default 8,
  clamped 2..64). Repeat coverage still comes from the tile list; the slider
  only controls how far the camera may zoom out.
- **Repeat borders.** Map borders are drawn for every visible tile as four
  triangle rectangles, not WebGL line primitives, so pixel thickness is stable
  across browsers. Live Display settings control `repeatBorderMinPx`,
  `repeatBorderScale`, and `repeatBorderOpacity`; defaults (`1px`, `0x`,
  `0.25`) preserve the old thin outline.

## Creature appearance

The body fragment shader (`DISC_FS`) has three LOD-driven branches keyed on
the `inner_px` sentinel (packed as `v_radii_px.y`):

- **Annulus branch** (`inner_px > 0`): flat-fill for trait rings and highlight
  rings. These are UI elements, not creature bodies.
- **Shaded disc branch** (`inner_px = 0`, mid and near band): AA edge +
  radial shade + soft outline ring. The ring is **zoom-driven**: near zoom
  (≥ `NEAR_THRESHOLD_PX = 12 px`) → full 4%-wide ring (`0.96..1.0`); mid zoom
  (`MID_THRESHOLD_PX = 4 px` to `NEAR`) → slim ring (`0.993..1.0`); below mid →
  ring suppressed.
- **Radial-shaded dot branch** (`inner_px = -1`, bottom band: raw radius < 1 px):
  a radial gradient (`shade = mix(1.3, 0.6, r)`, bright centre -> dark edge),
  alpha hardcoded to 1.0, no AA disc and no outline ring. The JS pack chooses
  the dot radius from `creatureSurveyDotMinPx` and
  `creatureSurveyDotScale`, so faction territories stay tunable at survey
  zoom without changing the snapshot layout.

**Halo (per-instance LOD multiplier).** A separate `HALO_VS`/`HALO_FS` program
issues a second instanced draw against the SAME body instance buffer. Peak
halo strength is `readHaloStrength() × 0.6` (global theme alpha × 0.6 weakening).
Per-instance LOD band multiplier is encoded in `a_color.a` (= `v_color.a`):
- `HALO_ALPHA_NEAR = 1.0` — near band (full halo).
- `HALO_ALPHA_MID = 0.4` — mid band (40% strength; shrunk appearance).
- `HALO_ALPHA_BOTTOM = 0.0` — bottom band (no halo on radial-shaded dots).
The `HALO_FS` multiplies `v_color.a` into the final alpha, so bottom-band
instances produce zero output without a separate draw call per band. The
vertex shader scales the quad 2× around the body centre (4.0 corner multiplier in
NDC). Blend mode `(SRC_ALPHA, ONE)` during the draw. The rim-light Gaussian is
`exp(-(r - 0.55)² × 90)`, gated by `outside = smoothstep(0.48, 0.53, r)` and
`outerFade = 1 - smoothstep(0.80, 1.0, r)`, with glow color mixed 25% toward
`vec3(0.55, 0.75, 1.0)`. No pulse animation. Halo pass skipped at high pop
— see threshold in `render/gl.ts` → `renderWorldImpl` (`pop <= ...` guard).

**Action-tint (bottom-band dots).** When a creature in the bottom band has
`flashTicks > 0` (decoded from `packed_u32` lane 6 — no snapshot layout
change), the dot color is blended toward the flash tag's action color by
`tintStrength = (flashTicks / FLASH_TICKS_MAX) × 0.6`. Uses the existing
`flashTag`/`flashTicks` fields from the creature SoA's `packed_u32`; no additional
snapshot fields. The tint is applied only to bottom-band (radial-shaded dot)
instances; mid/near-band creatures express the flash via the separate ring-flash
annulus pass (unchanged).

**Draw order per frame:** grass → world-bounds frame → halo → trails →
bodies → highlight ring. Halo runs first so each body paints over its
own glow; trails run between halo and body so the body's leading edge
always covers the trail's lead-in pixel.

## Trail state

- `prevById: Map<id, {x, y}>` — positions at the previous snapshot.
- `currById: Map<id, {x, y}>` — positions at the current snapshot.

The renderer does **not** interpolate duplicate snapshot frames. Main
paints a snapshot once, stores `CTRL_CONSUMED_SEQ`, and waits for either
a new seq or a paused camera repaint. Inside `renderWorldImpl`, each
paint pointer-swaps `prevById` and `currById`, clears the new current map,
and walks the current SoA once to populate id → position entries. Bodies
render at their raw current positions.

The map rebuild is measured as
`frame.render_world.creatures.trail_state` so future optimization passes
can decide from profiler data whether the id-keyed map remains a scaling
problem. Both maps are reused across flips (clear, don't recreate) so the
heap does not churn.

### Velocity trails

A thin quad-as-line per moving creature draws from `prev` (one painted
snapshot ago) to the current body position. The fragment fades alpha
from 0 at the back to `v_color.a` at the front, so the body looks like
it's dragging a comet tail one snapshot long. Implementation: a
dedicated `TRAIL_VS` / `TRAIL_FS` program with its own VAO and
instance buffer (per-instance: start xy, end xy, color rgba, thickness
in px). The instance buffer is pre-allocated to the
`MAX_POP_FOR_SIM` ceiling so no per-frame growth. Thickness is
`max(2, radiusPx · 0.6)` per creature so big creatures get visible
tails and small ones stay legible at low zoom. Trails are skipped
when `dx² + dy² < 0.04` (stationary creatures) and the whole pass is
disabled at high pop — see threshold in `render/gl.ts` → `renderWorldImpl`
(`enableTrails` guard).

## Action ring-flash pass

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
far/bottom zoom (`< MID_THRESHOLD_PX = 4 px`) → ring suppressed (`inner == outer ==
1.0`); mid zoom (`MID` to `NEAR_THRESHOLD_PX = 12 px`) → slim ~0.7%-wide
ring (`0.993..1.0`); near zoom (`≥ NEAR`) → full 4%-wide ring
(`0.96..1.0`). Thresholds are `const` in `renderWorldImpl`.

**Flash tag colors and meanings** are owned by
[`architecture/simulation-core.md → Action ring-flash (FlashTag)`](simulation-core.md).

## Highlight pass

Triggered when `highlightMap.size > 0` and `pop > 0`. Builds a
`Uint32Array` view over the same backing buffer as `creatures` and
reads `id_lo` / `id_hi` from each creature's 32-byte stride; current
lane indices in `render/gl.ts` → `renderWorldImpl` (id_lo at lane 4,
id_hi at lane 5 — see [shared-memory-and-protocol.md → Creature SoA](shared-memory-and-protocol.md)
for the canonical layout). Emits one ring instance per match: outer =
radiusPx + 4, inner = radiusPx + 2, color (255, 200, 50) — the distinct
yellow selection ring.

The pass is rare (≤ 1–2 rings/frame typically) so it does not need to
fit in the body-pack loop.

## Grass and biome textures

Both the grass density and biome layers share a fixed **4096×4096 R8
texture** allocated once in `initRenderer` (`GRASS_LOD_BUDGET_AXIS = 4096`,
generated from Rust). The texture is never resized; only the window sub-region
changes per frame.

**Grass.** The snapshot grass region is a u8 clipmap window:
`min(grass_dim, 4096)²` bytes per slot, carrying exactly `win_w × win_h`
meaningful bytes at the sim-chosen LOD level. The renderer calls
`texSubImage2D` each painted frame to upload only the `win_w × win_h`
window into the `(0, 0)` corner of the 4096² texture (subarray-guarded to
guard against `UNPACK_ROW_LENGTH` latent footguns). `UNPACK_ALIGNMENT` is
set to 1 at init; R8 is a normalized unorm format so the shader's
`texture(...).r` returns 0..1 with no shader rescale.

**No-grass-zone layer.** The per-slot zone window (mode-downsampled, same `win_w × win_h`
as grass) is appended immediately after the grass region in the slot
(`biomeWinOffset`). The renderer uploads it with `texSubImage2D` only
when the upload key changes: mip/window origin and dimensions,
published texture dimensions, wrap mode, budget axis, or wasm-memory
buffer identity after restart. The zone grid is static, so identical
metadata means the already-uploaded texture bytes are still valid. The
texture uses `NEAREST` filtering so each cell renders as a flat
color. For no-grass-zone semantics (id-to-capacity mapping), see
[`architecture/biome.md`](biome.md).

**LOD level.** The sim's Rust worker computes the pyramid level and window
origin each tick (`write_snapshot` reads the camera SAB lanes and publishes
the window via `budget_axis = 4096`, margin 1.5×,
`level = floor(log2(visible_cell_span / budget_axis))`). After computing the
float level, `lod_bias` (clamped ≥ 0) is subtracted to allow finer-detail
nudging. At default construction scale (`world_size = 9600`,
`grass_cell_size = 20`, `grass_dim = 480`) the window equals the full field
and the upload is byte-identical to a full-field path. The LOD level is
carried in `windowMeta.mipLevel` (snapshot header
bytes [32..36)).

**UV transform.** Both grass and biome programs receive the same two uniforms
to map the world-space quad UV into the window sub-region of the 4096² texture.
`grass_cell_size` (from `boot_ready`) is used instead of a hard-coded 5.0:

```
u_uv_scale  = world_size / (grass_cell_size × 2^mipLevel × BUDGET_AXIS)
u_uv_offset = -vec2(winOriginX, winOriginY) / BUDGET_AXIS
```

At default scale (mip=0, origin=(0,0), win=480, grass_cell_size=20):
`scale = 9600 / (20 × 1 × 4096) ≈ 0.1171875` and `offset = (0,0)`.
Toroidal seam windows use signed logical `winOriginX/Y`; the Rust copy path
normalizes that origin modulo the level dimensions before uploading bytes.
`u_uv_offset` is **constant across all ghost copies** — only the camera
position is shifted per copy (see toroidal tiling below). `u_lod_blend` is
reserved for future trilinear blending and is always `0.0`.

**Toroidal tiling on zoom-out.** When `wrap_world` is on and the camera
frustum extends past a world edge, `renderWorldImpl` issues extra grass
and biome draw calls with a shifted camera position (the per-axis offset
set `{-W, 0, +W}` shared with creatures). Each ghost quad keeps the **same**
`u_uv_offset` as the primary draw: because every offset is exactly one world
width and the field is toroidally periodic, the same `a_corner → v_uv`
mapping samples the correct wrapped content one world over.

The one catch is the fixed-budget texture: the window data fills only the
`win_w × win_h` corner of the 4096² `CLAMP_TO_EDGE` texture, so a `v_uv` that
leaves the window would smear the edge texel. The fix is a per-axis wrap
period `u_uv_wrap` (`render/gl.ts → windowUvWrap`): when the window spans the
**full world on an axis** — exactly the case where ghost copies are drawn, so
the upload is a complete toroidal period — the fragment shader wraps the
sample UV with `mod(v_uv, win_dim / BUDGET_AXIS)` before every tap (crisp,
bicubic, and blur for grass; the single fetch for biome). Slice windows
(zoomed in, never tiled) pass `u_uv_wrap = 0` and keep the plain clamp path,
which is correct inside a single world view. Without this wrap, zone patches
and grass clamp into hard rectangular blocks at each world-tile boundary when
zoomed out and panned (the centered case happened to have `winOrigin = 0` and
hid the bug).

**Texture filters.** The **biome** texture uses `gl.NEAREST` (min + mag) so each
cell stays a flat color. The **grass** texture uses `gl.LINEAR` (min + mag) to feed
the grass shader's smoothing path. Every grass knob below is a **live Display
setting** whose default reproduces the original blocky look exactly (so a fresh
blob renders identically); the renderer reads `getSettings()` each frame. The
`GRASS_FS` pipeline is: **smooth the density intensities → ramp them → optionally
overlay procedural texture**.

*Smoothing (intensity filtering):*
- **`grassSmoothing`** ("Grass smoothing", 0..1) — blends crisp snapped-texel
  sampling (0, original) → a **16-tap Catmull-Rom bicubic** upscale (`texBicubic`):
  smooth curved gradients, no bilinear diamonds, not mushy.
- **`grassSoftness`** ("Grass softness", 0..1) — an extra 3×3 tent **blur**
  (`texBlur`) layered on top, radius scaling with the value, for a soft cloud-like
  field. 0 = none.

*Intensity → look ramp* (maps smoothed density `d` to alpha + color):
- **`grassDensityFloor`** (0..0.5) — density below this reads empty; the rest
  rescales up. `d' = clamp((d − floor)/(1 − floor))`.
- **`grassContrast`** (gamma, 0.3..3, def 1.0 = linear) — `d' = pow(d', contrast)`.
- **`grassBrightness`** (0.5..2, def 1.0) — peak green multiplier (lushness).
- Output: **alpha = d' × grassOpacity** (low density = faint, shows terrain),
  **color = grass-tint × grassBrightness**.

*Procedural texture overlay* (optional, **`grassTexture`** master, default 0 = off):
- **`grassEdgeErosion`** — coarse fbm multiplied into density (empties stay empty),
  eroding patch borders into ragged/clumpy shapes.
- **`grassShadeVariation`** — finer fbm varying blade brightness.
- **`grassBladeSize`** — fine-noise wavelength in **world units** (patch band 6×).

**The procedural noise is keyed on absolute world position** (`v_world`, passed
from `GRASS_VS`), **not** texture/clipmap coordinates. This is load-bearing:
keying on `v_uv × textureSize` made the pattern swim/jump while zooming because the
clipmap window origin and mip level shift per frame. World-space keying locks it to
the ground. (The underlying *density* still LOD-pops at mip switches — clipmap
design, separate from this overlay; pin "Grass resolution" / `grass_lod_step` to a
fixed level to avoid it.) Per-mip trilinear density blending is still reserved via
`u_lod_blend` (always `0.0`).

If `settings.showGrass === false` the grass upload + draw is skipped
entirely — at high cell counts this is a real perf win.

## No-grass-zone layer

A second full-screen world-quad drawn **UNDER the grass**, before it, so
grass density brightens green on top. It samples an **R8 zone-id texture**
with **NEAREST** filtering so each cell is a flat color. The biome data is
carried as a windowed clipmap in the per-slot biome window (mode-downsampled, same
`win_w × win_h` as the grass window); the renderer uploads it via
`texSubImage2D` when the biome metadata key or wasm-memory buffer changes,
using the same UV transform as grass. Zone tint therefore tracks the grass
window at every LOD level. For zone id-to-capacity mapping and semantics, see
[`architecture/biome.md`](biome.md).

The fragment alpha is the `u_opacity` uniform (the live **`biomeOpacity`**
"Zone opacity" Display setting, default 0.0). The zone quad is the bottom
layer drawn over the cleared canvas with the standard `SRC_ALPHA,
ONE_MINUS_SRC_ALPHA` blend; grass (with its own `grassOpacity`) still paints
on top. If the world was constructed with no-grass zones disabled, the
published zone texture is all plains, so opacity cannot reveal a disabled
dead-zone pattern.

### Grass shading

- Texture filter is `LINEAR` (min + mag); the `u_detail` path (`grassSmoothing`
  setting) fades from the snapped-texel (nearest, blocky) look at 0 toward
  smoothstep-interpolated density + procedural fbm grass texture at 1 (see
  "Texture filters" above). Per-mip trilinear blending across LOD levels is still
  separately reserved via `u_lod_blend` (always `0.0`).
- `u_grass_tint` (vec3) is parsed from the `--grass-tint` CSS var via
  `getComputedStyle` and cached against the previous string so we
  re-parse only on theme swap.
- `u_time` (float) = `performance.now() / 1000`. Drives a procedural
  sin-wave term. When the clipmap window is reused as a full-world repeat
  period, the wave and optional grass texture noise use tile-periodic UVs
  rather than raw world coordinates so repeated terrain does not show a
  shader-generated seam. The wave multiplies the sampled density before alpha
  output, so empty cells stay empty.

## Code anchors

- `render/gl.ts` → `renderWorld`, `renderWorldImpl`,
  `initRenderer`, `DISC_VS`/`DISC_FS`, `GRASS_VS`/`GRASS_FS`,
  `BIOME_VS`/`BIOME_FS`, `FRAME_VS`/`FRAME_FS`, `FLOATS_PER_INSTANCE`,
  `PERMANENT_EXP`, `HIGHLIGHT_FADE_MS`, `windowUvWrap`.
- `sim/bridge.ts` → `GRASS_LOD_BUDGET_AXIS`, `WindowMetadata`,
  `readWindowMetadata`, `makeSlotLayout`, `grassOffset`, `biomeWinOffset`,
  `SNAPSHOT_HEADER_BYTES`, `CTRL_CAMERA_CX_BITS`/`CTRL_CAMERA_CY_BITS`/
  `CTRL_CAMERA_ZOOM_BITS`/`CTRL_CAMERA_VIEWPORT_W`/`CTRL_CAMERA_VIEWPORT_H`.
- `render/scene.ts` → `Camera`, `PX_PER_SIZE`, `DEFAULT_MAX_ZOOM_OUT_MAPS`,
  `MAX_ZOOM`, `makeCamera`, `clampCamera`, `minCameraZoom`, `worldToScreen`.
- `render/camera.ts` → `attachCameraControls`.
- `web/src/main.ts` → the RAF `frame` callback, camera SAB lane writes,
  `getLatestWindowMetadata`, and `spawnSimWorker`'s SAB binding.
- `rail/highlight.ts` → `highlights` map, TTL semantics.

## Update when

- The creature SoA stride changes (renderer's stride guard, instance
  pack reads, id extraction offsets all break).
- The grass/biome texture format changes (the `gl.R8` / `gl.RED` /
  `gl.UNSIGNED_BYTE` triple and the upload path). The texture dimensions
  are fixed at `GRASS_LOD_BUDGET_AXIS²` and do not change per boot.
- The clipmap window metadata layout changes (snapshot header bytes
  [32..64); `readWindowMetadata`; the `u_uv_scale`/`u_uv_offset`/`u_uv_wrap`
  derivation in `renderWorldImpl` + `windowUvWrap`; the `GRASS_CELL_SIZE`
  constant; the camera SAB lanes at control-SAB slots **136–140**; or
  the consumed-seq ack lane at slot **141**).
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
- [`simulation-core.md`](simulation-core.md)
- [`biome.md`](biome.md)
- [`worker-runtime.md`](worker-runtime.md)
- [`profiler.md`](profiler.md)
- [`../decisions/render.md`](../decisions/render.md)
- [Agent-docs authoring rules](~/agent-docs/v1/rules/authoring-rules.md)
