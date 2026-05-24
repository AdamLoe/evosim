# Per-tick allocation audit (sim hot path)

Scope: `src/{world,creature,grid,vision,brain,sun,carrion,events}.rs`.
"Per-tick?" = call frequency in steady state: **Yes** = always on every tick, **Sometimes** = only on specific events (birth, death, speciation, milestone), **No** = setup / load / test code only.

Already addressed (recent commits — do not rework):
- `perf-1-sector-trig`: `eye_trig` SoA cache → eliminates 24× sin/cos per creature per tick in vision.
- `perf-2-scratch-pool`: 9 `scratch_*` Vecs on `World` (fx, fy, neighbors, damage, gain, cooldown_set, attempted_eat, attempted_scavenge, got_a_bite) replace per-tick `vec![]` allocs in `apply_movement_and_repulsion` + `eat_and_scavenge`.
- `perf-3-grid-cursor`: `SpatialGrid::cursors` reused via `copy_from_slice(&starts)` — no per-rebuild clone.
- `perf-5-genome-soa`: 7 hot `g_*` mirror Vecs on `CreatureSoA` — hot loops no longer deref full `Genome` (~208 B).

## Allocation table

| file:line | allocation | per-tick? | fix |
|---|---|---|---|
| `world.rs:437,439` (threads) | `Vec<(f32,f32,Action)>` from rayon `flat_map().collect()` for NN forward results — N × 12 B | **Yes** | Pool a `scratch_nn_results: Vec<(f32,f32,Action)>` on `World`; resize+fill in place. Or skip materialization and write directly into per-chunk slices of `vx`/`vy`/`action_this_tick` (requires unsafe split or `par_chunks_mut` over a triple SoA, but eliminates the alloc and the drain loop). |
| `world.rs:676` | `let mut candidates: Vec<usize> = Vec::with_capacity(8);` inside `eat_and_scavenge` `Action::Eat` arm | **Yes** (per eater per tick, N_eaters × tick) | Replace with `smallvec::SmallVec<[usize; 16]>` (heap-free in common case) **or** lift `candidates` onto `World` as another scratch field (drained per-creature via `clear()`). Capacity 8 is too small — overlap with 5u cells in dense regions can exceed it, triggering realloc. |
| `world.rs:831` | `let mut dead: Vec<usize> = Vec::new();` in `collect_deaths` | **Yes** (returned, consumed by caller) | Promote to `scratch_dead: Vec<usize>` field; caller already drains; remove the `Vec` return. |
| `world.rs:834` | `let mut species_lost: Vec<u32> = Vec::new();` in `collect_deaths` | **Yes** | Same pattern: `scratch_species_lost: Vec<u32>` on `World`. Already eagerly extended into `pending_extinction_check` — collapse it: push directly into `pending_extinction_check`. |
| `world.rs:1008` | `let mut alive = std::collections::HashSet::<u32>::new();` in `finalize_extinctions` | **Yes** (called every tick from `tick_once`) | `species_id` is dense and small (≤ live_species_count, typically <100). Use a `scratch_alive_species: Vec<bool>` indexed by species id, resized to `species.list.len()` and cleared each call. Or `smallvec`/sorted `Vec` + dedup. Avoids HashSet's heap + hashing. |
| `world.rs:1012` | `std::mem::take(&mut self.pending_extinction_check)` allocs replacement empty Vec | **Yes** | `mem::take` only swaps in `Vec::new()` which is zero-sized; cheap. **No fix needed**, but if you want to keep the high-water mark, swap with a stashed empty Vec from `World`. |
| `world.rs:479,480,484,853,854,860,861,872,873,892,893,977,1019` | `genome.clone()`, `species_name.clone()` for `HallOfFame` and events | **Sometimes** (per death / per speciation / per first-mover) | Death-rate-bounded (≤ N/tick worst case but realistically <1%). Genome is ~208 B; Brain is **not** cloned for HoF (good). HoF clones are necessary — `HallOfFame` owns the genome. **Lower-priority**, but `species_name` is cloned 2–3× per death; cache it in a local once and `Rc<str>` / `Arc<str>` if HoF retention becomes an issue. |
| `world.rs:943,972,973` | `child_genome = self.creatures.genomes[i].clone()` and child brain weights clone (within `Brain::child_from`) | **Sometimes** (per birth) | Genome clone (~208 B) is fundamental — child needs its own. **Brain clone** (`brain.rs:169`, see below) is the big one: 13.5 KB heap per birth. Lower priority than the per-tick items above unless birth rate is high. |
| `world.rs:153,176,177` (init only) `pending_extinction_check: Vec::new()`, scratch fields | **No** (setup) | Already pre-sized lazily on first tick via `resize`. **OK as-is** but consider `with_capacity(2048)` to avoid the first-tick growth cascade. |
| `world.rs:1101` | `let vision: Vec<VisionBuf> = vec![[0.0f32; VISION_LEN]; n];` (load path) | **No** | Setup-only. **OK.** |
| `world.rs:2154` | `format!("e20-live-{seed_n}")` | **No** (test) | Test. **OK.** |
| `vision.rs:327` | `dst.resize_with(total, Vec::new)` in `build_cell_to_carrion` (one-shot grow to HASH_DIM²) | **No** (first tick only) | Steady-state: just `clear()` each existing inner Vec. **OK.** |
| `vision.rs:329-331` | `for cell in dst.iter_mut() { cell.clear(); }` then `dst[cell].push(ci as u32)` | **Yes** (the inner `Vec<u32>` may grow) | `cell_to_carrion: Vec<Vec<u32>>` is **the remaining big gap**. 14,400 inner Vecs × pointer triple (24 B) = 345 KB of headers, plus heap fragmentation as carrion churns. Replace with CSR layout: `cell_starts: Vec<u32>` (len HASH_DIM²+1) + `cell_indices: Vec<u32>` (len = carrion count). Build identical to `SpatialGrid::rebuild` — 2 passes, **zero per-cell heap allocs**, contiguous cache-friendly reads. Big win. |
| `brain.rs:41,85` | `vec![0.0_f32; NN_WEIGHT_COUNT]` (3456 floats = 13.5 KB) in `Brain::founder`/`zero` | **No** (founder once; `zero` only test) | OK. |
| `brain.rs:169` | `let mut child = parent.clone();` inside `Brain::child_from` — clones 13.5 KB weights | **Sometimes** (per birth) | The whole weights Vec is cloned then ~p×3456 entries mutated in place. **Could pool brains**: when a creature dies, push its `Vec<f32>` (capacity already correct) onto a freelist; new births `pop()` a buffer and `copy_from_slice(&parent.weights)` (memcpy, no alloc). Saves 13.5 KB heap alloc + 13.5 KB free per birth. Births/death is the dominant alloc once population stabilizes. **High priority**, not yet addressed. |
| `grid.rs:30,31,34` (init only) | `vec![0; HASH_DIM²+1]`, `Vec::with_capacity(2048)` | **No** | Setup. **OK.** Already addressed by perf-3. |
| `sun.rs:23,31,32` (init only) | `vec![0.0; SUN_DIM²]` × 3 | **No** | Setup. **OK.** |
| `events.rs:63` | `self.recent.push_back(ev.clone())` | **Sometimes** (per emitted event) | Event is small (`u32` + enum with at most a String inside). String clones (`species_name`) inside `EventKind::Speciation`/`Extinction` are the real cost. Lower-priority — event emission is rare. Could use `Rc<str>` for species name across `Event`/`HallOfFame`/`species.list`. |
| `creature.rs:104-129` (init only) | 25× `Vec::with_capacity(cap)` in `CreatureSoA::with_capacity` | **No** | Setup. **OK.** |

## Per-tick summary (steady state)

After perf-1..5, the **only** per-tick heap allocations on the hot path are:

1. **`Vec::with_capacity(8)` in `eat_and_scavenge`** (line 676) — N_eaters × tick.
2. **`Vec::new()` for dead + species_lost** in `collect_deaths` (831, 834).
3. **`HashSet::new()` in `finalize_extinctions`** (1008) — 1× tick, but allocates buckets.
4. **Threads-path `flat_map().collect::<Vec<_>>()`** in `nn_forward_all_chunks` (437, 439) — 2 nested collects per tick, allocating N × 12 B + overhead.
5. **`cell_to_carrion` inner Vec growth** in `build_cell_to_carrion` (vision.rs:335) — bounded by carrion churn per cell, but 14,400 Vec headers always resident.

Fix order by expected impact:
- **#5 (CSR cell_to_carrion)** — biggest memory footprint reduction + cache win.
- **#4 (NN-results pool or in-place writes)** — N × 12 B every tick under threads feature.
- **#1 (smallvec/scratch candidates)** — N_eaters × tick; very hot.
- **#2, #3 (small scratch Vec / dense bool vec for alive species)** — modest, but cheap to do.

## Per-birth (high-rate when pop > stable)

- **Brain weight clone (13.5 KB)** in `Brain::child_from`. Pool freed weight Vecs from deaths. (`brain.rs:169`)
- Genome clone (~208 B) is unavoidable per spec.

## Struct sizes (mental math)

| Type | Size | Notes |
|---|---|---|
| `Genome` | ~208 B (4+4+4·4+1+pad+24·4+4+4·2+12+14·4) | Cloned per birth + per HoF capture. Stays AoS in `creatures.genomes` (cold reads via `g_*` mirrors). **OK.** |
| `Brain` | 24 B struct + **13.5 KB heap** (3456 × f32) | Cloned per birth; pool the heap (see #4 fix). |
| `Carrion` | 32 B (u64+f32×3+u32+usize) | Vec is fine; swap_remove. |
| `Event` | tag + 4-byte tick + largest variant ~ 24-32 B (String=24 B in variant) | Cheap. |
| `HallOfFame` | ~240 B (Genome ~208 + u64 + String + u32×3) | 5 instances on `World`. Inexpensive. |
| `CreatureSoA` (struct itself, not contents) | 25 Vec headers × 24 B = ~600 B | Trivial; contents dominate. |
| `SpatialGrid` | 3 Vecs (~72 B + ~57 KB heap × 2 + indices) | Fine. |
| `SunMap` | 3 Vecs × 24 B + 3·(20·20·4 = 1.6 KB) + 6 floats hotspots | Fine. |

No "fat structs cloned in hot paths" beyond `Brain` (~13.5 KB per birth clone, already noted) and `Genome` (208 B, per birth + per HoF — acceptable).

## Notes on borrowed-from-self patterns

- `collect_deaths` returns a `Vec<usize>` which the caller drains. Easy win to make it pooled (#2). The current path is: allocate, fill, hand off, walk back-to-front, drop. Replace with `&mut self.scratch_dead` + clear-on-entry.
- `std::mem::take(&mut self.pending_extinction_check)` is fine — `Vec::new()` is zero-cost — but if `pending_extinction_check` ever grows large the take-and-replace forfeits the high-water-mark capacity. Consider stashing a sister `pending_extinction_buf` to swap with.

## Files audited but no per-tick allocs found

- `src/carrion.rs` — pure data struct, no allocations.
- `src/sun.rs` — all allocations are constructor-only.
- `src/grid.rs` — perf-3 already pooled the only per-tick alloc.
