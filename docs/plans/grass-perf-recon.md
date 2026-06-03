---
status: active
owner: orchestrator
last_updated: 2026-06-02
okay_to_delete: false
long_lived: false
owning_docs:
  - docs/plans/grass-perf-hub.md
---

# Grass-perf recon: orchestrator MAP (Stage 1)

Synthesis of 7 recon reports (1a density-storage, 1b blur/scatter, 1c tiles,
1d sliders, 1e graze/sensing, central-collision map, spec distillation).
Authority for *what* to build = the two sub-plans; this doc = *who touches what*,
*serialize vs parallel*, and *scope fences*. Hub ([`grass-perf-hub.md`](grass-perf-hub.md))
owns sequencing + resolved decisions (do NOT relitigate those here).

**Spec-vs-stream naming caveat.** The spec-distillation report's per-stream
`what_to_build` is MIS-LABELED relative to the hub's stream IDs (it lists u8
storage under "1d", scatter under "1b", disc-table under "1c", tests under
"1e"). This MAP uses the HUB's canonical IDs: **1a=u8 storage, 1b=scatter
kernel, 1c=active-tile set, 1d=six sliders, 1e=graze/sensing u8**. The
"What-to-build" section below is re-mapped to hub IDs.

---

## 1. Per-file OVERLAP MATRIX (Stage-1 streams 1a/1b/1c/1d/1e)

| File | 1a | 1b | 1c | 1d | 1e | Collision risk |
|------|----|----|----|----|----|----------------|
| `src/grass.rs` | **W (core)** | **W (core)** | **W (core)** | — | R (post-1a) | **CRITICAL — 3 writers, one struct/impl block. SERIALIZE 1a→1b→1c.** |
| `src/world/tick.rs` | W (graze closure) | R (graze marks active) | R (graze consume) | — | **W (graze energy)** | HIGH — 1a + 1e both edit graze. 1e owns it post-1a. |
| `src/world/proximity.rs` | **W (sector scan u8)** | — | R | — | R (sensing) | MED — 1a owns the u8 conversion; 1e reads after. |
| `src/world/nn.rs` | W (sampler u8) | — | — | — | R (sensing) | LOW — 1a converts sampler; 1e reads. (test writer in 1a) |
| `src/world/mod.rs` | **W (init loop l.454)** | R (tick order, unchanged) | R (tick order) | **W (DevSliders struct)** | — | MED — 1a edits init loop; 1d adds DevSliders fields. Different regions (init body vs struct decl) but SAME FILE → coordinate / serialize 1a then 1d. |
| `src/constants.rs` | R | R/W (scatter consts) | R (tile consts) | **W (SLIDER_NAMES append)** | R (energy consts) | MED — 1d appends SLIDER_NAMES (load-bearing order); 1b may add scatter consts. Append-only → low real conflict if 1d lands first. |
| `src/wasm_api.rs` | W (quantize read u8) | — | — (snapshot unchanged) | **W (apply_* + dispatch)** | R (slider idx 8) | MED — 1a edits quantize read; 1d adds apply arms + names. Different fns; SAME FILE → serialize or fence by region. |
| `src/control_sab.rs` | — | — | — | **W/verify (assertion)** | — | LOW — 1d-only; assertion passes (see Blockers). |
| `src/rng.rs` | — | R/W (hash fn) | — | — | — | LOW — 1b-only. |
| `web/src/generated/slider-ids.ts` | — | — | — | **W (gen)** | — | 1d-only (auto-gen). |
| `web/src/generated/control-sab.ts` | — | — | — | **W (gen)** | — | 1d-only (auto-gen). |
| `web/src/widgets/devpanel.ts` | — | — | — | **W (SliderSpec ×6)** | — | 1d-only. |
| `web/src/settings.ts` | — | — | — | **W (DEFAULTS ×6)** | — | 1d-only. |
| `web/src/sim-bridge.ts` | (comment only) | — | — | (generic, no change) | (no change) | None — forward-ready for u8. |
| `web/src/sim-worker.ts` | — | — | — | (generic on SLIDER_COUNT) | — | None. |
| `web/src/render-gl.ts` | (R8 ready) | — | — | — | — | None. |
| Test files† | **W (44 writes)** | — | R | — | — | 1a owns all density-write test adaptation. |

† Test files with direct density writes (all 1a-owned): `src/grass.rs` (inline
tests), `src/world/tick.rs` (graze tests), `src/grass_v201_tests.rs`,
`src/world/grass_biome_tests.rs`, `src/world/nn.rs` (one test).

**Legend:** W=writes, R=reads/depends, — =untouched. Bold = primary owner.

### The one-line truth
`src/grass.rs::GrassGrid` (density field @ l.147 + `compute_propagation` @ l.609
+ `classify_tile` @ l.442 + `consume` @ l.893 + tile bitsets) is ONE struct in
ONE `impl` block. 1a (retype density + adapt readers), 1b (replace kernel body),
1c (rewrite tile bookkeeping) ALL edit overlapping lines/symbols here. **There is
no clean impl-block partition — they SERIALIZE.**

---

## 2. Stage-1 WAVE PLAN (hub order, refined by file-disjointness)

Hub intent: `1a → (1b, 1c) → (1d, 1e)`. Refinement by ACTUAL central-map
disjointness **tightens** it: 1b and 1c CANNOT parallelize (both rewrite the same
`compute_propagation`/`classify_tile`/tile-state surface in grass.rs). 1d and 1e
CAN parallelize (disjoint file sets once 1a + the SLIDER_NAMES order are locked).

| Wave | Streams | Parallel? | Why |
|------|---------|-----------|-----|
| **W1** | **1a** | No (alone) | Foundational. Retypes `density: Vec<f32>→Vec<AtomicU8>` (grass.rs:147) + every reader + 44 test writes. 1b/1c/1e all read the post-1a density API. Must land + compile first. |
| **W2a** | **1b** | No (serial w/ 1c) | Replaces the kernel body inside `compute_propagation`/`tile_body` (grass.rs:609,648) + deletes nothing yet (blur stays behind flag). Same lines 1c rewrites. |
| **W2b** | **1c** | No (serial after 1b) | Rewrites tile_active/tile_dirty/tile_class bookkeeping + `classify_tile` + active-set rebuild — same `impl GrassGrid`, same `compute_propagation` 1b just touched. **Edits collide → serialize 1b then 1c (or fuse into one stream).** |
| **W3** | **1d**, **1e** | **Yes (parallel)** | Disjoint file sets. 1d = SLIDER_NAMES/SAB/UI/TS (no grass.rs core edits). 1e = graze energy + sensing read-path (tick.rs, proximity.rs, nn.rs reads). **Precondition:** 1a done AND SLIDER_NAMES order locked (1d's append finalized) so 1e's `set_slider_by_index` reads are index-stable. wasm_api.rs is touched by both (1d adds apply arms; 1e only READS slider idx 8) → 1e makes no wasm_api.rs edit, so no write-write conflict. |

**SERIALIZE rule applied:** same-file writers on overlapping symbols ⇒ serial.
**PARALLELIZE rule applied:** disjoint write sets ⇒ parallel.

### Critical refinement vs hub
- Hub says "(1b, 1c)" as if parallel. **They are NOT file-disjoint** — both
  writers own `compute_propagation` + the tile-class/active-set logic in grass.rs.
  **Recommendation: run 1b then 1c serially, OR fuse 1b+1c into a single
  scatter-kernel-plus-frontier stream** (the spec couples them: scatter's
  active-set is "tiles with grass OR within range z", a frontier rule 1c owns).
  Fusing avoids a mid-rewrite handoff on the same function.
- Hub says "1d / 1e parallel" — **confirmed disjoint, keep parallel.**

### Cross-wave file coordination (same-file, non-overlapping regions)
- `src/world/mod.rs`: 1a edits the init loop (l.454); 1d adds DevSliders fields
  (l.73+). Non-overlapping but same file → 1a (W1) lands before 1d (W3): no
  conflict by ordering.
- `src/wasm_api.rs`: 1a edits `quantize_grass_into` (l.1328); 1d adds apply_*
  arms + names. 1a in W1, 1d in W3 → ordered, no conflict.
- `src/constants.rs`: 1b may add scatter consts; 1d appends SLIDER_NAMES. If 1b
  (W2) and 1d (W3) are ordered, append-only edits don't collide.

---

## 3. Per-stream SCOPE FENCES

### 1a — u8 storage (W1, alone)
- **MAY touch:** `src/grass.rs` (density field l.147 → `Vec<AtomicU8>`;
  `new_with_capacity` init l.265/288; `bilinear_sample` l.838; `classify_tile`
  l.442; `rebuild_row_bitset` l.322; `consume` l.893; `quantize_dirty_tiles_into`
  l.792; all inline tests), `src/world/proximity.rs`
  (`compute_grass_density_sectors` l.407), `src/world/mod.rs` (init loop l.454),
  `src/world/nn.rs` (sampler + one test), `src/wasm_api.rs`
  (`quantize_grass_into` l.1328, `write_snapshot_to_native`), `src/world/tick.rs`
  (graze density_chunk quantization + graze test writers),
  `src/grass_v201_tests.rs`, `src/world/grass_biome_tests.rs`.
- **MUST NOT touch:** nothing in-flight (alone in W1). Do NOT pre-emptively
  rewrite the kernel body (1b) or tile bookkeeping (1c) — keep edits to the
  type migration + read adaptation only.

### 1b — scatter kernel (W2a, after 1a)
- **MAY touch:** `src/grass.rs` — `compute_propagation`/`tile_body` kernel body
  (l.609/648/662), add scatter impl (per-cell hash RNG, decay/spread RMW on
  AtomicU8, disc-table sampling), the blur-vs-scatter FLAG; `src/constants.rs`
  (scatter consts); `src/rng.rs` (per-cell hash fn).
- **MUST NOT touch:** the density field DECLARATION (owned by 1a, now landed —
  read it, don't retype); 1c's tile_active/tile_dirty/tile_class STRUCTURE +
  active-set rebuild rules (coordinate — 1c lands right after); `h_at`/`blur_at`/
  `grass_cell_next` must be RETAINED behind the flag (hub safety valve), not
  deleted.

### 1c — active-tile set (W2b, after 1b)
- **MAY touch:** `src/grass.rs` — `tile_active`/`tile_active_next`/`tile_dirty`/
  `tile_class` (atomic set-bit or per-worker merge), `classify_tile` (l.442),
  `rebuild_active_from_class` (l.483), `mark_tile_active` (l.416),
  `resync_active_from_density` (l.431), `activate_tile_and_fringe`,
  next-tick active-set build in `compute_propagation` (l.763-775),
  `quantize_dirty_tiles_into` dirty-lifecycle (l.792); `src/grass_v201_tests.rs`
  equivalence tests; `src/wasm_api.rs` `quantize_dirty_tiles_into` CALL (l.614,
  no signature change expected).
- **MUST NOT touch:** the scatter kernel BODY 1b just wrote (read it, build the
  frontier around it); density field type (1a's).

### 1d — six sliders (W3, parallel w/ 1e)
- **MAY touch:** `src/wasm_api.rs` (SLIDER_NAMES append ×6, apply_* setters ×6,
  `apply_slider_by_index` arms), `src/world/mod.rs` (DevSliders fields ×6 +
  defaults + `sliders_defaults_json`), `src/constants.rs` (6 *_DEFAULT consts),
  `src/control_sab.rs` (verify assertion only), `web/src/generated/slider-ids.ts`
  + `control-sab.ts` (regen via `cargo run --bin gen-bindings`),
  `web/src/widgets/devpanel.ts` (SliderSpec ×6), `web/src/settings.ts`
  (DEFAULTS + interface ×6).
- **MUST NOT touch:** `src/grass.rs` core (kernel/tiles — 1b/1c's); the consume
  energy path (1e's). Append SLIDER_NAMES — never reorder existing entries.

### 1e — graze/sensing u8 (W3, parallel w/ 1d)
- **MAY touch:** `src/world/tick.rs` (graze energy: `density_chunk = GRASS_MAX /
  bites_per_block`, decode u8 → energy, `graze_yield`),
  `src/grass.rs::consume` energy-calc ONLY (l.893 — coordinate: 1a already
  adapted the read; 1e refines the energy quantization semantics),
  `src/world/proximity.rs` (sensing read, already u8 post-1a),
  `src/world/nn.rs` (sensing read).
- **MUST NOT touch:** SLIDER_NAMES / wasm_api.rs apply arms (1d's — 1e only
  READS slider idx 8); the scatter kernel + tile bookkeeping (1b/1c's); density
  field type (1a's). Note: `consume` is in grass.rs — if 1c is still in flight
  on grass.rs, 1e's consume-energy tweak must wait for 1c to land OR be folded
  into 1a/1c. Safest: keep 1e's grass.rs edit to the energy arithmetic inside
  `consume` body only, after 1c lands.

---

## 4. Per-stream WHAT-TO-BUILD (re-mapped to hub IDs; quote in dispatch)

**1a — u8 storage migration** (spec: v2.0.2 §u8 density + graze + sensing)
- `density: Vec<f32>` ⟹ `Vec<AtomicU8>` (4× memory). Adapt EVERY reader:
  relaxed load → `f32` convert. `bilinear_sample` u8→f32 on read.
- Adapt all `.density` touches: proximity sector scan, row-bitset rebuild,
  `classify_tile`, `quantize_dirty_tiles_into`, mod.rs init loop, 44 test writes.
- Writers via relaxed load→clamp→store RMW. Init/`consume`/propagation-copyback.
- Keep public method signatures (`bilinear_sample`→f32, `consume`→f32) stable so
  1e and NN sensing are source-compatible.

**1b — scatter kernel** (spec: v2.0.2 §What each tick does)
- Add scatter model alongside blur, behind a flag (do NOT delete h_at/blur_at/
  grass_cell_next/ScratchPtr — hub safety valve; Stage 3 deletes post-bench).
- Per-cell `(world_seed, tile/cell, tick)` → seeded hash RNG draw (position-
  addressable, never touches SimRng).
- Per-tile frozen-source snapshot: worker copies its tile's source u8 into a
  transient O(tile) local buffer before scattering.
- Spread + decay as relaxed load→clamp→store RMW on AtomicU8 (cross-tile legal;
  collisions clobber wholesale — accepted).
- Disc-offset sampling (table from 1c) for spread targets.

**1c — active-tile set + cross-tile bookkeeping** (spec: v2.0.2 §Persistence &
Spread shape + §Concurrency gap)
- Precomputed disc offset table (NOT Chebyshev rings): radial bands
  ring1/2/3_pct over uniform angular dist; sample via RNG draw + binary-search
  or alias table; rebuild on slider change.
- Active-tile set = tiles with grass OR within range z of grass; append-compact
  maintenance (drop saturated-skip class). Convert tile_active/tile_dirty to
  atomic set-bit (or per-worker touched-tile merge) so cross-tile spread writes
  from multiple workers are safe (today plain `Vec<u64>` + sequential set-bit).
- Single-seed spread footprint must be ROUND (disc, not axis-biased diamond) —
  test deferred to Stage 3 but design for it.

**1d — six sliders end-to-end** (spec: v2.0.2 §sliders)
- Append to SLIDER_NAMES (order, hub-decided): `grass_spread_percentage`,
  `grass_spread_amount`, `grass_spread_ring1_pct`, `grass_spread_ring2_pct`,
  `grass_spread_ring3_pct`, `grass_decay_percentage`, `grass_decay_amount`.
  (NOTE: that is SEVEN names — see Open Questions; hub/spec both say "six".)
- For each: apply_* setter + dispatch arm (wasm_api.rs), DevSliders field +
  default + `sliders_defaults_json` (mod.rs), *_DEFAULT const (constants.rs),
  SliderSpec (devpanel.ts), DEFAULTS + Settings field (settings.ts). Regen TS
  (`cargo run --bin gen-bindings`); commit generated files.
- Guard: cap `grass_bites_per_block` slider max (≤ ~32) so u8 energy-reward
  granularity stays valid.

**1e — graze/sensing read-path u8** (spec: v2.0.2 §graze + sensing)
- `consume`: energy reward quantized to 256 levels over [0, GRASS_MAX]; per-cell
  gain = `energy_per_bite * (taken / density_chunk)`, `taken =
  min(decoded_density, density_chunk)`, `density_chunk = GRASS_MAX /
  bites_per_block`. Final `gain *= (1.0 - 0.5*diet)`.
- Sensing read path (proximity sector scan, bilinear_sample) consumes u8 density
  via 1a's adapted readers — verify f32 outputs unchanged in shape.
- Must NOT touch SimRng (read-only on grass).

---

## 5. NEW decisions / BLOCKERS / OPEN QUESTIONS

### Blockers (must resolve before the named wave)
- **None hard.** SAB assertion headroom CONFIRMED safe: current `SLIDER_COUNT =
  54` (30 scalar + 8 buckets × 3), `CTRL_SLIDERS_BASE(16) + 54 = 70 < 80`. After
  +6 sliders → `SLIDER_COUNT = 60`, `16 + 60 = 76 < CTRL_INSPECT_REQ_EPOCH(80)` —
  passes with 4 slots to spare. 1d is NOT blocked on a SAB redesign. **But: zero
  remaining headroom for any further sliders this stage.**

### New decisions surfaced (orchestrator should log in hub)
- **Fuse or serialize 1b+1c.** They are NOT file-disjoint (shared
  `compute_propagation` + tile-class/active-set in one impl block). Recommend
  fusing into one scatter+frontier stream, or strict serial 1b→1c with a single
  agent owning grass.rs for both. (Refines hub's "(1b,1c)".)
- **1e's grass.rs edit ordering.** `consume` lives in grass.rs (1b/1c's file).
  1e's energy-quantization tweak to `consume` must land AFTER 1c (or fold the
  consume-energy change into 1a/1c). Keep 1e's grass.rs footprint to the energy
  arithmetic inside `consume` only; everything else 1e edits is tick.rs/
  proximity.rs/nn.rs (disjoint, parallel-safe w/ 1d).

### Open questions (need a call before/within the named stream)
- **Slider count mismatch: six vs seven.** Hub prose says "the six sliders"; both
  the hub's resolved-decisions list AND the spec enumerate SEVEN names
  (spread_percentage, spread_amount, ring1_pct, ring2_pct, ring3_pct,
  decay_percentage, decay_amount). RESOLVE the canonical count before 1d (affects
  SLIDER_COUNT → 60 vs 61 and SAB headroom: 61 still < 80, so not a blocker, but
  names must be final). [1d precondition]
- **Slider RANGES / defaults** — still open (hub open-question). Suggested
  starting points from recon (not yet decided): spread_percentage∈[0,1] def 0.5;
  spread_amount (u8 units) small int; ring1/2/3_pct∈[0,1] cumulative falloff;
  decay_percentage∈[0,1] def ~0.1; decay_amount small int. **Orchestrator must
  pin before 1d code lands** (spec explicitly says "confirm with orchestrator").
- **Construction-only vs live** for the 6/7 sliders — spec doesn't pin it. If
  any are construction-only, add to `devpanel.ts::CONSTRUCTION_ONLY_SLIDERS`;
  else live-apply group. [1d precondition]
- **Disc-table sampling impl** — binary-search vs alias-table for target
  sampling; ring1/2/3_pct cumulative-falloff encoding. [1c design call]
- **`grass_bites_per_block` max cap** — hub says ≤ ~32; pin exact max for u8
  energy granularity. [1d/1e]

### Genuinely NEW vs already-resolved
Already resolved by hub (do NOT relitigate): AtomicU8 + relaxed RMW clobber;
per-cell hash RNG / no-SimRng; disc table not Chebyshev; no spontaneous spawn;
per-tile frozen source + raster seam accepted; array-only biome; determinism
not required. The above questions are the genuinely-open residue.
