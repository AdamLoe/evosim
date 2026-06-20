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
- **Code anchors**: `web/src/render/gl.ts → renderWorld`,
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
- **Code anchors**: `web/src/render/gl.ts → renderWorldImpl`
  (the body-pack loop, the `frame.render_world.creatures` span).

### Grass texture is R8, not R32F

- **Decision**: Grass density quantizes to `u8` Rust-side
  (`(d * 255).clamp(0,255) as u8`) and uploads as `gl.R8` /
  `gl.UNSIGNED_BYTE`.
- **Why**: One byte per cell instead of four. At the default
  (1920² runtime grid) that's a ~3.7 MB `texImage2D`/`texSubImage2D`
  instead of ~14.7 MB. Quantization loses ~8 bits of density precision;
  visually indistinguishable, well under any visual-fidelity threshold.
- **Tradeoffs**: A future per-cell-derivative use would want more bits;
  if that lands, the grass texture format becomes a per-consumer
  decision rather than a global one.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render/gl.ts → renderWorldImpl`
  (the `gl.R8` upload); `crates/evosim/src/wasm_api/mod.rs → write_snapshot`,
  `quantize_grass_into`.

### Renderer reads the SAB directly; no postMessage snapshot path

- **Decision**: Per RAF, the renderer reads the live slot via
  `Atomics.load(controlI32, CTRL_CURRENT_SLOT)` and constructs typed
  arrays over the SAB. No structured-clone, no copy.
- **Why**: Structured-clone cost at pop > 4000 dominated frame time on
  the previous postMessage path. SAB views are O(1) per frame.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/main.ts → frame`,
  `web/src/sim/bridge.ts → slotOffset` / `creatureSoAOffset` /
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
  special-case), `web/src/main.ts` (the outer span), `web/src/render/gl.ts`
  (the inner spans).

### Highlight rings decode ids from the SAB id-pair view, not from JSON

- **Decision**: Per frame, build a `Uint32Array` view over the same
  backing buffer as `creatures`, read `id_lo` / `id_hi` from each 32-byte
  stride (lanes 4 / 5, byte offset `+16` / `+20`), reassemble via
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
- **Code anchors**: `web/src/render/gl.ts → renderWorldImpl`
  (the highlight pass with the `idView` build), `web/src/rail/highlight.ts`.

### App FPS caps expensive paint work, not sim ticks

- **Decision**: The Display setting `appFPS` is restricted to
  `15 | 30 | 60 | 120` (default 60) and caps snapshot read, render, and
  upload work in `main.ts`. The RAF callback still runs at browser cadence
  for camera SAB writes and paused camera repaint. The perf status line's
  FPS is counted from painted frames in the trailing 1 s window.
- **Why**: The sim's target TPS and the app's paint budget are separate
  controls. Users need high TPS for simulation throughput without forcing
  main to read/upload/render snapshots the browser cannot display.
- **Tradeoffs**: While running, camera-only visual response can wait for
  the next configured app frame; while paused, camera movement bypasses the
  cap so panning/zooming a frozen world still repaints.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/worker-runtime.md`, `architecture/app-shell.md`.
- **Code anchors**: `web/src/main.ts → frame`,
  `web/src/settings.ts → APP_FPS_CHOICES`,
  `web/src/widgets/devpanel.ts → makeAppFpsRow`.

### Repeat camera survey is display-only and coverage-derived

- **Decision**: The camera can render repeated visual map tiles through the
  Display setting `visualRepeats`, independent of sim-side `wrapWorld`
  physics. Repeat mode derives tile offsets from the camera frustum and renders
  every tile needed to cover the supported viewport at the live
  `maxZoomOutMaps` zoom-out floor; bounded mode keeps a single `{0,0}` tile
  and clamps camera center to the runtime map. Survey-zoom bodies use
  formula-based radial points
  (`creatureSurveyDotMinPx`, `creatureSurveyDotScale`) with defaults that
  preserve the old 1px bottom-band point, and repeat borders have live
  min/scale/opacity controls.
- **Why**: Users need to inspect one-map, multi-map, and max-survey views
  without changing simulation topology or adding per-repeat-count controls.
  A display-only repeat camera gives an unbounded wrapped viewer for rendering
  while leaving saved worlds, physics, and snapshot layout unchanged.
- **Tradeoffs**: The worker still receives the camera center folded into the
  base map for clipmap windowing, so the visual camera can pan across repeated
  tiles without asking Rust to understand visual tile identity.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render/scene.ts → visibleRepeatTiles`,
  `clampCamera`, `initialCameraZoom`; `web/src/render/gl.ts →
  renderWorldImpl`; `web/src/main.ts → frame`.
- **Revisit when**: a minimap / species heatmap becomes the primary survey
  mechanism or repeat drawing needs an explicit GPU-budget guard that is still
  proven to cover every supported viewport/zoom combination.

### Snapshot carries render-only fields, packed compactly — stride stays 32 B

- **Decision**: Per-creature brain state and lineage hue live in `CreatureSoA`
  sim-side and do **not** ride the snapshot except for the packed display
  color. The snapshot carries only what the renderer reads: position, body
  radius, a packed display color (`color_u32`), the stable id
  (`id_lo`/`id_hi`), the action ring-flash + `species_id`
  (`packed_u32`). The stride does **not** grow — the 7 used lanes pad back to
  the existing 8 (`CREATURE_STRIDE = 8`, 32 B); only the grass + biome regions
  became settings-derived.
- **Why**: The snapshot is a render feed, not a state dump. The inspector
  reads details via separate inspect calls, so non-render state never needs to
  cross in the hot per-tick SoA. Packing the single-pool color into one RGBA8
  lane and bit-packing ring-flash + `species_id` keeps the stride stable, which
  keeps both the per-tick snapshot write and renderer decode cheap.
- **Applies to**: `architecture/shared-memory-and-protocol.md`,
  `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → write_creatures_each_into`,
  `pack_render_u32`, `lineage_color_u32`; `web/src/render/gl.ts → renderWorldImpl`.
- **Tradeoffs**: Packed encodings are less human-readable in a memory dump; the
  decode lives in the renderer. Worth it for bandwidth + a stable stride guard.

### Body color = lineage hue (single-pool) or species color (mating)

- **Decision**: Steady body color is identity. Single-pool color derives from
  the heritable lineage `hue` via HSV; species mode paints every creature its
  species palette color (written into the same `color_u32` lane, so the
  renderer needs no branch). Action information lives in the ring-flash.
- **Why**: "Which lineage/faction is this?" and "what did it just do?" are
  separate visual questions. A stable identity color plus a transient action
  ring answers both without growing the snapshot stride.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`, `decisions/sim.md`
  (the sim-side ring-flash state).
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → lineage_color_u32`,
  `crates/evosim/src/world/species.rs → SPECIES_PALETTE`.

### Per-action highlight is a transient 5-tick outer ring-flash

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
- **Code anchors**: `crates/evosim/src/creature.rs → FlashTag`,
  `crates/evosim/src/world/tick.rs → flash_decay`;
  `web/src/render/gl.ts → renderWorldImpl`.

### Grass uses a u8 box-filter mip pyramid (clipmap) for render LOD and snapshot windowing

- **Decision**: `GrassGrid` carries a `GrassPyramid` — a u8 mip pyramid built
  by box-filter mean-downsampling from L0 (the live density field) up through
  the levels implied by `GrassPyramid`. L0 aliases the density field directly;
  L1+ are owned, edge-clamped. The L1+ pyramid refresh is cadence-gated in
  `World::step` after grass advances and before energy. It serves two consumers: (1)
  render LOD — the snapshot worker extracts a clipmap window at the appropriate
  mip level; (2) snapshot windowing — only the visible region at that level is
  published per slot.
- **Why**: At `grass_dim` > 4096 a full-resolution copy would exceed the
  budget; even at the default 1920² the copy is ~3.7 MB per tick. A pyramid
  lets the snapshot be both bandwidth-bounded (`GRASS_LOD_BUDGET_AXIS = 4096`
  cells per axis) and LOD-correct at any zoom. Mean downsampling (box filter)
  is the correct aggregate for a density field — no aliasing from nearest.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/grass/mod.rs → GrassPyramid`,
  `GrassGrid`; `crates/evosim/src/world/mod.rs → World::step`;
  `crates/evosim/src/wasm_api/mod.rs → write_snapshot`.

### LOD budget = 4096, single-sourced via gen_bindings

- **Decision**: `GRASS_LOD_BUDGET_AXIS = 4096` cells per axis. This is the
  **LOD switch threshold** (`level = floor(log2(span/4096))`),
  the **window/texture clamp ceiling**, and the **snapshot slot sizer**
  (grass + biome region each `min(grass_dim, 4096)²` bytes). It is
  **single-sourced in Rust** (`crates/evosim/src/wasm_api/mod.rs`)
  and emitted to TS via `cargo run --bin gen-bindings` →
  `web/src/generated/lod-constants.ts`, covered by the `bindings_in_sync` test.
  Margin factor `GRASS_LOD_MARGIN_FACTOR = 1.5×` is unchanged.
- **Why**: The budget keeps full-resolution rendering available for practical
  default and large-world views while bounding per-slot grass and biome window
  memory. **`bindings_in_sync` fails if Rust and TS drift.**
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → GRASS_LOD_BUDGET_AXIS`;
  `web/src/generated/lod-constants.ts → GRASS_LOD_BUDGET_AXIS`;
  `web/src/sim/bridge.ts → GRASS_LOD_BUDGET_AXIS` (re-exported from generated);
  `crates/evosim/src/bin/gen_bindings.rs → main`, `bindings_in_sync`.
- **Revisit when**: memory footprint of the 4096² slot becomes a concern at
  very large grass_dim values, or the budget needs to be dynamically adjusted
  per zoom level.

### grass_lod_step: discrete effective-resolution stepper

- **Decision**: `lod_bias` stays as the internal derived knob; the
  `grass_lod_step` setting is the user-facing discrete stepper. The stepper sets
  absolute mip levels; when non-zero it overrides the auto formula and `lod_bias`
  entirely. When `grass_lod_step == 0` (Auto), the original formula + `lod_bias`
  path is unchanged (backward compat).
- **Setting name**: `grass_lod_step` (Rust slider index 63, TS `grassLodStep`).
- **Displayed values and Auto mapping** (at default grass_dim=1920, 5u cells):
  - `Auto` (0): auto-selects from visible-span formula; at default zoom=1 this
    is L0 (1920 cells, full resolution). `lodBias` still active in Auto mode.
  - `Full ~1920` (1): force L0 — one step finer than auto at any zoom-out
    level where auto would pick L1+. Same as auto at default zoom.
  - `Half ~960` (2): force L1 — 4× fewer cells, intentionally coarse.
  - `Quarter ~480` (3): force L2 — 16× fewer cells.
  - `Eighth ~240` (4): force L3 — 64× fewer cells.
  - `Sixteenth ~120` (5): force L4 — 256× fewer cells.
- **LIVE setting** — LOD selection happens in `write_snapshot` per-snapshot. No
  restart required. Exposed as a staged (non-construction-only) DevPanel dropdown.
- **Why**: `lod_bias` only nudges finer as an offset against the auto formula; it
  does not intentionally force a coarse level for performance testing or
  low-power use. The stepper offers direct absolute level selection while
  preserving persisted `lodBias` values for existing sessions.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → apply_lod_step`,
  `write_snapshot`; `web/src/settings.ts → Settings`;
  `web/src/widgets/devpanel.ts → installDevPanel`;
  `crates/evosim/src/wasm_api/tests/lod_stepper.rs → stepper_overrides_lod_bias`.
### lod_bias: "nudge within budget" LOD fine-tuning

- **Decision**: `lod_bias` (slider idx 60, live f32, default 0.0) subtracts from
  the computed float mip level before flooring, allowing finer-detail rendering.
  It is clamped ≥ 0 (no coarsening). **Semantic constraint**: biasing finer than
  the budget can hold causes the window to clamp at `GRASS_LOD_BUDGET_AXIS` and
  **crop viewport edges**. The bias is therefore a "nudge within budget," not an
  independent resolution override. The user-facing note in the DevPanel explains
  this edge-crop behavior.
- **Why**: Keeps the LOD formula deterministic (no per-session budget changes)
  while giving the user a live knob to trade edge coverage for center detail.
  Default 0.0 is the FEEL-TUNE point (the lead should confirm it is the right
  default for typical use).
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → apply_lod_bias`, `write_snapshot`
  (the `biased_level_f` computation).

### Aspect-ratio fix: vis_cells_y from viewport_h, not vis_cells_x

- **Decision**: `vis_cells_y` is now derived from `viewport_h` independently:
  `visible_cell_span_y = visible_cell_span_x × (viewport_h / viewport_w)`.
  This keeps non-square viewports from using a square window sized only from the
  horizontal span.
- **Why**: The worker needs the vertical clipmap window to match the actual
  viewport aspect ratio, or grass/biome uploads can miss visible vertical
  coverage.
- **Tradeoffs**: The full-field invariant (`win_w = grass_dim`) holds for the
  **horizontal** direction only; `win_h` is correctly aspect-matched. E2E test
  `grass-lod-smoke.spec.ts` asserts the aspect-correct values.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (the `vis_cells_y` derivation via the `aspect` variable). The
  `cd app/web && pnpm test:e2e` gate covers the default-scale smoke assertion.

### Grass and biome textures are fixed budget² R8; static biome uploads are cached

- **Decision**: Both the grass and biome textures are allocated once at
  `GRASS_LOD_BUDGET_AXIS² = 4096×4096` (R8, `UNSIGNED_BYTE`). Grass uploads
  only the `win_w × win_h` window bytes into the `(0,0)` corner on each
  painted frame. Biome uploads the same window only when the metadata key or
  wasm-memory buffer changes, because the biome grid is static after boot.
  `NEAREST` filtering is used for biome; grass uses its render-path filter.
  UV transform uniforms map the world-space quad into the window
  sub-region: `u_uv_scale = world_size / (grass_cell_size * 2^mipLevel * BUDGET_AXIS)`,
  `u_uv_offset = -vec2(winOriginX, winOriginY) / BUDGET_AXIS`. `grass_cell_size`
  comes from `boot_ready` (not hard-coded 5.0) so non-default `grass_size`
  settings render correctly. Toroidal ghost-copy draws keep the same UV offset
  as the primary draw; shader-side `u_uv_wrap` handles full-period wrapped
  sampling when the upload covers the whole world on an axis.
- **Why**: A fixed allocation avoids per-frame `texImage2D` resize overhead.
  Uploading only the window avoids uploading the entire budget² texture when the
  window is smaller. Caching biome upload avoids repeating static bytes when
  only grass density changed.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render/gl.ts → initRenderer`,
  `renderWorldImpl`; `web/src/sim/bridge.ts → GRASS_LOD_BUDGET_AXIS`.

### Biome tint uses a mode-downsampled snapshot biome channel, fed from a precomputed BiomePyramid

- **Decision**: The biome texture fed to the `BIOME_FS` shader is a windowed
  mode-downsampled biome channel appended to the snapshot slot (same `win_w × win_h`
  as the grass window, same UV transform). `write_snapshot` copies the correct window
  from a **precomputed `BiomePyramid`** (built once at world construction) rather than
  recomputing mode-downsampling per tick. The biome UV transform in `BIOME_FS` is
  identical to `GRASS_VS`, so tint stays locked to the grass window at every LOD level.
- **Why**: Mode downsampling is correct for a categorical field (unlike mean, which
  produces fractional biome ids). Keeping biome as a snapshot channel rather than a
  separate SAB avoids another buffer. The `BiomePyramid` precompute eliminates the
  per-tick recompute cost that was 83% of total snapshot write time (11.4 ms out of
  13.7 ms). See `decisions/sim.md → BiomePyramid precomputed at construction`.
- **Tradeoffs**: `BiomePyramid` adds memory proportional to `O(grass_cell_count × 4/3)`.
  Static-only — becomes invalid if biome editing ships (requires rebuild on mutation).
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → BiomePyramid`,
  `write_snapshot`; `web/src/render/gl.ts → renderWorldImpl`;
  `web/src/sim/bridge.ts → biomeWinOffset`, `WindowMetadata`.

### Camera SAB lanes carry cx/cy/zoom/viewport to the worker for window computation

- **Decision**: Five control-SAB slots at indices **136–140** carry the current
  camera state from main to the worker each RAF:
  `CTRL_CAMERA_CX_BITS = 136` (cx as f32 bits),
  `CTRL_CAMERA_CY_BITS = 137` (cy as f32 bits),
  `CTRL_CAMERA_ZOOM_BITS = 138` (zoom as f32 bits),
  `CTRL_CAMERA_VIEWPORT_W = 139` (u32),
  `CTRL_CAMERA_VIEWPORT_H = 140` (u32).
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
- **Code anchors**: `web/src/generated/control-sab.ts → CTRL_CAMERA_CX_BITS`,
  `CTRL_CAMERA_CY_BITS`, `CTRL_CAMERA_ZOOM_BITS`, `CTRL_CAMERA_VIEWPORT_W`,
  `CTRL_CAMERA_VIEWPORT_H`; `web/src/main.ts → frame`;
  `crates/evosim/src/wasm_api/mod.rs → write_snapshot`.

### SNAPSHOT_HEADER_BYTES carries window metadata

- **Decision**: The per-slot snapshot header is 64 bytes. Bytes
  `[32..64)` carry window metadata: `mip_level: u32`, signed logical
  `win_origin_x/y: i32`, then `win_w`, `win_h`, `tex_dim_w`, `tex_dim_h`,
  `wrap_mode` as u32. The creature SoA still starts at offset 64
  (32-byte aligned).
- **Why**: The renderer needs the window parameters to compute the UV transform
  and to call `texSubImage2D` with the correct dimensions. Embedding them in the
  header is the lowest-latency path (same atomic flip as the snapshot data)
  and keeps the contract self-describing.
- **Applies to**: `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → SNAPSHOT_HEADER_BYTES`,
  `write_snapshot`;
  `web/src/sim/bridge.ts → readWindowMetadata`, `SNAPSHOT_HEADER_BYTES`,
  `WindowMetadata`.

### Default-scale window equals the full field (byte-identical to pre-LOD path)

- **Decision**: The LOD and window logic is designed so that at the shipping
  default (world_size=9600, zoom=1.0, grass_dim=480, grass_cell_size=20,
  `GRASS_LOD_BUDGET_AXIS=4096`):
  mip_level=0, window=(0,0,480,480), win_w×win_h = 480² = grass_cell_count.
  The slot grass bytes are byte-identical to the pre-pyramid path.
- **Why**: The default-scale invariant keeps the common path full-resolution and
  validates the LOD formula: the `max(1.0)` pre-clamp before `log2` keeps
  `level=0` when the visible span is below the budget axis.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → write_snapshot`.

### Toroidal clipmap windows use signed origins plus shader UV wrapping

- **Decision**: On toroidal worlds, the worker publishes signed logical
  `win_origin_x/y` metadata while copying grass/biome bytes from the
  modulo-normalized origin. The renderer applies the same base UV transform for
  primary and ghost draws, shifts the camera position for ghost world copies,
  and uses `u_uv_wrap` when the uploaded window spans a full toroidal period on
  an axis.
- **Why**: A wrapped upload is ordered in logical window space, not canonical
  `[0, world_size]` space. Signed origins keep primary seam samples inside the
  uploaded window; shader wrapping prevents fixed-budget texture edge clamps
  from smearing at world-tile boundaries during ghost draws.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (signed metadata + wrapped copy origins);
  `crates/evosim/src/grass/mod.rs → GrassPyramid::viewport_window`;
  `web/src/render/gl.ts → renderWorldImpl`, `windowUvWrap`.

### No-grass zones render as flat per-cell color under the grass

- **Decision**: The no-grass-zone layer is a second full-screen world-quad
  drawn **under** the grass, sampling an R8 zone-id texture with NEAREST
  filtering. The internal bytes are still `Biome` tags for compatibility, but
  the visible model is normal grass capacity vs no-grass/dead-grass zones. The
  texture data comes from the per-slot windowed zone channel in the snapshot
  (mode-downsampled, same `win_w × win_h` as the grass window), uploaded via
  `texSubImage2D` into the `(0,0)` corner of a fixed
  `GRASS_LOD_BUDGET_AXIS² = 4096²` R8 texture when metadata changes. The UV
  transform is identical to the grass channel (same `u_uv_scale/u_uv_offset`
  uniforms).
- **Why**: Feeding biome from the snapshot slot (rather than a separate static
  SAB view) keeps biome tint locked to the grass clipmap window at every LOD
  level — no UV mismatch when zoomed out. Drawing under the grass lets grass
  density brighten green on top of no-grass-zone tint.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render/gl.ts → BIOME_VS`, `BIOME_FS`,
  `renderWorldImpl`;
  `crates/evosim/src/wasm_api/mod.rs → write_snapshot` (biome mode-downsample into slot).

### Creature appearance uses per-LOD halo, radial-shaded bottom dots, and action tint

- **Decision**: Creature appearance is LOD-aware inside `render/gl.ts`: global
  halo strength is scaled in `readHaloStrength()`, `a_color.a` carries the
  per-instance halo multiplier, the bottom-band `inner_px = -1` branch renders a
  radial-shaded point, and bottom-band action tint uses the existing
  `flashTag`/`flashTicks` data from `packed_u32`. No snapshot/protocol fields are
  added for these effects.
- **Why**: Per-LOD halo encoding avoids an extra `drawArraysInstanced` call per
  band, radial-shaded dots preserve faction readability at survey zoom, and
  bottom-band action tint gives the same "creature just acted" signal that
  ring-flash gives at mid/near zoom without widening the hot snapshot layout.
- **Tradeoffs**: `a_color.a` is a halo strength multiplier rather than body
  opacity. Body `DISC_FS` branches hardcode alpha to 1.0, so body opacity is
  unaffected. Action-tint affects only bottom-band creatures; mid/near-band
  creatures express flash via the ring-flash annulus pass.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render/gl.ts → DISC_FS`, `HALO_FS`,
  `renderWorldImpl`.

### Per-species population graph, fed by the polled species table

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
- **Code anchors**: `crates/evosim/src/wasm_api/mod.rs → species_table_json`,
  `web/src/widgets/perf-panel.ts → installProfilerPanel`,
  `web/src/sim/bridge.ts → latestSpeciesTable`.

## See also

- [`../architecture/render-pipeline.md`](../architecture/render-pipeline.md)
- [`../architecture/shared-memory-and-protocol.md`](../architecture/shared-memory-and-protocol.md)
- [`profiler.md`](profiler.md)
