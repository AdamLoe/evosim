# Decisions — render

---

### One WebGL2 instanced draw call for all bodies

- **Decision**: Every visible creature body packs into one
  `Float32Array` and renders via a single
  `gl.drawArraysInstanced(TRIANGLE_STRIP, 0, 4, bodyCount)`. The
  fragment shader does the disc vs annulus distinction.
- **Why**: Per-`arc` Canvas2D overhead dominated frames at pop > 1500.
  Instancing collapses N draws to 1. The same program handles
  highlight rings via a separate instance set (inner > 0 → annulus).
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render-gl.ts → renderWorld`,
  `DISC_VS`, `DISC_FS`, `FLOATS_PER_INSTANCE`.

### JS-side frustum cull and instance pack (zero wasm crossing)

- **Decision**: The renderer walks the SAB-backed `Float32Array` view
  over the creature SoA directly. There is no Rust-side
  `pack_render_buffer` export. Off-screen creatures are skipped before
  they touch the GPU buffer.
- **Why**: The SAB view is a contiguous typed-array load — already
  zero-copy from wasm memory. Going back through wasm to pack would
  reintroduce a wasm-bindgen boundary for no gain. The cull halves the
  GPU upload at typical zooms.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render-gl.ts → renderWorldImpl`
  (the body-pack loop, the `frame.render_world.creatures` span).

### Grass texture is R8, not R32F

- **Decision**: Grass density quantizes to `u8` Rust-side
  (`(d * 255).clamp(0,255) as u8`) and uploads as `gl.R8` /
  `gl.UNSIGNED_BYTE`.
- **Why**: One byte per cell instead of four. At the v2.0 default
  (1920² runtime grid) that's a ~3.7 MB `texImage2D`/`texSubImage2D`
  instead of ~14.7 MB. Quantization loses ~8 bits of density precision;
  visually indistinguishable, well under any visual-fidelity threshold.
- **Tradeoffs**: A future per-cell-derivative use would want more bits;
  if that lands, the grass texture format becomes a per-consumer
  decision rather than a global one.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render-gl.ts → renderWorldImpl`
  (the `gl.R8` upload), `src/wasm_api.rs → write_snapshot_to`
  (the quantize loop).

### Renderer reads the SAB directly; no postMessage snapshot path

- **Decision**: Per RAF, the renderer reads the live slot via
  `Atomics.load(controlI32, CTRL_CURRENT_SLOT)` and constructs typed
  arrays over the SAB. No structured-clone, no copy.
- **Why**: Structured-clone cost at pop > 4000 dominated frame time on
  the previous postMessage path. SAB views are O(1) per frame.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/main.ts` (the `frame` body),
  `web/src/sim-bridge.ts → slotOffset` / `creatureSoAOffset` /
  `grassOffset`.

### Outer `frame` span is a real RAII, not a rollup

- **Decision**: `span("frame")` at the top of the RAF callback
  attaches to the pre-seeded TS-side `frame` root and brackets the
  whole body. Children include `frame.snapshot.read` and
  `frame.render_world`.
- **Why**: `frame.total - sum(frame.*.total)` surfaces unaccounted-for
  RAF overhead (browser composite, event dispatch, GC) as a positive
  number, instead of hiding it in a rollup. That delta is the
  diagnostic value the no-rollup rule (see
  [`profiler.md`](profiler.md)) buys.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/profiler.md`.
- **Code anchors**: `web/src/perf.ts → span` (the empty-stack `frame`
  special-case), `web/src/main.ts` (the outer span), `web/src/render-gl.ts`
  (the inner spans).

### Highlight rings decode ids from the SAB id-pair view, not from JSON

- **Decision**: Per frame, build a `Uint32Array` view over the same
  backing buffer as `creatures`, read `id_lo` / `id_hi` from each 32-byte
  stride (v2.0 Wave 2b: lanes 4 / 5, byte offset `+16` / `+20`, after the
  render-only repack — were `+24` / `+28`), reassemble via
  `idHi * 2^32 + idLo`.
- **Why**: Highlight map lookups need the stable creature id every
  frame; round-tripping through a JSON inspector reply would be
  unacceptable. Reading two u32s straight from the same buffer is
  free.
- **Tradeoffs**: ids must round-trip through `f64` (no native u64 in
  JS). The mantissa is exact up to `2^53` — well above any session's
  id count.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render-gl.ts → renderWorldImpl`
  (the highlight pass with the `idView` build), `web/src/rail/highlight.ts`.

### Status bar updates every RAF; TPS + FPS rendered into `#status`

- **Decision**: The top-left `#status` text rewrites unconditionally on
  every RAF as
  `seed: … · tick N · pop N · NN TPS · NN FPS[(world ended)]`. TPS
  comes straight from `header.tps` in the per-frame SAB snapshot; FPS
  is a 1 s rolling main-side counter (`framesThisSecond++` each RAF,
  sampled and reset when `now - fpsWindowStart >= 1000`). FPS shows
  `—` until the first 1 s window closes.
- **Why**: Reading the tick from `header.tick` is a 4-byte
  typed-array load — there is nothing to throttle. The canvas already
  paints at full RAF cadence; throttling only the text was the lone
  reason the tick counter visibly jumped. TPS belongs on the
  always-visible bar because the bottom-left perf widget can scroll
  off-screen; FPS belongs on main because the worker can't observe
  RAF cadence. A "frames in the last 1 s" counter (not a rolling
  average) matches the conventional games-industry meaning and lets
  FPS trend naturally to 0 while paused or world-ended without a
  misleading transient.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/worker-runtime.md`.
- **Tradeoffs**: Writing `textContent` every RAF is essentially free
  on a single DOM node with no layout impact; the throttle saved
  nothing real. `#perf-tps` in the bottom-left perf widget stays as
  belt-and-suspenders for heavy debugging.
- **Code anchors**: `web/src/main.ts → frame` (the RAF loop,
  `framesThisSecond` / `fpsWindowStart` / `lastFps` closure state),
  `web/src/sim-bridge.ts → readSnapshotHeader` (`header.tps`).

### Zoom-out is the v2.0 survey tool: `MIN_ZOOM = 0.04` + min-1px survey points

- **Decision**: Camera zoom clamps to `[MIN_ZOOM = 0.04, MAX_ZOOM]`; center
  clamps to the runtime `[0, world_size]`. Below ~1px on-screen radius a body
  is drawn as a **flat min-1px point** in its display color (the `inner_px =
  -1` sentinel short-circuits `DISC_FS` past disc shading / AA / ring) instead
  of vanishing. v2.0 lowered the floor from the old `0.5` clamp (tuned for the
  1200u world) so the whole runtime-sized world frames from one view.
- **Why**: With editable/large worlds (default 9600u) you need *some* way to
  survey the whole map and see *where the factions are*, not just terrain. A
  minimap / species heatmap is the right long-term answer (v2.1+); zoom-out +
  1px points is nearly free and good enough for v2.0. The old `0.5` floor
  existed because below it the 1200u world turned to noise — a non-issue once
  the point is a bird's-eye survey and bodies clamp to a visible minimum.
  Center clamp still prevents pan-into-the-void.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render.ts → clampCamera`, `MIN_ZOOM` / `MAX_ZOOM`,
  `PX_PER_SIZE`; `web/src/render-gl.ts` (the point-size floor + `inner_px = -1`
  branch).
- **Revisit when**: a real minimap / species heatmap lands and a hard zoom-out
  floor is no longer the survey mechanism (see `v2-possible-next-steps.md`).

### Snapshot carries render-only fields, packed compactly — stride stays 32 B (v2.0)

- **Decision**: The per-creature `Genome` lives in `CreatureSoA` sim-side and
  does **not** ride the snapshot. The snapshot carries only what the renderer
  reads: position, body radius, a packed display color (`color_u32`), the
  stable id (`id_lo`/`id_hi`), the action ring-flash + `species_id`
  (`packed_u32`). The stride does **not** grow — the 7 used lanes pad back to
  the existing 8 (`CREATURE_STRIDE = 8`, 32 B); only the grass + biome regions
  became settings-derived.
- **Why**: The snapshot is a render feed, not a state dump. The inspector
  already reads full genome via the separate `creature_inspect_json` path, so
  traits never need to cross in the hot per-tick SoA. Packing the single-pool
  color into one RGBA8 lane and bit-packing ring-flash + `species_id` keeps the
  stride bump to zero, which keeps both the per-tick snapshot write and the
  per-flip interpolation Map rebuild cheap. An earlier draft feared a 32 B →
  64 B stride doubling from putting genome in the snapshot; render-only + packed
  avoids it.
- **Applies to**: `architecture/shared-memory-and-protocol.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `src/wasm_api.rs → write_snapshot_to`, `pack_render_u32`,
  `genome_color_u32`; `web/src/render-gl.ts` (instance pack + id/ring decode).
- **Tradeoffs**: Packed encodings are less human-readable in a memory dump; the
  decode lives in the renderer. Worth it for bandwidth + a stable stride guard.

### Action-EMA color deleted; body color = genome hue (single-pool) or species color (mating) (v2.0)

- **Decision**: The per-creature R/G/B action-EMA color (`color_r/g/b` SoA
  columns + the 3 f32 snapshot lanes) is **removed**. Steady body color is now
  *identity*: single-pool derives it from the genome via HSV (hue ← `diet`,
  saturation ← `body_size`, value ← `max_speed`); species mode paints every
  creature its species's palette color (written into the same `color_u32`
  lane, so the renderer needs no branch). The action information moves to the
  ring-flash (below).
- **Why**: With an evolving body genome, "what is this creature" (identity /
  morphology / faction) and "what did it just do" (a brief highlight) are
  *separable*. The old EMA conflated the two and rode 3 f32 lanes; splitting
  them keeps the steady color readable for identity and frees the lanes.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`, `decisions/sim.md`
  (the sim-side ring-flash state).
- **Code anchors**: `src/wasm_api.rs → genome_color_u32`,
  `src/world/species.rs → SPECIES_PALETTE`.

### Per-action highlight is a transient 5-tick outer ring-flash (v2.0)

- **Decision**: When a creature acts, an outer ring pulses in a tag color for
  ~5 ticks: teal = born, green = grazed, yellow = attacked, blue = created a
  child (mate/split initiator), red = killed. Packed as `flash_tag` (3 bits) +
  `flash_ticks` countdown (4 bits) in the snapshot's `packed_u32`; priority when
  several fire in one tick is Killed > CreatedChild > Attacked > Grazed (Born
  set at spawn). Drawn as a separate instanced annulus pass on top of bodies,
  alpha fading with the countdown; follows the same wrap-aware ghost offsets.
- **Why**: Replaces the action information the EMA color used to carry, but as
  a transient overlay so the steady body color stays free for identity. A
  separate pass + scratch buffer avoids clobbering the body / highlight instance
  buffers.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`, `decisions/sim.md`.
- **Code anchors**: `src/creature.rs → FlashTag`, `src/world/tick.rs →
  flash_decay`, `web/src/render-gl.ts` (the flash pass + `flashScratch`).

### Grass uses a u8 box-filter mip pyramid (clipmap) for render LOD and snapshot windowing

- **Decision**: `GrassGrid` carries a `GrassPyramid` — a u8 mip pyramid built
  by box-filter mean-downsampling from L0 (the live density field) up through
  ~12 levels at the default `grass_dim = 1920`. L0 aliases the density field
  directly; L1+ are owned, edge-clamped. The pyramid is refreshed each tick at
  step 7b (after grass advances, before energy). It serves two consumers: (1)
  render LOD — the snapshot worker extracts a clipmap window at the appropriate
  mip level; (2) snapshot windowing — only the visible region at that level is
  published per slot.
- **Why**: At `grass_dim` > 2048 a full-resolution copy would exceed the
  budget; even at the default 1920² the copy is ~3.7 MB per tick. A pyramid
  lets the snapshot be both bandwidth-bounded (`GRASS_LOD_BUDGET_AXIS = 2048`
  cells per axis) and LOD-correct at any zoom. Mean downsampling (box filter)
  is the correct aggregate for a density field — no aliasing from nearest.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `src/grass.rs → GrassPyramid`, `GrassGrid`
  (pyramid field, step-7b refresh); `src/wasm_api.rs → write_snapshot`
  (LOD level + window extraction via `pyramid.viewport_window`).

### LOD level formula and budget constants

- **Decision**: `GRASS_LOD_BUDGET_AXIS = 2048` cells per axis; margin factor
  `GRASS_LOD_MARGIN_FACTOR = 1.5×` (the published window covers the visible
  viewport × 1.5, centered on the camera). LOD level =
  `floor(log2(max(1.0, visible_cell_span / GRASS_LOD_BUDGET_AXIS)))`, where
  `visible_cell_span = (world_size / cam_zoom) / GRASS_CELL_SIZE`. The `max(1.0)`
  pre-clamp ensures `log2` is never called on a value < 1.0 (which would give
  a negative level). Level is then clamped to `[0, max_level]`.
- **Why**: The budget axis of 2048 is >= `grass_dim` at the default world size
  (1920 < 2048), so the default-scale window equals the full field and output
  is byte-identical to the pre-pyramid path — zero regression at the shipped
  default. The 1.5× margin means a full-viewport pan in any direction stays
  covered for at least one tick without a new window being needed.
- **Tradeoffs**: The square-viewport approximation (vis_cells_x == vis_cells_y)
  means portrait/landscape viewports may over-request slightly on the narrow
  axis. Acceptable given the margin.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `src/wasm_api.rs` (`GRASS_LOD_BUDGET_AXIS`,
  `GRASS_LOD_MARGIN_FACTOR`, `write_snapshot` LOD computation block).
- **Revisit when**: viewport aspect ratio becomes significant (tall/wide worlds
  benefit from separate x/y window dimensions).

### Grass and biome textures are fixed 2048² R8; per-frame texSubImage2D of the window

- **Decision**: Both the grass and biome textures are allocated once at
  `GRASS_LOD_BUDGET_AXIS² = 2048×2048` (R8, `UNSIGNED_BYTE`). Per frame,
  `texSubImage2D` uploads only the `win_w × win_h` window bytes into the `(0,0)`
  corner of each texture (subarray-guarded). `NEAREST` filtering is used for
  both min and mag (trilinear is reserved via the `u_lod_blend` uniform, always
  `0.0` now). UV transform uniforms map the world-space quad into the window
  sub-region: `u_uv_scale = world_size / (GRASS_CELL_SIZE * 2^mipLevel * BUDGET_AXIS)`,
  `u_uv_offset = -vec2(winOriginX, winOriginY) / BUDGET_AXIS`. At default
  scale (mip=0, origin=(0,0)) this reduces to identity — no behavior change.
- **Why**: A fixed allocation avoids per-frame `texImage2D` resize overhead.
  Uploading only the window avoids uploading the entire budget² texture when the
  window is smaller. Nearest-mip first keeps the implementation simple; switching
  to trilinear later requires only a `texParameteri` change and making `u_lod_blend`
  non-zero.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render-gl.ts → initRenderer` (texture allocation,
  `GRASS_LOD_BUDGET_AXIS`); `web/src/render-gl.ts → renderWorldImpl` (the
  `texSubImage2D` upload + UV uniform writes for both grass and biome programs).

### Biome tint uses a mode-downsampled snapshot biome channel (not a separate mip)

- **Decision**: The biome texture fed to the `BIOME_FS` shader is not a
  precomputed biome mip pyramid. Instead, `write_snapshot` recomputes a
  mode-downsampled biome window (the most-frequent biome tag in each
  `2^mipLevel × 2^mipLevel` block of level-0 cells) on every tick, using the
  same window parameters as the grass channel. The result is appended to the
  slot immediately after the grass region (`biome_win_bytes = min(grass_dim,
  GRASS_LOD_BUDGET_AXIS)²` allocation). The biome UV transform in `BIOME_FS` is
  identical to `GRASS_VS`, so tint stays locked to the grass window at every LOD
  level.
- **Why**: Mode downsampling is correct for a categorical field (unlike mean,
  which would produce fractional biome ids). Keeping biome as a snapshot channel
  rather than a separate SAB avoids another buffer, and the biome field is static
  so the recompute could be precomputed — but that is deferred (noted as a perf
  TODO in `write_snapshot`). The alternative (a precomputed static biome
  mode-pyramid struct) is out of scope for the 2d grass-perf effort.
- **Tradeoffs**: Biome window recomputed every tick even though biome is static;
  acceptable at current performance. A precomputed biome mode-pyramid would
  eliminate this work but requires a new struct in `world/mod.rs`.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `src/wasm_api.rs → write_snapshot` (biome mode-downsample
  loop); `web/src/render-gl.ts → renderWorldImpl` (biome channel upload +
  UV uniforms); `web/src/sim-bridge.ts → biomeWinOffset` / `WindowMetadata`.

### Camera SAB lanes carry cx/cy/zoom/viewport to the worker for window computation

- **Decision**: Five control-SAB slots at indices 120–124 carry the current
  camera state from main to the worker each RAF:
  `CTRL_CAMERA_CX_BITS = 120` (cx as f32 bits),
  `CTRL_CAMERA_CY_BITS = 121` (cy as f32 bits),
  `CTRL_CAMERA_ZOOM_BITS = 122` (zoom as f32 bits),
  `CTRL_CAMERA_VIEWPORT_W = 123` (u32),
  `CTRL_CAMERA_VIEWPORT_H = 124` (u32).
  `main.ts` writes these each RAF (and pre-seeds them at first-tick init to
  `world_size / 2`, `world_size / 2`, zoom `1.0`). The sim worker passes them
  into `write_snapshot` as `cam_cx/cam_cy/cam_zoom/viewport_w/viewport_h`; wasm
  cannot read the JS SAB directly.
- **Why**: The worker needs camera state to compute the LOD level and clipmap
  window without polling JS. The SAB is the natural low-latency channel for
  main→worker state without a message-passing round trip. Storing as raw f32
  bits avoids any float-packing issues across the SAB i32 view.
- **Applies to**: `architecture/shared-memory-and-protocol.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `web/src/generated/control-sab.ts`
  (`CTRL_CAMERA_CX_BITS`…`CTRL_CAMERA_VIEWPORT_H`); `web/src/main.ts`
  (RAF write + first-tick init); `src/wasm_api.rs → write_snapshot`
  (the five camera parameters).

### SNAPSHOT_HEADER_BYTES bumped 32→64 to carry window metadata

- **Decision**: The per-slot snapshot header is 64 bytes (was 32). Bytes
  `[32..64)` carry 8 × u32 LE window-metadata fields: `mip_level`,
  `win_origin_x`, `win_origin_y`, `win_w`, `win_h`, `tex_dim_w`, `tex_dim_h`,
  `wrap_mode`. The creature SoA still starts at offset 64 (32-byte aligned).
- **Why**: The renderer needs the window parameters to compute the UV transform
  and to call `texSubImage2D` with the correct dimensions. Embedding them in the
  header is the lowest-latency path (same atomic flip as the snapshot data)
  and keeps the contract self-describing.
- **Applies to**: `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `src/wasm_api.rs` (`SNAPSHOT_HEADER_BYTES = 64`,
  the window-metadata write block in `write_snapshot`);
  `web/src/sim-bridge.ts → readWindowMetadata`, `SNAPSHOT_HEADER_BYTES`,
  `WindowMetadata`.

### Default-scale window equals the full field (byte-identical to pre-LOD path)

- **Decision**: The LOD and window logic is designed so that at the shipping
  default (world_size=9600, zoom=1.0, grass_dim=1920, GRASS_CELL_SIZE=5.0):
  mip_level=0, window=(0,0,1920,1920), win_w×win_h = 1920² = grass_cell_count.
  The slot grass bytes are byte-identical to the pre-pyramid path.
- **Why**: Zero-regression guarantee at the default scale means the new code
  cannot silently introduce a rendering difference for the most common case.
  The design invariant also validates the LOD formula: the `max(1.0)` pre-clamp
  before `log2` is what keeps `level=0` when `visible_cell_span / BUDGET_AXIS ≈ 0.9375 < 1.0`.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `src/wasm_api.rs → write_snapshot` (the DEFAULT-SCALE
  INVARIANT comment block).

### Toroidal wrap-seam is a known limitation at grass_dim > 2048

- **Decision**: When the camera is near the world seam on a toroidal world with
  `grass_dim > 2048` (i.e., non-default zoom-out), the clipmap window origin is
  clamped to `[0, level_dim - win]` rather than allowed to overflow and wrap.
  This means a window straddling the seam shows the wrong grass/biome region.
  The limitation is documented in `write_snapshot` and is NOT fixed.
- **Why**: Supporting toroidal wrap in `viewport_window` requires the window
  origin to overflow modulo `level_dim`, which `pyramid.viewport_window` does
  not currently implement. At the shipped default (`grass_dim=1920 < 2048`) the
  window always equals the full field and the seam never arises. Fixing this is
  a Stage-3 item (the TODO comment in `write_snapshot` marks the exact clamp to
  remove).
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `src/wasm_api.rs → write_snapshot`
  (the `TODO (Stage-3 wrap-seam)` comment on the `win_origin_x/y` clamp).

### Biomes render as flat per-cell color under the grass, fed from the snapshot biome channel

- **Decision**: The biome layer is a second full-screen world-quad drawn
  **under** the grass, sampling an R8 biome-id texture (one u8 `Biome`
  tag per grass cell) with NEAREST filtering — each cell a flat color
  (Plains/Water/Desert). The texture data comes from the per-slot windowed
  biome channel in the snapshot (mode-downsampled, same `win_w × win_h` as
  the grass window), uploaded via `texSubImage2D` into the `(0,0)` corner of
  a fixed `GRASS_LOD_BUDGET_AXIS² = 2048²` R8 texture each frame. The UV
  transform is identical to the grass channel (same `u_uv_scale/u_uv_offset`
  uniforms).
- **Why**: Feeding biome from the snapshot slot (rather than a separate static
  SAB view) keeps biome tint locked to the grass clipmap window at every LOD
  level — no UV mismatch when zoomed out. Drawing under the grass lets grass
  density brighten green on top of terrain. Textured / height-shaded biomes are
  a future concern (v2.1+).
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render-gl.ts → BIOME_VS/BIOME_FS`,
  `web/src/render-gl.ts → renderWorldImpl` (biome upload + UV uniforms);
  `src/wasm_api.rs → write_snapshot` (biome mode-downsample into slot).

### Per-species population graph, fed by the polled species table (v2.0)

- **Decision**: In species mode the population chart (in the bottom perf-box,
  `#chart-pop`) draws **one line per live species**, colored by each species'
  `color_u32` so chart hues match the canvas exactly. Per-species counts are
  aggregated **sim-side** and read from the polled `species_table_json` report
  (the same SAB-report mechanism as the NN-stats / profile polls), **not**
  packed into the fixed 20-byte snapshot header. Single-pool keeps the single
  pop line. Species appearing/disappearing leave gaps (broken polyline) rather
  than false zeros.
- **Why**: The single most informative artifact for watching a species sim. A
  polled report avoids widening the hot-path snapshot header for data the chart
  only needs a few times a second; single-sourcing the palette with the canvas
  (`SPECIES_PALETTE`) keeps chart and canvas hues from drifting. Color reuse is
  acceptable because typical runs end with 1–3 surviving species.
- **Applies to**: `architecture/app-shell.md` (the `#perf-box` pop chart),
  `architecture/shared-memory-and-protocol.md` (the species-table report).
- **Code anchors**: `src/wasm_api.rs → species_table_json`,
  `web/src/widgets/perf-panel.ts` (the per-species sampler + draw),
  `web/src/sim-bridge.ts → latestSpeciesTable`.

## See also

- [`../architecture/render-pipeline.md`](../architecture/render-pipeline.md)
- [`../architecture/shared-memory-and-protocol.md`](../architecture/shared-memory-and-protocol.md)
- [`profiler.md`](profiler.md)
