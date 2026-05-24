# Architecture

## System shape

The single Rust crate at the repo root (`Cargo.toml`) compiles to both a `cdylib` (wasm) and an `rlib` (native, used by tests). `wasm-pack` bundles the cdylib into `web/wasm/` (gitignored). The web shell lives at `/web` — a Vite + TypeScript project that imports the wasm module at runtime, boots a `WorldHandle`, and drives it via a `requestAnimationFrame` loop. There is one `<canvas>` element; no framework; no second wasm bundle.

## Rust modules (`src/`)

**constants.rs** — Every magic number used in the simulation, annotated with the v5/v6 section that defines it. All tunable values live here; nothing is hardcoded elsewhere. See [../src/constants.rs](../src/constants.rs).

**rng.rs** — `SimRng` wraps `xoshiro256++` (seeded once per world via xxHash64 of the seed string). All random draws in the sim go through the single `SimRng` threaded through tick functions, which is what makes runs deterministic given the same seed. The parallel rayon path uses fixed-N sub-RNGs derived from the same seed but is gated behind the `threads` feature flag (default off) — enabling it requires re-bootstrapping the golden snapshot.

**sun.rs** — `SunMap`: 20x20 grid of energy cells (capacity + current). Capacity is set by an east-west gradient plus 3 Gaussian hotspots placed at world init. Refills at `SUN_REFILL_RATE` per tick (slider-adjustable). Carries a per-tick `demand` accumulator zeroed before each photosynth two-pass.

**grid.rs** — `SpatialGrid`: 120x120 spatial hash over the 600-unit world (5-unit cells). Rebuilt at tick step 1. Used by the vision raycast pass and the soft-repulsion movement step.

**genome.rs** — `Genome` struct (all heritable traits: size, max_age, photosynth/eat/scavenge efficiency, move_speed, eye_count, eye_offsets, vision_range, armor, bite_reach, pigment_rgb, lifespan, mobility_flag, gut_flag). Also holds `TraitMutationRates` (per-trait mutation rates that themselves mutate). Bounds enforcement is here.

**brain.rs** — `Brain`: fixed-shape NN (136 inputs -> 24 hidden ReLU -> 8 outputs). Weights are a flat `Vec<f32>` (row-major, hidden-unit-contiguous). Forward pass uses SIMD (`wide::f32x8`) over 17 input chunks. Geometric-skip mutation per v6 §E. `Brain::founder()` hardwires a minimal energy-sensor into hidden unit 0 so founders survive long enough to split (see `NN_FOUNDER_*` constants and [../BUILD-REPORT.md](../BUILD-REPORT.md) Known Issue #1).

**creature.rs** — `CreatureSoA`: Structure of Arrays for the live creature population. Hot scalar fields (`x`, `y`, `vx`, `vy`, `energy`, `age`, etc.) live in parallel `Vec<f32>`/`Vec<u32>` arrays for cache efficiency. `Genome` and `Brain` are stored as `Vec<Genome>`/`Vec<Brain>` (AoS within SoA — acceptable because they are only touched on split/mutation, not in inner loops). `Action` enum (Rest/Photosynth/Eat/Scavenge/Move/Split) is defined here.

**carrion.rs** — `Carrion`: a dead creature's energy pool. Lives up to `CARRION_MAX_AGE` ticks or until fully drained by scavengers.

**vision.rs** — Vision pass (tick step 2). 24 sector slots, 5 features each (dist, size, r, g, b) = 120-float buffer per creature. Active sectors determined by `eye_count`; inactive slots write zeros (dense layout for NN). Carrion appears as fixed gray (0.4, 0.4, 0.4). Raycasts use the spatial grid.

**species.rs** — `SpeciesRegistry` + `Species`. Each species carries an anchor genome and anchor brain weights; new species are detected when a creature's distance from its current species anchor exceeds the threshold (v5 §12). Naming follows v6 §H (alternating letter/number per generation). `parent_id` is stored for the lineage tree (not yet visualized in v1).

**world.rs** — `World`: owns everything (SoA, sun, grid, carrion, species, RNG, sliders, event log, HoF snapshots). Contains the tick function `step()` and all 12 per-tick sub-functions. Also owns `DevSliders` (the five adjustable parameters). This is the largest file; see Tick ordering below.

**save.rs** — `SaveV1` schema (serde JSON). `From<&World>` and the reverse conversion. Schema version is `SCHEMA_VERSION = 1`. Camera/UI state is not included — that stays on the JS side.

**snapshot_hash.rs** — xxHash64 over canonical world state per v6 §M (tick, creature SoA, sun, carrion, species, RNG state). Used by the §16 acceptance test.

**wasm_api.rs** — `WorldHandle`: the wasm-bindgen boundary. Wraps `World` and exposes only what JS needs. See Wasm API surface below.

**events.rs** — `EventLog`: ring buffer (last 200 entries) plus full history for save. `EventKind` variants: Speciation, Death, PopulationMilestone, FirstToMove, FirstToEat.

**hof.rs** — `HallOfFame`: snapshot of a notable creature (biggest, weirdest, last_survivor, first_mover) captured during the run for the eulogy card.

## Tick ordering (v5 §3.5)

Each call to `World::step()` runs these steps in order:

```
1.  grid.rebuild()                  — spatial hash from start-of-tick positions
2.  run_vision_pass()               — raycasts; fills per-creature VisionBuf (120 floats)
3.  nn_forward_all_chunks()         — SIMD forward pass; decodes velocity + action
4.  apply_movement_and_repulsion()  — update positions; soft repulsion; wall clamp; rebuild grid
5.  photosynth_two_pass()           — demand accumulation then payout
6.  eat_and_scavenge()              — bite resolution; carrion drain
7.  sun.refill()                    — add R * capacity to each sun cell
8.  energy_bookkeeping()            — subtract upkeep; apply age penalty
9.  collect_deaths()                — mark dead; emit death events; update HoF
10. decay_carrion()                 — age carrion; drain expired pools back to sun
11. handle_births()                 — split resolution; spawn offspring; speciation check
12. (species detection)             — wired into handle_births; SpeciesRegistry updated
    promote last_action             — this-tick action becomes next-tick last_action
    increment tick; update peaks    — PopulationMilestone events fire here
```

## Web layer (`web/src/`)

**render.ts** — Pure drawing helpers. `renderWorld()` is called once per RAF frame with fresh wasm-sourced buffers. `renderCreaturePortrait()` draws a single creature for the eulogy card. `RING_COLORS`, `PX_PER_SIZE`, camera transform, and `screenToWorld` conversion live here.

**camera.ts** — Pan and zoom controls (pointer drag, wheel, pinch). Attaches to the canvas element; mutates a `Camera` object passed in from main.ts. Clamping per v6 §N is in `render.ts::clampCamera`.

**main.ts** — Entry point. Inits wasm, creates `WorldHandle`, wires up the RAF loop, connects persistence and rail, exposes `world` on `window` for console access (sliders, debugging). Handles the resume-from-save flow.

**rail/** — Right-side panel: events list (`events.ts`), species list (`events.ts`), stats chart (`stats.ts`), inspector panel (`inspector.ts`), toast notifications (`toast.ts`), speciation highlight ring (`highlight.ts`). Polled from main.ts each RAF frame via `pollRail()`.

**persistence/** — Save/load via IndexedDB. `PersistenceClient` (main-thread) posts JSON to the IDB worker (`worker.ts`) which does the actual write — no main-thread hitch. `ui.ts` shows the resume prompt and schema-mismatch modal. `toast.ts` shows autosave failure toasts.

**eulogy.ts** — Eulogy card shown when population reaches 0. Renders 4-image HoF grid (biggest, weirdest, last_survivor, first_mover), Copy Share Text button, Download Save button, New World button.

## Wasm API surface (`WorldHandle` methods)

| Method | Purpose |
|---|---|
| `new(seed)` | Create a new world; empty string picks a random seed |
| `step()` / `step_n(n)` | Advance one or N ticks; returns false if world ended |
| `tick()` | Current tick count |
| `seed()` | Seed string for this world |
| `population()` / `species_count()` | Live counts |
| `world_ended()` | True after population hits 0 |
| `world_size()` / `sun_dim()` | World and sun grid dimensions |
| `creatures_buffer()` | Float32Array SoA snapshot for renderer |
| `sun_buffer()` / `sun_capacity_buffer()` | Sun current/capacity grids |
| `carrion_buffer()` | Carrion positions + pool sizes |
| `set_slider(name, value)` | Adjust a dev slider live (e.g. `"base_sun_rate"`) |
| `recent_events_json()` | JSON array of last N events for rail |
| `events_total_count()` | Total event count (for dedup) |
| `species_list_json()` | JSON array of live species for rail |
| `creature_ids_buffer()` | Float64Array of creature IDs (for inspector) |
| `creature_at(wx, wy)` | Index of creature at world coords (for click handler) |
| `creature_inspect_json(idx)` | Full JSON of one creature for inspector panel |
| `stats_sample()` | Box<[f32]> stats snapshot for stats chart |
| `snapshot_json()` / `from_json(json)` | Save / restore world state |
| `hof_json()` | Hall-of-fame JSON for eulogy card |
| `creature_stride()` (free fn) | Number of floats per creature in creatures_buffer |

## Magic numbers and spec references

Every constant in [../src/constants.rs](../src/constants.rs) carries a comment citing the v5/v6 section that defines it. To find where any given value comes from: grep `constants.rs` for the section number (e.g. `§7` for energy economy) or the constant name, then cross-reference [archive/PITCH-v5.md](archive/PITCH-v5.md) or [archive/PITCH-v6.md](archive/PITCH-v6.md).

## Determinism

A world is seeded once: the seed string is hashed with xxHash64 into a `u64`, which seeds `xoshiro256++`. The single `SimRng` is threaded through every tick function in a fixed order (steps 1-12 above). No other source of randomness exists. Given the same seed string, `step()` always produces identical state at every tick. The parallel rayon path (behind the `threads` feature, default off) uses a different sub-RNG scheme and produces different but still deterministic output; enabling it requires re-bootstrapping `tests/golden_snapshot_t10000.txt`.
