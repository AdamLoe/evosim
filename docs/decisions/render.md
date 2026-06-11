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
- **Code anchors**: `app/web/src/render/gl.ts → renderWorld`,
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
- **Code anchors**: `app/web/src/render/gl.ts → renderWorldImpl`
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
- **Code anchors**: `app/web/src/render/gl.ts → renderWorldImpl`
  (the `gl.R8` upload), `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot_to`
  (the quantize loop).

### Renderer reads the SAB directly; no postMessage snapshot path

- **Decision**: Per RAF, the renderer reads the live slot via
  `Atomics.load(controlI32, CTRL_CURRENT_SLOT)` and constructs typed
  arrays over the SAB. No structured-clone, no copy.
- **Why**: Structured-clone cost at pop > 4000 dominated frame time on
  the previous postMessage path. SAB views are O(1) per frame.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/web/src/main.ts` (the `frame` body),
  `app/web/src/sim/bridge.ts → slotOffset` / `creatureSoAOffset` /
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
- **Code anchors**: `app/web/src/perf.ts → span` (the empty-stack `frame`
  special-case), `app/web/src/main.ts` (the outer span), `app/web/src/render/gl.ts`
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
- **Code anchors**: `app/web/src/render/gl.ts → renderWorldImpl`
  (the highlight pass with the `idView` build), `app/web/src/rail/highlight.ts`.

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
- **Code anchors**: `app/web/src/main.ts → frame` (the RAF loop,
  `framesThisSecond` / `fpsWindowStart` / `lastFps` closure state),
  `app/web/src/sim/bridge.ts → readSnapshotHeader` (`header.tps`).

### Zoom-out survey: `MIN_ZOOM = 0.04` + min-1px survey points

- **Decision**: Camera zoom clamps to `[MIN_ZOOM = 0.04, MAX_ZOOM]`; center
  clamps to the runtime `[0, world_size]`. Below ~1px on-screen radius a body
  is drawn as a **flat min-1px point** in its display color (the `inner_px =
  -1` sentinel short-circuits `DISC_FS` past disc shading / AA / ring) instead
  of vanishing. The floor is tuned so the whole runtime-sized world frames from
  one view.
- **Why**: With editable/large worlds (default 9600u) you need *some* way to
  survey the whole map and see *where the factions are*, not just terrain. A
  minimap / species heatmap is the right long-term answer; zoom-out +
  1px points is nearly free and good enough now. Center clamp still prevents
  pan-into-the-void.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/web/src/render/scene.ts → clampCamera`, `MIN_ZOOM` / `MAX_ZOOM`,
  `PX_PER_SIZE`; `app/web/src/render/gl.ts` (the point-size floor + `inner_px = -1`
  branch).
- **Revisit when**: a real minimap / species heatmap lands and a hard zoom-out
  floor is no longer the survey mechanism.

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
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot_to`, `pack_render_u32`,
  `lineage_color_u32`; `app/web/src/render/gl.ts` (instance pack + id/ring decode).
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
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → lineage_color_u32`,
  `app/crates/evosim/src/world/species.rs → SPECIES_PALETTE`.

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
- **Code anchors**: `crates/evosim/src/creature.rs → FlashTag`, `app/crates/evosim/src/world/tick.rs →
  flash_decay`, `app/web/src/render/gl.ts` (the flash pass + `flashScratch`).

### Grass uses a u8 box-filter mip pyramid (clipmap) for render LOD and snapshot windowing

- **Decision**: `GrassGrid` carries a `GrassPyramid` — a u8 mip pyramid built
  by box-filter mean-downsampling from L0 (the live density field) up through
  ~12 levels at the default `grass_dim = 1920`. L0 aliases the density field
  directly; L1+ are owned, edge-clamped. The pyramid is refreshed each tick at
  step 7b (after grass advances, before energy). It serves two consumers: (1)
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
- **Code anchors**: `app/crates/evosim/src/grass/mod.rs → GrassPyramid`, `GrassGrid`
  (pyramid field, step-7b refresh); `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (LOD level + window extraction via `pyramid.viewport_window`).

### LOD budget = 4096, single-sourced via gen_bindings

- **Decision**: `GRASS_LOD_BUDGET_AXIS = 4096` cells per axis (raised from
  2048). This is the **LOD switch threshold** (`level = floor(log2(span/4096))`),
  the **window/texture clamp ceiling**, and the **snapshot slot sizer**
  (grass + biome region each `min(grass_dim, 4096)²` bytes — ~4× the 2048-budget
  at `grass_dim > 2048`). It is **single-sourced in Rust** (`app/crates/evosim/src/wasm_api/mod.rs`)
  and emitted to TS via `cargo run --bin gen-bindings` →
  `app/web/src/generated/lod-constants.ts`, covered by the `bindings_in_sync` test.
  Margin factor `GRASS_LOD_MARGIN_FACTOR = 1.5×` is unchanged.
- **Why**: The 2048 budget downsampled "a layer too early" for worlds past ~2048
  cells/axis (the default 1920² was fine but any non-default zoom-out or larger
  grass_dim crossed the boundary). At 4096 full-resolution holds to ~16.7M cells,
  covering every practical world size. Cost: ~4× slot grass bytes ≈ 16 MB at
  grass_dim=4096 + correspondingly larger `texSubImage2D` upload vs the old 2048
  budget. **`bindings_in_sync` fails if Rust and TS drift.**
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → GRASS_LOD_BUDGET_AXIS`;
  `app/web/src/generated/lod-constants.ts` (generated);
  `app/web/src/sim/bridge.ts → GRASS_LOD_BUDGET_AXIS` (re-exported from generated);
  `app/crates/evosim/src/bin/gen_bindings.rs` (codegen + drift test).
- **Revisit when**: memory footprint of the 4096² slot becomes a concern at
  very large grass_dim values, or the budget needs to be dynamically adjusted
  per zoom level.

### grass_lod_step: discrete effective-resolution stepper

- **Decision**: `lod_bias` stays as the internal derived knob; the new
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
- **Why**: The old `lod_bias` could only nudge finer and only worked as an
  offset against the auto formula — it gave no way to intentionally force a
  coarse level for performance testing or low-power use. The stepper offers
  direct absolute level selection. Strategy (b) keeps a single code path in
  `write_snapshot` (the `lod_step > 0` branch vs the auto branch) and preserves
  persisted `lodBias` values for existing sessions.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → apply_lod_step`, `WorldHandle::lod_step`,
  `write_snapshot` (the `lod_step > 0` branch); `app/web/src/settings.ts → grassLodStep`;
  `app/web/src/widgets/devpanel.ts` (Render/LOD section dropdown);
  `crates/evosim/src/wasm_lod_stepper_tests.rs` (12 mapping-assertion tests).
- **SAB note**: adding slider index 63 pushed SLIDER_COUNT to 64, which required
  shifting `CTRL_INSPECT_REQ_EPOCH` from 80 → 96 (and all downstream inspect/stats
  constants +16). The camera slots shifted from 120–124 → 136–140.

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
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → apply_lod_bias`, `write_snapshot`
  (the `biased_level_f` computation).

### Aspect-ratio fix: vis_cells_y from viewport_h, not vis_cells_x

- **Decision**: `vis_cells_y` is now derived from `viewport_h` independently:
  `visible_cell_span_y = visible_cell_span_x × (viewport_h / viewport_w)`.
  Previously `vis_cells_y = vis_cells_x` assumed a square viewport.
- **Why**: Non-square viewports produced a mis-sized vertical window — under the
  old code a 1280×720 viewport produced a square window sized for the horizontal
  span, cropping the bottom. The fix is aspect-correct.
- **Tradeoffs**: The full-field invariant (`win_w = grass_dim`) holds for the
  **horizontal** direction only; `win_h` is correctly aspect-matched. E2E test
  `grass-lod-smoke.spec.ts` was updated to assert the aspect-correct values
  (not the old square-window values which encoded the pre-fix bug).
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (the `vis_cells_y` derivation via the `aspect` variable);
  `app/web/tests/e2e/grass-lod-smoke.spec.ts` (updated assertion).

### Grass and biome textures are fixed budget² R8; per-frame texSubImage2D of the window

- **Decision**: Both the grass and biome textures are allocated once at
  `GRASS_LOD_BUDGET_AXIS² = 4096×4096` (R8, `UNSIGNED_BYTE`). Per frame,
  `texSubImage2D` uploads only the `win_w × win_h` window bytes into the `(0,0)`
  corner of each texture (subarray-guarded). `NEAREST` filtering is used for
  both min and mag (trilinear is reserved via the `u_lod_blend` uniform, always
  `0.0` now). UV transform uniforms map the world-space quad into the window
  sub-region: `u_uv_scale = world_size / (grass_cell_size * 2^mipLevel * BUDGET_AXIS)`,
  `u_uv_offset = -vec2(winOriginX, winOriginY) / BUDGET_AXIS`. `grass_cell_size`
  comes from `boot_ready` (not hard-coded 5.0) so non-default `grass_size`
  settings render correctly. For toroidal ghost-copy draws, the renderer adds
  the ghost world translation divided by the window's world span to the base
  UV offset.
- **Why**: A fixed allocation avoids per-frame `texImage2D` resize overhead.
  Uploading only the window avoids uploading the entire budget² texture when the
  window is smaller. Nearest-mip first keeps the implementation simple; switching
  to trilinear later requires only a `texParameteri` change and making `u_lod_blend`
  non-zero.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/web/src/render/gl.ts → initRenderer` (texture allocation,
  `GRASS_LOD_BUDGET_AXIS`); `app/web/src/render/gl.ts → renderWorldImpl` (the
  `texSubImage2D` upload + UV uniform writes for both grass and biome programs).

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
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → BiomePyramid`, `write_snapshot`
  (`biome_pyramid.copy_window` call); `app/web/src/render/gl.ts → renderWorldImpl`
  (biome channel upload + UV uniforms); `app/web/src/sim/bridge.ts → biomeWinOffset` /
  `WindowMetadata`.

### Camera SAB lanes carry cx/cy/zoom/viewport to the worker for window computation

- **Decision**: Five control-SAB slots at indices **136–140** carry the current
  camera state from main to the worker each RAF:
  `CTRL_CAMERA_CX_BITS = 136` (cx as f32 bits),
  `CTRL_CAMERA_CY_BITS = 137` (cy as f32 bits),
  `CTRL_CAMERA_ZOOM_BITS = 138` (zoom as f32 bits),
  `CTRL_CAMERA_VIEWPORT_W = 139` (u32),
  `CTRL_CAMERA_VIEWPORT_H = 140` (u32).
  (Indices shifted from 120–124 when `grass_lod_step` at slider index 63 pushed
  `SLIDER_COUNT` to 64, displacing all downstream control-SAB blocks.)
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
- **Code anchors**: `app/web/src/generated/control-sab.ts`
  (`CTRL_CAMERA_CX_BITS`…`CTRL_CAMERA_VIEWPORT_H`); `app/web/src/main.ts`
  (RAF write + first-tick init); `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (the five camera parameters).

### SNAPSHOT_HEADER_BYTES bumped 32→64 to carry window metadata

- **Decision**: The per-slot snapshot header is 64 bytes (was 32). Bytes
  `[32..64)` carry window metadata: `mip_level: u32`, signed logical
  `win_origin_x/y: i32`, then `win_w`, `win_h`, `tex_dim_w`, `tex_dim_h`,
  `wrap_mode` as u32. The creature SoA still starts at offset 64
  (32-byte aligned).
- **Why**: The renderer needs the window parameters to compute the UV transform
  and to call `texSubImage2D` with the correct dimensions. Embedding them in the
  header is the lowest-latency path (same atomic flip as the snapshot data)
  and keeps the contract self-describing.
- **Applies to**: `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs` (`SNAPSHOT_HEADER_BYTES = 64`,
  the window-metadata write block in `write_snapshot`);
  `app/web/src/sim/bridge.ts → readWindowMetadata`, `SNAPSHOT_HEADER_BYTES`,
  `WindowMetadata`.

### Default-scale window equals the full field (byte-identical to pre-LOD path)

- **Decision**: The LOD and window logic is designed so that at the shipping
  default (world_size=9600, zoom=1.0, grass_dim=1920, grass_cell_size=5.0,
  `GRASS_LOD_BUDGET_AXIS=4096`):
  mip_level=0, window=(0,0,1920,1920), win_w×win_h = 1920² = grass_cell_count.
  The slot grass bytes are byte-identical to the pre-pyramid path.
- **Why**: Zero-regression guarantee at the default scale means the new code
  cannot silently introduce a rendering difference for the most common case.
  The design invariant also validates the LOD formula: the `max(1.0)` pre-clamp
  before `log2` is what keeps `level=0` when `visible_cell_span / BUDGET_AXIS ≈ 0.4688 < 1.0`.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot` (the DEFAULT-SCALE
  INVARIANT comment block).

### Toroidal clipmap windows use signed origins plus per-ghost UV offsets

- **Decision**: On toroidal worlds, the worker publishes signed logical
  `win_origin_x/y` metadata while copying grass/biome bytes from the
  modulo-normalized origin. The renderer applies the base UV transform for the
  primary draw and adds each ghost draw's world translation divided by the
  window world span.
- **Why**: A wrapped upload is ordered in logical window space, not canonical
  `[0, world_size]` space. Signed origins keep primary seam samples inside the
  uploaded window; per-ghost offsets make the `±world_size` copies sample the
  wrapped complement instead of repeating the primary region.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot`
  (signed metadata + wrapped copy origins);
  `app/crates/evosim/src/grass/mod.rs → GrassPyramid::viewport_window`;
  `app/web/src/render/gl.ts → renderWorldImpl`.

### Biomes render as flat per-cell color under the grass, fed from the snapshot biome channel

- **Decision**: The biome layer is a second full-screen world-quad drawn
  **under** the grass, sampling an R8 biome-id texture (one u8 `Biome`
  tag per grass cell) with NEAREST filtering — each cell a flat color
  (Plains/Water/Desert). The texture data comes from the per-slot windowed
  biome channel in the snapshot (mode-downsampled, same `win_w × win_h` as
  the grass window), uploaded via `texSubImage2D` into the `(0,0)` corner of
  a fixed `GRASS_LOD_BUDGET_AXIS² = 4096²` R8 texture each frame. The UV
  transform is identical to the grass channel (same `u_uv_scale/u_uv_offset`
  uniforms).
- **Why**: Feeding biome from the snapshot slot (rather than a separate static
  SAB view) keeps biome tint locked to the grass clipmap window at every LOD
  level — no UV mismatch when zoomed out. Drawing under the grass lets grass
  density brighten green on top of terrain. Textured / height-shaded biomes are
  a future concern.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `app/web/src/render/gl.ts → BIOME_VS/BIOME_FS`,
  `app/web/src/render/gl.ts → renderWorldImpl` (biome upload + UV uniforms);
  `app/crates/evosim/src/wasm_api/mod.rs → write_snapshot` (biome mode-downsample into slot).

### Creature render redesign: weakened halo, per-LOD halo multiplier, radial-shaded bottom dot, action-tint from existing flash data

- **Decision**: Five targeted changes to `render/gl.ts`, no snapshot/protocol changes:
  1. **Halo weakened** — global halo alpha scaled ×0.6 in `readHaloStrength()`.
  2. **Per-LOD halo strength** — `a_color.a` (the alpha lane of the per-instance
     color, previously unused in the body pass) now carries a per-instance LOD band
     multiplier: `1.0` (near), `0.4` (mid), `0.0` (bottom). `HALO_FS` multiplies
     this into the final glow alpha so bottom-band instances produce zero halo output
     per-instance without a separate draw call per band.
  3. **Mid-band ring tightened** — mid zoom ring changed from `0.985..1.0` → `0.993..1.0`
     (slimmer appearance at mid zoom).
  4. **Bottom-band dot is radial-shaded** — the `inner_px = -1` (sub-pixel creature)
     branch in `DISC_FS` now renders a radial gradient (`mix(1.3, 0.6, r)`, bright
     centre → dark edge) instead of a flat single-color fill.
  5. **Action-tint for bottom-band dots** — when `flashTicks > 0` (from the existing
     `packed_u32` lane 6, `flashTag` bits 0..3), the dot color is blended toward the
     action color by `(flashTicks / FLASH_TICKS_MAX) × 0.6`. No snapshot widening —
     uses `flashTag`/`flashTicks` already decoded in the body-pack loop.
- **Why**: The halo weakening reduces visual clutter at high pop (strong halos merged
  into blob at pop > 2000). Per-LOD halo encoding avoids an extra `drawArraysInstanced`
  call per band. A radial-shaded dot at survey zoom preserves faction territory
  readability with a low-cost in-shader gradient. Action-tint at the bottom band
  gives the same "creature just acted" visual signal that ring-flash gives at mid/near
  zoom, but in the cheaper 1px point at survey zoom. Using existing flash data avoids
  the S2R-blocked snapshot layout change.
- **Tradeoffs**: `a_color.a` is repurposed from "body alpha" (always 1.0) to "LOD halo
  strength multiplier." Body `DISC_FS` branches hardcode alpha to 1.0, so body opacity
  is unaffected. Action-tint affects only bottom-band (sub-pixel) creatures; mid/near-band
  creatures express flash via the ring-flash annulus pass.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `app/web/src/render/gl.ts → DISC_FS` (radial-shaded dot branch + halo
  alpha repurposing comment); `app/web/src/render/gl.ts → HALO_FS` (the `v_color.a` LOD
  multiplier); `app/web/src/render/gl.ts → renderWorldImpl` (the halo ×0.6 line,
  `HALO_ALPHA_NEAR/MID/BOTTOM` constants, action-tint blend, mid-band ring constants).

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
- **Code anchors**: `app/crates/evosim/src/wasm_api/mod.rs → species_table_json`,
  `app/web/src/widgets/perf-panel.ts` (the per-species sampler + draw),
  `app/web/src/sim/bridge.ts → latestSpeciesTable`.

## See also

- [`../architecture/render-pipeline.md`](../architecture/render-pipeline.md)
- [`../architecture/shared-memory-and-protocol.md`](../architecture/shared-memory-and-protocol.md)
- [`profiler.md`](profiler.md)
