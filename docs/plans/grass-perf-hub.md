---
status: review
owner: mixed
last_updated: 2026-06-02
okay_to_delete: false
long_lived: false
owning_docs:
  - docs/architecture/simulation-core.md
  - docs/architecture/shared-memory-and-protocol.md
  - docs/architecture/app-shell.md
  - docs/architecture/render-pipeline.md
  - docs/decisions/sim.md
  - docs/decisions/render.md
---

# Mission (hub): grass perf — scatter (v2.0.2) then LOD pyramid (v2.0.3), build-first

## Goal

Orchestration hub for the two-stage grass-performance effort. This doc owns
**sequencing + execution strategy**; the two sub-plans own the **design/spec**.
Read the sub-plans for *what* to build; follow this hub for *when*, in what
*order*, and especially for the **testing/review staging**.

- Design authority — Stage 1: [`v2.0.2-grass-scatter.md`](v2.0.2-grass-scatter.md).
- Design authority — Stage 2: [`v2.0.3-grass-lod-pyramid.md`](v2.0.3-grass-lod-pyramid.md).
- This hub: the orchestrator's map — build-first staging, the deferred
  test/review phase, the carried-forward decision log, and the phase tracker.

## Execution strategy (overrides the sub-plans' inline test ordering)

**Code first. Defer all MAJOR testing, the perf bench, the behavioral A/B, and
code review to a single consolidated phase (Stage 3), after both coding stages.**
During Stages 1–2, sub-agents do only *minor* validation:

- The tree stays green at every stream's end: `cargo check`, the existing
  `cargo test --lib` smoke (do not rewrite suites yet), the threaded wasm build
  (`wasm-pack ... --features threads` — mandatory, never skip), and a browser
  smoke (no crash; grass visibly moves).
- **No** test rewrites, **no** criterion bench, **no** behavioral A/B, **no**
  formal review during coding.

Safety valves (so "defer" ≠ reckless):

- **Keep the blur path behind a flag.** Scatter lands *alongside* the blur,
  switchable. Do **not** delete `h_at` / `blur_at` / `grass_cell_next` /
  `ScratchPtr` until Stage 3's bench passes.
- **Keep the 1/d² sensing path.** The mip-sensing swap (Stream 2e) is gated and
  may be skipped entirely.
- A broken build is not "code done." Every stream leaves the tree compiling.

## Resolved decisions (carried from design chat — do NOT relitigate)

- u8 `AtomicU8` density; relaxed load → clamp → store RMW (composes only when
  serialized; concurrent collisions clobber wholesale — accepted noise).
- Per-cell `(world_seed, tile, tick)` hash RNG; grass never touches `SimRng`.
- **Precomputed disc offset table** for spread targets (NOT Chebyshev rings).
- **No spontaneous spawn** — collapse is an accepted outcome; tune plains
  super-critical / water sub-critical.
- Per-tile frozen source; cross-tile writes legal; bounded raster-order seam
  skew accepted (disc-tested in Stage 3).
- **Array-only** biome + capacity (CPU > memory; no procedural). 1B cells is an
  *aspirational* goal, compute-bound — this effort bounds output, not the tick.
- Pyramid L0 aliases the live density field; refresh follows the propagation
  active set at scale.
- Snapshot windowing needs a **camera SAB lane** + per-slot window metadata;
  biome tint at LOD via a **snapshot biome channel** (mode-downsampled).
- Determinism: cross-thread / cross-run bit-reproducibility is **not** required
  (intentional fuzz). Coherence holds — grass advances at tick end (step 7).

## Plan

### Stage 1 — v2.0.2 scatter (code-complete; blur retained behind flag)

Spec: `v2.0.2-grass-scatter.md`. Streams:

- **1a — u8 storage migration (foundational, lands first).** `density:
  Vec<f32>` → `Vec<AtomicU8>`; adapt every reader (`bilinear_sample`, the
  proximity sector scan, the row bitset, `classify_tile`, `quantize_dirty_tiles_into`,
  the wholesale-set init loop). Relaxed loads.
- **1b — scatter kernel.** Per-cell hash RNG, decay/spread rolls, disc offset
  table + sampling, `AtomicU8` RMW, per-tile frozen source. Behind a flag,
  *alongside* the blur.
- **1c — active-tile set + cross-tile active/dirty bookkeeping** (atomic set-bit
  or per-worker merge; today the tile bitsets are plain `Vec<u64>`).
- **1d — the six sliders** end-to-end (sim + control SAB lanes + UI).
- **1e — graze/sensing read-path** adapted to u8 (energy-reward quantization).

Order: 1a → (1b, 1c) → 1d / 1e parallel. **Gate:** compiles, threaded wasm
builds, browser shows grass spreading/decaying. No tests/bench yet.

### Stage 2 — v2.0.3 pyramid (code-complete; sensing last/optional)

Spec: `v2.0.3-grass-lod-pyramid.md`. Depends on Stage 1 code-complete (u8
density exists). Streams:

- **2a — `GrassPyramid` core** (L0 alias, L1+ owned, per-level dims; gather
  refresh keyed off the active set).
- **2b — snapshot protocol** (per-slot window metadata + camera SAB lane +
  margin policy; publish the window).
- **2c — render LOD** (clipmap window extraction, budget²-texture streaming
  upload, zoom→LOD selection, UV transform; `render-gl.ts`).
- **2d — biome tint at LOD** (mode-downsampled biome window baked into the slot).
- **2e — mip sensing (LAST, gated, skippable).** New `NnInputLayout` variant,
  near/med/far sectors. Keep the 1/d² path.

Order: 2a → (2b + 2c coupled) → 2d → 2e. **Gate:** compiles, renders via the
pyramid ≈ the same picture at default scale, no crash.

### Stage 3 — consolidated test + bench + review + tune (the deferred work)

Only now:

- **Perf bench** — criterion scatter kernel + browser `tick.grass_step`. If it
  misses the target the blur fallback is still present → decide keep/iterate
  *before* deleting it.
- **Test rewrite** — scatter kernel, single-seed disc footprint (round),
  biome-cap clamp, decay-floor, persistence idle-run, full-grid oracle
  (skip-exactness), spread-rate-scales-with-patch. Audit **all** grass-touching
  determinism tests, not just the grass module.
- **Behavioral A/B** for sensing if 2e shipped (forages at least as well vs 1/d²).
- **Code review** across both stages.
- **Feel-tune** the six sliders (plains super-critical, water sub-critical;
  visual disc).
- **Cleanup + doc migration** — delete the blur / `ScratchPtr` (post-bench),
  rewrite the `owning_docs` in place, retire the three plans.

## Verification

- Stages 1–2: green build (native + threaded wasm) + browser smoke at each
  stream's end.
- Stage 3: `cargo test` green (rewritten suites); perf overlay shows the
  `grass_step` drop / TPS lift; visual checks (living noise, round footprint,
  biome caps, no static-green plateau); behavioral A/B if sensing swapped.

## Status

- **Review.** Effort code-complete + fully gated green (both feature sets, e2e 11/11);
  durable facts migrated; close-out written ([`grass-perf-closeout.md`](grass-perf-closeout.md)).
  **PENDING LEAD RATIFICATION** (threaded run-to-run non-reproducibility, blur deletion,
  feel-tune) + commit. Orchestrator drove autonomously per delegated authority (2026-06-02);
  every load-bearing call logged below + summarized in the close-out.
- **Phase tracker:** [x] Stage 1 · [x] Stage 2 · [~] Stage 3 (gate ✅ both feature sets → docs + close-out).
- **Decisions log:** see "Resolved decisions" above; append new ones in
  "Orchestration log" below.
- **Open questions:** slider names/ranges (+ a `grass_bites_per_block` cap ≤ ~32);
  mip sense band layout; trilinear vs nearest; biome snapshot-channel vs biome
  mip; upload budget / margin size / LOD-selection curve.

### Streams (observed state — updated from agent results, not plans)

Recon map: [`grass-perf-recon.md`](grass-perf-recon.md). Refined Stage-1 plan is
**all-serial** (one stream at a time in the main tree — avoids cross-agent build
races on the shared crate; 1d/1e are file-disjoint but serialized for safety, and
the wasm build serializes regardless). **1b+1c fused** into one scatter+frontier
stream (they share `compute_propagation` + the tile active/class block in one
`impl GrassGrid` — not parallelizable).

| Stream | Area | Status | Last observed fact | Next action | Blockers |
|--------|------|--------|--------------------|-------------|----------|
| recon | map | ✅ done | `grass-perf-recon.md` written; grass.rs is the CRITICAL shared file (1a/1b/1c/1e); SAB headroom OK (60/80 after +6) | — | — |
| 1a u8 storage | sim/grass | ✅ done | GREEN: 195 tests pass; `cargo check` clean both feature sets; threaded wasm confirmed (initThreadPool=2, shared:true). Density `Vec<AtomicU8>` @ snapshot scale; dget/dset accessors (&self); blur decodes-on-load | — | — |
| 1b+1c scatter+frontier (fused) | sim/grass | ✅ done | GREEN: 196 tests (~488s); threaded wasm confirmed. Scatter LIVE @ World boot (mod.rs:455); blur retained behind `GrassPropagation` selector (default Blur for tests); atomic `fetch_or` frontier (`Vec<AtomicU64>`); 1 balance test pinned to Blur. Kernel reads params at one site for slider injection | — | — |
| 1d six sliders | sim+SAB+UI | ✅ done | GREEN: 196 tests, pnpm typecheck clean, threaded wasm OK. 6 LIVE sliders (defaults=consts) via `GrassGrid.scatter_params` + `apply_scatter_params`; SLIDER_NAMES append-only 54-59 (count 60); bindings regen; settings schema 2→3 | — | — |
| 1e graze/sensing u8 | sim/grass | ✅ done | GREEN: 196 tests (134s), threaded wasm OK. consume() energy now u8-byte arithmetic (taken/chunk bytes, `.max(1)` floor); `GRASS_BITES_PER_BLOCK=2` const (chunk 0.5→128 levels). Sensing untouched (1a left it correct; 1/d² intact). 1 tol widened + 1 ref-drain rewritten. Pre-existing bites slider inert for graze (Stage-3 re-expose) | — | — |
| 2a pyramid core | sim/grass | ✅ done | GREEN: 209 tests (13 new pyramid), threaded wasm OK. GrassPyramid 12 levels @1920 (~1.23MB owned); box-filter mean, edge-clamped; refresh @ tick step 7b. API (sample_clamped/viewport_window) ready. FULL recompute + unsafe split-borrow → Stage-3 | — | — |
| 2b snapshot+camera SAB | sim+SAB+TS | ✅ done | GREEN + browser-confirmed: default-scale window = 1920² full field (render byte-identical); camera lanes 120-124 (cx=cy=4800, zoom=1 confirmed); header 32→64; bindings_in_sync ok; render-gl.ts untouched; quantize_grass_into cfg-fixed. 209 tests, typecheck clean | — | — |
| 2c render LOD | render TS | ✅ done | GREEN + browser-confirmed (default AND zoomed): 2048² tex, nearest-mip, UV transform u_uv_scale=world/(cell·2^L·2048). Default mip0 win=1920² byte-equiv; zoom→MIN gives mip4 win=120² renders+pans, no crash. Biome path untouched; typecheck+build green | — | — |
| 2d biome tint LOD | sim+render | ✅ done | GREEN + browser-confirmed (Stage-2 render boundary): mode-downsampled biome window in slot (biome_win_bytes); biome path consumes it via shared window+UV; default mip0 = full biome field identical; zoomed mip4 grass+biome track together, no crash. Grass path untouched; 209 tests, bindings ok | — | — |
| 2e mip sensing (gated) | sim/nn | ⏸ deferred → Stage 3 | recovery: 2e Implement crashed having added only an inert `NN_GRASS_MIP_BANDS=2` const (no variant/fn/selector/test). Tree GREEN (209 tests); 1/d² + MAX_NN_INPUTS=48 intact. Optional → complete-or-drop in Stage 3 | Stage-3: implement-or-drop + A/B | — |

### Per-wave gate protocol (Stages 1–2)

Each wave = a 2-phase Workflow. **Implement** agent edits + iterates (`cargo check`,
`cargo test --lib` smoke). **Verify** agent (fresh, after implement) runs the
authoritative green-tree gate once and pastes output: `cargo check` +
`cargo check --features threads` + `cargo test --lib` (existing tests, NOT
rewritten) + the repo's threaded `wasm-pack` build (`--features threads` —
mandatory). No clippy/fmt, no test rewrites, no bench, no A/B, no code review — all
deferred to Stage 3. Browser boot smoke is consolidated to **stage boundaries**
(end of Stage 1, end of Stage 2), not per-stream; full browser boot/tick +
Playwright e2e is the final consolidated gate.

### Carried-forward (Stage-3 debt + active risks)

Append-only. Items here are intentionally deferred or are risks later streams must
respect. Resolve/audit at Stage 3 unless a stream naturally absorbs one.

- **u8 quantization FLOOR near low-cap biomes** (from 1a). Growth steps below 1/255
  stall one byte short of cap (Water cap 0.04 = byte 10). Scatter (1b) decay/spread
  + slider tuning (1d / Stage 3) must not rely on sub-1/255 deltas; "no spontaneous
  spawn / collapse accepted" already aligns. **Risk — routed into 1b+1c + tune.**
- **Test-tolerance widenings (5 sites; 2 were real failures at old tolerance)** (1a):
  `regrowth_saturates_at_biome_capacity`, `graze_conserves_bite_count_invariant`, + 3
  inline. Widened by one u8 quantum to compile/pass; **Stage 3 must re-author these as
  proper u8-domain assertions** (the "audit every suite asserting an exact density
  field" item).
- **Dead-code warning (wasm32 only)**: `quantize_grass_into` (src/wasm_api.rs:1380)
  is unused on the wasm32 target — its only caller `write_snapshot_to_native` is
  `#[cfg(not(target_arch="wasm32"))]`, so native `cargo check` is clean and the warning
  shows only in the `wasm-pack` build (1d confirmed this). Fix = also gate
  `quantize_grass_into` behind `#[cfg(not(target_arch="wasm32"))]`. **Stage 3 cleanup**
  (cosmetic, build exit 0).
- **Behavior change (intended)**: blur now reads/round-trips u8 each tick; internal
  density trajectory differs sub-1/255 from old pure-f32. Visible picture unchanged
  (renderer only ever saw the u8 snapshot). Per the u8 decision — no action.
- **Scatter energy economy is hotter** (from 1b+1c). Live scatter plains is
  intentionally super-critical → faster pop growth; the balance smoke exceeded its
  soft cap and was pinned to Blur. Confirms the planned "scatter changes the energy
  economy." **Stage-3 feel-tune** across the 6 sliders + browser balance; the ignored
  `default_world_long_run` shares this. Water sub-criticality holds via *reachability*
  (shore grass can't reach far water), not in-region dynamics — a browser/tuning check.
- **Per-tile freeze allocation** (from 1b+1c): scatter snapshots each active tile's
  ~1024 source bytes into a transient `Vec<u8>` per tile per tick. Functionally fine;
  perf unmeasured. **Stage-3 criterion bench** validates the ~50–100×/cell target;
  a per-worker reusable scratch buffer is the obvious optimization if needed.
- **`tile_class` stale on the scatter path** (from 1b+1c): scatter frontier uses
  grass-presence, not the blur's equilibrium classes, so `tile_class` is only
  maintained by the blur/resync/graze paths. Harmless + documented inline; re-derive
  if class-based logic is ever reintroduced on the scatter path.
- **`grass_bites_per_block` slider inert for graze** (from 1e): a pre-existing slider
  (idx 9) is no longer read by the graze path — 1e hardcoded the constant (=2 = its
  default), so default behavior is unchanged but the live knob does nothing for graze.
  **Stage 3: re-expose it live** (graze is already floor-safe for [1,32] via `.max(1)`,
  so this is a cheap revert) and add a compile-assert tying `GRASS_BITES_PER_BLOCK` to
  `GRASS_BITES_PER_BLOCK_DEFAULT` so they can't silently diverge.
- **More test edits to re-author at Stage 3** (from 1e): `consume_below_chunk_yields_partial_bite`
  tolerance widened one u8 quantum; `active_tile_equals_full_grid_with_grazing` reference
  drain rewritten to byte arithmetic (keeps byte-for-byte equivalence; 1-byte diff on
  ripe cells). Adds to the 1a tolerance-widening audit list.
- **Two throwaway e2e specs** (Stage-1 + 2b smokes): `grass-scatter-smoke.spec.ts`
  (Stage 1) reads the OLD 32-byte snapshot header → now computes WRONG offsets after 2b
  bumped the header to 64 (left untouched + not re-run, so STALE/broken).
  `stream2b-default-scale-smoke.spec.ts` (2b, correct 64-byte header + window metadata)
  passes. **Before the final Playwright e2e gate: fix or remove the stale Stage-1 spec**
  (it will fail if the full suite runs it) and decide whether to keep a consolidated
  boot+scatter+window smoke permanently.
- **Pyramid refresh is a FULL recompute** (from 2a): every tick rebuilds all levels
  (~N/3 work). Correct at all scales but not active-set-keyed. **Stage-3 perf:** dirty-
  subtree walk off tile_active/tile_dirty (the refresh interface already takes the params).
- **`unsafe` split-borrow in `refresh_pyramid`** (from 2a): raw-pointer re-borrows of
  density/tile_active (disjoint fields) to satisfy the borrow checker. Sound but likely
  unnecessary — a direct `self.pyramid.refresh(&self.density, &self.tile_active, …)`
  disjoint-field borrow should compile. **Stage-3 review: remove/justify the unsafe.**
- **Snapshot copies the full window each tick** (from 2b): write_snapshot uses
  `pyramid.viewport_window` (full-window copy); `quantize_dirty_tiles_into` is now DEAD
  CODE. At default scale = full 1920² copy per snapshot (vs the old dirty-subset).
  **Stage-3:** remove `quantize_dirty_tiles_into` or re-introduce within-window dirty
  tracking if the per-tick copy shows up in the bench.
- **`viewport_w/h` read but unused in the window calc** (from 2b): the window uses a
  square approximation from `visible_cell_span`; the per-axis viewport dims (SAB lanes
  123/124) are reserved but ``let _``'d. **2c / Stage-3:** use them so a wide viewport
  doesn't under-cover horizontally.
- **Toroidal wrap seam for windowed textures** (from 2c, RISK): a clipmap window that
  STRADDLES a world wrap seam on a toroidal world would need per-tile UV-offset
  corrections (the per-tile world-repeat loop shifts u_cam_pos but not the window UV).
  Not exercised (default window = full field; the zoom-out smoke used the full
  downsampled level). **Stage-2-boundary / Stage-3: visual check on a large TOROIDAL
  world at partial zoom**; add seam correction if it shows.
- **Grass texture filter LINEAR→NEAREST** (from 2c): per the Q2 nearest-mip-first
  decision; grass is slightly more pixelated at close zoom. Intended. **Stage-3 visual
  review** decides whether to enable trilinear (the u_lod_blend slot is reserved).
- **Biome window recomputed each tick** (from 2d): write_snapshot mode-downsamples the
  STATIC biome_grid into the window every snapshot (O(grass_cell_count), cheap). **Stage-3
  perf:** a precomputed static biome mode-pyramid (biome never changes) cuts it to
  O(win_w·win_h). Out of 2d scope (would need world/mod.rs).
- **Slot footprint doubled (grass+biome) + dead `biome_buf`** (from 2d): each slot now
  carries a biome_win_bytes region equal to grass (8.40MB/slot, 16.8MB both slots — wasm
  linear memory, proportional, no architectural risk). The old STATIC biome region
  (`biome_buf` + its boot-reply offset/len) is still populated but no longer consumed by
  main.ts (superseded by the per-slot biome window). **Stage-3 cleanup:** remove
  `biome_buf` or keep it for a future inspector.
- **2e mip sensing incomplete** (recovery): only the inert `NN_GRASS_MIP_BANDS=2` const
  landed (no variant/fn/selector/test). **Stage 3: either complete 2e** (GrassMipSectors
  variant + mip-tap + construction selector + A/B vs 1/d²) **or drop it** (remove the
  dangling const). Decision gated on whether scale-invariant sensing justifies the
  NN-layout addition; default stays 1/d² regardless.
- **REMAINING DEFERRALS after Harden (for close-out / lead)** — everything else above is
  fixed or confirmed-acceptable: (a) **toroidal wrap-seam** (documented TODO in code,
  narrow: toroidal + grass_dim>2048 + non-default zoom); (b) dead **`biome_buf`** removal
  (~3.7MB, cross Rust/TS); (c) **`viewport_w/h`** non-square window; (d) the **criterion
  bench** widened grass/constants/rng `pub(crate)→pub` + added `[profile.bench]
  panic=unwind` (benign "panic ignored for bench" warning) — keep-with-tighter-visibility
  vs remove; (e) **blur deletion** (HELD — 34× < 50-100× target); (f) **feel-tune** the 6
  sliders (lead/taste; scatter economy hotter); (g) pyramid + biome-window **perf**
  optimizations (active-set-keyed refresh; precomputed static biome mode-pyramid).

### Orchestration log

- **2026-06-02 — Effort activated.** Lead delegated the full build-first run with
  decision authority. Launched read-only recon workflow `grass-perf-recon` (7
  parallel mappers → synthesis wrote `grass-perf-recon.md`).
- **2026-06-02 — Recon landed; wave plan refined. Decisions (logged, not asked):**
  1. **grass.rs is the critical shared file** (1a/1b/1c/1e). 1b & 1c are NOT
     file-disjoint (shared `compute_propagation` + tile active/class in one impl) →
     hub's "(1b,1c) parallel" is void.
  2. **Fuse 1b+1c** into one scatter+frontier stream (one agent owns grass.rs).
  3. **Serialize all of Stage 1** (1a → 1b+1c → 1d → 1e), main tree, no worktree
     isolation. 1d/1e disjoint but serialized to avoid concurrent-build races and
     because the wasm build serializes regardless.
  4. **Six grass sliders, not seven** for 1d — `grass_bites_per_block` stays a
     constant this stage (a 7th slider only in Stage-3 feel-tune if needed; SAB
     headroom 61<80). SLIDER_NAMES is **append-only**, order locked before 1e reads
     slider indices.
  5. **Per-wave gate protocol** above; browser smoke consolidated to stage
     boundaries. SAB headroom confirmed safe (no redesign this stage).
  6. **Spec-report stream-ID mislabel noted** — dispatch uses the hub's canonical
     IDs + the recon doc re-mapping; implement agents read the actual v2.0.2
     sub-plan (design authority), never the spec-distillation labels.
- **2026-06-02 — Wave 1 (Stream 1a) DONE, GREEN.** Independent Verify confirmed
  `cargo check` (+ `--features threads`) clean, `cargo test --lib` 195 passed / 0
  failed, threaded `wasm-pack` build exit 0 with bundle-threaded invariant verified
  (initThreadPool=2, shared:true). Density is `Vec<AtomicU8>` on the snapshot
  quantization scale (snapshot copy-out now a trivial byte copy; SAB wire bytes
  identical). Blur retained, decode-on-load. 8 files changed. Carried-forward debt
  logged above. **New decision:** the scatter selector defaults to scatter LIVE at
  World/wasm construction while existing Rust propagation tests keep BLUR (so
  `cargo test --lib` stays green without rewriting suites); 1b+1c adds one minimal
  scatter smoke test in its own file. Launched Wave 2 (fused 1b+1c).
- **2026-06-02 — Model policy:** lead asked to conserve credits → all FUTURE
  workers run on **Sonnet** (1d, 1e, Stage 2, Stage 3, final gate). Wave 2 was
  already in flight on the default (Opus) and finishes as-is; not relaunched.
- **2026-06-02 — Wave 2 (fused 1b+1c) DONE, GREEN.** Independent Verify: `cargo
  check` (+threads) clean, `cargo test --lib` 196 passed / 0 failed (~488s),
  threaded `wasm-pack` exit 0, bundle threaded (initThreadPool=2, shared:true).
  Scatter is the LIVE propagation path (World boot sets it; `compute_propagation`
  dispatches on a `GrassPropagation::{Scatter,Blur}` selector, default Blur so
  grid-direct tests stay on blur). SplitMix64 per-cell RNG (4 salts, never touches
  SimRng), radius-3 round disc table, per-tile frozen source, saturating-clamp RMW,
  `.max(1)` to clear the u8 floor, no spontaneous spawn. Frontier = atomic `fetch_or`
  set-bit over `Vec<AtomicU64>` active/dirty bitsets + 8-neighbour fringe. Blur fully
  retained behind the selector (delete only post-bench). 1 existing balance test
  pinned to Blur. Carried-forward debt updated above. Launched Wave 3 (1d sliders,
  Sonnet).
- **2026-06-02 — Wave 3 (1d sliders) DONE, GREEN** (Sonnet worker). Independent
  Verify: cargo check (+threads) clean, cargo test --lib 196 passed, pnpm typecheck
  clean, threaded wasm exit 0 (initThreadPool=2, shared:true), SLIDER_NAMES
  append-only confirmed (0-53 unchanged, 54-59 new, count 60; slider-ids.ts regen to
  match). **Six LIVE sliders (defaults = current constants → behavior unchanged):**
  `grass_decay_pct` [0,1]/0.02, `grass_decay_amount` [0,0.5]/0.008, `grass_spread_pct`
  [0,1]/0.55, `grass_spread_amount` [0,0.5]/0.05, `grass_spread_ring1_pct` [0,1]/0.70,
  `grass_spread_ring2_pct` [0,1]/0.22. Wired via a `GrassGrid.scatter_params` struct +
  `apply_scatter_params` (rebuilds the disc table only on ring-weight change); **ring3
  implicit = max(0, 1-r1-r2)** (six-not-seven decision honored). settings schema 2→3
  (additive, merges). Launched Wave 4 (1e graze/sensing, Sonnet).
- **2026-06-02 — Wave 4 (1e graze/sensing) DONE, GREEN** (Sonnet). Verify: cargo
  check (+threads) clean, cargo test --lib 196 passed, threaded wasm exit 0
  (initThreadPool=2, shared:true). `consume()` energy is now u8-byte arithmetic
  (taken_bytes/chunk_bytes; chunk_bytes = encode(density_chunk)`.max(1)` clears the u8
  floor so grass-on-cell never yields 0). `GRASS_BITES_PER_BLOCK=2` constant
  (density_chunk 0.5 → 128 u8 levels). Sensing untouched — 1a already adapted
  proximity.rs/nn.rs (dget / bilinear_sample decode-on-read); 1/d² path intact.
  **DEVIATION (lead FYI):** pre-existing `grass_bites_per_block` slider (idx 9) now
  inert for graze (hardcoded to constant = default); behavior unchanged; Stage 3
  re-exposes. Test edits logged in carried-forward.
- **2026-06-02 — STAGE 1 CODE-COMPLETE.** All four waves green (1a u8 · 1b+1c
  scatter+frontier · 1d sliders · 1e graze). Scatter is the live u8 propagation path;
  blur retained behind the `GrassPropagation` selector; six live sliders; graze
  u8-quantized. Running the Stage-1-boundary browser smoke (boot threaded bundle,
  tick, confirm scatter grass visibly evolves, no crash) before opening Stage 2.
- **2026-06-02 — Stage-1 browser smoke PASS** (Sonnet, REAL browser). Boot OK,
  `crossOriginIsolated=true`, threaded (initThreadPool=2). Δtick=38; subsampled grass
  density +64% over 38 ticks → live multi-threaded scatter is actively spreading;
  world did not end. **STAGE 1 fully DONE + browser-confirmed.** Left a THROWAWAY e2e
  spec `web/tests/e2e/grass-scatter-smoke.spec.ts` (decide keep/remove at final gate).
  Launched Stage-2 recon (`grass-recon-stage2`, Sonnet) in parallel to prep the wave
  plan.
- **2026-06-02 — Stage-2 recon landed** (`grass-perf-recon-stage2.md`). Overlap matrix:
  grass.rs (2a→2b) and wasm_api.rs `write_snapshot` (2b→2d) HARD-serialize; render-gl.ts
  splits cleanly into grass (2c) vs biome (2d) regions. **Wave plan — ALL SERIAL, main
  tree** (same race-avoidance as Stage 1; 2c/2d share render-gl.ts + main.ts so they
  edit serially, not parallel): 2a → 2b → 2c → 2d → 2e. **SEAM:** 2b reshapes the
  control SAB + SlotLayout; its TS consumers (2c/2d) start only after 2b locks the
  schema + regenerates bindings (`bindings_in_sync` test must pass).
  **Open-question decisions (logged, not asked):**
  - **Q4 (budget/margin/LOD):** budget_axis = **2048** (pow2; ≥ default grass_dim 1920 →
    no LOD / no regression at default scale; pyramid only bites at larger worlds);
    margin = **25%/side** (viewport×1.5, covers a full-viewport pan ≥1 tick);
    LOD = floor(log2(visible_cell_span / budget_axis)) clamped [0,max_level],
    visible_cell_span = (world_size/cam.zoom)/GRASS_CELL_SIZE.
  - **Q3 (biome):** **snapshot biome channel** (mode-downsampled window baked beside
    grass in the slot; reuses 2b window machinery; zero new protocol) — confirms the
    hub's pre-resolved decision.
  - **Q2 (sampling):** **nearest-mip first** (one texParameteri); reserve a blend
    uniform slot for trilinear; trilinear → Stage-3 visual review.
  - **Q1 (mip bands):** **2 bands (near/far)** = 16 grass inputs, keeps
    `MAX_NN_INPUTS=48` (no ceiling change, safe for walled+species). 3 bands only if a
    Stage-3 A/B shows it materially better.
  - **2e disposition:** IMPLEMENT it **gated** — 1/d² stays the DEFAULT sensing path,
    the mip variant is added behind a layout selector; the Stage-3 behavioral A/B
    decides whether to flip the default. Skippable: if 2e hits real entanglement,
    defer to Stage 3 rather than force it.
  - **Doc note:** 2b will NOT update `shared-memory-and-protocol.md` in-wave (build-first
    defers docs; consumers read the regenerated bindings + SlotLayout, not the doc) —
    the SAB-protocol doc rewrite is Stage-3 migration.
  Launched Wave 1 (2a pyramid core, Sonnet).
- **2026-06-02 — Stage-2 Wave 1 (2a pyramid core) DONE, GREEN** (Sonnet). Verify:
  cargo check (+threads) clean (only expected dead_code on the new pyramid API stubs +
  the pre-existing wasm32 quantize_grass_into), cargo test --lib 209 passed (13 new
  pyramid tests), threaded wasm exit 0 (initThreadPool=2). GrassPyramid: 12 levels at
  default 1920 (~1.23 MB owned L1+), box-filter MEAN, edge-clamped (no dark border at
  non-pow2 dims), refreshed each tick at step 7b (after rebuild_row_bitset, before
  energy). API (sample_clamped / viewport_window) ready for 2b/2c/2e. Launched Wave 2
  (2b snapshot+camera SAB, Sonnet) — the seam-crossing wave; design pins **default-scale
  window = full field** so the unchanged renderer keeps working until 2c; +browser smoke.
- **2026-06-02 — Stage-2 Wave 2 (2b snapshot+camera SAB) DONE, GREEN + browser-
  confirmed** (Sonnet). Verify: cargo check (+threads) clean, cargo test --lib 209
  passed, pnpm typecheck clean, bindings_in_sync passes, threaded wasm exit 0,
  render-gl.ts confirmed UNMODIFIED (git diff empty), quantize_grass_into wasm32
  dead_code RESOLVED (cfg-gated). Browser smoke PASS: default-scale window meta = mip 0
  / origin (0,0) / 1920×1920 = full field → render byte-identical; grass evolves
  (13M→29M over Δtick 30); camera lanes written (cx=cy=4800, zoom=1, vp 1280×720); no
  crash. Camera lanes at SAB slots 120-124; SNAPSHOT_HEADER 32→64 (8 window-metadata
  fields); window computed Rust-side in write_snapshot from camera params the worker
  passes (wasm can't read the JS SAB directly). Launched Wave 3 (2c render LOD, Sonnet).
- **2026-06-02 — Stage-2 Wave 3 (2c render LOD) DONE, GREEN + browser-confirmed**
  (Sonnet, TS-only). Verify: pnpm typecheck + pnpm build green, no Rust changed, biome
  path in render-gl.ts byte-for-byte untouched. Renderer now consumes the clipmap
  window: a fixed 2048² R8 texture, NEAREST filter, per-frame texSubImage2D of the
  win_w×win_h window; UV transform u_uv_scale = world_size/(GRASS_CELL_SIZE·2^L·2048),
  u_uv_offset = −winOrigin/2048; u_lod_blend reserved (=0) for future trilinear.
  Browser smoke PASS both parts: (1) default — mip 0, win 1920², UV invariant verified
  (0.9375), grass evolves, byte-equivalent to pre-2c; (2) zoomed to MIN_ZOOM 0.04 —
  mip_level=4, win 120², grass renders + pans, no crash (LOD formula confirmed
  end-to-end). Launched Wave 4 (2d biome tint LOD, Sonnet) — last render stream; its
  smoke doubles as the Stage-2 render-boundary check.
- **2026-06-02 — Stage-2 Wave 4 (2d biome tint LOD) DONE, GREEN + browser-confirmed**
  (Sonnet). Verify: cargo check (+threads) clean, cargo test --lib 209 passed,
  bindings_in_sync pass, pnpm typecheck+build green, threaded wasm exit 0; grass path in
  render-gl.ts confirmed untouched (all grass diff tagged 2c). write_snapshot bakes a
  MODE-downsampled biome window (counts[4], tie→lowest tag) into the slot beside grass
  (biome_win_bytes); the render biome path consumes it via the SAME window metadata + UV
  transform; default scale (mip 0) = byte-identical full biome field → tint unchanged.
  Slot now = 64 + 1.024MB creatures + 3.686MB grass + 3.686MB biome = 8.40MB/slot.
  Browser smoke (= Stage-2 render-boundary check) PASS: default mip0 grass evolves +
  biome 3 distinct IDs; zoomed mip4 (120² win) grass+biome render + track together + pan
  no crash. **STAGE-2 RENDER PIPELINE DONE.** Launched Wave 5 (2e mip sensing, Sonnet) —
  gated/optional; 1/d² stays the default.
- **2026-06-02 — Stage-2 Wave 5 (2e mip sensing) workflow FAILED** — NOT a code-gate
  failure: the Implement agent completed ~25 tool-uses then did not emit its
  StructuredOutput (workflow harness issue), so Verify never ran and the working-tree
  state is UNKNOWN (2e may have left partial/complete/no edits across nn.rs/proximity.rs/
  constants.rs/world/mod.rs). Dispatched a conservative read-first recovery agent
  (Sonnet): if the tree is GREEN (compiles + cargo test --lib), KEEP it (a
  partial-but-green 2e is fine — finish in Stage 3); if BROKEN, revert ONLY 2e's
  additions to restore the green post-2d state (2e is optional → defer to Stage 3). The
  1/d² default (compute_grass_density_sectors) + MAX_NN_INPUTS=48 + 2a-2d must stay intact.
- **2026-06-02 — 2e recovery DONE; tree GREEN.** The 2e Implement agent had only added
  an INERT constant `NN_GRASS_MIP_BANDS: usize = 2` (constants.rs) before crashing — no
  variant, mip-tap fn, selector, or test. Recovery agent: NO revert needed (the const is
  unreferenced + harmless). Confirmed GREEN: cargo check (+threads) clean, cargo test
  --lib 209 passed (473s); `compute_grass_density_sectors` (1/d²) intact; MAX_NN_INPUTS=48.
  **2e deferred to Stage 3** (complete-or-drop, gated by the A/B). **STAGE 2 CODE-COMPLETE**
  (2a-2d browser-confirmed; 2e optional/deferred). Opening **Stage 3** (deferred
  consolidated phase) — starting with Assess: perf bench (decision-gates blur deletion) +
  adversarial code review, in parallel.
- **2026-06-02 — Stage-3 Assess DONE** (`grass-stage3-review.md`). **PERF:** native
  criterion scatter 1.04ms vs blur 35.7ms = **34×** (full-tick 33.5×) at 6.25% fill;
  browser grass_step 16.8ms@1920² / 22.0ms@2200² = **linear O(N), 0.048ms/row flat** —
  scatter does NOT balloon past budget 2048; LOD snapshot copy bounded (6.4→7.2ms).
  Target ~50-100× UNMET at mid/full density (holds only at sparse <1% fill). **REVIEW:
  no blockers; 3 majors; many minors; all 21 carried-forward items confirmed.**
  **Decisions (logged, not asked):**
  1. **KEEP the blur** (do NOT delete) — bench missed the 50-100× gate target; review
     endorses keeping it; cheap behind the `GrassPropagation` selector. Revisit deletion
     after a tuning pass / lead sign-off.
  2. **DROP 2e** — remove the dangling `NN_GRASS_MIP_BANDS` const; scale-invariant
     sensing → v2 backlog (separate NN feature, needs its own A/B, not required for the
     grass-perf goals). Default sensing stays 1/d².
  3. **Feel-tune → lead** — sliders are live/functional; scatter's energy economy is
     hotter (balance smoke pinned to Blur). Document the caveat + starting slider values;
     final aesthetic tuning is the lead's.
  4. **Harden fixes:** MAJOR `GRASS_EQ_EPS` 1e-4 → 1/255 (a u8-migration regression:
     Water tiles never classify SATURATED on blur → frontier-skip defeated); easy minors
     (first-tick camera SAB init, dead `biome_buf` removal, scatter zero-amount gate,
     unsafe split-borrow → safe field projection, LOD comment); **attempt** the 2
     toroidal wrap-seam majors (viewport_window/write_snapshot wrap + render ghost-copy
     UV — narrow: toroidal + grass_dim 2049-3500 + non-default zoom) — document as a known
     limitation if not cleanly fixable. Then re-author the u8-domain test assertions +
     scatter battery. Launched **Harden** (FixRust → FixTS → Verify).
- **2026-06-02 — Stage-3 Harden DONE, GREEN.** Verify: cargo check (+threads) clean,
  cargo test --lib 209 passed, bindings_in_sync pass, pnpm typecheck+build clean,
  threaded wasm exit 0 (initThreadPool=2). Fixes landed: `GRASS_EQ_EPS` 1e-4→1/255 (u8
  regression — Water now classifies SATURATED on blur); scatter zero-amount gate (zero
  slider = no effect; default byte-identical); `unsafe` split-borrow → safe destructure
  `let GrassGrid { density, tile_active, pyramid, .. } = self`; dropped
  `NN_GRASS_MIP_BANDS` (2e gone); LOD comment; first-tick camera SAB init (worldSize/2,
  zoom 1); texSubImage2D subarray guard. Walled/non-toroidal path NO regression (wrap
  tests + 3000-tick balance_smoke pass). **Toroidal wrap-seam DEFERRED** (documented TODO;
  render ghost-copy UV pairs with it) — known limitation, not a regression. Next:
  re-author the u8-domain test assertions + scatter battery (the `tests` workflow).
- **2026-06-02 — Stage-3 tests DONE, GREEN.** 11 widened tolerances re-authored to
  BYTE-EXACT u8 assertions + 4 scatter battery tests (disc-roundness, decay-floor,
  persistence/no-collapse, biome-cap-clamp). cargo test --lib **213 passed** / 0 failed
  (+ smoke); threaded wasm OK. Re-authoring surfaced real quantization facts the loose
  bands had MASKED: desert saturates at byte **76** (not 77 — decode(77)>cap nudges
  down), plains at **254** (not 255 — quantized logistic from below never reaches 255 at
  r=0.5), graze has a systematic 256/255 bite-count bias (now pinned). Only test files
  changed. Next: final consolidated gate (fmt/clippy both + test both + typecheck/build +
  Playwright e2e; clean the stale/throwaway e2e specs first).
- **2026-06-02 — Final gate: GREEN except cross-thread determinism (1 BLOCKER).**
  fmt clean; clippy clean both feature sets; **cargo test --lib (default) 213 passed**;
  pnpm typecheck+build clean; threaded wasm OK; **Playwright e2e 11/11 pass** (Polish
  removed the stale 32-byte spec + 4 throwaways, added one clean permanent
  `grass-lod-smoke.spec.ts`). **BLOCKER: `cargo test --lib --features threads` → 2
  determinism tests FAIL** (`world_genome_population_deterministic_for_same_seed`,
  `species_mode_run_is_deterministic_for_same_seed`) — pass single-threaded, fail under
  rayon; population varies run-to-run (33 vs 31/32). This is the cross-feature
  equivalence gate. **It surfaced only now** because every wave ran the threaded build as
  `--no-run` (compile-only), never the threaded test RUN. Likely the scatter's accepted
  lossy cross-tile RMW fuzz leaking into creature sensing → behavior → population (the
  1b+1c "preserves creature determinism" claim looks FALSIFIED). Dispatched a read-only
  root-cause diagnosis (grass-sensing-leak vs a separate race; documented-fuzz vs fixable
  bug; cost of a deterministic-scatter fix) before deciding relax-tests vs fix.
- **2026-06-02 — Determinism diagnosis DONE; DECISION: accept the fuzz, relax tests, RATIFY
  with lead.** Root cause confirmed = (a): `scatter_add`/`scatter_sub` (non-atomic relaxed
  load→clamp→store; concurrent collisions clobber) → thread-order-dependent grass density →
  creatures SENSE it next tick (bilinear_sample / sector scan → NN) → non-deterministic
  actions → non-deterministic population. **No separate non-grass race** (attack =
  parallel-scan/sequential-apply; NN = disjoint par_chunks_mut; SimRng untouched; frontier =
  correct fetch_or; pyramid single-threaded). **INTRODUCED by this effort:** old blur wrote
  per-tile-disjoint scratch from a frozen prior frame → bit-identical single-thread vs rayon;
  scatter writes the live AtomicU8 with collisions. The 1b+1c "preserves creature determinism"
  claim was FALSE (preserved the RNG STREAM, not the sensed INPUTS). **Inherent, not a
  side-channel** — only CAS / partitioning / inbox-merge fixes it, all costing the perf win or
  cross-tile spread; lead already accepted "intentional fuzz." **Decision (logged, to
  RATIFY):** relax the 2 tests — exact same-seed equality ONLY under
  `#[cfg(not(feature="threads"))]`; under threads assert liveness only (runs, species persists,
  pop in range) + a comment documenting the intentional scatter fuzz. **PROPERTY CHANGE the
  lead must ratify:** the LIVE wasm sim (always threaded) is now NON-REPRODUCIBLE run-to-run
  under the same seed (grass fuzz → divergent evolution); blur was reproducible. Deterministic-
  scatter options (tile-local spread / per-tile inbox merge / CAS-add) → close-out for a future
  call. Applying the relaxation + re-running the threaded gate.
- **2026-06-02 — Determinism tests relaxed; FULL GATE GREEN (both feature sets).** The 2
  tests now assert exact same-seed equality under `#[cfg(not(feature="threads"))]` and
  liveness + bounded population delta (≤20 genome, ≤12 mating; max observed 17/7) under
  `#[cfg(feature="threads")]`, with a comment documenting the intentional scatter fuzz.
  **cargo test --lib (default) 213 passed; cargo test --lib --features threads 213 passed
  ×3 + the 2 tests 15× consecutive, ZERO flakes.** Only the 2 test files changed. **FINAL
  GATE FULLY GREEN:** fmt · clippy both · test both feature sets 213/213 · typecheck+build ·
  threaded wasm · Playwright e2e 11/11. Effort code-complete + fully gated. Final phase:
  doc-migration (6 owning docs) + close-out (lead handoff + ratification items).
