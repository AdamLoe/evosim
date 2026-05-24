# evosim — architecture audit

Snapshot: post-perf-5, pre-UI-rewrite. ~6.5k LOC Rust + ~1.8k LOC TS. Single
crate at root, plain-TS Vite shell, wasm-pack bridge through `src/wasm_api.rs`.

Reading order before patching: `Cargo.toml`, `src/lib.rs`, `src/world.rs`
(2 291 LOC — the elephant), `src/wasm_api.rs`, `src/creature.rs`,
`src/save.rs`, `src/snapshot_hash.rs`, then the TS shell. Cross-reference
with `docs/plans/perf+ui-master.md` for in-flight churn.

The codebase is in remarkably good shape for a 6-hour build — the only
"big ball of mud" symptom is `world.rs` and even that is mostly
disciplined. Concrete proposals follow, ranked by effort × payoff.

---

## A. High-leverage refactors (recommended before more features)

### A1. Split `src/world.rs` (2 291 LOC) into a tick-pipeline module

**Rationale.** `world.rs` owns the `World` struct, all 11 named tick phases,
the NN dispatcher, the threaded NN block, action-decode, save/load
reconstruction, plus ~700 LOC of tests. Every new perf-N plan amends this
one file; every UI/save change risks brushing the wrong region. Cross-review
§D shows 4 of 5 perf pieces collide here, and master plan §6 R7 had to add a
debug invariant just to police hot-mirror sync. The file is the single
biggest blocker to parallel work.

**Concrete shape.**
- `src/world/mod.rs` — `World` struct, `new`, `step`, `tick_once`,
  `finalize_extinctions`, `population`, slider plumbing. ~350 LOC.
- `src/world/tick.rs` — the 11 step phases (`apply_movement_and_repulsion`,
  `photosynth_two_pass`, `eat_and_scavenge`, `energy_bookkeeping`,
  `collect_deaths`, `decay_carrion`, `handle_births`, `run_vision_pass`).
  ~1 200 LOC. Each phase is a free function `pub(super) fn
  apply_movement_and_repulsion(w: &mut World)` or methods on a
  `TickCtx<'_>`. This is where every perf-N plan lands.
- `src/world/nn.rs` — `nn_forward_all_chunks`, `chunk_ranges`,
  `pick_action_d`, `build_nn_input`, `is_valid_action`, `decode_action`,
  `count_carrion_overlap`, `compute_is_at_wall`. ~250 LOC. Has its own
  `#[cfg(feature = "threads")]` block instead of burying it inside the
  monolith.
- `src/world/save_v1.rs` — `from_save_v1` (currently 120 LOC inside
  `world.rs`). Co-located with `src/save.rs` would be even better, but
  the comment at line 1054 explains why it lives in world.rs: needs
  private SoA. Solution: move it to `world/save_v1.rs` and make
  `SaveV1` types `pub(crate)` so `from_save_v1` can read them.
- `tests/world_tick.rs` or `src/world/tests.rs` — the 30+ tests that
  currently bloat `world.rs` to 2 291 LOC.

**Effort.** Half a day, mostly mechanical. Zero behavior change. Golden
hash unchanged. One review checkpoint at the seam. Acceptance tests catch
any miswiring.

---

### A2. Replace the `events_enabled: bool` toggle with a no-op `EventLog`

**Rationale.** Per F.31 DECISIONS ("Events disable"), every `events.push`
call site is wrapped in `if self.events_enabled { … }` — there are 9 such
guards across `world.rs` plus 4 test sites that flip the flag on. This is
a control-coupling smell: callers must know whether the log is "really" on.
The state lives on `World` and serializes to `SaveV1` for no reason.

**Concrete shape.** Move the flag inside `EventLog`. `EventLog::push`
becomes a no-op when disabled. Construct `EventLog::enabled()` /
`EventLog::disabled()`; default to disabled. Delete `World.events_enabled`
and the 9 guards.

```rust
pub fn push(&mut self, ev: Event) {
    if !self.enabled { return; }
    // ... existing logic
}
```

**Effort.** 30 min. Removes 13 conditionals. Re-enabling events in v1.1
becomes a one-line `EventLog::set_enabled(true)` in `World::new` (or a
slider on `DevSliders`).

---

### A3. Push 9 `scratch_*` per-tick fields off `World` into a `Scratch` struct

**Rationale.** `World` now has 30+ fields, of which 9 are pure per-tick
allocator-suppression buffers (`scratch_fx`, `scratch_fy`,
`scratch_neighbors`, `scratch_damage`, `scratch_gain`,
`scratch_cooldown_set`, `scratch_attempted_eat`,
`scratch_attempted_scavenge`, `scratch_got_a_bite`). They are explicitly
excluded from save and hash via "don't mention them in `SaveV1`" — a
pattern that scales poorly. `World::new` and `from_save_v1` each carry
9 lines of `Vec::new()` initializers that the save round-trip must
duplicate (today they are correct only because all 9 are `Vec::new()`).

**Concrete shape.**
```rust
#[derive(Default)]
pub struct TickScratch {
    pub fx: Vec<f32>,
    pub fy: Vec<f32>,
    pub neighbors: Vec<usize>,
    pub damage: Vec<f32>,
    pub gain: Vec<f32>,
    pub cooldown_set: Vec<bool>,
    pub attempted_eat: Vec<bool>,
    pub attempted_scavenge: Vec<bool>,
    pub got_a_bite: Vec<bool>,
}
pub struct World {
    // ...
    pub(crate) scratch: TickScratch,
}
```

Save/hash exclusion becomes self-documenting (`TickScratch` has no
`Serialize` impl). `mem::take` dance for the neighbors borrow inside
the closure becomes `mem::take(&mut self.scratch.neighbors)` — same
pattern, less noise. **Bonus**: pairs naturally with A1 so each tick
phase takes `(&mut TickScratch, &mut CreatureSoA, &SpatialGrid, …)`
arguments instead of `&mut self` on a fat object.

**Effort.** 1 hour. Compiles after the field rename + the two
initializer sites collapse from 9 lines each to `scratch:
TickScratch::default()`.

---

### A4. Genome SoA mirror has no enforced invariant — add `Genome` accessor wrapper

**Rationale.** Perf-5 introduced 7 mirror Vecs (`g_size`, `g_photo_eff`,
…) on `CreatureSoA` that MUST stay bit-identical to `genomes[i].field`.
The sync happens in three places (`push`, `remove_indices`, `from_save_v1`
via `push`) and the test `hot_mirrors_match_genomes_after_births_and_mutations`
is the safety net. But:
- `genomes` is `pub Vec<Genome>` (creature.rs:62). Any future code can
  do `creatures.genomes[i].size = 5.0` and silently desync.
- The `resync_hot_mirrors_at` helper is marked `#[allow(dead_code)]`
  with "used only in `#[cfg(test)]` blocks" — meaning it's manually
  invoked from tests but production code has no enforcement.

**Concrete shape.** Make `genomes` private. Expose:
- `fn genome(&self, i: usize) -> &Genome` — read-only read.
- `fn mutate_genome(&mut self, i: usize, f: impl FnOnce(&mut Genome))`
  — calls `resync_hot_mirrors_at(i)` after `f` returns. Single mutation
  pathway.

Production hot mutation site is `handle_births` which clones the parent
genome, mutates, then `creatures.push(...)`. `push` already syncs.
Snapshot hash, `creature_inspect_json`, save serialization all read
fields — they keep using `genome(i)`. The compiler enforces the
invariant.

**Effort.** 1–2 hours. ~20 call-site edits. Removes one whole class of
"forgot to sync" footguns and makes the perf-5 cross-review M1/M2 notes
unnecessary for future hot-field additions.

---

### A5. `creatures_buffer` packs flag bits per-frame — push to renderer

**Rationale.** `WorldHandle::creatures_buffer` (wasm_api.rs:103–133)
allocates a `Vec<f32>` of stride 13 per creature per frame and computes
`if g.eye_count > 0 { 1.0 } else { 0.0 }` × 5 flags per creature. The
sim does not need these flags. They're a render-layer decision (which
rings to draw) leaking into the wasm-JS marshalling boundary. The flag
math also lives on the hot per-creature genome read.

**Concrete shape.** Cut the stride to 8 (x, y, radius, r, g, b,
energy_frac, age_frac). The renderer (`web/src/render.ts`) already has
access to genome details via `creature_inspect_json` for the
selected-creature inspector; for ring flags, expose a separate
`creature_flags_buffer() -> Uint8Array` (one byte per creature, bits
0–4 = eye/move/scav/mouth/armor). The renderer reads it once per frame
into a typed-array view. Halves the per-frame marshalled bytes and
removes 5 conditionals from the hot path.

Alternatively (smaller change): compute flags once at birth and store
them as a `Vec<u8>` on `CreatureSoA`; `creatures_buffer` just memcpys
the row. Same renderer-side experience.

**Effort.** 1–2 hours including renderer-side wire-up. Touches the
`creature_stride()` contract; bump it to 8 and document in
`wasm_api.rs:425` comment block.

---

### A6. Two `BODY_RADIUS_PER_SIZE` constants is a half-resolved boundary

**Rationale.** `src/world.rs:24` declares
`pub const BODY_RADIUS_PER_SIZE: f32 = 1.0;` and
`src/vision.rs:340` re-exports it as
`pub const BODY_RADIUS_PER_SIZE: f32 = crate::world::BODY_RADIUS_PER_SIZE;`.
This is the only world-shape constant that doesn't live in
`constants.rs`. `wasm_api.rs` imports it from `world`; `vision.rs` has
its own copy. The vision re-export is dead code (no caller imports it
from vision).

**Concrete shape.** Move `BODY_RADIUS_PER_SIZE` to `constants.rs`
alongside `WORLD_SIZE`, `HASH_CELL`, etc. Delete the vision re-export.
The `world` module re-exports the constant for back-compat (one line).

**Effort.** 5 min. Pure cleanup.

---

### A7. Hardcoded `score`-style enum logic in `decode_action` should be data-driven

**Rationale.** `world.rs:1322` `decode_action` sorts six `Action` enum
variants by logit, falls through `is_valid_action` per variant. The
"`Rest` is always valid" comment at the bottom relies on the enum
declaration order matching the iteration order. The two functions read
disjoint subsets of `&Genome` — `is_valid_action` reads `eat_efficiency`,
`scavenge_efficiency`; nothing reads `size` or movement. Adding a 7th
action (a v1.1 idea per the build report) requires touching three
files: enum decl, `is_valid_action`, `Action::ALL`.

**Concrete shape.** Move validity to a method:
```rust
impl Action {
    fn is_valid(self, hot: HotContext) -> bool { ... }
}
```
where `HotContext` is a small bag (`eat_eff: f32, scav_eff: f32, energy:
f32, cooldown: u32`). `decode_action` calls `action.is_valid(ctx)`. Adds
a new action without touching `decode_action`.

**Effort.** 30 min. Minor; do it next time the enum grows.

---

## B. Boundary leaks (sim ↔ wasm_api ↔ web)

### B1. `WorldHandle::set_slider` is stringly-typed

**Rationale.** `wasm_api.rs:171` `set_slider(name: &str, value: f32)`
does a string match on `"base_sun_rate"`, etc., silently ignoring
unknown names. The 5 valid slider names are also pinned in
`constants.rs` + `DevSliders` struct + UI plans, so JS callers learn
the names via documentation.

**Concrete shape.** Either expose one method per slider
(`set_base_sun_rate(f32)`, etc. — wasm-bindgen handles this fine and
gives JS callers typed signatures), or expose an enum repr-u8 from
Rust + a `SliderId` JS type. Five sliders, five wrapper methods — that's
~20 LOC and removes the silent-ignore failure mode.

**Effort.** 20 min. The Build Report (item 4 known issue) confirms no
slider UI exists yet, so changing the API is cost-free.

---

### B2. `creature_at` is O(N) and returns an unstable SoA index

**Rationale.** `wasm_api.rs:226` linearly scans all creatures on every
click. Comment notes "called only on click (≤ 1 Hz)" — fine for now,
but the SoA index it returns becomes invalid the next tick after a
death/birth. The TS-side inspector hides this by re-resolving via
`creature_ids_buffer` every frame (refreshInspector at
`rail/inspector.ts:113–152`). That re-resolution is itself a linear
scan over the ids buffer **per frame** while the inspector is open.

**Concrete shape.**
- Make `creature_at` return `Option<u64>` (stable id) instead of
  `Option<u32>` (transient index). Adds 5 LOC: walk the matched cell
  + look up the id.
- TS-side: cache the (id → index) mapping when the ids buffer changes,
  not every frame. Or invert the iteration — pass the id to a new
  `creature_inspect_by_id(id: u64)` that does the lookup once on the
  Rust side.

This also unblocks the v1.1 "Lineage tree" idea (build report priority
4) — clicking a node on the tree wants a stable id, not a frame-bound
index.

**Effort.** 1 hour. Net code reduction on the TS side.

---

### B3. Per-call JSON for inspector + species list — pre-shaped types are stuck on string

**Rationale.** `creature_inspect_json` (wasm_api.rs:243) builds a
`serde_json::Value`, serializes it to a `String`, ships it to JS where
`JSON.parse` rebuilds it. 22 fields × every-frame while inspector is
open. `species_list_json` is similar (1 Hz; less hot). Both have a
typed `CreatureInspectJson` TS interface (`rail/inspector.ts:56–80`)
that mirrors the JSON shape — so the **types exist on both sides**, the
JSON is just a slow string codec.

**Concrete shape.** Two options:
1. Keep JSON but emit it more cheaply — manual string building avoids
   the `serde_json::Value` allocation. ~3× faster for ~30 LOC of
   uglier code. Probably not worth it.
2. Return a `Float64Array` / `Uint32Array` view with a fixed layout
   for the 12 numeric fields; ship the four strings
   (species_name, parent_species_name, current_action) via separate
   getters. The TS shape stays — `getInspectorView(idx)` returns a
   `{ scalars: Float64Array; names: { species, parent, action } }`.

Hold this off until profiling shows the inspector JSON path actually
hurts. The Build Report notes raycasts dominate today. **No action
unless the perf-timing report singles it out.** Flagging because the
moment `creature_inspect_json` is called every frame for multiple
creatures (lineage tree hover, multi-select v1.x) the codec cost
compounds.

**Effort if/when needed.** 2 hours.

---

### B4. `wasm_api.rs::species_list_json_inner` is a back-door for native tests

**Rationale.** `species_list_json` (wasm-only) calls
`species_list_json_inner` (impl block at wasm_api.rs:396) so that
native unit tests can call the inner. This works but is the only sim
function with this shape, and it hides a public surface area item
(the impl block is `impl WorldHandle` without `#[wasm_bindgen]`, which
is a real Rust public method that wasn't intentionally exposed).

**Concrete shape.** Move the species-list computation onto `World`
itself (`World::species_list_json` or
`World::live_species_rows() -> Vec<SpeciesRow>`). `WorldHandle`
delegates. Tests live in `world.rs::tests` where they have all the
private fields they need. Closes the back-door.

**Effort.** 20 min.

---

### B5. `web/src/main.ts` is a 339-line god-function

**Rationale.** `main.ts` does: wasm init, thread pool init, IDB
persistence client, resume-prompt flow, schema-mismatch flow, seed
display + clipboard, autosave debouncing, autosave wall + tick gates,
beforeunload save, render loop, RAF scheduling, speed-button DOM
construction, eulogy gating, world construction (two paths: fresh /
from-save), camera restoration, profiler attach, rail install, click
handler install. Adding a feature touches this file by default.

**Concrete shape.** Three free-standing TS modules + a slim main:
- `web/src/boot.ts` — `bootWorld()` returns `{ world, camera }` after
  resolving the resume / fresh-construction question.
- `web/src/autosave.ts` — owns `lastSavedTick`, `lastSavedWallMs`,
  exposes `maybeAutosave(world, cam, now, persistence)` (already a
  function — just move it).
- `web/src/loop.ts` — the RAF frame function, parameterized by the
  pieces above.

`main.ts` shrinks to ~80 LOC: wire everything, start RAF. Aligns with
the v1.1 plan of splitting `rail/stats.ts` into per-widget modules
(perf+ui-master §5 ui-perf).

**Effort.** 1–2 hours. Decoupled from any Rust change. The 339-line
file becomes navigable and the autosave heuristics get a clean home
when they next need tuning.

---

### B6. `events.ts` re-exports `EvKind` types after the event log was disabled

**Rationale.** `rail/index.ts:19–25` exports the `EvKind` discriminated
union and the `EvEvent` interface; nothing reads them anywhere in the
TS shell (the polling at line 112 was removed per F.31). Build Report
known issue #10 references "rail/events.ts" — that file still exists
(97 LOC) and is wired in. The Events panel section in `index.html` is
`display:none`. Dead code with a re-enablement comment, but the
re-enablement plan is loose.

**Concrete shape.** Either:
- Delete the events-rendering TS module + types until re-enabled.
  v1.1 reverts the events_enabled flag flip (DECISIONS.md says it's a
  one-liner) and re-adds the file from git history.
- OR pull the trigger and re-enable events now (re-flip
  `events_enabled` to `true` in `World::new`, un-hide the section in
  `index.html`).

Pick one. The current "kept around with display:none, polling removed,
types re-exported" middle ground is the worst of both — it costs
review attention every time someone reads `rail/index.ts` looking for
"how do events flow today".

**Effort.** 15 min for either direction.

---

### B7. `_start` panic hook is only `cfg(debug_assertions)`

**Rationale.** `src/lib.rs:36–41` installs a panic hook only in debug
builds. Release builds in the browser silently abort on panic (per
`panic = "abort"` in Cargo.toml profile.release). For a sim that runs
for hours unattended in idle play, a release panic shows up as the
canvas freezing with no diagnostic.

**Concrete shape.** Install the panic hook unconditionally; route to
`console::error_1` always. The hook itself is 3 lines; the JS console
already gets the error, but a panicked wasm gives a less useful stack
without the hook. Cost: zero perf impact.

**Effort.** 2 min.

---

## C. Module-level proposals

### C1. `src/hof.rs` is 17 LOC — merge into `species.rs` or `creature.rs`

**Rationale.** `HallOfFame` is a single struct + serde derive. It's
used by `World` (six slot fields), `save.rs`, `wasm_api.rs::hof_json`,
and tests. The module-level overhead (1 file in `lib.rs`, an extra
import per consumer) buys nothing.

**Concrete shape.** Move to `src/world.rs` near the six HoF fields it
shapes, or to `src/save.rs` since it serializes alongside `SaveV1`. My
vote: `src/save.rs` because that's where the cross-cutting "persist a
notable snapshot" concern lives.

**Effort.** 5 min. Pure cleanup.

---

### C2. `src/carrion.rs` is 15 LOC — same story

**Rationale.** Single struct, no logic. Decays/spawns lives in
`world.rs`.

**Concrete shape.** Move `struct Carrion` next to `World.carrion: Vec<Carrion>`
in `world.rs`. Or merge with `src/sun.rs` since they both interact via
`sun_cell` refund.

**Effort.** 5 min. Not load-bearing; keep both standalone if A1 happens
first (more files becomes a feature once `world/` is split).

---

### C3. `src/save.rs` and `World::from_save_v1` straddle the module boundary

**Rationale.** Save shape lives in `save.rs` but the reconstruction
lives in `world.rs:1047` because `from_save_v1` needs to fabricate
`World`'s private fields (cell_to_carrion, pending_extinction_check,
scratch_*). The header comment at the top of `save.rs` claims "From<&World>
conversions kept in this module; World re-exports via to_save_v1 /
from_save_v1 helpers" — but `to_save_v1` is in `save.rs`, while
`from_save_v1` is in `world.rs`. Asymmetric.

**Concrete shape.** Two paths:
- (A) Move `from_save_v1` to `save.rs`. Requires making the World's
  private fields constructible from inside `save.rs`. Easiest: add a
  `World::with_save_state(public_fields..., transient_defaults..) -> Self`
  constructor that hides the boilerplate.
- (B) Move `to_save_v1` to `world.rs`. Forces save.rs to be data-only.
  More consistent with the "world owns its own serialization" idea.

I lean (B). The save shape *types* stay in `save.rs`; the conversion
*logic* both directions sit in `world.rs` next to the source of truth.

**Effort.** 30 min.

---

### C4. `profiler.rs` is 971 LOC — half is tests + a fake clock; that's fine but rename

**Rationale.** Not a problem per se, but `src/profiler.rs` is the
second-largest file and contains:
- The Profiler core (~250 LOC)
- A platform-conditional clock helper (`clock_now_ms` / `clock_now_us`
  with native + wasm32 branches and a fake-clock test override)
- The `profile_span!` macro (defined elsewhere; this file consumes it)
- ~500 LOC of tests including overhead and timing assertions.

**Concrete shape.** Split tests into `src/profiler/tests.rs`. The fake
clock helpers (`set_fake_clock_us`, `clear_fake_clock`) are public —
gate them behind `#[cfg(any(test, feature = "test-clock"))]` so they
can't be called from non-test code outside the crate. Currently they
are `pub fn` reachable from anywhere.

**Effort.** 30 min.

---

### C5. `species.rs::trait_body_distance_sq` mutation rates are NOT in the metric

**Rationale.** `trait_body_distance_sq` (species.rs:134) computes the
body-distance via 13 trait terms. The mutation rates themselves
(14 floats per genome) are NOT in the distance, even though they drift
generation-over-generation and a lineage with hyper-mutating
`eye_offsets` is meaningfully different from a stable one. This is a
**spec call**, not a bug — v6 §H defines body distance as the listed
trait terms. Flagging because:
- Snapshot hash includes mutation_rates (snapshot_hash.rs:101–116).
  Save round-trip preserves them. Inspector doesn't show them.
- If you ever want a "trait drift dashboard", the data is there but
  no module exposes it.

**No action recommended.** Note for v1.1 trait-histogram work (build
report priority 6).

---

## D. Dead code + duplication

### D1. `BODY_RADIUS_PER_SIZE` re-export in vision.rs (D6 above already mentioned).

### D2. Inline `count_carrion_overlap` and `compute_is_at_wall` are duplicated in the threaded NN block.

`world.rs:1174` (sequential helper) and `world.rs:394–425` (inlined
threaded version) compute the same overlap count and wall flag.
The cfg-gated dead-code allow at line 1173 admits this. Cross-piece
review flagged it in perf-4 but didn't resolve.

**Concrete shape.** Hoist the inlined logic to two free functions that
take `&CreatureSoA, &[Vec<u32>], &[Carrion], i: usize` (no `&self`).
Both code paths call them. Removes the cfg-allow and the duplication.

**Effort.** 20 min.

### D3. `pick_action_c` reference in DECISIONS.md — dead, no code follow-up needed.

DECISIONS line 42 confirms it was deleted in milestone D. Build Report
known issue #6 asks for a spot-check; I grepped: no references remain.
**No action.**

### D4. `genome.rs` includes an inline `heapless` clone (lines 286–325)

A 40-LOC `mod heapless { ... }` defines a stack-vec used by exactly one
caller (eye-count adjacent candidates, 2-element max). The doc comment
above it says "heapless was overkill here. Use std Vec —" yet the code
still uses the in-house heapless type.

**Concrete shape.** Replace with `arrayvec::ArrayVec<u8, 2>` (already
unlikely to add a dep — just use `[Option<u8>; 2]` + a count, or two
`if let` branches). 40 LOC → 5 LOC.

**Effort.** 10 min.

### D5. `Action::one_hot_index` exists but is unused except in one site

`creature.rs:36` — only `build_nn_input` calls it. The conversion is
literally `self as usize`. Marginal; flag for delete when next
touching the file.

---

## E. Public API surface to tighten

### E1. `World` exposes nearly every field as `pub`

`pub tick`, `pub seed`, `pub rng`, `pub sun`, `pub grid`,
`pub creatures`, `pub carrion`, `pub species`, `pub events`,
`pub events_enabled`, `pub sliders`, `pub next_creature_id`,
`pub peak_*`, `pub world_ended`, `pub live_species_count`,
`pub first_move_fired`, `pub first_eat_fired`,
`pub population_milestones_fired`, six `pub` HoF slots, …
Only `cell_to_carrion` and `pending_extinction_check` are private.

This is fine for an in-tree crate but the surface IS effectively
public-API because `wasm_api.rs` reads through it (`self.inner.creatures.x[i]`,
`self.inner.species.list[…]`, etc.). Every per-tick optimization risks
breaking a wasm_api invariant.

**Concrete shape.** Replace direct field access in `wasm_api.rs` with
narrow accessor methods on `World`: `World::creature_count()`,
`World::creature_pos(i)`, `World::creature_radius(i)`, etc. Field
visibility drops to `pub(crate)`. The wasm-bindgen boundary becomes a
formal contract.

**Effort.** 1 hour. Pairs naturally with A1 (world/ split) since the
new tick modules need crate-private access anyway.

### E2. `CreatureSoA` has the same issue

15 `pub Vec<...>` fields. perf-5's hot mirror fields are correctly
`pub(crate)` — so the convention is already partially established.
Bring the rest down.

**Effort.** 20 min after E1.

---

## F. Determinism + threading concerns (review for v1.1 perf-N)

### F1. Threaded NN block re-implements logic that should be one function

Already covered by D2. The risk is bigger than dead-code: if `decode_action`
or `build_nn_input` ever read a new field from CreatureSoA, the threaded
inline at world.rs:394–445 must be updated or it'll diverge. The
**dual-golden protocol** (`DECISIONS.md` "Threads (perf-4)") will catch
that drift, but only at golden-test bootstrap time, not at code-review
time.

**Concrete shape.** Per D2: extract the inlined-overlap/wall computation
to free functions. Same fix.

### F2. `chunk_ranges` in world.rs vs `par_chunks_mut(chunk_size)` in vision.rs

The two partitioning schemes are claimed equivalent at `world.rs:1247`
("Invariant (perf-4)"). They use different code (`chunk_ranges` builds
an array of (lo, hi); `par_chunks_mut` slices). If `N_CHUNKS` ever
changes, both need updating. Currently coordinated only via a
doc-comment.

**Concrete shape.** Add a unit test that asserts the partition is
identical for `n ∈ {0, 1, 7, 8, 9, 100, 1500}`. ~20 LOC.

**Effort.** 15 min. Prevents a silent threaded-golden drift.

---

## G. Suggested ordering

If you take only one thing: **A1 (split `world.rs`)** unlocks every
in-flight perf and UI plan, removes the biggest merge-conflict surface,
and is mechanical.

Recommended sequence (each is independently shippable):

1. **A1** — split `world.rs` (half day). Land before any further
   perf-N work to make their diffs reviewable.
2. **A2** — `events_enabled` removal (30 min). Tiny, immediately
   visible cleanup.
3. **A3** — `TickScratch` struct (1 h). Pairs with A1.
4. **B7** — always-on panic hook (2 min). Free safety.
5. **D2 + F2** — extract carrion-overlap/wall helpers + add chunk
   partition unit test (35 min). Closes the threaded-golden silent-drift
   risk.
6. **A4** — encapsulate `genomes` behind accessor (1 h). Prevents the
   next hot-mirror desync bug.
7. **E1 + E2** — tighten field visibility (1.5 h). Best done with A1
   because the mod split forces the question anyway.
8. **B5** — split `main.ts` (1.5 h). Independent of any Rust change;
   parallelizable with perf work.
9. **A5** — drop ring flags from `creatures_buffer` (1–2 h). Frees per-frame
   bandwidth; coordinates with the renderer.
10. **B2** — stable-id `creature_at` (1 h). Removes a per-frame TS scan.

Skip-for-now: A7, B3, C5. Defer to v1.x when the feature pulls them in.
