---
status: active
owner: unassigned
last_updated: 2026-06-05
okay_to_delete: false
long_lived: true
owning_docs:
  - architecture/simulation-core.md
---

# Open bugs — needs triage

Collected from grass-stage3-review.md and v2.0.5/v2.0.x orchestration logs at plans-cleanup (2026-06-05). Each item was either confirmed still-open in the code or could not be confirmed fixed. Fix-confirmed items were dropped (not listed here).

---

- **Toroidal wrap-seam: viewport_window edge-clamps instead of wrapping.** When the camera is near a seam on a `wrap_world=true` world, `viewport_window` saturates `win_origin` at `level_w - raw_win_w` instead of wrapping, producing a hard-cut edge. Same bug present in the biome mode-downsample loop. Narrow: toroidal + grass_dim > 2048 + non-default zoom. `app/crates/evosim/src/grass/mod.rs → GrassPyramid::viewport_window`. status: needs triage.

- **Toroidal wrap-seam: render ghost-copies use wrong UV offset.** `u_uv_offset` is set once before the tile loop and is not adjusted per ghost copy. At non-trivial window offset (zoomed in or partially windowed), ghost copies at ±world_size sample the primary window region again instead of the wrapped complement. Narrow: toroidal + grass_dim 2049–3500 + non-default zoom. `app/web/src/render/gl.ts` → ghost-copy UV loop. status: needs triage.

- **Dead `biome_buf` wastes ~3.7 MB wasm heap.** The static full-field `biome_buf` Vec is still allocated and its offset/len still exposed via wasm_api getters and the boot-ready message, but main.ts no longer consumes it (superseded by the per-slot windowed biome channel from v2.0.3 Stream 2d). `app/crates/evosim/src/wasm_api/mod.rs → WorldHandle::biome_buf`. status: needs triage.

- **`tile_dirty` bits set each tick by scatter/blur but never consumed in production.** `quantize_dirty_tiles_into` (the only production consumer) was superseded by `pyramid.viewport_window` in v2.0.3 Stream 2b; dirty bits are now only consumed by tests. The scatter and blur paths still issue `O(active_tiles × 2)` atomic `fetch_or` operations per tick whose results are discarded in production. `app/crates/evosim/src/grass/mod.rs → compute_propagation_scatter` and blur path (`tile_dirty0`/`tile_dirty1` writes). status: needs triage.

- **Scatter fringe loop double-activates self-tile.** When `tile_has_grass`, line ~1868 calls `bitset_set_atomic(tile_active_next, t)`; the 3×3 neighbour loop then hits `(ddy=0, ddx=0)` and issues a second `fetch_or` on the same bit — a superfluous RMW cache transaction every tick for every active tile. `app/crates/evosim/src/grass/mod.rs → compute_propagation_scatter` fringe loop. status: needs triage.

- **`grass_bites_per_block` slider (idx 9) is inert for graze.** Stream 1e (v2.0.2) hardcoded `GRASS_BITES_PER_BLOCK=2`; the pre-existing slider at index 9 no longer affects the graze path at runtime (default unchanged, so behavior is correct but the live knob does nothing). `app/crates/evosim/src/grass/mod.rs → consume`. status: needs triage.

- **`grass_multisight` is restart-inert (construction-only setting not wired through boot param).** `grass_multisight` (slider 61, bool) is applied via `set_slider` after world construction, so changing it and restarting does not take effect — same gap that `grass_size` had before v2.0.5 S4 fixed it. Unlike `grass_size`, this does not resize buffers, so it is lower risk (no crash), but the setting is silently ignored on restart. `app/crates/evosim/src/wasm_api/mod.rs → new_with_founder_count` (param absent) and `apply_grass_multisight`. status: needs triage.
