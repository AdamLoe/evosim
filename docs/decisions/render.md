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
- **Why**: ~921 KB per upload at 960 × 960 instead of ~3.7 MB.
  Quantization loses ~8 bits of density precision; visually
  indistinguishable, well under any visual-fidelity threshold.
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
  backing buffer as `creatures`, read `id_lo` / `id_hi` at offset
  `+24` / `+28` of each 32-byte stride, reassemble via
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

### `clampCamera` keeps zoom in `[0.5, 16]` and center in world bounds

- **Decision**: Camera zoom clamps to `[0.5, 16]`; center clamps to
  `[0, WORLD_SIZE]`.
- **Why**: Beyond zoom 16, creatures are bigger than the viewport and
  the renderer mostly draws off-screen quads. Below 0.5, body radii
  are sub-pixel and the canvas turns into noise. Center clamp
  prevents pan-into-the-void.
- **Applies to**: `architecture/render-pipeline.md`.
- **Code anchors**: `web/src/render.ts → clampCamera`,
  `web/src/render.ts → PX_PER_SIZE`.

## See also

- [`../architecture/render-pipeline.md`](../architecture/render-pipeline.md)
- [`../architecture/shared-memory-and-protocol.md`](../architecture/shared-memory-and-protocol.md)
- [`profiler.md`](profiler.md)
