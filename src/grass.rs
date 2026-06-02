//! GrassGrid — runtime-sized flat-`Vec<f32>` density field over the world.
//!
//! v2.0 Wave 1a: the grid is `grass_dim × grass_dim` cells (computed at
//! construction from `world_size`; 1920² at the 9600u default), each
//! `GRASS_CELL_SIZE` (5u) square. Independent of SpatialGrid (10u, body
//! queries). The world may be toroidal (`dims.wrap_world`): propagation,
//! overlap, and bilinear sampling wrap across the seam when so. v1.2
//! grass-mechanic-brief §Grass storage / §Grass dynamics per tick /
//! §Initial grass seed / §Bilinear sampling.

use crate::constants::{WorldDims, GRASS_CELL_SIZE, GRASS_MAX};
use crate::profiler::clock_now_us_threadsafe;
use crate::rng::SimRng;
#[cfg(feature = "threads")]
use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// v2.0.1 §2: side length (in cells) of an active-frontier tile. 32×32 = 1024
/// cells/tile; at `grass_dim = 1920` that is 60×60 = 3600 tiles. Chosen to
/// amortize per-tile bookkeeping while keeping the frontier-only active set small.
pub const GRASS_TILE_SIZE: usize = 32;

/// v2.0.1 §2: equilibrium epsilon. A cell is "empty" at ≤ EPS and "saturated"
/// at ≥ cap−EPS; a tile with no cell strictly inside `(EPS, cap−EPS)` and no
/// active neighbour is at equilibrium and can be skipped.
const GRASS_EQ_EPS: f32 = 1e-4;

/// v2.0.1 §2 tile equilibrium classes.
const TILE_EMPTY: u8 = 0;
const TILE_SATURATED: u8 = 1;
const TILE_MIXED: u8 = 2;

/// `Send` wrapper around the raw scratch pointer so the rayon closure can write
/// into disjoint tile cell-ranges in parallel. SAFETY at the call site: active
/// tiles are pairwise-disjoint cell ranges, each closure writes only its tile.
#[cfg(feature = "threads")]
#[derive(Clone, Copy)]
struct ScratchPtr(*mut f32);
#[cfg(feature = "threads")]
unsafe impl Send for ScratchPtr {}
#[cfg(feature = "threads")]
unsafe impl Sync for ScratchPtr {}

#[cfg(not(feature = "threads"))]
#[derive(Clone, Copy)]
struct ScratchPtr(*mut f32);

/// Read a cell from the frozen prior frame `d`, returning 0.0 (ghost) for an
/// out-of-bounds index when walled, or the opposite-edge cell when toroidal.
#[inline]
fn sample_d(d: &[f32], dim: usize, wrap: bool, ix: i64, iy: i64) -> f32 {
    let dimi = dim as i64;
    if wrap {
        let wx = ix.rem_euclid(dimi) as usize;
        let wy = iy.rem_euclid(dimi) as usize;
        d[wy * dim + wx]
    } else if ix < 0 || ix >= dimi || iy < 0 || iy >= dimi {
        0.0
    } else {
        d[iy as usize * dim + ix as usize]
    }
}

/// v2.0.1 §1 PASS H — horizontal `[1,2,1]/4` tap of the frozen frame `d` at
/// (ix, iy): `0.5·d[c] + 0.25·(d[w] + d[e])`. This is the first of the two
/// separable passes; the vertical pass ([`blur_at`]) consumes its output. Kept a
/// pure function of `d` (boundary via `sample_d`) so it composes identically
/// whether materialized into an intermediate buffer or evaluated on demand.
#[inline]
fn h_at(d: &[f32], dim: usize, wrap: bool, ix: i64, iy: i64) -> f32 {
    let c = sample_d(d, dim, wrap, ix, iy);
    let w = sample_d(d, dim, wrap, ix - 1, iy);
    let e = sample_d(d, dim, wrap, ix + 1, iy);
    0.5 * c + 0.25 * (w + e)
}

/// v2.0.1 §1 PASS V — vertical `[1,2,1]/4` tap applied to the horizontal pass
/// `h` at (ix, iy): `0.5·h[c] + 0.25·(h[n] + h[s])`. The composition V∘H is the
/// TRUE two-pass separable blur (its outer product is the 3×3
/// `[[1,2,1],[2,4,2],[1,2,1]]/16` Gaussian), evaluated here directly off `d` so
/// the result is bit-identical to running an explicit H-into-scratch pass then a
/// V pass — and identical regardless of how the grid is tiled, because every tap
/// reads only the frozen prior frame `d`. Boundary via `sample_d`.
#[inline]
fn blur_at(d: &[f32], dim: usize, wrap: bool, ix: i64, iy: i64) -> f32 {
    let c = h_at(d, dim, wrap, ix, iy);
    let n = h_at(d, dim, wrap, ix, iy - 1);
    let s = h_at(d, dim, wrap, ix, iy + 1);
    0.5 * c + 0.25 * (n + s)
}

/// v2.0.1 §1: next-frame value for cell (ix, iy). Separable-Gaussian-blurred
/// neighbour field + logistic in-cell growth, clamped to the cell's biome cap.
/// Pure function of the prior frame `d` → order-independent (single-thread ==
/// rayon bit-identical for a fixed seed).
#[inline]
#[allow(clippy::too_many_arguments)]
fn grass_cell_next(
    d: &[f32],
    cap: &[f32],
    dim: usize,
    wrap: bool,
    r_in_cell: f32,
    k_propagate: f32,
    ix: usize,
    iy: usize,
) -> f32 {
    let c = iy * dim + ix;
    let v = d[c];
    let k_cap = cap[c];
    let inv_cap = if k_cap > 0.0 { 1.0 / k_cap } else { 0.0 };
    let logistic = r_in_cell * v * (1.0 - v * inv_cap);
    // Max-of-neighbours spill over the TWO-PASS-BLURRED field's 8 neighbours.
    // Two reasons this is isotropic where the old 4-cardinal max was a diamond:
    //   (1) the spill samples ALL 8 neighbours (incl. diagonals), not just N/S/E/W
    //       — a cardinal-only max structurally favours the axes (an L1/Von-Neumann
    //       reach), which is the diamond; the 8-neighbour max gives an L∞ reach
    //       that the isotropic blur then rounds into a disc.
    //   (2) each sample is the separable two-pass Gaussian `blur_at`, so the
    //       diagonal corners are already smeared in before the max reads them.
    // Still a MAX (not a sum), so the anti-cascade "one ripe neighbour grants the
    // boost, neighbours don't stack" semantics are preserved.
    let ixi = ix as i64;
    let iyi = iy as i64;
    let mut max_neighbor = f32::NEG_INFINITY;
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            max_neighbor = max_neighbor.max(blur_at(d, dim, wrap, ixi + dx, iyi + dy));
        }
    }
    let prop = k_propagate * max_neighbor;
    (v + logistic + prop).clamp(0.0, k_cap)
}

/// Runtime-sized grass density field over the world.
///
/// Row-major layout: cell (ix, iy) is at index `iy * dims.grass_dim + ix`.
/// Density values are clamped to `[0.0, GRASS_MAX]` after each `step` call.
#[derive(Debug)]
pub struct GrassGrid {
    /// Runtime dimensions (grass_dim / cell_count / wrap). Set at construction.
    pub dims: WorldDims,
    /// Row-major density, length `dims.grass_cell_count`.
    pub density: Vec<f32>,
    /// Per-cell grass *carrying capacity* (`biome_factor * GRASS_MAX`), row-major,
    /// length `dims.grass_cell_count`. v2.0 Wave 1: biome drives grass richness —
    /// a cell's density saturates at `capacity[c]` (Plains→GRASS_MAX, Desert→0.30,
    /// Water→0.04) for BOTH the logistic carrying capacity and the post-step
    /// clamp. Built once at construction; never mutated per tick (static map).
    pub capacity: Vec<f32>,
    /// Double-buffer scratch. Same length as `density`. Recomputed every
    /// `step` call; never read between ticks.
    pub scratch: Vec<f32>,
    /// Per-row "any non-empty" bitset (v1.5 S5b). One bit per row: 1 if any
    /// cell in that row has `density > 0`, 0 otherwise. Lets the grass-sector
    /// NN scan skip whole rows in O(1) when sparse. Length =
    /// `GRASS_GRID_DIM.div_ceil(64)` u64s; regenerated after every `step()`.
    pub row_has_density: Vec<u64>,
    /// v2.0.1 §2: tiles per axis = `grass_dim.div_ceil(GRASS_TILE_SIZE)`.
    pub tiles_per_axis: usize,
    /// v2.0.1 §2: per-tile "active this tick" bitset, 1 bit per tile, row-major
    /// over the tile grid (`tiles_per_axis²` bits). A tile is active iff it (or a
    /// neighbour) may change this tick: it is FRONTIER, borders a FRONTIER tile,
    /// or was grazed. The frontier propagation pass processes ONLY active tiles;
    /// equilibrium tiles (uniformly ≈0 or uniformly saturated) are skipped exactly.
    pub tile_active: Vec<u64>,
    /// v2.0.1 §2: scratch "active NEXT tick" bitset, built during a pass from the
    /// FRONTIER tiles + their 8-neighbour fringe, then swapped into `tile_active`.
    tile_active_next: Vec<u64>,
    /// v2.0.1 §3: per-tile "dirty since each snapshot slot was written" — 2 bits
    /// per tile (one per ping-pong slot). A tile's bit for slot S is set when its
    /// density changed (propagation or graze) and cleared when re-quantized into
    /// slot S. Lets `quantize_dirty_tiles_into` re-quantize only changed tiles
    /// into a slot while keeping BOTH ping-pong slots valid. Indexed
    /// `tile_dirty[slot]`, each a `tiles_per_axis²`-bit bitset.
    pub tile_dirty: [Vec<u64>; 2],
    /// v2.0.1 §2: persistent per-tile equilibrium class — `TILE_EMPTY` (all cells
    /// ≤ EPS), `TILE_SATURATED` (all cells ≥ cap−EPS), or `TILE_MIXED` (a moving
    /// frontier). One byte per tile (`tiles_per_axis²`). Only ACTIVE tiles are
    /// re-classified each pass; skipped tiles are unchanged so their class
    /// persists. The next-tick active set = MIXED tiles + any tile bordering a
    /// tile of a DIFFERENT class (so a saturated/empty boundary keeps spreading).
    tile_class: Vec<u8>,
    /// Per-tick wall time of the threaded `par_chunks_mut` dispatch block in
    /// `compute_propagation` (us). Zero in non-threaded builds. Reset at the
    /// top of each `compute_propagation` call; drained by the World caller.
    pub par_chunks_us: AtomicU64,
    /// Per-tick wall time of the sequential `chunks_mut` block in
    /// `compute_propagation` (us). Zero in threaded builds.
    pub chunks_mut_us: AtomicU64,
    /// Per-tick sum-busy time inside the row_body closure across all
    /// invocations (us). In threaded builds, this is summed across rayon
    /// workers via `fetch_add`, so it can exceed `par_chunks_us` wall.
    pub row_body_us: AtomicU64,
    /// Per-tick sum-busy time bracketing just the row_body closure body
    /// itself (us), measured from inside the closure. Differs from
    /// `row_body_us` by excluding the per-chunk `chunks_mut` dispatch overhead;
    /// also summed across rayon workers via `fetch_add`.
    pub row_body_self_us: AtomicU64,
    /// Per-tick count of row-body closure invocations. Always equals
    /// `GRASS_GRID_DIM` (one per row) regardless of how rows are partitioned
    /// across rayon workers. Paired with `row_body_us` / `row_body_self_us`
    /// so the profiler can report honest per-row `ms/call`. (v1.7.2)
    pub row_body_calls: AtomicU64,
    /// Per-tick count of dispatch invocations — exactly one per
    /// `compute_propagation` call. Paired with `par_chunks_us` + `chunks_mut_us`
    /// so the `grass_step.dispatch` row has a meaningful `ms/call`. (v1.7.2)
    pub dispatch_calls: AtomicU64,
}

impl Clone for GrassGrid {
    fn clone(&self) -> Self {
        Self {
            dims: self.dims,
            density: self.density.clone(),
            capacity: self.capacity.clone(),
            scratch: self.scratch.clone(),
            row_has_density: self.row_has_density.clone(),
            tiles_per_axis: self.tiles_per_axis,
            tile_active: self.tile_active.clone(),
            tile_active_next: self.tile_active_next.clone(),
            tile_dirty: self.tile_dirty.clone(),
            tile_class: self.tile_class.clone(),
            par_chunks_us: AtomicU64::new(0),
            chunks_mut_us: AtomicU64::new(0),
            row_body_us: AtomicU64::new(0),
            row_body_self_us: AtomicU64::new(0),
            row_body_calls: AtomicU64::new(0),
            dispatch_calls: AtomicU64::new(0),
        }
    }
}

impl GrassGrid {
    /// World-init constructor. Seeds `initial_seed_count` random cells
    /// with `GRASS_MAX`; all others zero. Draws from `rng` via the same
    /// world RNG used everywhere else (deterministic given fixed seed).
    ///
    /// Capacity is uniform `GRASS_MAX` (all-Plains). Production callers use
    /// [`GrassGrid::new_with_capacity`] to apply per-biome carrying capacity;
    /// this plain ctor preserves today's all-plains behavior for the grass-only
    /// unit tests.
    ///
    /// Test/back-compat ctor: production routes through `new_with_capacity`.
    #[allow(dead_code)]
    pub fn new(rng: &mut SimRng, initial_seed_count: u32, dims: WorldDims) -> Self {
        Self::new_with_capacity(rng, initial_seed_count, dims, None)
    }

    /// World-init constructor with optional per-cell biome carrying capacity.
    ///
    /// v2.0 Wave 1: when `biome_grid` is `Some`, each cell's carrying capacity is
    /// `capacity_factor_from_u8(biome) * GRASS_MAX` (Plains ×1.0, Desert ×0.30,
    /// Water ×0.04). A seeded cell is filled to **its own** capacity (so a water
    /// cell seeds near 0.04, a desert cell near 0.30), and the same per-cell cap
    /// bounds logistic regrowth in [`compute_propagation`]. When `None`, capacity
    /// is uniform `GRASS_MAX` (== legacy all-plains behavior).
    ///
    /// Deterministic: the RNG draw order is identical to the legacy ctor (one
    /// `rng.index` per seed), so grass seeding stays pinned to the seed; biome
    /// only changes the *value* written, never the cell selection.
    pub fn new_with_capacity(
        rng: &mut SimRng,
        initial_seed_count: u32,
        dims: WorldDims,
        biome_grid: Option<&[u8]>,
    ) -> Self {
        let cell_count = dims.grass_cell_count;
        let capacity = match biome_grid {
            Some(bg) => {
                debug_assert_eq!(bg.len(), cell_count);
                bg.iter()
                    .map(|&b| crate::world::biome::capacity_factor_from_u8(b) * GRASS_MAX)
                    .collect::<Vec<f32>>()
            }
            None => vec![GRASS_MAX; cell_count],
        };
        let mut density = vec![0.0f32; cell_count];
        let scratch = vec![0.0f32; cell_count];
        for _ in 0..initial_seed_count {
            let idx = rng.index(cell_count);
            // Seed to the cell's own carrying capacity (Plains→GRASS_MAX,
            // Desert→0.30, Water→0.04) so a freshly seeded cell already sits at
            // its biome's ceiling rather than over-filling.
            density[idx] = capacity[idx];
        }
        let tiles_per_axis = dims.grass_dim.div_ceil(GRASS_TILE_SIZE);
        let tile_words = (tiles_per_axis * tiles_per_axis).div_ceil(64);
        let mut g = Self {
            dims,
            density,
            capacity,
            scratch,
            row_has_density: vec![0u64; dims.grass_dim.div_ceil(64)],
            tiles_per_axis,
            tile_active: vec![0u64; tile_words],
            tile_active_next: vec![0u64; tile_words],
            tile_dirty: [vec![0u64; tile_words], vec![0u64; tile_words]],
            tile_class: vec![TILE_EMPTY; tiles_per_axis * tiles_per_axis],
            par_chunks_us: AtomicU64::new(0),
            chunks_mut_us: AtomicU64::new(0),
            row_body_us: AtomicU64::new(0),
            row_body_self_us: AtomicU64::new(0),
            row_body_calls: AtomicU64::new(0),
            dispatch_calls: AtomicU64::new(0),
        };
        g.rebuild_row_bitset();
        // Seed the active set from the initial density so the first tick
        // processes every non-equilibrium tile, and mark ALL tiles dirty so the
        // first snapshot writes the full grass region into both slots (the SAB
        // slots start zeroed; seeded-but-saturated tiles aren't "active").
        g.resync_active_from_density();
        g.mark_all_tiles_dirty();
        g
    }

    /// Recompute the per-row "any non-empty" bitset. Called from `step()`
    /// after the density swap; exposed for tests that mutate `density` directly.
    pub fn rebuild_row_bitset(&mut self) {
        let dim = self.dims.grass_dim;
        let words = dim.div_ceil(64);
        if self.row_has_density.len() != words {
            self.row_has_density.resize(words, 0);
        }
        for w in self.row_has_density.iter_mut() {
            *w = 0;
        }
        for iy in 0..dim {
            let row_off = iy * dim;
            let any = self.density[row_off..row_off + dim]
                .iter()
                .any(|&d| d > 0.0);
            if any {
                self.row_has_density[iy / 64] |= 1u64 << (iy % 64);
            }
        }
    }

    /// Test whether row `iy` may contain any non-zero density cells.
    #[inline]
    pub fn row_nonempty(&self, iy: usize) -> bool {
        let w = iy / 64;
        if w >= self.row_has_density.len() {
            return false;
        }
        (self.row_has_density[w] >> (iy % 64)) & 1 == 1
    }

    // ─── v2.0.1 §2/§3 active-tile + dirty-tile helpers ─────────────────────────

    /// Tile index for a cell index (row-major over the tile grid).
    #[inline]
    fn tile_of_cell(&self, cell_idx: usize) -> usize {
        let dim = self.dims.grass_dim;
        let ix = cell_idx % dim;
        let iy = cell_idx / dim;
        let tx = ix / GRASS_TILE_SIZE;
        let ty = iy / GRASS_TILE_SIZE;
        ty * self.tiles_per_axis + tx
    }

    #[inline]
    fn bitset_get(set: &[u64], i: usize) -> bool {
        (set[i / 64] >> (i % 64)) & 1 == 1
    }

    #[inline]
    fn bitset_set(set: &mut [u64], i: usize) {
        set[i / 64] |= 1u64 << (i % 64);
    }

    /// Mark a tile (and its 8 neighbours, honoring wrap) active for the NEXT
    /// tick AND dirty for both snapshot slots. `set_active` chooses whether the
    /// activation lands in the current `tile_active` set (used at resync / graze,
    /// which must affect the upcoming pass) — propagation builds the next-set
    /// separately. Marking the 8 neighbours guarantees the frontier can advance
    /// one tile/tick and that a re-quantize covers any cell a change can reach.
    fn activate_tile_and_fringe(&mut self, tile: usize) {
        let tpa = self.tiles_per_axis;
        let wrap = self.dims.wrap_world;
        let tx = tile % tpa;
        let ty = tile / tpa;
        for dy in -1i64..=1 {
            let ny = ty as i64 + dy;
            let ny = if wrap {
                ny.rem_euclid(tpa as i64)
            } else if ny < 0 || ny >= tpa as i64 {
                continue;
            } else {
                ny
            };
            for dx in -1i64..=1 {
                let nx = tx as i64 + dx;
                let nx = if wrap {
                    nx.rem_euclid(tpa as i64)
                } else if nx < 0 || nx >= tpa as i64 {
                    continue;
                } else {
                    nx
                };
                let nt = ny as usize * tpa + nx as usize;
                Self::bitset_set(&mut self.tile_active, nt);
                Self::bitset_set(&mut self.tile_dirty[0], nt);
                Self::bitset_set(&mut self.tile_dirty[1], nt);
            }
        }
    }

    /// Mark the tile containing `cell_idx` active+dirty (with its fringe).
    /// Called from the graze/`consume` path so a grazed (drained) cell re-enters
    /// frontier processing and regrows, and so the snapshot re-quantizes it.
    #[inline]
    pub fn mark_tile_active(&mut self, cell_idx: usize) {
        let t = self.tile_of_cell(cell_idx);
        // A drained cell makes its tile no longer uniformly saturated/empty in the
        // general case; conservatively flag it MIXED so the next pass processes it
        // (the pass re-classifies it exactly afterward). This guarantees the grazed
        // patch regrows and is re-snapshotted regardless of its prior class.
        self.tile_class[t] = TILE_MIXED;
        self.activate_tile_and_fringe(t);
    }

    /// Rebuild the per-tile class + active set from the current `density`. Used at
    /// construction and whenever `density` is mutated wholesale outside propagation
    /// (tests, full-grass-on-init). Classifies every tile, then sets the active set
    /// to the union of MIXED tiles and any tile bordering a different-class tile —
    /// the same rule the per-tick pass maintains incrementally.
    pub fn resync_active_from_density(&mut self) {
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;
        for t in 0..ntiles {
            self.tile_class[t] = self.classify_tile(t);
        }
        self.rebuild_active_from_class();
    }

    /// Classify tile `t` from the current `density`: `TILE_EMPTY` if every cell is
    /// ≤ EPS, `TILE_SATURATED` if every cell is ≥ cap−EPS, else `TILE_MIXED`.
    fn classify_tile(&self, t: usize) -> u8 {
        let dim = self.dims.grass_dim;
        let tpa = self.tiles_per_axis;
        let tx = t % tpa;
        let ty = t / tpa;
        let ix0 = tx * GRASS_TILE_SIZE;
        let iy0 = ty * GRASS_TILE_SIZE;
        let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
        let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
        let mut all_empty = true;
        let mut all_sat = true;
        for iy in iy0..iy1 {
            let row = iy * dim;
            for ix in ix0..ix1 {
                let c = row + ix;
                let v = self.density[c];
                let cap = self.capacity[c];
                if v > GRASS_EQ_EPS {
                    all_empty = false;
                }
                if v < cap - GRASS_EQ_EPS {
                    all_sat = false;
                }
                if !all_empty && !all_sat {
                    return TILE_MIXED;
                }
            }
        }
        if all_empty {
            TILE_EMPTY
        } else if all_sat {
            TILE_SATURATED
        } else {
            TILE_MIXED
        }
    }

    /// Rebuild `tile_active` (and dirty both slots for every active tile) from the
    /// current `tile_class`: active = MIXED tiles ∪ any tile bordering a tile of a
    /// different class. A uniform tile bordering a uniform tile of the SAME class
    /// is true equilibrium and stays inactive.
    fn rebuild_active_from_class(&mut self) {
        for w in self.tile_active.iter_mut() {
            *w = 0;
        }
        for w in self.tile_dirty[0].iter_mut() {
            *w = 0;
        }
        for w in self.tile_dirty[1].iter_mut() {
            *w = 0;
        }
        let tpa = self.tiles_per_axis;
        let wrap = self.dims.wrap_world;
        let ntiles = tpa * tpa;
        for t in 0..ntiles {
            let active = self.tile_class[t] == TILE_MIXED || self.borders_different_class(t);
            if active {
                Self::bitset_set(&mut self.tile_active, t);
                Self::bitset_set(&mut self.tile_dirty[0], t);
                Self::bitset_set(&mut self.tile_dirty[1], t);
            }
        }
        let _ = wrap;
    }

    /// True if any 8-neighbour of tile `t` has a different `tile_class` (wrap-aware).
    fn borders_different_class(&self, t: usize) -> bool {
        let tpa = self.tiles_per_axis;
        let wrap = self.dims.wrap_world;
        let tx = (t % tpa) as i64;
        let ty = (t / tpa) as i64;
        let my = self.tile_class[t];
        for dy in -1i64..=1 {
            let ny = ty + dy;
            let ny = if wrap {
                ny.rem_euclid(tpa as i64)
            } else if ny < 0 || ny >= tpa as i64 {
                continue;
            } else {
                ny
            };
            for dx in -1i64..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = tx + dx;
                let nx = if wrap {
                    nx.rem_euclid(tpa as i64)
                } else if nx < 0 || nx >= tpa as i64 {
                    continue;
                } else {
                    nx
                };
                if self.tile_class[ny as usize * tpa + nx as usize] != my {
                    return true;
                }
            }
        }
        false
    }

    /// One propagation tick. Reads `self.density`, writes `self.scratch`, swaps.
    ///
    /// v2.0.1 §1: organic spread via a TRUE two-pass SEPARABLE `[1,2,1]/4`-per-axis
    /// blur of the prior frame — a horizontal pass ([`h_at`]) feeding a vertical
    /// pass ([`blur_at`]); V∘H is the 3×3 `[[1,2,1],[2,4,2],[1,2,1]]/16` Gaussian.
    /// The max-of-neighbours spill is then taken over the BLURRED field's 8
    /// neighbours (not just the 4 cardinals — the cardinal-only max is an L1 reach
    /// = diamond), so a seed expands as a disc while keeping the "one ripe neighbour
    /// grants the boost, neighbours don't stack" anti-cascade behaviour (it stays a
    /// max, not a sum).
    ///
    /// Per cell c with `K = cap[c]` (ghost-zero / wrap boundary on each tap):
    /// ```text
    /// h[x]      = 0.5*d[x] + 0.25*(d[x-1] + d[x+1])     (PASS H)
    /// blur[x]   = 0.5*h[x] + 0.25*(h[x-dim] + h[x+dim]) (PASS V)
    /// v         = d[c]
    /// logistic  = r * v * (1 - v / K)
    /// prop      = k * max(blur over the 8 neighbours of c)
    /// d'[c]     = clamp(v + logistic + prop, 0, K)
    /// ```
    /// Out-of-bounds neighbours are zero (walled) / opposite edge (wrap). Empty
    /// cells with an empty neighbourhood stay zero (blur of zero is zero).
    ///
    /// Production callers in `World::tick_once` now invoke the split
    /// `compute_propagation` + `rebuild_row_bitset` directly so the profiler
    /// can break the two phases apart; this convenience wrapper survives for
    /// the grass-only unit tests in this module.
    #[cfg(test)]
    pub fn step(&mut self, r_in_cell: f32, k_propagate: f32) {
        // Test convenience: existing unit tests poke `density` directly between
        // steps, so resync the per-tile active/class state from the current field
        // before each step. Production callers drive the incremental active-set
        // path via `compute_propagation` directly (and graze's `mark_tile_active`).
        self.resync_active_from_density();
        self.compute_propagation(r_in_cell, k_propagate);
        self.rebuild_row_bitset();
    }

    /// Reference full-grid propagation: process EVERY cell (no tile skipping).
    /// Used by the active-tile equivalence test as the ground truth. Same per-cell
    /// math as `compute_propagation`; reads `self.density`, writes `self.scratch`,
    /// swaps. Not used in production (the tile path is the fast equivalent).
    #[cfg(test)]
    pub fn compute_propagation_full(&mut self, r_in_cell: f32, k_propagate: f32) {
        let dim = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let d = &self.density;
        let cap = &self.capacity;
        for iy in 0..dim {
            for ix in 0..dim {
                let c = iy * dim + ix;
                self.scratch[c] =
                    grass_cell_next(d, cap, dim, wrap, r_in_cell, k_propagate, ix, iy);
            }
        }
        std::mem::swap(&mut self.density, &mut self.scratch);
    }

    /// Phase 1 of `step()`: write the next density frame into `scratch` and
    /// swap. v2.0.1 §2: processes ONLY active (frontier + fringe + grazed) tiles;
    /// equilibrium tiles are an identity copy (`d`→`scratch`), which is exact
    /// because an EMPTY tile next to non-empty grass is kept active by the fringe
    /// rule and a SATURATED tile next to a frontier tile likewise — so any tile
    /// that is skipped has an unchanging value this tick. Builds the next-tick
    /// active set from the post-pass frontier + 8-neighbour fringe, and marks
    /// every changed tile dirty for both snapshot slots.
    pub fn compute_propagation(&mut self, r_in_cell: f32, k_propagate: f32) {
        debug_assert_eq!(self.density.len(), self.dims.grass_cell_count);
        debug_assert_eq!(self.scratch.len(), self.dims.grass_cell_count);

        self.par_chunks_us.store(0, Ordering::Relaxed);
        self.chunks_mut_us.store(0, Ordering::Relaxed);
        self.row_body_us.store(0, Ordering::Relaxed);
        self.row_body_self_us.store(0, Ordering::Relaxed);
        self.row_body_calls.store(0, Ordering::Relaxed);
        self.dispatch_calls.store(1, Ordering::Relaxed);

        let dim = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;

        // Build the dense active-tile list (sorted ascending → deterministic
        // dispatch order, so single-thread == rayon for a fixed seed).
        let mut active_list: Vec<u32> = Vec::with_capacity(ntiles);
        for t in 0..ntiles {
            if Self::bitset_get(&self.tile_active, t) {
                active_list.push(t as u32);
            }
        }

        // Per-active-tile post-pass class (EMPTY/SATURATED/MIXED). Index parallel
        // to `active_list`; written back into the persistent `tile_class` after.
        let mut new_class: Vec<u8> = vec![TILE_EMPTY; active_list.len()];

        let d = &self.density;
        let cap = &self.capacity;
        let row_body_self_us = &self.row_body_self_us;
        let row_body_calls = &self.row_body_calls;

        // Process one active tile: compute its cells into `scratch` and return the
        // tile's post-pass class. Writes are confined to the tile's own cells;
        // reads (the two-pass blur sampled at c's 8 neighbours) reach a 2-cell halo
        // into the FROZEN prior frame `d`, so tiles are independent → single-thread
        // == rayon bit-identical, and identical to a full-grid two-pass.
        let tile_body = |t: usize, scratch: &mut [f32]| -> u8 {
            let body_self_start = clock_now_us_threadsafe();
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            let mut all_empty = true;
            let mut all_sat = true;
            for iy in iy0..iy1 {
                let row = iy * dim;
                for ix in ix0..ix1 {
                    let c = row + ix;
                    let nv = grass_cell_next(d, cap, dim, wrap, r_in_cell, k_propagate, ix, iy);
                    scratch[c] = nv;
                    let k_cap = cap[c];
                    if nv > GRASS_EQ_EPS {
                        all_empty = false;
                    }
                    if nv < k_cap - GRASS_EQ_EPS {
                        all_sat = false;
                    }
                }
            }
            row_body_self_us.fetch_add(
                clock_now_us_threadsafe().saturating_sub(body_self_start),
                Ordering::Relaxed,
            );
            row_body_calls.fetch_add(1, Ordering::Relaxed);
            if all_empty {
                TILE_EMPTY
            } else if all_sat {
                TILE_SATURATED
            } else {
                TILE_MIXED
            }
        };

        // Compute active tiles into scratch. The whole `scratch` slice is passed
        // (each tile writes only its own cells; no two active tiles overlap).
        #[cfg(feature = "threads")]
        {
            let par_start = clock_now_us_threadsafe();
            let row_body_us = &self.row_body_us;
            let scratch_ptr = ScratchPtr(self.scratch.as_mut_ptr());
            // SAFETY: active tiles are disjoint cell ranges; each closure only
            // writes its own tile's cells. Reads of `d` are immutable & shared.
            active_list
                .par_iter()
                .zip(new_class.par_iter_mut())
                .for_each(|(&t, cls)| {
                    // Capture the whole wrapper (Send+Sync) by copy, not the inner
                    // `*mut f32` field (which a precise closure capture would grab
                    // as `&*mut f32`, defeating the Send/Sync impl).
                    let sp = scratch_ptr;
                    let body_start = clock_now_us_threadsafe();
                    let scratch: &mut [f32] =
                        unsafe { std::slice::from_raw_parts_mut(sp.0, dim * dim) };
                    *cls = tile_body(t as usize, scratch);
                    let body_end = clock_now_us_threadsafe();
                    row_body_us.fetch_add(body_end.saturating_sub(body_start), Ordering::Relaxed);
                });
            let par_end = clock_now_us_threadsafe();
            self.par_chunks_us
                .store(par_end.saturating_sub(par_start), Ordering::Relaxed);
        }
        #[cfg(not(feature = "threads"))]
        {
            let seq_start = clock_now_us_threadsafe();
            let body_start = clock_now_us_threadsafe();
            let scratch_ptr = ScratchPtr(self.scratch.as_mut_ptr());
            for (i, &t) in active_list.iter().enumerate() {
                let scratch: &mut [f32] =
                    unsafe { std::slice::from_raw_parts_mut(scratch_ptr.0, dim * dim) };
                new_class[i] = tile_body(t as usize, scratch);
            }
            let body_end = clock_now_us_threadsafe();
            self.row_body_us
                .store(body_end.saturating_sub(body_start), Ordering::Relaxed);
            let seq_end = clock_now_us_threadsafe();
            self.chunks_mut_us
                .store(seq_end.saturating_sub(seq_start), Ordering::Relaxed);
        }

        // Copy ONLY active tiles' fresh values from `scratch` back into `density`
        // (in place — NO full-grid swap). Skipped equilibrium tiles keep their
        // existing `density` value untouched (identity), so total memory traffic
        // is O(active cells), not O(all cells) — this is what makes the snapshot
        // AND propagation cheap when the frontier is small.
        for &t in active_list.iter() {
            let t = t as usize;
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            for iy in iy0..iy1 {
                let row = iy * dim;
                self.density[row + ix0..row + ix1]
                    .copy_from_slice(&self.scratch[row + ix0..row + ix1]);
            }
        }

        // Persist the post-pass class of every processed tile, and mark it dirty
        // for both snapshot slots (its cells were recomputed). Skipped tiles kept
        // their density (identity copy) so their class + bytes are unchanged.
        for (i, &t) in active_list.iter().enumerate() {
            let t = t as usize;
            self.tile_class[t] = new_class[i];
            Self::bitset_set(&mut self.tile_dirty[0], t);
            Self::bitset_set(&mut self.tile_dirty[1], t);
        }

        // Build the NEXT-tick active set: a tile must be processed next tick iff it
        // is MIXED, or it borders a tile of a DIFFERENT class (a class boundary is
        // a moving front — e.g. saturated→empty spreads, empty→mixed advances).
        // O(tiles) over the persistent class map; far cheaper than O(cells).
        for w in self.tile_active_next.iter_mut() {
            *w = 0;
        }
        for t in 0..ntiles {
            if self.tile_class[t] == TILE_MIXED || self.borders_different_class(t) {
                Self::bitset_set(&mut self.tile_active_next, t);
            }
        }
        std::mem::swap(&mut self.tile_active, &mut self.tile_active_next);
    }

    /// v2.0.1 §3: quantize ONLY the tiles dirty for ping-pong snapshot `slot`
    /// into `dst` (the slot's `grass_cell_count`-byte u8 grass region), then clear
    /// those tiles' dirty bits for that slot. Each cell becomes
    /// `round(clamp(density/GRASS_MAX, 0, 1) * 255)` — the same value the old
    /// full-grid `quantize_grass_into` produced, so the SAB wire bytes are
    /// unchanged. Unchanged tiles are left as-is in `dst` (the slot persists
    /// across writes, so it already holds valid prior bytes for them).
    ///
    /// COORDINATION (v2.0.1 writer wave): this is the single self-contained
    /// callable the upcoming snapshot `pack()` invokes — pass it the destination
    /// slot's grass region and the slot index and it writes only dirty tiles. Do
    /// not inline it; `write_snapshot` calls it today and `pack()` will tomorrow.
    ///
    /// `dst.len()` must equal `grass_cell_count`. `slot` is masked to 0/1.
    pub fn quantize_dirty_tiles_into(&mut self, dst: &mut [u8], slot: usize) {
        debug_assert_eq!(dst.len(), self.dims.grass_cell_count);
        let slot = slot & 1;
        let dim = self.dims.grass_dim;
        let tpa = self.tiles_per_axis;
        let ntiles = tpa * tpa;
        let inv_max = 1.0 / GRASS_MAX;
        for t in 0..ntiles {
            if !Self::bitset_get(&self.tile_dirty[slot], t) {
                continue;
            }
            let tx = t % tpa;
            let ty = t / tpa;
            let ix0 = tx * GRASS_TILE_SIZE;
            let iy0 = ty * GRASS_TILE_SIZE;
            let ix1 = (ix0 + GRASS_TILE_SIZE).min(dim);
            let iy1 = (iy0 + GRASS_TILE_SIZE).min(dim);
            for iy in iy0..iy1 {
                let row = iy * dim;
                for ix in ix0..ix1 {
                    let c = row + ix;
                    let q = (self.density[c] * inv_max).clamp(0.0, 1.0) * 255.0;
                    dst[c] = (q + 0.5) as u8;
                }
            }
            // Clear this slot's dirty bit — the slot now holds current bytes for t.
            self.tile_dirty[slot][t / 64] &= !(1u64 << (t % 64));
        }
    }

    /// v2.0.1 §3: mark EVERY tile dirty for both snapshot slots. Used after a
    /// wholesale `density` mutation outside propagation (full-grass-on-init,
    /// tests) so the next quantize refreshes the entire grass region.
    pub fn mark_all_tiles_dirty(&mut self) {
        for slot in 0..2 {
            for w in self.tile_dirty[slot].iter_mut() {
                *w = !0;
            }
        }
    }

    /// Bilinear sample at continuous world position `(x, y)`.
    ///
    /// Walled: clamps input to `[0, world_size)`; out-of-bounds cell indices read
    /// as ghost zero. Toroidal (`dims.wrap_world`): the world position wraps into
    /// `[0, world_size)` and the four sample cells wrap across the seam.
    pub fn bilinear_sample(&self, x: f32, y: f32) -> f32 {
        let w = self.dims.world_size;
        let dim = self.dims.grass_dim as i32;
        let wrap = self.dims.wrap_world;

        let (xw, yw) = if wrap {
            (x.rem_euclid(w), y.rem_euclid(w))
        } else {
            (x.clamp(0.0, w - 1e-4), y.clamp(0.0, w - 1e-4))
        };

        // Convert to fractional cell coordinates. Cell i's center is at
        // (i + 0.5) * GRASS_CELL_SIZE. Subtract 0.5 so cell centers fall at integers.
        let fx = xw / GRASS_CELL_SIZE - 0.5;
        let fy = yw / GRASS_CELL_SIZE - 0.5;

        let ix0 = fx.floor() as i32;
        let iy0 = fy.floor() as i32;
        let ix1 = ix0 + 1;
        let iy1 = iy0 + 1;

        let tx = fx - fx.floor(); // in [0, 1)
        let ty = fy - fy.floor();

        let dimu = self.dims.grass_dim;
        // Walled → ghost-zero out-of-bounds; wrap → fold indices into [0, dim).
        let sample = |iy: i32, ix: i32| -> f32 {
            if wrap {
                let wy = iy.rem_euclid(dim) as usize;
                let wx = ix.rem_euclid(dim) as usize;
                self.density[wy * dimu + wx]
            } else {
                if iy < 0 || iy >= dim || ix < 0 || ix >= dim {
                    return 0.0;
                }
                self.density[iy as usize * dimu + ix as usize]
            }
        };

        let d00 = sample(iy0, ix0);
        let d10 = sample(iy0, ix1);
        let d01 = sample(iy1, ix0);
        let d11 = sample(iy1, ix1);

        let a = d00 * (1.0 - tx) + d10 * tx;
        let b = d01 * (1.0 - tx) + d11 * tx;
        a * (1.0 - ty) + b * ty
    }

    /// Partial-bite consume. Drains `min(density, density_chunk)` and awards
    /// `energy_per_bite * taken / density_chunk` energy (proportional to what was
    /// actually consumed). A ripe cell (density ≥ chunk) still yields the full
    /// `energy_per_bite`; an under-threshold cell yields a fraction.
    /// Returns `0.0` only when the cell is fully empty.
    #[inline]
    pub fn consume(&mut self, cell_idx: usize, density_chunk: f32, energy_per_bite: f32) -> f32 {
        let cur = self.density[cell_idx];
        if cur <= 0.0 || density_chunk <= 0.0 {
            return 0.0;
        }
        let taken = cur.min(density_chunk);
        self.density[cell_idx] = cur - taken;
        // v2.0.1 §2/§3: a drained cell may turn a SATURATED tile into a frontier
        // tile and changes a snapshot byte — reactivate it + its fringe and mark
        // it dirty so it re-enters propagation, regrows, and is re-quantized.
        self.mark_tile_active(cell_idx);
        energy_per_bite * (taken / density_chunk)
    }

    /// Iterate every cell whose center is within Euclidean distance `r` of `(cx, cy)`.
    ///
    /// Walled: out-of-bounds cells are skipped (no toroidal wrap).
    /// Invokes `f(cell_idx)` once per hit. Used by P1e to find graze-overlap cells.
    /// Deterministic: row-major, left-to-right iteration order.
    /// Contract: `r` must be finite and non-negative.
    ///
    /// Uses circle-vs-AABB overlap (nearest point on the cell bounding box to the
    /// circle centre must be ≤ r). This is the correct test for "does a creature's
    /// body circle touch this grass cell" and avoids the old centre-to-centre test
    /// that could miss cells when `GRASS_CELL_SIZE >> r` (M1 regression fix).
    pub fn for_each_cell_overlapping_circle(
        &self,
        cx: f32,
        cy: f32,
        r: f32,
        mut f: impl FnMut(usize),
    ) {
        debug_assert!(r.is_finite() && r >= 0.0);

        // Search radius expanded by half cell size so the candidate window includes
        // every cell whose bounding box could intersect the circle.
        let search_r = r + GRASS_CELL_SIZE * 0.5;
        let r_cells = (search_r / GRASS_CELL_SIZE).ceil() as i32 + 1;
        let cx_cell = (cx / GRASS_CELL_SIZE).floor() as i32;
        let cy_cell = (cy / GRASS_CELL_SIZE).floor() as i32;
        let dim = self.dims.grass_dim as i32;
        let dimu = self.dims.grass_dim;
        let wrap = self.dims.wrap_world;
        let r2 = r * r;

        for jy in (cy_cell - r_cells)..=(cy_cell + r_cells) {
            // Walled: skip out-of-bounds rows; toroidal: fold to the cell index
            // but keep the *un-folded* jy/jx for the world-space box so the
            // nearest-point distance is measured across the seam, not clamped.
            let iy = if wrap {
                jy.rem_euclid(dim) as usize
            } else {
                if jy < 0 || jy >= dim {
                    continue;
                }
                jy as usize
            };
            for jx in (cx_cell - r_cells)..=(cx_cell + r_cells) {
                let ix = if wrap {
                    jx.rem_euclid(dim) as usize
                } else {
                    if jx < 0 || jx >= dim {
                        continue;
                    }
                    jx as usize
                };
                // Circle-vs-AABB: find nearest point on the cell box to (cx, cy),
                // then check if that distance is ≤ r. The box uses the unfolded
                // jx/jy so a seam-straddling query measures the short way around.
                let box_x_min = jx as f32 * GRASS_CELL_SIZE;
                let box_x_max = box_x_min + GRASS_CELL_SIZE;
                let box_y_min = jy as f32 * GRASS_CELL_SIZE;
                let box_y_max = box_y_min + GRASS_CELL_SIZE;
                let nearest_x = cx.clamp(box_x_min, box_x_max);
                let nearest_y = cy.clamp(box_y_min, box_y_max);
                let dx = nearest_x - cx;
                let dy = nearest_y - cy;

                if dx * dx + dy * dy <= r2 {
                    f(iy * dimu + ix);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SimRng;

    // v2.0 Wave 1a: grass dims are runtime. These module-local test constants
    // pin a fixed *walled* grid so the existing walled-behavior assertions keep
    // their meaning. 1200u world / 5u cell → 240² cells.
    const TEST_DIMS: WorldDims = WorldDims {
        world_size: 1200.0,
        wrap_world: false,
        grass_dim: 240,
        grass_cell_count: 240 * 240,
        hash_dim: 120,
    };
    const GRASS_GRID_DIM: usize = TEST_DIMS.grass_dim;
    const GRASS_CELL_COUNT: usize = TEST_DIMS.grass_cell_count;

    fn fresh_grid() -> GrassGrid {
        grid_with_density(
            vec![0.0f32; GRASS_CELL_COUNT],
            vec![GRASS_MAX; GRASS_CELL_COUNT],
        )
    }

    /// Build a test grid with the given density + capacity and a fully-synced
    /// tile state (class + active + dirty) so the active-tile path is correct.
    fn grid_with_density(density: Vec<f32>, capacity: Vec<f32>) -> GrassGrid {
        let tpa = TEST_DIMS.grass_dim.div_ceil(GRASS_TILE_SIZE);
        let tile_words = (tpa * tpa).div_ceil(64);
        let mut g = GrassGrid {
            dims: TEST_DIMS,
            density,
            capacity,
            scratch: vec![0.0f32; GRASS_CELL_COUNT],
            row_has_density: vec![0u64; GRASS_GRID_DIM.div_ceil(64)],
            tiles_per_axis: tpa,
            tile_active: vec![0u64; tile_words],
            tile_active_next: vec![0u64; tile_words],
            tile_dirty: [vec![0u64; tile_words], vec![0u64; tile_words]],
            tile_class: vec![TILE_EMPTY; tpa * tpa],
            par_chunks_us: AtomicU64::new(0),
            chunks_mut_us: AtomicU64::new(0),
            row_body_us: AtomicU64::new(0),
            row_body_self_us: AtomicU64::new(0),
            row_body_calls: AtomicU64::new(0),
            dispatch_calls: AtomicU64::new(0),
        };
        g.rebuild_row_bitset();
        g.resync_active_from_density();
        g.mark_all_tiles_dirty();
        g
    }

    /// Walled: step propagates only within bounds — corner cell (0,0) should
    /// NOT propagate to the "far edge" cells (the torus seam neighbors no longer
    /// exist). Only the east (1,0) and south (0,1) cells should receive signal.
    #[test]
    fn step_zero_density_at_edge() {
        let mut g = fresh_grid();
        // Seed cell (0, 0) — top-left corner.
        g.density[0] = GRASS_MAX;
        g.step(0.0, 0.05); // pure propagation, no logistic

        // East neighbor: cell (1, 0) = index 1 — should receive propagation.
        let east = g.density[1];
        // West neighbor would be at ix=-1 — ghost zero; cell (dim-1, 0) should NOT receive.
        let west = g.density[GRASS_GRID_DIM - 1];
        assert!(
            east > 0.0,
            "east neighbor must receive propagation from (0,0); east={east}"
        );
        assert!(
            west < 1e-9,
            "west (wrap) must NOT receive propagation in walled world; west={west}"
        );

        // North neighbor at iy=-1 is ghost zero; cell (0, dim-1) must NOT receive.
        let north_wrap = g.density[(GRASS_GRID_DIM - 1) * GRASS_GRID_DIM];
        assert!(
            north_wrap < 1e-9,
            "north wrap cell must NOT receive propagation in walled world; north_wrap={north_wrap}"
        );

        // South neighbor: cell (0, 1) = index GRASS_GRID_DIM — should receive.
        let south = g.density[GRASS_GRID_DIM];
        assert!(
            south > 0.0,
            "south neighbor must receive propagation; south={south}"
        );
    }

    /// Test 2: empty world stays empty after 100 steps (no spontaneous spawn).
    #[test]
    fn step_empty_world_stays_empty() {
        let mut g = fresh_grid();
        for _ in 0..100 {
            g.step(0.005, 0.05);
        }
        assert!(
            g.density.iter().all(|&d| d == 0.0),
            "empty world must remain empty after 100 steps"
        );
    }

    /// Test 3: step clamps density to GRASS_MAX after 100 steps.
    #[test]
    fn step_clamps_to_grass_max() {
        let mut g = grid_with_density(
            vec![GRASS_MAX; GRASS_CELL_COUNT],
            vec![GRASS_MAX; GRASS_CELL_COUNT],
        );
        for _ in 0..100 {
            g.step(0.005, 0.05);
        }
        let max_val = g.density.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max_val <= GRASS_MAX + 1e-6,
            "max density {max_val} exceeds GRASS_MAX after 100 steps"
        );
    }

    /// Test 4: single seeded cell with no propagation grows logistically toward GRASS_MAX.
    #[test]
    fn step_logistic_grows_seeded_cell() {
        let mut g = fresh_grid();
        g.density[0] = 0.1;
        let mut prev = 0.1f32;
        for _ in 0..50 {
            g.step(0.5, 0.0); // high r, no propagation
            let cur = g.density[0];
            assert!(
                cur >= prev,
                "logistic growth must be monotone; prev={prev} cur={cur}"
            );
            prev = cur;
        }
        assert!(
            g.density[0] > 0.9,
            "single seeded cell with r=0.5 should approach GRASS_MAX after 50 steps; got {}",
            g.density[0]
        );
    }

    /// Test 5: bilinear sample at cell center equals the cell's density exactly.
    #[test]
    fn bilinear_sample_at_cell_center_is_exact() {
        let mut g = fresh_grid();
        // Cell (5, 0): index = 5. Center at (5.5 * GRASS_CELL_SIZE, 0.5 * GRASS_CELL_SIZE).
        g.density[5] = 0.42;
        let x = 5.5 * GRASS_CELL_SIZE;
        let y = 0.5 * GRASS_CELL_SIZE;
        let val = g.bilinear_sample(x, y);
        assert!(
            (val - 0.42).abs() < 1e-5,
            "bilinear_sample at cell center must equal density; got {val}"
        );
    }

    /// Test 6: bilinear sample at midpoint between two cells equals their average.
    #[test]
    fn bilinear_sample_at_midpoint_is_average() {
        let mut g = fresh_grid();
        // Cell (0,0) at index 0, cell (1,0) at index 1.
        g.density[0] = 0.0;
        g.density[1] = 1.0;
        // Cell-0 center at (0.5 * size, 0.5 * size); cell-1 center at (1.5 * size, 0.5 * size).
        // Midpoint x = 1.0 * size, y = 0.5 * size.
        let val = g.bilinear_sample(GRASS_CELL_SIZE, 0.5 * GRASS_CELL_SIZE);
        assert!(
            (val - 0.5).abs() < 1e-5,
            "bilinear midpoint must be average of neighbors; got {val}"
        );
    }

    /// Test 7 (walled): bilinear sample at x=0 returns the density of cell (0,0)
    /// and does NOT wrap to the far edge.
    #[test]
    fn bilinear_sample_walled_no_wrap() {
        let mut g = fresh_grid();
        // Only the corner cell (0,0) has density.
        g.density[0] = 0.5;
        // Far corner (dim-1, dim-1) has different density.
        g.density[GRASS_GRID_DIM * GRASS_GRID_DIM - 1] = 0.9;

        // Sample very close to (0,0) — ghost-zero boundaries mean it's mostly g.density[0].
        let val_corner = g.bilinear_sample(0.01, 0.01);
        // Should be influenced by g.density[0] = 0.5, NOT the far corner = 0.9.
        assert!(
            val_corner > 0.0,
            "near-origin sample must reflect cell(0,0) density"
        );
        // The far-corner cell must not affect the near-origin sample.
        assert!(
            val_corner < 0.8,
            "far corner (0.9) must not leak into near-origin sample via wrap; got {val_corner}"
        );
    }

    /// Below-threshold cell yields a partial bite. Density is drained to zero and
    /// energy is proportional (`energy_per_bite * taken / density_chunk`).
    #[test]
    fn consume_below_chunk_yields_partial_bite() {
        let mut g = fresh_grid();
        g.density[10] = 0.4;
        let taken = g.consume(10, 0.5, 10.0);
        let expected = 10.0 * (0.4 / 0.5);
        assert!(
            (taken - expected).abs() < 1e-5,
            "partial bite must scale energy; expected {expected}, got {taken}"
        );
        assert!(
            g.density[10].abs() < 1e-6,
            "density must drain to 0; got {}",
            g.density[10]
        );
    }

    /// Consume on an empty cell returns zero.
    #[test]
    fn consume_returns_zero_on_empty() {
        let mut g = fresh_grid();
        let taken = g.consume(0, 0.5, 10.0);
        assert_eq!(taken, 0.0, "consume on empty cell must return 0");
        assert_eq!(g.density[0], 0.0, "density must stay 0");
    }

    /// Ripe cell yields the configured energy and drains by the configured chunk.
    #[test]
    fn consume_ripe_cell_yields_energy_and_drains() {
        use crate::constants::GRASS_MAX;
        let mut g = fresh_grid();
        g.density[0] = GRASS_MAX;
        let taken = g.consume(0, 0.5, 7.5);
        assert!(
            (taken - 7.5).abs() < 1e-6,
            "consume should return the energy_per_bite arg; got {taken}"
        );
        assert!(
            (g.density[0] - (GRASS_MAX - 0.5)).abs() < 1e-6,
            "density must drop by exactly density_chunk; got {}",
            g.density[0]
        );
    }

    /// Test 11: for_each_cell_overlapping_circle with tiny radius finds exactly one cell.
    #[test]
    fn cells_overlapping_circle_finds_central_cell() {
        let g = fresh_grid();
        // Place the query at the last cell's center; a sub-cell radius hits only it.
        let ix = GRASS_GRID_DIM - 1;
        let iy = GRASS_GRID_DIM - 1;
        let cx = (ix as f32 + 0.5) * GRASS_CELL_SIZE;
        let cy = (iy as f32 + 0.5) * GRASS_CELL_SIZE;
        let mut hits = vec![];
        // Radius strictly less than half a cell guarantees a single hit.
        g.for_each_cell_overlapping_circle(cx, cy, GRASS_CELL_SIZE * 0.1, |idx| hits.push(idx));
        assert_eq!(
            hits.len(),
            1,
            "tiny circle at cell center should hit exactly 1 cell; got {:?}",
            hits
        );
        let expected = iy * GRASS_GRID_DIM + ix;
        assert_eq!(
            hits[0], expected,
            "hit cell {}, expected {}",
            hits[0], expected
        );
    }

    /// Test 12 (walled): for_each_cell_overlapping_circle at world edge does NOT
    /// wrap to cells on the far side.
    #[test]
    fn for_each_cell_skips_out_of_world() {
        let g = fresh_grid();
        // Circle near origin with sub-cell radius — cell (0,0) is hit via circle-vs-AABB
        // (the query point lies inside the cell's box), the far edge is not.
        let mut ix_hits = std::collections::BTreeSet::new();
        g.for_each_cell_overlapping_circle(1.0, 1.0, 3.0, |idx| {
            ix_hits.insert(idx % GRASS_GRID_DIM);
        });
        assert!(
            ix_hits.contains(&0),
            "ix=0 must be in the hit set; got {:?}",
            ix_hits
        );
        assert!(
            !ix_hits.contains(&(GRASS_GRID_DIM - 1)),
            "ix=dim-1 must NOT be in the hit set (walled world, no wrap); got {:?}",
            ix_hits
        );
    }

    /// Test 13: for_each_cell_overlapping_circle matches brute-force circle-vs-AABB scan.
    /// The overlap test uses the nearest-point-on-cell-box distance (not center-to-center)
    /// so that a creature circle touching a cell's edge or corner still counts as overlapping.
    #[test]
    fn cells_overlapping_circle_count_matches_brute_force() {
        let g = fresh_grid();
        // Use a fixed deterministic position and radius away from edges.
        let cx = 37.5_f32;
        let cy = 22.5_f32;
        let r = 4.0_f32;

        // Collect via the method under test.
        let mut method_hits = std::collections::BTreeSet::new();
        g.for_each_cell_overlapping_circle(cx, cy, r, |idx| {
            method_hits.insert(idx);
        });

        // Brute-force: iterate all cells, compute circle-vs-AABB (nearest point on cell box).
        let mut brute_hits = std::collections::BTreeSet::new();
        for iy in 0..GRASS_GRID_DIM {
            for ix in 0..GRASS_GRID_DIM {
                let box_x_min = ix as f32 * GRASS_CELL_SIZE;
                let box_x_max = box_x_min + GRASS_CELL_SIZE;
                let box_y_min = iy as f32 * GRASS_CELL_SIZE;
                let box_y_max = box_y_min + GRASS_CELL_SIZE;
                let nearest_x = cx.clamp(box_x_min, box_x_max);
                let nearest_y = cy.clamp(box_y_min, box_y_max);
                let dx = nearest_x - cx;
                let dy = nearest_y - cy;
                if dx * dx + dy * dy <= r * r {
                    brute_hits.insert(iy * GRASS_GRID_DIM + ix);
                }
            }
        }

        assert_eq!(
            method_hits, brute_hits,
            "for_each_cell_overlapping_circle must match brute-force circle-vs-AABB scan"
        );
    }

    /// Test 14: initial seed is deterministic and produces the expected density pattern.
    #[test]
    fn initial_seed_is_deterministic() {
        let mut rng1 = SimRng::from_u64(42);
        let g1 = GrassGrid::new(&mut rng1, 16, TEST_DIMS);
        let mut rng2 = SimRng::from_u64(42);
        let g2 = GrassGrid::new(&mut rng2, 16, TEST_DIMS);
        assert_eq!(
            g1.density, g2.density,
            "same seed must produce identical density vectors"
        );
        let max_count = g1
            .density
            .iter()
            .filter(|&&d| (d - GRASS_MAX).abs() < 1e-6)
            .count();
        assert!(
            max_count <= 16,
            "seeded cell count must be ≤ initial_seed_count; got {max_count}"
        );
        assert!(
            max_count >= 1,
            "at least one cell must be seeded; got {max_count}"
        );
    }

    /// v1.5 S5b: per-row bitset reflects current density state.
    #[test]
    fn row_bitset_tracks_density() {
        let mut g = fresh_grid();
        // No density anywhere → all bits zero.
        g.rebuild_row_bitset();
        for iy in 0..GRASS_GRID_DIM {
            assert!(!g.row_nonempty(iy), "row {iy} must be empty initially");
        }
        // Seed cell (5, 7) — row 7 must now flag non-empty.
        g.density[7 * GRASS_GRID_DIM + 5] = 0.4;
        g.rebuild_row_bitset();
        assert!(
            g.row_nonempty(7),
            "row 7 must be flagged after seeding (5,7)"
        );
        for iy in 0..GRASS_GRID_DIM {
            if iy != 7 {
                assert!(!g.row_nonempty(iy), "row {iy} must remain empty");
            }
        }
    }

    /// Test 15: density and scratch vectors have correct length after new().
    #[test]
    fn density_length_invariant() {
        let mut rng = SimRng::from_u64(0);
        let g = GrassGrid::new(&mut rng, 8, TEST_DIMS);
        assert_eq!(
            g.density.len(),
            GRASS_CELL_COUNT,
            "density.len() must equal GRASS_CELL_COUNT"
        );
        assert_eq!(
            g.scratch.len(),
            GRASS_CELL_COUNT,
            "scratch.len() must equal GRASS_CELL_COUNT"
        );
    }
}

// v2.0.1 GRASS wave (#5 organic shape + #6 frontier/snapshot perf) tests live in
// their OWN file/module (merge-hazard rule: never appended to the shared `tests`
// module above). It is a child of `grass` so it can reach internal fields/fns.
#[cfg(test)]
#[path = "grass_v201_tests.rs"]
mod grass_v201_tests;
