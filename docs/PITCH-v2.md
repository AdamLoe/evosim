# evosim — PITCH v2

**Genre:** idle evolution sandbox / spectator sim
**Stack:** Rust → WebAssembly + WebGPU + Web Workers (browser-deployed)
**One-liner:** You open a tab. A single green dot is dividing. Three minutes later 2,000 dots cover the world. Twenty minutes later, one of them is red and is eating the green ones. You didn't design any of that.

## The foundational insight

**The map's photosynthetic income IS the compute budget.** The total sunlight entering the world is fixed. Every trait — bigger body, more eyes, bigger brain, longer life — costs energy to maintain. So expensive species can only exist if there's enough sun to support them. A world that runs out of compute is a world that's evolved too many predators. **Balance is physics, not a knob.**

## Stack rationale

- **Rust** for the core sim: ECS-ish, spatial hashing, tight loops, `rayon` for CPU parallelism.
- **WebAssembly** for deployment: compile once, host on any static CDN, anyone can open a URL — no install. Within ~80–90% of native CPU speed.
- **WebGPU** for batched NN inference: tens of thousands of small NNs evaluated as one batched matmul per tick.
- **Web Workers** for off-main-thread sim/render split.
- **OPFS / IndexedDB** for world saves.
- **Optional later:** Tauri build reusing the same Rust core, for users who want 100% native speed and background ticking.

**Trade-off accepted:** ~70% of native efficiency in exchange for zero-friction sharing.

## Core mechanic — the trait economy

Every creature carries a **genome** (mutable physical traits) and a **brain** (NN weights, mutated separately).

Each tick, every creature pays an upkeep equal to the sum of its trait costs. Income comes from photosynthesis, predation, scavenging. Net positive → store energy → eventually reproduce. Net negative → starve.

Mutable genome traits include:
- `size` — bigger means more HP and damage, costs more upkeep
- `move_speed` — top velocity, costs energy per actual movement
- `eye_count` — one of {0, 2, 3, 4, 6, 8, 12, 24}; divisor of 24, see sensing below
- `eye_directions` — per-eye angle relative to body heading
- `vision_range` — how far rays reach, costs energy proportional to range²
- `armor` — damage reduction, costs upkeep
- `diet_efficiency` — photosynth bonus, herbivory bonus, carnivory bonus (separate axes; can be omnivore)
- `bite_reach` — how close prey must be to be bitten; future "ranged" attacks deferred
- `max_age` — lifespan; sharp energy decay past this age
- `nn_capacity` — fraction of the max NN that's "active" (see brain)
- `kin_signature` — slow-mutating gene tuple that maps onto visible appearance (color/pattern)
- `mutation_rate` — meta-evolvable; some lineages mutate fast, some stable

**Day-0 starter genome:** `size=min, photosynth_on, everything_else=off, max_age=long`. Everything else has to evolve in.

## Anti-soft-lock: shadow + lifespan + sun knob

The "first cell conquers the planet" problem is solved by three interlocking mechanics:

- **Shadow mechanic:** each grid cell of the world has a fixed sun budget per tick. Multiple photosynths sharing a cell *split that budget*. Crowding crashes photosynthesis output → starvation → spatial dispersion pressure → movement becomes valuable.
- **Lifespan trait:** every creature has a `max_age`. Past that age, upkeep spikes hard. Forces turnover, recycles space and energy. Mutable: fast-living vs long-lived strategies can both evolve.
- **Global sun intensity:** a world-level slider (user-controlled, or driven by future seasons/events). Crank it to fuel a Cambrian explosion; dim it to trigger mass extinctions.

The first eater doesn't need to outcompete a static photosynth empire — it needs to mutate, then the lifespan + shadow mechanics constantly churn the photosynth population so there's always fresh meat and open space.

## Sensing — fixed slots, variable eyes

24 fixed angular sectors around each creature. The NN always has 24 sector slots as input (each carrying distance, target-size, and target's RGB-from-genome). The **genome** chooses how many physical eyes the creature has, from `{0, 2, 3, 4, 6, 8, 12, 24}` — divisors of 24. Eyes evenly partition the 24 slots. A 4-eyed creature lights 4 of 24 slots; an empty slot returns zero. Eye direction is genome-controlled (default forward-clustered, but mutation can shift focus).

This gives:
- Stable NN topology across mutations (so weight evolution works)
- Variable eye count and eye placement as evolvable traits
- Mimicry as a natural emergent strategy (see kin)
- Self-limiting compute via the energy economy

## Brain — fixed shape, masked capacity, batched on GPU

Every creature carries the same maximum-size NN (e.g. 64 hidden units, ~5k weights). The genome's `nn_capacity` trait specifies what fraction of those neurons are "active" — the rest are masked to zero on each forward pass. Energy upkeep scales with active capacity.

This means:
- All N creatures' NNs can be evaluated as **one batched GPU matmul per tick** (regardless of which fraction of each is active).
- Bigger brains genuinely encode more behavior but cost more.
- A small-brained drifting photosynth runs through the same dispatch as a strategic predator — they just pay different energy.

Inputs (fixed shape): self energy/age/size/velocity, 24 vision sectors × features, 4 smell channels (later), nearby signal levels, last action.
Outputs (fixed shape): `(vx, vy)` velocity continuous, plus a one-hot discrete action `{rest, photosynth, eat, split, signal}`.

## Action model — continuous velocity + 1 discrete action/tick

NN outputs `(vx, vy)` continuously (capped by `move_speed` genome), AND picks one discrete action per tick. Move-while-eating works naturally. No combinatorial action-cost bookkeeping.

## Reproduction

Asexual mitosis only in v1. Once a creature has enough stored energy, the `split` action spawns one offspring whose genome is the parent's with per-trait mutation (rate is itself an evolvable gene). NN weights inherit with small Gaussian noise. Parent loses the energy spent.

Sexual reproduction deferred — possibly evolved as a higher-tier trait in a future version.

## Kin & mimicry via genome projection

Appearance (circle color, feature dots) is a deterministic projection of a slow-mutating slice of the genome. Two siblings look almost identical; cousins similar; strangers different. **Kin recognition just falls out of vision** — the NN can learn "similar color = don't eat" if that's adaptive. No special kin sensor needed.

Mimicry emerges naturally: a weak creature whose appearance-genome drifts toward a predator's appearance gets attacked less, even though its NN behavior is its own. Body genome ≠ brain weights.

## World

- **2D, hard walls.** No torus in v1. Corners become real terrain features.
- **5–10k creatures** target population.
- **Pure spectator** in v1. God-buttons (disasters, sun-tweaks) in v1.1.
- **Variable speed + decoupled internals.** Sim ticks as fast as targeted; render samples at 30fps. User slider: pause / 1x / 10x / 100x.

## Day-1 starter conditions

**One cell.** Center of map. Minimal genome (size, photosynth, long lifespan, no eyes/movement/eat/brain capacity). Photosynths until it can split. Splits, splits, splits. Spreads across the map. Shadow starts killing batches. Movement mutates in. Or eating mutates in. Or lifespan-shortening mutates in. The Cambrian moment is *watchable* because it starts from one dot.

## History

- **Lineage tree, always on.** Every birth has a parent pointer. Side-panel visualization grows in real time. Click any node → highlight that subtree in the world.
- **Species detection (engine-side):** cheap genome-distance threshold. When a lineage drifts > X edits from its species' ancestor, declare a new species, assign a new tree branch color.
- **Event log (auto-generated):** "Day 14: first eater emerged from lineage A1." "Day 47: photosynths went extinct." Detected from stats deltas.
- **Stats time-series:** population, avg traits per species, sun budget consumed. Scrubbable graphs.

## MVP scope (v1 build target)

- Rust+wasm sim with WebGPU NN inference
- One-cell start, 2D hard-walled world, ~5k creatures sustained
- Genome: size, photosynth, eat, move_speed, eye_count, eye_directions, vision_range, max_age, nn_capacity, kin_signature, mutation_rate
- Fixed-shape NN with masked capacity, batched on GPU
- Continuous velocity + 1 discrete action per tick
- Shadow mechanic + global sun slider
- Live aquarium with pan/zoom + click-to-inspect
- Always-on lineage tree + auto species detection + event log
- IndexedDB autosave

**Deferred to v1.1+:** seasons, biomes, god-button disasters, sexual reproduction, ranged attacks, NEAT-style topology evolution, smell channels, signaling, Tauri desktop build.

## Open design questions (next planning round)

- Exact energy-cost table for traits (Claude proposes a starter)
- Lineage tree visualization (radial? horizontal time? collapsible?)
- Save format (JSON? bincode? versioning strategy?)
- Spatial hashing grid resolution
- Mutation rate ranges (initial values + per-trait variance)
- Signal/communication channels (skip for v1?)
- Visual identity: what feature-dots look like, color projection function from genome
