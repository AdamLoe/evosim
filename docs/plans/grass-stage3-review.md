---
status: active
owner: synthesis-agent
last_updated: 2026-06-02
---

# Stage-3 Assessment: Grass Scatter + LOD Pyramid — Consolidated Review

## PERF VERDICT

The criterion bench measured a **34x** scatter-vs-blur speedup (scatter: 1.04 ms,
blur: 35.7 ms) at 16 384 seeds on a 512² grid (6.25% fill). This does NOT meet the
architectural 50–100x target. The gap is explained by density: at 6.25% fill the
active-tile set activates a large fraction of all 256 tiles, so scatter still touches
O(many-tiles × active-cells) rather than a sparse frontier. The 50–100x range is
recovered at truly sparse early-game grids (< 1% fill), where scatter skips nearly all
cells while blur still walks every tile × 1024 cells × 9 taps × 8 neighbours. In the
browser at production scale (1920² scatter kernel, Playwright/Chromium), grass_step
measures 16.76 ms/call (default) and 22.01 ms/call (2200² large world), with tick TPS
of 16 and 14 respectively — dominated by total tick wall-clock (~27–33 ms), not
grass_step alone. Scaling is strictly linear O(N cells), confirmed by the per-row
cost staying flat at 0.048–0.049 ms/row across both world sizes. **The 50–100x
criterion target is NOT met at mid-game fill; the actual speedup is ~34x and real.**
The blur path should be retained until feel-tune confirms scatter is tuned well enough
for production; **deletion of the blur is conditionally safe but not yet warranted**
— see the blocks_blur_deletion list below.

---

## BLOCKERS

None. No finding rises to the level of blocking the Stage-3 work or shipping the
scatter kernel. The blur path is retained behind its selector flag exactly for this
purpose.

---

## MAJORS

1. **Water-biome SATURATED classification never fires on the blur path**
   (`src/grass.rs:233,1174–1209,1402–1430`). `GRASS_EQ_EPS = 1e-4` is 39x smaller
   than one u8 step (1/255 ≈ 3.92e-3). Water-cap tiles (cap = 0.04) encode to byte
   10/255 = 0.03922, which is less than `cap - GRASS_EQ_EPS = 0.03990`, so they
   never classify as SATURATED on the blur path and remain permanently MIXED, keeping
   every Water tile active every tick and preventing the frontier skip optimization.
   Scatter path unaffected. **Fix:** raise `GRASS_EQ_EPS` to `1.0 / 255.0` (≈ 0.004).
   _(Confirmed from hub carried-forward: "u8 quantization FLOOR near low-cap biomes".)_

2. **Toroidal wrap-seam: viewport_window edge-clamps instead of wrapping**
   (`src/grass.rs:569–580`; `src/wasm_api.rs:686–692`). For toroidal worlds with the
   camera near the seam, `win_origin_x = ox.min(level_w.saturating_sub(raw_win_w))`
   saturates at `level_w - raw_win_w` rather than wrapping. The grass texture window
   cuts at the field edge and shows a hard repeat of the left half rather than
   the correct wrapped region. The same bug is present in the biome mode-downsample
   loop. Affects all worlds with `wrap_world=true` when zoomed in or partially
   windowed. At the default world (grass_dim=1920, zoom=1) the full-field invariant
   masks it entirely. _(Confirmed from hub carried-forward: "Toroidal wrap seam for
   windowed textures.")_

3. **Toroidal wrap-seam: render ghost-copies use wrong UV offset**
   (`web/src/render-gl.ts:1054–1062, 1125–1133`). `u_uv_offset` is set once before
   the tile loop and is not adjusted per ghost copy. When the window is non-trivially
   offset (zoomed in or partially windowed), ghost copies at ±world_size sample the
   primary window region again instead of the wrapped complement. Affects any
   grass_dim between 2049 and ~3500 on a toroidal world at non-default zoom.
   _(Confirmed from hub carried-forward; raised independently by SAB, sim, render, and
   parallelism dimensions.)_

---

## MINORS

### Real bugs (wrong behaviour, no crash risk)

4. **`scatter_add`/`scatter_sub` `.max(1)` floor: zero-amount slider yields 1-byte
   effect** (`src/grass.rs:1574–1575`). `encode_density(0.0).max(1) = 1`, so setting
   `decay_amount=0.0` still removes ~GRASS_MAX/255 density per roll. Semantically wrong
   slider behaviour. Fix: gate the roll with `if pct > 0.0 && amount > 0.0`.

5. **`viewport_w`/`viewport_h` accepted but silently discarded — square-viewport
   approximation** (`src/wasm_api.rs:668–669, 845`). `vis_cells_y = vis_cells_x`
   regardless of actual viewport aspect ratio. Over-allocates grass rows on landscape
   displays; can under-cover on portrait. SAB infrastructure already carries the real
   dimensions. Fix: derive `vis_cells_y` from `viewport_h` independently.
   _(Confirmed from hub: "viewport_w/h read but unused in the window calc"; raised by
   all four dimensions.)_

6. **`texSubImage2D` passes full BUDGET²-sized buffer without `subarray` guard**
   (`web/src/render-gl.ts:1029–1034, 1093–1098`). Currently safe (WebGL reads
   `win_w × win_h` bytes from offset 0, matching Rust's packed write). Becomes a
   silent footgun if `UNPACK_ROW_LENGTH` is ever set to `BUDGET_AXIS`. Fix: pass
   `grass.subarray(0, winW * winH)` (and same for biomeWin) explicitly.

7. **First-tick SAB camera default is `cx=cy=0, zoom=0`** before the first RAF writes
   (`web/src/render.ts:19–23`; `src/wasm_api.rs`). The `safe_zoom` guard handles
   zoom=0, and at default scale the window is always full-field regardless of cx/cy.
   One-frame artifact at non-default zoom only. Fix: initialise SAB camera lanes to
   `worldSize/2, worldSize/2, 1.0` synchronously after boot handshake.

8. **`dead biome_buf`** (`src/wasm_api.rs:258, 299–310`; `web/src/main.ts:599–603`).
   The static full-field `biome_buf` Vec (~3.7 MB at default scale) is still allocated
   and its offset/len still exposed via wasm_api getters and the `boot_ready` message,
   but main.ts explicitly notes it is no longer consumed by the renderer (superseded by
   the per-slot windowed biome channel). Wastes wasm heap. Fix: remove `biome_buf`,
   its two getters, and the corresponding `SimReplyBootReady` fields.
   _(Confirmed from hub: "dead biome_buf"; raised by all four dimensions.)_

9. **LOD formula comment is wrong** (`src/wasm_api.rs:643–650`). Comment states
   "floor(−0.09) = 0 (clamped)" but `floor(−0.09) = −1` in mathematics. The actual
   code correctly uses `.max(1.0)` before `log2`, giving 0 — the comment misstates
   the mechanism. Only the documentation is wrong; the code is correct.

### Style / perf (no wrong behaviour)

10. **`unsafe` split-borrow in `GrassGrid::refresh_pyramid` is sound but removable**
    (`src/grass.rs:983–992`). Raw-pointer reborrows of distinct named struct fields are
    unnecessary; safe field projection compiles without `unsafe`. Fix: `let density =
    &self.density[..]; let tile_active = &self.tile_active[..]; self.pyramid.refresh(...)`.
    _(Confirmed from hub: "unsafe split-borrow in refresh_pyramid"; raised by all four
    dimensions.)_

11. **`GrassPyramid::refresh` is O(N) full recompute every tick** (`src/grass.rs:495–521`).
    `_tile_active` / `_tiles_per_axis` parameters are accepted but ignored (Stage-3
    TODO at line 399). At 1920² → ~1.3M cells rebuilt each tick regardless of frontier
    size. Fix deferred to Stage 3: dirty-subtree walk off `tile_active`. _(Confirmed
    from hub: "Pyramid refresh is a FULL recompute"; raised by all four dimensions.)_

12. **Biome window mode-downsampled from static `biome_grid` every tick**
    (`src/wasm_api.rs:778–841`). `biome_grid` never changes after construction; yet
    `write_snapshot` re-runs the full mode-downsample loop (O(win_w × win_h × block²))
    on every tick. Fix deferred to Stage 3: precompute a static biome mode-pyramid at
    boot. _(Confirmed from hub: "Biome window recomputed each tick"; raised by all four
    dimensions.)_

13. **`tile_dirty` bits set each tick by scatter/blur but never consumed in production**
    (`src/grass.rs:1673–1674, 1690–1691, 1510–1511`). `quantize_dirty_tiles_into` (the
    only consumer of dirty bits) is only called by tests; the production snapshot path
    uses `pyramid.viewport_window` directly and never clears dirty bits. Result: O(active_tiles
    × 2) superfluous atomic `fetch_or` operations per tick whose outputs are discarded.
    Fix: gate dirty-bit writes behind a flag or move them to the test helper only.
    _(Confirmed from hub: "Snapshot copies the full window each tick / quantize_dirty_tiles_into
    now dead"; raised by parallelism dimension.)_

14. **Scatter fringe loop double-activates self-tile** (`src/grass.rs:1688–1712`). When
    `tile_has_grass`, line 1689 already calls `bitset_set_atomic(tile_active_next, t)`;
    the 3×3 neighbour loop then hits `(ddy=0, ddx=0)` and issues a second `fetch_or`
    on the same bit. The second call is a no-op (bit already set) but issues a
    superfluous RMW cache transaction every tick for every active tile.

15. **Square UV-scale assumption in renderer** (`web/src/render-gl.ts:1038, 1109`).
    Both paths compute `uvScaleVal` as a single scalar and upload `vec2(val, val)`.
    Correct only while `win_w == win_h`. Becomes incorrect if/when viewport_w/h are
    wired up (minor #5 fix). Fix: compute separate X/Y scale and offset from `winW`/`winH`.

16. **Widened test tolerances at 7 sites** (5 from Stream 1a, 2 from Stream 1e). Sites
    widened from 1e-5/1e-6 to `1.0/255.0` or `quantum + 1e-5` to accommodate u8
    quantization. Masks future regressions where quantization is inadvertently worsened.
    Stage-3 action: re-author as proper u8-domain assertions. _(Confirmed from hub;
    raised by sim and parallelism dimensions.)_

17. **LOD gate assertion is soft-skip (best-effort) for mip > 0 path**
    (`web/tests/e2e/stream2c-grass-render-smoke.spec.ts:393–407`). Part 2 of the LOD
    test `return`s if `lodEngaged` is false, so a regression in the zoomed LOD path
    would not fail CI. Fix: set up a programmatic camera zoom that forces `mipLevel > 0`
    and assert it as a hard `expect`.

18. **Stale Stage-1 e2e spec reads old 32-byte header**
    (`web/tests/e2e/grass-scatter-smoke.spec.ts`). After Stream 2b bumped the snapshot
    header from 32 to 64 bytes, the Stage-1 throwaway spec computes wrong offsets. Will
    fail if the full Playwright suite runs it. Fix: remove or update before the final
    e2e gate. _(Confirmed from hub: "Two throwaway e2e specs.")_

19. **`grass_bites_per_block` slider (idx 9) is inert for graze** (`src/grass.rs`).
    Stream 1e hardcoded `GRASS_BITES_PER_BLOCK=2`; the pre-existing slider no longer
    affects graze behaviour at runtime (default unchanged). Fix: re-expose the constant
    live and add a compile-assert tying the slider default to the constant.
    _(Confirmed from hub: "grass_bites_per_block slider inert for graze.")_

---

## BLOCKS BLUR DELETION

The blur path must be retained (and not deleted) until ALL of the following are
resolved:

1. **Perf target not met at mid-game density.** The measured 34x ratio at 6.25% fill
   does not reach the 50–100x architectural target. Until feel-tune confirms that 34x
   is acceptable for the planned game density range (or scatter is optimised to recover
   more speedup at denser grids), the blur fallback is the production safety net.

2. **Water SATURATED classification bug not fixed (major #1).** On blur-path runs the
   EPS bug keeps all Water tiles permanently active, degrading exactly the case the blur
   frontier is meant to optimise. Fixing GRASS_EQ_EPS to 1/255 is required before any
   meaningful blur-vs-scatter frontier comparison.

3. **Scatter feel-tune not complete.** The hub log documents that the Stage-1 balance
   smoke pinned the `default_world_long_run` test to blur because scatter is
   intentionally hotter (plains super-critical). Slider tuning (Stage-3 feel-tune
   milestone) must verify: plains is super-critical but stable, water is sub-critical,
   energy economy is balanced. This verification has not yet run.

4. **`grass_bites_per_block` slider inert (minor #19).** Graze tuning cannot be
   completed while the bites-per-block knob is disconnected. Restore before final
   feel-tune.

---

## CROSS-REFERENCE: CARRIED-FORWARD DEBT STATUS

| Hub item | Status in review |
|---|---|
| u8 quantization FLOOR near low-cap biomes | **CONFIRMED + EXPANDED** — Water SATURATED never fires on blur (major #1) |
| Test-tolerance widenings (5 + 2 sites) | **CONFIRMED** — 7 sites total; Stage-3 re-author needed (minor #16) |
| Dead-code `quantize_grass_into` wasm32 | Not re-audited (cosmetic; build exit 0) |
| Scatter energy economy hotter / `default_world_long_run` pinned to blur | **CONFIRMED** — blocks blur deletion (see #3 above) |
| Per-tile freeze allocation (perf unmeasured) | **ADDRESSED** — criterion bench validates ~34x; per-worker scratch buffer not yet needed |
| `tile_class` stale on scatter path | **CONFIRMED HARMLESS** — documented inline; no correctness impact |
| `grass_bites_per_block` slider inert | **CONFIRMED** — minor #19; blocks blur deletion |
| `consume_below_chunk_yields_partial_bite` tol widened | **CONFIRMED** — part of minor #16 |
| Pyramid refresh FULL recompute | **CONFIRMED** — minor #11; Stage-3 dirty-subtree walk |
| `unsafe` split-borrow in `refresh_pyramid` | **CONFIRMED SOUND, REMOVABLE** — minor #10 |
| Snapshot full-window copy / `quantize_dirty_tiles_into` dead | **CONFIRMED** — minor #13; dirty bits also wasted each tick |
| `viewport_w/h` read but unused | **CONFIRMED** — minor #5 |
| Toroidal wrap seam for windowed textures | **CONFIRMED** — major #2 (Rust) + major #3 (render) |
| Grass texture filter LINEAR→NEAREST | **CONFIRMED ALREADY APPLIED** — NEAREST on both grass and biome textures; no action needed |
| Biome window recomputed each tick | **CONFIRMED** — minor #12 |
| Dead `biome_buf` | **CONFIRMED** — minor #8 |
| Two throwaway e2e specs (stale Stage-1 + 2b) | **CONFIRMED** — minor #18 |
| 2e mip sensing incomplete | Not in scope for this review; Stage-3 complete-or-drop |
| Scatter cross-tile RMW races (accepted noise) | **CONFIRMED SAFE** — saturating arithmetic bounds [0,255]; cannot panic |
| Default-scale window=full-field invariant | **VERIFIED HOLDS** — math traced: raw_win_w=level_w always at default scale |
| First-tick SAB cx=cy=0 before first RAF | **CONFIRMED ONE-FRAME ARTIFACT** — minor #7; safe at default zoom |

---

## DEDUPLICATION NOTE

The following findings were raised by multiple review dimensions and are listed once
above:

- Toroidal wrap-seam (Rust viewport_window + render UV): sim, SAB, render, parallelism
  → majors #2 and #3.
- `viewport_w/h` unused: sim, SAB, render, parallelism → minor #5.
- `unsafe` split-borrow: sim, SAB, render, parallelism → minor #10.
- Pyramid full-recompute: sim, SAB, render, parallelism → minor #11.
- Biome window recomputed each tick: sim, SAB, render, parallelism → minor #12.
- Dead `biome_buf`: sim, SAB, render → minor #8.
- Scatter cross-tile RMW races: all four confirmed SAFE → not listed as a finding.
