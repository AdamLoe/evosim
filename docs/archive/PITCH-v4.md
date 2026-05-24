# evosim — PITCH v4 (canonical spec for v1 build)

> Supersedes v3. v4 incorporates findings from four parallel reviews (game-design, engineering, evo-bio, scope). Subagents working on v1 should treat this as the source of truth.

## What changed from v3

1. **WebGPU cut from v1.** CPU SIMD via `rayon` only. NNs are too small for GPU dispatch to pay off.
2. **Population target dropped 5–10k → 1–2k** for v1. Removes rendering pressure and is plenty for interesting evolution.
3. **No Tabs UI.** Persistent aquarium with collapsible overlays for Stats/Events. Tree tab cut to v1.1.
4. **NN mutation gated by per-weight rate (default 2%), σ=0.02.** v3's "σ=0.05 every weight every birth" was destroying selection signal.
5. **Eat-trait carries standing upkeep** even when unused, to prevent the "predator trapdoor" ecosystem collapse.
6. **Sun is a regenerating capacitor**, not instantaneous occupancy. Creates the second-order dynamics needed for arms races and spatial niches.
7. **Sun capacity varies spatially** (gradient across map) — built-in heterogeneity from day one.
8. **Vision passes full RGB per sector** (was 1-channel summary) — restores kin / mimicry signal.
9. **Separate `scavenge_efficiency` from `eat_efficiency`** so the scavenger niche can differentiate.
10. **`nn_capacity` fixed in v1** (no capacity mutation). Ship at 24 hidden units. Avoids learning-discontinuity hazards.
11. **Seedable RNG.** Optional seed at world creation; "share seed XYZ" is a v1 feature.
12. **Eulogy card on game-over** with stats + best/weirdest creature snapshot.
13. **JSON saves**, not bincode (resilient to schema drift; v1 isn't perf-constrained on saves).
14. **Body mutation rate dropped 5% → 1% default** per trait. With short generations this was producing too many spurious species.

## 1. North star

A browser-deployed idle evolution sandbox. Open a tab, see a single cell. Watch evolution unfold over minutes-to-hours. The map's photosynthetic income is the compute budget; the energy economy self-regulates how much complexity the world can support.

## 2. Stack

- **Language/runtime:** Rust → WebAssembly.
- **Compute parallelism:** `rayon` via `wasm-bindgen-rayon` for CPU threading; single-threaded fallback when COOP/COEP headers are not available. **No WebGPU.**
- **Frontend:** TS/JS shell, HTML5 Canvas 2D rendering.
- **Persistence:** IndexedDB autosave (JSON serialization).
- **Deployment:** prefers a host that can set COOP/COEP headers (Cloudflare Pages, Vercel with config, self-hosted). Document headers; ship the single-threaded fallback for hosts that can't.
- **Dependencies to pin:** `wasm-bindgen`, `wasm-bindgen-rayon`, `getrandom` with `js` feature, `serde` + `serde_json`, `wide` or `std::simd` (nightly) for SIMD.

## 3. Game design

### 3.1 Core loop
1. World begins with one cell at center, minimal genome, random NN weights.
2. Photosynth → energy → split → lineage grows.
3. Body genome and NN weights mutate on each birth (independent processes; see §6).
4. Energy economy + regenerating-sun + lifespan + carrion create selection pressure.
5. Speciation auto-detected; new entries appear in the events overlay.
6. Population zero → game-over eulogy card with stats + a New World button.

### 3.2 Tick rate
- Sim decoupled from render. User slider: pause / 1x / 10x / 100x.
- Render samples world state at 30fps.
- Sim runs only while the tab is open.

### 3.3 World shape
- 2D continuous floats. 1000×1000 units. **Hard walls.**
- **Single spatial-hash grid** at the finest resolution needed (5u cells). Coarser queries (sun, vision range gating) derive from this grid by cell-stride or summed-area, not parallel structures.
- **Sun map** lives on a coarse 50u grid (so 20×20 = 400 cells). Each cell has a `sun_capacity` (max) and `sun_current` (running). Capacity varies by position (default: linear east-west gradient, capacity 1.0 east → 5.0 west, or similar). Current refills toward capacity at rate `R = 0.5/tick`. Photosynthing creatures drain it.

### 3.4 RNG / determinism
Seedable. World-creation panel includes an optional seed input. If unseeded, generate a random seed and display it (so any world is reproducible if its seed is copied). RNG is seeded once and accessed via a single `SimRng` passed through the tick loop.

### 3.5 Within-tick ordering
Each sim tick executes deterministically in this order, on the current snapshot:
1. Rebuild spatial hash grid from current positions.
2. Each creature: cast vision rays into pre-physics positions.
3. Each creature: NN forward pass with (self state, vision inputs).
4. Apply discrete actions (photosynth/eat/split/signal/rest).
5. Apply velocities to positions; apply soft-repulsion forces; clamp to walls.
6. Sun map: drain by photosynth consumption this tick; refill toward capacity.
7. Update creature energy: subtract upkeep, apply income from actions.
8. Resolve deaths → carrion.
9. Decay carrion / remove expired corpses.

## 4. Genome

```rust
struct Genome {
    size: f32,                     // 1.0 .. 10.0
    max_age: u32,                  // ticks
    photosynth_efficiency: f32,    // 0.0 .. 2.0
    eat_efficiency: f32,           // 0.0 (off) .. 2.0
    scavenge_efficiency: f32,      // 0.0 (off) .. 2.0  -- v4 split from eat
    move_speed: f32,               // 0.0 (off) .. 5.0
    eye_count: u8,                 // {0, 2, 3, 4, 6, 8, 12, 24}
    eye_offsets: [f32; 24],        // angle per slot; first eye_count used
    vision_range: f32,             // 0.0 .. 100.0
    armor: f32,                    // 0.0 .. 1.0
    bite_reach: f32,               // 1.0 .. 3.0 × body radius
    pigment_r: f32,                // 0..1
    pigment_g: f32,
    pigment_b: f32,
    mutation_rates: TraitMutationRates,   // per-trait, themselves evolvable
}

struct Brain {
    nn_weights: Vec<f32>,                 // fixed-size (see §5)
    nn_mutation_rate: f32,                // per-weight, evolvable
}
```

`nn_capacity` is REMOVED from the genome in v1. All creatures share one fixed brain shape.

## 5. Brain

### 5.1 Architecture
Fixed for all creatures: **120 inputs → 24 hidden (ReLU) → 8 outputs**. Total ~3,100 weights.

Inputs:
- Self state (10): energy fraction, age fraction, size, last-action one-hot summary, current velocity (2), is-at-wall flag, scavenge-mode hint (zeros for now), carrion-nearby hint (zeros for now), reserved.
- Vision (24 sectors × 5 features each = 120, minus self state = 110): per-sector `(distance, target_size, target_r, target_g, target_b)`. Empty sectors → zeros.
- (Total math: 10 self + 24×5 = 130 actually; v1 build target is approximately this — exact column count is set in code, spec the shape is "self + 24 sectors × 5".)

Outputs (8):
- `velocity_x`, `velocity_y` (continuous, scaled by genome.move_speed)
- 6 discrete-action logits → argmax → `{rest, photosynth, eat, scavenge, split, signal}` (`signal` is no-op in v1)

### 5.2 Inference
CPU SIMD via `wide` or `std::simd`. Forward passes parallelized across creatures with `rayon::par_iter`. Targeted ~1k–2k creatures at 30 ticks/s baseline → ~50–100k forward passes/sec → trivial on modern CPU.

### 5.3 NN weight mutation
On birth:
- Inherit parent weights.
- For each weight independently: with probability `brain.nn_mutation_rate` (default 0.02), add `N(0, 0.02)`.
- `nn_mutation_rate` itself mutates 0.5% per birth, jittered ±20%.

## 6. Body mutation

`TraitMutationRates` carries one f32 per body trait. Defaults at world start: 0.01 (1%). On birth, for each trait, with probability = that rate, jitter the value:
- Continuous: add `N(0, 0.1 × current_value)` clamped to trait bounds.
- Discrete (`eye_count`): pick uniformly from adjacent valid divisors of 24.

Mutation rates themselves mutate at 0.5% per birth, jitter ±20%.

## 7. Energy economy (starter values; expose 3–5 hand-picked sliders in v1)

### Income
| Source | Value |
|---|---|
| Photosynth (per tick, if sun cell has supply) | +0.5 × `photosynth_efficiency` × min(1, sun_current / consumption_demand) |
| Eat hit | deals `1.0 × size` damage, gains `1.5 × eat_efficiency` energy |
| Scavenge (per tick, while overlapping corpse) | +1.5 × `scavenge_efficiency`, drains corpse pool |

### Upkeep (per tick, summed)
| Term | Value |
|---|---|
| Base alive tax | 0.05 |
| Size beyond 1.0 | +0.04 per unit |
| Mobility flag (any nonzero move_speed) | +0.10 |
| Move speed (top) | +0.02 per unit |
| Per active eye | +0.02 |
| Vision range² | +0.001 × range² |
| **Mouth tax** (eat_efficiency > 0) | **+0.05 standing** (NEW in v4 — prevents predator trapdoor) |
| **Gut tax** (scavenge_efficiency > 0) | +0.02 standing |
| Armor | +0.03 per unit |
| Lifespan term | +0.01 per 1000 ticks of max_age |
| NN upkeep (fixed) | 0.05 (one constant for all creatures; ship as constant in v1) |

### One-time costs
| Action | Cost |
|---|---|
| `eat` attempt | 0.3 |
| `scavenge` attempt | 0.1 |
| `split` | 50 + child-gift |
| Movement | 0.02 per unit distance |
| `rest` / `photosynth` | 0 |

### Past-lifespan penalty
Past `max_age`: upkeep multiplied by 4 per 1000-tick excess.

## 8. Death and carrion

- Energy ≤ 0 → death.
- Body persists as **carrion** for 100 ticks at death position. Pool = 0.3 × cumulative_upkeep paid in lifetime (cap at 30).
- Scavengers consume from pool while overlapping. Drained corpses or expired corpses vanish; remaining pool returns to local sun cell.

## 9. Physics

Soft repulsion (boids-style): force ∝ overlap depth, applied each tick after action resolution. No hard collision. Movement is `position += velocity` per tick; velocity is clipped to `move_speed`. Walls clamp position; velocity into a wall is zeroed (no bounce).

## 10. Sensing

24 fixed angular sector slots. Genome `eye_count` ∈ {0, 2, 3, 4, 6, 8, 12, 24} (divisors of 24). Active eyes tile the 24 slots evenly starting at `eye_offsets[0]`. Each active sector raycasts one ray of length `vision_range` against the spatial grid; reports (distance, target_size, target_r, target_g, target_b) of the first creature or corpse hit. Inactive sectors return zeros.

## 11. UI (no tabs)

### Persistent aquarium
The world canvas is **always on screen**. Fills the viewport.

### Top bar
Day counter, population, species count, speed buttons (pause / 1x / 10x / 100x), sun-intensity slider (multiplies all sun-cell capacities), settings gear.

### Collapsible right-rail overlay
A single rail on the right edge with sections that expand on click; all overlay sections render *over* the aquarium without hiding it.
- **Events** (default open): scrolling text of speciation, extinctions, milestones. Also includes a flat "Species" list (active species with founded-day + parent link). Tree visualization deferred to v1.1.
- **Stats**: simple line charts of population and species count over time. Hidden by default.
- **Inspector**: opens when a creature is clicked. Shows genome stats and species/parent breadcrumb.

### Dev panel
Behind a hotkey (e.g. `~`). Exposes 3–5 hand-picked tunable sliders: base sun rate, mutation-rate multiplier, sun gradient strength, mouth-tax, NN-mutation σ. All other costs are constants in v1.

### Eulogy card (game-over)
When population reaches zero:
- Dim the aquarium, freeze the final frame.
- Show a card: "Your world lived **N days**, peaked at **P creatures** across **S species**. The strangest creature: [thumbnail with traits]. Click to download save (JSON) or **New World**."
- "New World" button starts fresh; "Save world" downloads the final state.

## 12. Species detection

For each new birth, compute genome-distance to its species' anchor genome. If > threshold (default tuned during balancing), declare new species. Generate species name (procedural like "Lineage A1a"), record parent species link, log a "speciation" event. Species track member count over time for the species list.

Threshold should be expressed as a multiple of expected per-generation drift (≈ √(num_traits) × σ_per_trait), not an absolute constant.

## 13. Persistence

- Autosave the world to IndexedDB every 10 sim-seconds (less frequent than v3 to limit hitch risk).
- Serialize world state with `serde_json` on a worker thread against a snapshot of the world buffer.
- On reopen: prompt "Resume world from day X (seed: …)?" or "New World".
- "Save world" button on eulogy card downloads JSON.

## 14. MVP scope (v1)

Included:
- Rust+wasm sim, single-threaded fallback + multithread when COOP/COEP allow.
- CPU SIMD NN inference parallelized over creatures.
- 1–2k creature target population, 1000×1000 hard-walled world.
- Full genome from §4, body + NN mutation processes per §5–6.
- Regenerating sun capacitor with east-west gradient.
- Soft repulsion physics.
- Carrion + scavenging + lifespan + game-over.
- Persistent aquarium UI with right-rail Events/Stats/Inspector overlays.
- Auto species detection + event log + flat species list.
- IndexedDB autosave + seedable RNG + eulogy card.
- 3–5 dev sliders.

Deferred to v1.1+:
- WebGPU NN path (bigger populations).
- Horizontal-time lineage tree visualization.
- Trait histograms in Stats overlay.
- NN visualization, follow-cam, offspring lists.
- Seasons / day-night, disasters, biomes, signaling, sexual reproduction.
- NEAT-style topology mutation, evolvable `nn_capacity`.
- Tauri desktop build.

## 15. Implementation plan (for orchestrator)

### Milestone A — Walking skeleton (small)
1. Cargo workspace + wasm-pack scaffold; static HTML page with a canvas; pinned deps (§2); CI lint/typecheck.
2. Render one bouncing colored circle from Rust through wasm to canvas.
3. Confirm `getrandom("js")` works; confirm single-threaded path works; document COOP/COEP for `wasm-bindgen-rayon`.

### Milestone B — Sim core (sequential)
4. World struct, Creature struct, tick loop ordering per §3.5.
5. Sun map with regenerating capacitor + east-west gradient.
6. Photosynth + mitosis with energy economy (NN-less: hardcoded "split when energy > threshold").
7. Carrion, lifespan, death handling.
8. Aquarium rendering complete with these primitives.
9. Game-over detection + eulogy card stub.

### Milestone C — Body genome + mutation (parallel possible after B)
10. Full Genome struct + per-trait mutation on birth.
11. Pigment → color rendering; feature dots for active traits.
12. Movement + eyes (drive raycasts but feed into hardcoded behavior; no NN yet).
13. Eat + scavenge actions with mouth/gut taxes.
14. Soft-repulsion physics.

### Milestone D — Brain (sequential after C)
15. Fixed-shape NN struct + CPU SIMD forward pass.
16. Wire NN inputs (§5.1) and outputs to action system.
17. NN weight mutation per §5.3.
18. Rayon parallelism across creatures.
19. Behavior sanity check at 1–2k creatures, 30fps target.

### Milestone E — Observability (some parallelism)
20. Species detection + species anchors + species list.
21. Right-rail overlay shell (Events / Stats / Inspector — can be subagented in parallel).
22. Stats line charts.
23. Inspector popover.
24. Event auto-detection (first-eater, extinctions, speciation, milestones).

### Milestone F — Persistence & polish
25. IndexedDB autosave + JSON serialization on worker.
26. Seeded RNG + seed display + "Resume?" prompt.
27. Dev panel with the 3–5 sliders.
28. Eulogy card content (final-frame freeze, weird-creature thumbnail, save download).
29. End-to-end watch session; spot-tune defaults; verify §16 DoD.

### Where parallelism is real
- Within Milestone E, the three right-rail sections (Events, Stats, Inspector) are mostly independent UI work.
- Persistence (F.25–F.26) can begin once Milestone D stabilizes the world schema.
- Everything else is essentially serial.

### Out-of-scope guardrails
- No WebGPU code in v1, not even stubbed.
- No tabs UI.
- No tree visualization.
- No new traits beyond what's in §4.
- The orchestrator should not add features it thinks would be cool; only what's in this doc.

## 16. Definition of done for v1

- Fresh build deploys; user opens URL, sees a single cell on a sun-gradient world.
- Within 10–15 min of watching at 100x speed (without manual intervention), a second species is detected and logged in the Events overlay.
- Aquarium renders at 30fps with 1k+ live creatures on a modern laptop.
- Body mutation produces visible color/size variation across lineages within the first generation.
- Closing & reopening the tab restores the world from autosave.
- When population reaches zero, eulogy card appears with stats + New World button.
- Seed is visible in UI and copy-pasteable.
- All five dev-panel sliders modify the live sim without restart.
- **Acceptance test for orchestrator**: a unit test that runs the sim headlessly with a fixed seed for N=10,000 ticks and asserts (a) population stays > 0, (b) at least one trait mutation occurred, (c) the species detector fires on a hand-injected divergent genome.
