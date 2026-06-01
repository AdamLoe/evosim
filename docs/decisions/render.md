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

### Biomes render as flat per-cell color under the grass (v2.0)

- **Decision**: The biome layer is a second full-screen world-quad drawn
  **under** the grass, sampling a static R8 biome-id texture (one u8 `Biome`
  tag per grass cell) with NEAREST filtering — each cell a flat color
  (Plains/Water/Desert). The texture is a view over wasm linear memory and is
  uploaded **once per worker swap** (a `biomeDirty` flag), not per frame.
- **Why**: Simple to ship and cheap — the biome map is static for a world's
  life, so a per-frame re-upload would be pure waste. Textured / height-shaded
  biomes are v2.1+. Drawing under the grass lets grass density brighten green
  on top of terrain.
- **Applies to**: `architecture/render-pipeline.md`,
  `architecture/shared-memory-and-protocol.md`.
- **Code anchors**: `web/src/render-gl.ts → BIOME_VS/BIOME_FS`,
  `web/src/main.ts` (`biomeView` / `biomeDirty`).

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
