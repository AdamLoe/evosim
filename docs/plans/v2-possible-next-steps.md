---
status: active
owner: mixed
last_updated: 2026-05-31
okay_to_delete: false
long_lived: true
owning_docs: []
---

# v2 — Possible Next Steps (permanent backlog)

## Long-lived purpose

A standing parking lot for v2-family ideas that are **not** in the
v2.0.0 plan: things we deliberately deferred, problems we expect to hit,
and design directions we discussed but haven't committed to. This file
is **never deleted** (`okay_to_delete: false`, but treat as permanent) —
when an item graduates into a real plan, leave a one-line "→ shipped in
vX.Y" pointer rather than removing it, so the reasoning survives.

This is not a spec. Entries can be half-baked. Capture the user's intent
and the open question, not a finished design.

Companion to [`v2.0.0-mission.md`](v2.0.0-mission.md) and
[`v2.0.0-decisions.md`](v2.0.0-decisions.md).

---

## Brain inheritance under sexual reproduction (long discussion, settled)

This section records a long design conversation so we don't relitigate it.
**Decision: keep both per-weight crossover modes — `fifty_fifty` and
`average` (a.k.a. `midpoint`) — applied to brain weights and genome
traits, same-species only. Drop the module / prototype-pull machinery we
explored. Keep crossover safe by keeping the species genetically tight,
not by adding structure.**

### The core problem (why every clever scheme felt like averaging)

Weight-space crossover of two trained nets is the classic **permutation /
"competing conventions"** failure: two networks can compute nearly the
same behavior with totally different internal wiring, so neuron 3 in A
plays the role neuron 7 plays in B. Mixing their weights splices
incompatible roles → a child worse than either parent. Crucially, NN
weights are **non-separable (highly epistatic)** — a weight only means
anything relative to the others — so there is no honest "seam" to cut.
That is why module schemes keep reducing to averaging.

### What we explored and rejected

- **Module-aware crossover (inherit whole functional blocks).** Rejected.
  Constraining a module's *inputs* doesn't pin its *outputs*, so the
  mismatch just relocates to the module→core boundary. The cheap
  *dense*-net version is worse — the "modules" are only labeled rows, not
  real circuits. (User: "neurons don't work alone, so this is the same
  problem as averaging.")
- **Prototype-pull (drag each child toward the species-mean brain).**
  Rejected. It's an artificial regularizer with no biological analog;
  nothing in real life computes a species centroid and pulls babies to it.
- **Per-module inspector provenance ("Mom's threat circuit").** Rejected
  as untruthful — with partial (first-layer-only) inheritance the behavior
  still lives in the whole net, so the provenance label lies.

### The realistic mechanism that *does* keep a species' brains together

Biological crossover works because genes sit at **fixed loci** and the two
parents carry interchangeable **alleles** of the same gene at the same
locus — picking Mom's-or-Dad's allele for gene 3 is safe because both fit
the same slot. NN weights have no stable locus **unless** the whole
population stays close to one common ancestor with only small drift. In
that **aligned regime**, every weight *is* a locus, the two parents'
values *are* interchangeable alleles, and plain per-weight crossover is
just Mendelian allele-shuffling — non-destructive. It only turns to mush
once lineages drift far enough to re-derive different internal
arrangements.

So the cohesion force is **not** a centroid attractor — it's **frequent
same-species recombination + small per-birth mutation** (a panmictic gene
pool stays coherent automatically; that's how real species stay species).
Concretely, keep brain crossover safe by:

- mating **same-species only** (load-bearing — prevents cross-basin mixing);
- **small per-birth brain mutation** (keep the pool tight);
- **frequent within-species mating** so recombination dominates drift.

No modules, no prototype-pull required. This is also simpler than anything
we sketched.

### Honest framing: crossover is a second-order lever for "smart"

The strongest methods for evolving capable neural controllers (ES,
CMA-ES, NES) are **mutation-only — no crossover**. The one crossover
method that works (NEAT) only does so by tracking gene ancestry to
*align* parents first. The "crossover is powerful" intuition comes from
**separable** GA problems (bitstrings), not epistatic NN weights. So in
evosim, crossover's real job is **narrative** (sexual reproduction,
species gene pools, faction storytelling), not raw optimization. At best
it's neutral-to-mildly-helpful in the aligned regime.

### What actually makes them smart (priority order)

1. **An environment that rewards intelligence with a gradient toward it.**
   ~80% of the outcome. If "drift to grass, eat, split" is an easy local
   optimum that survives fine, evolution stops there. Biomes, scarcity,
   predation, and mating-requires-clustering are good *because* each makes
   a smarter policy pay. The real question for "smart": does the fitness
   landscape have a climb that *requires* memory / anticipation /
   coordination?
2. **Effective population size × generations.** Big pools, fast sim, long
   runs. Note: splitting 8k across 10 species gives each only a few
   hundred individuals — a weak engine per species. For "smart," prefer
   **fewer, larger species** (or a bigger cap / a world that sustains big
   per-species pops). This trades against the "10 factions" visual — a
   real choice.
3. **Mutation regime.** The ES recipe is *largish* mutation + large
   population + strong selection — which is in tension with the
   small-mutation cohesion point above. That tension is itself a signal
   that **mutation-only may serve "smart" better than sexual repro**.
4. **Capacity and especially memory/recurrence.** The brain is a tiny
   *feedforward* net (32→48→24→5, ~2,800 weights) whose only "memory" is a
   few self-input slots (`prev_vx`, `last_action`, `ticks_since_split`).
   A reactive feedforward policy has a hard ceiling on "smart" — it can't
   remember where food was or that it's being chased. **Adding real
   recurrent hidden state carried tick-to-tick would unlock more
   intelligence than any crossover change** — the highest-ceiling lever
   after the environment.
5. **Crossover scheme.** Minor. Keep it simple and in the aligned regime.

### The fork to decide (open)

"Smartest agents" and "10 visible competing factions with sexual
reproduction" are **not the same optimization and they trade off.** One
clean resolution: make **single-pool mode the 'get smart' lab** (could
even go mutation-only ES, big population, long runs) and **species mode
the 'watch factions' story**. Not yet decided.

### Deferred research bets (only if the simple version disappoints)

- **Recurrence / memory** — biggest intelligence unlock; own milestone.
- **Indirect / generative encoding (HyperNEAT / CPPN family).** Genome is
  a compact *code* that *generates* the weights; recombining codes is
  gene-like and meaningful, and it enables true **dominant/recessive**
  (two gene copies + an expression rule). The user's "inheritable
  sequence that adds numbers to weights over a shared base" is a tractable
  form (shared base ⇒ cohesion by construction; mixing edit-lists ⇒ safe
  recombination). Biggest lift; uncertain raw-task payoff; do not gate
  v2.0 on it.
- **Mutation-only species mode** — drop crossover entirely if playtest
  shows it only hurts; species cohesion then comes from low mutation +
  shared founder alone.

---

## Mating cold-start levers (if seeding still collapses)

v2.0 already de-risks the mating bootstrap with: founders spawn adjacent,
founders get **biome-appropriate starting genomes** for their anchor's
biome, and **initiator-only** energy + cooldown (the partner is just a
target). If default runs still extinct-end before any learning:

- **8-direction `available_mate_proximity` NN input block** — per sector,
  the nearest off-cooldown same-species partner. Gives the brain a signal
  to steer toward mating opportunities instead of stumbling onto them.
  Costs 8 more inputs in species mode (re-derive the width table +
  `MAX_NN_INPUTS`). Strongest single lever if cold-start is the blocker.
- **Scale `starting_species_count` / `starting_species_member_count` very
  high** — denser founder clumps make contact-mating likely at gen 0.
  Costs more species colors, which is fine. Cheapest thing to try first.

**Watch item:** with initiator-only cooldown, a single popular partner can
be mated by many initiators in one tick (no partner cooldown gate).
If births spike unnaturally, add a per-tick "already mated as partner this
tick" guard or a light partner cooldown.

---

## Performance deep-dive at 1920² (deferred, profiler-gated)

v2.0 ships the straightforward implementation and **trusts the profiler**
to show where the time goes. Revisit when spans say so:

- **Per-tick whole-grid O(N) scans grow ~4×** (921k → 3.69M cells):
  `compute_propagation`, `rebuild_row_bitset`, and `SpatialGrid::rebuild`
  (`HASH_DIM²` start/cursor arrays). The row-empty grass-sector
  optimization also degrades when biome-seeded grass fills most rows.
- **Grass GPU upload.** v2.0 starts with **full-texture upload** at 1920²
  (~3.7 MB R8 + mipmap; measure before assuming it's a problem — it's 4×,
  not 64×). If it blows the RAF budget, go to a **tile / changed-cell
  bitset** sub-upload, not a single accumulated AABB (which degenerates to
  the whole map when quantized changes scatter across a biome).
- **`HASH_CELL` tuning** trades clump-query efficiency against per-tick
  rebuild/allocation cost — measure both, don't just match the grass cell.
- **Starburst early-bail rarely fires** at 64× sparser density / 16
  sectors, so creatures walk the full (shorter) starburst list each tick.
  Launch as-is; revisit if the proximity span dominates.

---

## Survey-scale faction visibility

Zoom-out + 1px species-colored points (v2.0) shows *where* factions are,
but not density/ownership cleanly. Deferred richer tools:

- Minimap.
- Species/population heatmaps and overlays.
- Per-region species-dominance shading.

---

## Inter-species richness

- **Per-other-species discrimination.** v2.0's 16-sector creature
  proximity collapses all rival species into one "other" channel — a
  creature can't tell species B from C. Per-species channels or a
  density-per-sector second block would unlock differential behavior
  ("flee the big predator, hunt the small grazer").
- **Dynamic species:** splits, merges, hybrid zones, interbreeding;
  populating the species-history breadcrumb; species respawn; a
  split-aware color generator ("one color becomes two similar-but-distinct
  hues").

---

## World & biome richness

- More biomes: forest, swamp, tundra, mountain — each needs trait axes
  v2.0 doesn't ship (cold tolerance, fertility, humidity, social/fear).
- Dynamic biome shifts; human-editable / paintable biome maps.
- Impassable terrain (would require collision + pathfinding — big).
- Linear vegetation suppression under high pop (only if the cap binds).

---

## Misc deferred

- Save/load; shareable world seeds.
- Event log, evolutionary timeline, major-event detection, eulogies.
- Photosynthesis / extra energy channels.
- Bases, nests, territory markers, signaling, civilization.
- Random species-name generator (v2.0 uses `Species-A..J`).

## See also

- [`v2.0.0-mission.md`](v2.0.0-mission.md)
- [`v2.0.0-decisions.md`](v2.0.0-decisions.md)
- [`index.md`](index.md)
