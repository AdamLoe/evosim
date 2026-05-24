# evosim — PITCH v3 (canonical spec for v1 build)

> This is the authoritative design doc. Subagents working on v1 implementation should treat this as the source of truth and defer to it over earlier pitches or conversation history. PITCH-v1.md and PITCH-v2.md are kept for evolution history only.

## 1. North star

A browser-deployed idle evolution sandbox. Open a tab, see a single cell. Watch evolution unfold over minutes-to-hours. No player agency in v1 except world speed, sun intensity, and a "new game" button. Sharing the URL works; sims do not persist across sessions in v1 beyond IndexedDB autosave.

**Foundational design rule:** *The map's photosynthetic income is the compute budget.* Total sunlight per tick is fixed. Every trait/brain capacity costs upkeep. The energy economy self-regulates how much complexity the world can support.

## 2. Stack

- **Language/runtime:** Rust → WebAssembly, deployed to any static host.
- **Compute parallelism:** `rayon` for CPU parallel (where wasm threads available via SharedArrayBuffer + COOP/COEP); `wgpu` (WebGPU) for batched NN inference.
- **Frontend:** Plain TS/JS shell with HTML5 Canvas 2D rendering. Tabs UI (Aquarium / Tree / Stats / Events).
- **Persistence:** IndexedDB autosave (JSON serialization of world state). No server.
- **No PyTorch, no Python, no backend.** Future Tauri build may reuse the same Rust core.

## 3. Game design

### 3.1 Core loop
1. World begins with **one cell** at world center, minimal genome, random NN weights.
2. Cell photosynthesizes, eventually splits. Lineage grows.
3. Mutation introduces traits (eyes, movement, eating, …) and adjusts NN weights.
4. Energy economy + shadow + lifespan create selection pressure.
5. Speciation auto-detected; new branches appear on lineage tree.
6. Player watches, optionally adjusts speed and sun intensity.
7. If population reaches zero → **end-game state with "New World" button**. No mid-game auto-respawn.

### 3.2 Tick rate
- Sim ticks decoupled from render. User slider: pause / 1x / 10x / 100x.
- Render samples world state at 30fps regardless of sim rate.
- Sim runs only while tab is open (browser-only v1).

### 3.3 World shape
- 2D continuous floats. Default size ~1000×1000 units.
- **Hard walls** (no wrap). Creatures bounce or stop at edges.
- Multiple spatial-hash grids of different cell sizes:
  - Tight grid (~5u) for collision/soft-repulsion
  - Medium grid (~50u) for vision raycasts
  - Coarse grid (~20u) for sun-budget shadow accounting
  - Reserved sizes for future smell/signal layers

### 3.4 RNG / determinism
Fully non-deterministic. Standard `thread_rng()` everywhere. No seeding in v1.

## 4. Genome

Plain Rust struct, field-by-field mutation. All fields except `nn_weights` belong to the body genome.

```rust
struct Genome {
    // Body traits (each has its own mutation_rate gene; see section 6)
    size: f32,                  // 1.0 .. 10.0
    max_age: u32,               // ticks; sharp upkeep spike past this
    photosynth_efficiency: f32, // 0.0 (off) .. 1.0+
    eat_efficiency: f32,        // 0.0 (off) .. 1.0+
    move_speed: f32,            // 0.0 (off) .. 5.0 (units/tick)
    eye_count: u8,              // one of {0, 2, 3, 4, 6, 8, 12, 24}
    eye_offsets: [f32; 24],     // angle per slot; only first `eye_count` used
    vision_range: f32,          // 0.0 .. 100.0
    armor: f32,                 // 0.0 .. 1.0 (damage reduction)
    nn_capacity: f32,           // 0.0 .. 1.0 (fraction of max neurons active)
    bite_reach: f32,            // 1.0 .. 3.0 multipliers of body radius
    mutation_rates: TraitMutationRates,  // per-trait rates, themselves evolvable
    pigment_r: f32,             // 0..1, drives visible color
    pigment_g: f32,
    pigment_b: f32,
}

struct Brain {
    nn_weights: Vec<f32>,   // fixed max size, masked by genome.nn_capacity
}
```

### 4.1 Pigment → appearance
Visible color = `(pigment_r, pigment_g, pigment_b)` directly. Pigments mutate slowly so siblings look similar. Mimicry emerges if a body genome drifts toward another species' pigments.

### 4.2 Day-0 starter cell
- `size = 1.0`, `max_age = 5000`, `photosynth_efficiency = 1.0`, all others zero/off.
- `nn_capacity = 0.1` (tiny brain to start).
- `nn_weights` randomly initialized.
- Only viable actions for a starter genome: `photosynth` and `split`. Random NN will fire one of those quickly.

## 5. Brain

### 5.1 Architecture
Fixed maximum topology, e.g. **80 inputs → 32 hidden (ReLU) → 8 outputs**. Same shape for every creature.

`nn_capacity ∈ [0, 1]` masks active neurons. With capacity 0.5, only 16 of 32 hidden neurons participate; rest masked to zero. Active-neuron count drives per-tick upkeep (see section 7).

Implementation detail: all creatures' NNs are stored as one big tensor in WebGPU buffer; one batched compute shader does the forward pass per sim tick. Inactive neurons are zero'd via a mask uniform per creature.

### 5.2 Inputs (80 total)
- Self state (8): energy fraction, age fraction (age/max_age), size, last action one-hot summary, current velocity (x, y), is-at-wall
- 24 sectors × 3 features (72): nearest object distance (or 0 if empty), object size, average pigment R/G/B compressed to 1 channel — total per sector: dist, size, color_summary. (Final field count tunable during impl; spec is "24 × ~3".)
- Reserved slots for v1.1 smell channels and signal channels (zero-filled in v1)

### 5.3 Outputs (8 total)
- `velocity_x` (continuous, scaled by genome.move_speed)
- `velocity_y` (continuous, scaled by genome.move_speed)
- 6 discrete-action logits → argmax → one of: `rest`, `photosynth`, `eat`, `split`, `signal` (no-op in v1), `block` (no-op in v1)

### 5.4 NN weight mutation
Independent from body genome. Each child gets parent's weights + Gaussian noise σ=0.05 on every weight. No structural mutation in v1.

## 6. Mutation

Every trait carries its own mutation rate, all collected in `TraitMutationRates`. Defaults at world start: 5% chance per birth per trait.

On birth:
- For each trait, with probability = its rate, jitter the value by `N(0, 0.1 × current_value)` (continuous) or pick adjacent valid value (discrete like `eye_count`).
- Mutation rates themselves mutate at 0.5% per birth, jitter ±20%.
- All NN weights mutate every birth (Gaussian σ=0.05 on every weight, no rate gate).

## 7. Energy economy (starter values; expose as live sliders)

### Income
| Source | Value |
|---|---|
| Photosynth (per tick, full sun, uncrowded) | +0.5 × `photosynth_efficiency` |
| Sun budget per coarse-grid cell | 5.0/tick (split among photosynths there) |
| Eat hit | deals 2.0 damage, gains 1.5 × `eat_efficiency` energy |
| Scavenge corpse | gains 1.5 energy per tick of eating, no damage roll |

### Upkeep (per tick, summed)
| Term | Value |
|---|---|
| Base alive tax | 0.05 |
| Size beyond 1.0 | +0.04 per unit |
| Mobility flag (any nonzero move_speed) | +0.10 |
| Move speed (top) | +0.02 per unit |
| Per active eye | +0.02 |
| Vision range² | +0.001 × range² |
| Per active NN neuron | +0.0015 |
| Armor | +0.03 per unit |
| Lifespan (max_age) | +0.01 per 1000 ticks |

### One-time costs
| Action | Cost |
|---|---|
| `eat` attempt | 0.3 |
| `split` | 50 + child-gift (capped at parent's current) |
| `move` (per unit distance actually moved) | 0.02 |
| `rest` / `photosynth` | 0 |

### Past-lifespan penalty
Past `max_age` ticks, upkeep is multiplied by 4 per 1000-tick excess. Old creatures starve hard.

## 8. Death, carrion, decay

- Energy ≤ 0 → death.
- Body persists as **carrion** for 50 ticks at the death position, fixed energy pool = creature's body cost in lifetime accumulated × 0.3 (cap).
- Any creature with eat_efficiency > 0 can scavenge: choose `eat` while overlapping the corpse → gain 1.5 energy/tick, no damage roll, drains the pool.
- After 50 ticks (or pool drained), the corpse vanishes; remaining pool returns to global sun budget.

## 9. Physics

- **Soft repulsion** (boids-style): when bodies overlap, apply a separating force inversely proportional to overlap. No hard collision.
- Movement: NN outputs (vx, vy), clamped to genome.move_speed magnitude. Position += velocity per tick.
- Wall behavior: bounce with zero restitution (stop at wall).

## 10. Sensing details

24 fixed angular sector slots. Genome's `eye_count` selects `{0, 2, 3, 4, 6, 8, 12, 24}` (divisors of 24). Eyes evenly tile the 24 slots starting at `eye_offsets[0]`. Empty slots return zero.

Per active sector: cast a single ray of length `vision_range`. Report distance to first hit (creature or corpse), target's size, target's pigment summary. Use the vision-range grid for the raycast hit-test.

## 11. UI

### Tabs
- **Aquarium** (default): canvas of the world. Pan (drag) / zoom (scroll). Click a creature → inspector popover. Bottom bar has speed buttons (pause / 1x / 10x / 100x), sun-intensity slider, day counter, population counter.
- **Tree**: horizontal-time lineage tree, root at left, present at right. Species-level nodes only (drilling into individual creatures deferred). Click a node → highlight that species in aquarium.
- **Stats**: two line charts — total population and species count over time.
- **Events**: scrolling text log of auto-detected events ("Day 14: new species emerged from lineage A1", "Day 47: photosynths extinct"). Newest at top.

### Inspector popover
- Genome stats: all numeric values + active traits.
- Lineage breadcrumb: species name, parent species, founded on day X.
- (NN viz, follow-cam, offspring list — all deferred to v1.1.)

### Visuals
- Each creature = circle of radius proportional to `size`.
- Fill color = `(pigment_r, pigment_g, pigment_b)`.
- Border / feature dots on rim indicate active traits: small dots for each eye, mouth-dot if `eat_efficiency > 0`, motion-dot if `move_speed > 0`. Implementation may simplify these to a single ring with subtle ticks.
- Carrion: gray circle.

### Game-over
When `population == 0`: dim the canvas, show "All life has ended on day X" with a **New World** button. Button → fresh world with new starter cell.

## 12. Species detection

Compute genome-distance between each creature and its species' anchor genome (recorded at speciation). If distance > threshold, declare new species: assign new color hue tint (overlay on pigment) and a generated name (e.g. "Lineage A1a"); record new tree node with parent species edge.

Run detection on each new birth (cheap: one distance check vs parent's species).

## 13. Persistence

- Autosave entire world to IndexedDB every ~5 sim-seconds.
- On page reopen: prompt "Resume saved world from day X?" / "New World".
- World save = bincode-or-JSON of: world params, RNG state (none in v1 since non-det), creatures vec, lineage tree, event log.
- v1 has one save slot per browser. Multi-slot deferred.

## 14. MVP scope (v1 build target)

Included:
- Rust+wasm sim, soft-repulsion physics, hard-wall 1000×1000 world
- Multi-grid spatial hashing (collision/vision/sun)
- ~5–10k creature target population
- Full genome from section 4
- WebGPU batched NN inference (fallback to CPU if WebGPU absent)
- Energy economy + shadow mechanic + carrion + lifespan + game-over
- Tabs UI: Aquarium / Tree / Stats / Events
- Live sliders for sun intensity and tick speed; expose all cost-table values in a dev panel
- IndexedDB autosave + game-over flow
- Auto species detection + event log

Deferred to v1.1+:
- Seasons / day-night cycles (highest priority for v1.1)
- Disasters & god-buttons
- Biomes / terrain features
- Signaling + smell channels
- Sexual reproduction
- NEAT-style topology mutation
- NN visualization, follow-cam, offspring lists
- Tauri desktop build
- Seedable determinism
- Multi-slot saves / world export

## 15. Implementation plan (for orchestrator)

The build is structured as **modules**, with sequenced milestones. Subagents implement modules; the orchestrator decides per-module review depth and which modules can run in parallel.

### Milestone A — Walking skeleton
1. Cargo workspace + wasm-pack scaffold; static HTML page; CI lints.
2. Render a single colored circle that bounces around a canvas (no genome, no NN, no sim).
3. Verify Rust↔JS↔Canvas pipeline works end-to-end.

### Milestone B — Sim core
4. World data structure, creature struct, tick loop.
5. Photosynth + mitosis with the energy economy. No NN yet — hardcoded "split when energy > threshold".
6. Shadow mechanic via coarse grid.
7. Death + lifespan + carrion.
8. Render: aquarium tab fully working with these primitives.

### Milestone C — Mutation + body genome
9. Full genome struct + per-trait mutation on birth.
10. Pigment-driven color rendering.
11. Movement, eyes (genome-only, no NN consumption yet), eat action, armor.
12. Soft-repulsion physics.

### Milestone D — Brain
13. NN struct with masked capacity.
14. CPU forward pass first; verify behavior.
15. Wire NN inputs (24 sectors, self state) and outputs (velocity + action).
16. WebGPU batched forward pass; performance test 5k creatures.
17. NN weight mutation on birth.

### Milestone E — Observability
18. Species detection + species-anchor tracking.
19. Lineage tree data structure.
20. Tabs UI shell: Aquarium / Tree / Stats / Events.
21. Tree tab horizontal-time rendering.
22. Stats tab population + species count charts.
23. Events tab auto-event detection (first-eater, extinction events, speciation notifications).
24. Inspector popover.

### Milestone F — Persistence & polish
25. IndexedDB autosave.
26. Game-over flow + New World button.
27. Dev panel with live sliders for cost table.
28. End-to-end balance check; tune defaults.

### Modules eligible for parallel work
- Milestone E sub-tasks (tree, stats, events tabs) can run in parallel once species detection lands.
- Inspector + tree rendering can be parallel.
- Persistence (M F.25) can start once sim core (M B) is stable, in parallel with M D/E.

### Out-of-scope for the v1 orchestration pass
Anything in section 14's "Deferred" list. The orchestrator may stub interfaces but should not implement these.

## 16. Definition of done for v1

- Fresh build deploys to a static host; user opens URL, sees a single cell.
- Within 10–15 min at 100x speed, evidence of mutation: at least one new species detected and labeled on the tree.
- Aquarium runs at 30fps with ~5k live creatures on a modern laptop.
- Closing & reopening the tab restores the world from autosave.
- When population hits zero, game-over screen with New World button appears.
- All cost-table values are tunable via the dev panel.
