# evosim — PITCH v5 (canonical spec for v1 build)

> Supersedes v4. v5 incorporates findings from a second round of multi-perspective review (game-design, engineering, evo-bio, scope). Subagents working on v1 should treat this as the source of truth.

## What changed from v4

**Bug / underspec fixes:**
1. **Within-tick ordering rewritten (§3.5)** — two-pass photosynth (gather demand → proportional payout → drain). Grid rebuilt after movement. Eat/scavenge/death resolved on post-movement positions. Fixes the energy-conservation hazard reviewers caught.
2. **Species distance function fully specified (§12)** — z-scored per-trait, body-weighted vs brain-weighted, circular-distance for eye offsets, unused eye slots excluded.
3. **"Weirdest creature" pinned (§11)** — max genome-distance from day-0 ancestor among creatures that lived ≥ 500 ticks.
4. **Acceptance test gets snapshot hash + perf budget (§16)** — deterministic CI test, no eyeball checks.
5. **Milestone C relabeled sequential (§15)**.
6. **Raycast budget added to tick budget (§3.5)** with explicit cost line.
7. **Double-buffered SoA + transferable handoff spec'd (§13, §15 F.25)** — autosave no longer hitches the main thread.

**Tuning changes (validate in F.29 balance pass):**
8. **Sun refill rate R dropped 0.5 → 0.08/tick** — capacitor now creates real time-structured depletion.
9. **Eat gains a digestion cooldown** (50-tick refractory period after a successful bite) — closes the predator trapdoor by capping bite-payoff per unit time, not just by mouth-tax math.
10. **Body mutation rate raised 1% → 3% per trait** (NN per-weight rate unchanged at 2%) — gives body drift enough signal to compete with brain drift in species detection.
11. **World shrunk 1000×1000 → 600×600** — better aquarium density at 1–2k creatures.

**Polish adopted:**
12. **Sun map = gradient + 3 Perlin/blob hotspots** instead of pure linear gradient — creates real spatial niches.
13. **In-aquarium event toasts + creature highlight ring** on speciation/extinction — drama no longer buried in the rail.
14. **Eulogy card is a 4-image grid** ("first to move," "biggest," "weirdest," "last survivor") with prominent seed display + Copy-Share button.

## 1. North star

A browser-deployed idle evolution sandbox. Open a tab, see a single cell. Watch evolution unfold over minutes-to-hours. The map's photosynthetic income is the compute budget; the energy economy self-regulates how much complexity the world can support.

## 2. Stack

- **Language/runtime:** Rust → WebAssembly.
- **Compute parallelism:** `rayon` via `wasm-bindgen-rayon` when COOP/COEP headers are set; single-threaded fallback otherwise. **No WebGPU.**
- **Frontend:** TS/JS shell, HTML5 Canvas 2D rendering.
- **Persistence:** IndexedDB autosave (JSON via `serde_json`).
- **Deployment:** Cloudflare Pages or similar host that can set COOP/COEP headers. Document required headers in README. Single-threaded fallback ensures the URL still works on hosts that don't.
- **Pinned dependencies:** `wasm-bindgen`, `wasm-bindgen-rayon`, `getrandom` (with `js` feature), `serde`/`serde_json`, `wide` for SIMD, a fast PRNG crate (`xoshiro` / `rand_xoshiro`).

## 3. Game design

### 3.1 Core loop
1. World begins with one cell at center, minimal genome, random NN weights.
2. Photosynth → energy → split → lineage grows.
3. Body genome and NN weights mutate on each birth (independent processes; see §5–6).
4. Energy economy + regenerating-sun-with-hotspots + carrion + lifespan create selection pressure.
5. Speciation auto-detected; events overlay logs them; in-aquarium toast highlights the moment.
6. Population zero → eulogy card with stats + 4-image hall-of-fame + New World button.

### 3.2 Tick rate
- Sim decoupled from render. User slider: pause / 1x / 10x / 100x.
- Render samples world state at 30fps.
- Sim runs only while the tab is open.

### 3.3 World shape
- 2D continuous floats, **600×600 units**, hard walls (clamp position, zero wall-ward velocity to avoid corner-jamming with NN-driven movement).
- **Single spatial-hash grid** at 5u cells (so 120×120 = 14,400 cells). Coarser queries derive by cell-stride.
- **Sun map** on a 30u grid (20×20 = 400 cells). Each cell has `sun_capacity` (max) and `sun_current` (running level). Capacity = `gradient(x,y) + hotspot_field(x,y)` where:
  - `gradient(x,y)` = linear east-west, 1.0 east → 3.0 west.
  - `hotspot_field(x,y)` = sum of 3 Gaussian bumps with σ=80u and peak +4.0, placed at randomized positions on world creation (deterministic given seed).
  - Effective capacity ranges roughly 1.0–7.0 across the map.
- `sun_current` refills toward capacity at rate **R = 0.08/tick** (was 0.5 in v4; lower R → real depletion windows). Refill formula: `sun_current = min(sun_capacity, sun_current + R)`.

### 3.4 RNG / determinism
Fully seedable. World-creation panel includes optional seed input; if unseeded, generate a random seed and display it prominently. Single `SimRng` (xoshiro256++) seeded once, threaded through tick functions. Non-deterministic threading (rayon) must use deterministic chunking + per-chunk sub-RNGs.

### 3.5 Within-tick ordering (rewritten)

**One sim tick executes deterministically in this order, on the world snapshot:**

1. **Rebuild spatial hash grid** from start-of-tick positions.
2. **Vision pass:** each creature raycasts into the grid; cache sector inputs in a per-creature scratch buffer.
3. **NN forward pass:** consume cached vision + self-state inputs; produce velocity + chosen discrete action; cache actions.
4. **Apply velocities** to positions; apply soft-repulsion forces; clamp to walls. **Rebuild grid** against new positions (cheap — re-bucket from positions).
5. **Photosynth two-pass (energy-conservation safe):**
   - 5a. Aggregate per-sun-cell *demand*: sum over all creatures choosing `photosynth` in that cell, weighted by `photosynth_efficiency × size`.
   - 5b. Proportional payout: each photosynther gets `(0.5 × eff × size) × min(1, sun_current / demand)` energy. (No over-draw possible.)
   - 5c. Drain the sun cell by total payout served.
6. **Eat / scavenge resolution** on post-movement positions:
   - Eat: for each creature choosing `eat`, find one target within `bite_reach × size` units; if found AND attacker is not in `digestion_cooldown`: deal `1.0 × attacker.size` damage, gain `1.5 × eat_efficiency` energy, set `digestion_cooldown = 50`.
   - Scavenge: for each creature choosing `scavenge` and overlapping a corpse, drain `1.5 × scavenge_efficiency` from the corpse pool into creature energy.
7. **Sun refill:** every sun cell refills `sun_current += R` (clamped to capacity).
8. **Energy bookkeeping:** subtract upkeep (§7) from every creature; decrement `digestion_cooldown` if > 0; increment `age`.
9. **Deaths → carrion:** creatures with energy ≤ 0 become corpses (pool = 0.3 × cumulative_upkeep_paid, capped at 60). Past-lifespan multiplier applied during upkeep, not here.
10. **Carrion decay:** corpses past 100 ticks since death OR fully drained → removed; remaining pool returns to local sun cell's `sun_current` (clamped to capacity).
11. **Births:** any creature whose chosen action was `split` and has energy ≥ 50 produces one child (genome mutated per §6; NN mutated per §5.3; child placed at parent's position with small jitter; parent's energy reduced by 50).
12. **Species detection** (one distance check per child, §12).

### 3.6 Tick budget (target single-threaded, 1500 creatures @ 30 tps)

| Stage | Estimated cost |
|---|---|
| Grid rebuild (×2) | ~0.2 ms |
| Raycasts (1500 × ~12 active rays avg × ~10 cells/ray) | ~3–5 ms (dominant) |
| NN forward (1500 × 130×24 + 24×8) | ~0.5 ms with SIMD |
| Soft repulsion | ~0.3 ms |
| Photosynth + eat + scavenge + energy + births | ~1 ms |
| **Total budget** | **~5–7 ms / tick** (~33ms at 100x speed-multiplied tick batching) |

Raycasts are the real bottleneck. Mitigations baked into spec:
- Vision range quadratic upkeep (`0.001 × r²`) — selection naturally pushes ranges shorter.
- Cap `vision_range` at 80u (was 100u) to bound worst-case ray traversal.
- Single shared spatial grid serves both raycasts and repulsion queries — no parallel structures.

If 30 tps × 100x is too tight on a single thread, the tick scheduler should fall back to running fewer sim-ticks per render frame and let the user observe at lower effective speed.

## 4. Genome

```rust
struct Genome {
    size: f32,                      // 1.0 .. 10.0
    max_age: u32,                   // ticks
    photosynth_efficiency: f32,     // 0.0 .. 2.0
    eat_efficiency: f32,            // 0.0 (off) .. 2.0
    scavenge_efficiency: f32,       // 0.0 (off) .. 2.0
    move_speed: f32,                // 0.0 (off) .. 5.0
    eye_count: u8,                  // {0, 2, 3, 4, 6, 8, 12, 24}
    eye_offsets: [f32; 24],         // angle per slot; only first eye_count used
    vision_range: f32,              // 0.0 .. 80.0
    armor: f32,                     // 0.0 .. 1.0
    bite_reach: f32,                // 1.0 .. 3.0 (× body radius)
    pigment_r: f32,                 // 0..1
    pigment_g: f32,
    pigment_b: f32,
    mutation_rates: TraitMutationRates,    // per-trait, evolvable
}

struct Brain {
    nn_weights: Vec<f32>,               // fixed-size
    nn_mutation_rate: f32,              // per-weight, evolvable
}
```

Runtime state (not genome): `energy`, `age`, `digestion_cooldown`, `cumulative_upkeep`, `species_id`, `position`, `velocity`, `last_action`.

## 5. Brain

### 5.1 Architecture
Fixed for all creatures: **130 inputs → 24 hidden (ReLU) → 8 outputs**. ≈ 3,300 weights total.

**Input layout (130 total, fixed):**
- 10 self-state: `energy_frac`, `age_frac`, `size`, `vx`, `vy`, `is_at_wall`, `digestion_cooldown_frac`, `carrion_overlap` (0/1), 2 reserved zeros.
- 120 vision: 24 sectors × 5 features `(distance, target_size, target_r, target_g, target_b)`. Empty sectors return zeros.

**Outputs (8):**
- `velocity_x`, `velocity_y` (continuous, scaled by `genome.move_speed`)
- 6 discrete-action logits → argmax → `{rest, photosynth, eat, scavenge, split, signal}` (signal is no-op in v1, kept for v1.1 wiring)

### 5.2 Inference
CPU SIMD via `wide`. Forward passes batched in chunks across creatures with `rayon::par_iter` when threads available, sequential otherwise.

### 5.3 NN weight mutation
On birth, child inherits parent weights, then for each weight independently: with probability `brain.nn_mutation_rate` (default 0.02), perturb by `N(0, 0.02)`. **Optimization:** use geometric-skip sampling (`Geom(p)`) to pick next-mutated-index rather than 3,300 branches per birth.

`nn_mutation_rate` itself mutates at 0.5% per birth, jitter ±20%.

## 6. Body mutation

`TraitMutationRates` carries one f32 per body trait. Defaults at world start: **0.03 (3%)** (was 1% in v4). On birth, for each trait independently, with probability = its rate:
- Continuous traits: add `N(0, 0.1 × current_value)`, clamp to bounds.
- `eye_count`: pick uniformly from adjacent valid divisors of 24.
- `eye_offsets[i]`: only mutate slots `i < current_eye_count`; circular `N(0, 0.1 rad)`.

Mutation rates themselves mutate at 0.5% per birth, jitter ±20%.

## 7. Energy economy (starter values; five live sliders in §11)

### Income
| Source | Value |
|---|---|
| Photosynth | proportional-payout per §3.5 step 5 |
| Eat hit (when not in digestion cooldown) | deals `1.0 × attacker.size` damage; gains `1.5 × eat_efficiency` energy; sets `digestion_cooldown=50` ticks |
| Scavenge tick (overlapping corpse) | gains `1.5 × scavenge_efficiency` from corpse pool |

### Upkeep (per tick, summed)
| Term | Value |
|---|---|
| Base alive tax | 0.05 |
| Size beyond 1.0 | +0.04 per unit |
| Mobility flag (any nonzero move_speed) | +0.10 |
| Move speed (top) | +0.02 per unit |
| Per active eye | +0.02 |
| Vision range² | +0.001 × range² |
| Mouth tax (eat_efficiency > 0) | +0.05 standing |
| Gut tax (scavenge_efficiency > 0) | +0.02 standing |
| Armor | +0.03 per unit |
| Lifespan term | +0.01 per 1000 ticks of max_age |
| NN upkeep (fixed) | 0.05 constant |

### One-time costs
| Action | Cost |
|---|---|
| `eat` attempt | 0.3 |
| `scavenge` attempt | 0.1 |
| `split` (resolved in tick step 11) | 50 (child receives a gift of `min(parent_energy - 50, 30)`) |
| Movement per unit distance | 0.02 |
| `rest` / `photosynth` | 0 |

### Past-lifespan penalty
Past `max_age`: upkeep multiplied by 4 per 1000-tick excess (applied in step 8).

## 8. Death and carrion

- Energy ≤ 0 → death (step 9).
- Carrion lasts up to 100 ticks. Pool = `0.3 × cumulative_upkeep`, capped at **60** (was 30 in v4).
- Scavengers drain the pool while overlapping. Expired/drained corpses vanish; remaining pool returns to the local sun cell's `sun_current`, clamped to capacity.

## 9. Physics

Soft repulsion (boids-style) in step 4: pair force ∝ overlap depth, applied symmetrically. No hard collision. Position += velocity; velocity clipped to `genome.move_speed`. Wall: clamp position; wall-ward velocity component zeroed (prevents NN-driven corner deadlock).

## 10. Sensing

24 fixed angular sector slots. `eye_count` ∈ {0,2,3,4,6,8,12,24} selects how many physical eyes; they tile the 24 slots evenly starting from `eye_offsets[0]`. Each active sector raycasts one ray of length `vision_range` against the shared spatial grid. Returns first hit's `(distance, target_size, target_r, target_g, target_b)`. Inactive sectors: zeros.

## 11. UI — persistent aquarium

### Always-on canvas
Aquarium fills the viewport. Never hidden. Pan (drag) / zoom (scroll).

### Top bar
Day counter, population, species count, speed buttons (pause / 1x / 10x / 100x), sun-intensity slider (multiplies all `sun_capacity`), settings gear.

### Right rail (collapsible overlays, render *over* aquarium)
- **Events** (default open, ~280px wide): scrolling log with the active species list above it. Each line clickable → highlights that species in aquarium.
- **Stats** (hidden by default): two small line charts — population over time, species count over time.
- **Inspector** (opens when a creature is clicked): genome stats, species/parent breadcrumb, current action.

### In-aquarium event cues (NEW in v5)
When a speciation, extinction, or "first" event fires:
- A small toast appears in upper-right ("New species: Lineage A1a from Lineage A1").
- The relevant creature(s) get a brief glow ring (1.5s, fades).
- Camera does NOT auto-pan (no jarring jumps).

### Dev panel (hotkey `~`)
Five sliders, named:
- `base_sun_rate` (R, default 0.08)
- `mutation_rate_multiplier` (×1.0 default — scales all body+NN rates)
- `sun_gradient_strength` (×1.0 default)
- `mouth_tax` (default 0.05)
- `nn_mutation_sigma` (default 0.02)

All other costs are constants in v1.

### Eulogy card (game-over) — 4-image hall of fame
When population reaches zero:
- Dim aquarium, freeze final frame.
- Card shows: "Your world lived **N days**, peaked at **P creatures** across **S species**. Seed: **XYZ**."
- 2×2 image grid of canvas-rendered creatures:
  - **First to move** (first creature with `move_speed > 0`)
  - **Biggest** (max `size` reached during run)
  - **Weirdest** (see §11.1)
  - **Last survivor** (creature that died last)
- Buttons: **Copy Share Text** (formatted "evosim seed=XYZ — lived 47 days, peaked 3 species, see XX.yy"), **Download Save (.json)**, **New World**.

### §11.1 "Weirdest" definition
Among creatures that lived ≥ 500 ticks, the one with maximum z-scored Mahalanobis-style genome distance to the day-0 founder genome (using the same metric as §12). If none qualify, fall back to the longest-lived creature.

## 12. Species detection — precise metric

For each new birth, compute distance to its species' anchor genome. If > threshold, declare new species: assign procedural name (`Lineage A1a` derived from parent name), record parent species link, log a "speciation" event with toast.

### 12.1 Distance function
```
dist(g1, g2) =
    W_body  * sqrt( Σ_t ((g1.t - g2.t) / σ_t)²
                  + categorical_jump_cost(g1.eye_count, g2.eye_count)
                  + Σ_{i < min(eye_count)} circular_dist²(eye_offsets[i]_1, eye_offsets[i]_2) / σ_eo² )
  + W_brain * (||nn_weights_1 - nn_weights_2|| / sqrt(num_weights))
```

- `σ_t` = per-trait scale (configured constants, roughly the standard deviation expected over many generations).
- `categorical_jump_cost` for `eye_count` = 1.5 if different valid divisor, 0 otherwise.
- `eye_offsets[i]` only summed for indices used by **both** creatures (i < min of their `eye_count`s).
- Circular distance: `min(|a-b|, 2π - |a-b|)`.
- `W_body` = 3.0, `W_brain` = 1.0 — body drift weighted heavier so NN noise doesn't swamp speciation signal.

### 12.2 Threshold
Default threshold = 4.0 (≈ "several standard deviations of body change OR equivalent brain shift"). Recorded relative to species *anchor* (the genome at speciation moment), not to live parent — prevents drift creep from declaring constant speciation.

### 12.3 Anchor maintenance
When a creature is declared a new species, store its genome as that species' anchor. All future births of that lineage measure distance against that anchor.

## 13. Persistence

- **Double-buffered SoA world layout:** the world is stored in two parallel buffer sets. Each tick writes to the back buffer; the previous buffer is available for read (snapshot).
- Every 10 sim-seconds, the front buffer is *transferred* (zero-copy via `Transferable`) to a Web Worker. The worker JSON-encodes via `serde_json` and writes to IndexedDB. Sim continues on the other buffer.
- On reopen: prompt "Resume world from day X (seed: …)?" or "New World".
- Eulogy card "Download Save" exports the same JSON.

## 14. MVP scope (v1)

**Included:**
- Rust+wasm sim with rayon (threaded when possible) + single-thread fallback.
- 1–2k creature target, 600×600 hard-walled world.
- Full genome (§4), separate body & NN mutation (§5–6).
- Regenerating sun map with gradient + 3 hotspots, R=0.08.
- Soft repulsion physics + digestion cooldown.
- Persistent aquarium UI with right-rail overlays + in-aquarium event toasts/highlights.
- Auto species detection with full §12 distance metric + flat species list.
- IndexedDB autosave via double-buffered transferable + worker JSON encode.
- Seedable RNG, eulogy card with 4-image hall of fame + Copy Share Text + Download Save.
- Five named dev sliders (§11).

**Deferred to v1.1+:**
- WebGPU NN path.
- Horizontal-time lineage tree visualization (bet for v1.1 per game-design reviewer).
- Trait histograms in Stats overlay.
- NN visualization, follow-cam, offspring lists.
- Seasons / day-night, disasters, biomes, signaling, sexual reproduction.
- NEAT-style topology mutation, evolvable `nn_capacity`.
- Tauri desktop build.

## 15. Implementation plan (for orchestrator)

### Milestone A — Walking skeleton (small)
1. Cargo workspace + wasm-pack scaffold; static HTML page with canvas; pinned deps; CI lint/typecheck.
2. Render one bouncing colored circle from Rust through wasm.
3. Confirm `getrandom("js")` works; confirm single-thread path; document COOP/COEP for `wasm-bindgen-rayon`.

### Milestone B — Sim core (sequential)
4. World struct, Creature struct, **double-buffered SoA layout**, tick loop ordering per §3.5.
5. Sun map (gradient + 3 hotspots) + capacitor refill.
6. Photosynth two-pass + mitosis with energy economy (hardcoded "split when energy > threshold" — no NN yet).
7. Carrion + lifespan + death.
8. Aquarium rendering with these primitives.
9. Game-over detection + eulogy card stub.

### Milestone C — Body genome + mutation (**sequential** — C.10 must precede C.11 must precede C.12, etc.)
10. Full Genome struct + per-trait mutation per §6.
11. Pigment → color rendering + feature dots.
12. Movement + eyes (drive raycasts; feed hardcoded behavior).
13. Eat + scavenge actions with mouth/gut taxes + digestion cooldown.
14. Soft-repulsion physics + wall-clamp velocity handling.

### Milestone D — Brain (sequential after C)
15. Fixed-shape NN struct + CPU SIMD forward pass (`wide`).
16. Wire NN inputs (§5.1) and outputs to action system.
17. NN weight mutation per §5.3 with geometric-skip sampling.
18. Rayon parallelism across creatures (with deterministic chunking).
19. Behavior sanity check at 1–2k creatures, 30fps target on a modern laptop.

### Milestone E — Observability (true parallelism within)
20. Full species detection per §12 (distance + anchors + naming) — sequential prerequisite.
21. Right-rail overlay shell + the three sections (Events, Stats, Inspector) — **parallel-safe** once §20 lands.
22. In-aquarium event toasts + highlight rings.
23. Stats line charts.
24. Inspector popover.
25. Event auto-detection (first-mover, first-eater, extinctions, speciations, population milestones).

### Milestone F — Persistence & polish
26. Double-buffered transferable handoff to worker + IndexedDB JSON autosave (parallel-eligible once Milestone D stabilizes the world schema).
27. Seedable RNG + seed display + "Resume?" prompt.
28. Eulogy card 4-image grid + Copy Share Text + Download Save.
29. Headless acceptance test harness (§16).
30. **Balance tuning pass** (cannot be subagented): live watch, tweak sliders, possibly adjust constants. Goal: §16 DoD passes.

### Where parallelism is real
- E.21–E.24 (the three overlay panels + in-aquarium toasts) — independent UI work.
- F.26 (persistence) can begin once Milestone D world schema is stable.
- Everything else is essentially sequential. **Milestone C is NOT parallelizable.**

### Out-of-scope guardrails
- No WebGPU code, not even stubbed.
- No tabs UI.
- No tree visualization (lineage tree is v1.1).
- No new traits beyond §4.
- No new actions beyond §5.1.
- The orchestrator may not add features it thinks would be cool. **Spec what's in this doc and nothing else.**

## 16. Definition of done for v1

**Headless acceptance test (must pass in CI):**
- Run sim with fixed seed `evosim-test-001` for 10,000 ticks.
- Assert (a) `population > 0` at tick 10,000.
- Assert (b) at least 2 distinct species detected by §12 metric.
- Assert (c) sim completes in **< 8 wall-seconds** on CI (this is the perf gate — orchestrator stops optimizing when this passes).
- Assert (d) world state hash at tick 10,000 matches a golden snapshot recorded once and pinned (regression catch).

**Manual / UI acceptance:**
- Fresh build deploys to Cloudflare Pages (or equivalent). Opening URL shows a single cell on a sun-gradient-with-hotspots world.
- Body mutation produces visible color/size variation across lineages within first 200 ticks.
- Closing and reopening the tab restores the world from autosave without UI hitch.
- Population zero → eulogy card with 4-image grid + Copy Share Text + Download Save + New World button.
- Seed copyable from top bar.
- All five dev sliders modify live sim without restart.
- In-aquarium toast + highlight ring fire when a speciation event occurs.
