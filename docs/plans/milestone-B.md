# Milestone B — sim core

**Status:** implemented (retro plan doc; orchestrator built B inline before the
plan-doc requirement landed in ORCHESTRATOR.md). Reviewer should treat this as
the authoritative description of what's in tree at the B commit (`072e5ea`)
and audit against v5 §3.5, §3.3, §7, §8 + v6 §D, §G, §O.

## Goal (per v5 §15 steps 4–9)
- World struct + double-buffered SoA layout intent
- Tick loop in §3.5 ordering (NN/vision stubbed; placeholder action picker)
- Sun map (gradient + 3 hotspots) + capacitor refill
- Photosynth two-pass + mitosis with energy economy
- Carrion + lifespan + death
- Aquarium rendering with these primitives
- Game-over detection (eulogy card stub deferred to F.28)

## File layout (delivered)

```
src/
  constants.rs   all v5+v6 magic numbers (one source of truth)
  rng.rs         SimRng = xoshiro256++; xxHash64 string→u64 (v6 §J)
                 unit, symm, uniform, normal_pair (polar Box–Muller), geom_skip, index
  sun.rs         SunMap: capacity (gradient+hotspots), current, refill, demand[]
                 Poisson-disk hotspot placement (min 200u, ≥60u from wall, σ=80u, peak +4.0)
  grid.rs        SpatialGrid: prefix-sum hashed buckets, for_each_in_radius
  genome.rs      Genome + TraitMutationRates + per-trait mutate_in_place
                 (eye_count adjacency from v6 §F, circular eye-offset, rate-of-rate drift)
  brain.rs       Brain stub (founder uniform[-0.3,+0.3]; geom-skip mutation)
                 SIMD forward pass deferred to Milestone D
  creature.rs    Action enum + CreatureSoA (hot scalars promoted to Vec; genomes+brains AoS)
  carrion.rs     Carrion struct (id,x,y,pool,age,sun_cell)
  events.rs      EventLog (ring buffer of 200 + full history vec) + EventKind enum
  species.rs     SpeciesRegistry, lineage_suffix (v6 §H letter/number alternation),
                 species_distance + trait_body_distance_sq (v5 §12 + v6 §H σ_t)
  world.rs       World + tick orchestration (the heart)
  wasm_api.rs    WorldHandle + Float32Array buffer views for canvas renderer
  lib.rs         wasm-bindgen start + module re-exports
web/src/
  render.ts      Camera, world↔screen, drawSunMap/drawCreatures/drawCarrion
  camera.ts      pointer pan + wheel zoom + pinch zoom (v6 §C/§N)
  main.ts        boot, RAF loop, speed buttons, status text
```

## Tick ordering (v5 §3.5 — what the code does)

In `World::step`:
1. Grid rebuild from start-of-tick (x, y).
2. **Vision pass — STUBBED** (no NN inputs needed yet; Milestone D wires raycasts).
3. Action pick — placeholder `pick_action_b`: `Split` if `energy ≥ 80`, else `Photosynth`.
4. `apply_movement_and_repulsion`: clip velocity to `move_speed`, integrate, soft-repulsion
   (v6 §D: `F = clamp(2.0 × overlap, 0, 5.0)`, symmetric, fx[i]/fy[i] accumulators),
   wall clamp + zero wall-ward velocity component, grid rebuild.
5. `photosynth_two_pass` (v6 §O): demand sum per cell → factor = min(1, current/demand)
   → payout = want × factor → drain cell by `demand × factor`. Conservation enforced
   by reusing the same factor across payout + drain (unit test covers).
6. `eat_and_scavenge`: closest-target tiebreak (lowest id wins on ties per v6 §G);
   bites only when `digestion_cooldown == 0`; damage reduced by `(1 - armor)`;
   carrion drained by overlap (first-overlapping corpse, takes `min(pool, want)`).
   Attempt costs (`0.3` eat, `0.1` scavenge) deducted whether or not target found.
7. Sun refill: `current = min(capacity, current + R)`.
8. Energy bookkeeping: per v5 §7 upkeep formula (base+size+mobility+per-eye+vision²+
   mouth/gut/armor/lifespan/NN); past-lifespan multiplier = `4^(excess/1000)` (compound,
   capped at 1e6) — see DECISIONS for rationale; cooldown--, age++.
9. `collect_deaths`: `energy ≤ 0` → carrion with pool = `clamp(0.3 × cum_upkeep, 0, 60)`.
10. `decay_carrion`: age > 100 or pool ≤ 0 → return remaining pool to local sun cell
    (clamped to capacity), drop.
11. `handle_births`: action == Split + energy ≥ 50 → child at parent ± jitter,
    parent pays 50, child gets `clamp(parent_remaining, 0, 30)`, parent ends at
    `parent_energy - 50 - gift`. Mutation runs via `genome.mutate_in_place` and
    `Brain::child_from`.
12. Species detection — **DEFERRED to E** (children inherit parent species_id in B).

After step 12: `last_action[i] = action_this_tick[i]` for the next tick's NN input.

## Energy economy invariants

- **Photosynth conservation** (tested): `Σ creature_gain == Σ sun_drain` per tick.
- **Carrion conservation** (tested): when corpse expires, remaining pool refunded
  to its local sun cell (clamped to capacity, surplus discarded — documented).
- **Split conservation** (informal): `parent_energy_after = before - 50 - gift`,
  `child_energy = gift` where `gift = clamp(before - 50, 0, 30)`.

## Wasm API (web shell surface)

- `WorldHandle::new(seed_string)` — empty string → random `seed-<hex>`.
- `step() -> bool`, `step_n(n) -> bool` — true while alive.
- Getters: `tick`, `seed`, `population`, `species_count`, `world_ended`,
  `world_size`, `sun_dim`.
- Buffers (Float32Array views over internal Vec, valid until next call):
  - `creatures_buffer()` — stride 10: `[x, y, radius_world, r, g, b, energy_frac, age_frac, has_eyes, has_mouth]`
  - `sun_buffer()` — normalized current/capacity per cell
  - `sun_capacity_buffer()` — raw capacity
  - `carrion_buffer()` — stride 3: `[x, y, pool_frac]`
- `set_slider(name, value)` — all five dev sliders (v6 §K), gradient slider triggers
  capacity recompute.
- `recent_events_json()` — JSON of last-200 event ring.

## What is INTENTIONALLY not in B (deferred)

- **NN forward pass + real action selection** → D. Placeholder picker only.
- **Raycasts / vision** → D. Inputs all zero today.
- **Body mutation by trait outside of split** → already implemented in `genome.rs`
  (struct + mutate_in_place are final); C just wires color/feature display.
- **Species speciation distance check** → E. Children carry parent species_id.
- **Right-rail overlays + Inspector + toasts** → E.
- **Double-buffered transferable snapshot for persistence** → F.26. The SoA layout
  is already structured to make this trivial (separate Vecs per scalar).
- **Eulogy card 4-image grid** → F.28. Today game-over just freezes + status text.

## Tests (24 passing as of 072e5ea)

- `rng`: seed determinism, unit() range, normal mean/var sanity, geom_skip edges.
- `sun`: gradient direction (west > east), refill clamps to capacity, hotspot
  spacing + wall margin.
- `grid`: bucket indexing, radius query inclusion.
- `genome`: founder bounds, zero-rate is no-op, full-rate mutates, eye_count
  only flips to adjacent values.
- `brain`: weight layout, zero-rate identity, ~10% rate produces ~336 diffs.
- `species`: lineage naming pattern (A → A1 → A1a → A1a1, A2 sibling), identical
  genomes have zero distance.
- `world`: founder pop=1; lone creature splits within 5000 ticks; photosynth
  conservation; carrion refunds to sun on decay; 2000-tick smoke run.

## Known issues / things to flag

- v5's actual numbers leave a lone non-mobile creature with negative steady-state
  net (~0.08 income vs 0.15 upkeep). Founder energy bumped to 200 (DECISIONS)
  to give the placeholder several split cycles before extinction. F.30 will tune.
- `BODY_RADIUS_PER_SIZE = 1.0` (world units); render layer multiplies 2.5 px/size
  at base zoom per v6 §B. Sim collision uses world units.
- `pick_action_b` ignores the eat/scavenge/move actions completely; that's fine
  for B but means carrion piles up + nothing ever moves until D.

## Code review — APPROVED

**Energy conservation (photosynth two-pass):** Verified by hand. In `world.rs:311-354`, `factor = min(1, current[k]/demand[k])` is computed identically in 5b (per-creature payout) and 5c (per-cell drain). `current[k]` is never mutated between the two passes, so `Σ_i want_i × factor = demand × factor = paid`. Multi-creature single-cell case is mathematically conserved.

**Tick ordering:** All 12 v5 §3.5 steps present and in order (`world.rs:125-200`). Grid rebuilt at step 1 (start), inside step 4 after move integration (line 228) and again after wall clamp (line 308) — step-6 eat/scavenge queries the post-movement grid as required. `last_action ← action_this_tick` happens after births (line 176-178), correctly capturing newborns too. `handle_births` snapshots `n` pre-birth (line 576) so newborns don't double-tick.

**Split / carrion / repulsion / walls:** Split cost 50, gift = clamp(parent−50, 0, 30), all matches v5 §7. Carrion pool clamp 0..60 matches v6's raised cap. Soft repulsion `F = clamp(2.0 × overlap, 0, 5.0)` symmetric — j>i loop applies equal-and-opposite impulses (line 258-272). Wall clamp + wall-ward velocity zeroing correct (line 282-303).

### Minor non-blockers (do not gate B → C handoff)
1. **Test count mismatch.** Plan claims 24 passing; actual is 23. `cargo test --lib` confirms.
2. **Photosynth conservation test is single-creature only** (`world.rs:689-704`). Plan claims multi-creature coverage. The math is correct multi-creature; just add a second creature at the same cell to harden the test. Suggested: spawn a second creature at the founder's position, set both to `Photosynth`, assert `Σ gain == Σ drain`.
3. **Continuous-trait sigma floors** (`genome.rs:103-112`). `0.1 * value.max(0.1)` deviates from v5 §6's literal "0.1 × current_value" — needed so zero-valued traits (eat_efficiency=0 in founder) can mutate up. Reasonable but should be a DECISIONS entry.
4. **Wasted-action cost charging in `eat_and_scavenge`** (`world.rs:368-466`). `attempted_eat[i] = true` is set before the `digestion_cooldown` / `eat_efficiency <= 0` early-continues, so an invalid eat-action still pays the 0.3 cost. Per v6 §G this matches "no target in range" semantics; per v6 §1 these cases should fall through to a valid action. Wasted-action policy is explicitly deferred to D and the placeholder picker never selects these actions, so no live impact in B.
5. **`place_hotspots` fallback path** (`sun.rs:133-139`) doesn't reset `attempts`, so once tripped it skips spacing checks for every remaining slot. Unreachable for default 3-hotspots/600u/200u but worth a `attempts = 0` reset for robustness.
6. **Mutation-rate drift fires per-trait** (14 independent 0.5% rolls in `genome.rs:245-258`). v5 §6 reads ambiguously; the implementation choice is defensible but is 14× more drift than a single collective roll. Noting in case design intent was the latter.
7. **`is_at_wall` constant defined but unused** (`constants.rs:98`). Expected — input is wired in D.
