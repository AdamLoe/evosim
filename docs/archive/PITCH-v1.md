# evosim — a living aquarium that discovers life

**Genre:** idle evolution sandbox / spectator sim
**Stack:** Rust → wasm, HTML5 canvas/WebGL frontend
**Vibe:** You open a tab. 8,000 colored cells drift in a 2D sea. Most are green (photosynth). Day 14, a red one eats a green one for the first time. Day 31, a fast one outruns the red ones. You didn't program any of that.

## Core loop (what the player sees)

- Live aquarium is the primary view. Pan, zoom, click a cell to see its genome + lineage.
- Every cell is a colored circle; size scales with body size; small feature-dots on the rim indicate which traits the genome has unlocked (eye dot, mouth dot, leg dot).
- A persistent **lineage tree** in a side panel grows in real time. Click any node to highlight that species in the world.
- A few god-buttons (post-v1): trigger drought, meteor, ice age.

## Core mechanic — *the trait economy*

Every creature has an energy budget per tick. Every trait costs energy.

- v0 genome: only `size=1, photosynth=on`. Cells passively gain energy from the sun, split when full.
- Mutation can flip ON: `eat`, `move`, `vision`, `armor`, `bigger size`, `faster speed`. Each has an upkeep cost.
- A predator trait is useless until vision+move also mutate in — so combos are what unlock new niches. The energy gradient discovers the food web.

## Brain

Fixed-topology tiny NN per creature. Inputs: own energy, nearby cell colors/sizes, signal levels, age. Outputs: action probs (rest, photosynth, eat, move dir, split, signal). Weights mutate alongside genome. **Brain learns *actions*; genome evolves the *body*.** Two clean axes of evolution to monitor.

## Reproduction

Asexual mitosis only in v1. Spend energy → spawn a child with mutated genome + jittered NN weights. Sexual reproduction stays as a future "evolved trait."

## History

Lineage tree is the persistent record. Every birth gets a parent pointer. Species are auto-detected by genome-distance clustering — when a cluster diverges enough, it gets a new color/name and a new branch on the tree.

## MVP scope (v1 build target)

- Phases 1–3 from `original_idea_docs/Ideal pace of game.md`: photosynth, mitosis, consume, move
- ~5–10k creatures in a 2D world with spatial hashing
- Fixed-topology NN with weight mutation
- Live aquarium + pan/zoom + click-to-inspect
- Always-on lineage tree

**Deferred to v1.1+:** seasons, biomes, god-button disasters, sexual reproduction, NEAT-style topology evolution, signaling.

## Why this fits a heavy-Claude-Code workflow

The hard parts — Rust ECS architecture, spatial hashing, wasm bridge, 8k-entity canvas/WebGL rendering at 60fps, lineage tree datastructure + viz, balancing the energy economy through fast iteration — are exactly the kind of focused, well-bounded engineering that Claude Code chews through fast. Constant numeric tuning (energy costs, mutation rates) is also more fun with an AI pair than alone.

## Open design questions (still being planned)

- World topology: toroidal wrap vs hard walls vs island
- Tick rate & time compression (real-time vs accelerated)
- Sensing model: how a fixed-topology NN handles a variable number of nearby creatures (ray-casting? top-K nearest? grid binning?)
- Species detection algorithm details (genome distance metric, threshold)
- Sun/energy regeneration model
- Mutation rate tuning and per-trait unlock cost curve
- Persistence: save/load worlds to disk?
- Performance budget breakdown
